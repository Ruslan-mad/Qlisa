# Qlisa documentation

## Project and contribution

- [Project guide](PROJECT_GUIDE.md) — architecture, cue lifecycle, data flow,
  build and test commands, platform prerequisites, and compatibility rules.
- [Release notes for 1.5.2](RELEASE_NOTES_1.5.2.md) — changes recorded for that
  version.
- [Release notes template](RELEASE_NOTES_TEMPLATE.md) — format for future
  release notes.

## Technical notes

- [Audio bus routing](audio-bus-routing.md) — routing rules for cue audio.
- [Multi-output behavior](qlisa-multi-output.md) — display/network destinations,
  routing, delivery, and platform limits.
- [Network I/O](network-io-design.md) — current network worker behavior and
  implementation caveats.
- [Runtime control audit](runtime-control-audit.md) — current transport fixes,
  regression evidence, and remaining device-validation risks.
- [Windows network runtime](windows-network-runtime.md) — NDI and FFmpeg/SRT
  runtime prerequisites and packaging notes.

## Maintainer and license references

- [Updater signing plan](release-signing-plan.md) — signing and release
  requirements. The updater has not been verified for release use.
- [Dependency license audit](dependency-license-audit.md) — dependency and
  binary source review dated 2026-09-24; recheck against the exact payload
  before any binary distribution.
- [Third-party notices](../THIRD_PARTY_NOTICES.md) — upstream attribution,
  license summaries, and distribution requirements.
