# Qlisa documentation

## Project and contribution

- [Release notes for 1.5.11](RELEASE_NOTES_1.5.11.md) — changes in this release.
- [Release notes for 1.5.10](RELEASE_NOTES_1.5.10.md) — previous release.
- [Project guide](PROJECT_GUIDE.md) — architecture, cue lifecycle, data flow,
  build and test commands, platform prerequisites, and compatibility rules.
- [Release notes for 1.5.6](RELEASE_NOTES_1.5.6.md) — changes recorded for that
  version.
- [Release notes for 1.5.2](RELEASE_NOTES_1.5.2.md) — changes recorded for that
  version.
- [Release notes template](RELEASE_NOTES_TEMPLATE.md) — format for future
  release notes.

## Technical notes

- [Audio bus routing](audio-bus-routing.md) — routing rules for cue audio.
- [Multi-output behavior](qlisa-multi-output.md) — display/network destinations,
  routing, delivery, and platform limits.
- [Inspector quick controls](inspector-quick-controls.md) — single and multi-cue
  actions, shared capabilities, mixed values, Fit routing, and loop restrictions.
- [Network I/O](network-io-design.md) — current network worker behavior and
  implementation caveats.
- [Runtime control audit](runtime-control-audit.md) — current transport fixes,
  regression evidence, and remaining device-validation risks.
- [Audio trim loop regression](audio-trim-loop-regression.md) — PCM and video
  trim-loop behavior, regression coverage, and runtime-test limits.
- [Number timeline](number-timeline.md) — Number action offsets, trim ranges,
  finite repeats, waveform reuse, and timeline playhead behavior.
- [Cue List navigation](cue-list-navigation.md) — visible-row keyboard
  navigation, Group/Number expansion, and Number creation order.
- [Headphone preview transport](headphone-preview-transport.md) — shared visual
  and audio preview transport, headphone mute, and repeated-seek behavior.
- [Cue wait progress and held video frames](cue-wait-and-hold.md) — Pre-Wait and
  Post-Wait progress cells plus Auto-Follow behavior at a held final frame.
- [Follow-up regression checklist](BUGFIXES_NEXT.md) — manual checks for seven
  source fixes after the 1.5.11 baseline; unchecked items are not verified.
- [Media preview cache](cache.md) — project-scoped waveform, thumbnail, and
  filmstrip cache behavior, invalidation, promotion, and storage limits.
- [Audio underrun diagnostics](audio-underrun-diagnostics.md) — callback-safe
  underrun context, reproduced cold-start evidence, and real-device test setup.
- [Windows network runtime](windows-network-runtime.md) — NDI and FFmpeg/SRT
  runtime prerequisites and packaging notes.

## Maintainer and license references

- [Windows release preparation](RELEASING.md) — local signing, compliance,
  packaging, updater checks, and the separate publication steps.
- [Dependency license audit](dependency-license-audit.md) — dependency and
  binary source review dated 2026-09-24; recheck against the exact payload
  before any binary distribution.
- [Third-party notices](../THIRD_PARTY_NOTICES.md) — upstream attribution,
  license summaries, and distribution requirements.
