// TypeScript types mirroring the Rust backend serialised structs.

export type CueId = string; // UUID as string

export type CueType = "audio" | "memo" | "wait" | "group" | "number" | "fade" | "stop" | "devamp" | "video" | "image" | "osc" | "midi" | "midi_file" | "light" | "mic" | "timecode" | "text" | "camera" | "browser" | "script" | CommandCueType;

/** Cues whose action is performed on *other* cues (QLab's control cues).
 *  Distinct types — own colour, own row label, 1:1 with QLab on import — over
 *  one shared Rust implementation. Grouped behind the toolbar's "+ Command". */
export type CommandCueType = "start" | "pause" | "resume" | "load" | "reset" | "goto" | "arm" | "disarm";

/** Hints are kept short enough to stay on one line in the toolbar dropdown —
 *  a wrapped row breaks the menu's rhythm. The inspector carries the longer
 *  explanation. */
export const COMMAND_CUE_TYPES: { type: CommandCueType; label: string; hint: string }[] = [
  { type: "start",  label: "Start",  hint: "Trigger the targets" },
  { type: "pause",  label: "Pause",  hint: "Pause the targets" },
  { type: "resume", label: "Resume", hint: "Resume the targets" },
  { type: "load",   label: "Load",   hint: "Bring them up paused" },
  { type: "reset",  label: "Reset",  hint: "Return them to standby" },
  { type: "goto",   label: "Goto",   hint: "Move the Playhead there" },
  { type: "arm",    label: "Arm",    hint: "Enable the targets" },
  { type: "disarm", label: "Disarm", hint: "Disable the targets" },
];

export const isCommandCueType = (t: CueType): t is CommandCueType =>
  COMMAND_CUE_TYPES.some((c) => c.type === t);

/** Accent color per cue type — the single source of truth for the toolbar
 *  buttons (App.tsx) and the cue-list context menu (CueListView.tsx). */
export const CUE_TYPE_COLORS: Record<CueType, string> = {
  audio:    "#3b82f6",          // blue
  video:    "#a78bfa",
  image:    "#86efac",
  stop:     "#ef4444",
  fade:     "#ec4899",
  wait:     "#fb923c",
  group:    "#fde047",
  number:   "#f59e0b",
  midi:     "var(--wc-accent)", // the green audio used to be
  midi_file: "#2dd4bf",         // teal — a MIDI cue, but one that has a length
  osc:      "#06b6d4",
  light:    "#fbbf24",
  mic:      "#22d3ee",          // cyan
  timecode: "#ff2d9c",          // hot pink
  text:     "#e2e8f0",
  memo:     "#e2e8f0",
  camera:   "#34d399",          // emerald
  browser:  "#60a5fa",          // blue — external web page
  devamp:   "#facc15",          // amber — the "release the vamp" action
  // Command cues: one hue family so they read as a group in the cue list,
  // shaded by what they do — go, hold, restore, arm.
  start:    "#4ade80",
  resume:   "#4ade80",
  pause:    "#facc15",
  load:     "#eab308",
  reset:    "#38bdf8",
  goto:     "#0ea5e9",
  arm:      "#fb923c",
  disarm:   "#f97316",
  script:   "#94a3b8",       // slate — a utility cue, not a stage element
};

/** Kinds of fade curve. `linear` is the hand-editable one: points set where
 *  the curve goes, Alt-dragging a segment sets how it gets there. (An earlier
 *  build had a separate smoothing mode; Alt-bending replaced it.) */
export type CurveKind = "linear" | "s_curve" | "exponential" | "parametric";

/** One control point. Both axes normalised 0..1: `t` is elapsed fade time,
 *  `v` is progress towards the target. The (0,0) and (1,1) endpoints are
 *  implicit and never stored, so a curve always reaches its target. */
export interface CurvePoint {
  t: number;
  v: number;
}

/** One fade envelope. */
export interface CurveShape {
  kind: CurveKind;
  /** Shaping parameter for the parametric kind, roughly -10..10. */
  intensity: number;
  /** Control points, meaningful for the linear and custom kinds. */
  points: CurvePoint[];
  /** Per-segment bow, -1..1, one per gap between resolved points. Alt-drag on
   *  a segment sets it; 0 leaves the segment as the kind draws it. */
  bends: number[];
}

/** The rising and falling envelopes of one fade. QLab's lock button mirrors
 *  them; `mirrored` is that lock, and is the default because a single curve
 *  used to drive both directions. */
export interface FadeShapes {
  up: CurveShape;
  down: CurveShape;
  mirrored: boolean;
}

export const DEFAULT_CURVE_SHAPE: CurveShape = { kind: "s_curve", intensity: 0, points: [], bends: [] };

export const CURVE_KIND_LABELS: Record<CurveKind, string> = {
  s_curve: "S-Curve",
  linear: "Linear (editable)",
  parametric: "Parametric Curve",
  exponential: "Exponential",
};

/** Whether control points do anything for this kind. */
export const curveUsesPoints = (kind: CurveKind) => kind === "linear";

/** One line of the QLab import report: what a QLab cue became. */
export interface ImportedCue {
  qlab_class: string;
  cue_number: string | null;
  cue_name: string;
  inkue_type: string;
  /** Set when the cue could not be represented faithfully. */
  note: string | null;
}

/** Outcome of importing a QLab workspace. */
export interface ImportReport {
  workspace_name: string;
  cue_count: number;
  cue_list_count: number;
  /** Cues needing a look — placeholders, or imported deliberately incomplete. */
  needs_attention: number;
  media_found: number;
  media_missing: string[];
  cues: ImportedCue[];
}

/** Kinds of MIDI message that can fire a cue. */
export type MidiTriggerType = "note_on" | "note_off" | "control_change" | "program_change";

/** A MIDI message bound to a cue. Stored on the Cue List, not the cue, so any
 *  cue type can carry one and it survives the rebuild every inspector edit does. */
export interface MidiTrigger {
  message_type: MidiTriggerType;
  /** 1–16, or 0 for any channel (omni). */
  channel: number;
  /** Note number, controller number, or program number. */
  data1: number;
  /** Required velocity / value. `null` = any — right for a note, whereas a
   *  footswitch that also sends 0 on release wants 127. */
  data2: number | null;
}

/** Machine-level MIDI trigger settings: which input drives per-cue triggers. */
export interface MidiTriggerConfig {
  enabled: boolean;
  port: string | null;
}

/** Memo Cue payload. A Memo performs no action; `memo_text` is its whole
 *  point — the note the operator reads in the cue list. */
export interface MemoCueData extends CueSummary {
  notes: string;
  memo_text: string;
}

/** MIDI File Cue payload. `sequence_duration_ms`, `track_count`, `channels`
 *  and `parse_error` are read-only: the backend derives them from the file
 *  every time it is parsed, and ignores them on the way back in. */
