// Typed wrappers around Tauri invoke() calls.
// All backend communication goes through this file.

import { invoke } from "@tauri-apps/api/core";
import { openUrl } from "@tauri-apps/plugin-opener";
import type {
  AppPreferences,
  AudioCueData,
  BrowserSurfaceState,
  AudioPreferences,
  BulkEditMetadata,
  CameraDeviceInfo,
  NdiSourceInfo,
  CollectReport,
  CueValidation,
  CueDurationMs,
  MachineAudioConfig,
  CueId,
  CueListMode,
  CueListSummary,
  CueSummary,
  CueType,
  DeviceInfo,
  DisplayPreferences,
  OutputDestination,
  DmxUniverseSnapshot,
  FixtureConflict,
  FixtureGroup,
  FixtureType,
  GeneralPreferences,
  CueListTcConfig,
  GroupMode,
  HealthAlert,
  ImportReport,
  ImageCueData,
  InputPatch,
  LogLine,
  MidiTrigger,
  MidiTriggerConfig,
  OscPatch,
  TcMachineConfig,
  TcPosition,
  TcTrigger,
  OscReceiveConfig,
  OutputPatch,
  OutputTransform,
  ParamTarget,
  PatchedFixture,
  RecoveryInfo,
  RelinkResult,
  ScreenInfo,
  TestPattern,
  UniverseOutput,
  VideoCueData,
  NumberCueData,
  WaveformData,
  WorkspaceInfo,
  MediaInfo,
  ProbeMetadata,
  MediaCompatibility,
  MediaConversionJob,
  MediaConversionRequest,
} from "./types";

// ---------------------------------------------------------------------------
// Transport
// ---------------------------------------------------------------------------

export const go = () => invoke<void>("go");
export const getBrowserSurfaceState = () => invoke<BrowserSurfaceState>("get_browser_surface_state");
export const goCue = (cueId: CueId) => invoke<void>("go_cue", { cueId });
export const setMasterVolume = (db: number) => invoke<void>("set_master_volume", { db });
export const stopAll = () => invoke<void>("stop_all");
export const hardStopAll = () => invoke<void>("hard_stop_all");
export const stopCue = (cueId: CueId) => invoke<void>("stop_cue", { cueId });
export const pauseCue = (cueId: CueId) => invoke<void>("pause_cue", { cueId });
export const seekCue = (cueId: CueId, positionMs: number) =>
  invoke<void>("seek_cue", { cueId, positionMs: Math.max(0, Math.round(positionMs)) });
/** Seek a media cue in source-file coordinates (waveform/filmstrip time). */
export const seekCueMedia = (cueId: CueId, filePositionMs: number) =>
  invoke<void>("seek_cue_media", { cueId, filePositionMs: Math.max(0, Math.round(filePositionMs)) });
export const resumeCue = (cueId: CueId) =>
  invoke<void>("resume_cue", { cueId });

// ---------------------------------------------------------------------------
// Cue management
// ---------------------------------------------------------------------------

export const getAllCues = () => invoke<CueSummary[]>("get_all_cues");
export const getCue = (cueId: CueId) =>
  invoke<AudioCueData | VideoCueData | ImageCueData | NumberCueData>("get_cue", { cueId });
export const addCue = (cueType: CueType, position = -1) =>
  invoke<CueId>("add_cue", { cueType, position });
export const addTargetedCue = (cueType: CueType, targetCueId: CueId, position = -1) =>
  invoke<CueId>("add_targeted_cue", { cueType, targetCueId, position });
export const removeCue = (cueId: CueId) =>
  invoke<void>("remove_cue", { cueId });
export const removeCues = (ids: CueId[]) =>
  invoke<void>("remove_cues", { ids });
export const moveCue = (cueId: CueId, newPosition: number) =>
  invoke<void>("move_cue", { cueId, newPosition });
export const moveCues = (ids: CueId[], beforeId: CueId | null) =>
  invoke<void>("move_cues", { ids, beforeId });
export const renumberCues = () =>
  invoke<void>("renumber_cues");
