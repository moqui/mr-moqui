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

## Resumed after the merge — batches 13 to 17 (Nov 5)

After `02d6b325` merged 98 upstream commits, a review of the local commits and the merge (no P1, no authorization or
contract regression; upstream's asynchronous compose send, CSV formula guard, ModelDetail transactions and the bundled
screens all settled with the local work) led to five more batches: post-merge corrections (13); test time, an offline
acceptance suite and macOS loopback skips, ending with the first exit-zero full gate (14); the deferred replay, report
fallback, incident and recovery proposals, re-reviewed and corrected (15); every remaining open finding (16); and the
review's cleanups (17). Each batch passed `./moq test-framework`. The webroot client budget is 44 s and basic-comms
client 7 s, with the user's agreement. These sessions committed locally and pushed nothing themselves. `/tmp` paths in the earlier notes are historical evidence locations and may no longer exist; what
the deferred proposals needed is now in the repository and in the batch notes.

| Commit | Batch |
|---|---|
| `cfe13827` | 13: history under a default subscreen, ServiceRun output, index bounds, BasicComms cleanup, alert title order, shared sort parser |
| `70511f5d` | 14: webroot ThemeLayout acceptance, offline `./moq accept`, budgets, macOS loopback skips, recorded-call spool wait |
| `d5215abf` | 15: UI-088/089/092/102–107 |
| `71d1ad5a` | 16: UI-016/017/033/035/084/085, in-app confirmations, read-only form note, histogram not reproduced |
| This commit | 17: review cleanups and this record |

What remains is coverage, not open findings: the activities the ledger below marks pending (actual 200% browser zoom,
access-limited and narrow walks, the remaining application activities and Chat TUI, successful external providers and
replay of real user sessions beyond the local fixture) before expanding into addons.

## Wrap-up — stopped after batch 12 at the user's request

The user ended this review after the alert configuration batch. **This is a completed bounded review effort,
not completion of the original six-wave plan.** No whole wave met its full coverage criteria. All unperformed
activities, unresolved findings and private follow-up patches are deferred; no additional batch is scheduled.
The detailed ledger and historical notes below retain the distinction between implementation, automated checks
and activities actually observed. The reusable guide is [UI-Review.md](../docs/dev/UI-Review.md).

| Wave | Delivered scope | Deferred scope |
|---|---|---|
| Shared foundations | Semantic contrast, list dialogs/controls, confirmation, focus, responsive choices, TUI navigation/forms; many Gallery activities | Remaining Gallery activity/recovery/mode cases, actual 200% browser zoom and remaining shared checks |
| Administration | System/Tools entry points, service execution, SQL recovery, job/visit/model details | Deep user/group, reference-data, resource, LLM, edge, voice and bridge activity coverage |
| Diagnosis/reporting | Diagnostic layouts, charts, settings, dashboard authoring, saved searches, playlists, report configuration and alerts | Replay capture/playback, synthetic release comparisons, incident/recovery refinements, Monitor-only access walk, Git sync and remaining report flows |
| Development | Workspace/navigation/editor recovery and eight disposable-repository acceptance journeys | Remaining live modeling, simulation, screen building and mode-specific activity coverage |
| Communication | Mail/draft/settings recovery, local two-user chat, threads, forwarding, membership, attachment and keyboard flows; call/history controls | Live encrypted-room/device recovery, Chat TUI, successful real-media/provider workflows and remaining settings |
| Integration | Representative cross-app retests during shared changes | Final complete inventory and cross-app integration pass |

The local commit sequence is below; none was pushed. The final batch is this commit. Screenshot files remain
available only in ignored `plans/ui-refinement-evidence/`; no images from that directory are tracked.

| Commit | Batch |
|---|---|
| `67d3270f` | Plan and baseline |
| `883fffe8` | Shared readability and administration entry points; obsolete GroovyShell removed |
| `da924d4b` | Retained UI work, diagnostic recovery and development navigation |
| `1e88b494` | Diagnostic controls and aligned CSV export |
| `8c8ce50b` | Shared feedback and Gallery keyboard recovery |
| `b1a2ffc2` | In-app confirmations and editor recovery |
| `92ca156a` | Named list controls and diagnostic details |
| `2b9e7e5e` | Mail workflow/settings and job details |
| `f26f815b` | Report-test and terminal-playlist module preparation |
| `cc99c4ce` | Report playlists and terminal form content |
| `bf10126a` | Terminal navigation, responsive choices and drawer text |
| `aec0202f` | Chat pickers/keyboard actions and visible shared selections |
| This commit | Alert configuration, numeric round trips and state feedback; final review record |

Prioritized deferred work, if the review is resumed (as recorded at batch 12; items 1–3 are done in batches 15–16,
item 5 in batch 14):

1. **UI-088/089, P1/P2:** replay capture exceeds the receiver body limit; replay JSON requests use the wrong app
   mount. Private proposals exist under `/tmp/ui-batch10-replay-{capture,url}.patch`, but are unapplied and untested.
   Preserve masking, bounded ingestion and authorization when revisiting them.
2. **UI-103/105/107, P2:** human-readable incident times, loaded policy/adapter editing and visible Manual recovery
   instructions. UI-102/104/106 cover related grouping/recovery. Private `/tmp/ui102-104-incidents*.patch` and
   `/tmp/ui105-106-recovery*.patch` were reviewed but not applied or run. They need fresh review and verification.
3. **UI-084/085/092, P3:** replay/release list hierarchy and Standard report time formatting. Prepared proposals
   remain outside the repository; no passing implementation is claimed.
4. Complete access-limited, keyboard, narrow and actual 200% zoom checks, then the remaining application activities
   in the ledger before expanding into addons. Browser download-policy refusal and unavailable integrations remain
   unverified, not passing coverage.
5. Resolve the recurring SIP source-address test environment failure separately. Do not conceal it by weakening
   assertions or raising budgets. Full-gate results are recorded per batch.

One final query-editor observation remains unclassified: locator typing/Tab left an aggregation field's visible
text ahead of its generated JSON. Source inspection found the correct update path with native `change` semantics,
matching a previously observed browser-automation event limitation. The explicit JSON field saved correctly.
No new defect or fix is claimed without a reliable native-input reproduction.

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
| Shared shell and widgets | Navigation, lists, saved finds, columns, forms, preferences | Partial; remainder deferred | Find/filter and save/update/default/reset observed; delete cancellation passed. Columns move/undo/hide/save/reset and keyboard move/hide/Display, 1280/390 Find, Escape focus return and appearance reload passed. Material confirmation Cancel/Escape/focus and approved saved-find deletion now pass in Gallery; real200% browser zoom remains unverified. |
| Gallery | Archetypes and shared widget reference | Refined; partial activities | Server59/59 and browser23/23 acceptance pass. Live list/find confirmation, forms/dialogs, wizard, split, scan/task, board/calendar, message and Editor version/recovery activities recorded at1920 plus targeted390/keyboard checks. G03/G10/G11 and full recovery/mode coverage remain open. |
| Embedded TUI | Navigation, dialogs, themes and focus | Refined; partial retest | System and F1 help readable across live light/dark switches; fresh light and dark launches and F1 help after saved appearance reload also passed. |
| System and Tools | Administration and data/service tools | Refined; partial activities | Overview both themes, Components, SQL success/error/recovery, service XML and echo output/validation/retry observed. Deep administration groups remain pending. |
| Monitor and Dashboards | Diagnosis, reporting and authoring | Refined; partial activities | Monitoring 19/19 at 1440×1000; dashboard copy/edit/save/open, CSV export and filter/remove visually observed at 1920; saved log search restores query/range. See assertion limits and screen map below. |
| Dev AIDE | Artifact discovery, editing, simulation, previews and Git | Refined; remaining activities deferred | Home, creation-dialog cancellation, search empty states, source navigation and Search → References → Docs/copy/package/diagram navigation observed. All eight disposable-repository journeys passed; detailed limits are recorded below. |
| BasicComms | Mail, chat, calls and settings | Refined; partial activities | Local fake-media/encrypted-call/chat10/10; draft recovery, captured mail, wizard/settings/Standard flows recorded below. Live two-user DM, forward, private-room membership/leave, thread/edit/attachment, picker recovery, keyboard actions and call-history resets pass. TUI rich bodies/dates and narrow radio labels are corrected; remaining chat TUI/encryption/media/provider activities stay explicit. |
| Integration | Cross-app patterns and remaining routes | Partial; final pass deferred | Shared styles observed in System, Tools, Dashboards and Dev AIDE; full remaining inventory is explicit below. |

## Findings

Severity: **P1** blocks an activity or makes important information unusable; **P2** causes substantial friction or
misleading presentation; **P3** is a localized refinement. A static suspicion is not an observed failure.

| ID | Area / reproduction | Finding | Severity | Evidence type | State |
|---|---|---|---|---|---|
| UI-001 | System → Users → Find Options; search for a nonexistent username, light theme | “No results found” is `#f2c037` on white (about 1.70:1); the green filter value is also faint. Shared list feedback needs readable semantic text colors. | P2 | Browser screenshot and computed style | Fixed; measured default semantic roles and observed empty-list feedback |
| UI-002 | System → Users → Find Options | The tall, single-column filter dialog puts the Find action below the visible region. Sorting and Clear Parameters precede the search fields without useful grouping. | P2 | Browser screenshot | Fixed; 1920, 1280 and 390px Find actions fit and filtering completed |
| UI-003 | Account → Terminal UI → System, dark theme | Section headings and shortcut help are faint against the terminal background. | P2 | Browser screenshot | Fixed; light/dark System and F1 help observed |
| UI-004 | System dashboard | Obsolete Hazelcast control, blank memory values and a literal `null` thread count undermine the instance overview. | P2 | Browser screenshot and accessibility text | Fixed; overview observed in both themes; focused regression passed |
| UI-005 | Tools dashboard | Obsolete GroovyShell screen remains although the runtime uses JavaScript. | P3 | Static inspection and explicit removal request | Removed, including unused seed permission/grant. Replaced by the web console/Terminal UI and the SSH admin shell's JS executor; legacy screen bookmarks now return 404 by request. |
| UI-006 | Shared pagination, dark theme | Explicit `grey-8` is not covered by the current dark text overrides. | P2 | Static suspicion; browser confirmation pending | Fixed; generated client refreshed; final dark pagination check pending |
| UI-007 | Embedded TUI, light/dark switch | The browser updates xterm's background while the TUI queries its background only at session start; the palettes may diverge during a live switch. | P2 | Static suspicion; browser confirmation pending | Fixed; live switches in both directions and fresh dark launch observed |
| UI-008 | Columns dialog | Directional icon controls lack explanatory tooltips; moving between columns and within a column is hard to distinguish. | P3 | Static inspection; browser confirmation pending | Move/undo/hide/save/reopen/reset passed; final keyboard move → Hide → Display → Column 1 retained field focus; 28/28 widget checks |

Additional findings use the same John Doe administrator in the disposable framework runtime. Unless stated
otherwise, browser observations are Material at 1920×1080; routes are under `/qapps`.

| ID | Activity / route and reproduction | Severity and basis | Owning layer / correction | Retest and evidence |
|---|---|---|---|---|
| UI-009 | Shared saved-find lifecycle, `/system/Security/UserAccount/UserAccountList`: active find opens a large scheduling form above its actions | P2 design judgment, light screenshot | ScreenQvt/Static and sharedCSS: action list first, new find separate, schedule collapsed, named deletion confirmation | Save/update/default/clear-default passed; batch5 Material dialog replaces the blocked native prompt, with live Gallery Cancel/Escape/Confirm deletion retest. `saved-finds-*-1920.jpg` |
| UI-010 | T06, `/tools/Service/ServiceRun`: successful BasicServices.echo#Data returns blank output fields | P2 observed functional defect | Screen-local session result envelope replaces unavailable `ec.webImpl` method; bind output form separately, clear stale failed-run result | Echo text, default ping and numeric 0 visible; invalid number rejected, corrected 2 succeeds. `service-output-after-1920.jpg`; AdminOverview |
| UI-011 | Appearance → Dark → reload: mode returns to Light | P2 observed functional defect | Core appearance serialization preserves forced mode; QvtConfig distinguishes permission boolean from mode string | Rust roundtrip, QvtConfig and dark selection → reload browser retest passed |
| UI-012 | Find Options at 390×844: full-screen dialog body retains 80vh limit and clips actions | P2 observed layout defect | Shared m-dialog body class and maximized height correction | 390×844 final screenshot confirms visible actions; keyboard filter submitted and returned the expected row |
| UI-013 | B03 Compose: failed server/device save followed by Close | P1 source-confirmed with focused browser regression | Compose closes only after confirmed persistence, retains text and exposes Retry save; draft storage returns the persistence outcome while preserving synchronous in-memory draft reads | ShellSync server/host/localStorage refusal+retry passed; live missing-identity Close and Refresh preserve subject/body at 1920/390 |
| UI-014 | DA-01/05 Home and Workspace: creation forms precede work; GoTo lacks title/cancel and can close on empty Enter | P2 observed hierarchy/labels plus focused reproduction | Dev AIDE home task-first, compact creation dialogs; labelled GoTo with loading/error/empty/retry and stale-result guard | Home, Escape cancellation, empty Enter, missing-name recovery and successful workspace lookup inspected; 26 client and 6 ArtifactIndex checks pass |
| UI-015 | T06 ServiceRun: select unknown service name | P2 observed raw rendering error | Existence guard and helpful message/recovery link; implicit entity services preserved | 5 AdminOverview checks pass; live unknown-service message and Browse Service List recovery link verified |
| UI-016 | S01 Components: no deployment-history rows says no component was loaded above six loaded components | P3 observed misleading empty state | Screen-local wording; deferred to next administration batch | Distinguish deployment history from loaded components. Fixed in batch 16. |
| UI-017 | Appearance color editor: new semantic foreground tokens are not listed | P3 source-inspection gap | Core appearance token inventory; deferred | Schemes can override CSS tokens, editor discovery remains pending. Fixed in batch 16. |
| UI-018 | D04 shipped dashboard definition/panel editor: copy action below long editor; query selector disabled in read-only view | P2 observed hierarchy and focused component rule | Copy-first layout, grouped labelled panel fields, read-only query inspection, Open dashboard action | Copy/native edit/Save version/Open dashboard passed with changed title visible; CSV headers/row verified, filter applied and removed; 47 WebrootReport and 19 Monitoring checks pass |

| UI-019 | DA-08: replace a live designer preview iframe | P2 observed console/network warnings | DevAideQvue/Screen mark the old same-origin frame as leaving immediately before replacement; retain real network error reporting | QVue and XML screen acceptance passed; 12/12 focused client checks; cancelled old shellData request identified in netlog |
| UI-020 | Columns: Enter on Move/Hide/Display | P2 observed keyboard workflow defect | Shared component restores focus to the moved field/action after Vue update; Display activation belongs to its clickable menu item | Failure reproduced before fix; widget 28/28 and live keyboard move/hide/redisplay passed; Escape returns to Columns |
| UI-021 | G01/G03: Landing counters and Detail subtitle in light mode | P2 observed readability; P3 layout judgment | Gallery uses m-muted instead of text-grey-6 for nine captions; Landing minimum card width 260px | Subtitle changes from rgb158 to rgb97; all six counter labels fit at 1920; Example 7/7 |
| UI-022 | B03: Compose with no sending identity | P2 observed raw save error / missing recovery | Explicit unavailable From state, Mail accounts new-tab link and identity-only refresh; save validates before POST and retains draft | Live 1920/390 Close and refresh retain subject/body; ShellSync 9/9 and focused server 18/18; no external provider configured |
| UI-023 | DA-05 References: outgoing Uses follows more than 300 incoming references; native section jump reloads the screen | P2 observed navigation defect | References adds counted section links; shared history handler preserves fragment-only navigation and unsent form state | Live Uses, Back/Forward and Back to artifact retain entered text and reach the section; existing shell regression 28/28. Follow-up adds an opt-in section scroll margin: Back to artifact now places its heading at 66px below the 51px toolbar, preserving unsent input. |
| UI-024 | DA-12 Docs: dark Markdown links and light→dark package diagram | P2 measured contrast and observed stale SVG | Markdown uses semantic link text; shared Markdown/Mermaid retain source and redraw default diagrams on theme change, preserving explicit themes | Link 4.07:1→8.46:1. Fresh dark and dark→light→dark observed; edges change rgb211→rgb51→rgb211; zoom reopens with current diagram. Markdown 38/38, CSS 10/10. |
| UI-025 | M01 Monitor overview, dark at 1920: faint canvas labels and five narrow panels | P2 observed contrast/layout | Shared chart theme defaults, radial tick backdrops and 640px minimum automatic report grid; explicit layouts preserved | Real canvas text meets 4.5:1; default radar baseline reproduced at 1.71:1 then fixed. 30/30 shared and 47/47 report checks. Live 1920 light/dark and 1280/390 layouts inspected. Same-mode custom scheme repaint remains a limitation. |
| UI-026 | M02 Logs: Search query `(` | P2 observed recovery defect | Explore isolates ordinary query errors/transactions and retains controls; hides results and Save Explore until correction; authorization exceptions propagate | Live invalid→`severity_text:INFO` succeeds without Back and retains the range. Error/after evidence captured; changed recovery journey 1/1, full Tools acceptance 19/19 including Standard. |
| UI-027 | B03: edit a draft while an earlier save is in flight, then Close | P1 reproduced data-loss race | Acknowledge the content snapshot after watchers settle; Close awaits newer writes, with local recovery retained through reconnect failure | Before failed; ShellSync 9/9 after. Deferred server/device saves, failed follow-up/retry, assigned ID and clearing an initially pending offline draft covered. |
| UI-028 | Shared app menu: keyboard Enter on Dev AIDE → Home | P3 observed console defect; navigation completes | Routed QItems prevent native Enter keydown activation, leaving Quasar keyup as the single activation | 30/30 focused checks include Enter, Space, modified click and popup closure. Live Monitor menu→Logs Enter navigates with no console warning/error. |

| UI-029 | M01 Overview time-series labels | P2 fixed and visually retested | Compact clock/date ticks with one date/timezone caption replace repeated ISO timestamps | Light/dark at 1920 and dark at 390 visually retested; existing 47-feature fixture checks unchanged underlying labels/tooltips/export data. |
| UI-030 | M02 Explore filters and reporting native controls | P2 fixed and visually retested | Grouped query/range controls and semantic native-control styles replace full-width stacking and browser-white fills | 1920 light/dark, 1280 and 390; query width 831px in a 1209px group, visible disclosure, keyboard/filter creation; follow-up UI-037. |

| UI-031 | S01 Cache List → populated native cache → Cache Elements | P2 fixed and retested | Reported count can be nonzero while the generic entry facade cannot enumerate native cache entries; distinguish unavailable details from empty cache | ScreenColorScheme count 1, zero detail rows observed; source ownership confirmed. Screen-only correction retested at 1920 and 390, Material/Standard; zero entries has a distinct message. |
| UI-032 | S01 Thread List and S05 Service Job List, macOS | P2 fixed and retested | Empty platform thread enumeration is shown as Threads: 0; scheduler last-execute value is raw milliseconds | ThreadList screenshot; use nullable Process.Threads and localized scheduler timestamp/clear labels. Corrected and retested in both themes and 390px; AdminOverview 7/7. |
| UI-033 | S11 Usage → Check a Quota → empty Check | P3 observed validation gap | Empty form closes silently; valid EX_JOHN_DOE displays quota without a clear cost currency | Empty and valid checks observed; no model request. Required user input and clearer quota presentation deferred. Fixed in batch 16. |
| UI-034 | Shared nested breadcrumb menu, System → LLM and Agents → Models | P2 fixed and retested | Menu opener was a span with tabIndex -1 and no role; Tab from System skips to Search | Real button opener: Tab/Enter/Space/Escape and child navigation passed. Text contrast raised from 2.72:1 to 16.9:1 in default dark; existing QVT fixture 30/30. |
| UI-035 | T02 Tools entity browser and edit | P3 observed hierarchy/layout | Database maintenance actions precede discovery; two short edit fields span 1920px; Find return loses the prior filter | Create/find/edit and duplicate recovery work; layout and return-context refinement deferred. `tools-entity-edit-before.jpg`. Fixed in batch 16. |
| UI-036 | T02/T03 EntityDataFind CSV export | P2 fixed and browser retested | Headers and values share the same exportable field set; conditional empty cells retain their column | Existing CSV fixture/core tests pass; rebuilt debug browser export has two matching headers, quoted text and empty cell. `/tmp/ui-review-entity-export-after.csv` |
| UI-037 | M02 Explore → Add filter without Label | P3 fixed and retested | Known predicates have readable summaries; authored labels and typed queries remain unchanged; Add filter aligns with inputs | Default light/dark 1920 and dark390; severity_text = "ERROR" applies and returns7 errors. Existing reporting fixture covers operators, negation and conservative JSON fallback. |
| UI-038 | G08 Wizard required-field recovery, default dark | P2 fixed and retested | Semantic validation text and subtle changed-field fill replace low-contrast defaults; default notifications use readable tonal pairs | Error helper 2.63→7.38:1; label on changed field 3.78→6.19:1. Default light/dark and Forest checked; notification message/caption/action regression passes. |
| UI-039 | G08 Wizard at 390px | P2 fixed and retested | Alternative labels keep step names visible and accessible instead of contracting them away | Customer/Options/Review visible; previous-step Enter preserves the draft, completion resets it. No future-step header navigation. |
| UI-040 | G04 New container dialog → Escape | P2 earlier observation; not reproduced in final check | Earlier closing returned focus to BODY | Fresh built-in browser at 390 dark and mounted Chromium both restore exact New opener after Escape and Close, including reopening. No speculative source fix. |
| UI-041 | G07 Split filter → country detail → Countries | P2 fixed and retested | Preserve the incoming find before row context replaces field values | United → United States → Countries retains four rows and United input at 390; server acceptance covers selection/edit parameters. |
| UI-042 | G06 QVue Board and Calendar controls | P2 fixed and retested | Keyboard Move menu/title links, named date controls and readable tonal event/count presentation | Board Enter move updates counts/sums and restores focus; Calendar Enter/Escape restores date focus, mobile events have full names, event text passes 4.5:1 in both themes. |
| UI-043 | G06 Calendar → another app screen | P2 fixed and retested | Bind endpoints to the Calendar instance and guard watcher/completion/unmount | Next → Today → 390→1920 → Gallery Task produces no stale events request or error toast; browser acceptance confirms. |
| UI-044 | G05 Task Confirm → Short | P2 fixed and retested | Full-page Short/Skip responses clear fragment-only stepOnly | Confirm retains demo line 1; Short opens line 2 with intro and Remaining lines (5), URL only lineIndex=1. |
| UI-045 | G09 CodeField edit | P2 fixed and retested | Visible field label reaches CodeMirror content and fallback textarea; optional names reconfigure without losing text/undo | Live dark1920 Description (JSON) textbox named correctly; mounted fixture covers dynamic names, restored editor state, unnamed standalone and failed-bundle fallback. |
| UI-046 | G09 Editor compare → Close → another page | P2 fixed and retested | Clear comparison/version state when changing context; invalid comparison retains current content, Edit, History and Close recovery | Live stale-version bookmark → local warning → Close → current Home passes; existing Gallery acceptance covers invalid bookmark and normal navigation. |
| UI-047 | G13 Message → Show images | P2 fixed and retested | Use actual request scheme/host for the local image fixture | HTTP review alias now loads the image (naturalWidth 114); Trust/Hide and keyboard trimmed-content expansion pass. |
| UI-048 | G09 Editor → Home | P2 fixed and retested | Existing explicit-null wire sentinel distinguishes Home from omitted default Welcome | Welcome → Home → reload → unchanged Save retains root page/version01; server and browser acceptance pass, including TUI literal sentinel. |
| UI-049 | G09 Editor → save appended Markdown → compare | P3 fixed and retested | Normalize Markdown at Editor input and comparison presentation; retained resource history/restores stay byte-exact | Existing mixed-line-ending versions02/03 now show2 added/0 removed, live light/dark1920; resource/Git server and browser acceptance pass. |
| UI-050 | Gallery guide text | P3 corrected | Task links current Handheld example; Editor describes real writes; List distinguishes demonstration row actions from stored finds/preferences | Task/Editor acceptance render passes; final List copy linted and covered by framework gate. |
| UI-051 | S06 Audit Log / Visit Detail, shared list controls | P2 fixed and retested | Named field/filter controls; ascending and descending arrows request their stated direction and recognize legacy sort syntax | Live Material Visit and Standard Audit Log: repeating descending stays descending, ascending updates aria-current; focused QVT/static/generator checks pass. |
| UI-052 | S06 Visit Detail request history | P2 fixed and retested | Static requests show HTTP request; host and instance have truthful labels; timestamps use local millisecond precision and timezone | Live visit100000/light1920 and existing monitor-reader integration pass; zero byte counts remain zero. |
| UI-053 | S06 → M02 Trace Detail selected span | P2/P3 fixed and retested | Durations sit outside timing bars; concise selected-span status/request/time summary precedes collapsible raw data | Live GET trace in both themes at1920/390; mounted ERROR-span fixture verifies contrast, geometry and unchanged raw data. |
| UI-054 | S12 Models → Model default | P3 fixed and retested | Group identity, context/pricing, execution and advanced limits; keep Save Row above the override form | Invalid Temperature retains Description; valid save retains advanced JSON through collapse/reopen; disposable override removed after Cancel/Confirm checks. Light/dark1920,390 and dark1280 inspected. |

| UI-055 | G09 Editor History → Publish | P2 fixed and retested | Confirmation identifies page, version and space before changing the published marker | Existing Gallery browser acceptance verifies Cancel causes zero writes and Confirm exactly one, then verifies the marker. |
| UI-056 | G09 Editor Edit dialog | P2 fixed and retested | Markdown content has an explicit accessible name | Existing Gallery browser acceptance locates and edits the named textbox. |
| UI-057 | G09 Git-backed Editor comparison | P3 corrected and tested | Short comparison headings preserve reading width; full version IDs remain in data and navigation | Existing Gallery acceptance checks short headings and full version parameters; resource/Git lifecycle coverage passes. |

| UI-058 | S05 Job Run Detail after safe manual run | P3 fixed and retested | Outcome, results, timing and runtime have distinct sections; millisecond times and timezone explain short runs | Run101012, light/dark1920 and light390. Mobile sections use explicit full width. View trace reaches retained6.35ms span; View logs preserves thread/time window. |
| UI-059 | Shared navigation: Find → immediate result link | P2 corrected; deterministic regression passes | Latest menu request wins; stale completion cannot replace newer route metadata | Deferred-response browser fixture passes; live Jobs→Job Runs→run→Trace/Logs navigation succeeds. Original intermittent Find race is not claimed to be repeatedly reproduced. |

| UI-060 | B04 hosted mail setup Options → Back → Next | P2 fixed and retested | Unsaved name and filing policy survive Back and step-header navigation within the scoped wizard | Disposable100051: both paths preserve edits; Next/Finish saves; Add another account starts empty. Deferred-response/scope/teardown fixtures pass. |
| UI-061 | B04 hosted mail setup, Account and list | P3 fixed and retested | Hosted copy explains gateway delivery and activation; remote import controls are absent | Live100001/100051; list now shows Delivery / Mail gateway. Screenshot comms-hosted-list-dark-after-1920.png. |
| UI-062 | B04 Account → Open Inbox | P2 fixed and retested | Both links resolve the current app mount and preserve the account filter | Account100001 opens imported thread; setup-completion100051 opens its empty Inbox. |
| UI-063 | B02 thread reading | P2 fixed and retested | Named message and recipient disclosure buttons work with Enter/Space and retain focus | Dark1920, imported thread100000: older body/attachment and recipients expand/collapse; row name identifies subject, senders and count without nested action text. |
| UI-064 | B05 Text Blocks and Identities row actions | P2 fixed and retested | Delete has visible text and a record-specific tooltip; confirmation names the record | Keyboard activation opens confirmation; Cancel retains alias and text block. |
| UI-065 | B05 Text Blocks / Identities Edit → Escape | Not reproduced with real keyboard focus | Earlier direct locator activation ended with BODY focused; real Shift+Tab navigation to Edit then Enter/Escape returns to Edit | Dark1920: Identities Name/Signature and Text Blocks rich Text paths return to the invoking BUTTON. No speculative product correction. |
| UI-066 | M02 trace flame graph | P2 fixed and retested | Label foreground follows actual composited fill; clipped percentage frames preserve12px text without horizontal scaling | Live light/dark1920/390 and switches both ways; mounted fixture checks contrast, palette, zoom and responsive geometry. Yellow labels are black in both themes. |
| UI-067 | M02 Trace Detail title at390 | P3 fixed and retested | Shared wrapping utility keeps the full trace ID within the viewport | Live light/dark390: document scrollWidth390; title readable and selected-span controls retained. |
| UI-068 | Shared box headers, observed S12 ModelDetail at390 | P2 fixed and retested | Title remains above wrapping phone actions; Collapse/Expand controls name their section and expose expanded state | Live Model title width100.83px at390, keyboard toggle works; dark1280 and desktop grouping retained; mounted layout regression passes. |
| UI-069 | B02 thread message flag | P2 fixed and retested | Message action changes from Flag to Remove flag after acknowledgment | Dark1920: Enter flags Alex message; accessible name updates; Enter removes flag and restores original state. It is a next-action button, without aria-pressed. |
| UI-070 | B05 Out of office save → reload | P2 fixed and retested | Carry instants separately from public display strings and format once | Material, Standard and terminal Save/reload retain Nov8/9 09:00; normal/DST/null fixtures pass. Reply remains off. |
| UI-071 | D07 Reports overview | P3 fixed and retested | Group preview, playlists, scheduling and scheduled reports; bounded responsive forms and useful empty states | John Doe, both themes1920; light1280/390 and keyboard dialog cancellation. Populated8096 and empty8094 inspected. |
| UI-072 | D07 playlist playback | P1 fixed and retested | Authorized in-page dashboard data replaces the refused iframe; frame policy is unchanged | Both themes1920/390, keyboard Previous/Next, timed advance, pause/resume and owning-dashboard filter/time confirmed. Panel inspection now succeeds with its separate server-transition mount (UI091). |

| UI-073 | D07 TUI playlist after inaccessible items are filtered | P2 fixed and regression-tested | Retain each accessible item's original request index instead of its filtered position | Existing bounded-rotation test covers noncontiguous indices; terminal crate202 pass,1 ignored. No live inaccessible-playlist claim. |
| UI-074 | B05 terminal Out of office | P2 fixed and retested | No phantom N checkbox; row buttons remain usable; omitted read-only HTML is preserved, with explicit guidance | Fresh light/dark1920 and narrow hint checks; keyboard subject/toggle/Save preserves both bodies and09:00 dates. Explicit empty clears in Material. Long radio options remain UI087. |
| UI-075 | B04 wizard post-submit navigation superseded by browser Back | P2 source-confirmed and regression-tested | Wizard-owned navigation tokens release a superseded wait; obsolete callbacks cannot release a newer submission | Existing deferred A/B menu fixture checks retained wizard recovery and teardown. Shared navigation/wizard43/43 pass; this race is not claimed as a live browser reproduction. |
| UI-076 | D07 scheduled report state and actions | P3 fixed and retested | Active/Paused, Pause/Resume and Run now distinguish state from actions; confirmation names the saved delivery settings | Schedule100000, future2099/file-only: Resume→Active→Pause→Paused. Keyboard Run now/Cancel returns focus; no report executed. |


| UI-077 | D06 Alerts empty overview | P3 fixed and retested in batch12 | No page/rules/instances title and five empty tables show only headers; shared sections have weak separation | John Doe/dark1920 on8096. Evidence dashboard-alerts-dark-before-1920.png. Group rule activity, instances and shared delivery/quieting definitions without changing actions. Final correction/retest: see Batch12. |
| UI-078 | D06 AlertRule editor | P3 fixed and retested in batch12 | Long ungrouped form, generic Panel options/Save a version copy, raw paused N/Y, equally weighted threshold/range fields | John Doe/dark1920. Invalid threshold refuses and retains state; ui-d06-alert saved paused with value_count(trace_id) query and no contacts. Evidence dashboard-alert-rule-dark-before-1920.png. Final correction/retest: see Batch12. |

| UI-079 | D06 shared contact editor | P3 fixed and retested in batch12 | Full-width form shows notification, email and webhook settings together; empty Used in shows bare headers | Dark1920, ui-d06-contact. Configured private local ReportAlert self-notification; no email/webhook. Evidence dashboard-alert-contact-dark-before-1920.png. Final correction/retest: see Batch12. |
| UI-080 | D06 notification → Alerts → New alert rule | P2 fixed and retested in batch12 | Creation links inherit definitionId from the notification route; New alert rule reopens the existing record | Dark1920, ui-d06-alert notification. All four creation links carry the inherited ID; rule route also needs dashboard/panel context cleared. Existing detail links remain intact. Final correction/retest: see Batch12. |
| UI-081 | D06 Evaluate now feedback | P2 fixed and retested in batch12 | Submit successful appears even when pause or the interval prevents a fresh evaluation | Dark1920, interval60. Evaluated timestamp remains unchanged on immediate repeat. Existing service counters include skipped claims; UI must describe the request without claiming fresh results. Final correction/retest: see Batch12. |
| UI-082 | D06 firing → recovery notifications | P2 fixed and retested in batch12 | Firing and Normal notifications have the same title and warning icon | Dark1920, local ReportAlert self-notifications at20:56/20:58. Evidence dashboard-alert-notification-states-dark-before-1920.png. Both remain navigable, but their state is indistinguishable. Final correction/retest: see Batch12. |
| UI-083 | D06 AlertRule reload → change pause → save | P2 fixed and retested in batch12 | Saved1000000000 reloads as1,000,000,000 and fails finite-number validation on a later unrelated save | Dark1920, ui-d06-alert. Removing commas through the field permits Save and retains Paused Y. Inspect shared formatting and other JSON-backed numeric fields before selecting a correction. Final correction/retest: see Batch12. |
| UI-084 | M08 Replays search | P3 observed layout/empty state / fixed in batch 16 (deferred at wrap-up) | User/Visit inputs span1920px and both initial empty and no-match result show only table headers | Dark1920, no recording fixture yet; filter ui-review-no-recording and clear both complete. Evidence monitor-replays-dark-before-1920.png. Playback and masking remain unverified. |
| UI-085 | M09 Releases → Release Detail | P3 observed hierarchy/readability / fixed in batch 16 (deferred at wrap-up) | Empty regression list has no explanation or heading; revision/times are compressed into prose and error fingerprints dominate row labels | Dark1920, moqui0.1.0. Error link reaches the corresponding known schedule-validation error; Back retains release context. Evidence monitor-release-detail-dark-before-1920.png. Synthetic comparative regression/disposition remains pending. |