export interface MidiFileCueData extends CueSummary {
  notes: string;
  file_path: string | null;
  port_name: string;
  playback_rate: number;
  sequence_duration_ms: number | null;
  track_count: number | null;
  channels: number[] | null;
  parse_error: string | null;
}

/** Script Cue payload. Arguments are pre-split: no shell is involved, so a
 *  path containing spaces is one argument and needs no quoting. */
export interface ScriptCueData extends CueSummary {
  notes: string;
  command: string;
  args: string[];
  working_dir: string | null;
  timeout_ms: number;
}

export type CueState = "standby" | "running" | "paused" | "completed";

export type ContinueMode = "do_not_continue" | "auto_continue" | "auto_follow";

export type CueColor =
  | "none"
  | "red"
  | "orange"
  | "yellow"
  | "green"
  | "cyan"
  | "blue"
  | "purple"
  | "pink"
  | "white"
  | "black";

export type FadeCurve = "linear" | "s_curve" | "exponential";

export type GroupMode = "simultaneous" | "sequential" | "playlist" | "start_random";
export type NumberStartStopMode = "none" | "all" | "audio" | "video" | "selected";

export interface FadeSpec {
  duration_ms: number;
  curve: FadeCurve;
}

/** Compact row data used to render the cue list table. */
export interface CueSummary {
  id: CueId;
  cue_type: CueType;
  name: string;
  number: string | null;
  notes: string;
  state: CueState;
  continue_mode: ContinueMode;
  color: CueColor;
  pre_wait_ms: number;
  post_wait_ms: number;
  duration_ms: number | null;
  file_path: string | null;
  /** Resolved cue names and numbers for command cue Target cells. */
  target_cues?: CueTargetSummary[];
  /** Stop Cue with no explicit target means all cues. */
  targets_all?: boolean;
  /** On-disk media file size. Null for cues without a readable media file. */
  file_size_bytes: number | null;
  /** Source pixel dimensions for Image and Video cues. */
  media_width: number | null;
  media_height: number | null;
  /** Background ffprobe compatibility cache: 0 compatible, 1 recommended, 2 unsupported. */
  media_compatibility_status?: number | null;
  /** Stable reason code resolved to a localized message by the row. */
  media_compatibility_reason?: number | null;
  /** True only when an assigned media path could not be read from disk. */
  media_file_missing: boolean;
  /** Background media analysis is queued or currently probing. */
  media_metadata_pending?: boolean;
  /** Non-fatal reason why ffprobe compatibility analysis did not finish. */
  media_probe_error?: string;
  /** Memo Cue content shown in the Notes column. */
  memo_text?: string;
  /** True while the audio file is being decoded in a background thread. */
  is_loading: boolean;
  /** True when this cue is disabled — skipped by the transport on GO. */
  is_disabled: boolean;
  /** True when the cue has a missing media file or a runtime playback error. */
  is_broken: boolean;
  /** Volatile error reported while the cue was running. */
  error_message?: string;
  /** True for non-critical problems (no file assigned, zero duration, empty group). */
  is_warning: boolean;
  /** Human-readable warning description, present when is_warning is true. */
  warning_message?: string;
  /** Output Patch name this cue plays through (explicit or workspace default).
   *  Absent for cue types with no audio output. */
  output_patch_name?: string;
  /** Persisted audio level for audio-producing Audio/Video/Camera cues. */
  volume_db?: number;
  /** Persisted media loop count. `4294967295` means infinite looping. */
  loop_count?: number;
  /** Explicit named visual destinations; [] means the configured default. */
  visual_output_ids?: string[];
  /** Duration of one loop iteration in ms (raw file duration, no loop multiplier). null for non-media cues. */
  file_duration_ms: number | null;
  /** Cached media duration from full cue serialization (not present in summaries). */
  cached_duration_ms?: number | null;
  /** For Group cues: direct child cue summaries (recursive). */
  children?: CueSummary[];
  /** For Group cues: playback mode. */
  group_mode?: GroupMode;
  /** For Playlist Group cues: whether the playlist loops (wraps last → first). */
  playlist_loop?: boolean;
  /** For running Sequential/Playlist Group cues: ID of the currently active child. */
  active_child_id?: string;
  /** Number-only master and timed action metadata. */
  number_master_id?: string | null;
  number_master_type?: CueType | null;
  number_master_name?: string | null;
  number_action_offsets_ms?: Record<string, number>;
  number_action_ready?: boolean;
  number_start_stop_mode?: NumberStartStopMode;
  number_start_stop_ids?: string[];
  number_finish_start_ids?: string[];
}

export type MediaConversionMode = "optimize" | "size";
export type MediaConversionQuality = "high" | "medium" | "low";
export type MediaConversionResolution = "original" | "1080p" | "720p";
export type MediaConversionImageFormat = "png" | "jpg";
export type MediaConversionStatus = "queued" | "running" | "completed" | "failed" | "cancelled";

/** Backend ffprobe metadata, kept separate from the cue summary. */
export interface ProbeMetadata {
  path: string;
  file_size: number;
  format_name: string | null;
  duration_seconds: number | null;
  width: number | null;
  height: number | null;
  video_codec: string | null;
  video_profile: string | null;
  video_bit_depth: number | null;
  audio_codec: string | null;
  audio_channels: number | null;
  sample_rate: number | null;
  pixel_format: string | null;
  frame_rate: number | null;
}

export interface MediaCompatibility {
  status: string;
  compatible: boolean;
  reasons: string[];
}

export interface MediaInfo {
  cue_id: CueId;
  path: string;
  format: string | null;
  codec: string | null;
  duration_ms: number | null;
  width: number | null;
  height: number | null;
  bitrate_kbps: number | null;
  audio_channels: number | null;
  frame_rate: number | null;
  source_size_bytes: number | null;
  video_codec: string | null;
  audio_codec: string | null;
  compatibility_known: boolean;
  compatibility_status: string | null;
  compatible: boolean;
  compatibility_note: string | null;
}

export interface MediaConversionRequest {
  cue_id: CueId | null;
  input_path: string;
  mode: MediaConversionMode;
  quality: number | null;
  max_resolution: number | null;
  image_format?: MediaConversionImageFormat | null;
}

export interface MediaConversionJob {
  id: string;
  cue_id: CueId | null;
  input_path: string;
  output_path?: string | null;
  mode: MediaConversionMode;
  status: MediaConversionStatus;
  progress: number | null;
  processed: number | null;
  total: number | null;
  speed: number | null;
  source_size: number | null;
  new_size: number | null;
  error?: string | null;
  error_category?: string | null;
  applied_to_cue: boolean;
}

/** Full Number data. Children are existing Cue payloads; generated transport
 * wrappers are never persisted. */
