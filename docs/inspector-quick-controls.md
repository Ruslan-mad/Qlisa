# Inspector quick controls

The Basics tab shows the same quick actions for single and multiple cue selections. Continue has three actions: do not continue, Auto-Continue, and Auto-Follow.

Multi-cue controls appear only when every selected cue supports the action. Audio and Video support loop. Audio, Video, and Camera support mute. Video, Image, and Camera support Fit by output. Continue is common to all cue types.

Mixed values are marked as mixed. The first loop or mute click enables the action for every selected cue. A click turns it off only when all selected values are already on. A Continue action sets the chosen mode for every cue.

Fit rows cover the union of outputs used by the selected cues. A Fit change applies only to cues routed to that output. It updates `fit_mode` while preserving each cue's other output geometry, including crop and position. Routing and output assignment do not change.

Loop changes require every selected cue to be editable while stopped or reset. The control stays disabled and shows a hint while the backend marks any selected cue as unsafe to rebuild. Mute, Continue, and Fit use the existing bulk update path.
