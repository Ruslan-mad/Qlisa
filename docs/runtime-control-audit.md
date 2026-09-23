# Runtime control and cue-lifecycle report

Status: the eight GO/STOP/PAUSE/seek/continue findings from the earlier
read-only audit have been addressed in the current source tree. The fixes were
verified with focused Rust tests on **2026-09-18**. The desktop app was not
launched, and no installer/package was built as part of this runtime-control
pass.

## Root cause

The reported failure combined two separate state machines. Media seeking moved
the cue's displayed/action position, while the audio/output engine still owned
the actual voice. The event loop could treat the moved action clock as natural
completion and reset the cue before the engine had ended its voice. Separately,
the playlist's delayed Auto-Follow bookkeeping identified a cue/list, not the
particular cue execution that scheduled the deadline. Group children also
entered through direct `go()` calls, bypassing the transport's action dispatch.
Those gaps affected neighboring Stop, Start, Fade, Devamp, pause, and continue
paths as well as the reported Video → Audio → Stop sequence.

## Findings and fixes

1. **Group action cues now use the shared transport dispatcher.** Group-fired
   children are recursively resolved and dispatched for Start, Stop, Pause,
   Resume, Load, Reset, Goto, Fade, Devamp, Arm, and Disarm. This also applies
   to absorbed Group GO and later children fired by Group ticks. The dispatcher
   guards recursive Start cycles and ancestor restarts, preserves the existing
   `GoResult` contract, deduplicates starts in one dispatch batch, and suppresses
   queued actions canceled by an earlier Stop/Reset.

2. **A seeked media clock no longer drops a live voice.** Event-loop completion
   now accounts for voice ownership/liveness before resetting Audio or Video;
   Group children use the same ownership check. A stale `Completed` report is
   ignored while that voice is still alive, including after seeking back. Reset
   and Stop no longer orphan a paused Group child: recursive paused descendants
   count as active work.

3. **Delayed Auto-Follow is execution-bound.** Pending entries capture source
   generation, an explicit fired marker, cue-list identity/order, and expected
   Playhead. Stop/Reset, retrigger, Goto/Playhead change, list switch, or cue
   structure change invalidates stale work. Formerly marker-less instant cue
   types (Stop, Control/Start, Devamp, OSC, MIDI, Script, and Memo) now carry
   execution markers too.

4. **Stop resolves nested cues.** Specific Stop targets are found recursively;
   Stop All also reaches active descendants even when a containing Group's
   state is stale. Recursive stopping avoids duplicating the reported cue IDs.

5. **Start executes action targets rather than only changing their state.** A
   Start aimed at a Stop/Fade/Devamp/Control cue dispatches that action through
   the shared path. Disarmed targets are skipped, and recursive/self/ancestor
   Start cycles are rejected before invoking the target.

6. **Groups skip disabled children.** Simultaneous, Sequential, Playlist, and
   random Group starts do not start disabled children; sequence advancement
   proceeds to the next enabled child.

7. **Pause freezes supported clocks.** Audio, Video, MIDI File, Mic, Image,
   Text, Wait, Group, and Fade timing/pre-wait clocks preserve elapsed time over
   Pause/Resume. Group source deadlines freeze while their child is paused.
   Camera and Timecode retain their designed no-op pause behavior where the
   underlying live/timecode source is not pausable. A native fade already
   submitted to an engine may continue visually/audibly while the cue's own
   timing is paused; that engine-level behavior is not equivalent to pausing a
   media voice.

8. **Auto-Continue uses action time, not the scrub position.** Audio, Video,
   and MIDI File expose a monotonic, pause-aware continuation clock separate
   from the seekable progress/action clock. Seeking forward or backward does
   not move the Auto-Continue deadline; the UI still follows the media
   position. Sequential Group Auto-Continue polls this same source clock on
   ticks instead of using a wall-clock timer, so a pause/resume between polls
   cannot consume the remaining delay. Auto-Continue is measured from action
   start and can overlap the current child; Auto-Follow begins after completion
   and then applies its post-wait.

## Cancellation and operator behavior

Delayed Group Auto-Follow/Playlist advancement is bound to the source child,
generation, marker, and expected current child. If Stop/Reset invalidates a
pending deadline while the Group remains on that same child, the Group parks at
that sequence boundary instead of silently arming a replacement post-wait. A
manual GO (or a new Playhead placement) is required to proceed. A new source
execution has a new generation and gets its own continuation timeline.

## Verification

The serial verification used the one-off diagnostic settings
`CARGO_INCREMENTAL=0` and `CARGO_BUILD_JOBS=2` from `src-tauri/`. These are not
normal build settings; see [the project guide](PROJECT_GUIDE.md):

- `cargo test --test runtime_control_regressions -- --nocapture`:
  **28 passed, 0 failed**. Includes the exact
  video/action/seek ownership cases, nested dispatch and cancellation, Group
  AutoContinue/AutoFollow timing, pause freezing (including pause+resume between
  sparse Group polls), and Stop/Reset cancellation across a second full
  post-wait interval.
- `cargo test --test transport_go_tests -- --nocapture`:
  **27 passed, 0 failed**.
- `cargo test --lib show::event_loop::tests`: **9 passed, 0 failed** (618
  filtered out), including the pending Auto-Follow status/token policy for an
  instant Memo cue.
- `cargo check --lib`: **passed**.
- `git diff --check`: **passed**.

These are deterministic/fake-engine tests and a library check, not a physical
device or packaged-app test. The reported looping Video #1 → separate
Auto-Follow Audio #2 → Stop #3 scenario should still be manually confirmed on
the target machine with its real output/audio devices. NDI/SRT, hardware audio,
and OS-specific output behavior were not exercised here.

## Remaining policy and risks

- Seeking controls the media playhead/progress display. It does **not** rewind
  or fast-forward Auto-Continue's transport clock. A seek to the displayed end
  does not complete a cue while its engine voice is alive; after the engine has
  actually completed, seek alone does not resurrect that voice—a new GO is
  required.
- Camera/Timecode pause remains a no-op by design. Engine-submitted fades are
  not guaranteed to pause with the cue clock; validate their expected behavior
  separately if operator requirements change.
- Verification did not include the frontend test/build, a Tauri dev launch,
  device disconnect/reconnect, physical NDI/SRT receivers, or a release bundle.

See [PROJECT_GUIDE.md](PROJECT_GUIDE.md#runtime-control-status-post-fix) for
the concise handoff status and relevant source areas.