export interface NumberCueData extends CueSummary {
  children: CueSummary[];
  number_master_id: string | null;
  /** Shared transition envelope applied to all active audio/video children. */
  fade_in_ms?: number | null;
  fade_in_curve?: FadeCurve | null;
  fade_out_ms?: number | null;
  fade_out_curve?: FadeCurve | null;
  /** Omitted by the backend when no child offsets have been configured. */
  number_action_offsets_ms?: Record<string, number>;
  number_start_stop_mode?: NumberStartStopMode;
  number_start_stop_ids?: string[];
  number_finish_start_ids?: string[];
}

export interface CueTargetSummary {
  number: string | null;
  name: string;
}

/** A requested authored action duration in milliseconds. `null` means an
 * indefinite hold, which the backend permits only for Image and Text cues. */
export type CueDurationMs = number | null;

/** Runtime-only capability metadata attached by getCues for the multi-cue
 * inspector. It is never part of a saved cue or project file. */
export interface BulkEditMetadata {
  fade_editable: boolean;
  /** Whether this cue is stopped/unloaded and can safely be rebuilt for
   * type-specific edits such as routing, timing and fades. */
  rebuild_editable?: boolean;
}

/** Full cue data returned by get_cue. */
/** Play count meaning "loop this slice forever" (a vamp) — u32::MAX. */
export const PLAY_COUNT_INFINITE = 4294967295;

/** QLab-style slices: markers split the clip into segments, each with a play
 *  count (PLAY_COUNT_INFINITE = vamp until a Devamp Cue releases it). */
export interface SliceList {
  /** Marker positions in ms from file start, sorted ascending. */
  markers: number[];
  /** Play count per segment (markers.length + 1 entries). */
  play_counts: number[];
}

export const EMPTY_SLICES: SliceList = { markers: [], play_counts: [1] };

export interface AudioCueData extends CueSummary {
  notes: string;
  volume_db: number;
  pan: number;
  /** Crosspoint levels in dB, `[input channel][patch channel]`. `null` = the
   *  cue routes with Pan instead. Replaces pan when set. */
  level_matrix?: number[][] | null;
  fade_in_ms: number | null;
  fade_in_curve: FadeCurve | null;
  fade_out_ms: number | null;
  fade_out_curve: FadeCurve | null;
  start_time_ms: number | null;
  end_time_ms: number | null;
  loop_count: number;
  output_patch_id: string | null;
  rate: number;
  slices: SliceList;
}

/** How the source frame is mapped onto the output surface. */
export type FitMode = "fit" | "fill" | "stretch";

/** Per-cue visual geometry for Video and Image cues. */
export interface VideoGeometry {
  fit_mode: FitMode;
  /** Horizontal offset as a fraction of the scaled video width (−1..1). */
  pan_x: number;
  /** Vertical offset as a fraction of the scaled video height (−1..1). */
  pan_y: number;
  /** Linear scale factor (1.0 = 100%). */
  scale: number;
  /** Clockwise rotation in degrees (0–359). */
  rotation: number;
  /** Crop per edge as a fraction of the source size (0..0.45 each). */
  crop_left: number;
  crop_right: number;
  crop_top: number;
  crop_bottom: number;
}

/** Neutral geometry (engine defaults). */
export const DEFAULT_GEOMETRY: VideoGeometry = {
  fit_mode: "fit",
  pan_x: 0,
  pan_y: 0,
  scale: 1,
  rotation: 0,
  crop_left: 0,
  crop_right: 0,
  crop_top: 0,
  crop_bottom: 0,
};

/** How a visual cue's pixels combine with the layers below it. */
export type BlendMode =
  | "normal" | "add" | "multiply" | "screen" | "overlay"
  | "soft_light" | "hard_light" | "darken" | "lighten"
  | "color_dodge" | "color_burn" | "difference" | "exclusion" | "subtract";

/** Compositing properties of a visual cue (QLab layer model). */
export interface LayerStyle {
  /** Stacking order 1–1000 (higher = closer to the viewer). null = automatic (newest on top). */
  layer: number | null;
  /** Base opacity 0.0–1.0. */
  opacity: number;
  blend_mode: BlendMode;
}

/** Default compositing (automatic layer, fully opaque, normal blending). */
export const DEFAULT_LAYER_STYLE: LayerStyle = {
  layer: null,
  opacity: 1,
  blend_mode: "normal",
};

/** Full cue data returned by get_cue for a Video Cue. */
export interface VideoCueData extends CueSummary {
  notes: string;
  volume_db: number;
  /** Crosspoint levels in dB for the video's audio track. See AudioCueData. */
  level_matrix?: number[][] | null;
  /** Audio track fade-in. */
  fade_in_ms: number | null;
  fade_in_curve: FadeCurve | null;
  /** Audio track fade-out. */
  fade_out_ms: number | null;
  fade_out_curve: FadeCurve | null;
  /** Visual (GL overlay) fade-in — independent from audio. */
  video_fade_in_ms: number | null;
  video_fade_in_curve: FadeCurve | null;
  /** Visual (GL overlay) fade-out — independent from audio. */
  video_fade_out_ms: number | null;
  video_fade_out_curve: FadeCurve | null;
  start_time_ms: number | null;
  end_time_ms: number | null;
  loop_count: number;
  output_surface_id: string | null;
  /** Stable named destination (alias-compatible with output_surface_id). */
  output_id?: string | null;
  output_ids?: string[];
  output_patch_id: string | null;
  /** Freeze on the last frame at natural EOF instead of cutting to black. */
  hold_last_frame: boolean;
  geometry: VideoGeometry;
  layer_style: LayerStyle;
  slices: SliceList;
  /** Full media duration (serialized form only — summaries carry
   *  file_duration_ms instead). */
  cached_duration_ms?: number | null;
}

/** 9-point position grid for TextCue. */
export type TextPosition =
  | "top_left" | "top_center" | "top_right"
  | "middle_left" | "center" | "middle_right"
  | "bottom_left" | "bottom_center" | "bottom_right";

/** Full cue data returned by get_cue for a Text Cue. */
export interface TextCueData extends CueSummary {
  notes: string;
  /** Text content to display (multi-line supported). */
  text: string;
  /** Font family name. */
  font: string;
  /** Font size in mpv OSD/ASS points. */
  font_size: number;
  /** Text colour as "#RRGGBB". */
  text_color: string;
  /** Position on the output surface. */
  position: TextPosition;
  /** Stable named destination; null uses display.default_output_id. */
  output_id?: string | null;
  output_ids?: string[];
  /** Target monitor index. null = use workspace display setting. */
  screen_index: number | null;
  /** Auto-complete after this duration in ms. null = hold until stopped. */
  display_duration_ms: number | null;
}

/** Full cue data returned by get_cue for an Image Cue. */
export interface ImageCueData extends CueSummary {
  notes: string;
  fade_in_ms: number | null;
  fade_in_curve: FadeCurve | null;
  fade_out_ms: number | null;
  fade_out_curve: FadeCurve | null;
  /** How long the image stays on screen in ms. null = infinite (hold until stopped). */
  display_duration_ms: number | null;
  geometry: VideoGeometry;
  layer_style: LayerStyle;
  output_id?: string | null;
  output_ids?: string[];
}

