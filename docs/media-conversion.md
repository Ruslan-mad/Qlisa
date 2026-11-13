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

Video conversion uses stream copy only for Optimize jobs with no size limit and
an H.264/yuv420p 8-bit video stream plus AAC audio or no audio. Size mode,
resize requests, and other codec combinations use the H.264/AAC encode preset.
The preset uses even YUV 4:2:0 output dimensions. It preserves
the source frame rate and display aspect ratio. A resize can adjust the sample
aspect ratio to keep the display aspect ratio unchanged.

Output validation checks that the expected audio or video stream exists and
that video and image dimensions are positive. Compatibility warnings such as
high frame rate or large dimensions do not reject a valid output. Unsupported
streams still fail validation. The converter accepts OGV and AIFC files when
the bundled FFmpeg runtime can decode them. SVG is not a supported converter
input.

Temporary output and diagnostic names use short unique IDs. The final output
name keeps the source name when it fits the Windows filename limit. The
converter shortens a long name at a Unicode character boundary when required.
Automatic replacement applies while the cue points to the job's original input.
If the cue already points to the job output, the action is idempotent. If the
cue has since been cleared or assigned another file, the job output stays
unapplied.
