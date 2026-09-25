# Audio bus routing

Qlisa uses one exclusive logical route per cue:

- `Main` is mandatory and uses the machine's main audio stream.
- `Aux` buses are optional. A cue selects `Main` or one enabled `Aux` bus.
- Preview/Headphones is an operator-only route. It is never part of cue bus
  selection, the program bus, program VU, or NDI/SRT audio taps.

`OutputPatch` remains the compatibility-facing Rust/IPC type, but its `kind`
and `enabled` fields define the logical bus model. Missing legacy role fields
are migrated by making the old default patch `Main` without changing its UUID.
Deleting Main is rejected.

Physical Aux bindings (`device_id` and `channels`) are machine-local. They are
stored in `audio.json` as `aux_buses`, keyed by the logical bus UUID. Workspace
serialization writes empty physical fields for Aux buses. On load, old
workspaces seed `audio.json` once, then the machine binding wins. This keeps
cue assignments portable while allowing each rig to patch its own hardware.
The Settings device selector lists the complete device inventory so an Aux can
be patched to any local endpoint. The Mixer intentionally lists only Main,
Preview/Headphones, and enabled Aux buses. An enabled Aux with no local binding
is shown as unavailable; preflight and GO return an error and produce silence,
never an implicit Main fallback.

Remaining migration debt:

- The compatibility field names `output_patch_id`, `output_patches`, and the
  `get_output_patches` command remain in project JSON and IPC contracts. They are
  intentionally not exposed as a second UI model.
- `OutputPatch` still carries runtime physical fields after load so the audio
  engine can build an immutable transport snapshot. New persistence strips
  those fields; a future schema can replace the compatibility names outright.
- Legacy matrix routes still use the resolved bus channels. A matrix remains
  inside one selected bus and cannot create a send to another bus.
- The existing Mic/Timecode cue fields retain `output_patch_id` for backward
  compatibility; their selectors use the same Main/Aux bus list.

ASIO preview-pair validation and isolation remain unchanged. The callback keeps
using bounded snapshots/rings, and the canonical program tap is built before
physical routing, so adding Aux bindings does not add callback locks or sends.