// ---------------------------------------------------------------------------
// Camera types
// ---------------------------------------------------------------------------

/** Where a Camera Cue's live feed comes from (matches the Rust enum tagging). */
export type CameraSource =
  | { kind: "device"; id: string; name: string }
  | { kind: "url"; url: string }
  | { kind: "network"; input: NetworkInputSource };

/** A managed live video input. SRT is decoded by libmpv/FFmpeg; NDI uses a
 * selected sender from the Core NDI discovery list. */
export type NetworkInputSource =
  | { protocol: "ndi"; source_name: string }
  | { protocol: "srt"; settings: SrtSettings };

/** A source announced by the local Core NDI runtime. */
export interface NdiSourceInfo {
  name: string;
  url_address: string | null;
}

/** One connected capture device (from list_camera_devices). */
export interface CameraDeviceInfo {
  id: string;
  name: string;
}

/** Full cue data returned by get_cue for a Camera Cue. */
export interface CameraCueData extends CueSummary {
  notes: string;
  volume_db: number;
  pan: number;
  level_matrix?: number[][] | null;
  output_patch_id: string | null;
  source: CameraSource;
  /** Visual (GL overlay) fade-in from black. */
  video_fade_in_ms: number | null;
  video_fade_in_curve: FadeCurve | null;
  /** Visual (GL overlay) fade-out to black on stop. */
  video_fade_out_ms: number | null;
  video_fade_out_curve: FadeCurve | null;
  geometry: VideoGeometry;
  layer_style: LayerStyle;
  /** Core NDI receiver bandwidth profile for this Camera Cue. */
  ndi_quality?: NdiQuality;
  output_id?: string | null;
  output_ids?: string[];
}

/** Full cue data returned by get_cue for a Browser Cue. */
export interface BrowserCueData extends CueSummary {
  notes: string;
  /** Safe HTTP(S) URL. The page itself may refresh its own content. */
  url: string;
  /** Stable named destination; null uses the configured default. */
  output_id?: string | null;
  output_ids?: string[];
  /** Navigate on every GO, or keep the loaded page between GOs. */
  reload_on_go: boolean;
  /** WebView zoom scale, clamped by the backend to 0.25..3. */
  zoom: number;
}

/** Runtime state of the one shared Browser WebView. */
export interface BrowserSurfaceState {
  active: boolean;
  cue_id: string | null;
  output_id: string | null;
  url: string | null;
  status: string;
  error: string | null;
}

// ---------------------------------------------------------------------------
// MIDI types
// ---------------------------------------------------------------------------

export type MidiMessageType = "note_on" | "note_off" | "control_change" | "program_change";

export interface MidiMessage {
  port_name: string;
  message_type: MidiMessageType;
  /** MIDI channel 1–16 */
  channel: number;
  /** Note / CC number / program (0–127) */
  data1: number;
  /** Velocity / CC value (0–127); unused for program_change */
  data2: number;
}

/** Full cue data returned by get_cue for a MIDI Cue. */
export interface MidiCueData extends CueSummary {
  notes: string;
  messages: MidiMessage[];
}

/** Full cue data returned by get_cue for a Fade Cue. */
export interface FadeCueData extends CueSummary {
  notes: string;
  /** UUIDs of cues to fade (empty = no-op). */
  target_cue_ids: string[];
  /** Display labels kept in sync with target_cue_ids. */
  target_cue_numbers: string[];
  /** Target audio volume in dB (−60 = silence, 0 = unity). */
  target_volume_db: number;
  /** Target visual brightness in percent (0 = black, 100 = fully visible). Independent from volume. */
  target_brightness_pct: number;
  /** Target stereo pan (-1 = left, +1 = right); null = leave pan untouched. */
  target_pan: number | null;
  /** When true (default) the fade drives volume toward target_volume_db; false = pan-only. */
  fade_volume: boolean;
  /** Fade duration in milliseconds. */
  fade_duration_ms: number;
  /** Legacy single-curve name. Still written for older builds; `fade_shapes`
   *  is what the engine uses. */
  fade_curve: FadeCurve;
  /** Rising and falling envelopes — QLab's Curve tab. */
  fade_shapes: FadeShapes;
  /** Stop the target cue(s) once the fade completes. */
  stop_at_end: boolean;
}

/** Full cue data returned by get_cue for a Wait Cue. */
export interface WaitCueData extends CueSummary {
  notes: string;
  /** The configured wait duration in milliseconds. */
  wait_duration_ms: number;
}

/** Full cue data returned by get_cue for a Stop Cue. */
export interface StopCueData extends CueSummary {
  notes: string;
  /** UUIDs of cues to stop. Empty = stop all running cues. */
  target_cue_ids: string[];
  /** Display labels kept in sync with target_cue_ids. */
  target_cue_numbers: string[];
  /** true = immediate cut (no fade); false = soft stop with workspace fade-out. */
  hard_stop_mode: boolean;
}

export interface DevampCueData extends CueSummary {
  notes: string;
  /** UUIDs of the cues to devamp. */
  target_cue_ids: string[];
  /** Display labels kept in sync with target_cue_ids. */
  target_cue_numbers: string[];
  /** true = the target stops at the end of its current slice. */
  stop_at_end: boolean;
}

/** Information about a connected monitor. */
export interface ScreenInfo {
  index: number;
  width: number;
  height: number;
  x: number;
  y: number;
  is_primary: boolean;
}

export interface DeviceInfo {
  id: string;
  name: string;
  channels: number;
  sample_rate: number;
}

export type AudioRuntimeState = "working" | "fallback" | "error" | "not_configured" | "not_tested";

/** Runtime truth for the two operator-facing audio routes. */
export interface AudioRuntimeStatus {
  main_state: AudioRuntimeState;
  main_device_name: string | null;
  preview_state: AudioRuntimeState;
  preview_device_name: string | null;
}

export interface OutputPatch {
  id: string;
  name: string;
  device_id: string;
  channels: number[];
  /** Mixer fader in dB (0 = unity). */
  gain_db: number;
  /** Logical route. Main is mandatory; Aux buses are optional. */
  kind: "main" | "aux";
  /** Disabled Aux buses remain available for migration but cannot be selected. */
  enabled: boolean;
}

/** A named live-audio input mapping (Mic Cues). Mirror of OutputPatch, stored in the workspace. */
export interface InputPatch {
  id: string;
  name: string;
  device_id: string;
  /** Zero-based input device channel indices this patch exposes. */
  channels: number[];
}