/** Resequence only `ids`, in list order, from `start` stepping by `increment`. */
export const renumberSelectedCues = (ids: CueId[], start: number, increment: number) =>
  invoke<void>("renumber_selected_cues", { ids, start, increment });
export const clearCueNumbers = () =>
  invoke<void>("clear_cue_numbers");

// Live level edits — engine only, no workspace write and no undo entry. Used
// while a slider or a matrix cell is being dragged; the value is persisted
// once on release with the ordinary updateCue.
export const setLiveLevel = (cueId: CueId, volumeDb: number, pan: number) =>
  invoke<void>("set_live_level", { cueId, volumeDb, pan });
export const setLiveCrosspoint = (cueId: CueId, input: number, output: number, gainDb: number) =>
  invoke<void>("set_live_crosspoint", { cueId, input, output, gainDb });

export const groupCues = (ids: CueId[]) =>
  invoke<CueId>("group_cues", { ids });
export const addNumberAction = (numberId: CueId, cueType: CueType, offsetMs = 0) =>
  invoke<CueId>("add_number_action", { numberId, cueType, offsetMs });
export const setNumberMaster = (numberId: CueId, childId: CueId) =>
  invoke<void>("set_number_master", { numberId, childId });
export const setNumberActionOffset = (numberId: CueId, actionId: CueId, offsetMs: number) =>
  invoke<void>("set_number_action_offset", { numberId, actionId, offsetMs });
export const removeNumberAction = (numberId: CueId, actionId: CueId) =>
  invoke<void>("remove_number_action", { numberId, actionId });
export const ungroup = (groupId: CueId) =>
  invoke<void>("ungroup", { groupId });
export const setGroupMode = (groupId: CueId, mode: GroupMode) =>
  invoke<void>("set_group_mode", { groupId, mode });
export const setPlaylistLoop = (groupId: CueId, loopOn: boolean) =>
  invoke<void>("set_playlist_loop", { groupId, loopOn });
export const addCueToGroup = (cueId: CueId, groupId: CueId, position = -1) =>
  invoke<void>("add_cue_to_group", { cueId, groupId, position });
export const removeCueFromGroup = (groupId: CueId, cueId: CueId) =>
  invoke<void>("remove_cue_from_group", { groupId, cueId });
export const moveToTopLevel = (cueId: CueId, beforeId: CueId | null) =>
  invoke<void>("move_to_top_level", { cueId, beforeId });
export const duplicateCue = (cueId: CueId) =>
  invoke<CueId>("duplicate_cue", { cueId });
export const duplicateCues = (ids: CueId[]) =>
  invoke<CueId[]>("duplicate_cues", { ids });

// ---------------------------------------------------------------------------
// Undo / Redo / Copy / Paste
// ---------------------------------------------------------------------------

export const undo = () => invoke<void>("undo");
export const redo = () => invoke<void>("redo");
export const canUndo = () => invoke<boolean>("can_undo");
export const canRedo = () => invoke<boolean>("can_redo");
export const copyCue = (cueId: CueId) => invoke<void>("copy_cue", { cueId });
/** Copy an ordered playlist selection, preserving selected Group hierarchy. */
export const copyCues = (cueIds: CueId[]) => invoke<void>("copy_cues", { cueIds });
export const pasteCue = (afterCueId?: CueId | null) =>
  invoke<CueId | CueId[]>("paste_cue", { afterCueId: afterCueId ?? null });
/** Properties merged into a cue. Deliberately loose: the backend merges this
 *  JSON into the cue's serialised form and rebuilds it, so every cue type's
 *  fields are valid here — typing it as one concrete cue was always a fiction. */
export type CueProperties = Record<string, unknown>;

/** One cue-specific patch in an atomic multi-cue inspector edit. */
export interface BulkCueUpdate {
  cueId: CueId;
  properties: CueProperties;
}

/** Complete serialised cue data returned by the batch inspector read. */
export type CueData = Record<string, unknown> & {
  /** Runtime-only editability from the authoritative backend lifecycle guard. */
  _bulk_edit?: BulkEditMetadata;
};

