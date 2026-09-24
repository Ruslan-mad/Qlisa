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

    // Old builds copied media runtimes into target/. Remove only those exact
    // generated paths so incremental builds cannot leave stale payloads behind.
    // Runtime binaries are downloaded by the app into the user's LocalAppData.
    #[cfg(target_os = "windows")]
    {
        let out_dir = std::env::var("OUT_DIR").unwrap();
        let target_dir = std::path::Path::new(&out_dir)
            .ancestors()
            .nth(3)
            .unwrap()
            .to_path_buf();
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
        let stale_media_paths = [
            target_dir.join("libmpv-2.dll"),
            target_dir.join("resources/libmpv-2.dll"),
            target_dir.join("resources/ffmpeg/ffmpeg.exe"),
            target_dir.join("resources/ffmpeg/ffprobe.exe"),
        ];
        for path in stale_media_paths {
            if path.exists() {
                if let Err(error) = std::fs::remove_file(&path) {
                    if std::env::var("PROFILE").as_deref() == Ok("release") {
                        panic!(
                            "Could not remove stale media runtime {}: {error}",
                            path.display()
                        );
                    }
                    println!(
                        "cargo:warning=Could not remove stale media runtime {}: {error}",
                        path.display()
                    );
                }
            }
        }
    }
}
