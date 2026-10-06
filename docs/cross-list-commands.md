# Cross-list commands

This note describes the cross-list target model in the current source tree. The
manual checks below are a checklist; no result is implied until each check is
run on the target desktop build.

## Ownership and identity

Each Cue belongs to one Cue List. Each list owns a separate Playhead. Cue UUIDs
identify targets across the loaded workspace, including cues nested in Group
and Number trees. The active list is the list shown in the current tab.

Start, Pause, Resume, Load, Reset, Goto, Stop, Fade, Devamp, Arm, and Disarm
targets are stored as cue UUID arrays on their source cue. Project serialization
keeps those arrays in the existing cue data; it does not add target list IDs.
Moving a target between lists therefore preserves the reference while the UUID
remains in the workspace. Removing a target leaves a dangling UUID that
workspace validation reports.

Legacy number-only target fields remain supported. Cue numbers are local labels,
so a legacy number resolves only in the source cue's list. Two lists can both
contain cue number `1` without changing that fallback. New or edited targets
should use UUIDs.

## Runtime behavior

- A linked command operates on the target's owning list. It does not select that
  list as the active tab.
- Goto changes the target list's Playhead. It does not start the target cue.
- Start starts the target cue without parking its list's Playhead on the Start
  target. Auto-Continue or Auto-Follow then advances the target's owning list
  Playhead as usual.
- Stop, Fade, and Devamp use the target cue's runtime voices and state. Group
  descendants are resolved recursively.
- An empty Stop target array and ordinary Soft Stop All retain their existing
  source/active-list scope. Hard Stop All remains workspace-wide.
- `get_all_cues` remains the active-list table contract. The workspace cue
  catalog supplies the lightweight cue trees for all lists, including empty
  lists and runtime state needed by target pickers.
- `playhead-moved` events include `cue_list_id` when the owner is known. Older
  ownerless events still mean the active list.

Automatic continuation follows the target cue's own list order and can advance
that list's Playhead. It does not select that list as the active tab.

## Local fixture

The ignored native-app fixture is at
`fixtures/cross-list-native/cross-list-native.qlisa`. Its small media files are
under `fixtures/cross-list-native/tmp/go-latency/`.

- **A — Commands** has Start → B/1 and Goto → B/2.
- **B — Remote targets** has an image in row 1 and a long Wait in row 2.
- **C — Media** has a one-second tone in row 1 and a short uncompressed AVI in
  row 2.

Open the `.qlisa` file in a development or installed desktop build. This fixture
is local test data and is not part of the project or release payload.

## Manual checks

Mark each item only after running it on the intended desktop build.

## Verification status

2026-10-07 source-tree checks passed: 503 frontend tests across 72 files, 1,010
Rust tests passed with 11 ignored, and `pnpm tauri:check` passed. These checks
cover the working tree, not a released installer. The browser review used mock
IPC and covered UI only; it does not verify linked-command IPC or playback. No
native GUI smoke test or cross-list end-to-end playback test was run. The
installed user app remained open, and the manual checks below remain unchecked.

- [ ] Press GO on A/1. Confirm B/1 starts and the active tab stays A.
- [ ] Compare B's Playhead before and after A/1. Confirm Start does not park it
  on B/1. With Auto-Continue or Auto-Follow enabled, confirm successors and
  Playhead advances follow B's normal sequence.
- [ ] Press GO on A/2. Confirm B's Playhead moves to B/2 and the active tab stays
  A. Confirm the Wait does not start from Goto alone.
- [ ] Open Active Cues and verify remote playback state updates. Pause, resume,
  and stop the remote cue by its UUID.
- [ ] Switch to C and play the tone and AVI. Confirm media paths resolve from the
  project directory and playback can be stopped.
- [ ] Save, close, and reopen the project. Confirm target UUIDs and list
  ownership persist.
- [ ] Give a different cue in C the same number as a target in B. Confirm a
  legacy number-only command still resolves within its source list.
- [ ] Delete a referenced target. Confirm workspace validation reports a
  dangling target instead of binding another cue with the same number.
- [ ] Exercise existing Cart, MIDI, and Timecode GO paths. Confirm their
  Playhead and automatic-transition behavior remains unchanged.
