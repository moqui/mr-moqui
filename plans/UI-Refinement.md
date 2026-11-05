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
| Shared shell and widgets | Navigation, lists, saved finds, columns, forms, preferences | In progress | Find/filter and save/update/default/reset observed; delete cancellation passed. Columns move/undo/hide/save/reset and keyboard move/hide/Display, 1280/390 Find, Escape focus return and appearance reload passed. Material confirmation Cancel/Escape/focus and approved saved-find deletion now pass in Gallery; real200% browser zoom remains unverified. |
| Gallery | Archetypes and shared widget reference | Refined; partial activities | Server59/59 and browser23/23 acceptance pass. Live list/find confirmation, forms/dialogs, wizard, split, scan/task, board/calendar, message and Editor version/recovery activities recorded at1920 plus targeted390/keyboard checks. G03/G10/G11 and full recovery/mode coverage remain open. |
| Embedded TUI | Navigation, dialogs, themes and focus | Refined; partial retest | System and F1 help readable across live light/dark switches; fresh light and dark launches and F1 help after saved appearance reload also passed. |
| System and Tools | Administration and data/service tools | Refined; partial activities | Overview both themes, Components, SQL success/error/recovery, service XML and echo output/validation/retry observed. Deep administration groups remain pending. |
| Monitor and Dashboards | Diagnosis, reporting and authoring | Refined; partial activities | Monitoring 19/19 at 1440×1000; dashboard copy/edit/save/open, CSV export and filter/remove visually observed at 1920; saved log search restores query/range. See assertion limits and screen map below. |
| Dev AIDE | Artifact discovery, editing, simulation, previews and Git | Refined; verification in progress | Home, creation-dialog cancellation, search empty states, source navigation and Search → References → Docs/copy/package/diagram navigation observed. All eight disposable-repository journeys passed; detailed limits are recorded below. |
| BasicComms | Mail, chat, calls and settings | Recovery fixes tested; partial visual | Local fake-media/encrypted-call/chat 10/10; failed-save retention/retry regression passed. No-account Inbox/Compose inspected at1920/390; failed Close/identity refresh retain text. Hosted setup, two-message import/thread and empty-search recovery observed; triage/sending/settings remain partial. |
| Integration | Cross-app patterns and remaining routes | In progress | Shared styles observed in System, Tools, Dashboards and Dev AIDE; full remaining inventory is explicit below. |

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
| UI-016 | S01 Components: no deployment-history rows says no component was loaded above six loaded components | P3 observed misleading empty state | Screen-local wording; deferred to next administration batch | Distinguish deployment history from loaded components |
| UI-017 | Appearance color editor: new semantic foreground tokens are not listed | P3 source-inspection gap | Core appearance token inventory; deferred | Schemes can override CSS tokens, editor discovery remains pending |
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
| UI-033 | S11 Usage → Check a Quota → empty Check | P3 observed validation gap | Empty form closes silently; valid EX_JOHN_DOE displays quota without a clear cost currency | Empty and valid checks observed; no model request. Required user input and clearer quota presentation deferred. |
| UI-034 | Shared nested breadcrumb menu, System → LLM and Agents → Models | P2 fixed and retested | Menu opener was a span with tabIndex -1 and no role; Tab from System skips to Search | Real button opener: Tab/Enter/Space/Escape and child navigation passed. Text contrast raised from 2.72:1 to 16.9:1 in default dark; existing QVT fixture 30/30. |
| UI-035 | T02 Tools entity browser and edit | P3 observed hierarchy/layout | Database maintenance actions precede discovery; two short edit fields span 1920px; Find return loses the prior filter | Create/find/edit and duplicate recovery work; layout and return-context refinement deferred. `tools-entity-edit-before.jpg` |
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
| UI-051 | S06 Audit Log / Visit Detail, shared list controls | P2 observed missing accessible names / follow-up | Sort arrows, filter operator buttons and active-filter removal buttons have no accessible name | Material default light 1920, DOM/AX inspection; investigate shared generator/widget ownership before correction. |
| UI-052 | S06 Visit Detail request history | P2 observed misleading metadata / follow-up | Static requests show literal undefined Type; hostname and instance UUID are labeled Server IP Address / Server Host Name; timestamps mix local and raw UTC | Visit 100001, default light 1920; local screenshot and trace payload confirm the values. Screen/data mapping investigation pending. |
| UI-053 | S06 → M02 Trace Detail selected error span | P2 measured contrast; P3 information hierarchy / follow-up | Duration text overlaps red timing bar at 3.28:1; selected span exposes full duplicated JSON without a concise diagnostic summary | POST validation failure, default light 1920. Separate duration from bars; summarize status/error/request context with raw data still available. |
| UI-054 | S12 Models → Model default | P3 observed configuration hierarchy / follow-up | More than 30 override controls form one uninterrupted column; Save is far below the viewport while the capabilities column becomes empty | Default light 1920 screenshot. Group identity, context/pricing, execution and advanced limits; distinguish effective configuration from overrides and keep the save action easy to reach. |

