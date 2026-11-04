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
review runtime. Use the existing binary for screen, CSS and most JavaScript changes. ScreenQvt generator templates are embedded
in the core binary; rebuild when their source changes so the live server actually uses the new generator.

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

Use existing tests when they already cover the behavior. Focused rule/bug regressions belong in `test/`; complete
activities, responsive/access walks and sweeps belong in `acceptance/`. Follow the testing skill's rendering and
budget limits. Run the relevant acceptance journeys when the review requests them, and the affected component's
acceptance suite when adding or changing an acceptance test.

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

For native change-on-blur editor fields, verify the rendered sibling/model changes after input. If an automation fill helper changes only the DOM value, use native typing and Tab before Save; do not report a product persistence defect from that helper alone.
