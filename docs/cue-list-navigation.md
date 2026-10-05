# Cue List navigation and Number grouping

The Cue List uses `flattenVisibleCueTree` for keyboard row order. Up/Down
selects the next visible row and sends that cue to the backend Playhead. With
no selection, either key selects the first visible row. Shift+Up/Down extends
the selection range; Ctrl/Cmd+Up/Down keeps the global Playhead navigation.
Space runs the cue currently on the backend Playhead.

Left collapses and Right expands a selected Group or Number when a Cue List row
has focus. Other focused editors, including video preview frame controls, keep
their arrow-key behavior. Double-clicking a Group or Number row expands or
collapses it; inline editors keep their own double-click behavior.

Creating a Number from selected cues inserts it at the root-level position of
the first selected cue in list order. The children retain cue-list order even
when selection order differs. Backend cue moves preserve the existing nested
membership and undo snapshots.

## Tree appearance

Groups and Numbers show thin tree lines in the cue list. Each child row branches from its parent, and a branch ends at the last visible child. Nested branches continue while an ancestor has later siblings. Collapsing a container hides its child lines with the rows.

Number container rows use a subtle surface tint to distinguish them from ordinary cue rows while keeping the table columns aligned.
