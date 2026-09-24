fn main() {
    // Keep Cargo's build-script cache in sync with platform bundle settings.
    // Tauri reads tauri.windows.conf.json while creating installers, but a
    // stale native build can otherwise make incremental release commands
    // appear up-to-date after resource mappings change.
    println!("cargo:rerun-if-changed=tauri.windows.conf.json");
    tauri_build::build();

    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();

    if target_os == "windows" {
        // The generic linker args also reach the lib unit-test harness, which
        // Cargo does not treat as a `rustc-link-arg-tests` target. The qlisa
        // binary already embeds Tauri's manifest resource, so suppress a second
        // generated manifest there.
        println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
        println!(
            "cargo:rustc-link-arg=/MANIFESTDEPENDENCY:type='win32' name='Microsoft.Windows.Common-Controls' version='6.0.0.0' processorArchitecture='*' publicKeyToken='6595b64144ccf1df' language='*'"
        );
        println!("cargo:rustc-link-arg-bin=qlisa=/MANIFEST:NO");
    }

    // macOS: the GL output path creates/manages its own NSWindow via raw `msg_send!`
    // (engine/output_engine/macos_window.rs), so AppKit must be linked. Foundation is
    // pulled in transitively by objc2-foundation.
    if target_os == "macos" {
        println!("cargo::rustc-link-lib=framework=AppKit");
    }

    // Copy libmpv-2.dll next to the compiled binary so it can be loaded at runtime.
    // OUT_DIR is target/{profile}/build/qlisa-<hash>/out — three levels up is target/{profile}.
    #[cfg(target_os = "windows")]
    {
        let manifest_dir = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let target_dir = std::path::Path::new(&out_dir)
            .ancestors()
            .nth(3)
            .unwrap()
            .to_path_buf();

        let dll_src = manifest_dir.join("vendor/mpv/libmpv-2.dll");
        let dll_dst = target_dir.join("libmpv-2.dll");
        println!("cargo:rerun-if-changed=vendor/mpv/libmpv-2.dll");

        if dll_src.exists() {
            let dll_len = std::fs::metadata(&dll_src).map(|m| m.len()).unwrap_or(0);
            if dll_len == 0 {
                // CI uses an empty placeholder for dependency/resource validation.
                // A release binary with that placeholder is unusable, so fail early
                // instead of shipping the misleading headless-mode banner.
                if std::env::var("PROFILE").as_deref() == Ok("release") {
                    panic!(
                        "vendor/mpv/libmpv-2.dll is empty; provide the official Windows libmpv build before a release build"
                    );
                }
                println!("cargo:warning=vendor/mpv/libmpv-2.dll is empty — video playback will fail at runtime");
            }
            if let Err(e) = std::fs::copy(dll_src, &dll_dst) {
                // The destination is locked while the app is running (`tauri dev`
                // holds libmpv-2.dll open). If a copy is already in place, keep going
                // rather than failing the whole build; otherwise it's a real error.
                if dll_dst.exists() {
                    println!("cargo:warning=libmpv-2.dll in use — keeping existing copy ({e})");
                } else {
                    panic!("Failed to copy vendor/mpv/libmpv-2.dll to target dir: {e}");
                }
            }
        } else {
            println!("cargo:warning=vendor/mpv/libmpv-2.dll not found — video playback will fail at runtime");
        }

        // FFmpeg is generated locally from the pinned archive for Windows
        // releases. NDI is loaded from the user's installed NDI Runtime and is
        // never copied into the application or installer.
        let stale_ndi_paths = [
            target_dir.join("Processing.NDI.Lib.x64.dll"),
            target_dir
                .join("resources")
                .join("ndi")
                .join("Processing.NDI.Lib.x64.dll"),
        ];
        for path in stale_ndi_paths {
            if path.exists() {
                if let Err(error) = std::fs::remove_file(&path) {
                    if std::env::var("PROFILE").as_deref() == Ok("release") {
                        panic!(
                            "Could not remove stale bundled NDI runtime {}: {error}",
                            path.display()
                        );
                    }
                    println!(
                        "cargo:warning=Could not remove stale bundled NDI runtime {}: {error}",
                        path.display()
                    );
                }
            }
        }

        for (relative, description) in [
            ("vendor/ffmpeg/ffmpeg.exe", "FFmpeg executable with SRT"),
            ("vendor/ffmpeg/ffprobe.exe", "ffprobe executable"),
            ("vendor/ffmpeg/LICENSE", "FFmpeg GPL licence"),
            ("vendor/ffmpeg/README-Gyan-build.txt", "FFmpeg build notice"),
        ] {
            println!("cargo:rerun-if-changed={relative}");
            let path = manifest_dir.join(relative);
            let length = std::fs::metadata(&path)
                .map(|metadata| metadata.len())
                .unwrap_or(0);
            if length == 0 {
                let message = format!("{description} is missing or empty at {}", path.display());
                if std::env::var("PROFILE").as_deref() == Ok("release") {
                    panic!("{message}; run scripts/sync-network-runtime.ps1 before creating a Windows release");
                }
                println!("cargo:warning={message}");
            } else {
                // Keep a runnable copy beside the direct `target/{profile}/qlisa.exe`.
                // Tauri also consumes the same vendor files from tauri.windows.conf.json
                // when producing the installer, so this does not change the bundle layout.
                let destination = relative.replacen("vendor/ffmpeg/", "ffmpeg/", 1);
                copy_runtime_resource(&manifest_dir, &target_dir, relative, &destination);
            }
        }
    }
}

#[cfg(target_os = "windows")]
fn copy_runtime_resource(
    manifest_dir: &std::path::Path,
    target_dir: &std::path::Path,
    relative_source: &str,
    relative_destination: &str,
) {
    let source = manifest_dir.join(relative_source);
    let destination = target_dir.join("resources").join(relative_destination);

    println!("cargo:rerun-if-changed={relative_source}");

    if let Err(error) = std::fs::create_dir_all(destination.parent().unwrap()) {
        panic!(
            "Failed to create runtime resource directory {}: {error}",
            destination.parent().unwrap().display()
        );
    }

    match std::fs::copy(&source, &destination) {
        Ok(_) => {}
        Err(error) if destination.exists() => {
            // A running Qlisa process can keep a DLL or executable open. Keep
            // the last known-good resource in that case so an incremental
            // rebuild does not fail just because the app is still open.
            println!(
                "cargo:warning=runtime resource in use — keeping existing {} ({error})",
                destination.display()
            );
        }
        Err(error) => panic!(
            "Failed to copy runtime resource {} to {}: {error}",
            source.display(),
            destination.display()
        ),
    }
}