/** Full cue data returned by get_cue for a Mic Cue. */
export interface MicCueData extends CueSummary {
  notes: string;
  input_patch_id: string | null;
  /** Device channel indices to take (empty = use the patch's own channels). */
  input_channels: number[];
  output_patch_id: string | null;
  volume_db: number;
  pan: number;
  fade_in_ms: number | null;
  fade_in_curve: FadeCurve | null;
  fade_out_ms: number | null;
  fade_out_curve: FadeCurve | null;
}

export type CueListMode = "sequential" | "cart";

export interface CueListSummary {
  id: string;
  name: string;
  mode: CueListMode;
}

export interface WorkspaceInfo {
  name: string;
  is_modified: boolean;
  file_path: string | null;
}

/** Metadata for an unsaved-work snapshot left by an abnormally-terminated session. */
export interface RecoveryInfo {
  name: string;
  original_path: string | null;
  modified_at: string | null;
}

// --- Preflight (Check Workspace) -------------------------------------------

export type Severity = "error" | "warning";

export interface CueIssue {
  severity: Severity;
  message: string;
}

/** One cue's preflight result (only cues with at least one issue are returned). */
export interface CueValidation {
  cue_id: CueId;
  cue_number: string | null;
  cue_name: string;
  cue_type: CueType;
  issues: CueIssue[];
  /** The unresolved media path when the problem is a missing file (drives relink). */
  missing_file: string | null;
}

export interface RelinkResult {
  relinked: number;
}

// --- Logs ------------------------------------------------------------------

export interface LogLine {
  ts: string;
  level: string;
  target: string;
  message: string;
}

// --- Runtime health (device/network faults) --------------------------------

export type HealthLevel = "error" | "warning" | "info";

export interface HealthAlert {
  key: string;
  level: HealthLevel;
  message: string;
  /** Action id the banner maps to a command (e.g. "restore_audio_device"). */
  action: string | null;
  action_label: string | null;
}

/** Extensible, read-only runtime snapshot used by the Diagnostics page. */
export interface DiagnosticsSnapshot {
  system?: {
    processWorkingSetBytes?: number | null;
    processPrivateBytes?: number | null;
  } | null;
  audio?: {
    health?: string | null;
    sourceCounts?: Record<string, number | null> | null;
    scheduler?: Record<string, unknown> | null;
    memory?: Record<string, unknown> | null;
    peaks?: Record<string, unknown> | null;
    events?: unknown[] | null;
    sources?: unknown[] | null;
  } | null;
  /** Optional video diagnostics. The page accepts newer backend fields defensively. */
  video?: Record<string, unknown> | null;
  /** Optional network diagnostics assembled from existing transport runtime state. */
  network?: Record<string, unknown> | null;
  [key: string]: unknown;
}

export interface CollectReport {
  workspace_path: string;
  files_copied: number;
  files_skipped: number;
  files_missing: string[];
}

export interface WaveformData {
  peaks: number[];
  /** RMS per bin — the "body" of the sound, drawn inside the peak envelope. */
  rms: number[];
  file_duration_s: number;
}

// ---------------------------------------------------------------------------
// OSC types
// ---------------------------------------------------------------------------

export type OscArgType = "int" | "float" | "str" | "bool";

export type OscArg =
  | { type: "int";   value: number }
  | { type: "float"; value: number }
  | { type: "str";   value: string }
  | { type: "bool";  value: boolean };

export interface OscMessage {
  patch_id: string;
  address: string;
  args: OscArg[];
}

export interface OscPatch {
  id: string;
  name: string;
  ip: string;
  port: number;
}

/** Full cue data returned by get_cue for an OSC Cue. */
export interface OscCueData extends CueSummary {
  notes: string;
  messages: OscMessage[];
}

export interface OscReceiveConfig {
  enabled: boolean;
  port: number;
  allowed_ips: string[];
  feedback_enabled: boolean;
  feedback_host: string;
  feedback_port: number;
  /** Send rate (Hz) for /inkue/cue/{i}/progress|elapsed|remaining|duration.
   *  0 disables progress feedback. */
  feedback_progress_hz: number;
}

/** One IPv4-capable network interface (Preferences → Network). */
export interface NetworkInterfaceInfo {
  name: string;
  ip: string;
  is_loopback: boolean;
}

/** Machine-level network interface selection. All-null = Automatic. */
export interface NetworkInterfaceConfig {
  interface_name: string | null;
  interface_ip: string | null;
}

// ---------------------------------------------------------------------------
// DMX / Lighting
// ---------------------------------------------------------------------------

export type OutputProtocol = "Sacn" | "ArtNet";

/** One workspace-level universe output mapping (matches `engine::dmx_sink::UniverseOutput`). */
export interface UniverseOutput {
  universe: number;
  protocol: OutputProtocol;
  /** Destination IP string, or null for the sACN multicast group. */
  destination: string | null;
  enabled: boolean;
}

/** Live output bytes of one universe, pushed via the `dmx-monitor` event. */
export interface DmxUniverseSnapshot {
  universe: number;
  channels: number[];
}

/** Resolution of one fixture parameter on the wire (matches `engine::dmx_engine::ChannelWidth`). */
export type ChannelWidth = "Bit8" | "Bit16";

/** What a fixture parameter controls. */
export type ParamKind =
  | "intensity"
  | "red"
  | "green"
  | "blue"
  | "white"
  | "amber"
  | "uv"
  | "pan"
  | "tilt"
  | "generic";

/** One controllable parameter of a fixture, offset from its base address. */
export interface FixtureParam {
  kind: ParamKind;
  name: string;
  channel_offset: number;
  width: ChannelWidth;
  default: number;
}

/** The channel layout of a kind of lighting instrument. */
export interface FixtureType {
  name: string;
  parameters: FixtureParam[];
}

/** A fixture placed at a DMX address in the workspace patch. */
export interface PatchedFixture {
  id: string;
  label: string;
  universe: number;
  base_address: number;
  fixture_type: FixtureType;
}

/** A detected address clash between two patched fixtures. */
export interface FixtureConflict {
  fixture_a: string;
  fixture_b: string;
  universe: number;
  message: string;
}

/** A named set of fixtures driven together by one Light Cue control. */
export interface FixtureGroup {
  id: string;
  label: string;
  fixture_ids: string[];
}

/** One thing a Light Cue drives: a fixture parameter, or a group parameter-kind. */
export type ParamTarget =
  | { kind: "fixture"; fixture_id: string; param_index: number; value: number }
  | { kind: "group"; group_id: string; param_kind: ParamKind; value: number };

/** Full cue data returned by get_cue for a Light Cue. */
export interface LightCueData extends CueSummary {
  notes: string;
  targets: ParamTarget[];
  fade: FadeSpec;
}

// ---------------------------------------------------------------------------
// Timecode
// ---------------------------------------------------------------------------

export type TcRate = "24" | "25" | "29.97" | "29.97df" | "30";

export type TcSource = "mtc" | "ltc";

export interface TcPosition {
  h: number;
  m: number;
  s: number;
  f: number;
  rate: TcRate;
}