export const updateCue = (cueId: CueId, properties: CueProperties) =>
  invoke<void>("update_cue", { cueId, properties });
/** Update an authored action duration and return its recalculated cue-list row.
 * `null` means hold indefinitely and is accepted only for Image/Text cues. */
export const setCueDuration = (cueId: CueId, durationMs: CueDurationMs) =>
  invoke<CueSummary>("set_cue_duration", { cueId, durationMs });
/** Read selected cues in the exact requested order, including Group children. */
export const getCues = (cueIds: CueId[]) =>
  invoke<CueData[]>("get_cues", { cueIds });
/** Apply all inspector patches as one atomic backend transaction and one Undo step. */
export const bulkUpdateCues = (updates: BulkCueUpdate[]) =>
  invoke<void>("bulk_update_cues", { updates });
export const setAudioFile = (cueId: CueId, filePath: string) =>
  invoke<void>("set_audio_file", { cueId, filePath });
export const setVideoFile = (cueId: CueId, filePath: string) =>
  invoke<void>("set_video_file", { cueId, filePath });
export const getWaveformPeaks = (cueId: CueId, bins: number) =>
  invoke<WaveformData>("get_waveform_peaks", { cueId, bins });
export const getNormalizeDb = (cueId: CueId) =>
  invoke<number>("get_normalize_db", { cueId });
export const getMediaThumbnail = (path: string, seekInto: boolean) =>
  invoke<string>("get_media_thumbnail", { path, seekInto });

/** Read backend-probed media metadata and compatibility without exposing
 * arbitrary ffmpeg arguments to the UI. */
export const probeCueMedia = (cueId: CueId) =>
  invoke<ProbeMetadata>("probe_cue_media", { cueId });
export const analyzeMediaCompatibility = (metadata: ProbeMetadata) =>
  invoke<MediaCompatibility>("analyze_media_compatibility", { metadata });
export const getMediaInfo = async (cueId: CueId): Promise<MediaInfo> => {
  const metadata = await probeCueMedia(cueId);
  const compatibility = await analyzeMediaCompatibility(metadata);
  return {
    cue_id: cueId,
    path: metadata.path,
    format: metadata.format_name,
    codec: [metadata.video_codec, metadata.audio_codec].filter(Boolean).join(" / ") || null,
    duration_ms: metadata.duration_seconds == null ? null : Math.round(metadata.duration_seconds * 1000),
    width: metadata.width,
    height: metadata.height,
    bitrate_kbps: null,
    audio_channels: metadata.audio_channels,
    frame_rate: metadata.frame_rate,
    source_size_bytes: metadata.file_size,
    video_codec: metadata.video_codec,
    audio_codec: metadata.audio_codec,
    compatibility_known: true,
    compatibility_status: compatibility.status.toLowerCase(),
    compatible: compatibility.compatible,
    compatibility_note: compatibility.reasons.join(" ") || null,
  };
};
export const startMediaConversion = (request: MediaConversionRequest) =>
  invoke<MediaConversionJob>("start_media_conversion", { request });
export const listMediaConversions = () =>
  invoke<MediaConversionJob[]>("list_media_conversions");
export const getMediaConversionStatus = async (jobId: string): Promise<MediaConversionJob> => {
  const jobs = await listMediaConversions();
  const job = jobs.find((item) => item.id === jobId);
  if (!job) throw new Error("Conversion job no longer exists");
  return job;
};
export const cancelMediaConversion = (jobId: string) =>
  invoke<void>("cancel_media_conversion", { jobId });
export const replaceCueMedia = (jobId: string) =>
  invoke<void>("replace_cue_media_path", { jobId });
export const restoreCueMedia = (jobId: string) =>
  invoke<void>("restore_cue_media_path", { jobId });
export const openMediaOutputFolder = (jobId: string) =>
  invoke<void>("open_media_output_folder", { jobId });

/** Authorize the selected Video Cue's canonical file for the local asset
 * protocol. The backend resolves the path from `cueId`; callers cannot grant
 * arbitrary filesystem access by supplying a path. */
export const prepareVideoPreview = (cueId: CueId) =>
  invoke<string>("prepare_video_preview", { cueId });