| UI-086 | TUI Ctrl+K → Apps → Comms | P2 fixed and retested | Normalize local shell navigation to /apps while preserving query and unrelated URLs | Comms/Gallery open, F3 returns, Gallery quick-note opens/cancels, and Contrived search opens EXT_CONTRIVED detail. Synthetic route fixtures also pass. |
| UI-087 | B05 TUI Reply to at390/1280 | P2 fixed and retested | Shared radio layout wraps labels and reserves their rows within the field width | All three audience choices remain readable at1920/1280/390; arrows select each, Tab moves to frequency, restored Everyone saves and agrees with Material. |
| UI-088 | M08 enable recording → Gallery interaction → Replays | P1 observed capture failure / fixed in batch 15 (deferred at wrap-up) | Initial replay exceeds256KiB receiver body limit and capture stops without user feedback | Confirmed server log03:14:31 and Monitor error; no recording stored. All temporary settings restored. Preserve masking and bounded ingestion in the correction. |
| UI-089 | M08 replay player data request | P2 source-confirmed route mismatch / fixed in batch 15 (deferred at wrap-up) | report.appPath changes the /apps JSON transition into a /qapps HTML wrapper | Same distinction as playlist loading; focused private patch prepared. Live playback awaits a successful recording fixture. |
| UI-090 | Shared dark drawer current/on-path labels | P2 fixed and retested | Readable semantic text replaces the brand-fill color for current/on-path labels | Current row improves from2.77:1 to5.76:1 dark;4.97:1 light. Default/Warm, desktop/phone and selected-bar preservation checked. Focus fill remains separate UI096. |
| UI-091 | D07 embedded playlist → Inspect panel | P2 fixed and retested | Keep the original server screenUrl for inspection/export transitions while navigation uses the Material mount | Reproduced JSON parse error, corrected, then live Inspect returns diagnostic data with no alert. Existing fixture clicks Inspect and asserts /apps endpoint plus definition/panel IDs;47/47 passes. |
| UI-092 | Standard Dashboard tabular fallback | P3 observed readability / fixed in batch 15 (was deferred) | Time columns display epoch milliseconds instead of human-readable dates | Targeted /apps playlist link reaches Monitor overview and shows data, but time1793242800000 requires interpretation. Retain compatibility; refine shared fallback formatting later. |
| UI-093 | B06 Chat member/destination pickers | P1 fixed and retested | Read-only choice transitions accept the existing widget POST as well as GET | User search, room membership, quick/main direct-message creation and forwarding complete locally. Existing authorization assertions pass; record search has no configured live provider and remains fixture-only. |
| UI-094 | B06 main/thread/edit message editors | P2 fixed and retested | Give each editor a distinct name and update it without replacing editor state | Main/thread/edit names verified; room rename and restoration update the composer name while retaining its unsent draft. Draft then cleared without posting. |
| UI-095 | B06 keyboard message actions | P2 fixed and retested | Labelled message articles expose the toolbar on keyboard focus and retain it through teleported menus | Tab reaches reactions/reply/thread/More; Enter opens Edit and Escape preserves text/returns to composer. Forwarding also completes. |
| UI-096 | Shared drawer keyboard focus | P2 fixed and retested | Inset semantic outline separates focus from the selected-row fill and bar | Selected Chat row shows a2px ring in default light/dark at390; existing fixture measures focus and selected states. Evidence shared-drawer-focus-{dark,light}-after-390.png. |
| UI-097 | B06 selected room preview in light mode | P2 fixed and retested | Use readable secondary foreground without reducing its opacity | Selected snippet improves from2.32:1 to5.72:1 light and measures6.99:1 dark. Draft/status colors remain separate. Before/after evidence retained locally. |
| UI-098 | B06 shell Create → Chat message | P2 fixed and retested | Use the supplied screen-container context to open the new-message picker immediately | Empty/no-match/clear/cancel/reopen and successful Start verified; a new local DM opens and a later main-screen Start returns to the same conversation. |
| UI-099 | B08 and shared multiple-choice inputs | P1 fixed and retested | Keep selected chips visible while retaining submitted IDs and cleared-value behavior | Calls recipient chip and hidden ID agree before and after unavailable-media refusal. Quick-chat clearing disables Start; fixture verifies keyboard clear and submitted values. Evidence shared-multiple-choice-visible-after-1920.png. |
| UI-100 | B08/B09 History filter recovery | P2 fixed and retested | Explicit empty parameters remove inherited outcome/folder/heard/page filters | Voicemail→Trash→Heard→Inbox preserves heard; heard All clears only heard; overall All clears voicemail context and returns to No calls here yet. |
| UI-101 | B06 direct-message and member pickers | P1 fixed and retested | Chat callers tolerate null cleared selections and main Start invokes its method without a click-event argument | Search/no-result/clear/reopen and quick/main Start pass; add-member succeeds and peer leave restores membership. Shared selection serialization remains unchanged. |
| UI-102 | M10 Incidents list | P3 observed layout/readability / fixed in batch 15 (deferred at wrap-up) | Full-width filters precede empty table headers; populated Detected dates show epoch milliseconds | John/light1920 on8094, empty list and incident100000. Evidence monitor-incidents-light-before-1920.png. |
| UI-103 | M10 IncidentDetail | P2 observed readability; P3 grouping / fixed in batch 15 (deferred at wrap-up) | Header and timeline show epoch milliseconds; update/note forms span the workspace and empty evidence/recovery sections provide no explanation | Minor incident100000, source and history agree but human-readable time and coherent sections are missing. Evidence monitor-incident-detail-light-before-1920.png. |
| UI-104 | M10 unknown IncidentDetail bookmark | P3 observed recovery presentation / fixed in batch 15 (deferred at wrap-up) | Main content contains only raw no incident ui-review-missing without a recovery link | Sidebar Incidents still returns to the list; preserve authorization and real error handling in the correction. |
| UI-105 | M10 Recovery edit flow | P2 observed edit friction / fixed in batch 15 (deferred at wrap-up) | Policies/adapters offer only Add or change; entering an existing ID does not load its values and rows have no Edit action | Entered DEFAULT in the policy dialog; Description/Steps remained blank. Canceled without saving. Provide explicit loaded edit state and preserve submitted values on refusal. |
| UI-106 | M10 Recovery page/dialog hierarchy | P3 observed layout / fixed in batch 15 (deferred at wrap-up) | Tall single-column policy form lacks logical groups; adapter form mixes all type-specific fields; Newest actions has empty headers | Default light1920; evidence monitor-recovery-policy-light-before-1920.png. Save remains reachable, but editing requires too much scanning/reentry. |
| UI-107 | M10 approved Manual recovery action | P2 observed missing task information / fixed in batch 15 (deferred at wrap-up) | AwaitingOperator shows Mark done/failed/Cancel but omits the saved manual instructions and adapter context | Action100000 correctly waits without infrastructure work; instructions exist in its requestJson but are not rendered. Display only the manual instructions, never arbitrary request payloads. Evidence monitor-recovery-waiting-light-before-1920.png. |

## Running implementation notes

### Batch 0 — plan and baseline

- Established the agreed plan, initial findings and coverage ledger in this document.
- Added the plan to the plans index. No application behavior changes.
- Verification: clean starting tree; `git diff --check`. Documentation-only batch, so no framework test run is needed.
- Next: shared semantic text contrast, list dialogs and terminal palettes, alongside independent app review.


### Batch 1 — shared readability, list workflows and administration/reporting entry points

Intent: correct shared contrast and dialog hierarchy before applying the same patterns to the individual apps.
The reusable method is in [UI-Review.md](../docs/dev/UI-Review.md).

- Separate semantic text foregrounds from brand fills; measure default light/dark text on plain panels, chips and
  tonal buttons. All seven semantic roles plus secondary text pass the focused 4.5:1 browser check.
- Find Options now groups filters in two columns, then sorting, then Clear Parameters/Find. Saved Finds separates
  applying/updating/default selection, saving a new find and optional email scheduling. Columns gains readable
  directional labels and tooltips. Preserve existing form IDs, routes and preference contracts.
- The embedded terminal synchronizes its background query during live theme switches. Section/dialog titles use
  the normal text palette independently from borders; xterm enforces 4.5:1 for ordinary text. Measured title pairs:
  dark 12.95:1 and light 16.67:1 (previous border-colored titles were 3.48:1 and 2.96:1).
- System and Tools entry points are grouped by task. Unsupported instance/cache metrics say Unavailable; timestamps
  are localized; obsolete Hazelcast and nonexistent generated-Groovy views are removed. At the user's explicit
  request, GroovyShell is removed rather than retaining an unavailable-screen placeholder. The web console/Terminal
  UI and SSH admin shell with its JS executor replace it. Its old screen route now returns 404; the unused permission
  seed and ADMIN_ADV grant are removed, while legacy permission sealing remains for existing databases.
- Dashboard discovery filters and metadata use grouped columns. Copying a shipped definition comes before its
  read-only editor; saved dashboards have an Open dashboard action. Panel controls use labelled responsive groups;
  read-only query selection remains available. Trace filters distinguish request context from time/result limits.
- Browser walks found two additional functional defects: service results were discarded by an unavailable legacy
  `ec.webImpl` method, and serialized appearance policy overwrote the mode with a boolean, resetting saved dark
  mode on reload. Both have focused corrections and regression coverage; browser retests passed after the build.

Observed activities so far (John Doe, isolated demo DB, primary 1920×1080):

| Activity | Observed outcome | Limits / next check |
|---|---|---|
| User list → Find → nonexistent username → result feedback | New filter layout fits; readable empty result and retained filter summary | Final 1280/390 filtering, reachable actions and Escape focus return passed |
| Save find → reopen → Make Default → reopen → Clear Default | Saved name/active/default states and reset to all 18 users behaved correctly | Update to current search and named-delete cancellation passed; Columns lifecycle passed; actual saved-find deletion remains unverified |
| System overview → Request details → Components | Health/task groups readable in both themes; platform gaps explicit; cookies redacted in request details | Components deployment-history wording remains misleading |
| SQL Runner → SELECT1 → missing column → SELECT2 | Correct one-row result, explicit error, successful recovery | Other data tools and access walks pending |
| Service detail → XML Actions → echo Run Service | XML visible; initial echo output blank reproduced | Echo text, default ping, numeric zero and corrected invalid numeric input verified |
| Dashboard definition → copy → native panel-title edit → Save version | Copy and edited title persisted; no console errors | Synthetic locator fill did not emit native change: automation artifact, not user data loss |
| Terminal → System → F1 → dark/light switches with help open | Headings/help readable; dialog and navigation retained across switches | Fresh dark launch and F1 help passed after saved appearance reload |
| Dev AIDE → Workspace → Go to artifact → UserAccount → source | Artifact lookup and source navigation worked; upstream source kept read-only | Missing dialog title/cancel and icon labels confirmed; next app batch |

Verification already completed: ScreenQvt 16/16; AdminOverview 5/5; MoquiHtmlCss 10/10;
WebrootReport 47/47; WebrootTerminal 2/2; QvtConfig 2/2; focused Rust terminal title test 1/1.
Authorized acceptance: tools Monitoring 19/19 (39.77s), GallerySmoke 23/23 (28.31s),
BasicComms call/guest/encrypted call/chat 10/10 (52.66s). These are automated journey results, not a claim of manual
visual coverage for every screen. Tracked-framework screen lint before GroovyShell removal: 363 files, zero parse
failures and 87 existing idiom/hazard flags; after removal, 362 files / 5435 expression sites, zero parse failures
and the same 87 flags. An earlier 371/88 scan counted framework templates twice because
MOQUI_FRAMEWORK used a different lexical path; the final environment uses the runtime's own framework link.

Environment lessons: use distinct local hostnames for independent sessions (cookies are not port-scoped). Use a
fresh local origin after changing cacheable browser assets. The first parallel sandboxed fixture process treated
an inaccessible live process as stale and removed review scratch databases; all review/test processes now run
with the same process visibility. Only disposable review DBs were affected. Keep the shared user runtime untouched.
The first framework gate identified expected generated-render/golden updates and an unrelated asynchronous agent
chat assertion; the script harness also inherited MOQUI_RUNTIME and failed its own path expectation. Regenerate
only affected artifacts, inspect their diffs, and rerun the gate without that inherited override.

Selected inspected screenshots are retained in [ui-refinement-evidence](ui-refinement-evidence/README.md), including
same-size before/after Find Options and Saved Finds. No wave is closed yet. The complete inventory below is a work
ledger, not a coverage claim.

Final browser retests passed for 1280/390 Find, Columns move/undo/hide/save/reset, saved appearance reload,
fresh dark TUI, unknown-service recovery and Standard overview/user filtering. A second native saved-find deletion
confirmation temporarily stalled pointer automation; keyboard input continued and the user subsequently cancelled
the prompt. Final saved-find deletion remains unverified. Browser zoom shortcuts did not change the reported
viewport or device scale, so actual 200% zoom is unverified; a separate 960×540 reflow check confirmed wizard fields
and Next remain keyboard reachable without horizontal overflow. This proxy is not recorded as a 200% zoom pass.

Additional palette check: Warm empty-list feedback measured 9.90:1 in light and 9.37:1 in dark. Default light palette
was restored afterward. Gallery wizard empty-name validation, correction, Back preserving text, Review and demo
completion were observed with explicit “nothing is written” success feedback. Landing counter labels no longer
truncate at 1920 after its local minimum card width changed from 150 to 260px.

The Dev AIDE documentation walkthrough exposed two more shared causes. Markdown links now use the semantic link
color (8.46:1 against the default dark background). Mermaid diagrams retain their source, redraw default themes on
mode changes, and close a stale zoom view; explicitly configured themes remain intact. The real-renderer regression
passes 38/38, including rapid switches, fresh dark and 3:1 relationship edges. Live package documentation passed
dark→light→dark and zoom reopening. Fragment-only history no longer reloads a screen: outgoing-reference jumps and
Back/Forward preserve an unsent artifact field. Different path/query navigation remains covered by the existing
shell regression. Final read-only integration review found no actionable shared-code defects.
The Standard References screen also completes the outgoing-section jump while retaining an unsent field value.

M02 was exercised manually in dark mode at 1920: Logs query `trace_id:*` → the failed Dev AIDE documentation
request → error service span → Logs for this trace → three correlated records → Surrounding entries → View entry.
Trace timing/error hierarchy, context anchor and JSON details were inspected; Escape returned focus to View entry.
The timeline's selected-span details appear below the whole trace rather than beside the selected row, a deferred
layout judgment. The filtered log histogram said No data while three records were present; this remains an observed
reporting discrepancy for investigation, not a verified query-engine cause. Monitor-only role and denied-dataset
walks remain pending.

Commit verification: `./moq test-framework --nobuild` ran with two framework workers, four build jobs and one Rust
test thread. Shell 342/342; framework server 348/348; demo 10/10; offline webroot 383/383; offline Dev AIDE 48/48;
served framework 148/148. The five environmental host failures were rerun successfully as recorded below.
`cargo test -p moqui-tui -j 4` passed separately, including its screen and session tests. Final lint: 362 files,
5435 expression sites, zero parse failures and 87 unchanged idiom flags. Generated clients/goldens and whitespace
checks passed. UI-025 through UI-028 are next-batch findings, not claims that this batch fixed them.

Local commits so far (unpushed): `67d3270f` establishes this plan and baseline; `883fffe8` contains the foundations, administration entry points, GroovyShell removal and guide; `da924d4b` contains retained-work, diagnostic and development flows; `1e88b494` contains diagnostic controls, breadcrumb access, cache/thread status and aligned CSV exports; `8c8ce50b` contains shared feedback, wizard and Gallery keyboard/recovery refinements. Each includes its verification notes. The evidence directory is retained locally and ignored by Git at the user’s request; the original foundations commit was rewritten to exclude it.

### Batch 2 — retained work, diagnostic recovery and workspace navigation

Intent: retain entered work on failure, prioritize ongoing tasks, and keep designer refreshes and Git exercises predictable.

- Compose closes only after a confirmed server/device save. Failed saves keep entered content, show Not saved and
  provide Retry save. Device storage returns its persistence outcome. Its in-memory cache still updates synchronously so chat badges and reopening a room see current text while host persistence is pending; failure retains that text in memory and reports false.
- A missing sending identity is explained before save/send. Mail accounts opens separately, and Refresh sending
  identities updates only identity choices without resetting the subject, body, attachments or signature already
  entered. The desktop offline save fallback remains available.
- Calls/Dialer and Call Handling use semantic unavailable-state banners; the inbox folder toggle has an accessible
  action name. Framework-only BasicComms specs tolerate the intentionally absent mr-boss component without loading it.
- Dev AIDE starts with workspace choices and compact creation dialogs. Search/References distinguish initial,
  missing and no-match states. Go to artifact has a title, Cancel, loading/error/retry handling and stale-result
  protection; empty Enter does not silently close it.
- Preview replacement marks the old iframe as leaving to suppress only expected cancelled requests. Both QVue
  design and XML screen-extension journeys now pass their final console checks.
- Acceptance fixtures use shorter PostgreSQL directory names for macOS Unix sockets. The merge journey waits
  until Commit is enabled after asynchronous conflict resolution; application Git semantics are unchanged.

Focused verification: BasicComms server set 18/18, ShellSync 9/9, MoquiLib 29/29, Dev AIDE client 26/26,
ArtifactIndex 6/6, preview clients 12/12. All eight Dev AIDE browser journeys passed; the detailed table below
records individual durations, initial failures, fixes and exact scope. These use disposable sites and local remotes.
Live Compose checks at 1920×1080 and 390×844 preserve subject/body after Close and identity refresh, with no browser
console errors. Successful external mail-provider setup/send remains unverified.

Final workspace lookup retest: empty Enter keeps Go to artifact open; a missing name shows the recovery hint;
correcting to UserAccount returns results and Enter opens SecurityEntities.xml at line 245. The source remains
read-only. A separate shell-menu Enter reproduced UI-028, so this session is not recorded as globally console-clean.

Local chat activity: created the private `UI review local room` with John Doe as its only member; posted a disposable
message and thread reply; inspected desktop and 390px thread layouts and used Back to return to the room. Attached
an 85-byte synthetic text file, sent it locally, found it in Files, and downloaded it. Downloaded bytes matched the
fixture exactly; only the review-created download was removed afterward. No external recipients or bridges were
configured. Room Settings also saved a new topic, verified in the room header. Multi-participant access, encryption/recovery, polls and call handoffs remain separate activities. The review runtime was then reset to a fresh isolated demo fixture; screenshots preserve this disposable activity evidence.

The second batch also includes UI-025–028: theme-aware canvas text (including radial tick backdrops), readable
automatic dashboard columns, inline query correction, save-snapshot acknowledgement and single keyboard menu
activation. References now opts into a section scroll margin; Standard keeps its normal document flow. The live
invalid-query recovery retains `now-15m`, hides Save Explore and restores results/save after `severity_text:INFO`.
Monitor automatic panels fit at 1920, 1280 and 390 without document horizontal overflow. Full timestamp labels
and unstyled report controls are newly recorded follow-ups, not visual sign-off of every report widget.

Cross-review corrected a draft-cache compatibility regression before commit: delayed host writes must not prevent
synchronous chat reads/badges from seeing current text. MoquiLib's cache again represents immediate UI intent,
while its returned boolean reports durable success. Delayed puts/removes and failure ordering pass in the existing
29/29 fixture. The line-chart test was also extended with real radar/polar canvases after reproducing white radial
text on the default pale backdrop at 1.71:1. Explicit chart colors/backdrops remain authored.

Gate 6 was intentionally interrupted during core Rust checks to include these review corrections and the invalid
query save guard; it is not a completed result. Gate 7 completed on frozen source with exit 1: all six non-Cargo
stages passed, but three unchanged SIP unit fixtures require loopback aliases unavailable on this macOS host.
All three fail at `ua_tests.rs:46` while binding `127.0.0.2`, `.4` or `.6`, with `AddrNotAvailable`, before their
source-isolation assertions. No host network configuration or SIP assertion was changed. The SIP crate had 57
passed and 3 failed; Cargo stopped there, so later crates/doc tests are not claimed as run by gate 7.
Host integration passed 166 with 23 intentionally ignored (701.74s), including all prior PostgreSQL failures.
The shell checks passed 342/342; framework server specs 348/348 (31.97s); demo 10/10 (16.82s); offline webroot
385/385 (49.51s); offline Dev AIDE 48/48 (4.48s); served framework clients 148/148 (48.87s).
Log: `/tmp/moqui-ui-framework-gate7.log`. This is a failed global gate with passing affected UI checks, not an
exit-zero full framework result; the loopback fixture portability issue remains separate follow-up work.
Final focused results: ShellSync 9/9 (4.69s), MoquiLib 29/29 (1.09s), shared chart/menu 30/30 (6.23s),
WebrootReport 47/47 (1.25s), changed query recovery acceptance 1/1 (6.93s), full Tools acceptance 19/19 (29.61s).
Final lint: 362 screens, 5437 expression sites, zero parse failures, 87 existing idiom flags. The client and desktop
screen bundles were regenerated successfully. Screenshots remain local in the ignored evidence directory.


Additional live activities during gate 7 (default dark, 1920×1080, John Doe):

- Saved `severity_text:INFO`/`now-15m` as `ui-review-info`, reopened with the saved title, query and range intact.
- Copied `monitor-overview` to `ui-review-overview`; native keyboard editing changed a panel title, Save version
  persisted it, and Open dashboard displayed it. The browser tool's fill helper does not commit these native
  change-on-blur inputs; native typing plus Tab does. Exported Job runs CSV has `outcome,runs` and `success,145`;
  only the review-created download was removed after verification, with a copy under `/tmp`. Applied the success
  filter, inspected its dataset scope, removed it and restored zero filters. No console warnings/errors.
- S01/S02: Cache filter `screen` → ScreenColorScheme summary/detail → Back; unsupported pattern `[` returns no
  literal matches, and correcting it recovers. Native-cache detail availability is UI-031. Thread availability and
  scheduler timestamp are UI-032. Application Instances has no rows; inspected Create App Instance and cancelled
  without provisioning. It manages configured deployments, not the list of currently running processes.
- S05: filtered `expire_UserStatuses`, inspected settings and error-only history policy, opened its empty history,
  cleared filters to all runs, opened run 100205 (`clean_ClientUploads_hourly`), read `removedCount:0`, and followed
  its correlated 2.78ms trace. Manual Run Job was not invoked because its native browser confirmation shares the
  previously observed automation blocker; no manual-run success is claimed.
- S11/S12: inspected LLM Dashboard zero-use state, Usage, empty/valid quota checks, Models/Endpoints and feature
  bindings. The default model clearly reports no provider model name. No external provider was invoked. Nested
  menu Enter worked with a clean console; its opener itself was found to lack keyboard focus (UI-034).


- S16: unpaired Edge Status → pairing wizard; empty Next reports required address; `http://` reports invalid URL
  and retains the fields; Edge node link cancels back to unpaired status. Edge Clients explains the full-node role
  and links back. No pairing/trust/apply action or external connection occurred; a successful peer fixture remains pending.
- B12/B13: Voice Status distinguishes supported/configured/active, no SIP server and an active local media server;
  Trunks explains its empty state and next setup step. Chat Bridges explains no adapters and queued-message behavior.
  No trunk, bridge, PSTN call or external provider was configured or invoked.
- S03 limited role: existing `sec.view.only` fixture on a separate origin, default light at 1920. Launcher exposes
  Comms/System/Tools; Monitor/Dashboards/Dev AIDE/Gallery are absent. System diagnostic links needing Monitor are
  disabled. Create User Account opens a fully disabled form, with disabled Create; no account or credential write.
  Direct Monitor bookmark reports View not permitted; All applications recovers to the allowed launcher. The enabled
  create-dialog opener gives no explanation for its disabled form, a deferred shared affordance refinement.
- T05 repeat on the final isolated runtime: `SELECT 1 AS ui_review_value` returns one row; `SELECT FROM` displays
  a syntax error with inputs retained; correction to `SELECT 2 AS ui_review_value` returns one row without navigation.

### Batch 3 — diagnostic readability, keyboard access and export alignment

Intent: resolve findings from the preceding live review while retaining chart data, query parameters, cache
contracts and navigation routes. Work covers compact time-axis labels, explicit theme styles for report-created
native controls, grouped Explore controls, keyboard breadcrumb menus, truthful cache/thread availability and
localized scheduler times. The actual CSV export found a shared header/row mismatch (UI-036); its correction belongs
in the existing CSV renderer and render fixture, without duplicating an application render.

Live checks so far: Thread List at 1920 light/dark and 390 light shows unavailable platform metrics, readable
scheduler time and stacked cards without horizontal overflow. ScreenColorScheme reports one cached entry with
zero enumerable details and explains the limitation. Its old long heading overflowed to 678px at a 390px viewport;
the name is now separate from the title with an opt-in wrapping utility. Material and Standard cache details both
retested at 390px with document width 390px and no console errors. The dark breadcrumb text resolves to white on
#1d1d1d (16.9:1), replacing the previously measured 2.72:1 gray. Breadcrumb Tab → Cache → Enter/Space
opens the menu; Escape returns focus; child Enter navigates once with no console warnings/errors.

Tools T02 fixture: created `UiReviewType`, found it by prefix, updated its description, attempted a duplicate,
then corrected the key to `UiReviewTypeRetry` without re-entering the retained description. Both rows are visible
and the original retains its updated description. Export contains exactly those two records but initially has
two extra action headers. No database-maintenance action was invoked. Deletion remains unverified because the
browser's native-confirmation problem is still open; these rows remain in the disposable review DB.

Focused verification for this batch:

- `AdminOverview`: 7/7, feature 1.5s within its 3s budget; `/tmp/ui-admin-cache-threads-regression-final.log`.
- Existing reporting fixture: 47/47 after time-axis/native-control changes; `/tmp/ui-refinement-report-time-axis.log`
  and `/tmp/ui-querybar-layout-after.log`. Final live review caught an overlapping Add filter disclosure; the empty negative-gutter row was
  replaced by explicit spacing and the native disclosure marker restored. Final regression: 47/47 in 1.41s
  (`/tmp/ui-querybar-spacing-after.log`); live Enter/Space opening/closing, filter creation and 1920/1280/390 reflow passed.
- Existing QVT client fixture: 30/30, feature 6.8s within 34s budget; `/tmp/ui-native-qvt-after.log`.
  Precompiled-client golden regeneration/check: 1/1; `/tmp/ui-breadcrumb-template-generation.log`.
- Tools Monitoring acceptance: 19/19 in 46.23s; `/tmp/ui-refinement-monitoring-layout.log`.
- CSV regression failed before the fix and passed afterward. `cargo test -p moqui-core --all-features -j 4 -- --test-threads=4`:
  1458 unit, 4 browser-library and 12 native-component tests passed, 3 unit and 2 doc tests ignored;
  `/tmp/ui-csv-alignment-core.log`. Only the affected existing CSV golden changed.
- Changed cache-heading compatibility assertion: named Tools/System host acceptance 1/1 in 27.58s;
  `/tmp/ui-cache-heading-tools-acceptance.log`. `cargo lint` and `cargo lint-min` passed;
  `/tmp/ui-csv-alignment-lint.log`, `/tmp/ui-csv-alignment-lint-min.log`.
- Screen lint: 362 files, 5446 sites, zero parse errors; 87 existing review flags. Command:
  `env -u MOQUI_CONF MOQUI_RUNTIME=/tmp/mui-8092-5l9n060c target/release/moqui lint --screens /tmp/ui-admin-cache-threads-screen-lint.txt`;
  log `/tmp/ui-admin-cache-threads-screen-lint.log`, exit 0.

The first browser export retest still used the old release binary: ScreenCsv is embedded by the generator's
`include_str!`, so editing the JavaScript file cannot hot-reload that macro. A current debug framework binary was
built with `CARGO_BUILD_JOBS=4 cargo build -p moqui -j 4` (2m39s; macOS linker unwind-table warning only), then
started in a separate disposable fixture runtime on port 8094. The actual browser CSV export now passes: exactly
`Enum Type ID,Description`, with two review rows, correctly quoted comma/quote text and a preserved empty final cell.
Evidence: `/tmp/ui-review-entity-export-after.csv` and local `tools-entity-export-after-1920.jpg`. The review-created
Downloads file was content-checked, copied to `/tmp`, and removed. The user clarified that the long download wait
was the harness awaiting their permission, not a product failure or server timeout.

Final Explore screenshots: local `monitor-explore-dark-after-1920.jpg` and `monitor-explore-dark-after-390.jpg`.
Default light and dark, 1280px, native disclosure focus, actual INFO filter creation and submission passed; console
errors were empty. Filter pills still expose raw JSON when no label is supplied, recorded as follow-up UI-037.
Full framework gate 8 completed with
`env -u MOQUI_RUNTIME MOQ_TEST_WORKERS=2 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 ./moq test-framework --debug --nobuild`
(`/tmp/moqui-ui-framework-gate8.log`, exit 1). The current debug binary includes the corrected embedded CSV generator.
Functional framework results: server 350/350, demo 10/10, offline webroot 385/385, offline Dev AIDE 48/48 and served
browser 148/148. Shell checks passed 342/342. Core tests passed 1458 unit, 4 browser-library and 12 native-component
checks; host integration passed 166 with 23 ignored. Cargo still stopped at the same three SIP tests that bind
unaliased macOS loopback addresses (`127.0.0.2`, `.4`, `.6`); their socket setup fails before the assertions.
Later Cargo suites were not reached by this gate.

Debug server regression timings exceeded the 15s spec limit for ArtifactIndex (49.63s), ArtifactDocs (28.68s) and
EntityModeler (28.13s), and component limits for Tools and Dev AIDE. A release server-stage rerun passed all 350
functional checks in 53.98s, but ArtifactIndex was still just over its limit at 15.79s under concurrent browser
and other-workspace test load (`/tmp/moqui-ui-framework-server-release.log`, exit 1). After this gate finished,
`env -u MOQUI_RUNTIME ./moq test --nobuild dev-aide --spec ArtifactIndex` passed 6/6 with 6.6s feature time
(`/tmp/moqui-ui-artifact-index-isolated.log`, exit 0). This targeted command selected only the Dev AIDE spec but used
the repository's full component registry in its disposable fixture; no addon tests were selected. No budget was
raised, and these narrower results do not turn the failed full gate into a passing result. The release browser
server remains on 8090; the current debug review server remains isolated on 8094.

Read-only continuation during gate 8 — Gallery:

- G08, John Doe, default dark, 1920 and 390: cleared Customer name → Next stayed on Customer with inline error;
  corrected to `UI review customer`, continued to Options, unchecked newsletter, and verified Review showed No.
  Back through both steps retained the name and option. Completing at 390 returned to Customer defaults with the
  explicit “gallery demo: nothing is written” message. No horizontal overflow. Validation helper text is
  `#c10015` on `#1d1d1d` (2.63:1), visibly too dark (UI-038); phone step names vanish and completed step buttons lose
  accessible names (UI-039). These are next-batch shared fixes, not closed by the passing wizard flow.
- G04, Dialog: New focuses ID; Escape closes; dynamic View loads the correct ENTCO_BETWEEN record. The 390px
  detail dialog fits the viewport with its close control visible. Keyboard New → Enter → Escape returns focus
  to BODY instead of New (UI-040). Create's native confirmation remains outside this manual run. The new dialog
  defaults Type to Agent Message Part Kind despite the Comparison operators list; inspect the Gallery default
  in the next batch. Existing server acceptance is supporting coverage, not proof of these UI checks.
- G05 Handheld: A-100 auto-submit, NOPE retained with no-match message, DUP-1 chooser cancellation returns focus
  and retains code, choosing the lot resolves it, wrong-type A-100 in Bin is refused, B-01-07 waits for explicit
  Confirm then reports success and refocuses/clears Bin. GS1 `(01)04006381333931(10)LOT-7` opens a lot/product chooser
  and resolves LOT-7. Default dark at 1920/390 and light at 390 inspected, no console errors. Success toast white/bright-green
  contrast belongs with UI-038; the persistent scan result already uses the readable semantic foreground.
- G10 quick create on ENTCO_BETWEEN: Gallery note and task both show the exact Enumeration context; each Save
  closes the dialog, leaves the detail route and reports explicitly simulated work. Empty note/task values are
  currently accepted by these demos; this is not evidence of required-field refusal. Dirty-discard remains pending.
- G07 Split at 390 light: Country=United → four results → United States opens a correctly named detail, with an
  honest no-regions state. Countries returns to an unfiltered list; selected-row navigation dropped the find
  parameters (UI-041), contrary to the Gallery explanation. No data edited.
- G06 Board at 1920 light: dragged Acme from Qualify to Propose; counts/sums updated from 3/$229,000 to 2/$109,000
  and 2/$248,500 to 3/$368,500, with successful transition feedback. Search with no match shows 0/10 and empty columns;
  clearing restores cards. Cards are draggable divs without keyboard semantics or a movement alternative (UI-042).
  The source transition writes nothing, so this verifies simulated movement, not persistence.
- G06 Calendar at 1920 light: clicked November 18 → New event shows 09:00–10:00; Create accepts an empty title and
  closes with explicit nothing-written feedback. Week shows its unsupported-view explanation and Back to month
  recovers. Next updates the bookmark to 2026-12; Today returns to November. At 390 the grid stays within the viewport
  with event dots. Calendar arrows/dates/events and Done-event contrast are follow-up UI-042.
- Calendar → Task produced `screen path not found: events under component://example/screen/Gallery/Task.xml`.
  The Calendar watcher resolves a request against the destination during navigation (UI-043). Task Confirm reloads
  the same first step with demo feedback; subsequent Short advances to line 2 but leaks `stepOnly=Y` into the full
  route, hiding the surrounding sections (UI-044). Source investigation has identified local fixes for the next batch.
- Local evidence: `gallery-wizard-error-dark-before-1920.jpg`. Full application/wave coverage remains open.

### Batch 4 — shared feedback and Gallery recovery

Implementation began after batch 3 committed. Shared styles/widgets, Gallery XML/server acceptance, and QVue/browser
acceptance have separate owners; one owner integrates all existing GallerySmoke rows. No edits began while batch 3 was frozen.

