# Live video output toggles: feasibility assessment

This note assesses live output toggles. It does not describe implemented
behavior.

## Decision — 2026-10-22

Keep current video-output behavior. Do not implement live output toggles in this
work. This assessment remains research only.

## Current behavior

The Inspector already displays output checkboxes for Video cues
(`src/components/Inspector/OutputSelector.tsx:105-178`). A click sends both
`output_id` and `output_ids` through `InspectorPanel` (`InspectorPanel.tsx:293-302`).
Those fields are persisted routing, not a live runtime mask.

The backend classifies routing as a rebuild edit
(`src-tauri/src/commands/cue_cmds.rs:1240-1261`). Rebuilds pass through
`ensure_rebuild_safe` in `update_cue` and bulk updates
(`cue_cmds.rs:3828-3882`, `1816-1827`). Running cues fail that lifecycle guard;
the existing test explicitly covers a stopped routing rebuild and rejection on
a running cue (`cue_cmds.rs:2339-2368`). The Inspector routing checkboxes do not
currently receive a running-state `disabled` flag, so the UI can offer an edit
that the backend rejects.

At GO, `VideoCue::start_video_action` submits one shared audio voice, then calls
`show_content_multi_with_geometry` with the selected output IDs
(`src-tauri/src/cue/video_cue.rs:523-557`). That function creates one video
voice per destination, attaches the audio voice only to the first, stores the
voice list under its first member as the canonical primary, and initializes
per-group completion tracking (`src-tauri/src/engine/output_engine/mod.rs:2095-2143`).
The primary is also returned to the cue as its active voice.

Lifecycle and live visual operations fan out from that primary. In particular,
`stop_content` removes the group by its primary and stops all members
(`output_engine/mod.rs:2266-2298`); `set_layer_props` and
`set_voice_opacity` apply to every member (`2440-2477`). Completion is emitted
once for the primary after all members complete (`3165-3205`). Removing the
first member from the vector/map would therefore risk losing the canonical cue
voice, attached audio ownership, or completion bookkeeping. Hiding the
primary's picture while retaining its voice avoids those failures.

`LayerStyle.opacity` and the compositor's zero-opacity skip can hide a layer
(`output_engine/slot.rs:1278-1305`, `output_engine/render.rs:1824-1831`), but the
current group API applies opacity to all outputs. It also shares state with
authored opacity and fades, so it is not a safe per-output toggle as-is. Global
FTB and physical-window show/hide controls affect an entire destination, not a
cue on that destination. No existing per-cue, per-output visibility mask was
found.

## Options

**Minimal: show/hide an output member already created at GO.** Add a transient
visibility override addressed by the canonical cue voice and destination ID.
Keep each member in `voice_groups`; do not stop or unload it. Apply the override
at the compositor layer so it does not alter authored opacity, fade state,
geometry, audio, or decoder position. This supports switching among outputs
selected when playback started. It does not add an output that was not active
at GO. Estimate: a medium task across the output engine and Inspector command
path, with focused state/render tests.

**Full: change routing membership during playback.** This must create or retire
destination voices while preserving the cue's canonical primary. Adding a
voice requires loading its slot, seeking it to the current trimmed media
position, matching pause/loop/hold/fade state, and avoiding another audio
voice. Removing a member must define how it leaves completion tracking and
handle the first/audio-owning member without changing canonical identity. The
command must serialize against GO, Stop, completion, and output retirement;
late events from retired members must not complete or fail the cue. Estimate:
an architectural change across cue commands, VideoCue runtime state,
OutputEngine group bookkeeping, and tests. Risk is high because timing and
voice ownership cross those layers.

Preloading every possible output is not a safe shortcut. Each active destination
has its own pipeline, mpv context, slot, and rendering work; selected destinations
also start their output/network workers. The repository documents independent
destination pipelines in `docs/qlisa-multi-output.md:13-20` and per-output sender
workers at `:68-73`. There is no resource measurement here that would justify
creating hidden voices for every configured output.

## Future considerations

The current decision is to keep video-output behavior as it is. Do not treat
either option below as planned work. If a separate product decision requests
live output controls, use these findings to scope that work. The minimal
visibility override is smaller than changing routing membership, but it still
needs its own design, implementation, and tests.

Checks for a future minimal visibility option:

- Toggle an already active secondary output off and on. The frame returns at
  the current position without a seek, restart, or cue rebuild.
- Preserve playback clock, pause/resume, trim, loops, fade, geometry, and the
  paired audio voice. No duplicate or interrupted audio occurs.
- Toggle the primary output off and on. Audio, cue identity, and canonical
  voice remain active; no cue completion or stale completion event is emitted.
- Stop, natural completion, and output reconfiguration still clean up every
  hidden and visible member exactly once.
- Saved routing remains unchanged by the transient toggle and still applies
  on the next GO.

Files a future minimal option would likely touch: `src-tauri/src/engine/output_engine/`
(`slot.rs`, `render.rs`, `mod.rs`), `src-tauri/src/commands/cue_cmds.rs`,
`src-tauri/src/cue/traits.rs` or VideoCue runtime state, plus
`src/components/Inspector/OutputSelector.tsx`, `InspectorPanel.tsx`,
`src/lib/commands.ts`, `src/lib/types.ts`, translations, and focused tests.
Full membership changes would also need more extensive updates to
`video_cue.rs`, group completion handling, error/GC paths, and transport tests.