export const getVideoFilmstrip = (path: string, tiles: number, tileWidth: number) =>
  invoke<string[]>("get_video_filmstrip", { path, tiles, tileWidth });
export const getVideoFilmstripRange = (
  path: string, startS: number, endS: number, tiles: number, tileWidth: number,
) =>
  invoke<string[]>("get_video_filmstrip_range", { path, startS, endS, tiles, tileWidth });
export const listVideoScreens = () => invoke<ScreenInfo[]>("list_video_screens");
export const listCameraDevices = () =>
  invoke<CameraDeviceInfo[]>("list_camera_devices");
/** Scan the local NDI discovery service. A saved source that is temporarily
 * absent remains valid in the cue; this list is only for operator selection. */
export const listNdiSources = () =>
  invoke<NdiSourceInfo[]>("list_ndi_sources");
export const identifyOutputScreen = (screenIndex: number | null) =>
  invoke<void>("identify_output_screen", { screenIndex });
export const getOutputTransform = () =>
  invoke<OutputTransform>("get_output_transform");
export const setOutputTransform = (transform: OutputTransform) =>
  invoke<void>("set_output_transform", { transform });
export const showTestPattern = (pattern: TestPattern) =>
  invoke<void>("show_test_pattern", { pattern });
export const clearTestPattern = () => invoke<void>("clear_test_pattern");
export const listSystemFonts  = () => invoke<string[]>("list_system_fonts");
export const previewOutputTimer = (
  font: string, fontSize: number, position: string, margin: number, text: string | null,
) => invoke<void>("preview_output_timer", { font, fontSize: fontSize, position, margin, text });
export const setImageFile = (cueId: CueId, filePath: string) =>
  invoke<void>("set_image_file", { cueId, filePath });
export const setMidiFile = (cueId: CueId, filePath: string) =>
  invoke<void>("set_midi_file", { cueId, filePath });

/** Authoritative session identity returned by every successful preview start.
 * `generation` is the same token carried by `preview-playhead` events. */
export interface PreviewStartResult {
  voice_id: string;
  generation: number;
}

export const previewCue = (cueId: CueId, startMs?: number, endMs?: number) =>
  invoke<PreviewStartResult>("preview_cue", {
    cueId,
    startMs: startMs != null ? Math.round(startMs) : null,
    endMs: endMs != null ? Math.round(endMs) : null,
  });

/** Explicit headphone-preview API for Live and future cue controls. `positionMs`
 * is file time; preview never falls back to the program/PA output. */
export const previewCueOnHeadphones = (cueId: CueId, positionMs?: number, endMs?: number) =>
  invoke<PreviewStartResult>("preview_cue", {
    cueId,
    startMs: positionMs != null ? Math.round(positionMs) : null,
    endMs: endMs != null ? Math.round(endMs) : null,
  });

export interface CuePreviewToggleResult {
  playing: boolean;
  voice_id?: string;
  generation?: number;
}

export const toggleCuePreview = (cueId: CueId, positionMs?: number, endMs?: number) =>
  invoke<CuePreviewToggleResult>("toggle_cue_preview", {
    cueId,
    positionMs: positionMs != null ? Math.round(positionMs) : null,
    endMs: endMs != null ? Math.round(endMs) : null,
  });

/** Stop the single active headphone preview, including one opened by another panel. */
export const stopCuePreview = () => invoke<void>("stop_cue_preview");

export const stopPreview = (voiceId: string) =>
  invoke<void>("stop_preview", { voiceId });

// ---------------------------------------------------------------------------
// Playhead
// ---------------------------------------------------------------------------

export const setPlayhead = (cueId: CueId | null) =>
  invoke<void>("set_playhead", { cueId });
export const getPlayhead = () => invoke<CueId | null>("get_playhead");

// ---------------------------------------------------------------------------
// Workspace
// ---------------------------------------------------------------------------

export const newWorkspace = () => invoke<void>("new_workspace");
export const saveWorkspace = (path: string) =>
  invoke<void>("save_workspace", { path });