- Shared field errors: use the semantic negative foreground for helper text; keep automatically changed-field
  backgrounds subtle enough for error labels. Preserve explicit field background overrides. Scope notification
  styling to built-in fill/text pairs and verify captions/actions along with body text.
- Wizard: retain visible and accessible step titles at phone width; preserve previous-step-only navigation.
  Dialog: reproduce opener capture/refocus in an existing mounted fixture before adding restoration behavior.
- Gallery Split: capture find/operator/case/sort parameters before row context replaces field names; preserve that
  whitelist through country links and Edit responses. Set New Enumeration's supported initial Type to ComparisonOperator.
- Board: add a keyboard Move control sharing the existing pointer-move path and a focusable title link. Describe
  demo-only movement accurately. Calendar: name month/date controls, expose mobile event descriptions, use readable
  status presentation and capture/guard its endpoint across SPA navigation. Remove its dialog on unmount.
- Task: keep fragment-only request parameters out of full-page navigation and inspect the intended demo progression.
- Editor: clear comparison/version parameters when closing a comparison or changing its page/space; preserve the
  intended page and current-version return path. UI-046 was reproduced during this batch's additional activity work.
- Message: derive its local image fixture's absolute URL from the actual request scheme/host instead of guessing
  HTTPS for non-literal localhost names. Image blocking and trusted-sender behavior remain unchanged.
- Extend existing fixtures and existing Gallery acceptance rows; no additional sweep or duplicate regression renders.
  Changed cases and full example acceptance passed after integration.

Additional live activity coverage on the isolated 8094 runtime, John Doe, default light at 1920:

- G04 Contact (hidden fixture, opened directly): empty Name and invalid Email produce field-specific server errors,
  retain values, and set `aria-invalid` plus `aria-describedby`. Correcting both produces an explicit simulated-save
  message and restores demo defaults. The expected failed request is recorded as a console warning; no real contact is written.
- G04 Search → New example: created `100000`, named `UI review code field`, Type Made Up, JSON description. Create
  opens EditExample with the saved record. Size `not-a-number` is refused inline with the value retained; correction
  to 12 saves successfully. This is a real write in the disposable database.
- G09 CodeField (hidden fixture, opened with exampleId=100000): changed JSON content, saved, reloaded and observed
  the exact persisted content. The editor lacks an accessible name (UI-045). No unavailable-library fallback was forced.
- G09 Editor: the shipped EXAMPLE space is database-backed (`dbresource://ExampleWiki/EXAMPLE`), separate from source.
  Added one review line to Welcome with a version note; current version advanced 02 → 03, History shows John Doe and
  the note, and comparison shows the added line. Close and page-switch recovery initially failed as UI-046 and now pass.
  The diff describes all old lines as removed and all new lines as added; a read-only SQL check through Tools confirms
  CRLF normalization as UI-049. Restore/Publish/git-space creation
  remain pending native confirmation handling. No filesystem source or external remote was edited.

Integrated retests on a fresh browser hostname, same disposable 8094 database:

- G08 dark 1920: error helper is #ff8a80 on #1d1d1d (7.38:1); changed-field label is 6.19:1. At 390, step titles
  remain visible and named; Customer Enter from Options returns with the draft retained. Completion resets the draft.
- G04 dark 390: New starts with Entity Comparison Operator and focuses ID. Escape and Close both return to the exact
  New button after the dialog hides. This no longer reproduces UI-040; the mounted Chromium fixture agrees.
- G07 dark 390: United → United States → Countries retains the four matching rows and all find operators.
- G06 dark 1920: Enter Move → Propose moves Acme, updates Qualify to 2/$109,000 and Propose to 3/$368,500, and restores
  focus to Acme's Move button. Feedback explicitly states temporary demo movement; its measured contrast is 6.12:1.
  Light presentation also inspected. Calendar date Enter opens correct 09:00–10:00; Escape returns date focus.
  Next/Today preserves the intended month, mobile event names remain available, and SPA exit to Task has no stale 404.
- G05 light 1920: Confirm's actual background message is “Line confirmed; the step reloaded”; Short then keeps the
  full page at line 2 with five remaining lines and no stepOnly parameter.
- G09 compare Close removes comparison content and parameters; Notes opens its own single-version page.
- G13 default light/dark, 1920/390: designed and plain messages readable; trimmed content opens by Enter; the actual
  local image loads after Show images, Trust shows it, and Hide restores blocking. Unsafe fixture link/form remain
  non-interactive text. No external message or external image request was used.
- Forest light/dark: invalid Contact fields remain readable on both panel and changed-field backgrounds: light
  5.62:1/4.86:1, dark 7.69:1/6.33:1. Corrected input produces tonal success feedback at 7.56:1 in dark mode.
  Restored Default/light afterward. Actual 200% browser zoom remains
  unverified; responsive checks are not represented as zoom checks.
- Fresh light web TUI: Account → Terminal UI → System → F1, at 1920×1080. Runtime groups, headings and shortcut
  help remain readable. This completes the fresh-launch color comparison; it does not close other TUI activities.

Verification:

- Existing QVT precheck 32/32, feature 9.9s/34s; `/tmp/ui-feedback-dialog-after3.log`. Existing layout fixture 6/6,
  2.05s total; `/tmp/ui-wizard-layout-after2.log`. These checked real field/notification contrast and actual dialog focus.
- Existing precompiled goldens regenerated with `UPDATE_GOLDEN=1 CARGO_BUILD_JOBS=4 cargo test -p moqui-core --all-features --lib are_precompiled -j 4`:
  3/3 passed; only the wizard client template changed in tracked output. `/tmp/ui-gallery-batch4-precompiled.log`.
- Focused Gallery server acceptance 59/59 in 4.74s; `/tmp/ui-gallery-navigation-acceptance-final.log`. Screen lint:
  362 files, 5463 sites, zero parse failures, 87 unchanged flags; `/tmp/ui-gallery-navigation-screen-lint.txt`.
- Focused GallerySmoke 23/23 in 29.40s after correcting four fixture assertions/selectors (not app source);
  `/tmp/ui-gallery-batch4-browser-retest.log`. Expected validation 500 is consumed only after its specific field error is asserted.
- Complete isolated `./moq accept --nobuild example`: server 59/59 in 5.40s, browser 23/23 in 30.66s; exit 0,
  `/tmp/ui-gallery-batch4-full-acceptance.log`. The existing 23 browser rows are unchanged in number.
- Required framework gate 9: `env -u MOQUI_RUNTIME MOQ_TEST_WORKERS=2 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 ./moq test-framework --nobuild`;
  `/tmp/moqui-ui-framework-gate9.log`, exit 1. Server 350/350 in 43.97s (all component targets met), demo 10/10,
  offline webroot 388/388 assertions, offline Dev AIDE 48/48, served browser 148/148, shell 342/342. Core 1458 unit
  plus browser/native tests and host 166 integration tests passed; Cargo again stops on the three previously recorded
  SIP loopback-address failures. Later Cargo targets are not represented as passed. No addon suites or accept-all were invoked.
- The full offline webroot run failed its timing gate: QVT spec 15.52s against 15s; aggregate feature time 49.3s/34s
  is a warning below the 1.5× failure threshold. Removed the new, redundant UI-040 dialog-focus regression fixture:
  there is no corresponding source fix, and the existing Gallery acceptance row plus manual keyboard review already
  cover this behavior. Product source did not change after gate 9. Focused final
  `env -u MOQUI_RUNTIME -u MOQUI_CONF ./moq test-client webroot --offline --spec WebrootVue.qvt --nobuild`
  passes 31/31 in 8.80s, feature time 8.1s; `/tmp/ui-batch4-qvt-budget-retest.log`. No budget was raised. The aggregate
  warning remains recorded; the complete gate was not repeated for deletion of a duplicate test.

Local inspected evidence: `gallery-wizard-error-dark-after-1920.png`, `gallery-board-dark-after-1920.png`,
`gallery-calendar-light-after-1920.png`, `terminal-help-light-after-1920.png`; compare the prior wizard baseline.
Images remain ignored and uncommitted.

Additional administration baseline while the batch-4 gate runs (John Doe, 8094, Default/light, 1920×1080):

- S06 Audit Log: initial screen requires a filter. Entity `Example` gives an intelligible empty result; reopening
  Find Options with `moqui.example.Example` yields 11 seeded status changes and preserves the filter. Their user/visit
  values are absent seed context, so they do not prove a user-to-visit audit drill-down. Unnamed shared controls are UI-051.
- S06 Visits: list shows the two local review sessions and retained-request counts. Visit 100001 opens its metadata
  and request history. Error=Y narrows 230 retained requests to three, including the deliberately invalid Gallery
  Customer and Contact submissions. No client-IP lookup was invoked. Metadata presentation defects are UI-052.
- S06 → M02: the Customer validation request's trace link opens Monitor Trace Detail with its exact trace and time
  window. Enter on the POST span exposes HTTP 500 and `Enter the customer name`, route, duration and visit context.
  Timing-label foreground #000000 on #c10015 is 3.28:1 (UI-053); a panel-colored text shadow does not provide a reliable
  readable background. Local evidence: `system-visit-detail-light-before-1920.png`,
  `monitor-trace-detail-light-before-1920.png`. This does not close S06 or Monitor permission/isolation coverage.
- M02 continuation: Logs for this trace returns the two correlated WARN/ERROR entries in the preserved range;
  Surrounding entries opens the Monitor-owned context page with an anchor indicator and both entries. View entry
  opens the selected JSON document; Escape closes it, and browser Back restores the original query/range. No new
  console errors appeared; the two recorded 500 warnings belong to the intentionally invalid Gallery submissions.
- S06 user context: the log's user link opens the correct nested User Account Detail route. Account Audit Log opens
  its seeded username change; Escape closes it. Visits returns to the user-filtered list.
- S11/S12 initial unavailable-provider inspection: LLM Dashboard shows zero runs/calls/usage and empty failure tables.
  Models lists the conf-backed default endpoint/model and explains that the provider model name is missing, including
  affected feature bindings. Model detail retains that reason and distinguishes conf values from database overrides.
  Test and Probe were not invoked; no external request or model configuration change was made. The ungrouped long
  override form is UI-054; evidence `system-model-detail-light-before-1920.png`. Successful local-provider activity,
  validation/retry, limited actor and other LLM administration groups remain pending.

### Batch 5 — confirmation, query labels and Editor recovery

Implementation began after batch 4 committed. Agents prepared private patches while the prior source was frozen;
the coordinator reviews and applies each patch, regenerates client templates, runs tests and owns the live retest.

- Material confirmation: replace native confirmation for data-confirm and m-link with explicit Confirm/Cancel in
  the existing dialog stack. Preserve normal form validation, submitter, modifiers and one authorized replay;
  Cancel/Escape/dismissal must not submit. Tree-move and reload native prompts remain a separate follow-up.
- Code fields: forward the visible field label to CodeMirror's editable surface and failed-bundle textarea;
  reconfigure optional accessible names without replacing text or undo state.
- Query pills: summarize known typed predicates, ranges and negation; preserve authored labels and original query
  data. Unknown/ambiguous structures retain their exact JSON display. Align Add filter controls using existing styles.
- Editor: distinguish explicit Home from the omitted default Welcome path using the existing null sentinel;
  keep Home through reload/save/history navigation. An unavailable comparison leaves the page and recovery controls
  usable. Normalize Editor Markdown input/diff presentation to LF; retained resource history and restores stay exact.
- Task guide: remove the obsolete handheld-density claim and link the existing Handheld example.
- Extend existing Gallery List/Editor activities, without another sweep or more browser rows. Actual acceptance
  outcomes, contrast evidence and the required complete component/gate results are recorded after integration.

Verification to date:

- Existing Gallery server acceptance:59/59 in5.35s; complete isolated `./moq accept --nobuild example`:server59/59
  in5.91s and browser23/23 in37.28s, exit0. Existing List and Editor activities extended, no extra sweep/rows.
  Log `/tmp/ui-batch5-gallery-acceptance.log`.
- Focused offline client regressions:WebrootReport47/47 in1.24s; WebrootVue.qvt33/33 in11.50s (feature10.5s);
  WebrootVue.qvt.markdown39/39 in4.84s (feature3.4s). The initial editor-name fixture returned a DOM object from a
  wait predicate, hitting the CDP serialization-depth limit; boolean predicates fixed the fixture, with no product
  behavior change. Logs `/tmp/ui-batch5-report.log`, `/tmp/ui-batch5-confirmation.log` and
  `/tmp/ui-batch5-editor-names-retest.log`.
- Precompiled client checks3/3; regenerated only the changed m-text-area render. Screen lint362files/5473sites,
  zero parse failures and87 existing flagged sites; final log `/tmp/ui-batch5-screen-lint-final.txt`.
- Live John Doe/default dark1920:created saved find100000, Cancel and Escape each leave it present and return focus
  to Delete in the underlying Saved Finds dialog. Confirm deletes it; reopening shows no item. Console warnings/errors
  empty. The browser acceptance additionally counts zero requests on cancellation and one on approval; focused
  regression covers required-field validation, submitter, duplicate click, modified links and stale controls.
  Evidence `saved-find-confirmation-dark-after-1920.png`. Tree move and re-login native prompts remain outside this fix.
- Live Editor/default light1920:Home bookmark/reload and unchanged Save keep root page/version01. Invalid comparison
  bookmark shows local warning and leaves content/Edit/History available; Close clears the bad comparison. Existing
  mixed LF/CRLF Welcome versions02/03 compare as2 added/0 removed. Default dark1920 visually inspected as well.
  Evidence `gallery-editor-recovery-light-after-1920.png`, `gallery-editor-diff-dark-after-1920.png`.
- Live CodeField/default dark1920:the actual editable surface has name Description (JSON), preserving the stored
  JSON. Failed-bundle fallback and live name updates pass the mounted regression, not a manual network-failure walk.
- Live Explore/default dark1920:Add filter without Label shows `severity_text = "ERROR"`; Search preserves the typed
  filter in its URL and returns7 error records. The same summary is readable at390 with no page overflow; light1920
  expanded Add filter aligns the action with its fields. Evidence `explore-query-label-{light,dark}-after-1920.png`.
- Additional G09 activity during the gate:at390/light, invalid comparison Close by Enter recovers Home; History
  opens and Escape returns focus to History with no page overflow. At1920/light, view EXAMPLE Welcome02 → Restore
  creates04 while keeping older versions; publish03 updates the marker, restore03 creates05 and preserves the review
  content; publish01 restores the original marker. Publish lacked a confirmation (UI-055), so this successful local
  write is not a passing safeguard check. All writes remain in the disposable database.
- Create the EXGIT database repository through its confirmation → edit Welcome → save a note → History → prev:
  new commit52f2ebee15 follows599d084608 and comparison shows2 added/0 removed. Console warnings/errors empty.
  Evidence `gallery-git-diff-light-after-1920.png`; remaining Editor refinements UI-056/057. No source checkout or
  external remote was changed. Light confirmation text21:1, Cancel5.98:1, Confirm4.60:1 on its rendered fill.
- Task → Open the Handheld example reaches the scan/Code/Bin page through the visible link.
- S05/default light1920:Service jobs → filter clean_ClientUploads_hourly → detail → Run Job → Material Confirm →
  Job Runs →101012. Finished as John Doe with Has Error N and `removedCount:0`; View trace opens matching
  50ef2ec81ac71ee11dc1ebdc388fb11e with6.35ms span. Job settings were unchanged. This completes the previously
  blocked safe manual-run branch; failed-run/limited-actor/settings-edit branches remain pending. UI-058 records
  the result hierarchy; UI-059 is a suspected quick-navigation race, kept separate from successful run coverage.
- B04/default light1920: created hosted account100001 (`ui.review@example.invalid`) through the wizard; invalid
  address recovery and activation succeeded. Options Back/revisit loses unsaved name/policy (UI-060); saving via Next
  retains Only what I link. Hosted setup/account copy also exposes inapplicable remote-sync controls (UI-061).
  Account's two Open Inbox links are disabled by an obsolete mount path (UI-062). No external account was connected.
- B02/default light1920: imported two synthetic messages into that account's Inbox from `/tmp/ui-review-mail.mbox`:
  UI reports2 imported,0 duplicates,0 unreadable. The matching thread contains Pat/Alex, both bodies and38B agenda.txt.
  Expanded the older message and recipient details; their click-only surfaces lack keyboard controls (UI-063).
  Search `subject:does-not-exist-ui-review` shows No conversations match and Clear filters returns to Unified inbox.
  Evidence `comms-thread-light-before-1920.png`; sending, attachment download and complete triage remain unverified.
- Required `env -u MOQUI_RUNTIME MOQ_TEST_WORKERS=2 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 ./moq test-framework --nobuild`
  gate10 finished, exit1: three known SIP tests fail binding loopback aliases (`AddrNotAvailable`,57pass/3fail).
  Every other invoked stage passed: shell342/342; framework server350/350 in46.60s; demo10/10; offline webroot390/390
  in55.56s; Dev AIDE48/48; served framework148/148 in48.07s. Webroot feature time40.1s/34s is a warning below the
  existing1.5×failure threshold; no budget was raised. Core1458unit+4browser-library+12native and host344unit+
  166integration (23ignored,690.83s) passed. Cargo stops at SIP; later workspace crates are not claimed as passed.
  Log `/tmp/moqui-ui-framework-gate10.log`. `git diff --check` passes and the evidence directory has no tracked files.
  Actual200%browser zoom remains unverified; a narrow viewport is not recorded as zoom.


### Batch 6 — named list controls and readable diagnostic detail

Batch5 committed locally as `b1a2ffc2`. This batch addresses UI-051–057: explicit ascending/descending controls,
field-specific filter names, truthful Visit Detail metadata, readable trace bars/durations and selected-span context,
grouped model overrides, named Markdown content, publish confirmation and compact version headings. Routes, saved
column/filter values and full version identifiers are preserved.

Cross-review identified valid legacy sort syntax (`NULLS LAST`, ASC/DESC, UPPER/LOWER wrappers) that the initial
exact-token parser did not recognize. Correct this before generator verification. Trace duration columns can grow
with long values; text remains outside the timing bar. Existing focused fixtures are extended rather than adding
application renders or another sweep. Broader Comms findings UI-060–063 remain a separate prepared batch.

- Initial focused WebrootReport:47/47 in1.10s, including both default-theme text/bar contrast and unchanged raw
  span data; log `/tmp/ui-batch6-report.log`. A subsequent duration-column CSS adjustment will be covered by the gate.
- During build, B05/default light1920: Text Blocks initial empty state → nonexistent search → New by Enter → blank
  name error → bad shortcut error retained all fields → correct/save → edit/save/reopen retained revised rich text and
  Just me scope. Screenshot `comms-text-block-light-1920.png`; UI-064/065 remain follow-ups. Compose text-block menu
  matches the saved entry and has a useful no-match state; inserting it, entering a subject, Close → Drafts → reopen
  retains both subject and body, with Saved status. No recipients or Send action; fixture retained for later checks.
- Generator regeneration325passed/1ignored; precompiled clients3/3; debug build passed (existing large unwind-table
  linker warning). Initial list-control fixture33/34: cleared range fields retain Quasar's empty mask rather than null;
  form serialization already omits unfilled masks. Corrected this fixture expectation, without product behavior change.
- Focused list controls34/34 in12.84s; static sorting26/26 in1.26s; Visit readers2passed/1external-OpenSearch-ignored
  in10.73s; existing LLM integration1/1 in3.84s. Complete Gallery acceptance59/59server in6.85s and23/23browser in42.90s.
  First Gallery run22/23 failed an order-sensitive JSON-object comparison in the new assertion; compare request fields
  directly instead, retaining the exact-one-write check. Native QVT rows also require `tr, .tr` in that fixture.
  Workspace Clippy and host no-default-feature Clippy passed. Logs `/tmp/ui-batch6-*.log`.
- Live8096/default light1920: Visit100000 shows HTTP request for static assets, localized millisecond timestamps,
  labeled host/instance and no undefined type. Started descending remains descending on repeat; ascending changes
  direction and aria-current correctly. Find Options names each field control; Request options open by Enter and
  close by Escape. Request trace link retains its bounded time window and opens matching selected-span details.
- Trace38ddbf4d default light/dark1920: readable separate duration/track, HTTP200, GET, /Login and local start summary;
  Raw span data opens/closes with Enter. Mobile retest found UI-066/067, so narrow trace coverage is not yet passing.
  Evidence `visit-detail-light-after-1920.png`, `trace-detail-{light,dark}-after-1920.png`.
- Model default/dark1920: related controls group visibly; invalid Temperature reports a field error and retains
  Description. Correct0.5, set API patch{}, collapse advanced section and Save Row; database override created and
  reopening advanced shows{}. At390 values persist and form does not overflow, but header title shrinks away (UI-068).
  All changes are disposable; remove the override after follow-up retest, without calling a real provider.
- Follow-up UI-066–068 fixes are now retested: final trace screenshots in light/dark at1920/390 show readable flame
  labels with fixed12px text; scrollWidth equals390. Both theme-switch directions work, selected-span keyboard
  expansion works, and console error/warning collection is empty. Evidence `trace-detail-{light,dark}-final-{1920,390}.png`.
- Model phone title now measures100.83px with wrapping actions; Collapse/Expand works by Enter. Dark1280 retains
  side-by-side groups without overflow (`model-detail-dark-final-1280.png`), and light1920/390 screenshots show the
  final header. Remove Row → Cancel retains the override; Confirm returns to ModelList with Source conf, and reopening
  shows an empty Description. No provider Test/Probe action was invoked.
- Standard `/apps/system/AuditLog` names Date ascending/descending; repeated descending preserves the URL and
  ascending changes it correctly. Full-page navigation was awaited between clicks; empty Standard table remains an
  inspection, not a completed audit investigation.
- Final WebrootReport47/47 in1.12s; header layout7/7 in2.94s. The initial header fixture lacked the Material Icons font
  and displayed expand_less as overflowing text; loading the production icon stylesheet corrected the fixture.
  Precompiled client regeneration passed. Screen lint scans362 files/5484 expression sites with zero parse failures
  (87 existing idiom/hazard sites;202 fancy templates compiled). `git diff --check` passes.
- Source frozen for complete gate11, command `env -u MOQUI_RUNTIME MOQ_TEST_WORKERS=2 CARGO_BUILD_JOBS=4
  RUST_TEST_THREADS=1 ./moq test-framework --nobuild`; log `/tmp/moqui-ui-framework-gate11.log`. The gate's CLI specs
  use the existing release binary; changed embedded generators are additionally covered by rebuilt debug core tests,
  updated goldens, complete Gallery acceptance and live8096. Comms private patches remain separate.


- B05 rule activity during the gate: default light1280/1920, hosted account100001. New rule → disable On and select
  Flag it → blank Name refusal retains conditions → subject Planning lunch dry run matches2/2; nonexistent subject
  returns0/2. Cancel creates no rule and returns focus to New rule. Save a uniquely named disabled rule → reopen
  retains Off/Subject/Flag/account → dry run still2/2; Escape returns focus to Edit. Dark390 fits without horizontal
  overflow and Tab from Test scrolls Save into view. Delete → Cancel keeps the row and permits reopening; Delete
  confirmation removes it and returns No rules yet. Rule was never enabled or executed. Both imported messages
  remained unflagged before the separate triage test. Evidence `comms-rule-test-light-{1280,1920}.png` and
  `comms-rule-dark-390.png`. No console warnings/errors collected.
- B02 triage: row Flag → Flagged view contains the fixture → Remove flag → Nothing flagged. Row action labels and
  filtered results update correctly; message control lacks the matching Remove flag name (UI-069). State restored.
- Gate11 exposed two old framework expectations: the HTML active arrow formerly toggled direction, and QVT reset
  tooltip formerly said Reset to Default. Updated these existing fixtures to assert named directions/current state
  and the new Reset filters to default accessible name. The existing built devkit template test passes1/1 in1.95s.
  Gate11's first Cargo stage stopped at the obsolete assertion (core1458+4+12 passed); server specs349/350 identified
  the reset-copy assertion. A clean final gate will follow after all gate11 stages finish.


- The final served gate11 failure was HandheldScan's substring locator for Confirm: the newly named Collapse Put away:
  scan the bin, then confirm header also matched. Select the exact Confirm action. Existing HandheldScan now passes3/3
  in13.35s wall time/4.8s feature time. Gate11 otherwise passed shell342, demo10, offline webroot392 and Dev AIDE48;
  served147/148. No budget was raised. Gate12 reruns the complete gate after these fixture corrections, with source frozen.
- B05 Identities: default light1920, non-default `UI review alias <ui.alias@example.invalid>`, Signature UI review
  signature, placement above the quote. Invalid address blocks submission and retains Name; corrected Add saves,
  Edit reopens with signature/placement intact. Delete confirmation identifies the alias and fallback identity behavior;
  Cancel retains it. Alias retained for UI-064/065 retests. Escape from the keyboard-opened Edit leaves BODY focused,
  reproducing UI-065 beyond Text Blocks.
- B05 OutOfOffice: explicit no-server warning and Off status; reversed From/Until rejected with The automatic reply
  ends before it starts, preserving subject/body/dates. Correct Until saves while disabled. Reload retains subject/body
  but changes09:00 to03:00 on both dates (UI-070). Keep the disabled fixture for the correction; no automatic reply or
  outbound mail was sent. Wide form grouping remains a design-review opportunity, separate from the functional defect.

- UI-065 qualification: explicitly focus the Identities Edit button before Enter, confirm active BUTTON, then Escape
  from either Name or the rich Signature editor. Both return focus to Edit. Earlier direct locator activation did not
  establish the same precondition; a product focus-loss cause is not established. Do not add speculative focus code.
- S12 endpoint/model lifecycle,8096/dark1920: Add Endpoint with blank optional fields creates100000 and shows the
  default path. Set Description UI review inert endpoint, Base URL http://127.0.0.1:1 and Disabled Y; save/reopen
  retains values. Add model ui-review-model, endpoint100000, provider model ui-review-inert. Usable and Test both
  explain that the endpoint is disabled. This exercises refusal without a real provider request. Model removal Cancel
  retains it; Confirm removes it; remove the endpoint and verify both fixtures disappear. Alias/client flows remain pending.
- Runtime preservation: logical save-db backup of8094 completed with both PGDMP files, then load-save restored them
  successfully into /tmp/mui-8094-b7-restore/db/embedded. The original8094 remains alive and has received no further
  fixture writes after backup. Cut over only after the next binary builds; preserve the backup and original until
  known fixture IDs are verified. This avoids copying a live database or losing the existing manual-review state.
- D07 initial activity,8096/dark1920: inspected Reports grouping (UI-071). New playlist blank Save reports Choose
  between one and fifty dashboards; duration4 reports Choose a duration from five to3600 seconds, preserving fields.
  Correct5 creates ui-d07-playlist with monitor-overview/monitor-services. Timed title/counter advances, Pause and
  keyboard Previous work, but iframe refuses to connect (UI-072). Retain the fixture for diagnosis/retest; do not claim
  successful playback or close D07. CSV/PDF/schedules remain separate pending activities.
- Gate12 completed: shell342/342; framework350/350 in45.62s; demo10/10; offline webroot392/392 in60.84s;
  Dev AIDE48/48; served browser148/148 in49.73s. Webroot feature time44.4s warns above34s but stays below
  the51s failure limit; no budget changed. Cargo passed core/devkit/host integration and the other completed packages,
  then failed only the same three unrelated moqui-sip tests with macOS AddrNotAvailable at ua_tests.rs:46
  (additional loopback address unavailable). Consequently the overall gate exits1; this is not an all-green gate.
  Affected UI regressions, Gallery acceptance, screen lint and diff checks pass. Log /tmp/moqui-ui-framework-gate12.log.
- D07 report composition: ui-d07-report saves Services then Monitor overview after Move up, landscape/A4/rows,
  Appendix on and absolute2026-11-04T00:00:00Z through2026-11-06T00:00:00Z ranges. Reload retains all settings.
  Native change-bound fields needed actual typing and Tab in the browser harness; fill alone changed the DOM value
  without committing the change event. CSV preview generates a Download report link named report.zip. The browser
  download action was rejected because data: URLs are disallowed, after waiting on the permission harness. Do not
  bypass that policy or claim the ZIP contents/download have been verified. Keep the saved report for later review.

### Batch 7 — mail workflow recovery and settings

Batch6 committed locally as `92ca156a`. Applied bounded corrections for UI-058–064 and UI-069/070, leaving
UI-071/072 Dashboards work as private patches for a subsequent batch. This batch preserves local mail Options
drafts across Back/header navigation, resolves mounted Inbox links, distinguishes hosted delivery from remote sync,
names keyboard message/recipient disclosures and delete/flag actions, groups Job Run Detail, and corrects the
OutOfOffice date roundtrip with a grouped form and accessible shared date/time pickers.

The wizard opt-in names an exact form, scalar field allowlist and account URL scope. It stores only in the mounted
wizard, excludes secrets and remote member selections, and discards on scope change/teardown/Finish. Submitted
snapshots are acknowledged individually so delayed responses cannot erase newer edits. Navigation menu responses
now accept only the latest request. Existing fixtures cover these races; verification results are recorded below.

Date cross-review caught a Standard/TUI compatibility problem in the first private proposal: numeric instants would
format as numbers in those modes. The integrated screen converts the additive service instants to Date values;
existing public display strings remain unchanged. Static QVT serializes host timestamps as epoch text, so the
shared explicit timestamp formatter now recognizes that representation too. The existing static fixture and one
new focused BasicComms feature cover normal/DST/null values without another application render.

Initial integration included embedded-template/precompiled-client regeneration and a debug rebuild for changed
generators, followed by the focused checks, live restored8094 review, lint and complete gate below.

- Generator325passed/1ignored in40.21s, precompiled clients3/3, debug build25.12s (existing linker unwind-table warning).
  Restored8094 started successfully with isolated persistent storage and retained hosted account100001, subject/body
  and dates. Original test-server shutdown completed; logical backup remains intact.
- Focused server BasicComms/AdminOverview16/16 in8.25s, including date normal/DST/null boundary. The initial added
  Account render started at its parent with lastStandalone=-2; correct it to-3 and use the existing wizard render
  to assert opt-in attributes at the same nested boundary. No extra render was added.
- Shared browser fixtures: first run69/72 exposed fixture synchronization/return-value mistakes, not established
  product failures. Wait for actual handleResponse after Response.text, return booleans from popup waits/side-effect
  evaluate calls, and use supported locator APIs. QvtKeys passes3/3; wizard/layout9/9 in2.98s after avoiding the
  action helper's10-second settle wait on the deliberately active loading mask. Submitted/newer/scope/teardown and
  actual completion-path assertions all pass. Static timestamp and navigation-race fixtures passed in the first run.
- Mail ShellSync9/9 in4.38s,3.3s feature time. Its existing Inbox case now mounts real sandboxed message previews;
  Chromium reports the blocked injected script. The in-page harness supports only a per-case allowance, so that
  known page-problem allowance is confined to this case; all message/body/focus/flag/network assertions remain.
- Requested BasicComms acceptance passes10/10 across CallFlow, CallGuestFlow, EncryptedCallFlow and EncryptedChatFlow
  in49.80s, using local participants and fake media. This does not establish external SIP/SMTP/provider delivery.
  Command MOQUI_RUNTIME=/tmp/mui-b7-specs-tj9gmjvq ./moq accept --nobuild basic-comms; log
  /tmp/ui-batch7-comms-acceptance.log. Screen lint362files/5517sites, zero parse failures,87 existing idiom/hazard
  sites and202 fancy templates. Gate13 was started with source frozen; subsequent corrections and gate results follow.
- Live8094: corrected OutOfOffice displays both stored dates at09:00. Save/reload retains09:00; Standard native
  datetime controls also show09:00, and unchanged Standard Save preserves it when returning to Material. Both
  picker buttons open by Enter/Space and Escape returns focus without changing values. Default light/dark1920
  grouping is readable; dark390 stacks with no horizontal document overflow (clientWidth/scrollWidth375 with the
  scrollbar). Collapsing Reply content keeps scheduling controls reachable. Automatic reply remains disabled.
  Evidence comms-away-{light,dark}-after-1920.png and comms-away-dark-after-390.png (viewport capture).


- B04 live hosted wizard: account100051, `ui.wizard.b7@example.invalid`. On Options, Back→Test→Next retains
  unsaved name and Only what I link. A revised name also survives keyboard header→Test→Next. Next and Activate
  account complete setup with gateway copy. Both Account100001 and completion100051 Inbox links work with their
  own account filter. Add another account starts with a blank address. Remove/keep mail: Cancel retains; Confirm
  soft-removes100051 with zero messages. Original100001 and its imported mail remain intact.
- B02 live dark1920: Enter expands the older Pat message and agenda.txt38B, with focus moving to the named Collapse
  control. Enter/Space operates recipient details and collapse. Alex Flag→Remove flag→Flag follows acknowledgment;
  the fixture is left unflagged. The row name is concise and includes two senders and the message count.
- B05 live dark1920: both Delete actions are named and confirmation Cancel retains the fixture. Actual Shift+Tab
  navigation to Edit, Enter, then Escape returns to Edit for Identities and Text Blocks, including rich text.
  UI-065 is not reproduced under a normal focus path; no focus-specific source change was made.
- TUI display compatibility: Comms→Settings→numbered Out of office link shows Nov8/9 at09:00. Screenshot
  comms-away-tui-dark-after-1920.png. This exposed UI-074 shared controls and an app-specific rich-field preservation
  risk; terminal saving is explicitly unverified. Moving Save/applyToAll out of a field-row avoids introducing a
  blank submit control with this layout. Fresh light TUI launch confirms visible Save and no hidden-field label;
  evidence comms-away-tui-light-after-1920.png. Full shared correction remains a subsequent bounded batch.
- Material date recovery: clear both dates → Save → reload keeps both empty; reversed dates produce the explicit
  ends-before-starts alert and retain the entered values. Correcting and saving restores Nov8/9 at09:00 on reload,
  with subject and rich body unchanged and reply still Off.
- S05 live run101012: successful outcome, removedCount0, elapsed5ms from recorded start/end, timezone and runtime
  metadata are grouped. Light/dark1920 and light390 inspected. First390 render exposed intrinsic-width columns;
  adding xs=12 makes sections343px wide in a375px document with no horizontal overflow. Trace drill-through finds
  the retained6.35ms telemetry span (a different measurement from the persisted5ms run timestamps); Logs opens the
  exact thread and bounded time window. Evidence system-job-run-{light,dark}-after-1920.png and light-after-390.png.