| UI-055 | G09 Editor History → Publish | P2 observed action safeguard / follow-up | Publish immediately changes the published marker without confirmation, unlike Restore | John Doe/default light1920, EXAMPLE Welcome03 published successfully; original marker01 restored. Add a version/page-specific confirmation and cancellation check. |
| UI-056 | G09 Editor Edit dialog | P2 observed accessibility / follow-up | Markdown content textbox has an empty accessible name because pageText has an empty title | Live default light1920 on EXGIT; version note is named. Supply a meaningful content title through the existing text-area label. |
| UI-057 | G09 Git-backed Editor comparison | P3 design judgment / follow-up | Full commit hashes size the two line-number columns to about150px each, wasting reading width | Light1920 Git diff; title already shows short hashes. Use short comparison display names while preserving full version parameters and history. |

| UI-058 | S05 Job Run Detail after safe manual run | P3 observed information hierarchy / follow-up | A full-width stack of read-only fields buries outcome/results/timing and repeats User; minute-only times cannot explain short runs | John Doe/light1920, run101012, removedCount0. Evidence `system-job-run-light-before-1920.png`; group outcome/results and runtime metadata, preserve diagnostic links. |
| UI-059 | Shared navigation: Find → immediate result link | P2 suspected race; source gap confirmed, deterministic retest pending | Older menu-data completion can overwrite the newer route; Find→job link once settled back at the list | Repeating the link after Find settled opens detail. Source shows unguarded menu-data completion; Run Job→Job Runs succeeded and is not a second reproduction. Prepare deferred-response fixture before claiming correction. |

