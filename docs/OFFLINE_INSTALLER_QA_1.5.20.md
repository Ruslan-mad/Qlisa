# Windows offline installer QA: Qlisa 1.5.20

The manual [Windows offline installer QA run](https://github.com/Ruslan-mad/Qlisa/actions/runs/40234786852) passed for tag `v1.5.20` on runner image `win25-vs2026` (`20261021.278.1`). The report had no errors.

The test verified both Tauri signatures and the `latest.json` entry. It removed WebView2 and NDI before installation and confirmed that no runtime cache existed. The full installer completed with all non-loopback network adapters disabled and no default route. Qlisa then opened its main UI; FFmpeg, ffprobe, libmpv, Vulkan, NDI, and SRT reported ready. A warm restart also passed.

The compact updater installed over the full package while the network remained disabled. The app opened after the update. SHA-256 values for FFmpeg, ffprobe, libmpv, and Vulkan matched before and after the update. NDI remained available.

The runner had no audio output device and no usable video-rendering context. Logs show silent audio mode and a failed OpenGL renderer initialization. This run verifies installation, offline runtime seeding, app startup, readiness reporting, and cache preservation. It does not verify physical audio or video playback, or hardware GPU acceleration.
