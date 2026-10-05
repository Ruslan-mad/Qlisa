# Media preview cache

Qlisa stores derived media previews beside a saved project. A project named
`show.qlisa` uses the sibling directory `show.qlisa.cache/`. The directory
contains versioned `.qcache` entries for waveform peaks, thumbnails, and video
filmstrips. It does not contain source media or decoded PCM. The project stays
ordinary JSON; the cache is disposable and can be deleted at any time.

Unsaved workspaces use a private cache directory under the application config
folder. The directory is scoped to that workspace. On the first save, Qlisa
copies valid entries to the project's cache folder. Save As and Collect and
Save do the same after the project file is written. These transfers are
best-effort and do not block save if the cache is read-only or damaged.

Each cache key includes a normalized source identity, file size, modification
time in nanoseconds, cache format version, and request parameters. Paths inside
the project use project-relative identities. Other paths use absolute
identities. Waveforms are cached per requested bin count. Trims, slices, crop,
and geometry do not invalidate full-file waveform or thumbnail entries. Video
range filmstrips use their requested time range, tile count, and width.

Entries use an atomic write and a payload checksum. A missing, corrupt, old
version, or unwritable entry is treated as a cache miss. Waveform cache hits
avoid decoding. On a miss, Qlisa decodes the current source file before it
stores peaks, so stale in-memory PCM cannot be saved under a new file
fingerprint. Confirmed video files without an audio track are negative-cached
by source fingerprint; transient decode errors are not. Each scope prunes its
own `.qcache` files above 256 MiB and keeps about 224 MiB after pruning. Other
files in the directory are never removed.

`media_source_revision` is runtime DTO data. It combines source identity, file
size, and modification time so the UI can notice a replacement at the same
path. Number timelines key each child by cue identity and source revision.
Trimming changes the video range filmstrip; it does not invalidate a full-file
waveform. Presentation fields such as cue name and disabled state do not
invalidate decoded assets. The revision is not written into cue JSON or the
`.qlisa` project.

Verification completed: 19 Rust cache and thumbnail tests passed, including the
libmpv smoke test. All 427 frontend tests passed. UI behavior has not been
checked in this implementation pass.

Run the Rust tests serially from the repository root in PowerShell:

```powershell
$env:CARGO_BUILD_JOBS = '1'
$env:CARGO_INCREMENTAL = '0'
cargo test --manifest-path src-tauri/Cargo.toml --lib --features asio-support -- media_cache::tests thumbnails::tests thumbnails::preview_cache_integration_smoke
```

To enable the optional installed-libmpv smoke test, set the environment variable
for that command:

```powershell
$env:QLISA_CACHE_SMOKE_RUNTIME = '1'
cargo test --manifest-path src-tauri/Cargo.toml --lib --features asio-support -- media_cache::tests thumbnails::tests thumbnails::preview_cache_integration_smoke
```
