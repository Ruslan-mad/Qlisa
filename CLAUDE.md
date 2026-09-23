# Qlisa contributor and coding-agent guide

Before implementation, read [PROGRESS.md](PROGRESS.md) and
[docs/PROJECT_GUIDE.md](docs/PROJECT_GUIDE.md). The project guide is the
architecture and operational handoff; this file lists rules to preserve while
changing the code. Treat external StageCUE research notes as research, not as a
specification of Qlisa.

## Project identity and stack

Qlisa is a community-maintained GPL-3.0-or-later fork of Inkue. It uses Tauri 2,
React/TypeScript, Rust, cpal/Symphonia for audio, and runtime-loaded libmpv with
an OpenGL compositor for visual outputs. `.inkue`, `/inkue/*`, serde fields,
Tauri command names, event names, and internal `inkue` strings may be
compatibility contracts; inspect before renaming. The release command keeps the
version synchronized in `package.json`, `src-tauri/Cargo.toml`,
`src-tauri/tauri.conf.json`, and `src-tauri/Cargo.lock`.

The current application version is 1.5.2. The planned public source URL is
<https://github.com/Ruslan-mad/Qlisa>; until publication, the source is only in
the local checkout. Binary releases and in-app updates are not available yet.
The updater stays disabled until Qlisa has its own signing key and update feed.
Never use Inkue updater metadata.

## Non-negotiable architecture rules

1. **Keep the layers distinct.** `engine/` implements audio/output/device
   primitives; `cue/` owns cue lifecycle/serialization; `show/` owns workspace,
   cue list, playhead, transport and timed chaining. Commands adapt IPC requests.
2. **Use the Cue contract and registry.** Cue implementations go through
   `cue/traits.rs` and `CueRegistry`. Register a new cue in
   `state/app_state.rs`; keep its serde representation, shared TypeScript DTO,
   inspector/UI, and QLab import/export mapping in sync. Do not put special
   playback behavior into the cue-list view.
3. **Preserve real-time audio safety.** The cpal callback must not allocate,
   block on a mutex, perform I/O, log, or call Tauri. Communicate with it via
   bounded ring buffers, atomics, and prebuilt snapshots. Do not make UI/show
   state depend on callback-side locks.
4. **Keep slow work off the UI/transport path.** Media decode/probe, file I/O,
   network setup and blocking device discovery belong in worker paths. Dedup
   background work and reject stale asynchronous results with generations where
   the owning subsystem already uses that pattern.
5. **Maintain event-driven UI state.** All frontend invokes should use
   `src/lib/commands.ts`; shared DTOs belong in `src/lib/types.ts`; subscribe to
   backend changes in `src/hooks/useTauriEvents.ts`. Prefer a backend event over
   a tight polling loop.
6. **Respect native window threading and capabilities.** Follow the owning
   output/window module for main-thread requirements. A new Tauri window may
   require its own `src-tauri/capabilities/*.json` permissions and config entry.
7. **Keep workspace and machine settings separate.** Shows and project data
   travel in `.inkue`; the authoritative `AppPreferences` tree is machine-global
   in `%APPDATA%/Inkue/preferences.json` (with audio hardware in `audio.json`
   and OSC/network/TC/MIDI in their existing machine files). The serialized
   `Workspace.preferences` tree remains a compatibility/runtime mirror only;
   settings must never dirty a project. Legacy migration seeds the global file
   from the first real workspace, then merges only unknown output IDs while
   preserving existing global values and default output.

## Cue and transport behavior to preserve

- Cue numbers are strings and are not identities. UUID `CueId` identifies a
  cue; UI selection and the GO Playhead are separate state.
- GO advances the Playhead before firing. Disabled cues are skipped. Stop
  actions execute before continue-chain evaluation. Auto-Continue with zero
  post-wait may recurse immediately; other continue behavior is advanced by
  `show/event_loop.rs`.
- Soft STOP and hard stop have different semantics. Hard Stop All is an engine
  backstop as well as a cue loop so stale/untracked voices cannot keep playing.
- Pause freezes elapsed/action position; media seek is supported while paused
  where the cue implements it. Preserve pre-wait/post-wait and completion state.
- Group cues own child cues/voices. Group target resolution and cleanup must be
  recursive (`Cue::all_voice_ids`, `CueList::get_recursive`); do not assume one
  cue maps to one top-level audio voice.
- Group modes: Simultaneous, Sequential, Playlist, and Start Random. Sequential
  and Playlist can retain the outer Playhead and absorb GO. Playlist is
  exclusive; Start Random consumes a shuffled bag before repeating children.
- Video picture and its separate audio-engine voice are one user-visible cue.
  Keep their start, pause/resume, seek, loop/slice, fade, stop, and natural-EOF
  handling coordinated. A visual fade targets that cue's layer opacity; do not
  fade unrelated compositor layers.
- Visual cues generally stack. Text's next-GO behavior is cue-specific. Inspect
  `stop_on_next_go()` and whether the incoming cue is visual before changing it.
- Out-of-band state changes must emit the state/list events the UI relies on;
  otherwise cue rows and playhead/highlight state can become stale.
- Audio/video slices use file-relative coordinates; playback trim/action time
  can be different. Infinite slice play count is `u32::MAX` and Devamp releases
  the active vamp according to its target semantics.
- Preview audio is separate from show playback/workspace/playhead and has a
  single active session. Preserve the preview generation guard so stale
  decodes/completions cannot replace a newer preview.

