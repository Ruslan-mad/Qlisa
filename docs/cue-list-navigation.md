# Cue List navigation and Number grouping

The Cue List uses `flattenVisibleCueTree` for keyboard row order. Up/Down
selects the next visible row and sends that cue to the backend Playhead. With
no selection, either key selects the first visible row. Shift+Up/Down extends
the selection range; Ctrl/Cmd+Up/Down keeps the global Playhead navigation.
Space runs the cue currently on the backend Playhead. Selection and Playhead
remain separate state, identified by cue UUIDs.

Selection and Playhead are separate state. Plain row click selects one cue and
queues its Playhead update. Ctrl/Cmd-click toggles a cue; Shift-click selects an
inclusive range. Up/Down uses the same Playhead update queue. Dragging through
empty space below the rows or through the former narrow left gutter no longer
selects a range. Reorder drag from a row remains available.

Mouse Playhead updates and GO share one command queue. A click on cue A, GO,
then a click on cue B keeps that order: A becomes Playhead, GO starts A, then B
becomes the next Playhead. Rapid clicks apply in order, so the final click sets
the next Playhead. GO waits for the positioning operation and reports its
failure instead of starting the previous cue. A queued GO is canceled by Stop,
selected Stop, Panic, or a workspace/list transition. A queued Playhead update
from an old workspace is canceled when that transition starts.

Current local GO follow behavior is unreleased and is not part of Qlisa 1.5.14.
After a successful toolbar, Space, or frontend OSC GO, the UI updates selection
and Playhead together to the cue reported as fired. Stale results cannot replace
a newer manual selection or cross a cue-list/workspace change. Ignored, canceled,
or failed GO does not change selection. This frontend rule does not promise
selection updates for backend-only OSC triggers.

The editor's former Timeline tab is now Preview. Preview keeps audition and
editing, with no runtime cursor or program seek. Live is a separate tab that
lists running and paused file-backed cues across lists, ordered by latest cue
fired. It omits protected background Numbers and their descendants. Its manual
dropdown choice is sticky; GO and cue selection do not change the tab, and an
empty Live view stays open. Live can runtime-seek the selected file. These are
current local changes, not published 1.5.14 behavior. The `clip_editor_active_tab_v2`
preference stores `Preview`, `Live`, or `Slice`; legacy `show_live_panel` and an
older saved Live tab both migrate to Preview.

The upper toolbar's Panic button performs Stop All on one click and Hard Stop
All on a second click within 500 ms. Double Escape also performs Hard Stop All.
The bottom STOP button and S shortcut stop the selected cue set.

Escape counts separate physical key presses. The first press requests soft Stop
All for the current list; a second press within 500 ms requests an immediate
Hard Stop All across the workspace. Keyboard auto-repeat is ignored and does
not count as a second press. Soft Stop uses `default_fade_out_ms` for Audio,
Video, Image, and Camera cues when the cue has no explicit fade value. An
explicit value of `0` takes priority over the preference. Video applies its
visual and audio fades separately. This describes code behavior; physical
audio hardware and capture devices are not validated here.

Left collapses and Right expands a selected Group or Number when a Cue List row
has focus. Other focused editors, including video preview frame controls, keep
their arrow-key behavior. Double-clicking a Group or Number row expands or
collapses it; inline editors keep their own double-click behavior.

Creating a Number from selected cues inserts it at the root-level position of
the first selected cue in list order. The children retain cue-list order even
when selection order differs. Backend cue moves preserve the existing nested
membership and undo snapshots.

An active Group or Number appears as one row in Active Cues; its active
descendants remain available to global Pause/Resume. The Active Cues row's
explicit Stop addresses that container and can stop a protected background
Number. Ordinary Stop, Hard Stop, and Number stop-on-start preserve a Number
marked as a background screensaver and its protected subtree. Global Panic also
preserves those protected voices. These are code-path guarantees; this note does
not claim hardware playback validation.

## Tree appearance

Groups and Numbers show thin tree lines in the cue list. Each child row branches from its parent, and a branch ends at the last visible child. Nested branches continue while an ancestor has later siblings. Collapsing a container hides its child lines with the rows.

Number container rows use a subtle surface tint to distinguish them from ordinary cue rows while keeping the table columns aligned.