export const loadWorkspace = (path: string) =>
  invoke<void>("load_workspace", { path });
export const importQlabWorkspace = (path: string) =>
  invoke<ImportReport>("import_qlab_workspace", { path });
export const getWorkspaceInfo = () => invoke<WorkspaceInfo>("get_workspace_info");
export const collectAndSave = (targetDir: string) =>
  invoke<CollectReport>("collect_and_save_workspace", { targetDir });

/** Returns snapshot metadata if a previous session ended without a clean exit. */
export const checkRecovery = () => invoke<RecoveryInfo | null>("check_recovery");
export const restoreRecovery = () => invoke<void>("restore_recovery");
export const discardRecovery = () => invoke<void>("discard_recovery");

// ---------------------------------------------------------------------------
// Preflight (Check Workspace) + relink
// ---------------------------------------------------------------------------

/** Validate the whole workspace; returns one entry per cue that has problems. */
export const checkWorkspace = () => invoke<CueValidation[]>("check_workspace");
/** Re-point a cue's missing media file (and any sibling missing files in the same folder). */
export const relinkMedia = (cueId: CueId, newPath: string) =>
  invoke<RelinkResult>("relink_media", { cueId, newPath });

// ---------------------------------------------------------------------------
// Logs
// ---------------------------------------------------------------------------

export const getRecentLogs = (limit?: number) =>
  invoke<LogLine[]>("get_recent_logs", { limit });
export const clearLogs = () => invoke<void>("clear_logs");
export const openLogsFolder = () => invoke<void>("open_logs_folder");

// ---------------------------------------------------------------------------
// Runtime health (device/network faults)
// ---------------------------------------------------------------------------

export const getHealthAlerts = () => invoke<HealthAlert[]>("get_health_alerts");
export const restoreAudioDevice = () => invoke<void>("restore_audio_device");

// Diagnostics is deliberately one extensible snapshot rather than a collection
// of polling calls. Keep the frontend contract here even while individual
// subsystems are still growing on the backend.
export const getDiagnosticsSnapshot = () =>
  invoke<import("./types").DiagnosticsSnapshot>("get_diagnostics_snapshot");
export const resetDiagnosticsStatistics = () =>
  invoke<void>("reset_diagnostics_statistics");

// ---------------------------------------------------------------------------
// Cue Lists
// ---------------------------------------------------------------------------

export const getCueLists = () => invoke<CueListSummary[]>("get_cue_lists");
export const addCueList = (name: string) =>
  invoke<string>("add_cue_list", { name });
export const removeCueList = (id: string) =>
  invoke<void>("remove_cue_list", { id });
export const renameCueList = (id: string, name: string) =>
  invoke<void>("rename_cue_list", { id, name });
export const setActiveCueList = (id: string) =>
  invoke<void>("set_active_cue_list", { id });
export const setCueListMode = (id: string, mode: CueListMode) =>
  invoke<void>("set_cue_list_mode", { id, mode });

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

export const getPreferences = () => invoke<AppPreferences>("get_preferences");
export const getAvailableBackends = () =>
  invoke<string[]>("get_available_backends");
export const getAsioOutputPairs = () =>
  invoke<number>("get_asio_output_pairs");
export const getMachineAudioConfig = () =>
  invoke<MachineAudioConfig>("get_machine_audio_config");
export const updateMachineAudioConfig = (config: MachineAudioConfig) =>
  invoke<void>("update_machine_audio_config", { config });
export const updateAudioPreferences = (prefs: AudioPreferences) =>
  invoke<void>("update_audio_preferences", { prefs });
export const updateGeneralPreferences = (prefs: GeneralPreferences) =>
  invoke<void>("update_general_preferences", { prefs });
export const updateDisplayPreferences = (prefs: DisplayPreferences) =>
  invoke<void>("update_display_preferences", { prefs });
export const listOutputDestinations = () =>
  invoke<OutputDestination[]>("list_output_destinations");
export const getOutputControlStatuses = () =>
  invoke<import("./types").OutputControlStatus[]>("get_output_control_statuses");