- Final two XML layout corrections were applied while gate14 was still in Rust host tests, before component specs.
  Focused BasicComms/AdminOverview rerun16/16 in8.66s; final screen lint362files/5517sites/zero parse failures,
 87 existing flags and202 fancy templates. Logs /tmp/ui-batch7-final-layout-specs.log and
  /tmp/ui-batch7-screen-lint-final.txt.
- Gate13 used the old release binary accidentally. It failed the new wizard-attribute assertion because its embedded
  generator predates this batch, plus an unrelated HTTP-resource fixture received an empty request. The exact HTTP
  resource test passed on rerun. Gate14 uses `./moq test-framework --debug --nobuild` with the regenerated debug
  binary, two framework workers and one Rust test thread; its final result follows. Do not count gate13 as a pass.
- D07 next-batch observation on8096: invalid cron is refused with the6/7-field explanation; file delivery without
  a location is refused. Created schedule100000 for ui-d07-report, CSV/file-only, cron `0 0 0 1 1 ? 2099`, location
  `dbresource://ui-review/report-output`, then paused it. Immediate-run confirmation Cancel retains the paused
  schedule and returns focus. No file was rendered or acquired and no mail was sent; this does not bypass or resolve
  the browser's blocked data-URL download. UI-076 copy refinements join the private Reports proposal.


- Final source cross-review found UI-075: suppressing an obsolete menu completion could leave the still-mounted
  wizard submitting after browser Back. A wizard-owned navigation token now releases only its superseded wait;
  completion, a new POST, refusal and teardown invalidate the old token. Root callbacks for obsolete routes remain
  suppressed. Existing deferred A/B fixture covers cancellation, newer submission and teardown. Focused shared
  navigation/wizard43/43 passes in16.39s, component feature time15.2/34s; log
  /tmp/ui-batch7-navigation-cancel-tests.log. Integrated before gate14 began the component JavaScript suites.
- Gate14 completed exit1: shell342/342, server352/352 functionally, demo10/10, webroot394/394, Dev AIDE client48/48
  and served framework148/148. Cargo failed only the same three SIP fixtures at ua_tests.rs:46 with macOS
  AddrNotAvailable. The debug server run failed timing budgets: ArtifactIndex27.73s, ArtifactDocs24.44s, Tools5.2s
  against3s, Dev AIDE53.5s against30s. Webroot44.4/34s is a warning below its51s failure threshold. No budgets changed.
  Rebuilding release with the current embedded generators to rerun the complete server component selection under
  its intended performance profile. Keep the full gate recorded as nonzero, even if targeted recovery passes.
- Stopped obsolete8090 review server cleanly and closed obsolete browser tabs4–23/25. Current8094 persistent
  runtime and8096 report fixtures remain live; keep tabs24/26/28. No Purchasing process/runtime/checkouts touched.


- Release rebuild passed in4m18s, four build jobs, /tmp/ui-batch7-release-build.log. Repeated the full framework
  server selection with `env -u MOQUI_RUNTIME -u MOQUI_CONF ./moq test --nobuild -- framework basic-comms tools webroot
  dev-aide example --workers 2`:352/352,51specs,37.25s, exit0. Every server component is within its normal budget:
  framework10.2/12s, BasicComms5.9/8s, Tools2.2/3s, Dev AIDE17.2/30s, example1.0/3s. No spec exceeds15s.
  Log /tmp/ui-batch7-release-server-gate.log. This resolves the debug-profile timing failures and verifies the
  current release generators; it does not relabel gate14's known SIP/environment failure as a clean full gate.
- Bounded batch7 is ready for its local commit. Shared, administration and communication waves remain open;
  UI-061 account-list copy, UI-071–074 and UI-076 are prepared follow-ups, with terminal saving and external-provider
  success explicitly unverified. Evidence files remain ignored and absent from the branch's commit history.


### Batch 8 — report module preparation

Batch7 committed locally as `2b9e7e5e`; no files under the evidence directory were included and nothing was pushed.
Before UI-073 changes, the Rust practices require extracting large inline test modules in their own commit.
`crates/moqui-tui/src/components/report.rs` contains two such modules. A private mechanical proposal will preserve
`report::tests`/`report::gap_tests` names through path attributes and isolate the small ReportPlaylist component.
No behavior change is intended in this preparation commit; focused crate tests, Clippy and the framework gate follow
integration. UI-082 also touches a report module with a large inline test block: its nine tests move unchanged to
`crates/moqui-report/src/alert_tests.rs`, retaining their `alert::tests` paths. The separately reviewed UI-073 and
UI-082 fixes will then target the extracted files. UI-074 forms/HTML-preservation
and contextual guidance, UI-071/072/076 Reports and UI-061 hosted list copy remain separate unapplied proposals.

Parallel read-only/live observations continue against8096: D06 Alerts starts empty, and New alert rule exposes
UI-077/078. Used visible controls to select otel_spans, add a value_count aggregation on trace_id, name
`ui-d06-alert` / UI review local span count, and select Paused Y. Threshold `not-a-number` is rejected with a finite
numbers error and preserves the form. Corrected0 saves successfully. No contact point or outgoing provider is
configured for this rule, and evaluation/delivery/quieting branches remain pending.

- Applied the four-file mechanical extraction. The playlist implementation and remaining production behavior are
  unchanged; the two test modules retain all24 test names under their original module paths. `cargo test -p moqui-tui`
  passes202 with1 ignored across14 test binaries/doc-tests; workspace `cargo lint` passes in29.18s. Logs
  /tmp/ui-batch8-tui-extraction-tests.log and /tmp/ui-batch8-extraction-clippy.log. Gate15 is running with the current
  release binary for server specs and newly compiled Rust tests; no behavior patch is included yet.
- D06 local silence: reversed ISO range is rejected and retains comment/dates; corrected Nov6→Nov7 UTC saves a row.
  Remove acts immediately and removes that disposable rule-local silence. Rule remains paused and has no contacts.


- D06 recurring mute:25:00/26:00 is rejected and retains inputs. Friday23:00→01:00 saves, but weekday displays raw[4]
  (UI-078). Removed that disposable mute before notification evaluation. Shared contact `ui-d06-contact`, title
  UI review local notification, Notification/ReportAlert toEX_JOHN_DOE, groupWait0/repeat14400, private sharing,
  no email/webhook, saves successfully. Used shared item from the rule, then followed its link: Used in names the
  rule. Delete Cancel retains it; confirmed Delete is refused with the referencing ID/title and recovery instruction.
  Both contact and rule remain present. Local notification delivery and alert evaluation have not yet been executed.

- D06 evaluation/delivery: activated the rule with threshold0 and evaluated it, producing Firing at20:56 and a local
  self-notification. Threshold1000000000 followed by an eligible evaluation produced Normal at20:58 and a recovery
  notification. An immediate repeat request returned generic success without a new evaluated timestamp (UI-081).
  Both history entries have identical title/warning icon (UI-082). Following the first notification and choosing
  New alert rule reopened the existing definition because creation links inherited its ID (UI-080).
- Re-pausing the fixture exposed UI-083: the reloaded numeric threshold contains grouping commas and Number(value)
  rejects it. Re-entering1000000000 and saving succeeds; the fixture is now paused with its shared local contact
  retained. No silence or recurring mute remains; no email/webhook was configured. No Data/Error, suppression and
  failed-sink activity branches remain unverified. All observed evidence remains local and ignored.
- The report alert test extraction preserves the production prefix byte-for-byte and extracted bodies modulo
  whitespace. Fresh `cargo test -p moqui-report` passes74/74; final workspace Clippy passes in19.15s, logs
  /tmp/ui-batch8-alert-extraction-tests.log and /tmp/ui-batch8-final-clippy.log. Gate15's earlier report tests ran the
  equivalent inline source; this focused run covers the extracted source. No UI082 behavior is included here.
- M08/M09 read-only coverage while the gate runs: Replays begins empty; no-match user filter and clearing it work but
  leave bare table headers (UI-084). Releases shows the current moqui0.1.0 marker; its detail has no regressions and
  seven error groups from the local review. The schedule-validation error link opens its message/trace context, and
  Back returns to the release. This covers navigation/empty state only, not playback, comparison or disposition.
- Gate15 completed: `./moq test-framework --nobuild` returned1 solely for the same three SIP unit-test
  `AddrNotAvailable` failures at ua_tests.rs:46. Host SIP integration passed. All remaining sections passed:
  shell342, framework server352/352 (31.35s, all component budgets), demo10, webroot394, Dev AIDE client48 and served
  client148. Webroot44.9/34s warns but stays below the51s failure limit. Log /tmp/moqui-ui-framework-gate15.log.
  No budgets were raised or failures suppressed. This mechanical commit has passing focused crates/Clippy;
  the full gate remains nonzero and the recurring SIP fixture assumption is being inspected separately.

### Batch 9 — playlist playback and terminal form preservation

Batch8 committed locally as `f26f815b`. Applied UI-061 hosted-account list labels, UI-071/076 Reports grouping and
schedule action copy, UI-072 authorized in-page playlist rendering and UI-073 original playlist indices, plus
UI-074 terminal check/row controls, omitted HTML preservation and read-only guidance. A private cross-review found
that an embedded dashboard filter used the Playlist route; it now opens the owning dashboard with its filter and
displayed absolute time. Ordinary Monitor routes remain unchanged. No framing/security policy was relaxed.

- Fresh terminal crate regressions pass; core TUI generator26/26 pass; debug build passes in49.80s. BasicComms8/8
  passes in6.06s (2.4/8s component), including explicit-empty versus omitted HTML and unchanged09:00 dates.
  A missing closing brace initially prevented the report fixture from registering; corrected before rerun.
  WebrootReport47/47 passes in1.15s (0.2/34s). Debug and release builds pass. Logs /tmp/ui-batch9-*.log.
- M11 on isolated8096: created15-minute Replay enabled=true, Replay sample ratio=1 and RUM enabled=true overrides,
  each with a review reason. Ratio2 is rejected with0–1 guidance and retained inputs; correcting1 succeeds.
  Active overrides expose owner/reason/expiry; one-setting Revert restores Replay=false. Revert all Cancel preserves
  the remaining two, Confirm restores RUM=false and ratio0.1, and full reload removes WebrootRum from the document.
  Evidence monitor-settings-overrides-dark-1920.png and monitor-settings-restored-dark-1920.png.
- M08 capture remains unverified: with all three overrides active, full reload emitted replay=true/sample1 settings
  and the rrweb script, and a disposable Gallery navigation/filter interaction completed. Replays remained empty.
  No relevant console error appeared. Server log03:14:31 confirms receive#BrowserReplay failed because its body
  exceeded the configured256KiB limit (UI088). The error also appears in the Monitor overview. Inline styles alone
  exceed300KiB. Bounded capture and playback request-mount proposals are separate follow-ups, not successful coverage.
  All overrides were explicitly restored. No external browser data or provider was involved.
- Repeated SIP gate failures are platform-fixture prerequisites: macOS has127.0.0.1 but no separately bindable
 127.0.0.2–.6 aliases. The three security tests fail before assertions. Different ports would invalidate IP-based
  trust/rate/block coverage, so no skip or weakened test was added. A portable virtual UDP fixture is separate work.

- Live UI074 on8094: terminal Save changed the subject with both rich bodies preserved. Enabled was toggled on/off
  before save; reply stayed off. Material restored the subject and explicitly cleared the temporary internal body;
  reload retained that clear, the original external body and09:00 dates. Hint is readable at1920/1280/390; radio clipping
  is recorded separately as UI087. Fresh dark and light screenshots are local evidence only.
- Live D07 on8096: playlist loads actual panels, keyboard Previous/Next retain focus, Resume advances automatically,
  and Pause stops rotation. Filtering the overview's service cell opens its owning dashboard with the same absolute
  time bounds and filter. Light/dark1920 and390 screenshots inspected. Reports at1920/1280/390 retain grouped forms;
  phone dialog Save/Close are reachable and closing returns focus. Schedule100000 resumes then pauses correctly;
  keyboard Run now/Cancel returns focus without running the report. Empty playlists/schedules inspected on8094.
- A live load exposed /qapps HTML rewriting on the playlist JSON source; preserve its supplied /apps transition URL.
  The regression now uses the actual server mount. Final review then found the same distinction on embedded panel
  inspection (UI091), reproduced through Inspect. Fixed the embedded screenUrl without changing navigation links;
  fresh-client retest returns inspection/requests data with no alert. The extended existing fixture passes47/47
  in1.08s. Evidence dashboard-playlist-inspect-dark-after-1920.png. Export download remains unverified.
- Gate16 found the expected TUI generator golden change; regenerated and inspected its three-line diff. Gate17
  completed with only the three known SIP loopback-bind failures: shell342, server352/352(30.42s), demo10,
  offline webroot394, Dev AIDE48 and served client148 all pass. Webroot44.4/34s warns, below51s failure threshold.
  Final screen lint:362files,5527sites,0parse failures,87pre-existing idiom flags;202fancy bodies compiled.
  Log /tmp/moqui-ui-framework-gate17.log. No budgets raised or failures hidden.
- Gate18 after UI091: core1458, shell342, server352/352, demo10, webroot394, Dev AIDE48 and all148 served client
  features pass. The overall command remains nonzero: two host tests could not initialize Postgres because macOS
  shared-memory IDs were exhausted, and served framework timing16.8s exceeded its9s failure threshold during
  concurrent build load. This Rust run stopped at the host binary before reaching the known SIP unit failures;
  it is not a new SIP result. After verifying zero attachments and dead creator/last-user PIDs, removed five stale
  56-byte segments; both exact host tests then pass (7.54s and6.28s). No active database or kernel limit changed.
  The separate served framework rerun passes135/135 in17.67s, with5.7/6s feature time. Webroot45.9/34s warns but
  remains below51s. Logs: gate18, sim-step/duplicate retests and framework-client-retest. No threshold was changed.
- Standard compatibility on8096: Reports keeps the grouped forms and Paused/Resume state; New dashboard playlist
  opens and keyboard Close returns focus. Standard Playlist intentionally lists dashboard links, and Monitor overview
  opens with its tabular data. The fallback still exposes epoch-millisecond time cells; formatting is deferred (UI092).
  Evidence dashboard-reports-standard-after-1920.png. No Standard autoplay claim.
- Reusable guide now covers authorized embedded rendering, owning-dashboard context, latest-response identity and
  preserving omitted read-only HTML without preventing explicit clears. Screenshots stay ignored and untracked.

### Batch 10 — shared navigation and terminal layout

Batch9 committed locally as `cc99c4ce`. This batch addresses UI-086/087/090 before continuing app-specific fixes.
The terminal shell converts only local /qapps navigation to /apps, preserving suffixes, unrelated mounts and external
addresses. Radio choices stay inline when they fit and otherwise wrap within their own field width; layout and
drawing share the row calculation. Drawer current/on-path text uses the existing theme-aware link foreground while
retaining brand fills, the selection bar and authored overrides. Independent source review found no blocker.
Existing shell and contrast fixtures are extended; one synthetic terminal draw regression covers label wrapping,
adjacent-field boundaries and unchanged keyboard selection. Focused checks, live retest and the framework gate follow.

- TUI203 tests pass, one documentation example remains ignored; workspace Clippy passes; debug build completes
  in1m33s. The contrast fixture initially measured Quasar's in-progress color transition. It now disables animation
  for that rule check and follows the real drawer dark class; no production timing changed. WebrootVue.qvt34/34
  passes in13.14s after the build, within the15s spec limit. An earlier run during builds exceeded15s; no budget raised.
- Live UI090: default current-row contrast is4.97:1 light and5.76:1 dark; active app5.53:1/6.79:1. Warm current-row
  contrast is5.68:1 light/6.33:1 dark and active app6.34:1/7.55:1. Default scheme restored. At390 the drawer opens by
  Enter and normal Tab reaches its links; selected text and the brand bar remain visible. Focus fill itself is a
  separate follow-up (UI096). Evidence shared-drawer-{light,dark}-after-1920.png,
  shared-drawer-warm-dark-after-1920.png and shared-drawer-dark-after-390.png.
- Live UI086: Ctrl+K→Comms reaches the real Inbox, F3 returns to App List; Gallery opens its Landing. F6→Gallery
  note opens the authorized form and Escape cancels. Search Contrived returns the enumeration and opens Detail with
  EXT_CONTRIVED retained. Evidence tui-launcher-comms-dark-after-1920.png. Tray destinations have focused fixture
  coverage only; no full tray workflow is claimed.
- Live UI087: the audience choices are inline at1920, stacked at1280 and word-wrapped at390. Right selects Contacts
  then Internal; two Left presses restore Everyone; Tab advances once to frequency. Light/dark changes preserve the
  choices. Save succeeds with reply off; Material confirms Everyone,4days, unchanged09:00 dates, subject, original
  external body and empty internal body. Evidence comms-away-radio-{dark,light}-after-{1920,390}.png plus
  comms-away-radio-dark-after-1280.png. Default dark and1920×1080 restored. No new browser errors; historical405
  from UI093 remains in the browser log. No XML screen changed in this batch; previous screen lint remains applicable.
- Gate19 uses the current debug binary. Before its overlay tests,22 verified stale56-byte segments plus3 active
  segments again occupied most macOS slots. Removed eight stale segments only after rechecking zero attachments
  and dead creator/last-user PIDs; all active segments were retained. A separate test-lifecycle investigation found
  two successful driver fixtures that always SIGKILL their owned Postgres process. A graceful-cleanup proposal is
  queued; exact PID-to-test attribution is inferred from run order, not asserted as logged fact.
- Gate19 finishes with all352 server,10 demo,394 webroot client,48 Dev AIDE client and148 served-client features
  passing. Cargo fails only the same three SIP tests with AddrNotAvailable; the server section fails timing limits
  in Tools and Dev AIDE (including ArtifactDocs23.21s and ArtifactIndex25.18s). The full gate is not exit-zero.
  Serial debug rerun passes44/44 assertions but again exceeds timings. The unchanged Tools/Dev AIDE server code
  passes44/44 with the existing batch9 release binary in39.69s, within both component budgets (Tools1.9/3s,
  Dev AIDE20.3/30s). This isolates debug-build overhead; the changed terminal code remains covered by its current
  debug tests/live server. Logs: /tmp/moqui-ui-framework-gate19.log, /tmp/ui-batch10-server-budget-retest.log and
  /tmp/ui-batch10-release-budget-retest.log. No budget raised and no claim of a fully passing gate.

### Batch 11 — chat and shared input recovery

Batch10 committed locally as `bf10126a`. This batch addresses UI093–101: picker request compatibility, editor names
and metadata updates, keyboard message actions, readable room previews, an explicit drawer focus ring, quick-create
context, visible multiple selections and call-history filter resets. Public routes, selected IDs and service contracts
remain unchanged. Quick chat uses the existing loader context because a QVue screen does not retain the dynamic
container's ID as a DOM wrapper. Multiple dropdowns keep their chips instead of suppressing all selected content.
Empty reset values explicitly remove inherited URL filters; folder changes preserve the user's heard/unheard choice.

Existing regression fixtures are extended without another app sweep: BasicComms/BasicCommsChat, ShellSync,
QvtKeys and WebrootVue.qvt. A source review also caught editor names becoming stale after a room rename; the shared
rich-text watcher changes only editor attributes while preserving content, selection, handlers and focus. Private
cross-reviews found no remaining implementation blocker. Live keyboard/dialog and refused-save outcomes follow below.

The recurring test-environment leak is corrected in the same batch: two successful embedded-Postgres driver tests
retain their original owning handle and stop it normally after proving adoption/refusal. Their behavior assertions
are unchanged; the intentional crash/restart test still uses a real crash. No production DB code, running database,
OS limit or global cleanup mechanism is changed. Compare IPC inventory around the affected driver suite and retain
the normal framework gate. This correction does not claim cleanup after every externally killed test process.

- Focused server specs11/11 pass. Shared WebrootVue.qvt34/34 passes; QvtKeys3/3 passes in12.49s after serializing
  its reactive-array assertion and using Enter on Clear. Screen lint362files/5537sites has zero parse failures,
  with87 pre-existing idiom flags and202 compiled fancy bodies. Driver tests54/54 pass in64.16s; before/after IPC
  inventories contain the same segment IDs, with only normal attachment/time changes on active servers. No IPC
  objects were removed during that verification. Workspace Clippy passes in29.21s.
- All three precompiled-template checks pass. The initial bundle-only regeneration omitted the shared dropdown
  render artifact; correcting it resolves the runtime-only Vue Compose failure. With actual styles loaded, the
  fixture also needs to await answered dialogs leaving the DOM and the requested draft's controls becoming ready.
  Final ShellSync9/9, including UI101, passes in5.80s (4.8/6s component budget). Canvas contrast checks request frequent readback to
  avoid test-generated console warnings. Existing sandboxed-message script refusal remains the expected allowance.
- Live UI094/095: room/thread/edit textboxes have distinct names. Room→Tab enters a labelled message article with
  a visible2px ring; subsequent Tabs reach reply count, reactions, Reply, thread Reply and More. Enter opens Edit;
  Escape retains the text and returns focus to the composer. At390 the thread remains readable in both themes with
  scrollWidth390; Back remains available. UI096's selected drawer row has an inset2px ring in both themes. Evidence
  comms-chat-thread-{light,dark}-after-390.png, comms-chat-thread-light-after-1920.png,
  comms-chat-dark-after-1920.png and shared-drawer-focus-{light,dark}-after-390.png.
- Live UI097's selected room preview is opaque#616161 on#f4f6f8 (5.72:1) in light and#9e9e9e on#121212 (6.99:1) in dark.
- UI101 is corrected at the Chat callers: null cleared selections are guarded; main Start explicitly calls its
  optional-user-list method. The shared selection/serialization contract is unchanged. Quick Create opens the
  picker immediately; no-match, select, clear, Cancel, Close and reopen pass without stale recipients or a crash.
  Start creates DM100053; the main New direct message picker returns to that same DM rather than duplicating it.
- B06 two-user flow: John and the existing limited sec.view.only actor use separate localhost origins. The peer
  sees unread state and receives John's local fixture message, replies, and John receives it without reloading.
  Adding the peer to room100000 exposes its history; Leave→Cancel retains membership and Leave→Confirm removes it
  from the peer list. John's Members returns to one owner. No real mail/provider/media connection is involved.
- Forward finds the local DM and posts the fixture with its optional comment, original sender and time. Room
  rename and restoration update the composer accessible name while preserving an unsent draft; the draft is then
  cleared by keyboard. Evidence comms-direct-local-after-1920.png and comms-forward-local-after-1920.png.
- UI099: Calls shows the chosen recipient chip and hidden ID100000 before submission and after the expected
  unavailable-media refusal. UI100: Voicemail→Trash→Heard→Inbox preserves heard; heard All clears only heard;
  overall All removes voicemail context and returns to the empty all-calls result. No call was established.
- Current-host console inspection found no new errors after the final fixes; earlier picker exceptions and the
  expected unavailable-media500 remain historical evidence. Room/member fixtures are restored; DM100053 with its
  local test conversation/forward remains disposable. Encryption, media success, record-provider search and the
  Chat TUI still need their distinct activities. Actual200% browser zoom remains unverified.
- The release/maximal build completes in3m43s. Gate20 runs the current release binary with
  `env -u MOQUI_RUNTIME -u MOQUI_CONF MOQ_TEST_WORKERS=2 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1 ./moq test-framework --nobuild`.
  Focused logs: /tmp/ui-batch11-{comms-server1,webroot-regression1,qvtkeys3,shellsync5,drivers,clippy,final-precompiled}.log;
  screen lint: /tmp/ui-batch11-screen-lint.txt; full gate: /tmp/moqui-ui-framework-gate20.log.
- Gate20 passes shell342, server352/352 (33.47s, all component budgets), demo10/10, offline webroot394/394,
  offline Dev AIDE48/48 and served148/148. Webroot46.8/34s and served Comms6.8/6s produce budget warnings but remain
  below the existing1.5× failure threshold; no budget is changed. The overall gate exits1 only because the same
  three moqui-sip source-address tests fail with AddrNotAvailable at ua_tests.rs:46. No new Rust/feature failure is
  present; the full gate is not represented as exit-zero. A final independent read-only diff review is clean.

### Batch 12 — alert configuration and state feedback (final batch)

Batch11 committed locally as `aec0202f`; screenshots remain ignored and untracked. This batch integrates the prepared
UI077–083 changes: grouped alert lists and rule/shared-definition editors, query-specific labels, fresh-create context,
accurate evaluation-request acknowledgment, canonical numeric edit values, and notification titles/tones that identify
firing versus recovery. Notification payloads, contact delivery contracts, routes and saved definition IDs remain
unchanged. Read-only/shared and editable states retain their distinct actions. The M10 incident/recovery patches are
queued separately and are not part of this source batch.

Verification and observed outcomes:

- Report crate75/75, host report-alert tests3/3, WebrootReport47/47 and AdminOverview11/11 pass. Host assertions cover
  persisted firing/recovery titles and tones while retaining original contact payloads; mixed/unknown states retain
  conservative notification presentation. Clippy and minimal-feature lint pass. Debug and release/maximal builds
  pass; the latter takes3m47s with four jobs.
- Screen lint checks362files/5557sites with zero parse errors,87 existing idiom reports and202 fancy sites.
  Logs: `/tmp/ui-batch12-{report,alert-host,report-client,admin,clippy,clippy-min,release-build}.log` and
  `/tmp/ui-batch12-screen-lint.txt`. The required full gate is `/tmp/moqui-ui-framework-gate21.log`.
- UI077–079: inspected the grouped overview, rule and contact forms in default light/dark at1920. The rule also
  fits1280; rule/contact forms stack at390 without horizontal overflow. Keyboard collapse/expand works; save and
  return actions remain available. Contact delivery sections stay explicit and preserve inactive fields.
- UI080: all four New links remove inherited definitionId; New alert rule opens a blank ID, while detail links
  retain their selected record. UI081: evaluating the paused8096rule reports no active rule was processed. The
  active8094request explains skipped intervals/claims instead of claiming a fresh result.
- UI083: threshold1000000000 reloads without grouping separators and survives an unrelated save; shared timing
  values0/14400 also survive. Invalid contact matcher JSON retains the entered value and version note, shows the
  parse error, and saves after correction. Only the expected invalid-JSON response occurs in that exercise.
- UI082: a new8094fixture `ui-b12-state-alert` uses value_count(trace_id), threshold-1 and the local in-app contact
  `ui-b12-local-contact` addressed only to EX_JOHN_DOE. Firing produces `Firing — UI review alert state delivery 35`
  with an error icon; threshold1000000000 yields `Normal — ...` with a check icon. Both are visible in notification
  history in light/dark. The list reflects Normal after navigation; this is not a claim of live list refresh.
  The fixture is paused again and the final overview confirms Paused/Normal. No email/webhook/provider is used.
  Measured dark notification contrast against#1d1d1d: title16.86:1, caption11.13:1 and recovery icon9.03:1.
- Final evidence: `dashboard-alerts-empty-light-after-1920.png`, `dashboard-alerts-dark-after-1920.png`,
  `dashboard-alert-rule-{light,dark}-after-1920.png`, `dashboard-alert-rule-dark-after-1280.png`,
  `dashboard-alert-rule-{light,dark}-after-390.png`, `dashboard-alert-contact-{light,dark}-after-1920.png`,
  `dashboard-alert-contact-light-after-390.png`, and `dashboard-alert-notification-states-{light,dark}-after-1920.png`.
  These are inspected local evidence, not committed artifacts or an unattended screenshot suite.
- No acceptance test is added/changed in this final batch. Previously requested named acceptance results remain
  in their activity notes; no addon suite or accept-all was invoked. All remaining work is deferred by user request.
- Independent final source review finds no introduced contract, authorization or data-loss blocker. This review
  complements the recorded tests and live checks; it does not expand the claimed activity coverage.
- Final gate21 runs `env -u MOQUI_RUNTIME -u MOQUI_CONF MOQ_TEST_WORKERS=2 CARGO_BUILD_JOBS=4 RUST_TEST_THREADS=1
  ./moq test-framework --nobuild` and exits1. Its only failing phase is cargo test: the same three moqui-sip
  source-address tests fail with AddrNotAvailable at `crates/moqui-sip/src/ua_tests.rs:46` (57pass/3fail in that
  crate). Host integration passes166 with23 explicitly ignored. No new Rust failure is observed.
  Shell342/342, framework server355/355 (42.33s, all component budgets), demo10/10, offline webroot394/394
  (60.85s), offline Dev AIDE48/48 (4.05s) and served148/148 (47.86s) pass. Webroot46.0/34s and served Comms6.6/6s
  produce timing warnings below the existing1.5x failure threshold; budgets are unchanged. This is not an exit-zero
  full-gate claim. Detailed results: `/tmp/moqui-ui-framework-gate21.log`.
- Final lifecycle: both review servers8094/8096 stop gracefully with exit0; neither HTTP port remains listening.
  Persistent8094data remains in `/tmp/mui-8094-b7-restore`. Before stopping disposable8096, `save-db` preserves both
  databases at `/tmp/mui-8096-final-backup/{moqui,moqui_content}.dump`; PGDMP headers are verified and `load-save`
  into `/tmp/mui-8096-final-runtime` succeeds. Protected runtime keys remain mode0600 and are never printed or
  committed. Backup/restore logs are `/tmp/ui-final-{save-db,load-save}.log`. The restored runtime is stopped.
  Review browser tabs are closed. Purchasing's runtime/database/addons are untouched.

### Batch 13 — post-merge review corrections

The review resumed on Nov 5 after `02d6b325` merged 98 upstream commits (no textual conflicts). A three-part source
review of the local commits (Rust and templates, shared client, application screens and tests) and of the merge found
no P1 defect, authorization weakening or broken contract. Upstream's asynchronous `send#Compose` (ES_READY, "Sending…")
fits the local Compose/Mail changes, `ScreenCsv.js` keeps both the formula guard and the column alignment,
`ModelDetail.xml` keeps `begin-transaction="false"`, and `bundled-screens.js` is unchanged by the merge. This batch
corrects what the review did find:

- UI-023 follow-up: history compared the address bar with `currentLinkUrl`, which the menu data builds (`/qapps/dev`
  shows `/qapps/dev/Home`), so a fragment link under a default subscreen still reloaded the screen. The shell now keeps
  the bar as last pushed or replaced (`historyUrl`, `rememberHistory`) and popstate compares with that. The existing
  start-page history case adds the default-subscreen path.
- ServiceRun (UI-010 follow-up): the kept output is shown once and leaves the session; output that cannot become JSON
  is a warning after the run rather than a failed request. AdminOverview checks the session is empty after the show.
- ArtifactIndex: da924d4b had cut the index bounds to `> 0`. They are back to read/unchanged > 500, artifacts > 10000
  and refs > 5000, which a framework-only runtime (about 900 indexed files) also meets; this checkout's addons give
  read 3232, artifacts 109880, refs 40469.
- BasicComms spec: the two hosted accounts it makes are deleted by their owner (with their local copies), and the
  first feature checks no row is left, so its `mailAccountCount == 0` holds on a later run against the same database.
- UI-082 follow-up: notification titles list states by severity (Error, Firing, No data, Pending, Recovering,
  Normal, then Unknown), mapped through an exhaustive match on `State`, so a new state must be placed to compile.
- UI-051 follow-up: the order-by parser was copied into ScreenHtml.js and ScreenQvt.js. It is ScreenSupport's
  `parseOrder`, given to each runtime as source (as ScreenQvtStatic gives `renderModeHelpers`); the template specs
  that stub ScreenSupport pass the real helper. Both templates record that each sort arrow requests its own direction,
  where the FreeMarker macro reversed the active one.

Verification: AdminOverview 11/11, BasicComms 8/8, ArtifactIndex 6/6, ScreenHtml/ScreenQvt 29/29, DatePeriod 6/6,
FieldTitle 6/6, WebrootVue.qvt client 33/33, moqui-report crate, core screen tests 330, devkit template specs, screen
lint (0 parse failures). The full gate result is under Batch 14, which was gated with this batch.

### Batch 14 — test time, suite placement and macOS loopback sources

- Webroot client regression time was 46.9 s against its 34 s budget. The measured theme and layout checks moved to a
  new offline acceptance suite, `webroot/acceptance/client/ThemeLayout.spec.js` (field feedback and drawer contrast in
  both themes, Mermaid redraw on a color-mode change, box headers at phone width). QvtKeys waits for each outcome
  instead of fixed sleeps, the routed-menu fixture turns off menu animation and fails loudly if the shell markup it
  slices changes. The suite now measures about 40.7 s; with the user's agreement the webroot client budget is 44 s,
  and basic-comms client is 7 s for its measured 6.6 s.
- `./moq accept` ran only acceptance client suites that declare a server; one that declares none (offline specs) was
  never run. It now runs those with `--offline --acceptance` (help and docs/dev/Client-Testing.md updated).
  `./moq accept webroot`: ThemeLayout 3/3.
- macOS routes only 127.0.0.1. The three moqui-sip tests of separate source addresses, upstream's HL-032 cluster-edge
  share test and the HL-032 sub-checks of the SSH integration test say they are skipped there; on Linux a failure to
  bind still fails. The rest of the SSH test runs. This removes the recurring SIP gate failure without weakening an
  assertion where the addresses exist.
- `rtc::a_recorded_call_stores_each_leg_s_track_and_the_mixed_audio_in_segments` failed once under full-gate load: the
  store thread removes the spool after the last segment. The test waits up to 5 s for it.
- Gate (Batches 13 and 14): `env -u MOQUI_RUNTIME -u MOQUI_CONF MOQ_TEST_WORKERS=2 CARGO_BUILD_JOBS=4
  RUST_TEST_THREADS=1 ./moq test-framework` exits 0, the first exit-zero full gate of this review. Host integration
  167 passed with 23 explicitly ignored (655.5 s); moqui-core lib 1505. Feature time: webroot client 40.6/44 s,
  basic-comms client 6.6/7 s, framework server 8.5/12 s, dev-aide 17.2/30 s, tools 1.9/3 s. Clippy on moqui-report,
  moqui-sip and moqui-host with tests is clean. Two earlier runs of this gate found the stubs that lacked `parseOrder`
  (DatePeriod spec, devkit template specs) and the loopback and spool cases above.
- Not changed: ShellSync's one confined `allowProblems()` (the in-page harness takes no pattern) and the wizard
  supersession case, which drives a stand-in root through setUrl.

### Batch 15 — the deferred patches: replay capture and playback, Standard report values, incidents and recovery

