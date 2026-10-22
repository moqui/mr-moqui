# Plans

Design, status and review documents for the MR-Moqui port. The first pass is complete: phases 0–12 and gates 1–4 of [Implementation-Phases](archive/Implementation-Phases.md) were built by 2026-10-14, with LLM/A2A deferred (Phase 13, now redesigned in [LLM-A2A-Assist](LLM-A2A-Assist.md)). The entity and service parity review finished on 2026-10-16, and the screen and web parity review on 2026-10-20; the gap review of the rest of the framework is in progress. After it come performance work and then the proposals.

**Start here**
- [Remaining-Work.md](Remaining-Work.md): what is still open, in one place: the deferred phase, known gaps, the parity gap review's progress, the code quality review's leftovers, Mantle follow-ups.
- [Future-Plans.md](Future-Plans.md): digest of the unbuilt proposals.
- [Performance-Summary.md](Performance-Summary.md): digest of the performance plan, the recorded numbers and the Mantle USL audit.

**Phase 13: LLM, A2A and Assist**
- [LLM-A2A-Assist.md](LLM-A2A-Assist.md): the design and implementation plan for the agent runtime, the Assist drawer, chat and triggered agents, A2A, MCP and the OpenAI-compatible gateway, with the review of the Java prototype it replaces and the ideas taken from other assistants. Nothing built yet.

**Reference** (long-lived, kept current)
- [Parity-Mapping.md](Parity-Mapping.md): every Rust module mapped to the Java, Groovy, XML and FreeMarker it was ported from, and the reverse index; what is excluded on purpose; the cross-cutting threads a review carries.
- [Parity-Diffs.md](Parity-Diffs.md): the deliberate behaviour differences from Java inside ported features, each with its rationale and the code comment that records it. Companion to the mapping.
- User guides for built features are in [`../docs/`](../docs/), not here: [Native-Components](../docs/Native-Components.md), [Testing](../docs/Testing.md) (`moqui test`), [Revision-Management](../docs/Revision-Management.md) (version diffs, `git://` resources, drives), [Git-Hosting](../docs/Git-Hosting.md) (clone and push over HTTPS and SSH, gitECA rules, projects and tasks), [Packaging](../docs/Packaging.md) (the build packages, their features and profiles), [Services](../docs/Services.md) (errors, transactions and probe calls), [Mail-OAuth](../docs/Mail-OAuth.md) (sign-in with Google and Microsoft for mail accounts) and [Mail-Inbound](../docs/Mail-Inbound.md) (the SMTP listener for hosted and shared accounts behind the edge gateway) and [TUI](../docs/TUI.md) (`moqui tui`, `apps` over SSH, `agent-apps` for AI agents, `--server` for a running server, its keys, widths and themes, and `tui` texts for screen authors).

**Next phase: performance** (digested in Performance-Summary)
- [Performance.md](Performance.md): the target (100,000 durable commits or reads per second), one-node and maximum-scale setups, startup budget, capacity limits.
- [Performance-Implementation.md](Performance-Implementation.md): outcomes and recorded before/after numbers for schema inspection, macro compilation reuse, definition loading and build feedback.
- [Profiling.md](Profiling.md): the speed tests and how to run and profile them, what four rounds of entity, service, data load and XML work changed, the measured results, and next steps (external Postgres next).
- [MantleUSL-Performance-Audit.md](MantleUSL-Performance-Audit.md): static review of mantle-usl for avoidable work amplification.
- [MantleUSL-Execution-Benchmark-Plan.md](MantleUSL-Execution-Benchmark-Plan.md): Stages A–E, the correctness gate, and the execution-method comparison. The audit and this plan cite `../experiments/mantle-execution/README.md`, which lives on a separate experiment branch, not in this repository.