export interface TcTrigger {
  /** SMPTE string HH:MM:SS:FF or HH:MM:SS;FF */
  position: string;
  /** true = position was entered as Real Time (ms) */
  real_time: boolean;
  rate: TcRate;
}

export type TcOnStop = "continue" | "pause" | "stop";

export interface CueListTcConfig {
  enabled: boolean;
  rate: TcRate;
  freewheel_ms: number;
  on_stop: TcOnStop;
}

export type TcOutputType = "mtc" | "ltc";

export interface TimecodeCueData extends CueSummary {
  tc_type: TcOutputType;
  midi_port: string | null;
  output_patch_id: string | null;
  rate: TcRate;
  /** SMPTE string */
  start_frame: TcPosition;
  end_frame: TcPosition | null;
}

export interface TcMachineConfig {
  enabled: boolean;
  receiver_config: {
    source: TcSource;
    midi_port: string | null;
    ltc_device_id: string | null;
  };
}

// ---------------------------------------------------------------------------
// Preferences
// ---------------------------------------------------------------------------

export type AudioBackend = "wasapi_shared" | "wasapi_exclusive" | "asio";

/** Hardware-specific settings — stored in %APPDATA%\Inkue\audio.json, not in the workspace. */
export interface MachineAudioConfig {
  backend: AudioBackend;
  device_id: string | null;
  /** Friendly name of the selected output device, captured at selection time. */
  device_name: string | null;
  /** Dedicated headphone / preview output. null disables preview safely. */
  preview_device_id: string | null;
  /** Friendly name of the configured preview output. */
  preview_device_name: string | null;
  /** ASIO stereo pair for preview on the already-open main stream. */
  preview_asio_pair: number | null;
  /** Operator preview fader in dB, machine-local. */
  preview_gain_db: number;
  /** Selected audio input device for Mic Cues. null = system default input. */
  input_device_id: string | null;
  /** Samples. Only applied for WASAPI Exclusive. */
  buffer_size: number;
  /** ASIO output pair index (0 = Out 1-2, 1 = Out 3-4, …). */
  asio_out_pair: number;
  /** Machine-local physical bindings for logical Aux buses. */
  aux_buses: Array<{ bus_id: string; device_id: string; channels: number[] }>;
}

export const DEFAULT_MACHINE_AUDIO_CONFIG: MachineAudioConfig = {
  backend: "wasapi_shared",
  device_id: null,
  device_name: null,
  preview_device_id: null,
  preview_device_name: null,
  preview_asio_pair: null,
  preview_gain_db: 0,
  input_device_id: null,
  buffer_size: 256,
  asio_out_pair: 0,
  aux_buses: [],
};

/** Hardware settings comparison used by Preferences Apply. */
export function machineAudioConfigsEqual(a: MachineAudioConfig, b: MachineAudioConfig): boolean {
  return a.backend === b.backend
    && a.device_id === b.device_id
    && a.device_name === b.device_name
    && a.preview_device_id === b.preview_device_id
    && a.preview_device_name === b.preview_device_name
    && a.preview_asio_pair === b.preview_asio_pair
    && a.preview_gain_db === b.preview_gain_db
    && a.input_device_id === b.input_device_id
    && a.buffer_size === b.buffer_size
    && a.asio_out_pair === b.asio_out_pair
    && JSON.stringify(a.aux_buses) === JSON.stringify(b.aux_buses);
}

/** Machine-global audio defaults, mirrored in .inkue for compatibility. */
export interface AudioPreferences {
  default_volume_db: number;
  default_fade_out_ms: number;
  default_fade_curve: FadeCurve;
}

export type CueRowHeight = "compact" | "normal" | "tall";

export interface GeneralPreferences {
  double_go_protection_ms: number;
  confirm_before_delete: boolean;
  auto_scroll_to_playhead: boolean;
  cue_row_height: CueRowHeight;
  /** When true (the built-in default), cue numbers follow their list position. */
  auto_renumber_on_reorder: boolean;
  /** Colour assigned only when a new cue of this type is created. Missing
   * field uses the built-in palette; a saved map is preserved as-is. */
  default_cue_colors: Partial<Record<CueType, CueColor>>;
}

export const DEFAULT_GENERAL_PREFS: GeneralPreferences = {
  double_go_protection_ms: 500,
  confirm_before_delete: false,
  auto_scroll_to_playhead: true,
  cue_row_height: "normal",
  auto_renumber_on_reorder: true,
  default_cue_colors: {
    fade: "orange",
    group: "yellow",
    memo: "black",
    number: "yellow",
    pause: "red",
    resume: "green",
    start: "green",
    stop: "red",
  },
};

export type TimerPosition = "center" | "top_left" | "top_right" | "bottom_left" | "bottom_right";

/** How a cue's colour tag is rendered in the Cue List. */
export type CueColorStyle = "stripe" | "full_row";

export interface DisplayPreferences {
  /** Monitor index for the unified output surface. null = floating window. */
  output_screen: number | null;
  /** When true, a countdown timer is shown on the output window. */
  show_output_timer: boolean;
  /** When true the timer counts down (remaining); when false it counts up (elapsed). */
  timer_count_down: boolean;
  /** Font family for the OSD timer (e.g. "Arial"). */
  timer_font: string;
  /** Font size in mpv OSD points for the timer (default 120). */
  timer_font_size: number;
  /** Position of the timer on the output window. */
  timer_position: TimerPosition;
  /** When true, milliseconds are shown (e.g. 00:00.000). */
  timer_show_ms: boolean;
  /** Margin in pixels from the edge for corner positions. */
  timer_margin: number;
  /** When true (and show_output_timer is true), show timer as floating Win32 window instead of OSD overlay. */
  timer_floating: boolean;
  /** UI colour theme: "dark", "light", or "system". */
  theme: "dark" | "light" | "system";
  /** How a cue's colour tag is rendered in the Cue List. */
  cue_color_style: CueColorStyle;
  /** Global projector-alignment transform, composed on top of per-cue geometry. */
  output_transform?: OutputTransform;
  /** Named destinations; omitted by legacy workspaces and normalised to Main. */
  output_destinations?: OutputDestination[];
  default_output_id?: string;
  /** Machine-global Clip Editor dock visibility, mirrored in workspaces for compatibility. */
  show_live_panel?: boolean;
  show_slice_panel?: boolean;
  /** Last selected Clip Editor tab, mirrored in the workspace for compatibility. */
  clip_editor_active_tab?: "Live" | "Slice";
}