The private proposals recorded at wrap-up (formerly under `/tmp`) still applied after the merge. Each was reviewed
again against the merged sources before it was applied; the corrections that review asked for are part of this batch.

- UI-088: replay requests have their own `replay-body-limit` (1 MiB default; an explicit legacy `body-limit` stays the
  cap unless overridden; trace/log admission keeps 256 KiB). The recorder sends complete events in order with one
  request in flight and one bounded pending chunk, and stops with a payload-free diagnostic when an event cannot fit.
  Masking is unchanged. Live: with RUM, replay and replay sample ratio 1 set temporarily in Monitor › Settings, a full
  Gallery load and scroll posted three `/rum/replay` chunks, all 200, and Replays listed the session; the three
  overrides were reverted afterwards (Revert all, in-app confirmation).
- UI-089: the player requests its events from the supplied `/apps` JSON transition rather than the `/qapps` shell
  path. Live: ReplayDetail loads 7.3 s of events, Play advances, and every text node in the replayed page is masked.
- UI-092: the Standard and print report fallbacks format typed time values in the viewer's zone and known number
  units; CSV, XML and text exports keep the original frame values.
- UI-102/103/104/107: Incidents groups its filters and has a real empty state; IncidentDetail shows readable times
  in the user's zone, grouped context/state/timeline/recovery sections, an explicit unavailable state with Back to
  incidents for a missing ID (checked only after the MONITOR_INCIDENT guard), and a waiting Manual action's saved
  instructions as escaped text in a box (the review replaced an unstyled `m-tonal` container). Live: a disposable
  incident shows Onset/Detected as 2026-11-05 02:26:43 in US/Central; `incidentId=ui-review-missing` shows Incident
  unavailable with its link.
- UI-105/106: Recovery rows have Edit, which loads the stored policy or adapter; New refuses an existing ID before
  storage and keeps the typed values. Review corrections: the duplicate check runs only for MONITOR_RECOVER_ADMIN
  holders and reads one row (others get the store service's permission refusal), the form-only mode markers are no
  longer screen parameters, Mode defaults to Advisory without an empty choice the service would refuse, and Newest
  actions shows a readable Created time. Live: DEFAULT's Edit shows order 1000 (no grouping), cooldown 60, daily
  limit 3; a New policy with ID DEFAULT warns "Policy ID already exists" with the dialog and values kept.
- Tests: the incidents feature is re-inserted at the end of AdminOverview (its old context no longer matched) with a
  literal instant; the recovery feature drops XML-text checks and the second duplicate POST, writes its fixture rows
  past entity authorization, and keeps one Recovery HTML render. AdminOverview 13/13 (tools 0.9/3 s), ScreenHtml
  14/14, WebrootRum and WebrootReport 56/56, moqui-report 77, host monitor_rum and monitor_tui, the shared report
  render rule, screen lint (0 parse failures). Requested acceptance: Tools Monitoring 19/19.
- Gate: `./moq test-framework` passed every phase but cargo, where the devkit test that pins each template spec's
  feature count still expected 13 ScreenHtml features; UI-092 adds the fourteenth. With the count updated,
  `cargo test --workspace --no-fail-fast` passes (69 test binaries). Feature time: webroot 40.6/44 s, tools 2.1/3 s,
  dev-aide 24.2/30 s.
- Observed, not changed: the browser pane refuses the push service-worker registration (`/sw.js` serves normally),
  which logs "An unknown error occurred when fetching the script".

### Batch 16 — the remaining open findings

- UI-084: Replays has Find replays and Replays sections in the 1600px workspace, user/visit filters in columns, times
  in the user's zone, phone columns, and distinct empty states: none recorded (with where replay is enabled) and no
  match for the filters. ReplayDetail shows the same readable times. Live: both empty states at 1920.
- UI-085: Release Detail groups Release (revision, released time in the user's zone, previous version, uncertain
  attribution), Regressions against the previous version and Error groups first seen, each with an explanation when
  empty; error rows are labelled by type or message rather than by fingerprint. Live: moqui 0.1.0.
- UI-016: an empty Deployed list says there is no deployment history and that the started components run as usual.
- UI-017: the appearance token inventory (`SURFACE_TOKENS`) lists the semantic text tokens (positive, warning, info,
  secondary, accent) beside negative-text, so the color editor offers them; Qvt-Client.md lists them.
- UI-033: Check a Quota with no user says to enter one; the cost names its currency when the usage rows share one,
  "mixed currencies" when not, and the models' price currency when there are no rows. Live: empty and EX_JOHN_DOE.
- UI-035: the Entity List puts Find entities first and Database maintenance after it, with a note on what changes the
  schema; the edit form is bounded to 960px; Edit's "Back to Find" returns to the last search of that entity (kept in
  the session by Find). Live: Enumeration enumTypeId=DataSourceType → Edit → Back to Find keeps the filter.
- Native confirmations: the list-tree move, the re-login reload, the dashboard panel Remove and declared extension
  actions now use the in-app confirmation (`moqui.confirmAction`); WebrootReport and WebrootExt keep the browser's
  where no qvt shell is loaded. The views regression and the Monitoring acceptance Remove step use the in-app dialog.
- S03 follow-up: a form the user may not submit (Material and Standard) says so above its disabled fields. Live: as
  `sec.view.only`, Create User Account shows the note over 12 disabled fields.
- Logs histogram (batch 2 observation): not reproduced. The trace link's `trace_id:<id>` over 1 h and over its 7-day
  range, and `trace_id:*` with 37 records, each draw a matching Documents over time chart; both queries share the
  dataset, range, query and filters. No change.
- Verification: core screen tests 409, framework template specs 42/42, AdminOverview 13/13, webroot client 396/396
  (40.5/44 s), screen lint (0 parse failures). Changed acceptance: Monitoring 19/19, and the Tools acceptance suite
  (Monitoring is its only spec) 19/19. `./moq test-framework` exits 0 (webroot 40.3/44 s, tools 2.2/3 s).

### Batch 17 — review cleanups

- Flame graphs recolor their labels only when the frames, zoom or theme change (not on every panel update), decide
  each fill/opacity composite once, and label frames of at least 1% of the graph; narrower ones keep their tooltip.
- Playlists: a pause made by a load failure ends when Retry loads a dashboard (a pause by hand stays), and Previous or
  Next restarts the interval so the chosen dashboard gets its full time.
- The Columns dialog restores focus by each button's `data-action`, not its English label (render golden updated).
- ScreenTui writes the current-value option only for drop-downs and radios, with no constant condition or comment in
  the generated code (tui-depends golden updated).
- The TUI go-to test types both `apps/tools` and `qapps/tools`; the report component's tests are `report/tests.rs`
  and `report/gap_tests.rs`. Gallery Editor tolerates a missing `ec.web`; Proof-Tests notes that `GROOVY_SHELL_WEB`
  stays sealed after the screen's removal.
- Checked, not changed: the TUI shell's `terminal_url` in `run_menu` is the one normalization for typed, menu, recent
  and saved-find entries, so it stays; `/termws` serves only the Moqui TUI session, so the terminal's OSC 11 theme
  query always reaches the TUI; OutOfOffice's workspace `<div>` split across two `render-mode` texts is the pattern
  Alerts, Incidents and Replays use.
- Verification: moqui-tui, core screen tests 409, webroot client 396/396 (40.8/44 s), example 32/32, screen lint
  (0 parse failures). `./moq test-framework` exits 0 (webroot 40.6/44 s, tools 2.2/3 s, dev-aide 25.1/30 s).

### Next activity baseline — M10 incident lifecycle

While gate20 runs, John exercises incident100000 on8094 at1920 in default light. New incident Escape/reopen works;
blank Title is locally refused while Minor/service/summary remain. Corrected Title creates exactly one incident,
`UI review manual incident 32`, service `ui-review-m10-32`, severity Minor. Blank Note is refused with an inline error;
the expected service-validation500 is recorded, then a corrected note persists. Open→Mitigating→Monitoring→Resolved
updates persist with their timeline entries. Default Open excludes the resolved record; Resolved + service filters
find it again and the same detail reopens. Unknown incident bookmark shows a raw missing-record message; sidebar
navigation remains available. UI102–104 track the layout, timestamp and missing-record refinements. No recovery
adapter/policy has been created and no infrastructure action executed in this baseline. The resolved fixture remains
for light/dark/responsive retests.

M10-B setup: emergency stop is off (Emergency stop action available), and remains unchanged. Saved Manual adapter
`UI_M10_MANUAL_32`, with no URL/secret and private addresses No; instructions explicitly prohibit infrastructure work.
Saved policy `UI_M10_32`, exact service pattern `^ui-review-m10-32b$`, Minor/Approval, order1, confidence0.99,
cooldown0, max3/day, deadline10min, verify1min. Its only step is restart of fake target `ui-review-only-a` through
that Manual adapter; notices go only to John's local ReportAlert topic. Invalid Steps JSON is refused and every
entered field remains; corrected JSON saves. Existing DEFAULT is untouched. Incident100001, `UI review manual
recovery 32`, starts Minor/Open with that service. Scheduled decision100000 for targetA becomes AwaitingApproval;
Refuse changes it to Refused and adds a timeline entry without an action. Reentering the complete policy with targetB
produces decision100001. Approve→Cancel retains the waiting state and returns keyboard focus to Approve; confirming
then produces action100000 AwaitingOperator and incident Mitigating. No restart occurs. Cancel makes the action
Cancelled and removes active controls; a note records the simulation and the incident is Resolved. Recovery's
Newest actions links to the same incident. Finally the complete policy is saved with Enabled No; the original stop,
DEFAULT policy and other adapters remain unchanged. The explicit manual instructions are missing from the action UI
(UI107). UI105/106 record editing/grouping issues. Both resolved incidents and the inactive fixture configuration
remain for retests. A separate sec.view.only session is refused IncidentDetail before data is shown and returns via
All applications→Comms. Provider actions, quantitative verification, callback/retry/reversal and incident-only actor
controls remain unverified by this local manual walk.

### Next activity baseline — B06 chat

While batch9 gate18 runs, inspected Chat on8094 as John Doe. The empty split workspace offers direct message/room
entry. Created private room100000, UI review room, topic Disposable local UI review conversation, with John only.
Member search failed with405 (UI093); no other participant was added. Posted a local root message, opened its thread
and posted one reply. Main count becomes1reply; phone Thread Back returns to the room. React 👍 adds1 and keyboard
activation removes it. Edit→Escape retains the original; Edit→Save changes it to UI review: local conversation fixture,
revised. and shows(edited). Both editors are unnamed (UI094), and Tab skips the pointer-only action toolbar (UI095).
The first pointer Edit attempt did not open an editor; a later keyboard activation worked, so no separate edit failure
is claimed. At390 the layout settles to one column with visible reply composer and no horizontal page overflow; the
initial resize frame still showed the old desktop layout and is not counted as a defect. Screenshots
comms-chat-empty-dark-before-1920.png, comms-chat-thread-dark-before-1920.png and
comms-chat-thread-dark-before-390.png. Search for fixture returns the revised message; ui-no-match-zz reports No messages match. Uploaded the synthetic
48-byte ui-review-chat.txt through Attach files, sent it without a body, and downloaded it through the posted attachment
button; downloaded bytes equal the fixture. Members shows only John Doe, Active/Owner. Evidence
comms-chat-attachment-dark-before-1920.png. This is one-user partial coverage, not two-user/encrypted success.
An attempted browser zoom shortcut left viewport1920×1080, devicePixelRatio1 and visual scale1 unchanged;
actual200% zoom remains unverified and is not substituted with a narrow viewport.
Further light1920 checks: Pin to the room adds the root to Pinned messages; Unpin removes it, shows Unpinned and the
useful empty instruction. Both events appear in the room history. Threads→Mine lists the existing root/reply with its
last-reply metadata. Files shows the48-byte attachment, author and time; Pictures shows No pictures yet; All restores
the file. The selected room's preview is visibly faint and measures2.32:1 (UI097). No pin remains.
Quick-create Chat message opens the compact room list (UI098); New message opens the picker, whose sec.view.only
search returns the same405 as UI093. Cancel and Close return to the room without creating a conversation.

### Next activity baseline — B08/B09/B10 unavailable calls

John/default light1920 on8094: Calls explains that no media server is running and phone calls are unavailable.
Empty New call reports Choose someone to call; selecting sec.view.only and submitting reports the no-media-server
refusal. Number invalid reports That is not a phone number. No call/history record appears and no PSTN call is made.
The selected recipient is already visually absent before submission despite hidden value100000 (UI099), so this is
not attributed solely to error recovery. Voicemail presents useful empty Inbox and Trash links; All fails to clear
the voicemail filter (UI100). Successful media, guest admission, recorded playback and phone routing remain pending.
Evidence comms-calls-unavailable-light-before-1920.png and shared-multiple-choice-hidden-before-1920.png.

### Verification environment corrections

The first full gate inherited the review runtime and created an ignored Shop template in the real dev-component
parent through a symlink. All 13 files matched the generated screens-only template; the directory was moved intact
to `/tmp/moqui-ui-generated-shop-backup`. It is not a tracked framework application and is excluded from coverage.
The review bootstrap now creates its own base/dev parent directories, links only individually tracked components,
copies mutable configuration and leaves addons empty. The shell self-test clears inherited MOQUI_RUNTIME and
MOQ_TEST_WORKERS only in its child process; explicit override checks remain. Hostile-environment self-test: 342/342,
with the sentinel caller runtime unchanged.

Gate 3 completed with the focused UI suites passing but global failures: one canonical `/private/tmp` versus
lexical `/tmp` assertion, and PostgreSQL startup failures from exhausted macOS shared-memory IDs. The expected path
now uses canonicalization. Five stale 56-byte segments were removed only after checking zero attachments and dead
creator/last-user PIDs; no active process, database file or kernel limit was changed. Gate 4 uses one Rust test
thread and two framework workers to coexist with the Purchasing review. It exposed a separate signal-test readiness
parser bug under serial Rust harness output; the parent missed a ready message prefixed by the harness test name
and never sent TERM. The test now accepts the ready suffix and explicitly exercises serial child output; all four
focused signal tests pass, including TERM and INT. Production signal handling and timeouts are unchanged. Gate 5
reruns the complete required framework check with these corrections. No addon suites or accept-all were requested
or run by this review.

Gate 5 completed: all shell/server/client sections passed. Cargo reached host integration tests with 161 passed,
five failed and 23 ignored; every failure was PostgreSQL initialization (`shmget`, 56-byte header, shared-memory IDs
exhausted), before its test operation. The corrected component-path and signal tests passed. Four additional dead,
detached headers from review runs were reclaimed. The remaining orphan inventory was then inspected read-only
against the installed PostgreSQL header magic and creator PID: 19 confirmed detached headers with dead creators
were reclaimed, while three unrecognized/incomplete headers and all active headers were left alone. No database
files, running processes or kernel limits changed. The exact five failed tests then passed 5/5 in 19.61s.

The changed GroovyShell absence assertion passed in the existing ignored Tools host acceptance journey (1/1,
21.28s). Tools' component acceptance suite also passed 19/19 in 29.49s. The browser's old Material GroovyShell route
now shows Screen not found. This is the requested interface removal, not a retained placeholder.

Gate 5 is recorded as an environmental failure with successful targeted recovery, **not** as an exit-zero full
gate. Cargo stopped at the host integration failure, so the affected TUI crate is checked separately before the
shared batch commit; a fresh complete gate is required for the next implementation batch after the resource cleanup.

## Activity inventory and screen mapping

The inventories below define the review work; they do **not** claim it has all been exercised.
The outcome ledger and running notes above record completed observations. `P` means pending visible activity;
`S/P`, `R/P` and `V/P` distinguish source, focused regression and limited visual evidence with the full activity
still pending. Acceptance outcomes are separate from manual visual coverage. Addon applications are excluded.

### System, Tools and communication activities


Each definition includes a recovery path and local scope. For an individual run, record the role/account, fixture IDs, exact viewport/theme, route, observed result, evidence and cleanup separately. Read-only inventory does not establish permission-limited or error-state coverage.

| ID / activity | Actor and starting state | Objective | Steps | Expected result | Recovery / boundary |
|---|---|---|---|---|---|
| S01 — Instance health and runtime | Administrator; signed in to disposable framework instance | Assess health without mistaking unavailable data for zero | Open System overview; inspect runtime/host/process; follow Cache, Instance and Threads; return by navigation | Readable timestamp, accurate availability, coherent diagnostics and preserved route/back state | No data/platform unsupported; no-op refresh; do not interrupt shared work |
| S02 — Components and configuration | Administrator; framework components loaded | Understand loaded/deployed state and configuration | Overview → Components → loaded/deployed sections and preview; open Conf and inspect a harmless property | Loaded components and deployment history are clearly distinct; source/value of setting clear | Missing component or refused change is explained; deployment/restart requires isolated fixture and remains pending |
| S03 — Users, groups and artifact permissions | Administrator plus disposable limited user; demo user unchanged | Create and manage a disposable identity with appropriate access | Find user → create/edit disposable user → add temporary group → inspect membership/artifact rules → sign in as limited user | Search/edit/group membership reflects saved changes; forbidden operations are denied coherently | Invalid or duplicate username, cancelled edit and refused access; remove only disposable memberships/records |
| S04 — Account verification | Disposable account with local capture/TOTP fixture | Understand verification and recovery | User detail → email/SMS/TOTP verification; try invalid then valid local code; cancel and reopen | Clear channel/state, errors and return path without losing account context | Expired/invalid code and unavailable sender; successful external delivery unverified without fixture |
| S05 — Jobs and printing | Administrator; safe fixture job and virtual/captured printer | Run a safe job and investigate its result | Find job → inspect settings → run named harmless job → Jobs Runs → run detail; inspect printer/job list | Run state/result/error links and timestamps are readable and correlated | Failed fixture run, empty history, unavailable printer; no physical printing |
| S06 — Audit, visits and system messages | Administrator; disposable activity and local failed-message fixture | Investigate an event from list to useful context | Filter AuditLog/Visits → detail; find SystemMessage → message text/type/remote → inspect retry outcome only with local sink | Filters retain context; payload and failure are understandable; related configuration is reachable | Empty match, invalid identifier and refused role; no external retry without local capture |
| S07 — Entity synchronization | Administrator; two disposable local endpoints if available | Inspect synchronization state and history | Find sync → detail/configuration → history → inspect one captured local success/failure | Source/destination, last result and recovery actions are explicit | Missing endpoint/failed run; successful sync pending local peer fixture |
| S08 — Data documents | Administrator; disposable document definition and entity data | Find/edit/export a data document | Find definition → view → edit fields → preview document → export/index local fixture | Fields and returned document align; definition edits and export are visible | Invalid field, missing entity and empty result; cancel edit and return; index only local disposable target |
| S09 — Localization | Administrator; disposable message/field translation | Add and verify a translation | Find message/entity field → add/edit test locale value → reopen/search → remove fixture | Locale, source key and translated value clearly distinguished | Duplicate/invalid locale, empty search and cancelled edit |
| S10 — Resource repositories | Administrator; local disposable repository and local remote | Inspect repository state and basic management | Repository list → detail → inspect ref/storage/status; add local fixture only when prepared | Repository location/state and permitted actions clear | Missing path/ref and denied access; external fetch/push remain unverified |
| S11 — LLM overview and usage | AgentAdmin; local recorded usage fixture | Understand available models and recent activity | LLM dashboard → Usage → Calls → Call detail → Conversations → detail | Usage units, status, timings and correlation clear | No provider/no calls, invalid filters, limited role denied |
| S12 — Models and gateway | AgentAdmin; inert model/endpoint/client fixture | Inspect endpoint configuration and gateway routing | Model list → model/endpoint detail; gateway clients/aliases → detail; inspect test errors with local stub | Provider, endpoint, model, alias and client distinctions clear; secrets not exposed | Missing endpoint, disabled client and unavailable provider; no billable invocation |
| S13 — Tools, policies and skills | AgentAdmin; harmless tool plus disposable skill | Understand what an agent may execute and retrieve | Exposure list/detail → Policies; Skill list/detail → RetrievalTest; inspect permission-limited result | Exposure/policy implications and retrieval inputs/results readable | Invalid parameters, no match and denied tool; no externally effective execution |
| S14 — Agent runs, approvals and triggers | AgentAdmin and limited approver; local inert run fixture | Trace run and safely decide a pending local action | Runs → run detail → approvals/detail → conversations; inspect triggers/detail and inbox/lessons/prompt preview | Run/action/approval status and actor responsibility remain clear | Declined/expired action, empty inbox, disabled trigger; any execution needs harmless local fixture |
| S15 — Remote agents and MCP | AgentAdmin; local stub only | Inspect remote connectivity and permissions | Remote agent/server lists → detail → inspect configuration and local error | Network target, available capability and failure reason clear | Unavailable server/invalid URL/refused permission; real remote execution unverified |
| S16 — Edge pairing and clients | Administrator; separate disposable local edge node if prepared | Understand pairing wizard and connection status | Edge Status → Pair EdgeUrl → Connect → Discover → Apply → Test; inspect EdgeClients | Steps, trust confirmation, applied changes and verification outcomes explicit | Bad URL/code, unavailable peer, back/cancel/reopen; successful pairing unverified until local peer exists |
| T01 — Tools entry and service/REST references | Developer administrator; framework-only instance | Find the right tool and inspect API contract | Tools overview → ServiceReference/ServiceDetail; open entity/REST reference and status-flow diagram | Task grouping, inputs/outputs and route labels accurately reflect available tools | Missing service/entity; obsolete GroovyShell removed by request, replaced by web console and SSH JS executor |
| T02 — Entity find/edit and generated screens | Developer administrator; disposable enumeration/reference row | Create, find, edit and remove a safe entity record | Entity list/detail → find/filter → create/edit; repeat through the Data Browse detail/master relationship path | Field types, keys, relationships, validation and saved result understandable | Duplicate key, invalid type, missing record, cancel and limited role |
| T03 — Data import/export and snapshot | Developer administrator; fixture-only DB and files | Transfer disposable data safely | Export fixture query → inspect artifact → import controlled row → find result; inspect snapshot controls | Scope/format/result/counts clear; downloads useful | Malformed import, unsupported upload and missing file; snapshot restore limited to fresh disposable runtime |
| T04 — Database view authoring | Developer administrator; disposable view | Compose and inspect a local database view | Find view → edit aliases/members → save → view/filter → remove fixture | Source entities, aliases and returned columns clear | Invalid alias/join, empty view and cancelled edit |
| T05 — SQL and script recovery | Developer administrator; disposable DB; read-only examples | Run a safe query and recover from error | SQL Runner SELECT 1 → invalid missing column → SELECT 2; SQL Script Runner controlled local query | Results/columns distinguish from input; error is readable and recovery works | Syntax/missing-column error and denied role; no nonfixture writes |
| T06 — Service invocation | Developer administrator; BasicServices.echo#Data | Inspect and invoke a harmless service | ServiceDetail → Run Service → enter TextIn1 → submit → verify TextOut1; invalid parameter then correct | Service identity, input/output grouping and values remain visible after run | Validation/service error and retry; output must show echoed text; no external services |
| T07 — Statistics and load tools | Developer administrator; isolated run with bounded settings | Inspect diagnostics and understand safe test controls | ArtifactStats, Database Queries and Tables → inspect/filter; Database Speed Test and Load testing (Tools/Load) inspect before running bounded fixture only | Units, metrics availability and test scope clear | Unsupported platform and empty stats; load execution pending coordinator budget |
| B01 — Comms home and navigation | Signed-in local user with no external accounts | Find conversations and next useful action | Open BasicComms/Home → Inbox/Chat/Calls/Settings; inspect empty/new-user states and shortcuts | Counts and titles describe user-visible destinations; no dead primary action | Partial data failure, no account and empty rooms; return/back works |
| B02 — Inbox triage | Local user; hosted account/folder/message fixtures | Read, find and organize local mail | Inbox → search/filter/saved search → thread → MessageDetail → mark/read/flag/archive/move/trash → return | Thread selection, unread state, content/source and feedback correct | No matches, inaccessible thread and unavailable request; retry preserves current view |
| B03 — Compose, draft and attachments | Local user; hosted identity with capture enabled | Write, retain and recover a message without external send | Compose/reply → recipients/subject/body → attach local file → save/close/reopen → discard or captured send/undo | Saved status truthful; intended identity and recipients preserved; files readable | Invalid recipient, no identity, failed save/upload/send; retain text and retry; schedule only captured fixture |
| B04 — Mail account setup and recovery | Local user; hosted disposable account or loopback mail fixture | Configure account and understand sync state | Settings/MailAccounts list → Add Address/Connect/Options/Test/Finish → Account → SignIn/SyncProgress → remove/retained mail | Wizard progress, ownership, sync status and consequences explicit | Invalid address, failed local connection, lost sign-in, cancel/back and retry; external OAuth/IMAP unverified |
| B05 — Mail preferences and reusable text | Local user with hosted identity | Configure signatures, rules, out-of-office and text blocks | Settings index → Identities → Rules → OutOfOffice → TextBlocks; edit fixture and reopen | Scope/account, enabled state, ordering and saved content clear | Invalid rule or empty account list, cancel and limited access; auto replies stay captured |
| B06 — Chat room and thread | Two local users; disposable private room | Create and use a conversation | Chat → New room/direct message → post/reply/react/edit → attach local file → search/member list → Panel chat | Room/thread hierarchy, sender/time, unread and attachment state coherent | Unavailable room, removed member, disconnected bridge, failed upload; retry without duplicates |
| B07 — Encrypted chat and devices | Local users and browser-local crypto devices | Verify protected conversation and recover device state | Encrypted private room → message/image → member leave → devices/verification/key backup → restore local test device | Device trust and history availability understandable; unauthorized member cannot read later messages | Unverified/revoked device, unreadable store, backup key failure; explicit reset/restore only |
| B08 — Browser calls and guest lobby | Two/three local users; fake camera/microphone and loopback RTC | Complete local call and guest admission | Calls/History → New call → lobby → answer/mute/layout/leave; create link → guest name → Ask to join → admit → leave | Ringing, admission, media state and end state clear; history links correct | No media server, denied fake device, missed/declined call and invalid/expired link; retry/leave works |
| B09 — Recordings and voicemail | Local fixture owner and unrelated user; synthetic stored audio | Find and play owned recordings/messages | Calls → Recordings/Voicemail → open/play/download → mark or delete disposable item | Owner/status/date/duration, playback and empty states clear | Missing/failed audio and forbidden owner; synthetic file is not a real media success |
| B10 — Phone calls and routing preferences | Local user; inert caller ID/phone configuration | Understand number eligibility and configure handling | Dialer/Panel Dialer → local number search → Settings/Phone → PhoneDevices → CallHandling → caller lists/rules; Calls/Queues | Unavailable reason, caller ID, rule order and save feedback clear | No caller ID, inactive SIP, invalid number/rule and denied forwarding; no PSTN dial |
| B11 — Chat, browser and app settings | Local user; fake media and disposable app credentials | Tune notification/device behavior | Settings/Chat → Calls device test → ChatApps credential → verify/display/revoke fixture | Browser-only versus account-wide choices clear; secrets shown only at intended moment | Denied fake media, unavailable XMPP and revoke confirmation; external app login unverified |
| B12 — Voice administration | Voice administrator; inert/local SIP fixture | Inspect service health and manage local routing configuration | System/Voice Status → Trunks/Numbers/CallerIds → Fraud/Emergency → Phones/RingGroups/Schedules/IvrMenus/Queues | Capability, assignment, limits and routing relationships explicit | No service/trunk, invalid routing target, missing emergency configuration and denied role; no live dial or emergency call |
| B13 — Bridge administration | Administrator; disconnected local bridge fixture | Diagnose queued delivery without external send | System/ChatBridges → bridge/failed-delivery state → local retry only with stub → inspect migration-copy section | Bridge state and queued-message implications clear; retry outcome visible | Disabled/disconnected adapter and denied role; real-network delivery and destructive old-copy removal unverified |

### System, Tools and communication screen mapping


All XML definitions in the two tools apps and basic-comms are listed, including containers, hidden routes and terminal twins, so no user-facing route is silently omitted. Each qvue is named on its XML row; the named ChatPanel component is associated with Panel/Chat. TUI JSON companions are included for completeness. The route is the logical path under `/qapps/` (or `/apps/` for Standard); use the actual menu URL's case. A container's default child is shown. Hidden/parameterized routes need their parent workflow, not a blind direct visit. Public CallGuest uses `/call/<token>`.

### tools: prefix `runtime/base-component/tools/screen/`

