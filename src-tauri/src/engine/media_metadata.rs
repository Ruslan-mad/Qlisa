//! Runtime cache for inexpensive cue-list media metadata.
//!
//! Summary construction only reads a snapshot. Filesystem validation and
//! media probing are reserved here and run asynchronously by the caller.

use std::{
    collections::HashMap,
    path::PathBuf,
    time::{Duration, Instant, SystemTime},
};

const READY_TTL: Duration = Duration::from_secs(10);
// Do not relaunch ffprobe for a broken file on every cue-list refresh.
const FAILED_TTL: Duration = Duration::from_secs(60);
const PENDING_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MediaMetadataKey {
    pub path: PathBuf,
    pub probe_dimensions: bool,
}

impl MediaMetadataKey {
    pub fn new(path: PathBuf, probe_dimensions: bool) -> Self {
        Self {
            path,
            probe_dimensions,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MediaMetadata {
    pub file_size_bytes: u64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    /// 0 = compatible, 1 = recommended, 2 = unsupported.
    pub compatibility_status: u8,
    /// Stable frontend translation code for the first compatibility reason.
    pub compatibility_reason: Option<u8>,
    /// Non-fatal ffprobe diagnostic. File facts can still be valid when the
    /// compatibility scan fails.
    pub probe_failure: Option<MediaProbeFailure>,
    /// Cheap fingerprint used to avoid repeating the media probe when a TTL
    /// refresh finds that the file has not changed.
    pub modified_at: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaProbeFailure {
    RuntimeUnavailable,
    TimedOut,
    Failed,
}

impl MediaProbeFailure {
    pub fn user_message(self) -> &'static str {
        match self {
            Self::RuntimeUnavailable => "Media analysis runtime is unavailable.",
            Self::TimedOut => "Media analysis timed out.",
            Self::Failed => "Media analysis could not read this file.",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaMetadataEntry {
    /// Waiting in the bounded metadata worker queue.
    Queued,
    /// The worker is actively probing this file.
    Pending,
    Ready(MediaMetadata),
    /// A file-system failure. Keep the cause for row-level feedback.
    Failed(MediaMetadataFailure),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaMetadataFailure {
    MissingFile,
    NotAFile,
    Unreadable,
}

impl MediaMetadataFailure {
    pub fn user_message(&self) -> &'static str {
        match self {
            Self::MissingFile => "Media file was not found.",
            Self::NotAFile => "Media path is not a file.",
            Self::Unreadable => "Media file could not be read.",
        }
    }

    pub fn is_missing_file(&self) -> bool {
        matches!(self, Self::MissingFile)
    }
}

#[derive(Debug, Clone)]
pub struct MediaMetadataReservation {
    pub key: MediaMetadataKey,
    pub generation: u64,
    pub previous: Option<MediaMetadata>,
}

#[derive(Debug)]
struct CacheRecord {
    entry: MediaMetadataEntry,
    checked_at: Instant,
    reserved_at: Option<Instant>,
    generation: u64,
}

#[derive(Debug, Default)]
pub struct MediaMetadataCache {
    entries: HashMap<MediaMetadataKey, CacheRecord>,
    /// Intentionally survives `clear`: otherwise a late worker from before a
    /// clear could share a token with a new reservation and overwrite it.
    next_generation: u64,
}

impl MediaMetadataCache {
    pub fn snapshot(&self) -> HashMap<MediaMetadataKey, MediaMetadataEntry> {
        self.entries
            .iter()
            .map(|(key, record)| (key.clone(), record.entry.clone()))
            .collect()
    }

    pub fn reserve_missing(
        &mut self,
        keys: impl IntoIterator<Item = MediaMetadataKey>,
    ) -> Vec<MediaMetadataReservation> {
        self.reserve_at(keys, Instant::now())
    }

    fn reserve_at(
        &mut self,
        keys: impl IntoIterator<Item = MediaMetadataKey>,
        now: Instant,
    ) -> Vec<MediaMetadataReservation> {
        let mut jobs = Vec::new();
        for key in keys {
            let due = match self.entries.get(&key) {
                None => true,
                Some(record) => match record.reserved_at {
                    // The coordinator owns queued jobs. Re-enqueueing one
                    // after 30 seconds duplicates ffprobe work on big presets.
                    Some(_) if matches!(record.entry, MediaMetadataEntry::Queued) => false,
                    Some(started) => now.saturating_duration_since(started) >= PENDING_TIMEOUT,
                    None => {
                        let ttl = match record.entry {
                            MediaMetadataEntry::Failed(_) => FAILED_TTL,
                            MediaMetadataEntry::Ready(_) => READY_TTL,
                            MediaMetadataEntry::Queued | MediaMetadataEntry::Pending => PENDING_TIMEOUT,
                        };
                        now.saturating_duration_since(record.checked_at) >= ttl
                    }
                },
            };
            if !due {
                continue;
            }

            self.next_generation = self.next_generation.wrapping_add(1).max(1);
            let generation = self.next_generation;
            let previous = self
                .entries
                .get(&key)
                .and_then(|record| match &record.entry {
                    MediaMetadataEntry::Ready(metadata) => Some(*metadata),
                    _ => None,
                });
            // Keep valid old metadata visible during a background TTL refresh.
            let entry = previous
                .map(MediaMetadataEntry::Ready)
                .unwrap_or(MediaMetadataEntry::Queued);
            self.entries.insert(
                key.clone(),
                CacheRecord {
                    entry,
                    checked_at: now,
                    reserved_at: Some(now),
                    generation,
                },
            );
            jobs.push(MediaMetadataReservation {
                key,
                generation,
                previous,
            });
        }
        jobs
    }

    /// Commit only if this reservation is still current. A clear,
    /// invalidation, or replacement reservation makes old worker results inert.
    pub fn finish(
        &mut self,
        reservation: MediaMetadataReservation,
        result: Result<MediaMetadata, MediaMetadataFailure>,
    ) -> bool {
        self.finish_at(reservation, result, Instant::now())
    }

    fn finish_at(
        &mut self,
        reservation: MediaMetadataReservation,
        result: Result<MediaMetadata, MediaMetadataFailure>,
        now: Instant,
    ) -> bool {
        let Some(record) = self.entries.get_mut(&reservation.key) else {
            return false;
        };
        if record.generation != reservation.generation || record.reserved_at.is_none() {
            return false;
        }
        record.entry = match result {
            Ok(metadata) => MediaMetadataEntry::Ready(metadata),
            Err(error) => MediaMetadataEntry::Failed(error),
        };
        record.checked_at = now;
        record.reserved_at = None;
        true
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn invalidate_path(&mut self, path: &std::path::Path) {
        self.entries.retain(|key, _| key.path != path);
    }

    pub fn mark_processing(&mut self, reservation: &MediaMetadataReservation) -> bool {
        let Some(record) = self.entries.get_mut(&reservation.key) else {
            return false;
        };
        if record.generation != reservation.generation
            || !matches!(record.entry, MediaMetadataEntry::Queued)
        {
            return false;
        }
        record.entry = MediaMetadataEntry::Pending;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(path: &str, visual: bool) -> MediaMetadataKey {
        MediaMetadataKey::new(PathBuf::from(path), visual)
    }

    fn metadata(size: u64) -> MediaMetadata {
        MediaMetadata {
            file_size_bytes: size,
            ..MediaMetadata::default()
        }
    }

    #[test]
    fn reserve_deduplicates_and_refreshes_failed_after_short_ttl() {
        let mut cache = MediaMetadataCache::default();
        let now = Instant::now();
        let missing = key("missing.png", true);
        let first = cache.reserve_at([missing.clone(), missing.clone()], now);
        assert_eq!(first.len(), 1);
        assert!(cache.finish_at(
            first[0].clone(),
            Err(MediaMetadataFailure::MissingFile),
            now
        ));
        assert!(cache
            .reserve_at(
                [missing.clone()],
                now + FAILED_TTL - Duration::from_millis(1)
            )
            .is_empty());
        assert_eq!(cache.reserve_at([missing], now + FAILED_TTL).len(), 1);
    }

    #[test]
    fn ready_refresh_keeps_previous_value_visible() {
        let mut cache = MediaMetadataCache::default();
        let now = Instant::now();
        let clip = key("clip.mp4", true);
        let first = cache.reserve_at([clip.clone()], now);
        assert!(cache.finish_at(first[0].clone(), Ok(metadata(10)), now));
        assert!(cache
            .reserve_at([clip.clone()], now + READY_TTL - Duration::from_millis(1))
            .is_empty());

        let refresh = cache.reserve_at([clip.clone()], now + READY_TTL);
        assert_eq!(refresh[0].previous, Some(metadata(10)));
        assert_eq!(
            cache.snapshot().get(&clip),
            Some(&MediaMetadataEntry::Ready(metadata(10)))
        );
    }

    #[test]
    fn stale_finish_after_clear_or_invalidate_is_ignored() {
        let mut cache = MediaMetadataCache::default();
        let now = Instant::now();
        let clip = key("clip.mp4", true);
        let before_clear = cache.reserve_at([clip.clone()], now).remove(0);
        cache.clear();
        assert!(!cache.finish_at(before_clear, Ok(metadata(1)), now));

        let before_invalidate = cache.reserve_at([clip.clone()], now).remove(0);
        cache.invalidate_path(&clip.path);
        assert!(!cache.finish_at(before_invalidate, Ok(metadata(2)), now));
        assert!(cache.snapshot().is_empty());
    }

    #[test]
    fn replacement_generation_rejects_late_worker() {
        let mut cache = MediaMetadataCache::default();
        let now = Instant::now();
        let clip = key("clip.mp4", true);
        let old = cache.reserve_at([clip.clone()], now).remove(0);
        assert!(cache.mark_processing(&old));
        let new = cache
            .reserve_at([clip.clone()], now + PENDING_TIMEOUT)
            .remove(0);
        assert!(!cache.finish_at(old, Ok(metadata(1)), now + PENDING_TIMEOUT));
        assert!(cache.finish_at(new, Ok(metadata(2)), now + PENDING_TIMEOUT));
        assert_eq!(
            cache.snapshot().get(&clip),
            Some(&MediaMetadataEntry::Ready(metadata(2)))
        );
    }

    #[test]
    fn visual_and_stat_only_requests_have_independent_entries() {
        let mut cache = MediaMetadataCache::default();
        assert_eq!(
            cache
                .reserve_missing([key("shared.dat", false), key("shared.dat", true)])
                .len(),
            2
        );
    }

    #[test]
    fn queue_transitions_to_processing_only_once() {
        let mut cache = MediaMetadataCache::default();
        let job = cache.reserve_missing([key("clip.mp4", true)]).remove(0);
        assert_eq!(cache.snapshot().get(&job.key), Some(&MediaMetadataEntry::Queued));
        assert!(cache.mark_processing(&job));
        assert_eq!(cache.snapshot().get(&job.key), Some(&MediaMetadataEntry::Pending));
        assert!(!cache.mark_processing(&job));
    }
}
