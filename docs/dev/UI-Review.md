# Reviewing and refining application UI

Review applications through the work people do in them. A successful render is a prerequisite; the review also asks
whether the next action is discoverable, the information is understandable, the result is clear, and the screen is
comfortable to use. The first framework review is recorded in [UI-Refinement](../../plans/UI-Refinement.md).

## Prepare an isolated review

Read the application development practices, the building/testing skills, and the component's README. Inventory the
app's user-facing screens, including nested menus, dialogs and settings. Identify existing regression golden paths
and acceptance walks before adding coverage.

Use a disposable runtime and database with representative demo data. For framework-only review, link only individually tracked base and
dev component directories into the runtime and leave its addon directory empty: installed addons can extend or hide framework
screens. Do not symlink the parent `dev-component` directory: component-creation fixtures must create their files only in the disposable runtime. Never link a mutable database, source-editing workspace or external integration configuration into the
review runtime. Use the existing binary for screen, CSS and most JavaScript changes. Built-in screen generator modules, including ScreenQvt and ScreenCsv, are embedded
in the core binary; rebuild when their source changes so the live server actually uses the new generator. Use an
incremental debug binary for iteration and retain the existing release binary until the final release build.

`moqui test framework --spec ClientFixtures --db fresh --serve 127.0.0.1:8090` supplies demo data and limited-access
fixture users in a scratch runtime; select the bootstrap runtime with `MOQUI_RUNTIME`. The fixture runner also
reads the runtime's component configuration: naming one component selects its specs, not the only component to load.
See [Client Testing](Client-Testing.md#--serve) for server lifecycle and fixtures.

A repeatable bootstrap, run from the repository root with the existing release binary:

```python
import os, pathlib, shutil, subprocess, tempfile
repo = pathlib.Path.cwd()
review = pathlib.Path(tempfile.mkdtemp(prefix="mui-review-", dir="/tmp"))
for group in ("base-component", "dev-component"):
    (review / group).mkdir()
    tracked = subprocess.check_output(
        ["git", "ls-files", f"runtime/{group}/*/component.xml"], text=True).splitlines()
    for manifest in tracked:
        source = repo / manifest
        (review / group / source.parent.name).symlink_to(source.parent, target_is_directory=True)
shutil.copytree(repo / "runtime/conf", review / "conf")
for name in ("install", "chromium"):
    if (repo / "runtime" / name).exists():
        (review / name).symlink_to(repo / "runtime" / name, target_is_directory=True)
(review / "component").mkdir()
env = dict(os.environ, MOQUI_RUNTIME=str(review),
    MOQUI_FRAMEWORK=str(review / "base-component/framework"),
    MOQUI_CONF="conf/MoquiLoadTestConf.xml", instance_purpose="dev", mail_capture_sends="true")
subprocess.run([str(repo / "target/release/moqui"), "test", "framework", "--spec", "ClientFixtures",
    "--db", "fresh", "--serve", "127.0.0.1:8090", "--log-level", "warn"], env=env, check=True)
```

The test runner owns its scratch database and prints its actual path. Keep this process attached so it can be
stopped cleanly. The linked component directories are source: all Dev AIDE write activities must target separate
fixture sites, never these links. Choose other free listener ports if local configuration enables secondary services.

For a review that survives binary restarts, stop a `--keep` fixture runner and its database, then move the entire kept
runtime to a stable review path outside the runner's `/tmp/mt-<pid>-*` namespace before serving it with `--db existing`.
Retain its configuration, `.env`, resources and bootstrap directory targeted by symlinks. A retained scratch path
still names its original process: later fixture cleanup can stop its Postgres after that process exits, even when
the keep marker preserves its files. Keep the fixture `--spec` explicit when restarting the served runner.

Use a unique hostname as well as a separate port for independent browser reviewers (for example
`review.localhost:8090` and `terminal-review.localhost:8091`). Cookies are not isolated by port. One coordinator owns
server lifecycle and shared files. Use separate disposable repositories and a local remote for Dev AIDE writing and
Git activities. Capture mail locally; use fake media and local participants for communication flows.

## Define activities and coverage

Write the activity before executing it:

| Field | Record |
|---|---|
| Actor and purpose | Role and the task they need to finish |
| Starting state | Data, permissions and external prerequisites |
| Entry and steps | Route, visible navigation and actions |
| Expected result | Visible confirmation and observable data result |
| Recovery | Empty/invalid state, cancellation, retry, denied access or failure relevant to the activity |
| Review context | Theme, viewport, input method and evidence |
| Outcome | Passed, failed or unverified, with finding references |

Map every user-facing screen to an activity or an explicit inspection. Keep supporting render/HTTP sweep coverage
separate from observed browser activity coverage. Do not call a workflow verified because its source or landing page
was inspected. Record external dependencies that prevent successful execution.

Use 1920×1080 as the primary desktop design target, in the default light and dark themes. Check selected constrained
layouts at 1280px and 390px, keyboard-only operation and 200% zoom. Apply these checks where the layout or interaction
can differ; avoid multiplying every screen by every environment combination.

## Judge the result

- **Purpose and hierarchy:** the title establishes context; related fields and actions are grouped; the primary
  action is clear; secondary detail supports its subject; wide layouts use space without stretching every input.
- **Flow:** navigation preserves useful context; filters and saved definitions are distinguishable; dialogs keep
  their actions reachable; cancellation and reopening behave predictably; success, validation and recovery are clear.
- **Content:** names and meaningful states replace unexplained codes; absent information is truthful; inactive or
  unavailable features are not presented as working actions; labels use the audience's vocabulary.
- **Visual consistency:** shared spacing, typography, surfaces and semantic colors establish reusable patterns;
  status does not rely on color alone; selected, focused, loading and disabled states are distinguishable.
- **Readability:** normal text reaches 4.5:1, qualifying large text 3:1, and meaningful control/state indicators 3:1.
  Measure foregrounds against the actual composited backgrounds, including tinted chips and buttons. Inspect visually
  as well: a passing number does not establish comfortable typography.
- **Input and adaptation:** accessible names explain icon actions; tab order follows reading order; Enter and Escape
  behave as expected; phone and zoom layouts preserve task content and controls.
- **Terminal:** inspect fresh launches and live light/dark changes, including open dialogs, section titles, focus,
  selection and shortcut help. Web CSS and terminal color roles require separate checks.

The contrast thresholds follow W3C's [text contrast](https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html)
and [non-text contrast](https://www.w3.org/WAI/WCAG22/Understanding/non-text-contrast.html) guidance.

## Record, fix and verify

Each finding records an ID, severity, actor/activity step, route, theme/viewport, reproduction, observed impact,
evidence, owning layer, correction and retest result. Distinguish observed failures from static suspicions and design
judgments. P1 blocks work or makes essential information unusable; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement.

Capture and inspect screenshots at important states and compare before/after at the same dimensions and data.
Keep evidence in the chosen review output directory and reference it from the running record. Do not add unattended
screenshot baselines to regression tests. Include browser console/network errors and data outcomes where relevant.

Fix shared widget or theme causes before local repetitions. Preserve routes, saved preferences and public contracts;
record compatibility handling where changes are unavoidable. Demonstrate reusable patterns in Gallery and verify
them again with real application content.

Established client patterns to reuse rather than reinvent:
- **Confirmation:** ask through `moqui.confirmAction(message, opener)` (the in-app dialog in the focus stack, where
  only Confirm approves), never `window.confirm`; a script that can also load without the qvt shell falls back to
  the browser's. A form the user may not submit says so above its disabled fields.
- **History:** popstate compares the address bar with the bar as last pushed or replaced (`rememberHistory`), never
  with a route the menu data derived, so in-page fragment links keep unsent input.
- **Times and empty states:** show service timestamps in the user's zone with the zone named once, keep raw values
  for actions and exports, and give each empty list its reason (nothing recorded, or nothing matching the filters).
- **Focus restore:** find controls by a `data-action`, not by a visible or accessible label that may be localized.

Use existing tests when they already cover the behavior. Focused rule/bug regressions belong in `test/`; complete
activities, responsive/access walks and sweeps belong in `acceptance/`. Follow the testing skill's rendering and
budget limits. Run the relevant acceptance journeys when the review requests them, and the affected component's
acceptance suite when adding or changing an acceptance test. Measured theme and geometry checks of shared components
belong in an offline acceptance client suite (a `suite.js` with no server, such as webroot's `ThemeLayout`), which
`./moq accept` runs with `--offline`.

Finish each bounded batch by rerunning affected activities, running the required regression/lint checks, updating
the running notes, and committing the changes with their notes. Notes identify findings addressed, design decisions,
test commands/results, evidence and remaining gaps. Keep a coverage ledger so later reviewers can continue the work
without repeating completed checks or treating unverified paths as passed.

## Framework verification details

- Copy mutable review configuration; link framework source and installed tools, leave `component/` empty, and let
  `--db fresh` create scratch storage. A separate port is not database isolation. Use one lifecycle owner.
- Keep server/test processes at the same process-visibility level. A sandboxed stale-process check must not be
  allowed to mistake an inaccessible live review process for a dead one.
- Use `MOQUI_RUNTIME` for the isolated review and targeted server specs. Run the complete `./moq test-framework`
  gate without an inherited runtime override (and explicitly bound its workers when sharing a machine): its shell harness and offline assets expect the repository runtime layout.
- After changing client component templates, regenerate the existing precompiled-client golden and inspect the
  diff; after changing bundled QVue screens, regenerate the bundled-screen golden. Do not regenerate unrelated
  outputs. The existing test names are `client_components_are_precompiled` and `bundled_screens_are_precompiled`.
- Cacheable browser assets can survive a reload. Confirm the loaded markup/version, or use a fresh local review
  hostname after regeneration. Do not diagnose stale assets as a product regression.
- Native text input and blur/change matter for editor checks. A locator fill that does not emit the control's
  expected change event is an automation limitation; repeat through normal typing before reporting data loss.

Established patterns: readable semantic foregrounds are independent from brand fills; list dialogs present fields,
sorting and actions in that order; optional scheduling follows saved-find management; entry pages prioritize work
that can be resumed; unsupported platform metrics say Unavailable; errors retain entered data and offer a useful
recovery action. Preserve routes and preference/service contracts while refining these surfaces.


Review screenshots remain local under the ignored `plans/ui-refinement-evidence/` directory. Record evidence filenames and observations in the active review document, but do not commit the image directory.

Further patterns established during the review:

- A save acknowledges a particular content snapshot. Compare it after reactive watchers settle; retain dirty state
  for later edits, and let Close await the latest successful write. Do not discard a local recovery copy until the
  server has the current content. Test deferred responses and failed follow-up saves in the existing component fixture.
- Fragment-only section jumps preserve form state and browser Back/Forward. Use an explicit `m-anchor-section`
  container for targets that need clearance below the fixed Material toolbar; keep existing fragment identifiers.
- Canvas charts need explicit readable text defaults, including legend and axis options. DOM text color alone
  does not recolor pixels already drawn. Preserve caller colors and the existing chart instance on theme changes.
- Abbreviate time-axis ticks without changing underlying labels, tooltips, exports or filter values. Retain date
  and timezone context, including offset changes, so shorter labels stay unambiguous.
- Give script-created native controls explicit component classes and theme-aware text, backgrounds, borders and
  focus indicators. Scope those styles to owned controls; embedded widgets retain their own styling.
- Menu openers are buttons, including those nested in breadcrumbs. Verify Tab order, Enter/Space opening, Escape
  focus return and text/focus contrast in both themes.
- An export's headers and records use the same data columns. Omit action-only columns and preserve empty cells
  when a conditional field has no value; inspect the downloaded file, not just the download event.
- Validate message text, captions and actions together on a notification's actual background. Semantic field-error
  text also needs to contrast with changed-field fills; keep explicit author color choices separate from defaults.
- Compact wizard headers retain visible step names and accessible previous-step controls. Test cancellation and
  focus return in a real mounted dialog and the actual browser before adding custom focus restoration.
- Preserve list filters before row context replaces their field values. Clear fragment-only and historical-version
  parameters explicitly when returning to a whole page or switching records; omitted values can inherit stale state.
- A screen's asynchronous work belongs to that screen instance. Capture its endpoint, ignore stale completions and
  remove registered dialogs when it unmounts so navigation cannot send requests against the destination screen.
- Drag interactions have a keyboard path using the same update operation, with focus restored after movement.
  Demonstrations clearly distinguish temporary visual changes from stored data.

- Confirmations stay in the application's dialog/focus stack. Only explicit approval resumes the original action;
  Cancel, Escape and backdrop dismissal do not submit. Preserve validation, the submitter and modified-link behavior,
  and discard approval if the original control is removed, disabled or changes its meaning while the dialog is open.
- Rich editors expose the visible field name on their actual editable surface and on the unavailable-library fallback.
  Reconfigure names without replacing content or undo history; an optional standalone editor label remains optional.
- Structured query summaries are display-only. Preserve typed values and authored labels; use exact JSON when an
  unfamiliar or ambiguous predicate cannot be summarized faithfully.
- Explicitly distinguish a root-page bookmark from an omitted default route using the existing parameter convention.
  Comparison failures retain current content and a clear recovery action. Normalize text only at the editor boundary
  and diff presentation, without rewriting retained resource history or restore payloads.

- Directional sort controls request the direction they name; clicking descending again does not silently reverse it.
  Match complete field identifiers and retain supported legacy sort prefixes, suffixes and wrappers.
- Diagnostic summaries describe the values actually available. Distinguish hostnames from instance IDs, preserve
  zero counts, and include timestamp precision and timezone appropriate to short events.
- Timing labels sit outside colored bars. Keep concise diagnostic context beside selected spans, with raw data in
  a keyboard-accessible disclosure. SVG charts preserve text size while geometry responds to available width;
  choose default text against the actual composited fill and preserve explicit author colors.
- Section headers reserve room for their title and wrap related actions below it on phones. Collapse/Expand controls
  name the section and expose their state. Layout fixtures load the same fonts and icons as production.
- Group long configuration forms by the decisions users make, separate effective values from overrides, and keep
  Save reachable. Collapsing advanced fields must preserve their values for the eventual submission.
- Keep machine-parsed numeric edit values canonical. Localized grouping belongs in display text; an unchanged
  large threshold must survive reopen and an unrelated save. Preserve invalid submitted values for correction.
- A request acknowledgment describes what the service actually guarantees. Interval/claim skips are not fresh
  evaluations; show the last evaluated time and state. Notifications identify firing, unknown and recovery states
  in their text as well as their tone/icon, without changing the delivery payload contract.
- Explicitly clear inherited record IDs on creation links. A New action reached from a notification or bookmarked
  detail must open a new record; ordinary detail links continue to preserve their selected identity.
- Use exact accessible names for short action words such as Confirm in focused browser tests. A newly named section
  toggle can legitimately contain the same word; substring matching must not activate a different control.
- An unsaved wizard draft is an explicit form/field allowlist scoped to one mounted workflow and record. Keep
  credentials and remote relationship selections out. A successful response acknowledges the submitted snapshot;
  it must not erase newer edits. Finish, scope changes and leaving the workflow discard the local draft.
  A superseded navigation must also release the mounted wizard's pending state, while old completion callbacks
  cannot unlock a newer submission. Exercise this ordering with a deferred-response fixture.
- Format temporal values once, in the user's zone. Keep instants across server/client boundaries and preserve any
  existing display-string contract for other callers. Verify normal, DST-crossing and cleared dates, plus Standard
  and TUI representations when changing shared date handling.
- Use named native buttons for message and recipient disclosures; preserve focus when collapsed and expanded
  presentations replace each other. Toggle actions describe the next action and expose their current state.
- Distinguish hosted delivery from remote synchronization. Account links resolve through the current app mount and
  carry the selected account. Copy, status metadata and available controls should match the actual account kind.
- Responsive field columns specify their phone width explicitly, for example `xs="12" md="8"`; a desktop-only
  width can leave narrow cards at their intrinsic width. Inspect the real phone layout instead of inferring stacking
  from the absence of horizontal overflow.
- Render embedded application views through authorized in-page data transitions and existing components. Keep the
  frame policy intact; an embed must not take over its host page's shortcuts, history or refresh behavior.
  Adapt navigation links to the current app mount, but preserve server transition URLs for JSON data, inspection
  and export requests; the Material wrapper route returns a page, not the transition response.
- Embedded dashboard actions target the owning dashboard, retaining its identity, displayed time range and active
  filters. Verify drill-through, filtering and sharing from the embed as well as from the ordinary dashboard route.
- Associate asynchronous frames with both the selected definition and the latest request. Clear previous frames
  while loading, ignore superseded responses even when cancellation races, and offer recovery when the saved
  selection has changed. Filtering inaccessible playlist items must preserve their original request indices.
- Read-only terminal HTML fields are omitted from submission. Preserve an omitted value in the application's save
  transition by reading the selected record through its authorized service; do not write if that read fails. An
  explicitly posted empty value still clears the field. Keep this handling scoped to the unsupported fields and
  preserve the normal semantics of dates, audience choices and apply-to-all actions.
- Shared shell destinations use the browser mount. The terminal adapts only that local mount at its navigation
  boundary, retaining query values and leaving other mounts and external addresses intact. Exercise launcher,
  search, quick-create and tray destinations with the same shell data the browser receives.
- Radio labels fit within their assigned field width. When an inline group will not fit, wrap each choice with a
  hanging indent and reserve the same rows in layout and drawing. Preserve one focus target and the option values;
  check a neighboring field as well as the next row.
- Selected navigation text uses a readable foreground independently of its brand-colored fill and selection bar.
  Measure translucent fills against their painted ancestor backgrounds. Preserve explicit theme overrides and
  retest the default light/dark cycle plus representative alternative schemes.
- Multiple-choice fields show each selected label while submitting its ID. Inspect the settled field before saving,
  after an unrelated render and after refusal; a hidden native select alone does not prove usable selection feedback.
- Identify quick-create and tray context through the loader's supplied screen context. A loaded QVue component may
  not have the wrapper element that a server-rendered fragment has. Exercise cancel, reopen and destination behavior.
- Message actions need a keyboard entry point and visible focus. Keep their owning toolbar mounted while a menu
  moves focus outside the message. Rich-editor accessible names follow metadata updates without replacing the editor
  or losing the draft, selection or focus.
- Reset links explicitly clear inherited filter parameters and pagination. Distinguish clearing all filters from
  changing one dimension, such as switching voicemail folders while retaining heard/unheard.
- Authored terminal list cells keep their native text and actions. Give every emitted action a document-wide unique
  ID, including empty or repeated authored IDs. Verify that two numbered rows open their own records; readable text
  alone does not establish correct navigation. Clip each native column's text and mouse targets to its allocated pane,
  even when a child list has a larger intrinsic width; restore the parent clip before painting the next column.
- Reactive selections can wrap source objects. Guard asynchronous summaries with a stable record/occurrence key,
  request generation and open state, including failures and repeated requests for the same occurrence.
- A full-width row can leave a popup positioner no room on either side. Anchor at a useful point and constrain
  contents to the actual menu width. Compare client and scroll widths, inspect complete actions, and retest the
  phone dialog branch separately.
- Send mutation context in the POST body while retaining read context in the URL. In shared mounts, keep the owning
  record's identity separate from the action's target; preserve the framework write guard and service authorization.