export type OutputSinkKind = "display" | "ndi" | "srt";
export type NdiQuality = "highest" | "low_bandwidth";
export type SrtMode = "listener" | "caller" | "rendezvous";
export interface NdiOutputSettings {
  enabled: boolean;
  stream_name: string;
  quality: NdiQuality;
  group: string;
}
export interface SrtSettings {
  enabled: boolean;
  mode: SrtMode;
  host: string;
  port: number;
  latency_ms: number;
  passphrase: string | null;
  stream_id: string | null;
  payload_size: number | null;
  too_late_packet_drop: boolean;
  bitrate_kbps: number | null;
  width: number | null;
  height: number | null;
  fps: number | null;
  codec: string | null;
}
export interface NetworkOutputSettings { ndi: NdiOutputSettings; srt: SrtSettings; }
export type TransportAvailability = "ready" | "unavailable" | "not_implemented";
export interface TransportProviderStatus {
  protocol: "ndi" | "srt";
  availability: TransportAvailability;
  detail: string;
  library_path: string | null;
}
export interface NetworkIoStatus { ndi: TransportProviderStatus; srt: TransportProviderStatus; }
export type NetworkOutputState = "disabled" | "waiting_for_frame" | "streaming" | "error" | "stopped";
export interface NetworkOutputRuntimeStatus {
  output_id: string;
  state: NetworkOutputState;
  submitted_frames: number;
  superseded_frames: number;
  last_error: string | null;
}
/** Volatile operator state for one physical or network video output. */
export interface OutputControlStatus {
  output_id: string;
  name: string;
  available: boolean;
  healthy: boolean;
  ftb: boolean;
  active: boolean;
  visible: boolean;
  monitor: number | null;
  network: boolean;
  detail: string | null;
}
/** Last normal geometry for a destination that uses a floating output window. */
export interface FloatingWindowGeometry {
  x: number;
  y: number;
  width: number;
  height: number;
  maximized: boolean;
}
export interface OutputDestination {
  id: string;
  name: string;
  sink_kind: OutputSinkKind;
  /** Kept for every destination; only the active sink kind is used. */
  network?: NetworkOutputSettings;
  monitor: number | null;
  /** Persisted by the native output window; unused for monitor-bound outputs. */
  floating_window?: FloatingWindowGeometry | null;
  enabled: boolean;
  transform: OutputTransform;
  fullscreen_locked: boolean;
  /** Keep the native window above other application windows (display outputs only). */
  always_on_top: boolean;
  /** Hide the pointer while it is over the native output window (display outputs only). */
  hide_cursor: boolean;
}

/** Clone a transform so draft edits never mutate a committed/default object. */
export function cloneOutputTransform(transform: OutputTransform = DEFAULT_OUTPUT_TRANSFORM): OutputTransform {
  return {
    pan_x: transform.pan_x,
    pan_y: transform.pan_y,
    scale: transform.scale,
    rotation: transform.rotation,
    corners: transform.corners.map(([x, y]) => [x, y] as [number, number]) as OutputTransform["corners"],
  };
}

/** Clone an output destination, including its nested calibration transform. */
export function cloneOutputDestination(destination: OutputDestination): OutputDestination {
  return {
    ...destination,
    // Old workspaces predate both window-behaviour options. Keep their
    // existing behaviour until the operator explicitly enables either one.
    always_on_top: destination.always_on_top ?? false,
    hide_cursor: destination.hide_cursor ?? false,
    transform: cloneOutputTransform(destination.transform),
    floating_window: destination.floating_window ? { ...destination.floating_window } : destination.floating_window,
    network: cloneNetworkOutputSettings(destination.network),
  };
}

export const DEFAULT_NETWORK_OUTPUT_SETTINGS: NetworkOutputSettings = {
  ndi: { enabled: false, stream_name: "Qlisa Program", quality: "highest", group: "" },
  srt: {
    enabled: false, mode: "listener", host: "", port: 9000, latency_ms: 120,
    passphrase: null, stream_id: null, payload_size: null, too_late_packet_drop: true,
    bitrate_kbps: null, width: null, height: null, fps: null, codec: null,
  },
};

/** Deep-copy and migrate partial network settings from pre-network workspaces. */
export function cloneNetworkOutputSettings(settings?: Partial<NetworkOutputSettings> | null): NetworkOutputSettings {
  const ndi = { ...DEFAULT_NETWORK_OUTPUT_SETTINGS.ndi, ...(settings?.ndi ?? {}) };
  const srt = { ...DEFAULT_NETWORK_OUTPUT_SETTINGS.srt, ...(settings?.srt ?? {}) };
  return { ndi, srt };
}

/** Stable browser-side id for a newly-created named output. */
let outputIdFallbackCounter = 0;
export function createOutputId(): string {
  const cryptoApi = globalThis.crypto;
  if (typeof cryptoApi?.randomUUID === "function") return cryptoApi.randomUUID();
  if (typeof cryptoApi?.getRandomValues === "function") {
    const bytes = new Uint8Array(16);
    cryptoApi.getRandomValues(bytes);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, "0")).join("");
    return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
  }
  outputIdFallbackCounter += 1;
  return `output-${outputIdFallbackCounter}-${Math.random().toString(36).slice(2, 10)}`;
}

/** Normalize legacy/current output preferences and select a valid enabled default. */
export function normalizeOutputDestinations(
  destinations?: OutputDestination[] | null,
  requestedDefaultId?: string | null,
  legacyMonitor?: number | null,
): { destinations: OutputDestination[]; defaultOutputId: string } {
  const source = destinations && destinations.length > 0
    ? destinations
    : [{ ...DEFAULT_OUTPUT_DESTINATION, monitor: legacyMonitor ?? null, transform: cloneOutputTransform(DEFAULT_OUTPUT_TRANSFORM) }];
  const normalized = source.map((destination) => cloneOutputDestination({
    ...destination,
    transform: destination.transform ?? DEFAULT_OUTPUT_TRANSFORM,
    network: cloneNetworkOutputSettings(destination.network),
    fullscreen_locked: true,
  }));
  const selected = getDefaultOutputDestination(normalized, requestedDefaultId) ?? normalized[0];
  return { destinations: normalized, defaultOutputId: selected.id };
}

/** Pick the effective default without ever selecting a disabled destination.
 * Program defaults intentionally remain display-only: a network destination
 * can be selected explicitly on a cue, but must not silently swallow every
 * unassigned visual cue while its sender worker is unavailable. */
export function getDefaultOutputDestination(
  destinations: readonly OutputDestination[],
  requestedDefaultId?: string | null,
): OutputDestination | undefined {
  const enabledDisplays = destinations.filter((destination) => destination.enabled && destination.sink_kind === "display");
  return enabledDisplays.find((destination) => destination.id === requestedDefaultId) ?? enabledDisplays[0] ?? destinations.find((destination) => destination.sink_kind === "display") ?? destinations[0];
}

/** Return the first duplicate monitor assignment among enabled display outputs. */
export function findDuplicateDisplayMonitor(destinations: readonly OutputDestination[]): { monitor: number; first: OutputDestination; second: OutputDestination } | null {
  const owners = new Map<number, OutputDestination>();
  for (const destination of destinations) {
    if (!destination.enabled || destination.sink_kind !== "display" || destination.monitor === null) continue;
    const first = owners.get(destination.monitor);
    if (first) return { monitor: destination.monitor, first, second: destination };
    owners.set(destination.monitor, destination);
  }
  return null;
}

