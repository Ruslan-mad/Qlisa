# Qlisa documentation index

## Project documentation

- [Project guide](PROJECT_GUIDE.md) — onboarding handoff: product, architecture,
  cue lifecycle, audio/video/network/output/preview, persistence, threading,
  build/test/package, troubleshooting, limitations, and file ownership.
- [Multi-output behavior](qlisa-multi-output.md) — named display/network
  destinations, routing, frame/audio delivery, and known limits.
- [Windows network runtime](windows-network-runtime.md) — NDI and FFmpeg/SRT
  payloads, resource lookup, build checks, update script, and license notices.
- [Network I/O implementation notes](network-io-design.md) — the current
  runtime workers and the boundary between implemented behavior and caveats.
- [Runtime control report](runtime-control-audit.md) — root cause and completed
  fixes for eight GO/STOP/PAUSE/seek/Group lifecycle risks, focused regression
  results, operator semantics, and the remaining hardware-validation caveats.

The root documents are also important: [README](../README.md) is the developer
quick start, [CLAUDE.md](../CLAUDE.md) carries contributor/agent invariants,
and [PROGRESS.md](../PROGRESS.md) is the concise current status snapshot.

## External StageCUE research (not Qlisa specifications)

These files record research into the separate StageCUE product and its public
materials. They are retained for reference only; they do not describe Qlisa's
implementation, product commitments, roadmap, UI, or supported hardware.

- [StageCUE capability research](stagecue-capabilities.md)
- [StageCUE design research](stagecue-design-code.md)
- [StageCUE release-history research](stagecue-release-history.md)
- [Research source register](source-register.md)