export const toggleOutputFtb = (outputId: string) =>
  invoke<boolean>("toggle_output_ftb", { outputId });
export const updateOutputDestinations = (destinations: OutputDestination[], defaultOutputId: string) =>
  invoke<void>("update_output_destinations", { destinations, defaultOutputId });
export const getNetworkIoStatus = () =>
  invoke<import("./types").NetworkIoStatus>("get_network_io_status");
export const getNetworkOutputStatuses = () =>
  invoke<import("./types").NetworkOutputRuntimeStatus[]>("get_network_output_statuses");
export async function listAudioDevices(backend?: string): Promise<DeviceInfo[]> {
  return invoke<DeviceInfo[]>("list_audio_devices", { backend: backend ?? null });
}
/** Generic system-host outputs used by the isolated headphone preview aux stream.
 * On Windows this intentionally stays WASAPI shared even when PA uses ASIO. */
export const listPreviewAudioDevices = () =>
  invoke<DeviceInfo[]>("list_preview_audio_devices");
export const testAudioDevice = (deviceId: string, backend: string) =>
  invoke<void>("test_audio_device", { deviceId, backend });
export const testOutputPatch = (deviceId: string, channels: number[]) =>
  invoke<void>("test_output_patch", { deviceId, channels });
export const testPreviewAudio = (previewDeviceId: string | null, backend: string, previewAsioPair: number | null) =>
  invoke<void>("test_preview_audio", { previewDeviceId, backend, previewAsioPair });
export const setPreviewGain = (gainDb: number) =>
  invoke<void>("set_preview_gain", { gainDb });
export const getAudioRuntimeStatus = () =>
  invoke<import("./types").AudioRuntimeStatus>("get_audio_runtime_status");
export const getOutputScreen = () =>
  invoke<number | null>("get_output_screen");
export const setOutputScreen = (screen: number | null) =>
  invoke<void>("set_output_screen", { screen });
export const toggleOutputWindow = () => invoke<void>("toggle_output_window");
export const getOutputWindowVisible = () => invoke<boolean>("get_output_window_visible");
export const setDisplayOutputMonitor = (outputId: string, monitor: number) =>
  invoke<void>("set_display_output_monitor", { outputId, monitor });
export const openPreferencesWindow = () => invoke<void>("open_preferences_window");
export const openDiagnosticsWindow = () => invoke<void>("open_diagnostics_window");

// ---------------------------------------------------------------------------
// Devices
// ---------------------------------------------------------------------------

/** Devices an Output Patch may target. Pass the backend currently selected in
 *  Preferences to see its device universe before it has been applied. */
export const listOutputDevices = (backend?: string) =>
  invoke<DeviceInfo[]>("list_output_devices", { backend: backend ?? null });

/** Workspace Output Patch table + default patch id. */
export interface OutputPatchTable {
  patches: OutputPatch[];
  default_patch_id: string | null;
}

export const getOutputPatchTable = () =>
  invoke<OutputPatchTable>("get_output_patches");
/** Flat patch list — convenience for patch-selector dropdowns. */
export const getOutputPatches = () =>
  getOutputPatchTable().then((t) => t.patches);
export const setOutputPatch = (
  patchId: string | null,
  name: string,
  deviceId: string,
  channels: number[],
  kind?: "main" | "aux",
  enabled?: boolean,
) => invoke<string>("set_output_patch", {
  patchId, name, deviceId, channels,
  ...(kind === undefined ? {} : { kind }),
  ...(enabled === undefined ? {} : { enabled }),
});
export const removeOutputPatch = (patchId: string) =>
  invoke<void>("remove_output_patch", { patchId });
export const setOutputPatchGain = (patchId: string, gainDb: number) =>
  invoke<void>("set_output_patch_gain", { patchId, gainDb });
export const openMixerWindow = () => invoke<void>("open_mixer_window");
export const openOutputMonitorWindow = () => invoke<void>("open_output_monitor_window");

export interface OutputMonitorSource {
  id: string;
  name: string;
}

export type OutputMonitorFrameStatus =
  | "frame"
  | "unchanged"
  | "black"
  | "no_frame"
  | "unavailable"
  | "error";

