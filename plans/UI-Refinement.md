# Framework UI review and refinement

## Plan

### Summary and working record

Review and improve all framework-owned applications through complete user activities, starting with shared UI.
Cover Material `/qapps` and the embedded TUI in default light and dark themes, with targeted Standard `/apps`
compatibility checks. Exclude addon applications under `runtime/component/`.

Refine primarily for **1920×1080**. Use a balanced workspace style: useful information density, clear grouping and
action hierarchy, readable secondary text, and restrained color. Substantial navigation and screen redesigns are
allowed when they improve workflows.

This document begins with the agreed plan, followed by baseline observations, the coverage ledger, findings, and
running implementation notes. It is the single active record for this effort. Establish the plan and baseline in an
initial commit; commit each subsequent bounded batch with its corresponding notes and verification results. **Keep
every commit local and unpushed.** Prefer iterative follow-up improvements.

### Review method and environment

- Use the existing release/maximal binary with a disposable framework-only runtime and demo database. Use HTTP port
  **8090**, isolated database storage, and free secondary listener ports. Keep the Purchasing agent's runtime,
  database, and addon checkouts untouched.
- One coordinator owns server lifecycle, shared theme/widgets, integration, documentation and commits. Delegate
  independent app reviews to up to three agents, with separate browser sessions/accounts and assigned files.
  Serialize shared-file edits and coordinated verification.
- Before reviewing each app, inventory its reachable screens and define activities by **actor, starting state,
  objective, steps, expected result, and recovery path**. Map every user-facing screen to an activity or an explicitly
  recorded inspection.
- Execute activities through visible UI controls. Inspect intermediate screens, dialogs, feedback, navigation and
  resulting data.
- Record each finding's severity, activity step, route, role, theme/viewport, reproduction, evidence, owning layer,
  correction and retest result. Distinguish observed defects, suspected causes and design judgments.
- Work in bounded batches: observe → prioritize → refine → rerun affected activities → compare before/after →
  verify → update notes → commit. Fix shared causes before repeating local corrections.
- Develop a reusable `UI-Review.md` guide in the development documentation, covering environment setup, activity and
  finding templates, review criteria and established patterns. Link it from this record.

### Coverage waves

| Wave | Areas | Activities to exercise |
|---|---|---|
| 1. Shared foundations | Shell, Gallery, shared widgets, web TUI | Sign-in and recovery; launcher, navigation and breadcrumbs; search, notifications and preferences; list filtering, sorting, pagination, export and columns; saved-find lifecycle; forms, dialogs, detail pages, boards, calendars and wizards; terminal navigation and theme changes. |
| 2. Administration | System and Tools | Assess instance health; inspect components/configuration; manage disposable users and groups; inspect jobs and run a safe job; investigate audit/visit/message records; manage reference data and resources; find/edit/import/export disposable entity records; inspect and invoke safe services; use JavaScript/SQL tools and API documentation. Cover LLM, edge, voice and bridge administration as distinct activity groups. |
| 3. Diagnosis and reporting | Monitor and Dashboards | Overview → filtered logs → trace → diagnostic context; metrics and time comparisons; temporary diagnostic settings; synthetic checks and incident/recovery views; open, filter, drill into and export dashboards; copy/edit dashboards, build panels, and inspect alerts, annotations, reports and history. |
| 4. Development | Dev AIDE | Find artifacts and references; navigate/edit files; model entity extensions; compose and simulate services; build and preview screens; inspect generated specs/docs; review changes and resolve conflicts. Perform writing/publishing exercises against disposable site repositories and a local remote. |
| 5. Communication | BasicComms | Inbox triage and message detail; compose/draft; chat rooms, threads and attachments; call lobby/history and voicemail; account, identity, rule and device settings. Use captured mail, local participants and fake media; exercise unavailable-provider and failure states. |
| 6. Integration | All framework apps | Revisit representative activities after shared changes, resolve cross-app inconsistencies, inspect remaining inventory entries, and finalize reusable patterns and outstanding findings. |

For each applicable activity, cover normal completion plus meaningful empty, invalid, unavailable,
permission-limited and recovery states. External integrations without a successful local fixture remain explicitly
unverified.

### Refinement and compatibility rules

- Improve shared semantic colors and component styles first. Separate readable text colors from fills, accents and
  status backgrounds; maintain visible focus and selected states.
- Target at least **4.5:1 for ordinary text**, **3:1 for qualifying large text**, and **3:1 for meaningful control/state
  indicators**, following W3C's [text contrast](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html) and
  [non-text contrast](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html) guidance. Measure rendered
  combinations and inspect them visually.
- Improve page titles, coherent sections, primary/secondary actions, supporting metadata, empty states and
  terminology. At 1920px, use available width deliberately while keeping forms readable and related content together.
- Begin shared interaction work with Find Options, Saved Finds and Columns. Make filtering, sorting,
  saving/updating/default selection and resetting distinguishable; keep dialog actions reachable; clarify icon
  controls; verify keyboard, cancellation and reopen behavior.
- Investigate TUI palettes on fresh light/dark launches and switches in both directions. Verify headings, muted text,
  selection, focus, dialogs and shortcut help.
- Preserve public service/entity contracts, screen routes, bookmarks and saved-find/column preferences. Document
  unavoidable interface changes and provide compatibility handling.
- Retain the existing framework/component architecture. Extend shared widgets where useful, demonstrate patterns in
  Gallery, and validate them with realistic content in actual apps.

### Verification, commits and completion

- Use **1920×1080 as the primary browser viewport** for desktop design decisions and before/after evidence, in both
  default themes.
