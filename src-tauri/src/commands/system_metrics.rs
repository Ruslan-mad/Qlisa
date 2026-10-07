//! Cached Windows system telemetry for the status bar.
//!
//! The Tauri command only reads a snapshot. A single worker samples while the
//! UI requests the data and releases its PDH/DXGI handles after a short idle
//! window. No sampling runs on the audio callback or renderer thread.

use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    sync::{Arc, OnceLock, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const SAMPLE_INTERVAL: Duration = Duration::from_secs(1);
const INTEREST_WINDOW_MS: u64 = 5_000;
#[cfg(windows)]
const ADAPTER_REFRESH_INTERVAL: Duration = Duration::from_secs(30);
#[cfg(windows)]
const PDH_RETRY_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SystemStatusSnapshot {
    /// Unix timestamp in milliseconds for this sample. Zero means no sample yet.
    pub timestamp_ms: u64,
    pub system_cpu_percent: Option<f64>,
    pub system_ram_used_bytes: Option<u64>,
    pub system_ram_total_bytes: Option<u64>,
    pub process_cpu_percent: Option<f64>,
    /// Process working set, which is the resident memory shown by Windows.
    pub process_ram_bytes: Option<u64>,
    pub gpu_adapters: Vec<GpuAdapterSnapshot>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GpuAdapterSnapshot {
    /// Stable adapter LUID, formatted as `luid:HIGH:LOW`.
    pub id: String,
    pub name: String,
    pub usage_percent: Option<f64>,
    pub dedicated_used_bytes: Option<u64>,
    pub dedicated_total_bytes: Option<u64>,
    pub video_decode_percent: Option<f64>,
}

impl Default for GpuAdapterSnapshot {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            usage_percent: None,
            dedicated_used_bytes: None,
            dedicated_total_bytes: None,
            video_decode_percent: None,
        }
    }
}

static SNAPSHOT: OnceLock<Arc<RwLock<SystemStatusSnapshot>>> = OnceLock::new();
#[cfg(windows)]
static WORKER: OnceLock<Option<std::thread::JoinHandle<()>>> = OnceLock::new();
#[cfg(windows)]
static INTEREST_UNTIL_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn snapshot_cache() -> Arc<RwLock<SystemStatusSnapshot>> {
    Arc::clone(SNAPSHOT.get_or_init(|| Arc::new(RwLock::new(SystemStatusSnapshot::default()))))
}

