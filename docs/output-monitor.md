# Output Monitor

Output Monitor previews the final composition of one physical display output.
It uses the selected output's existing renderer. It does not create another
decoder or capture the desktop. The preview is limited to 640 × 360 pixels.
Browser Cue content is not part of this OpenGL composition and may be absent
from the preview.

## Frame path

The render thread draws the final output texture into a small monitor FBO. The
draw includes the output warp and fade layers. A three-slot pixel buffer object
(PBO) ring queues BGRA readback with a GPU fence. The renderer checks fences
without waiting. It selects the newest ready frame from the current selection
session before mapping a PBO, maps at most one frame per pass, and drops older
ready frames. If no PBO is free, it skips the monitor capture.

The CPU frame is top-down BGRA. The renderer publishes it to a one-frame
mailbox. The mailbox stores an `Arc` snapshot; renderer publication uses
`try_lock` and drops the monitor frame if another thread holds the mailbox.
This keeps monitor work from waiting on a frame reader.

The frontend pulls frames through one Tauri invoke at a time. It does not queue
requests. After a request completes, the next starts after the remainder of
the 33.33 ms target interval, or immediately if the request took longer. While
the monitor is active, the render thread schedules capture deadlines at up to
30 FPS, including for a static output. Animated output keeps its existing
faster render wakeups. With the monitor inactive, the renderer keeps its
existing idle timeout.

Selection tokens identify a monitor session. A late command with an older
token is rejected. PBO frames from earlier sessions and frames older than the
last published capture sequence are discarded. Capture sequence numbers are
assigned to capture attempts, so gaps are normal when a busy PBO ring skips an
attempt. Sequence numbers use wrapping serial order, and the mailbox always
keeps the newest frame.

## Binary packet, version 1

The command returns `tauri::ipc::Response::new(Vec<u8>)`. The frontend receives
an `ArrayBuffer`. All integers are little-endian. The fixed header is 64 bytes.

| Offset | Size | Field |
| ---: | ---: | --- |
| 0 | 4 | ASCII magic `QLMF` |
| 4 | 2 | Version: `1` |
| 6 | 2 | Header size: `64` |
| 8 | 1 | Status: `0` no frame, `1` frame, `2` unchanged, `3` black |
| 9 | 1 | Pixel format: `1` BGRA8 |
| 10 | 2 | Reserved; must be zero |
| 12 | 4 | Width in pixels |
| 16 | 4 | Height in pixels |
| 20 | 4 | Stride in bytes |
| 24 | 8 | Capture sequence |
| 32 | 8 | Selection session token |
| 40 | 8 | Capture time, Unix microseconds |
| 48 | 8 | Response time, Unix microseconds |
| 56 | 4 | Backend packet preparation duration, microseconds |
| 60 | 4 | Payload length in bytes |

The payload starts at offset 64. Status `frame` carries tightly packed,
top-down BGRA8 pixels with `stride = width × 4` and
`payload length = stride × height`.
Status `black` carries width, height, and stride but no pixel payload. Statuses
`no frame` and `unchanged` have zero width, height, stride, and payload length.
An unchanged response retains the latest capture sequence and capture time.
The packet parser validates the complete header and payload length before it
exposes the pixel view.

The Canvas expects RGBA, so the frontend swaps red and blue while preparing the
image data. It reuses the Canvas element and preserves the frame on an
`unchanged` response. A `black` response clears the Canvas to black.

## Diagnostics and limits

Capture diagnostics include attempts, PBO captures, skipped captures when all
PBO slots are busy, dropped stale frames, published frames, estimated capture
FPS, CPU map/copy total and maximum time, copy sample count, and mailbox lock
drops. The capture FPS is the session's published-frame count divided by time
since its first capture attempt. Frontend diagnostics include received and
displayed FPS, frame age, BGRA-to-RGBA conversion time, and request time.
Packet diagnostics include payload bytes and backend preparation time.

These timings describe Qlisa's capture and preview path. The CPU map/copy time
includes mapping a signaled PBO and copying/flipping rows; it is not GPU
execution time. The diagnostics do not measure physical display scanout, LED
latency, or Canvas presentation time. Frame age is the age inside Qlisa from
capture timestamp to frontend processing, not the time at which a display
emitted the image.

`QLISA_MONITOR_CAPTURE_FPS` is a renderer debug cap, read once per process. Its
default is 30 and accepted values are clamped to 1–30. Leaving the source
unselected disables capture. A cap of zero is treated as one FPS; it does not
disable the monitor.

## Native benchmark

Run from `src-tauri`. Supply a local video or image path. The example creates a
temporary `APPDATA` profile so its Tauri settings do not use the normal
operator profile.

```powershell
cargo run --release --example output_monitor_bench --features asio-support -- "C:\media\test.mp4" 30 off 1
cargo run --release --example output_monitor_bench --features asio-support -- "C:\media\test.mp4" 30 4 1
cargo run --release --example output_monitor_bench --features asio-support -- "C:\media\test.mp4" 30 30 1
```

The modes are capture off, 4 FPS, and 30 FPS. The final argument selects one
to three compositor layers. On Windows, include `--features asio-support` to
match the application build; omit that feature on other operating systems.
The benchmark exercises the native renderer,
mailbox, and production packet encoder. It does not exercise Tauri IPC, the
WebView, Canvas conversion, or physical display scanout. Record measured
results separately; the command itself is not evidence that a performance
target passed.