/** Validate the operator-edited output list before any settings are persisted. */
export function validateOutputDestinations(destinations: readonly OutputDestination[]): string | null {
  if (destinations.length === 0) return "Добавьте хотя бы одно выходное назначение.";
  if (!destinations.some((destination) => destination.enabled)) {
    return "Включите хотя бы одно выходное назначение перед применением настроек.";
  }
  if (!destinations.some((destination) => destination.enabled && destination.sink_kind === "display")) {
    return "Включите хотя бы один экран: основной выход должен оставаться экраном.";
  }
  const ids = new Set<string>();
  const names = new Set<string>();
  for (let index = 0; index < destinations.length; index += 1) {
    const destination = destinations[index];
    const id = destination.id.trim();
    const name = destination.name.trim();
    if (!id || !name) return `У выхода №${index + 1} должны быть непустые ID и название.`;
    if (id !== destination.id || name !== destination.name) {
      return `У выхода «${name || id}» уберите пробелы в начале и конце ID и названия.`;
    }
    if (ids.has(id)) return `ID выхода «${id}» используется несколько раз.`;
    if (names.has(name.toLocaleLowerCase())) return `Название выхода «${name}» используется несколько раз.`;
    const network = cloneNetworkOutputSettings(destination.network);
    if (destination.sink_kind === "ndi" && destination.enabled && !network.ndi.stream_name.trim()) {
      return `У выхода NDI «${name}» должно быть имя потока.`;
    }
    if (destination.sink_kind === "srt" && destination.enabled) {
      if (!Number.isInteger(network.srt.port) || network.srt.port < 1 || network.srt.port > 65535) return `У выхода SRT «${name}» укажите порт от 1 до 65535.`;
      if (network.srt.latency_ms < 20 || network.srt.latency_ms > 10000) return `У выхода SRT «${name}» задержка должна быть от 20 до 10000 мс.`;
      if ((network.srt.mode === "caller" || network.srt.mode === "rendezvous") && !network.srt.host.trim()) return `У выхода SRT «${name}» в этом режиме нужен IP-адрес.`;
    }
    ids.add(id);
    names.add(name.toLocaleLowerCase());
  }
  const duplicateMonitor = findDuplicateDisplayMonitor(destinations);
  if (duplicateMonitor) {
    return `Монитор ${duplicateMonitor.monitor + 1} назначен выходам «${duplicateMonitor.first.name}» и «${duplicateMonitor.second.name}».`;
  }
  return null;
}

/** Resolve cue routing while accepting pre-multi-output documents. */
export function resolveOutputId(prefs: Partial<DisplayPreferences>, requested?: string | null): string {
  const outputs = prefs.output_destinations ?? [defaultOutputDestination()];
  if (requested && outputs.some((output) => output.id === requested && output.enabled)) return requested;
  return getDefaultOutputDestination(outputs, prefs.default_output_id)?.id ?? outputs[0].id;
}

/** Global projector-alignment transform (Preferences → Display). */
export interface OutputTransform {
  /** Horizontal offset as a fraction of the output width (−1..1). */
  pan_x: number;
  /** Vertical offset as a fraction of the output height (−1..1). */
  pan_y: number;
  /** Linear scale factor (1.0 = 100%). */
  scale: number;
  /** Clockwise rotation in degrees — fractional values supported (0.1° steps). */
  rotation: number;
  /**
   * Corner-pin offsets in fractions of the output size, storage order
   * TL, TR, BL, BR; positive = right/down. Applied after scale/rotation/pan.
   */
  corners: [[number, number], [number, number], [number, number], [number, number]];
}

/** Identity transform (engine defaults). */
export const DEFAULT_OUTPUT_TRANSFORM: OutputTransform = {
  pan_x: 0,
  pan_y: 0,
  scale: 1,
  rotation: 0,
  corners: [[0, 0], [0, 0], [0, 0], [0, 0]],
};

export const DEFAULT_OUTPUT_DESTINATION: OutputDestination = {
  id: "default", name: "Main", sink_kind: "display", monitor: null,
  network: cloneNetworkOutputSettings(),
  enabled: true, transform: cloneOutputTransform(DEFAULT_OUTPUT_TRANSFORM), fullscreen_locked: true,
  always_on_top: false, hide_cursor: false,
};
function defaultOutputDestination(): OutputDestination { return cloneOutputDestination(DEFAULT_OUTPUT_DESTINATION); }

/** Built-in calibration patterns; custom_image carries the file path. */
export type TestPatternKind =
  | "grid" | "smpte_bars" | "rgb_test" | "test_card"
  | "white" | "gray" | "black" | "custom_image";

/** Serialised form matching the Rust `TestPattern` enum (tag + content). */
export interface TestPattern {
  kind: TestPatternKind;
  /** Only for custom_image. */
  path?: string;
}

export const DEFAULT_DISPLAY_PREFS: Pick<DisplayPreferences, "theme" | "cue_color_style"> = {
  theme: "system",
  cue_color_style: "full_row",
};

export interface AppPreferences {
  audio: AudioPreferences;
  general: GeneralPreferences;
  network: Record<string, never>;
  display: DisplayPreferences;
}

// ---------------------------------------------------------------------------
// Events emitted by the backend
export interface CueListsChangedEvent {
  cue_lists: CueListSummary[];
  active_cue_list_id: string;
}

export interface CueStateChangedEvent {
  cue_id: CueId;
  old_state: CueState;
  new_state: CueState;
}

/** Emitted when the transport fires a cue; stopping it later does not undo history. */
export interface CueFiredEvent {
  cue_id: CueId;
}

export interface PlayheadMovedEvent {
  cue_id: CueId | null;
}

export interface WorkspaceModifiedEvent {
  /* empty */
}

export interface DeviceChangedEvent {
  devices: DeviceInfo[];
}

/** Key pressed while the output window has focus, forwarded by the backend. */
export interface OutputKeyEvent {
  key: string;
  ctrl: boolean;
  alt: boolean;
  shift: boolean;
  meta: boolean;
}

export interface CueTimeUpdateEvent {
  cue_id: CueId;
  elapsed_ms: number;
  action_elapsed_ms: number;
  remaining_ms: number;
  /** File-relative playhead for Audio/Video leaves; null for other cues. */
  media_position_ms: number | null;
}

/** Operator-local headphone preview timing. This never represents the show
 * playhead or a running cue's program audio. */
export interface PreviewPlayheadEvent {
  cue_id: CueId;
  /** File-relative source position, including preview click/drag seeks. */
  media_position_ms: number | null;
  playing: boolean;
  active: boolean;
  /** Monotonic preview lifecycle token; lower values are stale. */
  generation: number;
}