| UI-060 | B04 hosted mail setup Options → Back → Next | P2 observed draft loss / follow-up | Unsaved account name and filing policy reset to database defaults after revisiting Test | John Doe/light1920, account100001: UI review local mail + Only what I link became address-name + Every message with a known contact. Preserve scoped draft without implicitly saving sharing changes. |
| UI-061 | B04 hosted mail setup and Account | P3 observed misleading controls/copy / follow-up | Import window, Start syncing, Last sync and provider-removal wording imply an incoming provider for a gateway-delivered account | Light1920, hosted account100001. Evidence `comms-hosted-options-light-before-1920.png`; condition remote-import controls and clarify activation/local storage. |
| UI-062 | B04 Account → Open Inbox | P2 observed broken navigation / follow-up | Both Open Inbox links are disabled; source uses obsolete //Comms/Inbox instead of framework-relative route | John Doe/light1920, hosted account100001. Preserve account filter while resolving the current app mount. |
| UI-063 | B02 thread reading | P2 observed keyboard exclusion / follow-up | Older collapsed message and recipient-details disclosure are click-only; row accessible name concatenates nested actions | John Doe/light1920, two imported synthetic messages. Evidence `comms-thread-light-before-1920.png`; provide named disclosure buttons and focus continuity. |

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
| T02 — Entity find/edit and generated screens | Developer administrator; disposable enumeration/reference row | Create, find, edit and remove a safe entity record | Entity list/detail → find/filter → create/edit; repeat through AutoScreen detail/master relationship path | Field types, keys, relationships, validation and saved result understandable | Duplicate key, invalid type, missing record, cancel and limited role |
| T03 — Data import/export and snapshot | Developer administrator; fixture-only DB and files | Transfer disposable data safely | Export fixture query → inspect artifact → import controlled row → find result; inspect snapshot controls | Scope/format/result/counts clear; downloads useful | Malformed import, unsupported upload and missing file; snapshot restore limited to fresh disposable runtime |
| T04 — Database view authoring | Developer administrator; disposable view | Compose and inspect a local database view | Find view → edit aliases/members → save → view/filter → remove fixture | Source entities, aliases and returned columns clear | Invalid alias/join, empty view and cancelled edit |
| T05 — SQL and script recovery | Developer administrator; disposable DB; read-only examples | Run a safe query and recover from error | SQL Runner SELECT 1 → invalid missing column → SELECT 2; SQL Script Runner controlled local query | Results/columns distinguish from input; error is readable and recovery works | Syntax/missing-column error and denied role; no nonfixture writes |
| T06 — Service invocation | Developer administrator; BasicServices.echo#Data | Inspect and invoke a harmless service | ServiceDetail → Run Service → enter TextIn1 → submit → verify TextOut1; invalid parameter then correct | Service identity, input/output grouping and values remain visible after run | Validation/service error and retry; output must show echoed text; no external services |
| T07 — Statistics and load tools | Developer administrator; isolated run with bounded settings | Inspect diagnostics and understand safe test controls | ArtifactStats, QueryStats, TableStats → inspect/filter; SpeedTest/ServiceLoadRunner inspect before running bounded fixture only | Units, metrics availability and test scope clear | Unsupported platform and empty stats; load execution pending coordinator budget |
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
| `System/DataDocument/Export.xml` | XML widgets | S08 | P |
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
| `Tools/AutoScreen/AutoEdit/AutoEditDetail.xml` | XML widgets | T02 | P |
| `Tools/AutoScreen/AutoEdit/AutoEditMaster.xml` | XML widgets | T02 | P |
| `Tools/AutoScreen/AutoEdit.xml` | XML widgets | T02 | P |
| `Tools/AutoScreen/AutoFind.xml` | XML widgets | T02 | P |
| `Tools/AutoScreen/MainEntityList.xml` | XML widgets | T02 | P |
| `Tools/AutoScreen.xml` | Container → MainEntityList | T02 | P |
| `Tools/DataView/EditDbView.xml` | XML widgets | T04 | P |
| `Tools/DataView/FindDbView.xml` | XML widgets | T04 | P |
| `Tools/DataView/ViewDbView.xml` | XML widgets | T04 | P |
| `Tools/DataView.xml` | Container → FindDbView | T04 | P |
| `Tools/Entity/DataEdit/EntityDataEdit.xml` | XML widgets | T02 | V/P: disposable EnumerationType update persisted; relationship navigation and narrow layout pending |
| `Tools/Entity/DataEdit/EntityDataFind.xml` | XML widgets | T02 | V/P: create/filter/edit/duplicate correction passed; CSV mismatch found (UI-036), deletion unverified |
| `Tools/Entity/DataEdit/EntityDetail.xml` | XML widgets | T02 | P |
| `Tools/Entity/DataEdit/EntityList.xml` | XML widgets | T02 | V/P: exact entity filter and Find navigation passed; maintenance controls inspected only |
| `Tools/Entity/DataEdit.xml` | Container → EntityList | T02 | P |
| `Tools/Entity/DataExport.xml` | XML widgets | T03 | P |
| `Tools/Entity/DataImport.xml` | XML widgets | T03 | P |
| `Tools/Entity/DataSnapshot.xml` | XML widgets | T03 | P |
| `Tools/Entity/QueryStats.xml` | XML widgets | T07 | P |
| `Tools/Entity/SpeedTest.xml` | XML widgets | T07 | P |
| `Tools/Entity/SqlRunner.xml` | XML widgets | T05 | V/P; parent observation |
| `Tools/Entity/SqlScriptRunner.xml` | XML widgets | T05 | P |
| `Tools/Entity/TableStats.xml` | XML widgets | T07 | P |
| `Tools/Entity.xml` | Container → DataEdit | T02 | P |
| `Tools/GroovyShell.xml` | removed | T01 | No longer applicable; web console/Terminal UI and SSH JS executor replace it. Old route returns 404. |
| `Tools/Service/ServiceDetail.xml` | hidden menu | T01 | V/P; focused rule passed |
| `Tools/Service/ServiceLoadRunner.xml` | modes vue,qvue,tui | T07 | P |
| `Tools/Service/ServiceReference.xml` | XML widgets | T01 | P |
| `Tools/Service/ServiceRun.xml` | XML widgets | T06 | V/P; parent observation |
| `Tools/Service.xml` | Container → ServiceReference | T01 | P |
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
| `BasicComms/Chat.xml` | modes vue,qvue,tui; `BasicComms/Chat.qvue`; `BasicComms/Chat.tui.json` | B06/B07 | S/P |
| `BasicComms/ChatTui.xml` | hidden menu; explicit TUI screen | B06/B07 | P |
| `BasicComms/Home.xml` | XML widgets | B01 | S/P |
| `BasicComms/Inbox/Compose.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Inbox/Compose.qvue`; `BasicComms/Inbox/Compose.tui.json` | B03 | S/P |
| `BasicComms/Inbox/ComposeTui.xml` | hidden menu; explicit TUI screen | B03 | P |
| `BasicComms/Inbox/Mail.xml` | hidden menu; modes vue,qvue,qvt,html,tui; `BasicComms/Inbox/Mail.qvue`; `BasicComms/Inbox/Mail.tui.json` | B02 | S/P |
| `BasicComms/Inbox/MailTui.xml` | hidden menu; explicit TUI screen | B02 | P |
| `BasicComms/Inbox/MessageDetail.xml` | hidden menu | B02 | P |
| `BasicComms/Inbox.xml` | Container → Mail | B02 | S/P |
| `BasicComms/Panel/Chat.xml` | hidden menu; modes vue,qvue,tui; `BasicComms/Panel/ChatPanel.qvue`; `BasicComms/Panel/Chat.tui.json` | B06 | P |
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

`runtime/dev-component/demo/screen/Disabled.xml` replaces Tools Entity/{DataImport,DataExport,DataSnapshot,SpeedTest,SqlRunner,SqlScriptRunner} and Service/{ServiceRun,ServiceLoadRunner} only when the separate demo lock-down component is enabled. Map this single user-facing unavailable screen to T03/T05/T06/T07 permission-limited inspection; pending. The normal framework review must not confuse demo seed data with this optional lock-down mode.


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
- **Current evidence:** A19 list/Recovery text R only. IncidentDetail, policies, approvals and all execution/recovery states pending. No real infrastructure action is implied.

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
| `Monitor/Incidents.xml` | Create/filter/list incidents | M10 | A19 Standard R |
| `Monitor/IncidentDetail.xml` | Hidden incident evidence/decisions/actions | M10 | P |
| `Monitor/Recovery.xml` | Stop/policies/adapters/recovery action status | M10 | A19 Standard R |
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