export interface OutputMonitorFrame {
  source_id: string;
  status: OutputMonitorFrameStatus;
  sequence: number;
  width: number;
  height: number;
  data_url: string | null;
  error: string | null;
}

export const listOutputMonitorSources = () =>
  invoke<OutputMonitorSource[]>("list_output_monitor_sources");
export const setOutputMonitorSource = (sourceId: string | null) =>
  invoke<void>("set_output_monitor_source", { sourceId });
export const getOutputMonitorFrame = (sourceId: string, afterSequence: number | null) =>
  invoke<OutputMonitorFrame>("get_output_monitor_frame", { sourceId, afterSequence });
export const setDefaultOutputPatch = (patchId: string | null) =>
  invoke<void>("set_default_output_patch", { patchId });
export const refreshDevices = () => invoke<void>("refresh_devices");

// ---------------------------------------------------------------------------
// Timecode
// ---------------------------------------------------------------------------

export const listTcMidiInputPorts = () => invoke<string[]>("list_tc_midi_input_ports");
export const getTcConfig = () => invoke<TcMachineConfig>("get_tc_config");
export const setTcConfig = (config: TcMachineConfig) => invoke<void>("set_tc_config", { config });
export const getTcPosition = () => invoke<TcPosition | null>("get_tc_position");
export const getCueTcTrigger = (cueId: string) => invoke<TcTrigger | null>("get_cue_tc_trigger", { cueId });
export const setCueTcTrigger = (
  cueId: string,
  positionStr: string | null,
  rateStr: string | null,
  realTime: boolean,
) => invoke<void>("set_cue_tc_trigger", { cueId, positionStr, rateStr, realTime });
export const getCuelistTcConfig = () => invoke<CueListTcConfig | null>("get_cuelist_tc_config");
export const setCuelistTcConfig = (config: CueListTcConfig) =>
  invoke<void>("set_cuelist_tc_config", { config });

// ---------------------------------------------------------------------------
// Audio inputs + Input Patches (Mic Cues)
// ---------------------------------------------------------------------------

export const listInputDevices = () => invoke<DeviceInfo[]>("list_input_devices");
export const listInputPatches = () => invoke<InputPatch[]>("list_input_patches");
export const addInputPatch = (name: string, deviceId: string, channels: number[]) =>
  invoke<InputPatch>("add_input_patch", { name, deviceId, channels });
export const updateInputPatch = (patch: InputPatch) =>
  invoke<void>("update_input_patch", { patch });
export const removeInputPatch = (patchId: string) =>
  invoke<void>("remove_input_patch", { patchId });

// ---------------------------------------------------------------------------
// OSC Patches
// ---------------------------------------------------------------------------

export const listOscPatches = () => invoke<OscPatch[]>("list_osc_patches");
export const addOscPatch = (name: string, ip: string, port: number) =>
  invoke<OscPatch>("add_osc_patch", { name, ip, port });
export const updateOscPatch = (patch: OscPatch) =>
  invoke<void>("update_osc_patch", { patch });
export const removeOscPatch = (patchId: string) =>
  invoke<void>("remove_osc_patch", { patchId });

// ---------------------------------------------------------------------------
// OSC Receive Config
// ---------------------------------------------------------------------------

export const getOscConfig = () => invoke<OscReceiveConfig>("get_osc_config");
export const setOscConfig = (config: OscReceiveConfig) =>
  invoke<void>("set_osc_config", { config });
export const sendOscTest = (patchId: string, message: import("./types").OscMessage) =>
  invoke<string>("send_osc_test", { patchId, message });

// ---------------------------------------------------------------------------
// Network interface selection
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// External links (tauri-plugin-opener)
// ---------------------------------------------------------------------------

export const openExternalUrl = (url: string) => openUrl(url);

export const listNetworkInterfaces = () =>
  invoke<import("./types").NetworkInterfaceInfo[]>("list_network_interfaces");
export const getNetworkConfig = () =>
  invoke<import("./types").NetworkInterfaceConfig>("get_network_config");