- Add focused **1280px**, **390px**, keyboard-only and **200% zoom** checks for navigation, forms, dialogs and
  representative layouts. Check alternative color schemes where shared token changes affect them.
- Use screenshots as inspected review evidence, paired with DOM assertions, measured contrast, console/network
  errors and observable workflow outcomes. Avoid unattended screenshot regression suites.
- Extend existing tests where they already cover behavior. Keep focused bug/rule checks in regression; complete
  activities, responsive checks and access walks belong in acceptance. Preserve one golden-path regression browser
  walk per app and avoid duplicate renders or sweeps.
- Name and run relevant framework acceptance journeys as part of this requested review. When adding or changing an
  acceptance test, run it and its component's acceptance suite. Do not invoke addon suites or `accept-all` by default.
- During fixes, run affected regression specs using the existing binary unless Rust changes. Before each
  implementation commit, run `./moq test-framework`, lint changed screens, and respect existing test budgets and build
  concurrency limits.
- Each batch's running notes record its intent, findings addressed, changes made, verification commands/results,
  before/after evidence references and remaining issues. Commit these notes together with the associated source and
  tests.
- Close a wave when its activity inventory has been exercised, actionable findings are fixed and retested or
  explicitly documented as deferred, and relevant checks pass. Report blockers and external dependencies separately
  from successful coverage.
- Final delivery includes the local commit sequence, implemented refinements, reusable guide, coverage/findings
  ledger, selected before/after evidence, test results and prioritized remaining work for continuing into addon
  applications.

## Baseline — 2026-11-04

The repository started clean on `master`. The existing release/maximal binary was used for reconnaissance, with a
framework-only runtime, an empty addon directory, and a fresh disposable database. The server used port 8090; its
`framework/ClientFixtures` setup passed all 16 features. The server was stopped after reconnaissance. No source or
existing database was changed during planning.

The framework launcher exposes Comms, Gallery, Dashboards, Dev AIDE, Monitor, System and Tools. The seven app screen
trees contain approximately 317 XML definitions, including branches, helpers and mode twins. The shared shell,
authentication and terminal are additional review surfaces. Installed addons can hide or extend framework apps,
which is why the review runtime excludes them entirely.

The initial screenshots were exploratory at the browser's natural viewport. Subsequent design decisions and
before/after evidence use the agreed 1920×1080 target.

## Coverage ledger

| Area | Activity groups | State | Evidence / remaining work |
|---|---|---|---|
| Shared shell and widgets | Navigation, lists, saved finds, columns, forms, preferences | Reconnaissance | System user list, Find Options, empty search and Saved Finds inspected; full lifecycle pending. |
| Gallery | Archetypes and shared widget reference | Pending | Reuse existing Gallery acceptance sweeps. |
| Embedded TUI | Navigation, dialogs, themes and focus | Reconnaissance | Dark System dashboard inspected; fresh/switch/reconnect comparisons pending. |
| System and Tools | Administration and data/service tools | Reconnaissance | System dashboard and user list inspected; remaining activities pending. |
| Monitor and Dashboards | Diagnosis, reporting and authoring | Pending | Reuse the existing Monitoring acceptance walk. |
| Dev AIDE | Artifact discovery, editing, simulation, previews and Git | Pending | Use disposable site repositories for writing activities. |
| BasicComms | Mail, chat, calls and settings | Pending | Use local communication fixtures and fake media. |
| Integration | Cross-app patterns and remaining routes | Pending | Starts after shared and app batches. |

## Findings

Severity: **P1** blocks an activity or makes important information unusable; **P2** causes substantial friction or
misleading presentation; **P3** is a localized refinement. A static suspicion is not an observed failure.

| ID | Area / reproduction | Finding | Severity | Evidence type | State |
|---|---|---|---|---|---|
| UI-001 | System → Users → Find Options; search for a nonexistent username, light theme | “No results found” is `#f2c037` on white (about 1.70:1); the green filter value is also faint. Shared list feedback needs readable semantic text colors. | P2 | Browser screenshot and computed style | Open |
| UI-002 | System → Users → Find Options | The tall, single-column filter dialog puts the Find action below the visible region. Sorting and Clear Parameters precede the search fields without useful grouping. | P2 | Browser screenshot | Open |
| UI-003 | Account → Terminal UI → System, dark theme | Section headings and shortcut help are faint against the terminal background. | P2 | Browser screenshot | Open |
| UI-004 | System dashboard | Obsolete Hazelcast control, blank memory values and a literal `null` thread count undermine the instance overview. | P2 | Browser screenshot and accessibility text | Open |
| UI-005 | Tools dashboard | “Groovy Shell” remains in the source although the runtime uses JavaScript. | P3 | Static inspection; browser confirmation pending | Open |
| UI-006 | Shared pagination, dark theme | Explicit `grey-8` is not covered by the current dark text overrides. | P2 | Static suspicion; browser confirmation pending | Open |
| UI-007 | Embedded TUI, light/dark switch | The browser updates xterm's background while the TUI queries its background only at session start; the palettes may diverge during a live switch. | P2 | Static suspicion; browser confirmation pending | Open |
| UI-008 | Columns dialog | Directional icon controls lack explanatory tooltips; moving between columns and within a column is hard to distinguish. | P3 | Static inspection; browser confirmation pending | Open |

## Running implementation notes

### Batch 0 — plan and baseline

- Established the agreed plan, initial findings and coverage ledger in this document.
- Added the plan to the plans index. No application behavior changes.
- Verification: clean starting tree; `git diff --check`. Documentation-only batch, so no framework test run is needed.
- Next: shared semantic text contrast, list dialogs and terminal palettes, alongside independent app review.