| XML file / logical route | Presentation and companions | Activity | Review status |
|---|---|---|---|
| `System/AuditLog.xml` | XML widgets | S06 | P |
| `System/Cache/CacheElements.xml` | XML widgets | S01 | V/P: native count-only and reported-empty states explained; generic populated entries/clear pending |
| `System/Cache/CacheList.xml` | XML widgets | S01 | V/P: valid/literal-fallback filter and recovery, summary→detail/back; cache clearing pending |
| `System/Cache.xml` | Container → CacheList | S01 | V/P: normal parent navigation |
| `System/Components.xml` | XML widgets | S02 | V/P; parent observation |
| `System/Conf.xml` | XML widgets | S02 | P |
| `System/DataDocument/Edit/DataDocFields/FieldList.xml` | hidden menu | S08 | P |
| `System/DataDocument/Edit/DataDocFields.xml` | XML widgets | S08 | P |
| `System/DataDocument/Edit/EditDataDocument.xml` | XML widgets | S08 | P |
| `System/DataDocument/Edit/FindDataDocument.xml` | hidden menu | S08 | P |
| `System/DataDocument/Edit/ViewDataDocument.xml` | XML widgets | S08 | P |
| `System/DataDocument/Edit.xml` | Container → FindDataDocument | S08 | P |
| `System/DataDocument/Index.xml` | XML widgets | S08 | P |
| `System/DataDocument/ViewDocument.xml` | hidden menu | S08 | P |
| `System/DataDocument.xml` | Container → Search | S08 | P |
| `System/EdgeClients.xml` | XML widgets | S16 | V/P: wrong-role guidance and recovery link inspected; local peer success pending |
| `System/EdgeNode/Pair/Apply.xml` | XML widgets | S16 | P |
| `System/EdgeNode/Pair/Connect.xml` | XML widgets | S16 | P |
| `System/EdgeNode/Pair/Discover.xml` | XML widgets | S16 | P |
| `System/EdgeNode/Pair/EdgeUrl.xml` | XML widgets | S16 | V/P: required/invalid address and return to status; no trust or pairing change |
| `System/EdgeNode/Pair/Test.xml` | XML widgets | S16 | P |
| `System/EdgeNode/Pair.xml` | Container → EdgeUrl; hidden menu | S16 | P |
| `System/EdgeNode/Status.xml` | hidden menu | S16 | V/P: unpaired guidance and pairing navigation/cancel passed |
| `System/EdgeNode.xml` | Container → Status | S16 | P |
| `System/EntitySync/EntitySyncDetail.xml` | XML widgets | S07 | P |
| `System/EntitySync/EntitySyncHistory.xml` | XML widgets | S07 | P |
| `System/EntitySync/EntitySyncList.xml` | hidden menu | S07 | P |
| `System/EntitySync.xml` | Container → EntitySyncList | S07 | P |
| `System/Instance/InstanceDetail.xml` | XML widgets | S01 | P |
| `System/Instance/InstanceList.xml` | hidden menu | S01 | V/P: empty state and creation dialog cancellation; no provisioning |
| `System/Instance.xml` | Container → InstanceList | S01 | P |
| `System/Llm/Approvals/ApprovalDetail.xml` | hidden menu | S14 | P |
| `System/Llm/Approvals/ApprovalList.xml` | XML widgets | S14 | P |
| `System/Llm/Approvals.xml` | Container → ApprovalList | S14 | P |
| `System/Llm/Calls/CallDetail.xml` | hidden menu | S11 | P |
| `System/Llm/Calls/CallList.xml` | XML widgets | S11 | P |
| `System/Llm/Calls.xml` | Container → CallList | S11 | P |
| `System/Llm/Conversations/ConversationDetail.xml` | hidden menu | S11 | P |
| `System/Llm/Conversations/ConversationList.xml` | XML widgets | S11 | P |
| `System/Llm/Conversations.xml` | Container → ConversationList | S11 | P |
| `System/Llm/Dashboard.xml` | XML widgets | S11 | V/P: zero-use default state inspected; recorded-call fixture and limited role pending |
| `System/Llm/Gateway/AliasList.xml` | XML widgets | S12 | P |
| `System/Llm/Gateway/ClientDetail.xml` | hidden menu | S12 | P |
| `System/Llm/Gateway/ClientList.xml` | XML widgets | S12 | P |
| `System/Llm/Gateway.xml` | Container → ClientList | S12 | P |
| `System/Llm/Inbox.xml` | XML widgets | S14 | P |
| `System/Llm/Lessons.xml` | XML widgets | S14 | P |
| `System/Llm/Models/EndpointDetail.xml` | hidden menu | S12 | P |
| `System/Llm/Models/ModelDetail.xml` | hidden menu | S12 | P |
| `System/Llm/Models/ModelList.xml` | XML widgets | S12 | V/P: configured endpoint, unusable default model and bindings inspected; write/stub/limited-role pending |
| `System/Llm/Models.xml` | Container → ModelList | S12 | P |
| `System/Llm/PromptPreview.xml` | XML widgets | S14 | P |
| `System/Llm/Remote/AgentDetail.xml` | hidden menu | S15 | P |
| `System/Llm/Remote/AgentList.xml` | XML widgets | S15 | P |
| `System/Llm/Remote/McpServerDetail.xml` | hidden menu | S15 | P |
| `System/Llm/Remote/McpServerList.xml` | XML widgets | S15 | P |
| `System/Llm/Remote.xml` | Container → AgentList | S15 | P |
| `System/Llm/Runs/RunDetail.xml` | hidden menu | S14 | P |
| `System/Llm/Runs/RunList.xml` | XML widgets | S14 | P |
| `System/Llm/Runs.xml` | Container → RunList | S14 | P |
| `System/Llm/Skills/RetrievalTest.xml` | XML widgets | S13 | P |
| `System/Llm/Skills/SkillDetail.xml` | hidden menu | S13 | P |
| `System/Llm/Skills/SkillList.xml` | XML widgets | S13 | P |
| `System/Llm/Skills.xml` | Container → SkillList | S13 | P |
| `System/Llm/Tools/ExposureDetail.xml` | hidden menu | S13 | P |
| `System/Llm/Tools/ExposureList.xml` | XML widgets | S13 | P |
| `System/Llm/Tools/Policies.xml` | XML widgets | S13 | P |
| `System/Llm/Tools.xml` | Container → ExposureList | S13 | P |
| `System/Llm/Triggers/TriggerDetail.xml` | hidden menu | S14 | P |
| `System/Llm/Triggers/TriggerList.xml` | XML widgets | S14 | P |
| `System/Llm/Triggers.xml` | Container → TriggerList | S14 | P |
| `System/Llm/Usage.xml` | XML widgets | S11 | V/P: empty/valid quota check and no-use state; UI-033 |
| `System/Llm.xml` | Container → Dashboard | S11 | P |
| `System/Localization/EntityFields.xml` | XML widgets | S09 | P |
| `System/Localization/Messages.xml` | XML widgets | S09 | P |
| `System/Localization.xml` | Container → Messages | S09 | P |
| `System/Print/PrintJob/PrintJobDetail.xml` | XML widgets | S05 | P |
| `System/Print/PrintJob/PrintJobList.xml` | hidden menu | S05 | P |
| `System/Print/PrintJob.xml` | Container → PrintJobList | S05 | P |
| `System/Print/Printer/PrinterList.xml` | hidden menu | S05 | P |
| `System/Print/Printer.xml` | Container → PrinterList | S05 | P |
| `System/Print.xml` | Container → Printer | S05 | P |
| `System/Resource/GitRepositoryDetail.xml` | hidden menu | S10 | P |
| `System/Resource/GitRepositoryList.xml` | XML widgets | S10 | P |
| `System/Resource.xml` | Container → GitRepositoryList | S10 | P |
| `System/Security/ActiveUsers.xml` | XML widgets | S03 | P |
| `System/Security/ArtifactGroup/ArtifactGroupDetail.xml` | XML widgets | S03 | S/P |
| `System/Security/ArtifactGroup/ArtifactGroupList.xml` | hidden menu | S03 | P |
| `System/Security/ArtifactGroup.xml` | Container → ArtifactGroupList | S03 | P |
| `System/Security/UserAccount/UserAccountDetail/VerifyEmail.xml` | hidden menu | S04 | P |
| `System/Security/UserAccount/UserAccountDetail/VerifySms.xml` | hidden menu | S04 | P |
| `System/Security/UserAccount/UserAccountDetail/VerifyTotp.xml` | hidden menu | S04 | P |
| `System/Security/UserAccount/UserAccountDetail.xml` | XML widgets | S03 | P |
| `System/Security/UserAccount/UserAccountList.xml` | hidden menu | S03 | P |
| `System/Security/UserAccount.xml` | Container → UserAccountList | S03 | P |
| `System/Security/UserGroup/GroupUsers.xml` | XML widgets | S03 | P |
| `System/Security/UserGroup/UserGroupDetail.xml` | XML widgets | S03 | P |
| `System/Security/UserGroup/UserGroupList.xml` | hidden menu | S03 | P |
| `System/Security/UserGroup.xml` | Container → UserGroupList | S03 | P |
| `System/Security.xml` | Container → UserAccount | S03 | P |
| `System/ServiceJob/JobRuns/JobRunDetail.xml` | XML widgets | S05 | V/P: result and correlated trace inspected for successful run100205 |
| `System/ServiceJob/JobRuns/JobRunList.xml` | hidden menu | S05 | V/P: filtered empty history→clear→all runs→detail |
| `System/ServiceJob/JobRuns.xml` | Container → JobRunList | S05 | P |
| `System/ServiceJob/Jobs/ServiceJobDetail.xml` | XML widgets | S05 | V/P: safe job settings and history inspected; manual-run native confirmation unverified |
| `System/ServiceJob/Jobs/ServiceJobList.xml` | hidden menu | S05 | V/P: filter→detail; scheduler timestamp UI-032 |
| `System/ServiceJob/Jobs.xml` | Container → ServiceJobList | S05 | P |
| `System/ServiceJob.xml` | Container → Jobs | S05 | P |
| `System/SystemMessage/Message/SystemMessageDetail/EditMessageText.xml` | XML widgets | S06 | P |
| `System/SystemMessage/Message/SystemMessageDetail.xml` | XML widgets | S06 | P |
| `System/SystemMessage/Message/SystemMessageList.xml` | hidden menu | S06 | P |
| `System/SystemMessage/Message.xml` | Container → SystemMessageList | S06 | P |
| `System/SystemMessage/Remote/MessageRemoteDetail.xml` | XML widgets | S06 | P |
| `System/SystemMessage/Remote/MessageRemoteList.xml` | hidden menu | S06 | P |
| `System/SystemMessage/Remote.xml` | Container → MessageRemoteList | S06 | P |
| `System/SystemMessage/Type/MessageTypeDetail.xml` | XML widgets | S06 | P |
| `System/SystemMessage/Type/MessageTypeList.xml` | hidden menu | S06 | P |
| `System/SystemMessage/Type.xml` | Container → MessageTypeList | S06 | P |
| `System/SystemMessage.xml` | Container → Message | S06 | P |
| `System/ThreadList.xml` | XML widgets | S01 | V/P: unavailable state/localized scheduler verified at 1920 light/dark and 390; Linux details pending |
| `System/Visit/VisitDetail.xml` | hidden menu | S06 | P |
| `System/Visit/VisitList.xml` | XML widgets | S06 | P |
| `System/Visit.xml` | Container → VisitList | S06 | P |
| `System/dashboard.xml` | XML widgets | S01 | V/P; focused rule passed |
| `System.xml` | Container → dashboard | S01 | P |
| `Tools/ArtifactStats.xml` | XML widgets | T07 | P |
| `Tools/Data/Browse/AutoEdit/AutoEditDetail.xml` | XML widgets | T02 | P |
| `Tools/Data/Browse/AutoEdit/AutoEditMaster.xml` | XML widgets | T02 | P |
| `Tools/Data/Browse/AutoEdit.xml` | XML widgets | T02 | P |
| `Tools/Data/Browse/AutoFind.xml` | XML widgets | T02 | P |
| `Tools/Data/Browse/MainEntityList.xml` | XML widgets | T02 | P |
| `Tools/Data/Browse.xml` | Container → MainEntityList | T02 | P |
| `Tools/Data/Export.xml` | XML widgets | T03 | P |
| `Tools/Data/Grid.xml` | XML widgets | T02 | V/P: create/filter/edit/duplicate correction passed; CSV mismatch found (UI-036), deletion unverified |
| `Tools/Data/Load.xml` | XML widgets | T03 | P |
| `Tools/Data/Record.xml` | XML widgets | T02 | V/P: disposable EnumerationType update persisted; relationship navigation and narrow layout pending |
| `Tools/Data/Snapshots.xml` | XML widgets | T03 | P |
| `Tools/Data/Views/EditDbView.xml` | XML widgets | T04 | P |
| `Tools/Data/Views/FindDbView.xml` | XML widgets | T04 | P |
| `Tools/Data/Views/ViewDbView.xml` | XML widgets | T04 | P |
| `Tools/Data/Views.xml` | Container → FindDbView | T04 | P |
| `Tools/Database/Overview.xml` | XML widgets | T07 | P |
| `Tools/Database/Queries.xml` | XML widgets | T07 | P |
| `Tools/Database/SpeedTest.xml` | XML widgets | T07 | P |
| `Tools/Database/Tables.xml` | XML widgets | T07 | P |
| `Tools/Database.xml` | Container → Overview | T07 | P |
| `Tools/Data.xml` | Container → Grid | T02 | P |
| `Tools/Etl.xml` | hidden menu | T03 | P |
| `Tools/GroovyShell.xml` | removed | T01 | No longer applicable; web console/Terminal UI and SSH JS executor replace it. Old route returns 404. |
| `Tools/Schema/Entities.xml` | XML widgets | T02 | V/P: exact entity filter and Find navigation passed; maintenance controls inspected only |
| `Tools/Schema/Entity.xml` | XML widgets | T02 | P |
| `Tools/Schema.xml` | Container → Entities | T02 | P |
| `Tools/Service/ServiceDetail.xml` | hidden menu | T01 | V/P; focused rule passed |
| `Tools/Load/Run.xml`, `Tools/Load/NewRun.xml` | modes vue,qvue,tui | T07 | P |
| `Tools/Service/ServiceReference.xml` | XML widgets | T01 | P |
| `Tools/Service/ServiceRun.xml` | XML widgets | T06 | V/P; parent observation |
| `Tools/Service.xml` | Container → ServiceReference | T01 | P |
| `Tools/Sql.xml` | XML widgets | T05 | V/P; parent observation |
| `Tools/StatusFlows.xml` | XML widgets | T01 | P |
| `Tools/dashboard.xml` | XML widgets | T01 | S/P |
| `Tools.xml` | Container → dashboard | T01 | P |

### basic-comms: prefix `runtime/base-component/basic-comms/screen/`

| XML file / logical route | Presentation and companions | Activity | Review status |
|---|---|---|---|
| `BasicComms/Calls/Call.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Calls/Call.qvue`; `BasicComms/Calls/Call.tui.json` | B08 | P |
| `BasicComms/Calls/CallTui.xml` | hidden menu; explicit TUI screen | B08 | P |
| `BasicComms/Calls/Dialer.xml` | modes vue,qvue,tui; `BasicComms/Calls/Dialer.qvue`; `BasicComms/Calls/Dialer.tui.json` | B10 | S/P |
| `BasicComms/Calls/DialerTui.xml` | hidden menu; explicit TUI screen | B10 | P |
| `BasicComms/Calls/History.xml` | XML widgets | B08 | S/P |
| `BasicComms/Calls/Queues.xml` | XML widgets | B10 | P |
| `BasicComms/Calls/Recordings.xml` | modes vue,qvue,tui; `BasicComms/Calls/Recordings.qvue`; `BasicComms/Calls/Recordings.tui.json` | B09 | P |
| `BasicComms/Calls/RecordingsTui.xml` | hidden menu; explicit TUI screen | B09 | P |
| `BasicComms/Calls/Voicemail.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Calls/Voicemail.qvue`; `BasicComms/Calls/Voicemail.tui.json` | B09 | P |
| `BasicComms/Calls/VoicemailTui.xml` | hidden menu; explicit TUI screen | B09 | P |
| `BasicComms/Calls.xml` | Container → History | B08 | S/P |
| `BasicComms/Chat.xml` | modes vue,qvue,tui; `BasicComms/Chat.qvue`; `BasicComms/Chat.tui.json` | B06/B07 | V/P: private room/thread/edit/reaction/search/attachment plus two-user DM, forward, membership/leave and picker recovery; UI093–095/101 fixed; encryption/TUI remain |
| `BasicComms/ChatTui.xml` | hidden menu; explicit TUI screen | B06/B07 | P |
| `BasicComms/Home.xml` | XML widgets | B01 | S/P |
| `BasicComms/Inbox/Compose.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Inbox/Compose.qvue`; `BasicComms/Inbox/Compose.tui.json` | B03 | S/P |
| `BasicComms/Inbox/ComposeTui.xml` | hidden menu; explicit TUI screen | B03 | P |
| `BasicComms/Inbox/Mail.xml` | hidden menu; modes vue,qvue,qvt,html,tui; `BasicComms/Inbox/Mail.qvue`; `BasicComms/Inbox/Mail.tui.json` | B02 | S/P |
| `BasicComms/Inbox/MailTui.xml` | hidden menu; explicit TUI screen | B02 | P |
| `BasicComms/Inbox/MessageDetail.xml` | hidden menu | B02 | P |
| `BasicComms/Inbox.xml` | Container → Mail | B02 | S/P |
| `BasicComms/Panel/Chat.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Panel/ChatPanel.qvue`; `BasicComms/Panel/Chat.tui.json` | B06 | V/P: quick-create picker/start/cancel/reopen; tray and TUI remain |
| `BasicComms/Panel/Dialer.xml` | hidden menu; modes vue,qvue,tui; reuses `BasicComms/Calls/Dialer.qvue`; `BasicComms/Panel/Dialer.tui.json` | B10 | P |
| `BasicComms/Panel.xml` | hidden menu | B06 | P |
| `BasicComms/Settings/CallHandling.xml` | modes vue,qvue,tui; `BasicComms/Settings/CallHandling.qvue`; `BasicComms/Settings/CallHandling.tui.json` | B10 | S/P |
| `BasicComms/Settings/CallHandlingTui.xml` | hidden menu; explicit TUI screen | B10 | P |
| `BasicComms/Settings/Calls.xml` | modes vue,qvue,tui; `BasicComms/Settings/Calls.qvue`; `BasicComms/Settings/Calls.tui.json` | B11 | S/P |
| `BasicComms/Settings/Chat.xml` | XML widgets | B11 | P |
| `BasicComms/Settings/ChatApps.xml` | XML widgets | B11 | P |
| `BasicComms/Settings/Devices.xml` | modes vue,qvue,tui; `BasicComms/Settings/Devices.qvue`; `BasicComms/Settings/Devices.tui.json` | B07 | P |
| `BasicComms/Settings/DevicesTui.xml` | hidden menu; explicit TUI screen | B07 | P |
| `BasicComms/Settings/Identities.xml` | XML widgets | B05 | P |
| `BasicComms/Settings/Index.xml` | hidden menu | B01/B05/B11 | S/P |
| `BasicComms/Settings/MailAccounts/Account.xml` | hidden menu | B04 | V/P: hosted status/settings/import inspected; Open Inbox links disabled (UI-062) |
| `BasicComms/Settings/MailAccounts/Add/Address.xml` | XML widgets | B04 | V/P: invalid address and hosted-address recovery passed |
| `BasicComms/Settings/MailAccounts/Add/Connect.xml` | XML widgets | B04 | P: hosted branch bypasses external connection |
| `BasicComms/Settings/MailAccounts/Add/Finish.xml` | XML widgets | B04 | V/P: local activation passed; misleading sync copy UI-061 |
| `BasicComms/Settings/MailAccounts/Add/Options.xml` | XML widgets | B04 | V/P: saved name/policy retained; Back loses draft UI-060 |
| `BasicComms/Settings/MailAccounts/Add/Test.xml` | XML widgets | B04 | V/P: hosted ready state and Back navigation inspected |
| `BasicComms/Settings/MailAccounts/Add.xml` | Container → Address; hidden menu | B04 | P |
| `BasicComms/Settings/MailAccounts/List.xml` | hidden menu | B04 | V/P: create and open hosted account observed |
| `BasicComms/Settings/MailAccounts/Removed.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Settings/MailAccounts/Removed.qvue`; `BasicComms/Settings/MailAccounts/Removed.tui.json` | B04 | P |
| `BasicComms/Settings/MailAccounts/SignIn.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Settings/MailAccounts/SignIn.qvue`; `BasicComms/Settings/MailAccounts/SignIn.tui.json` | B04 | P |
| `BasicComms/Settings/MailAccounts/SyncProgress.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Settings/MailAccounts/SyncProgress.qvue`; `BasicComms/Settings/MailAccounts/SyncProgress.tui.json` | B04 | P |
| `BasicComms/Settings/MailAccounts.xml` | Container → List | B04 | P |
| `BasicComms/Settings/OutOfOffice.xml` | XML widgets | B05 | P |
| `BasicComms/Settings/Phone.xml` | modes vue,qvue,tui; `BasicComms/Settings/Phone.qvue`; `BasicComms/Settings/Phone.tui.json` | B10 | P |
| `BasicComms/Settings/PhoneDevices.xml` | modes vue,qvue,tui; `BasicComms/Settings/PhoneDevices.qvue`; `BasicComms/Settings/PhoneDevices.tui.json` | B10 | P |
| `BasicComms/Settings/PhoneDevicesTui.xml` | hidden menu; explicit TUI screen | B10 | P |
| `BasicComms/Settings/PhoneTui.xml` | hidden menu; explicit TUI screen | B10 | P |
| `BasicComms/Settings/Rules.xml` | modes vue,qvue,tui; `BasicComms/Settings/Rules.qvue`; `BasicComms/Settings/Rules.tui.json` | B05 | P |
| `BasicComms/Settings/RulesTui.xml` | hidden menu; explicit TUI screen | B05 | P |
| `BasicComms/Settings/TextBlocks.xml` | XML widgets | B05 | P |
| `BasicComms/Settings.xml` | Container → Index | B01/B05/B11 | S/P |
| `BasicComms.xml` | Container → Inbox | B01 | S/P |
| `CallGuest.xml` | hidden menu; modes vue,qvue,tui; `CallGuest.qvue`; public `/call/<token>` | B08 | P |
| `System/ChatBridges.xml` | XML widgets | B13 | V/P: empty/disconnected guidance inspected; no adapter configured |
| `System/Voice/CallerIds.xml` | XML widgets | B12 | P |
| `System/Voice/Emergency.xml` | XML widgets | B12 | P |
| `System/Voice/Fraud.xml` | XML widgets | B12 | P |
| `System/Voice/IvrMenus.xml` | XML widgets | B12 | P |
| `System/Voice/Numbers.xml` | XML widgets | B12 | P |
| `System/Voice/Phones.xml` | XML widgets | B12 | P |
| `System/Voice/Queues.xml` | XML widgets | B12 | P |
| `System/Voice/RingGroups.xml` | XML widgets | B12 | P |
| `System/Voice/Schedules.xml` | XML widgets | B12 | P |
| `System/Voice/Status.xml` | XML widgets | B12 | V/P: supported/configured/active and server availability inspected; fake trunk flow pending |
| `System/Voice/Trunks.xml` | XML widgets | B12 | V/P: empty guidance inspected; no provider configured |
| `System/Voice.xml` | Container → Status | B12 | P |

### Optional public-demo override (not enabled merely by loading demo data)

`runtime/dev-component/demo/screen/Disabled.xml` replaces Tools Sql, Data/{Load,Export,Snapshots}, Database/{SpeedTest,Connections} and Service/ServiceRun and Load only when the separate demo lock-down component is enabled. Map this single user-facing unavailable screen to T03/T05/T06/T07 permission-limited inspection; pending. The normal framework review must not confuse demo seed data with this optional lock-down mode.


### Dev AIDE screen mapping and activities

## Scope and review setup

- Primary target: `/qapps/dev`, 1920×1080, default light and dark. Check 1280px, 390px, keyboard navigation and 200% zoom on the workspace, artifact navigation and key dialogs. The existing automated Chromium harness starts at **1280×900**; its pass does not constitute the required 1920×1080 visual review.
- Compatibility: inspect representative XML pages under `/apps/dev` and through the TUI. Workspace, artifact graph, entity editor, service editor and screen designer explicitly declare `render-modes="qvue"`; record their browser requirement rather than treating absent Standard/TUI editors as a regression.
- Use the coordinator's disposable framework runtime, isolated database and separate browser hostname/account. Writing exercises use synthetic `my-site`, `upstream-app` and `plain-app` fixtures inside that disposable runtime. They are fixtures, not the user's addon checkouts. Keep the real repository and `runtime/component/` applications untouched.
- Full developer: disposable admin or `DEV_AIDE_USERS` account with `DEV_AIDE`. Extender: `DEV_AIDE_EXTENDERS` with `DEV_AIDE_EXTEND`. Viewer: disposable account granted `DEV_AIDE_VIEW` only. The services enforce these tiers; the review should also assess how the UI explains unavailable actions.
- For Git activities, use a local bare remote plus a second scratch clone. Network-provider success remains unverified. For Tasks and Assist, use a local REST fixture and the existing fake model fixture, respectively; no real message or remote repository is needed.
- Record every activity's observed route, role, state, theme/viewport, screenshot, console/network issues, result and recovery result. Status legend: **F/P** = specified browser acceptance steps passed, remaining activity steps pending; **R/P** = backend/client regression evidence only, UI walk pending; **V/P** = limited coordinator visual evidence, full activity pending; **P** = no claimed UI execution. F never implies both themes, alternate roles, all recovery cases, keyboard-only, 390px or 200% zoom coverage.

## Complete screen inventory

Paths below are relative to `runtime/dev-component/dev-aide/screen/`. There are **24 UI XML definitions** (app root plus 23 descendants) and **3 QVue templates**, all assigned below. Containers/default routes are listed explicitly; shared QVue templates are implementations of the named routes, not additional navigation entries.

| Definition | Reachable route or host | Activity | Required inspection/status |
|---|---|---|---|
| `DevAide.xml` | `/qapps/dev` → Home | DA-01, DA-16 | V/P: coordinator launcher/Workspace navigation observed; complete menu/breadcrumb/command-palette review pending |
| `DevAide/Home.xml` | `/qapps/dev/Home` | DA-01, DA-02, DA-10 | V/P: coordinator inspected Home hierarchy; creation, publish, reload/restart paths pending |
| `DevAide/Workspace.xml` | `/qapps/dev/Workspace` | DA-04, DA-05, DA-06, DA-07, DA-08, DA-09, DA-10, DA-11, DA-13, DA-16 | F/P: edit/save/reload, upstream read-only message, plain root, changed-on-disk Compare, hunk stage/commit/merge and assisted review passed; remaining editor/Git/data/tasks/recovery steps pending |
| `DevAide/Workspace.qvue` | Template of Workspace | DA-04, DA-10, DA-16 | F/P: loaded throughout Workspace journeys; explicit loading/failure states, accessibility and responsive review pending |
| `DevAide/Repositories.xml` | `/qapps/dev/Repositories` | DA-03, DA-10 | R/P: scratch-repository integration covers clone/fetch/pull/load outcomes; this UI route remains unwalked |
| `DevAide/Artifacts.xml` | `/qapps/dev/Artifacts` → Search | DA-05 | P: container navigation not asserted by the direct graph-route journey |
| `DevAide/Artifacts/Search.xml` | `/qapps/dev/Artifacts/Search` | DA-05 | V/P + R: coordinator initial/no-match inspection and index rules recorded; full Search→Refs→source/docs walk pending |
| `DevAide/Artifacts/Refs.xml` | `/qapps/dev/Artifacts/Refs?artifactId=…` | DA-05 | V/P + R: UserAccount identity/Used by and incoming AgentTypeData navigation observed; missing and unresolved UI states pending |
| `DevAide/Artifacts/Browse.xml` | `/qapps/dev/Artifacts/Browse?artifactId=…` | DA-05 | F/P: direct fixture graph route passed; route/list/source navigation pending |
| `DevAide/Artifacts/Browse.qvue` | Template of artifact Browser | DA-05 | F/P: two-generation expansion, connectors, across/down flip and collapse passed; pan/zoom controls, list fallback, loading/error states pending |
| `DevAide/Artifacts/Customizations.xml` | `/qapps/dev/Artifacts/Customizations` | DA-06, DA-08, DA-10 | R/P: extension index/reload and broken-anchor rules passed; this UI route remains unwalked |
| `DevAide/Artifacts/Doc.xml` | `/qapps/dev/Artifacts/Doc?artifactId=…` or docs path | DA-12 | V/P + R: entity docs, missing data-doc message/recovery, Copy Markdown (36,184 characters), package link and diagram open/close observed; dark link/edge contrast finding under correction |
| `DevAide/Artifacts/DocExport.xml` | `/qapps/dev/Artifacts/DocExport` | DA-12 | R/P: scratch export, no-op repeat, stale-manifest protection and viewer/upstream refusals passed; form execution/feedback inspection pending |
| `DevAide/Entities.xml` | `/qapps/dev/Entities` → List | DA-06 | F/P: entered with Widget filter and default list appeared; return/navigation states pending |
| `DevAide/Entities/List.xml` | `/qapps/dev/Entities/List` | DA-06 | F/P: fixture entity visible via filter parameter; other filters, empty/count-cap states and clicking editor link pending |
| `DevAide/Entities/Edit.xml` | `/qapps/dev/Entities/Edit?artifact=entity:…` | DA-06 | F/P: upstream Date/Picklist site extension, save/relink/schema/seed and resulting dropdown passed as admin; viewer/extender UI and remaining model states pending |
| `DevAide/Services.xml` | `/qapps/dev/Services` → List | DA-07 | P: composer journey navigated directly to Edit; container/list entry not exercised |
| `DevAide/Services/List.xml` | `/qapps/dev/Services/List` | DA-07 | P: filters/count-cap/metadata/empty state/editor link not exercised |
| `DevAide/Services/Edit.xml` | `/qapps/dev/Services/Edit?artifact=service:…` | DA-07 | F/P: compose/chip edit/save/reload, Try in sim, generated spec save/run, SECA creation passed; invalid input/error/Rust implementation/Doc states pending |
| `DevAide/Screens.xml` | `/qapps/dev/Screens` → Tree | DA-08 | P: designer journey navigated directly to Design; container/tree entry not exercised |
| `DevAide/Screens/Tree.xml` | `/qapps/dev/Screens/Tree` | DA-08 | R/P: ordering/move/remove backend rules passed; mounted-tree/archetype/menu UI remains unwalked |
| `DevAide/Screens/Design.xml` | `/qapps/dev/Screens/Design?page=…` or `rootId`/`path` | DA-08 | F/P: unsaved/saved list column, form field layout, named anchor, unsupported filter explanation and draft preview passed; tree creation/move/remove, error recovery and keyboard pending |
| `DevAide/GuidedEdit.qvue` | Entities/Edit, Services/Edit, Screens/Design | DA-06, DA-07, DA-08 | F/P: guided entity/service/screen loading and Save paths passed; missing-artifact/loading failure, Text switch and Open in Workspace controls pending |
| `DevAide/Eca.xml` | `/qapps/dev/Eca` | DA-09 | F/P: filtered active SECA visible after creation and condition simulation; other kinds, replaced rows, empty/error and list→file navigation pending |
| `DevAide/Data.xml` | `/qapps/dev/Data` | DA-11 | R/P: data-file listing service passed; data view/check/edit/load/export UI remains unwalked |
| `DevAide/Tasks.xml` | `/qapps/dev/Tasks` | DA-13 | R/P: local REST and synthetic task backend fixture passed; unavailable-provider, source/current-task/branch UI remains unwalked; real providers unverified |
| `DevAide/Settings.xml` | `/qapps/dev/Settings` | DA-14, DA-15 | R/P: tier/upstream/shared-network restrictions covered in scratch backend tests; settings UI, restore/cancel and alternate roles pending |

`devstatic.xml` is the non-UI asset mount at `/devstatic/`, not an application screen. Its eight JavaScript modules implement the workspace, Git views, artifact browser, blocks, entity/service/screen/QVue editors. Inspect loading failures through the assigned parent activity; do not add a fictitious user activity for each static asset. There is no `Eca/Edit` screen in this inventory: ECA files open in the Workspace's ECA view. Agent review is also a Workspace state, not a separate XML route.

## User activities

### DA-01 — Orient and resume work

- **Actor:** full developer; repeat the overview with extender and viewer.
- **Starting state:** signed into an isolated dev instance; begin once without a site, once with `my-site` and an unpublished edit.
- **Objective:** identify the active site, pending work and next useful action.
- **Steps:** open Dev AIDE from the launcher; read Home; open a root in Workspace; return using breadcrumbs; inspect reload history and any restart/rebuild notice; open Repositories and return.
- **Expected result:** root/branch and site ownership are clear; unpublished edits lead directly to review; creation controls are distinguishable from ongoing work; restart requirements accurately name their impact.
- **Recovery:** missing site provides a creation path; plain directory is explained; failed reload names the file and problem; stale root can be refreshed without losing an editor buffer. Cancel any restart until the coordinator schedules it.

### DA-02 — Create a disposable site or component

- **Actor:** full developer.
- **Starting state:** isolated runtime with no site configured and unused fixture names; no real addon directory is exposed as writable.
- **Objective:** create a site extension target and a small component with a clear next step.
- **Steps:** Home → create site with a unique name and framework fixture dependency; inspect result and root list; separately create a screens-only disposable component; inspect generated files in Workspace; coordinate restart/load when required; revisit Home.
- **Expected result:** valid component name, location, generated purpose and activation requirement are clear; created site is available as the extension target after activation.
- **Recovery:** blank/invalid/duplicate name gives useful validation without partial confusing state; canceled confirmation preserves inputs; creation failure names the failed operation. Cleanup is confined to the disposable fixture.

### DA-03 — Inspect and add repositories

- **Actor:** full developer; viewer inspects existing roots only.
- **Starting state:** `my-site` Git root, synthetic upstream Git root, plain directory, local bare remote and an unused clone destination.
- **Objective:** understand root status and bring in a local fixture component.
- **Steps:** inspect Repositories branch/ahead/behind/change/path columns; open each root; clone the local fixture remote; inspect Load-now versus restart feedback; perform Fetch and a fast-forward Pull on the fixture root. Inspect Get a component without fetching excluded addons.
- **Expected result:** status belongs to the correct root; plain roots clearly lack Git operations; load/restart outcomes are actionable; independent root failures do not conceal successful results.
- **Recovery:** invalid local URL, existing destination, absent remote and unavailable Git have distinct messages. Cancel Pull. Do not exercise Fetch all/Pull all unless every exposed repository is a disposable fixture.

### DA-04 — Find, edit and save a file

- **Actor:** full developer, then extender/viewer for read-only behavior.
- **Starting state:** `my-site/service/mysite/Things.xml` with a known `get#Thing` result; an upstream file and a plain directory also exist.
- **Objective:** locate a file, make a small change and understand live reload.
- **Steps:** Workspace → select root → tree and Go to file → open service; find text, inspect breadcrumbs and line information; edit a string; switch tabs and return; Save; inspect problems/reload status; reopen the file; invoke its safe result through the service UI. Inspect an upstream file and plain root.
- **Expected result:** unsaved status and active tab are clear; the saved file reloads immediately and returns the new value; read-only reasons are visible; keyboard focus and tab-close controls are understandable.
- **Recovery:** no search match, invalid regex, malformed XML, oversized/binary file, missing file and changed-on-disk conflict are meaningful states. Cancel close with unsaved work; use Compare and Take disk/keep edits intentionally on scratch files.

### DA-05 — Follow an artifact and its references

- **Actor:** viewer or full developer.
- **Starting state:** indexed framework artifacts and a synthetic service with at least one incoming and one outgoing reference.
- **Objective:** find an artifact, understand its dependencies and reach its definition/docs.
- **Steps:** Artifacts → Search; search `UserAccount` or fixture service; narrow kind/component; open References; follow an incoming/outgoing item and return; open Browser, expand a group, pan/zoom and flip direction; use its list view; open the source in Workspace and the generated doc.
- **Expected result:** identity and direction remain clear across views; long names and unresolved references are readable; browser graph has a usable list alternative; back navigation retains context.
- **Recovery:** initial blank query, zero results, unknown artifact, no references, unresolved reference and index/request failure each explain the state and offer a useful next step. Cancel and reopen Go to artifact; test keyboard navigation and focus return.

### DA-06 — Extend an entity

- **Actor:** extender; full developer for table inspection; viewer for unavailable edit controls.
- **Starting state:** scratch `upapp.Widget(widgetId, widgetName)` and writable `my-site`, with upstream editing off.
- **Objective:** add a field to the site's extension while understanding provenance and schema effects.
- **Steps:** Entities → filter Widget → guided editor; inspect fields/origins, relationships/incoming relations and table comparison; add a text field then a Picklist using disposable IDs; inspect Text and Doc; Save; confirm extension file and model/table column; inspect a view entity's members, aliases and generated SQL; open Customizations.
- **Expected result:** extension destination and upstream source are distinct; preview/unsaved changes agree with Text; save/relink result is clear; picklist relationship/seed records are understandable.
- **Recovery:** no site target, read-only role, duplicate/invalid field, missing relation target and failed save must retain useful input. Inspect destructive-DDL preview and typed confirmation, then cancel; destructive execution is outside this first UI batch.

### DA-07 — Compose, simulate and specify a service

- **Actor:** full developer or extender writing a site service.
- **Starting state:** `mysite.Things.make#Thing` and fixture entity `mysite.Thing`, using the existing test's site files.
- **Objective:** change logic, try it without persisting data and turn the result into a spec.
- **Steps:** Services → filter/open service; inspect parameters; add/edit a simple action block; switch generated JavaScript and Text; Save; Try in sim with valid values; inspect result/messages/log/rows; Save as spec in site test directory; Run spec; inspect Doc and linked SECA rules.
- **Expected result:** input requirements and action order are clear; simulated rows are distinguishable from committed data; the generated spec records the useful result and passes; saved logic is reloaded.
- **Recovery:** missing required input, invalid expression, service error, empty output and failed spec clearly identify the problem. Try does not retain rows. An unsupported Rust-backed service should explain which parts are editable and offer source/docs.

### DA-08 — Design and preview a screen

- **Actor:** extender for upstream extension; full developer for scratch tree operations.
- **Starting state:** fixture `/qapps/upapp` has a list, a single form with field layout, a named Notes anchor and an unnamed container; writable site exists.
- **Objective:** add useful UI content through the site extension and verify the result before saving.
- **Steps:** Screens → Tree → inspect mounted source; open the fixture designer through the visible action; select a list cell and add a column; add a form field after a selected field; inspect Outline, Properties, extension points, Text and Doc; preview unsaved result; Save and reopen the actual fixture page. Separately inspect/create a disposable archetype child and design one scratch QVue file.
- **Expected result:** selected widget/source/extension target are clear; canvas matches unsaved text and saved runtime; inserted fields appear in the intended layout; tree distinguishes file, mount and menu configuration.
- **Recovery:** missing artifact/site, invalid property, unnamed anchor, preview error and changed-on-disk conflict retain a route back to Text. Cancel moving/removing a scratch screen once, then complete only against fixture files. Check keyboard focus does not disappear into preview.

### DA-09 — Inspect and test an event rule

- **Actor:** full developer or extender with a writable site rule.
- **Starting state:** active fixture SECA plus a replaced rule and empty results for another rule kind.
- **Objective:** understand when a rule executes and test its condition safely.
- **Steps:** ECA Rules → filter kind/target/when/component; inspect active versus replaced rows; open file in Workspace ECA view; inspect condition/actions; evaluate condition against fixture context; edit/save a harmless site rule and inspect reload.
- **Expected result:** rule identity/order/source and replacement are explicit; evaluation result is distinguishable from running effects; save updates the active rule.
- **Recovery:** no matches, invalid condition/context and inaccessible file have useful messages and a path back to list/Text. Do not trigger external mail or Git side effects.

### DA-10 — Review changes, publish locally and recover a conflict

