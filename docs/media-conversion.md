# Media conversion

The Media Converter creates a separate output file. Replacing a cue assigns
that output path to the cue; it keeps the conversion job's input path so the
operator can restore the source later. Restore is allowed only while the cue
still points to that job's output. The cue must be stopped. Playback still
requires the original file to be available at its restored path.

Automatic replacement runs once on the worker's completion event. The event
includes `kind: "worker"`; status polls and later state events only update the
job display. Restore emits `kind: "applied-state"` with
`applied_to_cue: false`, so it cannot reapply the converted output. The UI also
keeps a per-subscription set of handled job IDs to ignore duplicate worker
completion events.

Audio, Video, and Image file assignments resolve cues by UUID, including cues
nested in Groups and Numbers. Video assignment clears duration values from the
previous file, invalidates metadata for the newly assigned path, and starts a
background duration/audio probe. A probe only updates the cue while the probed
path is still assigned. This prevents a delayed probe for the converted file
from overwriting runtime data after the source path has been restored.

Media dimensions shown in the cue summary come from the metadata cache keyed by
the resolved source path. A path change triggers a fresh metadata request; it
does not transcode or modify either media file.