/// Return the latest cached machine and Qlisa resource sample.
///
/// On Windows this also extends a short sampling-interest window. Sampling is
/// performed by one background worker at 1 Hz; this command never waits for a
/// system query. Other platforms return unavailable metrics.
#[tauri::command]
pub fn get_system_status() -> SystemStatusSnapshot {
    #[cfg(windows)]
    {
        let until = now_ms().saturating_add(INTEREST_WINDOW_MS);
        INTEREST_UNTIL_MS.fetch_max(until, std::sync::atomic::Ordering::Relaxed);
        let cache = snapshot_cache();
        let _ = WORKER.get_or_init(|| {
            std::thread::Builder::new()
                .name("qlisa-system-metrics".into())
                .spawn(move || sampling_worker(cache))
                .map_err(|error| log::warn!("Could not start system metrics worker: {error}"))
                .ok()
        });
        return snapshot_cache()
            .read()
            .map(|value| value.clone())
            .unwrap_or_default();
    }
    #[cfg(not(windows))]
    {
        SystemStatusSnapshot::default()
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

#[cfg(windows)]
fn sampling_worker(cache: Arc<RwLock<SystemStatusSnapshot>>) {
    let mut sampler: Option<windows::WindowsSampler> = None;
    loop {
        let interested = now_ms() < INTEREST_UNTIL_MS.load(std::sync::atomic::Ordering::Relaxed);
        if interested {
            let sampler = sampler.get_or_insert_with(windows::WindowsSampler::new);
            let snapshot = sampler.sample();
            if let Ok(mut current) = cache.write() {
                *current = snapshot;
            }
        } else {
            // Dropping the sampler closes the PDH query and unloads its helper
            // libraries. The worker itself remains a single sleeping thread.
            sampler = None;
        }
        std::thread::sleep(SAMPLE_INTERVAL);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SystemCpuTimes {
    idle: u64,
    kernel: u64,
    user: u64,
}

fn calculate_system_cpu_percent(
    previous: Option<SystemCpuTimes>,
    current: SystemCpuTimes,
) -> Option<f64> {
    let previous = previous?;
    let idle = current.idle.checked_sub(previous.idle)?;
    let kernel = current.kernel.checked_sub(previous.kernel)?;
    let user = current.user.checked_sub(previous.user)?;
    // GetSystemTimes reports idle time as part of kernel time.
    let total = kernel.checked_add(user)?;
    if total == 0 || idle > total {
        return None;
    }
    Some(((total - idle) as f64 / total as f64 * 100.0).clamp(0.0, 100.0))
}

fn calculate_process_cpu_percent(
    previous_process_time_100ns: u64,
    current_process_time_100ns: u64,
    elapsed: Duration,
    logical_processors: usize,
) -> Option<f64> {
    if logical_processors == 0 || elapsed.is_zero() {
        return None;
    }
    let process_delta = current_process_time_100ns.checked_sub(previous_process_time_100ns)?;
    let wall_seconds = elapsed.as_secs_f64();
    if wall_seconds <= f64::EPSILON {
        return None;
    }
    let process_seconds = process_delta as f64 / 10_000_000.0;
    Some((process_seconds / wall_seconds / logical_processors as f64 * 100.0).clamp(0.0, 100.0))
}

#[derive(Clone, Copy, Debug, Default)]
struct GpuEngineAggregate {
    utilization: f64,
    is_video_decode: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct GpuUsage {
    usage_percent: Option<f64>,
    video_decode_percent: Option<f64>,
}

fn valid_counter_value(status: u32, value: f64) -> Option<f64> {
    // PDH_CSTATUS_VALID_DATA and PDH_CSTATUS_NEW_DATA are the only usable
    // statuses. Missing counters and stale PDH values stay unavailable.
    if (status == 0 || status == 1) && value.is_finite() && value >= 0.0 {
        Some(value)
    } else {
        None
    }
}

fn parse_luid_from_instance(instance: &str) -> Option<String> {
    let lower = instance.to_ascii_lowercase();
    let start = lower.find("luid_")? + "luid_".len();
    let remainder = &lower[start..];
    let mut parts = remainder.split('_');
    let high = parse_hex_part(parts.next()?)?;
    let low = parse_hex_part(parts.next()?)?;
    Some(format_luid(high, low))
}

fn parse_hex_part(value: &str) -> Option<u32> {
    let value = value.strip_prefix("0x").unwrap_or(value);
    u32::from_str_radix(value, 16).ok()
}

fn format_luid(high: u32, low: u32) -> String {
    format!("luid:{high:08x}:{low:08x}")
}

fn aggregate_gpu_engine_samples(samples: &[(String, Option<f64>)]) -> HashMap<String, GpuUsage> {
    let mut engines: HashMap<(String, u32, u32), GpuEngineAggregate> = HashMap::new();
    for (instance, value) in samples {
        let Some(value) = *value else {
            continue;
        };
        let Some(adapter_id) = parse_luid_from_instance(instance) else {
            continue;
        };
        let Some(physical_engine) = parse_instance_number(instance, "phys_") else {
            continue;
        };
        let Some(engine) = parse_instance_number(instance, "eng_") else {
            continue;
        };
        let lower = instance.to_ascii_lowercase();
        let is_video_decode =
            lower.contains("engtype_videodecode") || lower.contains("engtype_video_decode");
        let aggregate = engines
            .entry((adapter_id, physical_engine, engine))
            .or_default();
        aggregate.utilization += value;
        aggregate.is_video_decode |= is_video_decode;
    }

    let mut result = HashMap::new();
    for ((adapter_id, _, _), engine) in engines {
        let usage = result.entry(adapter_id).or_insert_with(GpuUsage::default);
        let percent = engine.utilization.clamp(0.0, 100.0);
        usage.usage_percent = Some(usage.usage_percent.unwrap_or(0.0).max(percent));
        if engine.is_video_decode {
            usage.video_decode_percent =
                Some(usage.video_decode_percent.unwrap_or(0.0).max(percent));
        }
    }
    result
}

fn parse_instance_number(instance: &str, marker: &str) -> Option<u32> {
    let lower = instance.to_ascii_lowercase();
    let start = lower.find(marker)? + marker.len();
    let digits: String = lower[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if digits.is_empty() {
        None
    } else {
        digits.parse().ok()
    }
}

fn aggregate_gpu_memory_samples(samples: &[(String, Option<f64>)]) -> HashMap<String, u64> {
    let mut result = HashMap::new();
    for (instance, value) in samples {
        let Some(value) = *value else {
            continue;
        };
        let Some(adapter_id) = parse_luid_from_instance(instance) else {
            continue;
        };
        let Some(bytes) = value
            .is_finite()
            .then_some(value)
            .filter(|value| *value >= 0.0)
        else {
            continue;
        };
        let bytes = bytes.min(u64::MAX as f64) as u64;
        result
            .entry(adapter_id)
            .and_modify(|total: &mut u64| *total = total.saturating_add(bytes))
            .or_insert(bytes);
    }
    result
}

#[cfg(windows)]
mod windows {
    use super::{
        aggregate_gpu_engine_samples, aggregate_gpu_memory_samples, calculate_process_cpu_percent,
        calculate_system_cpu_percent, format_luid, now_ms, valid_counter_value, GpuAdapterSnapshot,
        SystemCpuTimes, SystemStatusSnapshot,
    };
    use libloading::Library;
    use std::{collections::HashMap, ffi::c_void, mem, time::Instant};

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct FileTime {
        low: u32,
        high: u32,
    }

    impl FileTime {
        fn as_100ns(self) -> u64 {
            (u64::from(self.high) << 32) | u64::from(self.low)
        }
    }

    #[repr(C)]
    struct MemoryStatusEx {
        length: u32,
        memory_load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }

    #[repr(C)]
    struct ProcessMemoryCountersEx {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
        private_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> *mut c_void;
        fn GetActiveProcessorCount(group_number: u16) -> u32;
        fn GetSystemTimes(idle: *mut FileTime, kernel: *mut FileTime, user: *mut FileTime) -> i32;
        fn GetProcessTimes(
            process: *mut c_void,
            creation: *mut FileTime,
            exit: *mut FileTime,
            kernel: *mut FileTime,
            user: *mut FileTime,
        ) -> i32;
        fn GlobalMemoryStatusEx(status: *mut MemoryStatusEx) -> i32;
    }

    #[link(name = "Psapi")]
    unsafe extern "system" {
        fn GetProcessMemoryInfo(
            process: *mut c_void,
            counters: *mut ProcessMemoryCountersEx,
            size: u32,
        ) -> i32;
    }

    #[derive(Clone, Copy)]
    struct ProcessCpuSample {
        time_100ns: u64,
        wall: Instant,
    }

    pub(super) struct WindowsSampler {
        previous_system: Option<SystemCpuTimes>,
        previous_process: Option<ProcessCpuSample>,
        adapters: Vec<GpuAdapterSnapshot>,
        adapters_refreshed_at: Instant,
        pdh: Option<PdhSampler>,
        pdh_retry_at: Instant,
    }

    impl WindowsSampler {
        pub(super) fn new() -> Self {
            let now = Instant::now();
            Self {
                previous_system: None,
                previous_process: None,
                adapters: enumerate_dxgi_adapters(),
                adapters_refreshed_at: now,
                pdh: PdhSampler::new().ok(),
                pdh_retry_at: now + super::PDH_RETRY_INTERVAL,
            }
        }

        pub(super) fn sample(&mut self) -> SystemStatusSnapshot {
            if self.adapters_refreshed_at.elapsed() >= super::ADAPTER_REFRESH_INTERVAL {
                self.adapters = enumerate_dxgi_adapters();
                self.adapters_refreshed_at = Instant::now();
            }
            if self.pdh.is_none() && Instant::now() >= self.pdh_retry_at {
                self.pdh = PdhSampler::new().ok();
                self.pdh_retry_at = Instant::now() + super::PDH_RETRY_INTERVAL;
            }
            let mut snapshot = SystemStatusSnapshot {
                timestamp_ms: now_ms(),
                ..Default::default()
            };
            let system_times = unsafe {
                let (mut idle, mut kernel, mut user) = (
                    FileTime::default(),
                    FileTime::default(),
                    FileTime::default(),
                );
                (GetSystemTimes(&mut idle, &mut kernel, &mut user) != 0).then_some(SystemCpuTimes {
                    idle: idle.as_100ns(),
                    kernel: kernel.as_100ns(),
                    user: user.as_100ns(),
                })
            };
            if let Some(current) = system_times {
                snapshot.system_cpu_percent =
                    calculate_system_cpu_percent(self.previous_system, current);
                self.previous_system = Some(current);
            }

            let current_process = unsafe {
                let (mut created, mut exited, mut kernel, mut user) = (
                    FileTime::default(),
                    FileTime::default(),
                    FileTime::default(),
                    FileTime::default(),
                );
                (GetProcessTimes(
                    GetCurrentProcess(),
                    &mut created,
                    &mut exited,
                    &mut kernel,
                    &mut user,
                ) != 0)
                    .then_some(ProcessCpuSample {
                        time_100ns: kernel.as_100ns().saturating_add(user.as_100ns()),
                        wall: Instant::now(),
                    })
            };
            if let Some(current) = current_process {
                if let Some(previous) = self.previous_process {
                    let processors = unsafe { GetActiveProcessorCount(0xffff) } as usize;
                    snapshot.process_cpu_percent = calculate_process_cpu_percent(
                        previous.time_100ns,
                        current.time_100ns,
                        current.wall.duration_since(previous.wall),
                        processors,
                    );
                }
                self.previous_process = Some(current);
            }

            unsafe {
                let mut memory = MemoryStatusEx {
                    length: mem::size_of::<MemoryStatusEx>() as u32,
                    memory_load: 0,
                    total_phys: 0,
                    avail_phys: 0,
                    total_page_file: 0,
                    avail_page_file: 0,
                    total_virtual: 0,
                    avail_virtual: 0,
                    avail_extended_virtual: 0,
                };
                if GlobalMemoryStatusEx(&mut memory) != 0 {
                    snapshot.system_ram_total_bytes = Some(memory.total_phys);
                    snapshot.system_ram_used_bytes =
                        memory.total_phys.checked_sub(memory.avail_phys);
                }

                let mut process_memory = ProcessMemoryCountersEx {
                    cb: mem::size_of::<ProcessMemoryCountersEx>() as u32,
                    page_fault_count: 0,
                    peak_working_set_size: 0,
                    working_set_size: 0,
                    quota_peak_paged_pool_usage: 0,
                    quota_paged_pool_usage: 0,
                    quota_peak_non_paged_pool_usage: 0,
                    quota_non_paged_pool_usage: 0,
                    pagefile_usage: 0,
                    peak_pagefile_usage: 0,
                    private_usage: 0,
                };
                if GetProcessMemoryInfo(
                    GetCurrentProcess(),
                    &mut process_memory,
                    mem::size_of::<ProcessMemoryCountersEx>() as u32,
                ) != 0
                {
                    snapshot.process_ram_bytes = Some(process_memory.working_set_size as u64);
                }
            }

            snapshot.gpu_adapters = self.adapters.clone();
            if let Some(pdh) = self.pdh.as_mut() {
                if let Some((engine_samples, memory_samples)) = pdh.sample() {
                    let usage = aggregate_gpu_engine_samples(&engine_samples);
                    let memory = aggregate_gpu_memory_samples(&memory_samples);
                    let mut by_id: HashMap<String, GpuAdapterSnapshot> = snapshot
                        .gpu_adapters
                        .drain(..)
                        .map(|adapter| (adapter.id.clone(), adapter))
                        .collect();
                    for (id, gpu_usage) in usage {
                        let adapter =
                            by_id
                                .entry(id.clone())
                                .or_insert_with(|| GpuAdapterSnapshot {
                                    id: id.clone(),
                                    name: format!("GPU {}", id.trim_start_matches("luid:")),
                                    ..Default::default()
                                });
                        adapter.usage_percent = gpu_usage.usage_percent;
                        adapter.video_decode_percent = gpu_usage.video_decode_percent;
                    }
                    for (id, bytes) in memory {
                        let adapter =
                            by_id
                                .entry(id.clone())
                                .or_insert_with(|| GpuAdapterSnapshot {
                                    id: id.clone(),
                                    name: format!("GPU {}", id.trim_start_matches("luid:")),
                                    ..Default::default()
                                });
                        adapter.dedicated_used_bytes = Some(bytes);
                    }
                    snapshot.gpu_adapters = by_id.into_values().collect();
                    snapshot.gpu_adapters.sort_by(|a, b| a.id.cmp(&b.id));
                }
            }
            snapshot
        }
    }

    type PdhQuery = *mut c_void;
    type PdhCounter = *mut c_void;
    type OpenQueryFn = unsafe extern "system" fn(*const u16, usize, *mut PdhQuery) -> u32;
    type AddEnglishCounterFn =
        unsafe extern "system" fn(PdhQuery, *const u16, usize, *mut PdhCounter) -> u32;
    type CollectQueryFn = unsafe extern "system" fn(PdhQuery) -> u32;
    type CloseQueryFn = unsafe extern "system" fn(PdhQuery) -> u32;
    type GetFormattedArrayFn = unsafe extern "system" fn(
        PdhCounter,
        u32,
        *mut u32,
        *mut u32,
        *mut PdhFormattedCounterValueItem,
    ) -> u32;

    #[repr(C)]
    #[derive(Clone, Copy)]
    union PdhFormattedValue {
        long_value: i32,
        double_value: f64,
        large_value: i64,
        ansi_string: *const i8,
        wide_string: *const u16,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct PdhFormattedCounterValue {
        status: u32,
        value: PdhFormattedValue,
    }

    #[repr(C)]
    struct PdhFormattedCounterValueItem {
        name: *const u16,
        value: PdhFormattedCounterValue,
    }

    struct PdhSampler {
        _library: Library,
        query: PdhQuery,
        collect: CollectQueryFn,
        close: CloseQueryFn,
        get_array: GetFormattedArrayFn,
        engine_counter: Option<PdhCounter>,
        memory_counter: Option<PdhCounter>,
    }

    impl PdhSampler {
        fn new() -> anyhow::Result<Self> {
            unsafe {
                let library: Library = libloading::os::windows::Library::load_with_flags(
                    "pdh.dll",
                    libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32,
                )?
                .into();
                let open: OpenQueryFn = *library.get(b"PdhOpenQueryW\0")?;
                let add: AddEnglishCounterFn = *library.get(b"PdhAddEnglishCounterW\0")?;
                let collect: CollectQueryFn = *library.get(b"PdhCollectQueryData\0")?;
                let close: CloseQueryFn = *library.get(b"PdhCloseQuery\0")?;
                let get_array: GetFormattedArrayFn =
                    *library.get(b"PdhGetFormattedCounterArrayW\0")?;
                let mut query = std::ptr::null_mut();
                if open(std::ptr::null(), 0, &mut query) != 0 || query.is_null() {
                    anyhow::bail!("PdhOpenQueryW failed");
                }
                let mut add_counter = |path: &str| {
                    let path: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
                    let mut counter = std::ptr::null_mut();
                    (add(query, path.as_ptr(), 0, &mut counter) == 0 && !counter.is_null())
                        .then_some(counter)
                };
                let engine_counter = add_counter(r"\GPU Engine(*)\Utilization Percentage");
                let memory_counter = add_counter(r"\GPU Adapter Memory(*)\Dedicated Usage");
                Ok(Self {
                    _library: library,
                    query,
                    collect,
                    close,
                    get_array,
                    engine_counter,
                    memory_counter,
                })
            }
        }

        fn sample(&mut self) -> Option<(Vec<(String, Option<f64>)>, Vec<(String, Option<f64>)>)> {
            if self.engine_counter.is_none() && self.memory_counter.is_none() {
                return None;
            }
            if unsafe { (self.collect)(self.query) } != 0 {
                return None;
            }
            let engines = self
                .engine_counter
                .map(|counter| self.read_array(counter))
                .unwrap_or_default();
            let memory = self
                .memory_counter
                .map(|counter| self.read_array(counter))
                .unwrap_or_default();
            Some((engines, memory))
        }

        fn read_array(&self, counter: PdhCounter) -> Vec<(String, Option<f64>)> {
            const PDH_MORE_DATA: u32 = 0x8000_07D2;
            const PDH_FMT_DOUBLE: u32 = 0x0000_0200;
            const MAX_PDH_BUFFER_BYTES: u32 = 8 * 1024 * 1024;
            unsafe {
                let (mut byte_count, mut item_count) = (0u32, 0u32);
                let status = (self.get_array)(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut byte_count,
                    &mut item_count,
                    std::ptr::null_mut(),
                );
                if status != PDH_MORE_DATA || byte_count == 0 || byte_count > MAX_PDH_BUFFER_BYTES {
                    return Vec::new();
                }
                let capacity_bytes = (byte_count as usize + 7) & !7;
                let mut storage = vec![0u64; capacity_bytes / 8];
                let status = (self.get_array)(
                    counter,
                    PDH_FMT_DOUBLE,
                    &mut byte_count,
                    &mut item_count,
                    storage.as_mut_ptr() as *mut PdhFormattedCounterValueItem,
                );
                if status != 0
                    || item_count == 0
                    || byte_count as usize > capacity_bytes
                    || item_count as usize
                        > (byte_count as usize) / mem::size_of::<PdhFormattedCounterValueItem>()
                {
                    return Vec::new();
                }
                let base = storage.as_ptr() as usize;
                let end = base.saturating_add(byte_count as usize);
                let items = std::slice::from_raw_parts(
                    storage.as_ptr() as *const PdhFormattedCounterValueItem,
                    item_count as usize,
                );
                items
                    .iter()
                    .filter_map(|item| {
                        if item.name.is_null() {
                            return None;
                        }
                        let name_address = item.name as usize;
                        if name_address < base
                            || name_address >= end
                            || (name_address - base) % mem::align_of::<u16>() != 0
                        {
                            return None;
                        }
                        let max_units = (end - name_address) / mem::size_of::<u16>();
                        let mut len = 0usize;
                        while len < max_units && *item.name.add(len) != 0 {
                            len += 1;
                        }
                        if len == max_units {
                            return None;
                        }
                        let name =
                            String::from_utf16_lossy(std::slice::from_raw_parts(item.name, len));
                        let value =
                            valid_counter_value(item.value.status, item.value.value.double_value);
                        Some((name, value))
                    })
                    .collect()
            }
        }
    }

    impl Drop for PdhSampler {
        fn drop(&mut self) {
            if !self.query.is_null() {
                unsafe {
                    (self.close)(self.query);
                }
            }
        }
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Guid {
        data1: u32,
        data2: u16,
        data3: u16,
        data4: [u8; 8],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Luid {
        low_part: u32,
        high_part: i32,
    }

    #[repr(C)]
    struct DxgiAdapterDesc1 {
        description: [u16; 128],
        vendor_id: u32,
        device_id: u32,
        subsystem_id: u32,
        revision: u32,
        dedicated_video_memory: usize,
        dedicated_system_memory: usize,
        shared_system_memory: usize,
        adapter_luid: Luid,
        flags: u32,
    }

    type CreateFactoryFn = unsafe extern "system" fn(*const Guid, *mut *mut c_void) -> i32;
    type EnumAdapters1Fn = unsafe extern "system" fn(*mut c_void, u32, *mut *mut c_void) -> i32;
    type GetDesc1Fn = unsafe extern "system" fn(*mut c_void, *mut DxgiAdapterDesc1) -> i32;
    type ReleaseFn = unsafe extern "system" fn(*mut c_void) -> u32;

    fn com_method<T: Copy>(object: *mut c_void, index: usize) -> T {
        unsafe {
            let vtable = *(object as *mut *mut *mut c_void);
            mem::transmute_copy(&*vtable.add(index))
        }
    }

    fn enumerate_dxgi_adapters() -> Vec<GpuAdapterSnapshot> {
        const IDXGIFACTORY1_IID: Guid = Guid {
            data1: 0x770a_ae78,
            data2: 0xf26f,
            data3: 0x4dba,
            data4: [0xa8, 0x29, 0x25, 0x3c, 0x83, 0xd1, 0xb3, 0x87],
        };
        let Ok(library) = (unsafe {
            libloading::os::windows::Library::load_with_flags(
                "dxgi.dll",
                libloading::os::windows::LOAD_LIBRARY_SEARCH_SYSTEM32,
            )
            .map(Library::from)
        }) else {
            return Vec::new();
        };
        let create: CreateFactoryFn = unsafe {
            match library.get::<CreateFactoryFn>(b"CreateDXGIFactory1\0") {
                Ok(symbol) => *symbol,
                Err(_) => return Vec::new(),
            }
        };
        let mut factory = std::ptr::null_mut();
        if unsafe { create(&IDXGIFACTORY1_IID, &mut factory) } < 0 || factory.is_null() {
            return Vec::new();
        }
        // IDXGIFactory1 appends EnumAdapters1 at vtable slot 12 after the
        // IDXGIFactory methods. Slot 13 is IsCurrent and takes no arguments.
        let enumerate: EnumAdapters1Fn = com_method(factory, 12);
        let release: ReleaseFn = com_method(factory, 2);
        let mut adapters = Vec::new();
        for index in 0..64u32 {
            let mut adapter = std::ptr::null_mut();
            if unsafe { enumerate(factory, index, &mut adapter) } < 0 || adapter.is_null() {
                break;
            }
            let get_desc: GetDesc1Fn = com_method(adapter, 10);
            let adapter_release: ReleaseFn = com_method(adapter, 2);
            let mut desc = unsafe { mem::zeroed::<DxgiAdapterDesc1>() };
            if unsafe { get_desc(adapter, &mut desc) } >= 0 && desc.flags & 2 == 0 {
                let end = desc
                    .description
                    .iter()
                    .position(|unit| *unit == 0)
                    .unwrap_or(desc.description.len());
                let name = String::from_utf16_lossy(&desc.description[..end]);
                let id = format_luid(
                    desc.adapter_luid.high_part as u32,
                    desc.adapter_luid.low_part,
                );
                adapters.push(GpuAdapterSnapshot {
                    id,
                    name,
                    usage_percent: None,
                    dedicated_used_bytes: None,
                    dedicated_total_bytes: Some(desc.dedicated_video_memory as u64),
                    video_decode_percent: None,
                });
            }
            unsafe {
                adapter_release(adapter);
            }
        }
        unsafe {
            release(factory);
        }
        adapters.sort_by(|a, b| a.id.cmp(&b.id));
        adapters
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(windows)]
    #[test]
    #[ignore = "manual Windows hardware and idle worker probe"]
    fn system_metrics_probe() {
        let _ = get_system_status();
        std::thread::sleep(Duration::from_secs(2));
        let first = get_system_status();
        println!("Initial Windows system metrics: {first:#?}");
        assert!(
            first.gpu_adapters.iter().any(|adapter| {
                !adapter.name.trim().is_empty()
                    && !adapter.name.starts_with("GPU ")
                    && adapter.dedicated_total_bytes.is_some_and(|bytes| bytes > 0)
            }),
            "DXGI should report a named physical adapter and its dedicated memory capacity"
        );

        // Let the worker pass its five-second interest deadline, drop PDH/DXGI
        // resources, then pulse it again to verify sampling resumes cleanly.
        std::thread::sleep(Duration::from_secs(8));
        let before_resume = get_system_status().timestamp_ms;
        let mut resumed = get_system_status();
        for _ in 0..3 {
            if resumed.timestamp_ms > before_resume {
                break;
            }
            std::thread::sleep(SAMPLE_INTERVAL);
            resumed = get_system_status();
        }
        println!("Metrics after idle and resume: {resumed:#?}");
        assert!(resumed.timestamp_ms > before_resume);
    }

    #[test]
    fn system_cpu_first_sample_and_invalid_deltas_are_unavailable() {
        let first = SystemCpuTimes {
            idle: 10,
            kernel: 20,
            user: 30,
        };
        assert_eq!(calculate_system_cpu_percent(None, first), None);
        assert_eq!(calculate_system_cpu_percent(Some(first), first), None);
        assert_eq!(
            calculate_system_cpu_percent(
                Some(first),
                SystemCpuTimes {
                    idle: 50,
                    kernel: 60,
                    user: 70
                }
            ),
            Some(50.0),
        );
        assert_eq!(
            calculate_system_cpu_percent(
                Some(first),
                SystemCpuTimes {
                    idle: 1,
                    kernel: 60,
                    user: 70
                }
            ),
            None,
        );
    }

    #[test]
    fn process_cpu_uses_machine_total_and_rejects_counter_reset() {
        assert_eq!(
            calculate_process_cpu_percent(0, 10_000_000, Duration::from_secs(1), 2),
            Some(50.0)
        );
        assert_eq!(
            calculate_process_cpu_percent(10, 9, Duration::from_secs(1), 2),
            None
        );
        assert_eq!(calculate_process_cpu_percent(0, 1, Duration::ZERO, 1), None);
    }

    #[test]
    fn invalid_pdh_sample_recovers_on_a_later_valid_sample() {
        assert_eq!(valid_counter_value(0xC000_0BC6, 35.0), None);
        assert_eq!(valid_counter_value(0, 35.0), Some(35.0));
        assert_eq!(valid_counter_value(1, f64::NAN), None);
    }

    #[test]
    fn gpu_utilization_sums_processes_per_engine_then_uses_busiest_engine() {
        let high = "pid_4_luid_0x00000001_0x00000002_phys_0_eng_0_engtype_3D".to_string();
        let same = "pid_9_luid_0x00000001_0x00000002_phys_0_eng_0_engtype_3D".to_string();
        let decode =
            "pid_4_luid_0x00000001_0x00000002_phys_1_eng_2_engtype_VideoDecode".to_string();
        let other_adapter = "pid_4_luid_0x00000003_0x00000004_phys_0_eng_0_engtype_3D".to_string();
        let samples = vec![
            (high, Some(25.0)),
            (same, Some(15.0)),
            (decode, Some(60.0)),
            (other_adapter, Some(8.0)),
        ];

        let result = aggregate_gpu_engine_samples(&samples);

        assert_eq!(result["luid:00000001:00000002"].usage_percent, Some(60.0));
        assert_eq!(
            result["luid:00000001:00000002"].video_decode_percent,
            Some(60.0)
        );
        assert_eq!(result["luid:00000003:00000004"].usage_percent, Some(8.0));
    }

    #[test]
    fn gpu_memory_aggregates_valid_process_instances_by_adapter() {
        let samples = vec![
            ("pid_1_luid_0x00000001_0x00000002".into(), Some(100.0)),
            ("pid_2_luid_0x00000001_0x00000002".into(), Some(250.0)),
            ("pid_3_luid_0x00000001_0x00000002".into(), None),
            ("bad_instance".into(), Some(999.0)),
        ];
        assert_eq!(
            aggregate_gpu_memory_samples(&samples)["luid:00000001:00000002"],
            350
        );
    }

    #[test]
    fn empty_cache_marks_every_metric_unavailable() {
        let snapshot = SystemStatusSnapshot::default();
        assert_eq!(snapshot.timestamp_ms, 0);
        assert_eq!(snapshot.system_cpu_percent, None);
        assert_eq!(snapshot.system_ram_used_bytes, None);
        assert_eq!(snapshot.process_ram_bytes, None);
        assert!(snapshot.gpu_adapters.is_empty());
    }
}