- **Actor:** full developer; extender for allowed site Publish; viewer inspects disabled actions.
- **Starting state:** scratch site repository, local bare origin, second scratch clone, two file hunks and one deliberate conflicting remote change.
- **Objective:** review exactly what will be saved/published and resolve a recoverable conflict.
- **Steps:** Workspace Changes → inspect file/hunk diff → stage one hunk and unstage it → stage intended change → commit with message/task trailer → inspect Commits and file history; Publish to the local origin; make the fixture divergence and Pull; inspect non-fast-forward response, merge conflict, side labels and resolution; mark resolved and commit; inspect Home unpublished count.
- **Expected result:** staged/unstaged scope and publication target are obvious; only intended files/hunks enter commits; a conflict names affected files and offers a recoverable path; clean state agrees across Home and Workspace.
- **Recovery:** cancel discard/publish, empty commit message, no remote, rejected push, stale branch and interrupted operation. All Git writes target scratch repositories. Never push the actual framework branch. Optional advanced branches/tags/stashes/settings are explicit inspections here before deeper exercise.

### DA-11 — Inspect, check and load data files

- **Actor:** full developer or extender writing site data.
- **Starting state:** disposable site data file with a handful of fixture rows and a known database comparison.
- **Objective:** understand records and changes before loading them.
- **Steps:** Data → narrow type/component/path → open file in Workspace Data view → inspect records/table → Check against database → edit one harmless fixture value → Load changed records → inspect result → Export fixture records to site file and review diff.
- **Expected result:** file identity/type, row values and load scope are clear; comparisons distinguish absent/changed/unchanged rows; export location is understandable.
- **Recovery:** no files, malformed XML, unknown entity/field, rejected record and failed load identify the problem. Independent failures are reported with successful records where supported. Recovery uses the fixture's known original data.

### DA-12 — Read and export documentation

- **Actor:** viewer for reading; extender/full developer for export.
- **Starting state:** known entity/service/screen artifacts and writable site `autodoc/` target.
- **Objective:** understand artifacts and generate a reviewable documentation tree.
- **Steps:** Docs → artifact → follow related/package/root links → inspect Mermaid zoom and Copy; toggle service source option; Export docs → choose site, artifact kind/component and directory → export → inspect counts and files in Workspace → repeat unchanged export.
- **Expected result:** docs navigation and identity remain clear; diagrams/readable text work in both themes; export summary reports written/unchanged/removed/failures; output stays in chosen site target.
- **Recovery:** unknown doc, absent writable roots, invalid relative path, refused upstream destination and partial export failures have actionable feedback. Review stale-removal scope; only remove generated scratch documentation.

### DA-13 — Connect work to tasks

- **Actor:** developer with local task fixture; repeat without any provider in the framework-only runtime.
- **Starting state:** empty/unavailable local provider first; then a local REST fixture returning open tasks, with a scratch Git root.
- **Objective:** find work, identify a current task and connect a branch/commit to it.
- **Steps:** Tasks → inspect source/app → select local fixture source → inspect tasks → open task/project fixture link → set current → create branch in scratch root → inspect Workspace task rail and commit trailer; inspect Add source and remove the disposable source.
- **Expected result:** unavailable provider is distinguished from zero open tasks; selected root/source/app are clear; task action scope is explicit; current-task feedback agrees in list and Workspace.
- **Recovery:** failed local endpoint, bad fixture token, no Git root and missing target app give usable errors. Real remote provider success remains unverified. Mantle quick-create is not exercised by loading excluded addons.

### DA-14 — Configure workspace roots and site targets

- **Actor:** full developer/admin; extender and viewer inspect limited states.
- **Starting state:** known scratch roots and original settings recorded.
- **Objective:** understand what can be edited and where generated changes go.
- **Steps:** Settings → inspect site order/default target → save scratch site choice → edit scratch root label/read-only flag/publish branch → add plain scratch directory → initialize its Git repository → inspect resulting Workspace status; inspect shared-server/network/token sections only when a dedicated fixture exposes them.
- **Expected result:** settings explain their scope, editable roots and default extension target; save feedback names what changed; plain directory becomes a Git root coherently.
- **Recovery:** unknown site, duplicate/nonexistent directory, invalid branch and refused setting preserve input and show the reason. Restore original fixture settings. Enabling upstream edits or network access is not needed for the first review; inspect the control and leave protected defaults intact.

### DA-15 — Check access and unavailable states

- **Actor:** full developer, extender and viewer in separate fixture sessions.
- **Starting state:** same scratch data, upstream editing off, a writable site and one read-only root.
- **Objective:** ensure users can understand their available actions and recover from denied operations.
- **Steps:** open Home/Settings; view upstream/site file; inspect Save/Publish, entity extension and destructive-table actions; attempt one representative disallowed upstream save via visible UI if enabled; open a bookmarked guided route without required artifact; inspect missing devtools/loading failure with a controlled fixture.
- **Expected result:** permitted viewing works; restricted actions are hidden/disabled or explain their requirement; denial does not lose work; browser-only editors explain unsupported rendering modes.
- **Recovery:** return to allowed site, cancel edit, go back to list and reopen valid artifact. Record server refusal separately from misleading UI affordance; do not broaden grants to make a failing activity pass.

### DA-16 — Review an assisted change and navigate the workspace

- **Actor:** full developer/extender with fake Assist provider fixture.
- **Starting state:** existing test's upstream page and site service; scripted fake `propose_edit` response, no external model.
- **Objective:** preview a suggested change, selectively accept it and retain orientation.
- **Steps:** Customize this screen → inspect artifact summary → Preview unsaved proposed screen → Review in Workspace → inspect hunks → reject one fixture service hunk → save accepted changes → inspect approval outcome; exercise command palette, Go to artifact/file, tab closing and return navigation by keyboard.
- **Expected result:** proposed versus saved content is clear; rejected hunks remain unchanged; accepted scope matches outcome; dialogs have readable titles, cancellation and focus return.
- **Recovery:** cancel preview/review, fake provider unavailable, rejected/empty change set and conflicting file preserve a clear route back. Real model/provider completion remains unverified.

## Existing acceptance journeys and exact commands

These eight tests are **opt-in Rust acceptance tests**, not individually `#[ignore]` tests. `crates/moqui-host/Cargo.toml` declares `gate3_browser` with `test = false`, so default `cargo test` omits it. Do **not** add `--ignored`: that would select no tests here. `./moq accept dev-aide --nobuild` searches component `acceptance/` files and does not dispatch these Rust tests; `accept-all` would be much broader than this review.

| Exact test name in `crates/moqui-host/tests/gate3_browser.rs` | Activity/fixture |
|---|---|
| `the_workspace_edits_and_saves_a_site_file_in_chromium` | DA-04: site service save/reload, upstream lock, plain directory, changed-on-disk compare |
| `a_pull_conflict_is_resolved_in_chromium` | DA-10: two hunks, local bare remote and second clone, pull/merge conflict resolution |
| `the_artifact_browser_expands_and_flips_in_chromium` | DA-05: synthetic artifact references, graph expand/flip/collapse; source/list navigation remains untested |
| `a_qvue_screen_is_designed_in_chromium` | DA-08: scratch QVue screen and buffer/canvas editing |
| `the_entity_modeler_extends_an_upstream_entity_in_chromium` | DA-06: Widget upstream entity, site extension, picklist seed data, form result |
| `the_screen_builder_extends_an_upstream_list_in_chromium` | DA-08: list/form/anchor fixture, unsaved preview and saved site extension |
| `the_service_writer_composes_tries_and_saves_a_spec_in_chromium` | DA-07/09: Thing entity/service, sim rows, generated spec, rule view |
| `a_dev_agent_change_set_is_previewed_reviewed_and_saved_in_chromium` | DA-16: fake model, proposal preview, selective hunks and approval outcome |

Run from `/Users/jonesde/work/moquiwork/mr-moqui-base`. The target has now been built and the exact executable recorded in `/tmp/ui-devaide-acceptance/binary.txt`. Reuse it for a specifically justified rerun while source/build inputs remain current. The release server executable is not a Rust test harness. If a build is needed, coordinate it with the owner and compile only this target; avoid `--release`, a new target directory, feature-set changes, clean or rebuilding the entire workspace.

```sh
cargo test -p moqui-host --test gate3_browser -j 4 --no-run --message-format=json > /tmp/ui-devaide-acceptance-build.jsonl
```

Then run the resulting executable directly, selecting only the eight named journeys. This avoids invoking Cargo again per journey, limits test processes to one beside the Purchasing work, preserves separate logs, and continues to report independent failures:

```sh
python3 - <<'PY'
import json
import os
import pathlib
import subprocess

artifacts = []
for line in pathlib.Path('/tmp/ui-devaide-acceptance-build.jsonl').read_text().splitlines():
    try:
        row = json.loads(line)
    except json.JSONDecodeError:
        continue
    if row.get('reason') == 'compiler-artifact' and row.get('target', {}).get('name') == 'gate3_browser' and row.get('executable'):
        artifacts.append(row['executable'])
if not artifacts:
    raise SystemExit('No gate3_browser executable: inspect the preceding build result.')
binary = artifacts[-1]
journeys = [
    'the_workspace_edits_and_saves_a_site_file_in_chromium',
    'a_pull_conflict_is_resolved_in_chromium',
    'the_artifact_browser_expands_and_flips_in_chromium',
    'a_qvue_screen_is_designed_in_chromium',
    'the_entity_modeler_extends_an_upstream_entity_in_chromium',
    'the_screen_builder_extends_an_upstream_list_in_chromium',
    'the_service_writer_composes_tries_and_saves_a_spec_in_chromium',
    'a_dev_agent_change_set_is_previewed_reviewed_and_saved_in_chromium',
]
env = dict(os.environ, MOQUI_TEST_PROCESSES='1')
# Never inherit another review server's runtime/configuration into fixture processes.
for key in ('MOQUI_RUNTIME', 'moqui_runtime', 'MOQUI_CONF', 'moqui_conf'):
    env.pop(key, None)
env.pop('MOQUI_TEST_OWN_PROCESS', None)
env.pop('MOQUI_DEV_SHOTS', None)
env.pop('MOQUI_SHELL_SHOTS', None)
listed = subprocess.run([binary, '--list'], env=env, text=True, capture_output=True, check=True).stdout
missing = [name for name in journeys if name + ': test' not in listed]
if missing:
    raise SystemExit('Expected acceptance tests absent from this executable: ' + ', '.join(missing))
out = pathlib.Path('/tmp/ui-devaide-acceptance')
out.mkdir(exist_ok=True)
failed = []
for name in journeys:
    with (out / (name + '.log')).open('w') as log:
        result = subprocess.run([binary, name, '--exact', '--nocapture', '--test-threads=1'],
                                env=env, stdout=log, stderr=subprocess.STDOUT)
    print(('PASS ' if result.returncode == 0 else 'FAIL ') + name, flush=True)
    if result.returncode:
        failed.append(name)
raise SystemExit(1 if failed else 0)
PY
```

For one journey only, the equivalent Cargo command is:

```sh
MOQUI_TEST_PROCESSES=1 cargo test -p moqui-host --test gate3_browser -j 4 the_workspace_edits_and_saves_a_site_file_in_chromium -- --exact --nocapture --test-threads=1
```

The package's existing defaults already include `maximal` and `devtools`. Each test calls `common::own_process`, which launches that one test in its own process; do not set `MOQUI_TEST_OWN_PROCESS` manually. `start_with_files_and_links` creates temporary runtime and database directories, links framework-owned base components plus Dev AIDE, writes its synthetic site/upstream fixtures, loads the required security records, and binds HTTP and cluster listeners to free ephemeral ports. These journeys do not use or stop the coordinator's 8090/8091 server. Git-writing fixtures initialize their own repositories, identities and local remotes. The harness discovers Chromium using `moqui_chromium_path`, `runtime/chromium` or an installed supported browser and creates a temporary profile. If sandbox restrictions prevent Chromium/Postgres launch, rerun the same bounded command under the coordinator's approved execution context; do not report a skipped/zero-test result as success.

Keep `MOQUI_DEV_SHOTS` unset. The existing optional screenshot branches are not the reviewed 1920×1080 evidence; capture and inspect that separately in the coordinator's built-in browser.

## Findings reconciliation and remaining risks

1. **Home hierarchy:** implemented by the coordinator's Dev AIDE batch (UI-014): ongoing work precedes creation and creation moved into compact dialogs. Limited visual evidence exists; creation/reload/restart flows still need completion. Do not describe the original layout as still present.
2. **Search/References recovery:** source now distinguishes initial query, no match, missing artifact, and empty incoming/outgoing lists. Index rules passed and the coordinator recorded initial/no-match inspection. Full References rendered states and cross-page navigation remain pending.
3. **Go to file/artifact dialogs:** source now has labelled titles, Cancel, loading/error/empty/retry and stale-result guards; focused client checks passed. The coordinator recorded Escape and initial/no-match inspection. Keyboard selection, focus return, failure/retry and reopened-state checks still need explicit UI evidence.
4. **Accessible names:** new file/folder and Go to file/artifact now have names in source. Do not infer that every icon-only control is repaired: tab-close, task refresh/more and file menu still require actual accessible-name inspection.
5. **XML headers (P3 source defect, still present):** 14 UI definitions contain a nested `<!--` before the license comment closes. Standard ElementTree parsing still fails at line 14 in Repositories, Tasks, Data, Eca, Entities, Services, Screens, Artifacts/Browse, Screens/Tree, Screens/Design, Entities/List, Entities/Edit, Services/List and Services/Edit. The framework parser/linter and several affected browser routes succeed, so this is interoperability/tooling debt, not an observed runtime outage. No source edit was made during this audit.
6. **Resolved acceptance failures:** merge automation clicked disabled Commit before status refresh (readiness condition fixed); simulation fixture exceeded macOS socket limit (short cluster basename fixed); preview frame replacement reported canceled old-frame shell requests (teardown marker fixed). All affected existing journeys passed afterward; no broad console filtering or weakened functional assertion was used.
7. **Component creation evidence:** `new_components_come_from_the_component_template` passed in framework gate 5 after canonicalizing the expected temporary path. The Home creation UI remains unverified; a backend pass does not close its visible activity.

## Executed acceptance journeys — 2026-11-04

The eight framework-owned Dev AIDE browser acceptance journeys ran sequentially using the opt-in
`moqui-host --test gate3_browser` target, with `MOQUI_TEST_PROCESSES=1`, the default 1280×900 harness
viewport, isolated framework runtimes and synthetic site/upstream repositories. These are functional
acceptance results, not the primary 1920×1080 visual review. No `MOQUI_DEV_SHOTS` screenshots were
requested/generated. Build: `cargo test -p moqui-host --test gate3_browser -j 4 --no-run
--message-format=json`, then the emitted test executable with each exact name, `--nocapture
--test-threads=1`. All commands were escalated as coordinated with the parent. No real addons or
external remotes were used.

| Journey | Initial result | Follow-up |
| --- | --- | --- |
| `the_workspace_edits_and_saves_a_site_file_in_chromium` | PASS, 17.39 s | No rerun needed |
| `a_pull_conflict_is_resolved_in_chromium` | FAIL, 41.86 s | PASS, 9.42 s in final source with explicit Commit readiness condition; diagnostic proved the previous click happened while disabled and status still showed the resolved file under Conflicts |
| `the_artifact_browser_expands_and_flips_in_chromium` | PASS, 14.63 s | No rerun needed |
| `a_qvue_screen_is_designed_in_chromium` | FAIL, 8.02 s | Functional and persisted-file assertions passed; final console check found one status-0 warning. Separate investigation confirmed a pending old-preview shellData request canceled by frame replacement; admin_refinement reports PASS, 8.53 s after the teardown marker fix |
| `the_entity_modeler_extends_an_upstream_entity_in_chromium` | PASS, 13.28 s | No rerun needed |
| `the_screen_builder_extends_an_upstream_list_in_chromium` | FAIL, 15.59 s | Functional and persisted-extension assertions passed; final console check found six status-0 warnings. Admin_refinement reports PASS, 13.46 s after the preview teardown marker fix |
| `the_service_writer_composes_tries_and_saves_a_spec_in_chromium` | FAIL, 42.56 s | PASS, 16.76 s after shorter fixture cluster basename; macOS rejected the original simulation socket path above its 103-byte limit |
| `a_dev_agent_change_set_is_previewed_reviewed_and_saved_in_chromium` | PASS, 7.11 s | No rerun needed |

Initial logs and structured outcomes: `/tmp/ui-devaide-acceptance/`.
Focused logs: `/tmp/ui-devaide-focused/`; merge readiness proof is `merge-readiness.log`, and final source passed in `merge-final.log`.
Source changes in this acceptance follow-up: `crates/moqui-host/tests/common/mod.rs` uses a short
`moqui-g3-pg` cluster basename while retaining the descriptive runtime name; the existing merge
journey in `crates/moqui-host/tests/gate3_browser.rs` waits for the newly staged entry to enable
Commit. No application git logic changed. Final target rebuild passed in 2.63 s and diff checks
passed. A temporary diagnostic selector failure was corrected during investigation; it is not
counted as an application failure and no diagnostic remains in source.

Passing preview retest logs: `/tmp/ui-preview-qvue-fixed.log` and
`/tmp/ui-preview-screen-fixed.log` (both inspected, 1/1 passed). Latest successful outcomes with
initial-failure references: `/tmp/ui-devaide-acceptance/latest-results.json`. Preview fix details
and client regression evidence are in `/tmp/ui-preview-investigation.md` (admin_refinement).

## Activity-level closure ledger

No complete DA activity is closed: the original definitions include alternate roles, recovery states and visual/accessibility work beyond the eight automated journeys. The following records the useful successful slices precisely.

| Activity | State / established evidence | Still to execute or explicitly defer |
|---|---|---|
| DA-01 Orient | V/P: coordinator Home hierarchy and Workspace entry | Breadcrumb/return/Repositories; no-site and unpublished fixtures; reload/restart feedback; alternate roles |
| DA-02 Create | P: template service test has a recorded older failure, no UI activity | Valid site/component creation, invalid/duplicate/cancel, activation/load feedback on disposable fixtures |
| DA-03 Repositories | R/P: scratch clone, load/restart reason, fetch/pull integration checks | Visible Repositories controls, plain/no-remote/error/cancel states; local fixture only |
| DA-04 Edit/save | F/P: save/reload returns changed service value; upstream read-only; plain directory; changed-on-disk Compare opens | Tree/Go to file/file search, tab handling, binary/large/missing/invalid file; completing Compare recovery; role/keyboard/responsive |
| DA-05 Artifact/reference | F/P + V/R: graph expansion/flip/collapse; coordinator Go to artifact→UserAccount→read-only source; index refs rules | Missing/unresolved/zero refs; graph pan/zoom/list fallback; accessibility. Search→UserAccount refs→incoming AgentTypeData and entity docs navigation now observed |
| DA-06 Entity | F/P: Date and Picklist site extension, saved model/column/seed, resulting form options | Relationships/incoming/table/view SQL/Doc/Customizations; duplicate/invalid/no-site; extender/viewer UI; cancel DDL |
| DA-07 Service/spec | F/P: compose, edit chip, save, simulate rows, save and run generated spec | Parameter validation, service/spec failure, empty output, Doc/JS/Text UI and Rust-backed read-only explanation |
| DA-08 Screen | F/P: XML site extension and QVue buffer design/save; list/form placement; anchor explanation; draft/unsupported-filter states | Tree/archetype/move/menu/remove; props/Doc/Text navigation; failed preview/save recovery; keyboard through frame |
| DA-09 ECA | F/P: create/save SECA; condition Try against service context; filtered active rule list | Replaced/other kinds/no-match/invalid context; list→file link; meaningful error/cancel |
| DA-10 Git/publish | F/P: stage intended hunk, commit, Pull divergence, merge, take theirs, mark resolved, two-parent commit; backend local Publish/advanced Git checks | Visible Publish/push target/rejected push/cancel; task trailer/Commits/history; Home count; viewer/extender UI; remaining advanced controls |
| DA-11 Data | R/P: data-file listing service only | Visible Data→table/check/edit/load/export and partial failure/recovery; isolate all rows/files |
| DA-12 Docs | R/P: doc generation/source/missing; html/tui server renders; scratch export/no-op repeat/stale protection/viewer/upstream refusal | Source toggle; export form summary and invalid destination; final dark retest. Docs/Copy/package navigation/Mermaid open-close and missing-doc recovery now observed |
| DA-13 Tasks | R/P: local REST and synthetic task backend fixture | Unavailable/no-results/provider configuration/current task/branch UI; failure recovery. Real providers and addon quick-create explicitly deferred |
| DA-14 Settings | R/P: backend tier/upstream lock/shared network constraints | Root/site/label/read-only/publish-branch/plain-directory UI and restore/cancel. Shared-server secret configuration deferred to dedicated fixture |
| DA-15 Access | F/P (admin upstream read-only) + R/P (tier refusals) | Actual viewer/extender sessions, bookmarked missing artifact, browser-only mode explanation and denial recovery |
| DA-16 Assist/navigation | F/P: fake-provider proposal, unsaved preview, selective hunk rejection, save and approval outcome | Cancel/unavailable/empty/conflicting proposals; command palette, tab close, full keyboard/focus walk. Real model provider explicitly unverified |

### Evidence reused without duplicate test runs

- `/tmp/moqui-ui-framework-gate3.log`: exact passed `devaide::autodoc_shows_a_doc_and_exports_a_tree_into_the_site`, `the_index_follows_reloads_and_extensions`, `the_workspace_saves_checks_commits_and_publishes`, `the_git_depth_writes_on_scratch_repositories`, `tasks_from_mantle_and_rest`, `the_screen_tree_orders_moves_and_removes`, `a_try_saved_as_a_spec_passes`, `a_site_seca_runs_after_its_save`, and `an_entity_save_relinks_and_adds_its_column`. These are individual successful backend results in a gate that also had other failures; no whole-gate pass is claimed here.
- `/tmp/moqui-ui-framework-final.log`: ArtifactDocs 7 passed feature lines; ArtifactIndex 6; ServiceWriter 7. Server renders are supporting regression evidence, not human visual inspection.
- Coordinator's `plans/UI-Refinement.md` UI-014 and observed-activity table: limited Home/search/Go to artifact visual results. Its exact fresh-client and focus retest gaps remain applicable.
- `/tmp/ui-devaide-acceptance/latest-results.json`: all eight functional browser journeys with exact logs, initial failures and successful retests. Harness default 1280×900; screenshot branches were unset. Alternate themes, 390px, 200% zoom and full keyboard-only operation are not implicitly covered.

### Small next UI checks, without another broad render sweep

1. **Read-only artifact chain (DA-05/12):** at 1920×1080, open Artifact Search empty, search `UserAccount`, open References through the result, follow one incoming reference and return, open Doc, follow a package breadcrumb, use Copy and open/close one Mermaid diagram. Repeat only the key result/dialog in dark mode; this closes navigation gaps the graph test does not touch.
2. **Recovery slice (DA-05/12):** use References with no ID, unknown `entity:ui.review.Missing`, and an artifact with zero outgoing refs; use Doc with the same missing ID, then its recovery link. Record feedback, retained query and focus. Avoid re-rendering unrelated app screens.
3. **Guided error slice (DA-07/15):** open Services/Edit with a missing artifact, recover through the list; inspect one real Rust-backed service's edit limitation. Use viewer/extender sessions only when separate disposable accounts exist.
4. **One export form journey (DA-12):** after a writable disposable site is explicitly confirmed, export a single fixture kind/component into its own fresh `autodoc/`, inspect summary, repeat unchanged and test a refused relative path. Backend behavior already passed, so the added value is visible destination/action hierarchy and feedback. Never point export at framework/addon sources.

No additional tests or builds were run during this reconciliation: the relevant existing checks above already passed, and the child CUA inventory returned no enabled browsers because the Mac was locked. This is a browser-access blocker, not a failed application check. Parent owns server 8090 and any unlock interaction. Additional fixtures must clear inherited `MOQUI_RUNTIME`/`MOQUI_CONF` (including lowercase variants) and explicitly avoid mutable runtime/addon symlinks.


### Monitor, Dashboards and Gallery


| ID | Existing feature | Verified assertion scope | Important limit |
|---|---|---|---|
| A01 | Shipped dashboard renders in the QVT shell | R: `monitor-overview`, heading and HTTP-request content | No interaction, visual or empty-state proof |
| A02 | Shipped definitions explain copy workflow and expose version history | R: Standard Definition contains copy guidance, Save as copy and export link | Does not submit the Copy form or compare/restore history |
| A03 | Authorized log search without QVT shell | R: `/apps/monitor/Logs` has permission guidance/query UI | No restricted actor or search result assertion |
| A04 | Explore typed filter form | F: add severity filter, negate, disable, submit and rerender | DOM-driven values; no saved-search, history or invalid-query lifecycle |
| A05 | New panel options are searchable | F: option search finds Decimal places | No save, keyboard or blur behavior checked here |
| A06 | Explicit navigation records recent dashboards | F: click overview in Home, return to Home, find it in Recent | Star/filter combinations, resetting and removal not exercised |
| A07 | OTLP JSON/protobuf for all three signals | F: create source key, post spans/logs/metrics in both encodings, query matching records, revoke fixture key | Fixture requests are browser helpers; not an external collector setup walk |
| A08 | Structured UI saves/joins/drills/exports dashboard | F: author aggregations and multiple queries, join/organize transforms, text panel, variables and nested layout; save/render; click normal/missing/other buckets and compare filtered CSV output; compose a two-dashboard report and inspect CSV/ZIP preview content | Several controls use dispatched change events and preview/export helper calls; schedule/send/PDF/XLSX, version restore, permissions and visual chart judgments remain open |
| A09 | Monitor Logs follows trace/context links | F: actual trace link resolves to Monitor TraceDetail; surrounding entries opens Monitor LogContext; Compare link stays under Monitor | Trace detail asserts identifier/return link, not detailed span interactions; no Monitor-only actor in this test |
| A10 | Monitor overview inspection/export | F: inspect panel and CSV export through Monitor-owned transitions, no panel error | One panel; exported data contents not checked by this feature |
| A11 | Ingest key shows its token once | F: Create ingest key form produces token message and returns to Sources | Does not revisit to prove absence or click revoke UI |
| A12 | Dashboard time form retains context | F: Standard Services form changes from-range while preserving service variable and selected panel | One valid range; no invalid/timezone/relative-boundary checks |
| A13 | Metric picker and grouped chart | F/R: fixture metric and group choices exist, expected selected-title/chart/query editor content | Values selected by URL, not a full picker interaction; no absent metric state |
| A14 | Trace filters and invalid duration | F: service/name/max-duration limits match expected fixture; ERROR returns none; `minDuration=abc` yields ignored-value guidance | Direct URL inputs; no manual filter form, paging or invalid trace-detail route |
| A15 | Errors, Services and Compare charts | R: expected headings/panels and version selectors render | No error-detail drill, version choice or diagnostic conclusion |
| A16 | Time picker, panel menu and embed | F: choose last hour; shipped menu offers Explore/Share but no Remove; Explore route; helper-copy definition; Duplicate/Remove panel UI persists; Panels embed has expected content/open link | Copy happens via helper request, Share not completed, native confirmation replaced in test, no permissions/focus/resize check |
| A17 | Synthetic check plus AI/log-pattern pages | F: add local `/health` check, Run now, see pass/HTTP200 and trace ID. R: AgentTraces and log-pattern content | Does not click trace, exercise failed check/recovery, scrape targets or AI trace fixture |
| A18 | Browser and replay pages | R: Browser dashboard, Standard Replays text, Sources partition guidance | Does not record/replay a session or partition tables |
| A19 | Operations/release/incident/recovery/settings pages | R: Operations panels; Standard Releases, Incidents, Recovery and Settings content | No incident lifecycle, release drill, recovery execution or setting change/revert |

The offline WebrootReport regression also passed 47/47 in this effort, including the added read-only query-selection rule. That is component behavior evidence, not application activity completion.

## Monitor activities

Use a disposable administrator for setup, then an operator granted the needed Monitor/dataset access for ordinary work. Repeat one representative navigation/data access with a Monitor-only actor and one without dataset access. Do not infer these roles from the admin acceptance run.

### M01 — Orient on instance health

- **Actor:** operator. **Start:** signed in, local telemetry fixture with a healthy interval and a failed request/job.
- **Objective:** identify a problem and choose the correct diagnostic entry point.
- **Steps:** launcher → Monitor overview; choose relative and absolute ranges; inspect request/job/error panels; inspect one panel's query/data; export; navigate to Logs, Traces, Metrics, Services or Artifacts; return using breadcrumbs.
- **Expected:** units, time bounds, empty-versus-zero meaning, drill targets and permissions are clear; context survives navigation; export matches the visible selection.
- **Recovery:** empty interval, unavailable query, invalid range, permission-limited dataset, failed export; narrow range or return to overview without losing orientation.
- **Current evidence:** A10 functional panel inspect/export; A01 generic dashboard render. Full overview-to-diagnostic native walk and visual review pending.

### M02 — Investigate log → trace → surrounding context

- **Actor:** operator, then Monitor-only viewer. **Start:** linked local log/span fixtures and one unrelated service.
- **Objective:** explain a failure using matching records rather than losing context between screens.
- **Steps:** Logs → filter by service/severity/text/time; inspect field values and patterns; open a trace; inspect hierarchy/duration/error span and flame graph; follow Logs for this trace; open surrounding entries; compare ranges; test sorting/paging/tail pause and resume where offered.
- **Expected:** query/filter pills and trace identifiers remain explicit; selected context stays under Monitor; truncation and unavailable spans are explained; display/export agree.
- **Recovery:** no matching logs, invalid query, invalid/missing trace ID, missing context record, span limit, disconnected tail, denied dataset. Clear one filter or widen range, retry, return to prior results.
- **Current evidence:** A03 R; A04 shared query-filter F; A09 link/context F; A17 pattern R. Detailed trace controls, tail, permission isolation and recovery pending.

### M03 — Compare metric, service and artifact performance

- **Actor:** operator. **Start:** two metric series/service versions with known values and a quiet interval.
- **Objective:** distinguish workload growth from latency or error-rate change.
- **Steps:** Metrics → choose metric/statistic/group; inspect chart and underlying documents; Services → choose service and time; inspect endpoints/instances; Artifacts → identify costly artifact; use Compare for two ranges/versions; inspect/export a panel.
- **Expected:** chosen dimensions, units, aggregation and time ranges are visible; missing samples are not implied zero; drilldowns retain the selection.
- **Recovery:** absent metric/attribute/version, sparse series, invalid range, query failure; choose known fixture values or reset selection.
- **Current evidence:** A12 F for Standard time form context; A13 fixture picker/chart R/F; A15 Services/Compare R. Artifacts mount and meaningful performance comparison pending.

### M04 — Triage a grouped error and compare a regression

- **Actor:** operator. **Start:** repeated local error fingerprint, sample traces and two releases.
- **Objective:** find when an error appeared and collect diagnostic evidence.
- **Steps:** Errors → filter/sort group → ErrorDetail → inspect count/stack → open sample trace and error logs → compare affected range/version → return to error group.
- **Expected:** fingerprint, sample versus total counts, boundaries and unavailable evidence are understandable; links keep the relevant scope.
- **Recovery:** missing fingerprint/time interval, missing sample trace, invalid duration, no comparator version; widen range or return to group list with filters retained.
- **Current evidence:** A14 invalid duration/search F; A15 error/compare render. ErrorDetail and complete triage flow pending.

### M05 — Connect a local telemetry source

- **Actor:** disposable telemetry administrator; ordinary operator inspects permitted status. **Start:** local fake collector, unused source name and isolated database.
- **Objective:** add a source, observe receipt and revoke access.
- **Steps:** Sources → create key → copy once → send local spans/logs/metrics → inspect counters/source rows → revoke key → verify later ingest is refused. Inspect partition maintenance explanation separately; execute only in a coordinator-approved disposable DB fixture.
- **Expected:** key purpose/expiry and one-time visibility are clear; accepted/rejected records are distinguishable; revocation feedback is immediate.
- **Recovery:** empty source, invalid expiry/key, expired/revoked key, partial batch failure, unavailable exporter; correct fixture input or revoke/recreate without exposing secrets in notes/screenshots.
- **Current evidence:** A07 successful local ingest and helper revocation F; A11 create form F; A18 partition text R. Revoke UI/failures and table-maintenance flow pending.

### M06 — Run and recover a synthetic check or scrape target

- **Actor:** disposable Monitor administrator. **Start:** localhost healthy and intentionally failing endpoints plus a local metrics endpoint.
- **Objective:** configure a check/target and understand its latest outcome.
- **Steps:** Checks → add synthetic check → Run now → inspect result and trace; change expected status or use failing fixture → inspect failure → restore; add/update/run local scrape target; disable/delete disposable entries.
- **Expected:** identity, target, timing and last result are grouped; failure explains its cause and leads to evidence; restoration is observable.
- **Recovery:** malformed URL, timeout, refused host, bad status/metrics payload, duplicate ID; preserve entered values and retry safely.
- **Current evidence:** A17 check create/run success F only. Failed check, trace click, recovery, scrape and delete/cancel pending.

### M07 — Investigate an AI/agent execution

- **Actor:** operator allowed the relevant agent telemetry. **Start:** fake-provider trace with model/provider/agent and one error.
- **Objective:** trace agent cost/latency/failure through the same diagnostics.
- **Steps:** AgentTraces → filter provider/model/agent/error/time → open trace → inspect steps → follow usage/dashboard link → return to filtered list.
- **Expected:** identity, duration/error status and available usage are legible; no empty field implies unavailable data is zero.
- **Recovery:** no provider data, incomplete trace, denied detail or unavailable usage; clear filter or return to known fixture.
- **Current evidence:** A17 page render only. All functional activity and fake-provider outcome pending.

### M08 — Diagnose browser experience from a sampled replay

- **Actor:** operator permitted browser telemetry/replay. **Start:** a masked local session recording with trace linkage and one empty user/visit filter.
- **Objective:** connect poor browser performance to a recorded action and server trace.
- **Steps:** Browser dashboard → inspect slow page; Replays → filter user/visit → ReplayDetail → play/pause/seek → follow related trace → return.
- **Expected:** sampling/masking limits, event time and trace context are clear; replay controls remain accessible at target sizes.
- **Recovery:** missing/expired recording, no events, unsupported event, denied replay, unavailable trace; show explanation and return to list.
- **Current evidence:** A18 dashboard/list R. ReplayDetail and playback pending.

### M09 — Inspect operations and a release regression