export const setNetworkConfig = (config: import("./types").NetworkInterfaceConfig) =>
  invoke<void>("set_network_config", { config });

// ---------------------------------------------------------------------------
// MIDI
// ---------------------------------------------------------------------------

export const listMidiOutputPorts = () => invoke<string[]>("list_midi_output_ports");
export const sendMidiTest = (
  portName: string,
  messageType: string,
  channel: number,
  data1: number,
  data2: number,
) => invoke<void>("send_midi_test", { portName, messageType, channel, data1, data2 });

// Per-cue MIDI triggers
export const listMidiInputPorts = () => invoke<string[]>("list_midi_input_ports");
export const getMidiTriggerConfig = () =>
  invoke<MidiTriggerConfig>("get_midi_trigger_config");
export const setMidiTriggerConfig = (config: MidiTriggerConfig) =>
  invoke<void>("set_midi_trigger_config", { config });
export const getCueMidiTrigger = (cueId: CueId) =>
  invoke<MidiTrigger | null>("get_cue_midi_trigger", { cueId });
export const setCueMidiTrigger = (cueId: CueId, trigger: MidiTrigger | null) =>
  invoke<void>("set_cue_midi_trigger", { cueId, trigger });
export const learnMidiTrigger = () => invoke<MidiTrigger | null>("learn_midi_trigger");
export const clearMidiLearn = () => invoke<void>("clear_midi_learn");

// ---------------------------------------------------------------------------
// DMX / Lighting
// ---------------------------------------------------------------------------

export const dmxSetOutputs = (outputs: UniverseOutput[]) =>
  invoke<void>("dmx_set_outputs", { outputs });
export const dmxGetOutputs = () => invoke<UniverseOutput[]>("dmx_get_outputs");
export const dmxSetChannel = (universe: number, address: number, value: number) =>
  invoke<void>("dmx_set_channel", { universe, address, value });
export const dmxSetBlackout = (on: boolean) => invoke<void>("dmx_set_blackout", { on });
export const dmxGetBlackout = () => invoke<boolean>("dmx_get_blackout");
export const dmxGetSnapshot = () => invoke<DmxUniverseSnapshot[]>("dmx_get_snapshot");

// ---------------------------------------------------------------------------
// DMX / Fixtures
// ---------------------------------------------------------------------------

export const listBuiltinFixtureTypes = () =>
  invoke<FixtureType[]>("list_builtin_fixture_types");
export const listFixtures = () => invoke<PatchedFixture[]>("list_fixtures");
export const addFixture = (
  label: string,
  universe: number,
  baseAddress: number,
  fixtureType: FixtureType,
) => invoke<PatchedFixture>("add_fixture", { label, universe, baseAddress, fixtureType });
export const updateFixture = (fixture: PatchedFixture) =>
  invoke<void>("update_fixture", { fixture });
export const removeFixture = (fixtureId: string) =>
  invoke<void>("remove_fixture", { fixtureId });
export const getFixtureConflicts = () =>
  invoke<FixtureConflict[]>("get_fixture_conflicts");
export const dmxTestFixture = (fixtureId: string, on: boolean) =>
  invoke<void>("dmx_test_fixture", { fixtureId, on });

// Live Dashboard
export const dmxSetFixtureParam = (fixtureId: string, paramIndex: number, value: number) =>
  invoke<void>("dmx_set_fixture_param", { fixtureId, paramIndex, value });
export const dmxClearFixtures = () => invoke<void>("dmx_clear_fixtures");
export const captureLiveTargets = () => invoke<ParamTarget[]>("capture_live_targets");

// Fixture groups
export const listFixtureGroups = () => invoke<FixtureGroup[]>("list_fixture_groups");
export const addFixtureGroup = (label: string, fixtureIds: string[]) =>
  invoke<FixtureGroup>("add_fixture_group", { label, fixtureIds });
export const updateFixtureGroup = (group: FixtureGroup) =>
  invoke<void>("update_fixture_group", { group });
export const removeFixtureGroup = (groupId: string) =>
  invoke<void>("remove_fixture_group", { groupId });