## Persistence and compatibility

- `.inkue` files are JSON workspaces, with recursive cue JSON. Keep old optional
  fields loadable through serde defaults/migrations and cover important
  migrations with fixture/tests.
- Workspace-contained file paths are stored relative to the workspace folder;
  outside paths can remain absolute. `Collect and Save` copies media to make a
  workspace portable. Don't serialize runtime engine IDs/pointers into a show.
- Check both Rust and frontend representations when changing preferences,
  output destination, cue, event, or command contracts.
- Keep QLab import native in `src-tauri/src/qlab_import/`; do not shell out to
  Python. Confirm format/property assumptions against fixtures/code before
  changing mappings.

## Platform and package rules

- Design for Windows/macOS/Linux using platform-specific code behind explicit
  `cfg`/runtime branches. Test target-specific behavior rather than assuming a
  successful compile proves runtime support.
- libmpv is loaded at runtime. Windows development/release uses the local
  `src-tauri/vendor/mpv/libmpv-2.dll`; macOS/Linux use native libmpv. A missing
  output engine should degrade with a health alert, not take down startup.
- The Windows Qlisa application is always built with Cargo feature
  `asio-support`. Do not use Windows app build commands that omit this feature.
  Use `pnpm release -- X.Y.Z` for a versioned release; it skips installers by
  default. The lower-level `pnpm tauri:build` command bundles installers.
- Windows release bundling requires local runtime resources staged under
  `src-tauri/vendor/` and is validated by `build.rs` /
  `tauri.windows.conf.json`. FFmpeg/ffprobe and their notices are staged from
  the pinned external archive and stay outside Git. NDI Runtime is installed
  separately by the user; never copy an NDI DLL into Qlisa or its installer.
  The Windows libmpv DLL remains a local development/release prerequisite.
  Keep notices/license files with distributed binaries.
- NDI/SRT destination status and local transport readiness do not guarantee a
  remote receiver is showing the signal. Network frame producers must remain
  bounded/non-blocking relative to rendering; network/audio worker code must
  never stall the cpal callback.

## Frontend conventions

- Use functional React components, typed Zustand stores, shared types in
  `src/lib/types.ts`, commands in `src/lib/commands.ts`, and the established
  event hook.
- Keep English/Russian copy in `src/i18n/dictionaries.ts` for user-facing
  strings. Avoid hardcoding new one-language labels into components.
- Preserve selection/playhead independence, cue table keyboard behavior,
  inspector save semantics, and window-specific Tauri capabilities.
- Tests live beside UI logic or in `__tests__`. Prefer tests for data/behavior
  contracts and regressions over snapshots of implementation details.

## Commands

From repository root:

```powershell
pnpm install
pnpm test
pnpm build
pnpm tauri:dev       # Windows app development build, with ASIO
pnpm tauri:check     # Windows debug/no-bundle build, with ASIO
pnpm tauri:build     # low-level ASIO build that bundles installers
pnpm exec tauri build --debug --no-bundle -- --features asio-support # quick test build, no installer
pnpm release -- 1.5.3 --dry-run # verify version state and preview the release
pnpm release -- 1.5.3          # sync versions, commit, then build the exe with ASIO
pnpm release -- 1.5.3 --bundle nsis # explicitly build only an NSIS installer
```

From `src-tauri/`:

```powershell
cargo test
cargo test --lib
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

The project Cargo config uses 20 build jobs, keeps the incremental cache, and
selects Rust's bundled `rust-lld` for `x86_64-pc-windows-msvc`. Do not run
`cargo clean` in the normal edit/check/test loop. Use `CARGO_BUILD_JOBS` or
`CARGO_INCREMENTAL=0` only for a specific diagnostic; neither is a normal
setting. Release builds use `opt-level = 3`, Thin LTO, and 16 codegen units.

After each completed and verified logical task, the team lead creates a
separate Git commit before building or handing off that version. Before a risky
experiment, create a checkpoint commit. Do not start the next task with
uncommitted changes; the only exception is work explicitly marked WIP. Record
the commit hash used to produce each build artifact so the artifact can be
traced back to its source.

The release command is Windows-only and requires a clean Git tree and a strictly
greater SemVer `X.Y.Z`. It updates the four version files, commits them as
`release: vX.Y.Z`, then builds with `asio-support`. It creates no installer by
default. Only an explicit `--bundle nsis` or `--bundle msi` creates one installer
of that type; never use `all`. The command writes artifact details to
`src-tauri/target/release/qlisa.release.json`.
`--dry-run` performs read-only platform, clean-tree, version-consistency, and
higher-SemVer checks, then only prints the plan: it does not edit or stage
files, commit, or build. A failure before the release commit restores only
version files changed by that run. After the commit, leave the version and
history in place; do not rewrite the release commit. If Git cannot establish
whether HEAD changed, skip rollback.

`scripts/publish.ps1` is a separate local preflight. `-RunChecks` runs frontend
and Rust checks; Cargo may download dependencies. `-PrepareRelease` is reserved
for future work and currently fails closed. The installer and release-metadata
preparation pipeline is not implemented. The script does not publish to
GitHub.

Close a running Windows Qlisa/tauri-dev process before relying on a fresh
libmpv/runtime copy; a loaded DLL can be locked and the build script may retain
the prior copy. See [the project guide](docs/PROJECT_GUIDE.md) for complete
platform prerequisites, test scope, troubleshooting, and file map.