- **Actor:** operator. **Start:** local operation outcomes and two service releases with one synthetic regression.
- **Objective:** explain changed business outcomes after a deployment.
- **Steps:** Operations → choose range/service; Releases → filter/open ReleaseDetail → inspect new errors/regressions → compare versions → open linked error/incident; dismiss only the disposable regression with a recorded reason.
- **Expected:** versions and before/after windows are visible; dismissal is distinguishable from repair; linked diagnostics retain context.
- **Recovery:** missing release, no baseline, no regression, invalid selection or permission refusal; recover to release list/comparison.
- **Current evidence:** A19 Operations/Releases R; A15 comparison R. ReleaseDetail and disposition pending.

### M10 — Work an incident and review proposed recovery

- **Actor:** incident operator, separate recovery approver where configured. **Start:** disposable incident, fake adapter/advisory policy and reversible local action; stop setting recorded.
- **Objective:** distinguish evidence, hypothesis, approval and outcome.
- **Steps:** Incidents → create/filter/open → IncidentDetail update/add note → inspect causes/evidence → confirm/reject disposable hypothesis → review decision/action; Recovery → inspect stop/policies/adapters → approve/refuse/cancel the fixture action → inspect result/regression evidence; restore original policy/stop state.
- **Expected:** action scope and approval requirement are explicit; state transitions and history agree; advisory/unavailable execution cannot look successful.
- **Recovery:** missing incident, refused approval, failed adapter, already-completed action, stop enabled and failed reversal; retain diagnostic evidence and provide a safe return path.
- **Current evidence:** A19 list/Recovery text R; live M10-A create/validation/note/status/filter/resolution and M10-B manual-policy/refusal/approval cancellation/approval/action cancellation/cleanup completed at1920 light. Limited actor denied and launcher recovery observed. UI102–107 need correction/retest; dark/responsive/TUI, cause disposition, quantitative verification and external adapters remain open. No real infrastructure action was performed.

### M11 — Temporarily adjust diagnostics and restore defaults

- **Actor:** disposable diagnostics administrator; viewer observes restrictions. **Start:** original setting values recorded, an allowed scoped target and known traffic.
- **Objective:** temporarily increase useful diagnostics without losing track of the override.
- **Steps:** Settings → find group/key → inspect default/current/source → apply scoped temporary override → inspect active overrides and effect → revert one → add a second harmless fixture override → revert all → confirm restored state.
- **Expected:** scope, temporary lifetime and restart implications are explicit; revert controls distinguish one/all; feedback reflects effective value.
- **Recovery:** invalid key/value/target, conflict, permission denial, failed revert; retain original values and a clear route to restore.
- **Current evidence:** A19 Standard Settings content R. Change/effect/revert and restricted state pending.

## Dashboards activities

### D01 — Find, organize and reopen a dashboard or panel

- **Actor:** report viewer; owner for own organization preferences. **Start:** shipped definitions plus disposable personal dashboard/panel, folders/tags and clean preferences.
- **Objective:** find useful content and resume it quickly.
- **Steps:** Home → filter folder/tag/starred → open dashboard → return → inspect Recent → star/unstar → change filters/reset; Panels → filter/open library definition and preview embed.
- **Expected:** list actions, filter state, stars and recents have distinct meaning; empty sections do not dominate; current filters survive row actions.
- **Recovery:** no match, stale definition, inaccessible panel, unset recent history; clear filters or return to library. Verify keyboard and cancellation.
- **Current evidence:** A06 recent navigation F; A16 embed R/F. Root observed Home layout and approved refinement. Star lifecycle/native filtering and all themes/states pending final coordinator evidence.

### D02 — Read, filter and export a dashboard

- **Actor:** report viewer. **Start:** shipped overview and fixture dashboard with variables, multiple panels and missing/other buckets.
- **Objective:** answer a question using a reproducible range/filter selection.
- **Steps:** open Dashboard → choose relative/absolute range and variables → drill chart bucket → inspect query/data → single-panel view → export CSV/XLSX → return to all panels; use panel menu Explore/Share and inspect kiosk/keyboard behaviors where offered.
- **Expected:** active range, variables and pills are readable; selected series/panel is clear; export matches filtered data; navigation preserves context.
- **Recovery:** no data, unknown variable/panel, invalid time, query/export failure, denied dataset; remove filter/reset range and retry.
- **Current evidence:** A01 R; A08 filtered buckets/CSV content F; A12 Standard range/context F; A16 time picker/menu F. Share completion, XLSX/PDF, keyboard/zoom and failure states pending.

### D03 — Discover a dataset and develop a query

- **Actor:** authorized analyst; repeat one denied dataset with a restricted viewer. **Start:** local log/metric/span datasets and known fields.
- **Objective:** move from dataset schema to useful query and reusable output.
- **Steps:** Datasets → inspect fields → Explore → text/typed filters → negate/disable/remove/pin → inspect sampled field values → sort/page → history → save/open/update search → add panel → open context/trace or compare split.
- **Expected:** dataset access and query mode are visible; typed values/operators survive edits; history/saved searches are distinct; panel receives intended query.
- **Recovery:** unknown field, invalid query, empty result, unavailable dataset, stale saved search, long-running tail; edit/remove offending condition and retry.
- **Current evidence:** A04 filter form F; A09 context links via Monitor F; A13 metric Explore presence R. Datasets page, saved-search/history/pinning/add-panel and restricted flow pending.

### D04 — Copy and edit a definition, then inspect history

- **Actor:** definition owner; read-only viewer on shipped definition. **Start:** shipped overview and disposable copy ID.
- **Objective:** safely adapt panels while retaining a route to view the result and prior versions.
- **Steps:** Definition → inspect shipped/read-only notice and query selection → fill Save as copy → edit title/query/aggregation/transform/variables/layout → Save version with note → Open dashboard → return → compare/restore version → export bundle with references → import under disposable prefix; test cancel/delete while referenced.
- **Expected:** copy versus update is unmistakable; native input/blur/save preserves draft; query/panel selectors are inspection controls when read-only; version and reference effects are clear.
- **Recovery:** duplicate ID, invalid JSON/query, stale version, missing reference, permission refusal; preserve draft, show validation and reopen the intended version. Never delete shipped data.
- **Current evidence:** A02 guidance R; A05 options search F; A08 author/save/render F; A16 helper-copy and panel duplicate/remove F; offline47 includes read-only query rule. Root native copy/edit observations need final outcome merged. Version compare/restore/import/delete safeguards remain pending UI execution.

### D05 — Annotate a range and compare periods

- **Actor:** analyst permitted annotation writes. **Start:** disposable event/range and dashboard with known data before/after.
- **Objective:** relate an annotation to a visible change.
- **Steps:** Annotations → create point/range annotation → filter/inspect it through Explore → open Dashboard → verify marker → Split → choose ranges or versions → inspect corresponding panels.
- **Expected:** annotation time/range/tags and comparison labels are clear; scope and timezone are preserved.
- **Recovery:** reversed/invalid time, empty text, no matching data, missing version; fix input, reset filters and return to annotated range.
- **Current evidence:** A15 compare R; offline annotation behavior exists. Annotations create/reopen and actual comparison user flow pending.

### D06 — Define and manage a local alert

- **Actor:** report owner; separate administrator for configured outgoing contacts if needed. **Start:** local metric query and captured-notification/local webhook fixture.
- **Objective:** express a condition, route its result and understand quieting rules.
- **Steps:** Alerts → new AlertRule → choose query/reducer/threshold/pending/recovery timing → save/evaluate → inspect instances → add contact/silence/mute → create/reuse/copy shared AlertShared item → inspect Used in → pause/unpause → remove disposable references and definition.
- **Expected:** pending/firing/recovered/no-data/error are distinguishable; shared versus rule-local configuration is clear; delivery and quieting status are explicit.
- **Recovery:** invalid threshold/JSON/time window, no data, failed local delivery, denied contact, referenced-delete refusal; correct input and reevaluate without external sends.
- **Current evidence:** P. Monitoring does not visit Alerts, AlertRule or AlertShared. Offline/service coverage elsewhere is not completion of this UI activity.

### D07 — Compose, preview and schedule a report or playlist

- **Actor:** report owner; fixture recipient/storage permission. **Start:** two accessible dashboards, temporary output location and captured mail.
- **Objective:** assemble reusable output and distinguish preview from scheduled delivery.
- **Steps:** Reports → compose Definition type report → order dashboards/set per-entry ranges/variables/layout → save → preview/download → create local file/captured-mail schedule → Send now → inspect output/status → pause/unpause; create Playlist → play/pause/advance/reverse → inspect unavailable member.
- **Expected:** ownership/access, report scope, playback timing and schedule destination are clear; downloaded output matches preview; empty/skipped delivery is explained.
- **Recovery:** empty report, inaccessible dashboard, invalid cron/destination, renderer unavailable, failed capture sink, missing playlist entry; correct and retry or pause. Real email delivery remains unverified.
- **Current evidence:** A08 two-entry composition and CSV/ZIP preview helper F. Reports visible preview controls, PDF/PNG/XLSX, schedule/send and Playlist UI pending.

### D08 — Synchronize definitions with a disposable repository

- **Actor:** administrator with report Git-sync authority. **Start:** scratch definition folder, local bare remote/repository fixture and recorded sync user.
- **Objective:** make definition changes reviewable and recover synchronization failures.
- **Steps:** GitSync → Add sync → select repository/directory/user → Push now → inspect files/status → modify valid fixture definition in local remote → Pull now → inspect loaded definition → disable/enable; inspect malformed definition and conflict handling.
- **Expected:** direction, folder mapping and run-as identity are explicit; counts/errors belong to the correct sync; independent failures do not obscure successful files.
- **Recovery:** missing repo/path, denied user, bad JSON/reference, failed local push/pull; retain last-known definition and repair fixture. Never push the framework branch.
- **Current evidence:** P. No Monitoring UI Git-sync execution.

## Complete Monitor/Dashboards screen-to-activity mapping

All paths below are relative to `runtime/base-component/tools/screen/`. Monitor and Dashboards have **25 and 16 XML definitions including their roots**, respectively. Neither tree has separate QVue files. Detail screens and non-menu helpers remain explicit entries because bookmarks, row links or included widgets reach them.

| XML definition | Primary route/role in UI | Activities | Current acceptance evidence |
|---|---|---|---|
| `Monitor.xml` | `/qapps/monitor` → dashboard; shell | M01–M11 | Root navigation itself P |
| `Monitor/dashboard.xml` | Monitor overview; includes Dashboard engine | M01 | A10 F |
| `Monitor/Logs.xml` | Monitor mount of Explore plus local context links | M02 | A03 R, A09 F, A17 R |
| `Monitor/LogContext.xml` | Hidden Monitor mount of shared surrounding-entry view | M02 | A09 F |
| `Monitor/Traces.xml` | Trace search | M02, M04 | A14 F |
| `Monitor/TraceDetail.xml` | Trace waterfall/flame; traceId required | M02, M04, M06, M07, M08 | A09 route/link R within F navigation |
| `Monitor/Metrics.xml` | Metric picker/chart + Explore | M03 | A13 F/R |
| `Monitor/Services.xml` | Included `monitor-services` dashboard | M03 | A12 F, A15 R |
| `Monitor/Artifacts.xml` | Included `monitor-artifacts` dashboard | M03 | P |
| `Monitor/Errors.xml` | Error groups/chart | M04 | A15 R |
| `Monitor/ErrorDetail.xml` | Error fingerprint → logs/sample traces | M04 | P |
| `Monitor/Split.xml` | Monitor-owned mount of Dashboards/Split | M03, M04 | A09 link, A15 R |
| `Monitor/Sources.xml` | Ingest keys, counters, source/partition administration | M05 | A07/A11 F, A18 R |
| `Monitor/Checks.xml` | Synthetic checks and scrape targets | M06 | A17 success F |
| `Monitor/AgentTraces.xml` | AI/agent filters, trace and usage links | M07 | A17 R |
| `Monitor/Browser.xml` | Included browser-experience dashboard | M08 | A18 R |
| `Monitor/Replays.xml` | Replay list/filter | M08 | A18 Standard R |
| `Monitor/ReplayDetail.xml` | Hidden replay playback/events/linked traces | M08 | P |
| `Monitor/Operations.xml` | Included business-operations dashboard | M09 | A19 R |
| `Monitor/Releases.xml` | Release list/filter | M09 | A19 Standard R |
| `Monitor/ReleaseDetail.xml` | Hidden release errors/regressions | M09 | P |
| `Monitor/Incidents.xml` | Create/filter/list incidents | M10 | A19 Standard R; live create/invalid/filter/resolved recovery, UI102 pending |
| `Monitor/IncidentDetail.xml` | Hidden incident evidence/decisions/actions | M10 | Live note/status/resolve, refuse/approve/cancel manual action, missing/denied bookmark; UI103/104/107 pending |
| `Monitor/Recovery.xml` | Stop/policies/adapters/recovery action status | M10 | A19 Standard R; live Manual adapter/policy create/invalid/edit/disable and canceled-action link; UI105/106 pending |
| `Monitor/Settings.xml` | Temporary diagnostic settings/overrides | M11 | A19 Standard R |
| `Dashboards.xml` | `/qapps/dashboards` → Home; shell | D01–D08 | Root navigation itself P |
| `Dashboards/Home.xml` | Dashboard library, stars/recent/folders/tags | D01 | A06 F |
| `Dashboards/Panels.xml` | Panel library + optional embed; includes Home | D01, D02, D04 | A16 embed R/F |
| `Dashboards/Dashboard.xml` | Dashboard view/time/variables/panels | D02 | A01 R, A08/A16 F |
| `Dashboards/Definition.xml` | Hidden generic definition editor/copy/history/import/export; dashboard/panel/report variants | D04, D06, D07 | A02 R, A05/A08 F |
| `Dashboards/Datasets.xml` | Dataset/schema inspection and Explore/Definition links | D03 | P |
| `Dashboards/Explore.xml` | Query/filter/history/save/add-panel/context view | D03 | A04 F; shared paths exercised via Monitor |
| `Dashboards/LogContext.xml` | Hidden surrounding entries, reused by Monitor | D03, M02 | Monitor mount A09 F; own direct route P |
| `Dashboards/Split.xml` | Hidden comparison view, reused by Monitor | D02, D05, M03 | Monitor mount A15 R; own route P |
| `Dashboards/Annotations.xml` | Create annotations + included Explore | D05 | P |
| `Dashboards/Alerts.xml` | Rules/shared items/instances/evaluate/delete | D06 | P |
| `Dashboards/AlertRule.xml` | Hidden rule query/conditions/contact/silence/mute editor | D06 | P |
| `Dashboards/AlertShared.xml` | Hidden contactPoint/silence/muteTiming variants and Used in | D06 | P; all three variants need inspection |
| `Dashboards/Reports.xml` | Compose/preview/schedule/send/playlist creation | D07 | A08 preview endpoint helper F; actual page flow P |
| `Dashboards/Playlist.xml` | Saved playlist playback and fallback list | D07 | P |
| `Dashboards/GitSync.xml` | Folder/repository synchronization | D08 | P |

**Shared/mode implementation accounting:** Monitor mounts of Dashboard/Explore/Split/LogContext are not separate copies of the engine, but their authorization, local links and contextual headings need route-specific checks. `Definition.xml` contains QVT/TUI/Standard render sections; `TraceDetail.xml` has QVT/TUI waterfall and Standard text; Explore has query editor/tail/value-filter render sections; ReplayDetail and Playlist have specialized rendering. Inspect these implementations within the same activities, not as fictitious extra screen routes. Shared client implementation is `webroot/screen/webroot/js/WebrootReport.js`; no additional application screen should be counted for its components.

## Gallery activities

Gallery is a framework fixture over real framework data. A successful demo message may deliberately mean that nothing was written; identify that separately from an actual entity/file update. Use disposable Enumeration, Example, user and wiki records when testing writes. Some Gallery actions intentionally simulate business behavior; verify the named outcome rather than inventing production side effects.

### G01 — Orient and navigate by task

- **Actor:** signed-in Gallery administrator; one unauthorized user checks denial. **Start:** framework demo data, clean navigation state.
- **Objective:** understand the archetypes and reach a related record without losing orientation.
- **Steps:** launcher → Landing → number card/assigned work/recent item → List/Detail/Record360 → breadcrumbs/back → shell search and Gallery palette provider → Widgets shortcut.
- **Expected:** cards and sections explain their scope; selected record and route are clear; unrelated static demo text is distinguishable from current data.
- **Recovery:** no recent/assigned rows, missing record, no palette results, access denied; return to known list/landing.
- **Evidence:** P here; existing Gallery server acceptance checks cards/provider URLs, browser sweep only opens pages.

### G02 — Find, arrange and act on a list

- **Actor:** Gallery administrator working only on disposable enumerations. **Start:** enough fixture rows for paging, a saved find and distinct row values.
- **Objective:** locate a record and retain a useful list arrangement.
- **Steps:** List/Widgets → Find Options → filter/sort/apply → inspect summaries/reset → Saved Finds create/apply/update/default/unset/delete → Columns reorder/hide/save/reset → page/export → select rows and run disposable bulk action; create/clone/open a fixture row.
- **Expected:** filtering, sorting, saved-find/default actions and column changes have distinct labels and outcomes; footer/actions remain reachable; preferences survive reopening; export matches scope.
- **Recovery:** empty/invalid query, no selected rows, duplicate name/ID, canceled deletion, inaccessible saved preference, failed submit; preserve inputs, clear/reset and reopen with keyboard focus intact.
- **Evidence:** P here. Coordinator's shared-widget manual results belong in the active plan. Existing sweep does not complete this lifecycle.

### G03 — Read and act on record detail/360 panels

- **Actor:** administrator on disposable record/user; limited user for representative refused action. **Start:** known Enumeration/Example record, disposable user and optional file/note fixture.
- **Objective:** distinguish record identity/status, related data and activity/actions.
- **Steps:** Detail → inspect header/status/siblings/history → valid status action and cancel another → Record360 tabs/groups/activity/notifications → inspect Chatter and RecordFiles → add a local note/attachment when supported → open ExampleDetail through Views peek.
- **Expected:** primary action reflects record state; labels separate metadata from editable fields; drawer/peek navigation preserves selection; simulated actions say so.
- **Recovery:** missing ID, empty related section, denied action, invalid status, upload failure; return to list and preserve the original record. Do not send real mail or impersonate a real user while reviewing the demo controls.
- **Evidence:** P here; existing server checks cover detail status transition, chatter markup/mention cleaning and Views peek shape, not full native interaction.

### G04 — Create and validate through dialogs

- **Actor:** Gallery administrator. **Start:** unused disposable Enumeration/Example IDs and a known country/state pair.
- **Objective:** complete a small creation/edit flow with recoverable validation.
- **Steps:** Dialog → create/edit fixture Enumeration → open Detail; Search → filter Example records → open create dialog → save → EditExample → edit/save/back; open address dialog and change country/state; Contact → submit intentionally invalid data, inspect field errors, correct or cancel.
- **Expected:** form grouping and primary action are clear; dependent choices follow parent selection; errors point to fields and retain entered values; nested dialogs restore focus.
- **Recovery:** blank/duplicate ID, dependent field mismatch, deliberate Contact refusal, canceled create, unknown Example bookmark; recover without accidental data loss.
- **Evidence:** P here; Gallery sweep covers Dialog/Search; Contact/EditExample need explicit walkthrough rather than assuming sweep coverage.

### G05 — Complete a focused task and scan workflow

- **Actor:** operator with Gallery access. **Start:** Task demo line and Handheld known scan fixtures (`A-100`, duplicate code, unknown code, GS1 and bin fixture).
- **Objective:** perform one action at a time with clear feedback and next step.
- **Steps:** Task → confirm/short/skip → inspect dynamic-step reload and remaining lines; Handheld → one-match scan → duplicate chooser → no-match → put-away bin → Confirm; repeat by keyboard and at 390px.
- **Expected:** current line/item and quantity are prominent; success/refusal explains whether data is real or demo-only; scan identity/type and focus are retained; explicit Confirm is distinct from auto-submit.
- **Recovery:** wrong/unknown scan, empty input, chooser cancellation, refused type and repeated scan; correct/rescan without advancing incorrectly.
- **Evidence:** P here; existing Gallery server scan checks and `HandheldScan` browser regression are available, not run by this agent in this assignment.

### G06 — Change a record through board, calendar and tree views

- **Actor:** Gallery administrator. **Start:** disposable Example rows with valid/invalid status transitions, scheduled times and WorkTree fixtures.
- **Objective:** understand how alternative list views modify the same records.
- **Steps:** Views → table/detail peek → board grouping/status move → calendar reschedule → tree expand/reorder/estimate → compare underlying row; separate Board and Calendar QVue pages → move card/create or move event; inspect their TUI twins.
- **Expected:** selected record, grouping, allowed action and persistence are clear; drag actions have usable alternatives; tree totals and timezones agree with edited data.
- **Recovery:** rejected status, canceled drag, out-of-range time, cyclic move, empty group, failed save; revert visual state to saved data and retain selection.
- **Evidence:** P here; Gallery server acceptance checks Views transitions/refusal/time changes, grouping/peek/tree. Render smoke of separate QVue/TUI pages is not a drag-and-drop or accessibility walk.

### G07 — Navigate split and tray-like panel layouts

- **Actor:** Gallery administrator. **Start:** countries with children plus Panel's demo groups.
- **Objective:** work through related items without losing list context.
- **Steps:** Split → filter countries → select one → inspect/edit disposable data if permitted → back to side list at narrow width; Panel → inspect overdue/due/later groups → mark/snooze demo item → quick create/cancel → open linked record and return.
- **Expected:** side selection appears in URL; split resizing/narrow back navigation is understandable; group headings and empty states read clearly; demo feedback cannot be mistaken for persisted business data.
- **Recovery:** no country, no matches, empty group, missing linked record, canceled dialog; return with list filter preserved.
- **Evidence:** P here; existing server Split assertion checks filter/selection separation. Panel actions explicitly write no business data in source.

### G08 — Complete and recover a wizard

- **Actor:** Gallery administrator. **Start:** empty session draft and unused demo customer input.
- **Objective:** progress through Customer → Options → Review with confidence about saved state.
- **Steps:** Wizard → attempt invalid Customer → correct → Options → back → verify retained draft → Review → inspect summary → confirm → observe reset/first step; repeat cancel/back/refresh.
- **Expected:** current/completed steps and primary/secondary actions are clear; validation blocks advancement; review matches inputs; completion and draft reset are explicit.
- **Recovery:** missing required field, invalid choice, direct later-step bookmark, refresh or cancellation; stay on actionable step without losing valid draft.
- **Evidence:** P here; existing server acceptance directly exercises step transitions/draft/refusal/completion. Native wizard navigation, focus and responsive behavior still require execution.

### G09 — Edit rich text, Markdown and code with history

- **Actor:** Gallery administrator. **Start:** disposable WikiSpace at a verified temporary resource location, optional scratch Git-backed space, disposable Example for CodeField.
- **Objective:** edit content, compare versions and recover a prior revision.
- **Steps:** Editor → edit/save → inspect history → compare/view/restore → inspect publish controls against local fixture only; CodeField → edit JSON description → save/reopen → force editor-library unavailable fixture and inspect text fallback; inspect long text at zoom.
- **Expected:** unsaved/current/historical content is distinguishable; diff sides and restoration scope are clear; code editor and fallback submit equivalent values.
- **Recovery:** invalid content, missing example, unavailable library/resource, failed save, conflict/cancel; preserve draft and return to current revision. Verify resource roots before writes; do not edit a source-checkout WikiSpace by assumption.
- **Evidence:** P here; existing server editor acceptance covers versions/compare/restore for resource and Git spaces. CodeField is outside the general Gallery sweep and needs explicit inspection.

### G10 — Use shell quick creation with record context

- **Actor:** signed-in Gallery administrator. **Start:** Detail/Record360 record and empty quick-create dialog.
- **Objective:** create a note/task without losing the current screen or record context.
- **Steps:** shell quick create → Gallery note → inspect context/validation → cancel/reopen → save → verify dialog/page behavior; repeat Gallery task QVue and its inline TUI form; edit then discard/cancel where offered.
- **Expected:** record context is explicit; close/save/discard actions are distinct; focus returns to invoker; Gallery task's simulated-save message is accurate.
- **Recovery:** blank required input, missing context, failed submit, dirty cancel; dialog retains state or confirms discard appropriately.
- **Evidence:** P here; existing server acceptance checks provider URLs and screen rendering, not this complete lifecycle.

### G11 — Configure preferences and inspect a report

- **Actor:** Gallery administrator. **Start:** original General settings recorded and fixture artifact hits across known dates.
- **Objective:** understand configuration scope and obtain a useful parameterized report.
- **Steps:** Settings → inspect General → change harmless disposable preference → save/reset → inspect Users/Enumerations/Status/Organization groups; Report → choose parameters/range → inspect chart/table totals and export where available.
- **Expected:** settings groups, applied values and reset are clear; report units/time range match the table; empty report is understandable.
- **Recovery:** invalid preference, canceled/reset state, no hits, invalid dates, export unavailable; restore original settings and widen known range.
- **Evidence:** P here; existing sweep only opens Settings/Report. No preference or report user flow established by this handoff.

### G12 — Inspect shared widgets and feedback semantics

- **Actor:** Gallery administrator, including keyboard-only pass. **Start:** Widgets fixture and alternate theme selection.
- **Objective:** compare the shared pattern used throughout applications.
- **Steps:** Widgets → text/select/date/code/rich-text/remote/dependent inputs → validation/refusal → record lookup/mention → editable cell → dialog/dynamic content → button hierarchy and status chips; use List controls per G02; compare default themes and one alternative scheme after token changes.
- **Expected:** ordinary text/control contrast meets plan targets; labels, hints, disabled state, focus and selected state remain distinguishable; keyboard and cancellation work at 390px/200%zoom.
- **Recovery:** invalid value, unavailable remote choices, empty dataset, failed inline save, canceled confirmation; retain value and expose a useful retry/reset.
- **Evidence:** P here; existing server acceptance checks list row binding, chips/editables and generic rendering. Coordinator's measured contrast and shared-widget screenshots should be merged separately.

### G13 — Read designed and plain-text messages

- **Actor:** Gallery administrator as reader. **Start:** Message's local HTML/plain text fixture, untrusted sender state.
- **Objective:** read content and reply history while understanding remote-image controls.
- **Steps:** Message → inspect designed/plain rendering → expand quoted history → Show images/Trust sender → hide again → inspect safe link and disabled unsafe content → repeat themes/zoom.
- **Expected:** readable body/metadata, bounded image/layout content and clear trust state; remote-image action is understandable; fixture script is blocked and no real message is sent.
- **Recovery:** missing image, long quote, malicious fixture link, empty body, canceled trust; return to safe readable state.
- **Evidence:** P here; Gallery browser smoke allows the expected sandboxed-script console report. That allowance does not prove every sanitization/reading interaction or light/dark readability.

## Complete Gallery screen-to-activity mapping

Paths are relative to `runtime/dev-component/example/screen/`. The Gallery root plus descendants contain **31 XML definitions**, **3 QVue templates** and **2 external TUI JSON twins**. The table accounts for every file. The last column lists **available test coverage**, not a new passing result. Coordinator browser execution is recorded separately in the bounded-batch notes; G05, G08 and parts of G04/G10 now have explicit live outcomes there.

`GS` = entry appears in `acceptance/Gallery.spec.js`'s general render list; `GB` = entry appears in `acceptance/client/GallerySmoke.spec.js`'s browser render list. `Targeted` means the existing server acceptance has additional stated assertions. The general lists have23 entries; they do not cover all31XML routes, every variant or every user action.

| Definition/template | Route or use | Activities | Available coverage, execution not claimed here |
|---|---|---|---|
| `Gallery.xml` | `/qapps/gallery` → Landing; menu/shell providers | G01, G10 | Root render and provider assertions |
| `Gallery/Landing.xml` | Landing cards/work/recent sections | G01 | GS/GB; targeted card links |
| `Gallery/List.xml` | Enumeration find/create/bulk/clone/detail list | G02 | GS/GB |
| `Gallery/Detail.xml` | Enumeration header/status/history/related panels | G03 | GS/GB; targeted status/provider context |
| `Gallery/Record360.xml` | User overview/groups/activity/notifications/chatter | G03 | GS/GB |
| `Gallery/Chatter.xml` | Hidden record-panel route; notes/mentions/card | G03 | GS/GB; targeted note cleaning/mentions/card |
| `Gallery/RecordFiles.xml` | Hidden record-file panel route | G03 | GS/GB |
| `Gallery/ExampleDetail.xml` | Hidden Views peek detail | G03, G06 | Targeted peek markup; direct detail needs inspection |
| `Gallery/Dialog.xml` | Create/edit Enumeration dialogs and operator examples | G04 | GS/GB |
| `Gallery/Search.xml` | Example search/create and dependent address dialog | G02, G04 | GS/GB; additional scripted TUI coverage exists elsewhere, not run here |
| `Gallery/EditExample.xml` | Hidden created/selected Example edit route | G04 | Outside general GS/GB list |
| `Gallery/Contact.xml` | Hidden deliberate contact-validation form | G04 | Outside general GS/GB list |
| `Gallery/Task.xml` | Focused step and dynamic stepOnly fragment | G05 | GS/GB; stepOnly is a variant of this route |
| `Gallery/Handheld.xml` | Scan/chooser/put-away confirmation | G05 | GS/GB; targeted scan transition assertions; existing HandheldScan browser regression |
| `Gallery/Views.xml` | Shared form-list table/board/calendar/tree and peek | G02, G06 | GS/GB; targeted status/time/peek/tree assertions |
| `Gallery/Board.xml` + `Gallery/Board.qvue` + `Gallery/Board.tui.json` | Board route, QVue implementation and external TUI twin; same activity | G06 | GS/GB plus generic TUI render; functional board walk pending |
| `Gallery/Calendar.xml` + `Gallery/Calendar.qvue` + `Gallery/Calendar.tui.json` | Calendar route, QVue implementation and external TUI twin | G06 | GS/GB plus generic TUI render; functional calendar walk pending |
| `Gallery/Split.xml` | Country side list/detail | G07 | GS/GB; targeted URL/filter/selection |
| `Gallery/Panel.xml` | Tray-content archetype, quick actions/create | G07 | GS/GB; actions explicitly simulated |
| `Gallery/Wizard.xml` | Wizard container/default Customer step | G08 | GS/GB; targeted step/draft/completion |
| `Gallery/Wizard/Customer.xml` | First step | G08 | Targeted direct transition/render checks |
| `Gallery/Wizard/Options.xml` | Second step | G08 | Targeted direct transition/render checks |
| `Gallery/Wizard/Review.xml` | Summary/completion step | G08 | Targeted direct transition/render checks |
| `Gallery/Editor.xml` | Wiki edit/version/diff/restore/publish | G09 | GS/GB; targeted resource/Git version behavior |
| `Gallery/CodeField.xml` | Hidden code-editor field; Example ID required | G09 | Outside general GS/GB list; browser-gate fixture noted in source |
| `Gallery/QuickNote.xml` | Hidden shell quick-create note route | G10 | GS/GB; provider URL assertion |
| `Gallery/QuickTask.xml` + `Gallery/QuickTask.qvue` | Hidden shell quick-create task; TUI twin is inline XML render section, not another file | G10 | GS/GB; generic TUI render/provider URL assertion |
| `Gallery/Settings.xml` | General and grouped settings archetype | G11 | GS/GB |
| `Gallery/Report.xml` | Parameters, chart/table report | G11 | GS/GB |
| `Gallery/Widgets.xml` | Fields/actions/labels/dialogs/markdown/code/lists guide | G02, G12 | GS/GB; targeted row/chip/editable binding |
| `Gallery/Message.xml` | HTML/plain-text reader fixture | G13 | GS/GB; expected blocked-frame-script allowance |

**Non-Gallery helper:** `PublicLink.xml` is present in the example component but is **not mounted by default** and is not a Gallery route. It is an anonymous token-route fixture configured explicitly by a webapp token-route. Record it as a conditional public-share inspection attached to G03 only when a disposable token-route/share fixture is configured; do not claim it was reachable or tested through Gallery. Its files transition checks the token again. This accounts for the component's remaining XML without inventing a default application activity.

## Remaining coverage and next bounded review choices

1. UI-038/039/041–044 are corrected and retested in batch4; UI-040 did not reproduce. Batch5 addresses query labels (UI-037), code-editor names (UI-045), Editor Home/comparison/line-ending recovery (UI-046/048/049), guide text (UI-050), and Material confirmation. Next refine shared list-control names/sorting (UI-051), Visit Detail metadata (UI-052), trace readability (UI-053) and model configuration grouping (UI-054).
2. Complete **G02/G04/G09/G10** keyboard/responsive coverage, including hidden Contact/EditExample/CodeField. G05 scanning, G08 wizard and portions of G04/G06/G07/G10 have now been exercised; retest the identified fixes and complete the remaining recovery/input modes. Carry shared fixes back to actual System/Tools/Monitor pages.
3. Give **D06 alerts**, **D07 reports/playlists/schedules**, **M08 replay**, **M09 release detail**, **M10 incident/recovery** and **M11 diagnostic settings** separate bounded fixtures and activity reviews. Their administrative lists currently have little or no functional acceptance coverage.
4. Verify one **Monitor-only** actor can follow Monitor-local trace/context/compare/export paths, and one denied dataset yields useful refusal. The admin19-feature run is not permission-isolation proof.
5. Exercise **D08 GitSync** only with scratch repository/local remote; reports/alerts only with captured mail/local sinks; recovery only with fake adapters. Explicitly retain external/provider failure or unavailable statuses when a successful fixture is missing.
6. Revisit QVue/TUI twins and in-XML mode sections with real controls; non-error placeholder renders cannot close a mode-specific activity. Include fresh light/dark terminal launch and switching under the coordinator's shared TUI wave.

## Existing commands and evidence use

The user requested named framework acceptance journeys as part of this review. `./moq accept --nobuild tools` runs the19 Monitoring features above; it has already passed once in this effort. `./moq accept --nobuild example` selects Gallery server acceptance and browser smoke. Use the isolated framework runtime for server-backed suites; use repository cwd without a scratch `MOQUI_RUNTIME` override for offline in-page `/files/...` asset fixtures. All test execution in this session must be escalated because sandbox process-liveness probing previously caused scratch cleanup to misidentify a live database. The coordinator owns lifecycle and schedules further runs; this documentation task ran no tests.

Do not duplicate render sweeps or turn these complete user activities into regression tests. Keep focused behavior regressions in existing tests, use the existing acceptance suites for broad journeys, and attach manually inspected screenshots to the active plan. A completed row needs the actual actor/state, observed results and recovery outcome; an inventory row or test name alone is insufficient.