**[proposals/](proposals/)**: unbuilt, no code in the workspace; each is digested in Future-Plans.
- [Messaging.md](proposals/Messaging.md): runtime-reflective FlatBuffers request/response and document subscriptions (1286 lines, 0 of 6 phases).
- [Monitoring.md](proposals/Monitoring.md): technology-independent monitoring, diagnosis and recovery requirements, with the implementation delta.
- [Data.md](proposals/Data.md): load types by ownership and sensitivity, three-way code/DB merge, dev export (not fully verified).
- [Usability.md](proposals/Usability.md): cross-platform hosts: desktop, embedded/POS, browser-local WASM, mobile.

**[archive/](archive/)**: built, resolved or superseded, kept as reference. Each has a note under its title saying where its live part went.
- Designs: [Architecture-Proposal](archive/Architecture-Proposal.md), [Crate-Layout](archive/Crate-Layout.md), [ContextObjects](archive/ContextObjects.md), [SearchFacade](archive/SearchFacade.md), [QvtServerStatic](archive/QvtServerStatic.md), [EmbeddedPostgres](archive/EmbeddedPostgres.md), [PostgresBackup](archive/PostgresBackup.md), [Cluster](archive/Cluster.md), [Ssh](archive/Ssh.md), [Sso](archive/Sso.md), [Office-Image-Print](archive/Office-Image-Print.md), [TypeGeneration](archive/TypeGeneration.md), [Wasm](archive/Wasm.md), [Native-Components](archive/Native-Components.md), [TestHarness](archive/TestHarness.md), [Screen-TUI-Revue](archive/Screen-TUI-Revue.md).
- Build records: [Implementation-Phases](archive/Implementation-Phases.md) (phases and gates), [Screen-TUI-Impl](archive/Screen-TUI-Impl.md) (the `tui` render mode and the terminal client, Phases 1 to 5, built by 2026-10-21; guide [TUI](../docs/TUI.md)), [ConvertProcess](archive/ConvertProcess.md) (the Mantle Groovy conversion), [MantleFlowTests](archive/MantleFlowTests.md) (mantle-usl spec status), [MarbleERPMigration](archive/MarbleERPMigration.md).
- Reviews: [License-Review](archive/License-Review.md) (still the license gate that `deny.toml` cites), [MimoReview](archive/MimoReview.md), [SonnetReview](archive/SonnetReview.md), [SonnetRecommendations](archive/SonnetRecommendations.md), [Code-Quality-Review](archive/Code-Quality-Review.md), [EntityService-Parity-Review](archive/EntityService-Parity-Review.md), [ScreenWeb-Parity-Review](archive/ScreenWeb-Parity-Review.md) (the screen, macro and web parts; every finding fixed, recorded or dropped by 2026-10-21), [Markdown-PDF-Review](archive/Markdown-PDF-Review.md) (pdfrs, Typst and Markdown through Chromium for print; PDF stays on Chromium).

## Conventions

- **Lifecycle.** A document starts in `proposals/` when it has no code (and gets a paragraph in Future-Plans), moves to the root when it is active or next (its first paragraph is a dated status line), and goes to `archive/` when it is built, resolved or superseded. Archiving does not delete anything: the design or review stays whole as the record of what was decided and compared.
- **Archiving a document.** `git mv` it; add `> Archived <date>: ...` under the title saying its state and where the live part went; grep the repository for its path (Markdown links, plain `plans/...` text, and `//!` doc comments under `crates/`) and fix each; move anything still open into Remaining-Work with a link back; move a parity review's accepted differences into Parity-Diffs and leave a pointer in the review; update this README.
- **Summaries.** Remaining-Work is the one list of open items; Future-Plans digests the proposals; Performance-Summary digests the performance set. A summary links to the detail and does not restate it.
- **Reviews going forward.** Name a parity review `<Area>-Parity-Review.md` and any other `<Area>-Review.md`. Keep the shape of the entity and service review: a summary with counts, a phase table per area with `ID | Rust location | Java location | Divergence | Severity | Intentional?`, a `####` section per open finding, a "checked and found equivalent" list, and an `## Accepted differences` table that moves to Parity-Diffs at archive time with its IDs.
- **User guides** for built features go in `../docs/`, not here. A plan may link to its guide.
- **Names.** Kebab-Case file names, no dates in names; dates go in the status line.
