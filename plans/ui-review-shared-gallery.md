# MR Moqui — UI review: Shared shell and Gallery

**Status — 2026-11-25.** [SH-B03](#sh-b03--post-cleanup-recheck-2026-11-25) rechecked all seven historical findings. Six passed their live checks. The export link and bytes passed, while in-browser download activation remains unverified in this curl-transfer workflow. The activity ledger records the scope of each walk.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review the common work surface before judging each app: get into the instance, find the right work, keep context
while navigating, use shared controls comfortably, and recover without losing edits. Gallery is the reference
consumer for widget/archetype behavior; confirm a shared correction in a real app as well. This file owns the
common protocol for the companion plans, not their individual workflows.

Sources: the [UI review guide](../docs/dev/UI-Review.md), [archived review](archive/UI-Refinement.md),
[Accounting example](../runtime/components/addon/mr-boss/plan/ui-review-accounting.md),
[webroot assets](../runtime/components/base/webroot/screen/webroot/README.md),
[Gallery README](../runtime/components/dev/example/README.md), [Qvt client](../docs/ui/Qvt-Client.md),
[TUI](../docs/ui/TUI.md), [Terminal](../docs/ui/Terminal.md), [Semantic find](../docs/ui/Semantic-Find.md),
[recent authentication](../docs/security/Recent-Auth.md) and current webroot/Gallery screens and shell client modules.
The BOSS style guide is a reference, not a requirement to load BOSS.

### Ownership and handoffs

| Surface | Primary owner / handoff |
| --- | --- |
| Login, shell menus/preferences, navigation/history, shared controls and floating panels | This file; app plans check their own context and controls |
| Gallery archetypes and shared editor/files/chatter demonstrations | This file; business behavior remains with the consuming app |
| Assist interaction and terminal sessions as a user | This file; AI configuration/runs in [System AI/integrations](ui-review-system-ai-integrations.md), developer changes in [Dev AIDE Workspace](ui-review-dev-aide-workspace.md) |
| Comms tray content and crypto/media | [BasicComms](ui-review-basic-comms.md); this file owns opening, docking, persistence and focus |
| User/grant policy and administration of sessions/devices | [System](ui-review-system.md); this file exercises the affected sign-in/user experience |
| Public call page and report wall | BasicComms and [Dashboards](ui-review-dashboards.md); public-shell navigation/feedback belongs here |

Native desktop/mobile installations are not a separate review matrix. Record their conditional host-specific
surfaces and reuse the owning app's planned branch when a suitable local host is available.

## 2. Review method and refinement criteria

### Common review protocol

This protocol applies to all nine companion plans. The [UI review guide](../docs/dev/UI-Review.md) supplies the
activity/evidence method; [application practices](../docs/dev/Practices-App-Dev.md), the
[building skill](../.agents/skills/building/SKILL.md) and [testing skill](../.agents/skills/testing/SKILL.md) govern later
changes. Preparing or auditing these plans starts no runtime, writes no fixture, executes no activity, runs no
test and grants no permission to contact a real provider or modify application source.

- **Environment:** use the existing suitable binary, a fresh disposable database and copied configuration. Load only
  tracked base/dev components individually; leave `components/addon/` empty. Record build features, component list,
  runtime/data/output locations, free non-default ports and hostnames. Never link a mutable database or workspace.
  Source component links are read-only review inputs; all Dev AIDE edits go to disposable copies/site repositories.
  Use separate hostnames for different browser identities because ports do not isolate cookies. One lifecycle owner
  sequences shared settings, jobs, fixture consumption and cleanup; separate runtimes where those activities conflict.
  Load the named review components, not every directory under `dev`: keep the `demo` lockdown disabled and the
  component `template` unloaded. Demo replaces several Tools routes and changes grants/protocols; its Disabled
  screen is a supporting variant below, not evidence that the full runtime lacks those features. Generated template
  screens are covered only after DEVW-01 creates a disposable component with real names.
- **Fixtures:** use `framework/ClientFixtures` only when a later review explicitly selects that fixture setup.
  It defines `sec.view.only` (view grants to Tools/System) and `sec.none.only` (no such grants); it does not prepare
  every app's specialized roles. Each plan names additional proposed review identities and the actual grant sources.
  Verify loaded memberships and grants before a refusal. ADMIN establishes setup, not least-privilege evidence.
  Keep secrets out of the plans and evidence. Record fixture IDs and before/after state in the first execution batch.
- **State and outputs:** distinguish empty, filtered empty, unavailable and forbidden. Inspect the stored result and
  any exported bytes, not just a success message. Capture mail with a served fixture that reports capture enabled;
  an ordinary server does not become a sink merely because `mail_capture_sends=true` was set. Use local remotes,
  fake media and synthetic providers; unavailable integrations remain blocked. Never treat queued as delivered.
  Record the real clock, zone and load date; prepare date-sensitive rows against them instead of moving the clock.
- **Modes:** primary visual target is Material `/qapps`, measured 1920×1080 in default light/dark. Sample layouts that
  differ at 1280px and 390px, with keyboard-only interaction. Exercise actual TUI operations, including fresh launches
  and live theme switches, and brief Standard `/apps` functionality and offered output/fallback paths. Source or JSON
  renders are supporting inspection. Unsupported browser-only editors/media must explain delegation; do not invent
  parity. No exhaustive users × screens × modes matrix, browser zoom matrix, DST exercise or native-app installation
  matrix is part of these plans. Desktop-only states are identified as conditional branches, not silently passed.
- **Judgment:** assess hierarchy, readable density, one clear next action, context retention, useful names, loading
  feedback, validation, cancel/reopen, focus return, keyboard alternatives and narrow layouts. Use the guide's contrast
  thresholds (4.5:1 ordinary text; 3:1 qualifying large text and meaningful controls). Color alone cannot carry state.
  Capture and inspect important screenshots, console/network errors and outcome evidence; do not add screenshot
  baselines. Keep local evidence under ignored `plans/ui-refinement-evidence/<review>/` and reference relative names.
- **Access:** pair a refused action with the same otherwise-valid action by its grant holder. Test direct helper,
  transition and output access where a UI restriction matters. A validation error, hidden button or masked field is
  not proof of access enforcement. Keep foreign/private records and secrets out of response and output bytes.
- **Two passes:** first walk every selected activity and record outcomes without source fixes. A blocking defect gets
  a named disposable workaround or a blocked branch. Then triage all companion findings together, merge shared causes
  and assign ownership before refinement batches. Fix the owning layer once and retest an appropriate second consumer.
  Restore temporary grants/settings, stop local jobs/listeners and remove fixture connections at the end.

### Verification policy for later execution

These plans authorize neither acceptance runs nor implementation. The current task's repository instructions and
[testing skill](../.agents/skills/testing/SKILL.md) take precedence over broader automatic suite/commit cadence in
historical reviews or the generic guide. During later fixes, run focused regressions for changed behavior; before a
framework-code commit use `./moq test-framework`. There is no addon application work in this set.

Complete journeys, access/phone walks and screen sweeps belong in acceptance, at most one sweep per app per mode;
retain the existing single golden-path regression walk where present. Run acceptance only when requested, or when
adding/changing an acceptance test (that test and its component's acceptance suite). Never add a full regression
screen walk or run `accept-all` merely to finish a review. Reuse existing assertions, respect budgets and use the
existing binary unless a changed embedded generator or Rust implementation requires rebuilding. Lint changed screens
with a report output path, never a screen path as the lint argument. Documentation-only validation does not need a
build, renderer, runtime, lint of screens or application tests.

### Environment, identities and records

- Load webroot, framework, tools, basic-comms, dev-aide and example without addons on a dev instance. Check local
  fonts/icons/editor assets and the terminal transport before interpreting a blank surface.
- Prepare `review.admin` for Gallery (the `EXAMPLE_GALLERY` grant in `ExampleSecurityData.xml`) and a non-Gallery reader;
  verify the actual seed grant at setup. Use `sec.view.only` and `sec.none.only` from ClientFixtures for Tools/System
  navigation boundaries. Prepare dedicated disposable users for password change, second factors and forced expiry;
  do not lock out the lifecycle owner. System's user-policy activity owns their administration.
- Gallery mutates Enumeration, example records, user/group examples, resource versions and files. Create named
  review records rather than alter shared seed rows or the administrator. Keep editor/version/files fixtures separate
  from app plans; use a local Git resource for publish and a local token route for the unmounted PublicLink example.
- Use a local model fixture for Assist and local mail/OTP capture. Device registration, browser notification
  permission, media, semantic search embeddings and host terminals each have a separate prerequisite. A fallback
  presentation can be inspected without claiming its successful external path.

### Area-specific criteria

- A shared action keeps the originating record and app across Back/Forward, dialogs, quick create and delayed work.
  Focus returns to a usable opener; a superseded screen cannot act on its successor's endpoint.
- Themes preserve meaningful selected/focused/disabled/error states in both web and terminal. Preference persistence
  is distinct from temporary theme switching. Check the actually painted background, including chips and dialogs.
- Gallery examples must distinguish ephemeral demonstration changes from persisted records. Demonstrations using
  users, files, messages or destructive actions still need explicit identity and confirmation.
- Terminal/browser-only limitations are stated where the user meets them. A generic render or TUI text placeholder
  is not successful editor, media or keyboard interaction.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Enter and orient | SH-01–03 | Establish identity, navigation and appearance baselines |
| 2. Work with records and forms | SH-04–09 | List → detail/edit; wizard; alternative views; handheld task |
| 3. Content and shell work | SH-10–14, SH-18 | Editor/files → notifications/Assist → personal memory/skills → persistent terminal |
| 4. Outputs and boundaries | SH-15–17 | Print/public states, companion handoffs and remaining variants |

Complete each wave's fixture-producing activity before consuming its record. Finish cross-app retests after the
companion activities that supply their records; do not repeat those whole walks here.

## 3. Screen and surface inventory

This inventory accounts for **70 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### example: `Gallery`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/gallery/Board` | [Board.xml](../runtime/components/dev/example/screen/Gallery/Board.xml) | Companions: [Board.tui.json](../runtime/components/dev/example/screen/Gallery/Board.tui.json), [Board.qvue](../runtime/components/dev/example/screen/Gallery/Board.qvue). Authored text: `html,qvt`, `qvue`, `tui`. Declared modes: `vue,qvue,tui`. | SH-08 |
| `/qapps/gallery/Calendar` | [Calendar.xml](../runtime/components/dev/example/screen/Gallery/Calendar.xml) | Companions: [Calendar.qvue](../runtime/components/dev/example/screen/Gallery/Calendar.qvue), [Calendar.tui.json](../runtime/components/dev/example/screen/Gallery/Calendar.tui.json). Authored text: `html,qvt`, `qvue`, `tui`. Context: `month`. Declared modes: `vue,qvue,tui`. | SH-08 |
| `/qapps/gallery/Chatter` | [Chatter.xml](../runtime/components/dev/example/screen/Gallery/Chatter.xml) | Authored text: `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `recordEntity`, `recordId`, `appRoot`, `defaultTab`, `hide`. | SH-11 |
| `/qapps/gallery/CodeField` | [CodeField.xml](../runtime/components/dev/example/screen/Gallery/CodeField.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `exampleId`. | SH-06 |
| `/qapps/gallery/Contact` | [Contact.xml](../runtime/components/dev/example/screen/Gallery/Contact.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SH-05 |
| `/qapps/gallery/Detail` | [Detail.xml](../runtime/components/dev/example/screen/Gallery/Detail.xml) | Context: `enumId`, `syncStatusId`. | SH-05 |
| `/qapps/gallery/Dialog` | [Dialog.xml](../runtime/components/dev/example/screen/Gallery/Dialog.xml) | Inspect the route and its visible controls through the activity. | SH-04, SH-06 |
| `/qapps/gallery/EditExample` | [EditExample.xml](../runtime/components/dev/example/screen/Gallery/EditExample.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `exampleId`. | SH-04 |
| `/qapps/gallery/Editor` | [Editor.xml](../runtime/components/dev/example/screen/Gallery/Editor.xml) | Authored text: `html,qvt`, `tui`. Context: `wikiSpaceId`, `pagePath`, `versionName`, `versionNameLhs`, `versionNameRhs`. | SH-10 |
| `/qapps/gallery/ExampleDetail` | [ExampleDetail.xml](../runtime/components/dev/example/screen/Gallery/ExampleDetail.xml) | Authored text: `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `exampleId`, `peek`. | SH-04 |
| `/qapps/gallery/Handheld` | [Handheld.xml](../runtime/components/dev/example/screen/Gallery/Handheld.xml) | Inspect the route and its visible controls through the activity. | SH-09 |
| `/qapps/gallery/Landing` | [Landing.xml](../runtime/components/dev/example/screen/Gallery/Landing.xml) | Authored text: `qvt`, `tui`. | SH-04 |
| `/qapps/gallery/List` | [List.xml](../runtime/components/dev/example/screen/Gallery/List.xml) | Inspect the route and its visible controls through the activity. | SH-04 |
| `/qapps/gallery/Message` | [Message.xml](../runtime/components/dev/example/screen/Gallery/Message.xml) | Context: `trusted`. | SH-06 |
| `/qapps/gallery/NavCards` | [NavCards.xml](../runtime/components/dev/example/screen/Gallery/NavCards.xml) | Inspect the route and its visible controls through the activity. | SH-02 |
| `/qapps/gallery/Panel` | [Panel.xml](../runtime/components/dev/example/screen/Gallery/Panel.xml) | Authored text: `qvt`, `tui`. | SH-12 |
| `/qapps/gallery/QuickNote` | [QuickNote.xml](../runtime/components/dev/example/screen/Gallery/QuickNote.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Context: `recordEntity`, `recordId`. | SH-12 |
| `/qapps/gallery/QuickTask` | [QuickTask.xml](../runtime/components/dev/example/screen/Gallery/QuickTask.xml) | Companions: [QuickTask.qvue](../runtime/components/dev/example/screen/Gallery/QuickTask.qvue). Authored text: `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `qvue,tui`. | SH-12 |
| `/qapps/gallery/Record360` | [Record360.xml](../runtime/components/dev/example/screen/Gallery/Record360.xml) | Authored text: `qvt`, `tui`. Context: `userId`. | SH-05 |
| `/qapps/gallery/RecordFiles` | [RecordFiles.xml](../runtime/components/dev/example/screen/Gallery/RecordFiles.xml) | Authored text: `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `recordEntity`, `recordId`, `appRoot`. | SH-11 |
| `/qapps/gallery/Report` | [Report.xml](../runtime/components/dev/example/screen/Gallery/Report.xml) | Authored text: `qvt`, `tui`. | SH-15 |
| `/qapps/gallery/Search` | [Search.xml](../runtime/components/dev/example/screen/Gallery/Search.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SH-04 |
| `/qapps/gallery/Settings` | [Settings.xml](../runtime/components/dev/example/screen/Gallery/Settings.xml) | Inspect the route and its visible controls through the activity. | SH-03, SH-06 |
| `/qapps/gallery/Split` | [Split.xml](../runtime/components/dev/example/screen/Gallery/Split.xml) | Context: `geoId`. | SH-15 |
| `/qapps/gallery/Task` | [Task.xml](../runtime/components/dev/example/screen/Gallery/Task.xml) | Context: `stepOnly`, `lineIndex`. | SH-09 |
| `/qapps/gallery/Views` | [Views.xml](../runtime/components/dev/example/screen/Gallery/Views.xml) | Inspect the route and its visible controls through the activity. | SH-08 |
| `/qapps/gallery/Widgets` | [Widgets.xml](../runtime/components/dev/example/screen/Gallery/Widgets.xml) | Authored text: `qvt`, `tui`. Context: `chip`. | SH-06 |
| `/qapps/gallery/Wizard` | [Wizard.xml](../runtime/components/dev/example/screen/Gallery/Wizard.xml) | Default: `Customer`. | SH-07 |

### example: `Gallery/Wizard`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/gallery/Wizard/Customer` | [Customer.xml](../runtime/components/dev/example/screen/Gallery/Wizard/Customer.xml) | Inspect the route and its visible controls through the activity. | SH-07 |
| `/qapps/gallery/Wizard/Options` | [Options.xml](../runtime/components/dev/example/screen/Gallery/Wizard/Options.xml) | Inspect the route and its visible controls through the activity. | SH-07 |
| `/qapps/gallery/Wizard/Review` | [Review.xml](../runtime/components/dev/example/screen/Gallery/Wizard/Review.xml) | Inspect the route and its visible controls through the activity. | SH-07 |

### example: `roots`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/gallery` | [Gallery.xml](../runtime/components/dev/example/screen/Gallery.xml) | Default: `Landing`. | SH-02, SH-04 |
| `unmounted PublicLink example` | [PublicLink.xml](../runtime/components/dev/example/screen/PublicLink.xml) | No default mount. Configure a local token route as fixture or mark blocked. | SH-11, SH-16 |

### webroot: `roots`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/ (root/default qapps)` | [webroot.xml](../runtime/components/base/webroot/screen/webroot.xml) | Default: `qapps`. Authored text: `html`. | SH-02, SH-17 |

### webroot: `webroot`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/ChangePassword` | [ChangePassword.xml](../runtime/components/base/webroot/screen/webroot/ChangePassword.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SH-01 |
| `/ChatKeys` | [ChatKeys.xml](../runtime/components/base/webroot/screen/webroot/ChatKeys.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-17 |
| `/Device` | [Device.xml](../runtime/components/base/webroot/screen/webroot/Device.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SH-16 |
| `/Login` | [Login.xml](../runtime/components/base/webroot/screen/webroot/Login.xml) | Companions: [Login.html.fancy](../runtime/components/base/webroot/screen/webroot/Login.html.fancy). Authored text: `html`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `initialTab`, `returnTo`. | SH-01 |
| `/PublicShell` | [PublicShell.xml](../runtime/components/base/webroot/screen/webroot/PublicShell.xml) | Authored text: `html`. Hidden/helper entry; enter through its parent action and inspect direct access. | SH-16 |
| `/ReAuth` | [ReAuth.xml](../runtime/components/base/webroot/screen/webroot/ReAuth.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-01, SH-13 |
| `/SecondFactor` | [SecondFactor.xml](../runtime/components/base/webroot/screen/webroot/SecondFactor.xml) | Companions: [SecondFactor.html.fancy](../runtime/components/base/webroot/screen/webroot/SecondFactor.html.fancy). Authored text: `html`. Hidden/helper entry; enter through its parent action and inspect direct access. | SH-01 |
| `/WebPush` | [WebPush.xml](../runtime/components/base/webroot/screen/webroot/WebPush.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-12, SH-17 |
| `/apps` | [apps.xml](../runtime/components/base/webroot/screen/webroot/apps.xml) | Default: `AppList`. Authored text: `html`. | SH-02, SH-03, SH-17 |
| `/assist` | [assist.xml](../runtime/components/base/webroot/screen/webroot/assist.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-13, SH-18 |
| `/echopath` | [echopath.xml](../runtime/components/base/webroot/screen/webroot/echopath.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-17 |
| `/emailOAuth` | [emailOAuth.xml](../runtime/components/base/webroot/screen/webroot/emailOAuth.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-01, SH-17 |
| `/error` | [error.xml](../runtime/components/base/webroot/screen/webroot/error.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. | SH-02, SH-16 |
| `/find` | [find.xml](../runtime/components/base/webroot/screen/webroot/find.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-12, SH-17 |
| `/qapps` | [qapps.xml](../runtime/components/base/webroot/screen/webroot/qapps.xml) | Authored text: `html`. | SH-02, SH-03, SH-12, SH-17 |
| `/rest` | [rest.xml](../runtime/components/base/webroot/screen/webroot/rest.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-17 |
| `/rum` | [rum.xml](../runtime/components/base/webroot/screen/webroot/rum.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-17 |
| `/sso` | [sso.xml](../runtime/components/base/webroot/screen/webroot/sso.xml) | Supporting endpoint/shell surface; inspect via its owner, not as an ordinary app page. | SH-01, SH-16 |

### webroot: `webroot/apps`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/AppList; /apps counterpart` | [AppList.xml](../runtime/components/base/webroot/screen/webroot/apps/AppList.xml) | Authored text: `html`, `qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. | SH-02 |
| `/qapps/ScreenTree; /apps counterpart` | [ScreenTree.xml](../runtime/components/base/webroot/screen/webroot/apps/ScreenTree.xml) | Hidden/helper entry; enter through its parent action and inspect direct access. Includes `component://webroot/screen/webroot/apps/ScreenTree/ScreenTreeNested.xml`. Context: `showAll`. | SH-02 |
| `/qapps/Terminal; /apps counterpart` | [Terminal.xml](../runtime/components/base/webroot/screen/webroot/apps/Terminal.xml) | Companions: [Terminal.qvue](../runtime/components/base/webroot/screen/webroot/apps/Terminal.qvue). Authored text: `html,qvt`, `qvue`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Declared modes: `qvue,tui,html,qvt`. | SH-14 |

### webroot: `webroot/apps/ScreenTree`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/ScreenTree/ScreenTreeNested; /apps counterpart` | [ScreenTreeNested.xml](../runtime/components/base/webroot/screen/webroot/apps/ScreenTree/ScreenTreeNested.xml) | Authored text: `html`. Recursive include/helper; follow ScreenTree rather than assume a standalone user page. Includes `component://webroot/screen/webroot/apps/ScreenTree/ScreenTreeNested.xml`. | SH-02 |

### webroot: `webroot/error`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/error/Forbidden` | [Forbidden.xml](../runtime/components/base/webroot/screen/webroot/error/Forbidden.xml) | Authored text: `html`. | SH-02, SH-17 |
| `/error/InternalError` | [InternalError.xml](../runtime/components/base/webroot/screen/webroot/error/InternalError.xml) | Authored text: `html`. | SH-02 |
| `/error/NotFound` | [NotFound.xml](../runtime/components/base/webroot/screen/webroot/error/NotFound.xml) | Authored text: `html`. | SH-02 |
| `/error/TooMany` | [TooMany.xml](../runtime/components/base/webroot/screen/webroot/error/TooMany.xml) | Authored text: `html`. | SH-02 |
| `/error/Unauthorized` | [Unauthorized.xml](../runtime/components/base/webroot/screen/webroot/error/Unauthorized.xml) | Authored text: `html`. | SH-01, SH-02 |

### Framework email outputs and conditional demo variant

These are rendered outputs or configuration replacements, not new launcher routes. Inspect the captured message
through the activity that produces it, including its HTML and plain-text alternatives, link destination and recipient.
Template registrations come from [MoquiSetupData.xml](../runtime/components/base/framework/data/MoquiSetupData.xml).

| Output / variant | XML source | Boundary / activity |
| --- | --- | --- |
| Password reset message | [PasswordReset.xml](../runtime/components/base/framework/screen/PasswordReset.xml) | SH-01; reset details and return to sign-in, including expired/reused credentials |
| Single-use code message | [SingleUseCode.xml](../runtime/components/base/framework/screen/SingleUseCode.xml) | SH-01; captured code and eligible sign-in path, no secret in review evidence |
| Email factor added notice | [AddedEmailAuthcFactor.xml](../runtime/components/base/framework/screen/AddedEmailAuthcFactor.xml) | SH-01 with SYS-02 factor setup; recipient and factor explanation |
| Email factor code message | [EmailAuthcFactorSent.xml](../runtime/components/base/framework/screen/EmailAuthcFactorSent.xml) | SH-01; verify the loaded template actually resolves this file. The seed registration spells it `EmailAuthcFActorSent.xml`; do not silently correct fixture data and count the unmodified path as reviewed |
| New-device sign-in notice | [AccountSignInEmail.xml](../runtime/components/base/framework/screen/AccountSignInEmail.xml) | SH-01; bounded device/network summary and session link; SYS-04 owns revocation |
| Individual notification | [NotificationEmail.xml](../runtime/components/base/framework/screen/NotificationEmail.xml) | SH-12; subject/body, recipient and originating-record link |
| Notification digest | [NotificationDigestEmail.xml](../runtime/components/base/framework/screen/NotificationDigestEmail.xml) | SH-12; multiple items, summaries and individual links; SYS-11 owns topic defaults |
| Rendered-screen attachment message | [ScreenRenderEmail.xml](../runtime/components/base/framework/screen/ScreenRenderEmail.xml) | SH-15; selected parameters, attachment and Material return URL; no session token in the message |
| Demo replacement screen | [Disabled.xml](../runtime/components/dev/demo/screen/Disabled.xml) | SH-17 supporting inspection of [demo mounts](../runtime/components/dev/demo/MoquiConf.xml) and [policy](../runtime/components/dev/demo/README.md); demo is disabled in the primary review runtime |

Dashboard report and alert email templates have their sole inventory owner in [Dashboards](ui-review-dashboards.md).
The template-registration spelling above is a source reconciliation item, not an observed runtime finding; record
its actual outcome when SH-01 runs, without advancing the pending ledger now.

### Embedded surfaces, dialogs, outputs and reconciliation

| Supporting surface | Inspection / activity |
| --- | --- |
| Account/appearance, launcher, breadcrumbs, navigation drawer, palette, notifications, quick create, Today/tray | SH-02–03, 12, 17; inventory menu choices as actually offered without addons, including no-provider states |
| Shared Find Options, Saved Finds, Columns, row actions, pagination/export, dynamic choices | SH-04, 06, 15; save/update/default/reset and selected value outside a choice page |
| Gallery dialog forms and nested widgets, record headers/More menus, validation and confirmation stack | SH-05–07; Cancel/Escape/backdrop, focus restoration, refused submit and reopen |
| `Board.qvue`, `Calendar.qvue`, `QuickTask.qvue` and their authored terminal counterparts | SH-08, 12; all significant controls, drag/keyboard alternative and honest supported actions |
| Editor history/compare/restore/publish; RecordFiles preview/upload/version/share; Chatter mentions/cards | SH-10–11; inspect actual stored bytes/audience, not only a notification |
| Assist conversations/history/tasks, attachments/dictation, queued messages, follow/context meter, tool consent, canvas, stop/retry and reference viewer | SH-13; configuration and run diagnostics delegated to AI/integrations |
| Assist Memory, My skills, Markdown/front-matter skill editor and Save as skill | SH-18; personal memory/skill ownership here, shared skill administration and lessons handed to AI-05/08 |
| Terminal tabs/splits/floats, reconnect, host-session controls and session attention | SH-14; terminal content remains when changing app; System owns administrative termination |
| PublicShell, Device, WebPush, ReAuth, ChatKeys, sso/emailOAuth callbacks, rum and find endpoints | SH-01, 12–13, 16–17; supporting surfaces, not ordinary launcher screens. Comms owns crypto and email OAuth completion; Monitor owns replay data |
| `rest` and `echopath` | SH-17 supporting navigation/error boundaries only; no generic REST/API audit |
| `example/screen/PublicLink.xml` | SH-11/16: no default mount; use a disposable configured token route, record it, or leave this branch blocked |
| Shell/layout templates and client modules under webroot | Inspect through the above activities; templates are not extra routable pages |
| [BasicTransitions.xml](../runtime/components/base/webroot/template/screen/BasicTransitions.xml), [BasicWidgetTemplates.xml](../runtime/components/base/webroot/template/screen/BasicWidgetTemplates.xml) and [HomeParts.xml](../runtime/components/base/tools/template/HomeParts.xml) | Supporting inspection for SH-02/04/06/15/17: inherited list/form/output helpers and shared home navigation sections; consuming apps retain context checks |

The old review's shell/Gallery recovery, narrow/access and actual TUI gaps remain useful prompts. Its pending browser
zoom work is not imported into this set's agreed viewport matrix. Current `Terminal` functionality and declared
shell panels take precedence over older screenshots. No archived finding is re-opened merely by listing it.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### SH-01 — Sign in, recover and continue to the intended work

- **Actor/start:** A disposable password user, second-factor user, expired-password user and anonymous visitor;
  captured reset/OTP messages and local SSO fixture where available.
- **Objective/steps:** Open a protected deep link → sign in → complete any second factor/password change →
  arrive at the original destination; sign out and follow reset/change-password; inspect initial-admin setup on
  a separate empty runtime.
- **Expected:** The intended destination and signed-in identity are clear; secrets are not redisplayed;
  password/factor changes affect the next login and logout removes access. Initial setup is offered only on its
  eligible runtime.
- **Recovery:** Wrong password/code, expired reset, resend/cancel, missing factor, denied destination and
  unavailable identity provider retain a clear next step without leaking account information.
- **Screens/modes:** Login, SecondFactor, ChangePassword, sso and ReAuth; browser/Standard and the TUI login
  dialog, with keyboard and narrow forms. Account policy administration is SYS-02/04.
- **Message branch:** Capture reset/single-use/factor messages and the new-device notice with their real template
  registrations. Compare HTML/text, address, expiry instructions and destination; follow the eligible session link
  into SYS-04. A template-render failure remains a blocked branch with its source spelling recorded.

### SH-02 — Find work and keep a reliable navigation history

- **Actor/start:** Administrator and view-only/no-app counterparts; at least two permitted apps and a bookmarked
  detail with filters.
- **Objective/steps:** Launcher → app home → menu/submenu → filtered list → detail → breadcrumb → Back/Forward;
  use ScreenTree, a new tab and a deep link; deliberately visit a missing/forbidden route.
- **Expected:** Titles, active menu and address agree; one Back returns to the origin; default routes, query
  values and fragment jumps preserve the intended context.
- **Recovery:** Missing/deleted record, expired session, forbidden app and failed screen load leave the shell
  navigable; a retry does not silently discard an unrelated draft.
- **Screens/modes:** AppList, ScreenTree/Nested, qapps/apps, error pages, Gallery NavCards; desktop/narrow,
  keyboard and TUI navigation.

### SH-03 — Choose an appearance and read every state

- **Actor/start:** User with editable appearance and a prepared locked-policy counterpart; a dense list, form,
  modal and terminal session.
- **Objective/steps:** Account menu → appearance/theme/density choices → inspect selected, disabled, warning and
  error controls → reload/sign in again → switch light/dark with a dialog open.
- **Expected:** Saved preferences persist for the user; policy restrictions explain the unavailable choice;
  text, icons and focus remain readable without relying on color.
- **Recovery:** Unavailable fonts/icons, rejected preference save, unsupported appearance and returning to
  defaults show truthful feedback; temporary selections do not corrupt stored preferences.
- **Screens/modes:** Shell appearance/account controls and Gallery Widgets/Settings; 1920×1080, selected
  1280/390, fresh terminal launches and live switches.

### SH-04 — Find a record, save the view and resume it

- **Actor/start:** Gallery administrator with multiple disposable enumerations/example records and a no-match
  search.
- **Objective/steps:** Landing → List/Search → Find Options → sort/page → edit Columns → save/update/default a
  Saved Find → open a row → return → reset/delete the saved view; export the offered list.
- **Expected:** Labels, displayed columns, row identity, query and exported data agree; selected dynamic values
  remain visible after reopening; reset clears filters and pagination.
- **Recovery:** Invalid filters, empty results, canceled create/delete, expired selected record and unavailable
  choice service retain useful input and recover without applying a different row action.
- **Screens/modes:** Landing, List, Search, Dialog, EditExample, ExampleDetail; visible list dialogs,
  browser/TUI/Standard, long filter dialog at 390 and keyboard reorder.

### SH-05 — Understand and act on a record

- **Actor/start:** Disposable record/user/contact fixtures with multiple states; an authorized actor and a
  restricted counterpart.
- **Objective/steps:** List → Detail → Record360/Contact → related record → return; edit a permitted fact, use
  the primary status action and More, then reopen the record and inspect history.
- **Expected:** Header, state, identity and next action remain coherent across tabs; destructive confirmations
  name the record; links do not inherit another record key.
- **Recovery:** Missing record, state change while a confirmation is open, refused write, canceled action and
  unavailable related record preserve a clear return path.
- **Screens/modes:** Detail, Record360, Contact and their dialogs; inspect offered actions only, with
  browser/TUI/Standard and keyboard menus.

### SH-06 — Complete and recover shared forms

- **Actor/start:** Gallery administrator; disposable inputs for choices, validation, conditional fields,
  code/rich text and settings.
- **Objective/steps:** Widgets → fill all distinct control families → trigger validation/refusal → correct and
  submit → reopen; follow CodeField and Message examples; change a Gallery setting, then reset it.
- **Expected:** Actual editable surfaces have names; required/conditional state is understandable; a refusal
  preserves entered values, selected labels and focus; settings show stored versus default values.
- **Recovery:** Empty/invalid numeric/date values, delayed choices, read-only fields, nested dialogs,
  Cancel/Escape and unavailable rich-editor library do not falsely acknowledge a save.
- **Screens/modes:** Widgets, CodeField, Message, Settings, Dialog; browser keyboard/narrow plus the controls
  supported by TUI and Standard.

### SH-07 — Leave and resume a multi-step task

- **Actor/start:** Fresh disposable wizard customer data; a validation failure prepared for the Options step.
- **Objective/steps:** Wizard/Customer → enter identity → Options → back/edit → Review → cancel/return → finish
  once → inspect the demonstration acknowledgement and cleared session draft. Also enter Review directly with
  an incomplete draft and check that required earlier steps cannot be bypassed.
- **Expected:** Step names and progress stay visible; review reflects the latest values; only the explicit final
  action finishes and clears the draft. This Gallery wizard writes no customer record; its acknowledgement must
  say so. Persistent creation and following a created customer require a real consuming app, not this demo.
- **Recovery:** Invalid step, delayed save, navigation away, stale review and repeated finish preserve the
  appropriate draft and do not create duplicates.
- **Screens/modes:** Wizard and Customer/Options/Review; narrow step header, keyboard movement and TUI/Standard
  supported continuation.

### SH-08 — Move work across boards, calendars and trees

- **Actor/start:** Gallery examples with multiple states/dates and a parent-child work tree.
- **Objective/steps:** Views → list/board/calendar/tree → move a card or event → inspect its original record;
  use Board and Calendar QVue examples, create an event, navigate dates and return.
- **Expected:** Record identity and dates survive view changes; drag and supported keyboard alternatives produce
  the same visible result; demonstrations distinguish persisted changes from temporary ones.
- **Recovery:** No records, no valid target, canceled edit, invalid date/range and failed save restore truthful
  state rather than leaving an optimistic result as saved.
- **Screens/modes:** Views, Board, Calendar and their dialogs/twins; browser pointer/keyboard/390, actual TUI
  alternatives and Standard fallback.

### SH-09 — Finish a scan-driven task

- **Actor/start:** Disposable scan targets for one match, several matches and no match; incomplete task lines.
- **Objective/steps:** Handheld → scan/select a match → confirm put-away; Task → load step → confirm/short/skip
  → inspect the displayed line and remaining list → reach the final-line boundary.
- **Expected:** The chosen record and quantity are visible at action time and acknowledgements distinguish
  demonstration from persistence. Handheld writes no stock movement. Task Confirm reloads the same line;
  Short/Skip advance its URL index, clamped to the seventh line. There is no persisted completion or completed-work
  screen: record that limitation explicitly and review whether the visible explanation makes it understandable.
- **Recovery:** Duplicate scan, wrong code, invalid quantity, lost focus, canceled chooser and failed
  confirmation leave the next safe action clear.
- **Screens/modes:** Handheld and Task, primarily 390px with keyboard/scanner input; desktop and actual
  supported terminal task controls.

### SH-10 — Edit content and inspect its retained versions

- **Actor/start:** Disposable wiki/resource page with several versions and a separate local Git-backed space.
- **Objective/steps:** Editor → change content → save → edit again → inspect history/compare → restore a prior
  version → publish where supported → reopen from another session.
- **Expected:** A save acknowledges its submitted snapshot; dirty state for later edits survives; historical
  comparison and restore identify exactly which content is selected.
- **Recovery:** Rejected/stale save, newer disk version, navigation while saving, failed diff and unavailable
  editor keep recoverable content and offer an explicit choice.
- **Screens/modes:** Editor, history/compare/publish dialogs and rich/code fallback; browser keyboard/narrow and
  truthful terminal/Standard support.

### SH-11 — Attach evidence and discuss it in context

- **Actor/start:** Two disposable users with different file/message rights, a record attachment and versioned
  resource; local token route only if configured.
- **Objective/steps:** RecordFiles → upload/preview/download → replace/inspect version → share; Chatter →
  mention/reply/open record card → follow notification → return to the same record.
- **Expected:** Bytes and version names match what was selected; only the intended audience reads the
  file/message; related-record links retain context.
- **Recovery:** Failed/oversized upload, revoked share, deleted attachment, canceled restore and inaccessible
  mention do not expose content or mislabel success.
- **Screens/modes:** RecordFiles, Chatter and conditional PublicLink; shared picker/viewer dialogs, TUI
  delegation and actual downloaded bytes. BasicComms owns its messaging workflows.

### SH-12 — Use shell shortcuts without losing the task

- **Actor/start:** User with permitted palette/quick-create/tray providers; a current app record and an unsaved
  form.
- **Objective/steps:** Open palette → search/select → open Today/notifications → act/return; open
  QuickNote/QuickTask/Panel quick create → cancel/reopen/save; dock/undock a tray panel and change app.
- **Expected:** Actions use the current declared context and opener; creating a record does not reuse a prior
  ID; closing/docking preserves the correct draft and returns focus.
- **Recovery:** No providers, inaccessible destination, removed opener, failed quick save and stale async
  response leave the shell usable and do not act on a newly selected record.
- **Screens/modes:** Gallery Panel, QuickNote, QuickTask; palette, tray, notifications, shell providers and
  WebPush permission state. Comms panel content is COM-07/12.
- **Notification branch:** Use SYS-11's fixture topic to receive one notification and a multi-item digest through
  local capture. Read both message alternatives and follow each offered link as its recipient; a removed/forbidden
  destination must not expose the original record. Compare unread/read state in the shell after the return.

### SH-13 — Ask for help, inspect a proposed action and recover

- **Pane/context branches:** Open History and Tasks, continue a waiting question/handoff in its conversation and
  stop a running fixture. Queue another message during a run, edit/remove it, and retry a failed send without a
  duplicate turn. Attach/remove a local file and, with a local transcription fixture, dictate/cancel; denial must
  preserve typed input. In two tabs switch Follow here, navigate while a response is pending, inspect the context
  meter and open/hide/reopen an offered canvas. Stale navigation/fill events must not act on a newer page or tab.
- **Actor/start:** Local model/tool fixture with a harmless read and a disposable proposed write; user with the
  required Assist access.
- **Objective/steps:** Open Assist on a record → inspect attached context → ask/read a response and references →
  inspect tool proposal → reject then approve a fresh harmless proposal → stop/retry → reopen the conversation.
- **Expected:** Context and tool intent are intelligible; only the approved operation runs; run state, response
  and cancellation agree; trusted references open without losing the conversation.
- **Recovery:** Provider unavailable, interrupted stream, denied tool, stale approval, missing attachment and
  retry keep a clear distinction between attempted and completed work.
- **Screens/modes:** Assist drawer, consent/reference/attachment dialogs and shell floats. AI-03 owns diagnostic
  run/approval details; DEVW-12 owns developer change-set acceptance.

### SH-14 — Keep a terminal task alive across navigation

- **Actor/start:** Disposable terminal sessions; ordinary user and TERMINAL_HOST holder on a dev instance, never
  a real working shell directory.
- **Objective/steps:** Open Terminal → run a harmless moqui command → create tabs/splits → float/dock → change
  app → reconnect → attach again → end the selected session; follow an attention event.
- **Expected:** Session identity, running state and scrollback remain attached to the right tab; resizing/focus
  work; ending one session leaves the others usable.
- **Recovery:** Transport loss, ended session, denied host shell, failed program and closing a pane versus
  terminating a process have distinct feedback.
- **Screens/modes:** Terminal XML/QVue, xterm, floats and shell attention; keyboard, narrow pane/dialog, theme
  switches; SYS-18 administers the same disposable session.

### SH-15 — Read, compare and export without losing the selection

- **Actor/start:** Gallery report and split-view fixtures with multiple rows/pages and a printable result.
- **Objective/steps:** Report → choose parameters → inspect chart/table → print/export where offered; Split →
  choose record/related row → edit → return to the selected master.
- **Expected:** Displayed range, row labels and output headers agree; action-only columns do not leak into
  exports; print content is legible and preserves meaningful identity.
- **Recovery:** No data, invalid range, blocked download, unavailable PDF renderer and failed detail load are
  recorded separately; no download event alone counts as inspected bytes.
- **Screens/modes:** Report, Split, generic list export/print and shared viewers; actual output files, narrow
  split layout, TUI/Standard.
- **Email-output branch:** Where the shared screen-email action is offered, send only to capture and inspect the
  ScreenRenderEmail body, selected attachment, parameter summary and link back to `/qapps`. Separate a rendered
  attachment from the later live page, and check that private/session parameters are not disclosed.

### SH-16 — Use a bounded public or shared-device entry

- **Actor/start:** Disposable registration code/token, an anonymous browser and a prepared receiving screen; no
  addon public site.
- **Objective/steps:** Open Device → enter code → continue to its permitted display; inspect PublicShell via a
  local example token route or the BasicComms guest entry → follow allowed navigation → expire/revoke and
  revisit.
- **Expected:** Public views show only their intended content with no staff shell leak; invalid/expired codes
  offer understandable recovery; device identity remains distinguishable from a person.
- **Recovery:** Wrong code/token, already-used registration, revoked access, unavailable host capability and
  failed public request leave no misleading signed-in state.
- **Screens/modes:** Device, PublicShell, public error states; DASH-11 owns wall playback and COM-13 owns guest
  media. No unconfigured PublicLink success is claimed.

### SH-17 — Reconcile cross-app shell behavior and remaining modes

- **Actor/start:** Companion reviews have prepared one permitted handoff each; user also has a restricted
  destination.
- **Objective/steps:** Follow System → Monitor → Dashboards, Tools → Dev AIDE and shell → BasicComms → Back;
  repeat representative launcher/shortcut paths in TUI and Standard; inspect remaining inventory helpers through
  their owner.
- **Expected:** History, app identity, selected record and parameters remain correct; a shared widget fix has a
  second real consumer; unavailable destinations explain their boundary.
- **Recovery:** Permission loss, failed loading, unavailable optional features and direct helper requests
  preserve context and do not expose data.
- **Screens/modes:** qapps/apps roots, error handling, find/rum/WebPush/ReAuth/ChatKeys support, rest/echopath
  boundary inspection; no duplicate full app walks.

### SH-18 — Maintain personal Assist memory and reusable skills

- **Actor/start:** Ordinary AgentUse user, AgentSkillAuthor holder and non-holder; harmless completed run,
  personal memory/lesson fixtures and a separate administrator for the AI-05 handoff.
- **Objective/steps:** Assist → Memory → add/edit a short memory with expiry → cancel another edit → switch
  always versus Only for this conversation → reopen another conversation → Forget; inspect/delete an eligible
  proposed lesson. Completed run → Save as skill → edit Markdown/front matter → validate/save → My skills →
  reopen/revise → Share for review → administrator inspects the site copy in AI-05 → retire the personal skill.
- **Expected:** Turning remembering off stops new automatic memories; the UI explains that existing memories
  may still be used until forgotten. Personal, conversation and shared-review scope are distinct; sharing for
  review does not silently activate a global skill. Stored version/status and next retrieval agree.
- **Recovery:** Invalid expiry/front matter, canceled draft/back navigation, denied authoring, failed save/share,
  expired memory and another user's skill ID leave recoverable input and no foreign content. Restore preferences
  and retire/remove only the review's fixture records.
- **Screens/modes:** Assist Memory/My skills/editor and their `/assist` helpers; browser light/dark, keyboard and
  narrow panes. Record actual terminal availability without assuming drawer parity. AI-05 owns administrative
  skill controls and AI-08 owns global lesson review.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| webroot / server | [LoginSetup](../runtime/components/base/webroot/test/LoginSetup.spec.js), [QvtConfig](../runtime/components/base/webroot/test/QvtConfig.spec.js) | Initial-admin setup and qvt configuration rules; not an interactive authentication walk. |
| webroot / client | [QvtKeys](../runtime/components/base/webroot/test/client/QvtKeys.spec.js), [Keys](../runtime/components/base/webroot/test/client/Keys.spec.js), [WebrootVue.qvt.shell](../runtime/components/base/webroot/test/client/WebrootVue.qvt.shell.spec.js), [WebrootVue.qvt.layout](../runtime/components/base/webroot/test/client/WebrootVue.qvt.layout.spec.js), [WebrootVue.qvt.records](../runtime/components/base/webroot/test/client/WebrootVue.qvt.records.spec.js), [WebrootVue.qvt.static](../runtime/components/base/webroot/test/client/WebrootVue.qvt.static.spec.js), [WebrootVue.qvt.views](../runtime/components/base/webroot/test/client/WebrootVue.qvt.views.spec.js) | Offline shell/widget behavior and navigation; no live app workflow evidence. |
| webroot / client | [WebrootAssist](../runtime/components/base/webroot/test/client/WebrootAssist.spec.js), [WebrootTray](../runtime/components/base/webroot/test/client/WebrootTray.spec.js), [WebrootTerminal](../runtime/components/base/webroot/test/client/WebrootTerminal.spec.js), [WebrootFloats](../runtime/components/base/webroot/test/client/WebrootFloats.spec.js), [MoquiFind](../runtime/components/base/webroot/test/client/MoquiFind.spec.js), [moqui-html](../runtime/components/base/webroot/test/client/moqui-html.spec.js) | Offline component/controller behavior; permissions, server effects and full interaction still need walks. |
| webroot / acceptance client | [ThemeLayout](../runtime/components/base/webroot/test/acceptance/client/ThemeLayout.spec.js), [TerminalWalk](../runtime/components/base/webroot/test/acceptance/client/TerminalWalk.spec.js) | Theme/geometry and terminal journeys; inspect their selected viewports and host requirements. |
| example / acceptance | [Gallery](../runtime/components/dev/example/test/acceptance/Gallery.spec.js) | Gallery render coverage, not user completion. |
| example / acceptance client | [GallerySmoke](../runtime/components/dev/example/test/acceptance/client/GallerySmoke.spec.js) | Browser loading/console smoke; does not finish every example task. |
| example / client | [HandheldScan](../runtime/components/dev/example/test/client/HandheldScan.spec.js) | Existing scan interaction fixture; reuse, do not duplicate it as a new regression sweep. |
| framework / server | [ClientFixtures](../runtime/components/base/framework/test/ClientFixtures.spec.js), [RecentAuth](../runtime/components/base/framework/test/RecentAuth.spec.js), [ShellQuickCreate](../runtime/components/base/framework/test/ShellQuickCreate.spec.js), [AdminNewDevice](../runtime/components/base/framework/test/AdminNewDevice.spec.js) | Fixture users, recent-auth and shell/device rules; setup is not review evidence. |
| framework / client | [SecurityAuthn](../runtime/components/base/framework/test/client/SecurityAuthn.spec.js) | Existing HTTP authentication checks; not all visible login/recovery states. |

Future acceptance candidates: **SharedNavigationWalk** (SH-01–03, 12, 16–17), **GalleryActivitiesWalk**
(SH-04–11, 15) and **AssistInteractionWalk** (SH-13/18); extend **TerminalWalk** for uncovered SH-14 branches.
WebrootAssist's existing offline page fixtures cover queued/follow/context, memory/skill and canvas controller
behavior, including its merged canvas and Constitution fixtures; they do not complete the local-provider journeys.
Extend Gallery/GallerySmoke only for a missing supporting surface, not another full sweep. Read their assertions and
registrations before placing a new scenario; ThemeLayout owns shared theme/geometry automation.

Follow the [verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). Proposed
walk names above describe future acceptance work, not tests created or authorized to run by this document. Keep
focused bug/rule checks in regression and reuse existing sweeps/walks instead of adding duplicates.

## 6. Coverage, findings and running record

### Activity coverage ledger

Statuses: **pending**, **inspected**, **exercised** (pass/fail/partial named separately), **blocked**, **deferred**
(with reason). Existing tests and the archived review do not advance these statuses. Every branch is pending until
execution records it; “exercised” never implies that all modes and recovery paths passed.

| Activity | Status | Outcome / remaining branches | Evidence / findings |
| --- | --- | --- | --- |
| SH-01 — Sign in, recover and continue to the intended work | exercised — partial | Protected Gallery deep link, wrong password/retry and protected-route refusal after logout passed in browser. Recovery mail, MFA, expired password, SSO, initial setup, ReAuth, narrow login and TUI remain pending. | SH-B01; `20-signed-out-protected-route.txt` |
| SH-02 — Find work and keep a reliable navigation history | exercised — partial | Launcher, desktop/narrow menus, filtered list/detail and Back/Forward exercised. SH-B03 confirmed Site Map navigation in both shells. Restricted users, new tab, missing/deleted routes, expired session and TUI remain pending. | SH-B01; SH-UI-004/005; `12-list-back.txt`; see SH-B03 for the current recheck. |
| SH-03 — Choose an appearance and read every state | exercised — partial | Light/dark, reload persistence, live Appearance dialog switch and compact/comfortable exercised. SH-B03 confirmed complete Compact Task text in both themes at 390. Locked policy, full contrast measurements, other appearance controls, save failure and terminal branches pending. | SH-B01; SH-UI-003; screenshots `01`–`03`, `10`–`11`; see SH-B03 for the current recheck. |
| SH-04 — Find a record, save the view and resume it | exercised — partial | Landing count → filtered List → Detail → Back, Find Options/Escape, no-match search and reset exercised. SH-B02 exercised Saved Find create/default/update/reload/clear/delete-cancel/delete, keyboard Columns reorder/hide/save/reload/reset and descending sort/page. SH-B03 confirmed exported IDs, State and sort order; direct browser download activation remains unverified this round. Edit dialogs, invalid filters, expired choices, 390 and other modes pending. | SH-B01/B02; SH-UI-006/007; batch-02 evidence `01`–`05`; SH-B03 |
| SH-05 — Understand and act on a record | inspected — partial | Opened STRT_APPEARANCE_POLICY Detail and checked identity/context at 1280. Actions, editing, related records, 360/Contact, history, refusal and all other modes pending. | SH-B01; `13-detail-1280.jpg` |
| SH-06 — Complete and recover shared forms | exercised — partial | Widgets Equipment required-Serial refusal and corrected demo submit, Part conditional field change, server-search selection and Markdown preview exercised. All other control families, settings persistence, nested dialogs, cancellation/refusal, dark/narrow/TUI/Standard remain pending. | SH-B02; evidence `06`–`07` |
| SH-07 — Leave and resume a multi-step task | exercised — partial | Empty-name validation, Back draft retention, normal finish/reset and skipped-Options deep link exercised; SH-B03 confirmed refusal with the incomplete draft and successful recovery. Cancel/navigation-away, invalid Options, stale/repeated finish, dark/narrow and other modes pending. | SH-B01; SH-UI-001; evidence `04`–`06`; see SH-B03 for the current recheck. |
| SH-08 — Move work across boards, calendars and trees | exercised — partial | Calendar month rendering, quick-create demo, invalid month and Today recovery, 390 layout exercised. Views/Board/tree, persistence, drag/keyboard moves, date-range validation, day/week fallback and other modes pending. | SH-B01; SH-UI-002; `07-calendar-invalid-month.jpg`, `08-calendar-390.jpg`; see SH-B03 for the current recheck. |
| SH-09 — Finish a scan-driven task | exercised — partial | At 390: unmatched scan, ambiguous chooser cancellation, single match, put-away confirmation, Task Enter-confirm and Skip through final boundary exercised. Choosing among matches, GS1, duplicate/error/quantity branches, Short, dark, desktop and TUI pending. No business persistence exists in these demos. | SH-B01; SH-UI-003; evidence `09`–`11`; see SH-B03 for the current recheck. |
| SH-10 — Edit content and inspect its retained versions | pending | All steps, modes and recovery branches pending. | — |
| SH-11 — Attach evidence and discuss it in context | pending | All steps, modes and recovery branches pending. | — |
| SH-12 — Use shell shortcuts without losing the task | pending | All steps, modes and recovery branches pending. | — |
| SH-13 — Ask for help, inspect a proposed action and recover | pending | All steps, modes and recovery branches pending. | — |
| SH-14 — Keep a terminal task alive across navigation | pending | All steps, modes and recovery branches pending. | — |
| SH-15 — Read, compare and export without losing the selection | pending | All steps, modes and recovery branches pending. | — |
| SH-16 — Use a bounded public or shared-device entry | pending | All steps, modes and recovery branches pending. | — |
| SH-17 — Reconcile cross-app shell behavior and remaining modes | exercised — partial | Standard launcher and Site Map checks. SH-B03 confirmed the visible Material → Standard switch and Show All in both shells. Cross-app consumers, remaining helpers and TUI pending. | SH-B01; SH-UI-004/005; evidence `16`–`19`; see SH-B03 for the current recheck. |
| SH-18 — Maintain personal Assist memory and reusable skills | pending | All steps, modes and recovery branches pending. | — |

### Findings register

Use `SH-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| SH-UI-001 / P2 | Observed; john.doe; SH-07 incomplete draft; `/qapps/gallery/Wizard/Review`; Material light, compact, 1920×1080 | Save Customer, return from Options without submitting it, open Review directly, then Create customer. Country/Currency are empty, but success is reported and the draft is cleared. The demo bypasses its required Options validation. This is a wizard-validation defect, not evidence of a persisted customer or authorization bypass. | `05-wizard-review-incomplete.jpg`, `06-wizard-skipped-options-result.jpg` (cleared draft), `review-observations.md` (observed acknowledgement); Gallery [Review.xml](../runtime/components/dev/example/screen/Gallery/Wizard/Review.xml) final validation versus [Options.xml](../runtime/components/dev/example/screen/Gallery/Wizard/Options.xml). | **Fixed and checked 2026-11-22.** Create customer checks the saved country before clearing the draft. GalleryInputs passed the incomplete, recovered, and already-finished cases with currency omitted. **2026-11-25:** See SH-B03 for the live recheck. |
| SH-UI-002 / P2 | Observed; john.doe; SH-08 invalid month; `/qapps/gallery/Calendar?month=2026-13`; Material light, compact, 1920×1080 | Open the invalid month bookmark. Header says January 2027 and 11 events, but only one empty week (Dec 27–Jan 2) appears; no validation explains the mismatch. Today restores November's full grid and 13 events. | `07-calendar-invalid-month.{jpg,txt}`; Gallery [Calendar.qvue](../runtime/components/dev/example/screen/Gallery/Calendar.qvue) month parsing/grid, [Calendar.xml](../runtime/components/dev/example/screen/Gallery/Calendar.xml) event parameter validation. | **Fixed and checked 2026-11-22.** Chrome opened 2026-00, 2026-13, and malformed bookmarks with a visible fallback notice and a full November grid. An omitted month opened normally. December advanced to January 2027. GalleryInputs checked the month of the returned events. **2026-11-25:** See SH-B03 for the live recheck. |
| SH-UI-003 / P2 | Observed; john.doe; SH-03/09; `/qapps/gallery/Task?lineIndex=7`; Material light, 390×844, compact | Set Compact, open Task, Skip through the final line. Large Location/Product values are vertically clipped inside read-only fields with internal scrollbars. Product textarea has 24px client height, 39px scroll height and 19.2px font; the bin/product cannot be read at a glance. Comfortable plus reload gives 30px client/scroll height and readable values. | `10-task-final-line-390.jpg`, `11-task-comfortable-390.jpg`; [Task.xml](../runtime/components/dev/example/screen/Gallery/Task.xml) large display styles; shared display/density sizing is the candidate owning layer, to confirm during triage. | **Fixed and checked 2026-11-22.** Chrome at 390×844, Compact, lineIndex=7: both Location and Product measured 24px client / 39px scroll height before the change and 39/39 afterward. Wizard Review’s read-only fields measured 32/32. **2026-11-25:** See SH-B03 for the live recheck. |
| SH-UI-004 / P2 | Observed; john.doe; SH-02/17; `/qapps/ScreenTree` and `/apps/ScreenTree`; light, comfortable, 1280×900 | Click footer Site Map. Both Material and Standard replace the tree with a raw render error: `not a function`, at `currentScreenDef = sri.getRootScreenDef();`. Site Map and its nested walk are unavailable. Launcher/Home remain usable. | `16-site-map-error-1280.jpg`, `16-site-map-error.txt`, `19-site-map-standard.txt`; [ScreenTree.xml](../runtime/components/base/webroot/screen/webroot/apps/ScreenTree.xml) line 22 and screen-script API integration. | **Fixed and checked 2026-11-22.** Site Map uses the permission-aware subscreen menu. Chrome counted 267 normal / 347 Show All links in each shell for john.doe. The ordinary Basic Comms fixture user saw 25/62 in each, including Chat, with System and Tools excluded throughout. **2026-11-25:** See SH-B03 for the live recheck. |
| SH-UI-005 / P2 | Observed; john.doe; SH-02/17; `/qapps/AppList`; Material light, comfortable, 1280×900 | Click Standard in the launcher. Its href is `/apps/AppList`, but the observed browser URL briefly becomes `/qapps/AppList/apps/AppList`, then returns to `/qapps/AppList`, with Material still selected. Direct `/apps/AppList` successfully opens Standard. The interface switch is broken through its visible entry path. | `17-standard-link-result.{json,jpg}` (settled Material page), `review-observations.md` (transient URL), `18-standard-direct.jpg`; [AppList.xml](../runtime/components/base/webroot/screen/webroot/apps/AppList.xml) mode link and shared client navigation. Precise cause remains for triage. | **Fixed and checked 2026-11-22.** Chrome clicked Standard in Material App List and settled on /apps/AppList. **2026-11-25:** See SH-B03 for the live recheck. |
| SH-UI-006 / P2 | Observed; john.doe; SH-04 List export; Material light/comfortable, 1920×1080 | Sort ID descending, go to page 2, Export → CSV. The shell navigates to `/qapps/gallery/List?...renderMode=csv` and displays raw CSV inside the Material page instead of producing the requested download. Back restores the list. | Batch-02 `05-csv-action.{jpg,txt,json}`; shared [ScreenQvt.js](../runtime/components/base/framework/template/screen/ScreenQvt.js) export anchor and [WebrootVue.qvt.js](../runtime/components/base/webroot/screen/webroot/js/WebrootVue.qvt.js) navigation interception. Supporting inspection finds the `/apps` link intercepted without excluding output modes; source notes retain exact lines. Canonical `/apps/gallery/List` downloaded with curl separately. | **Fixed and checked 2026-11-22.** Chrome downloaded EnumList.csv while remaining on page two with descending ID order. A second export retained the optionIndicator=Y filter and contained the three Flagged rows. XLS downloaded EnumList.xlsx from the same filtered list. **2026-11-25:** See SH-B03 for the transfer and browser boundaries. |
| SH-UI-007 / P2 | Observed output; john.doe; SH-04 CSV contents; canonical `/apps/gallery/List` with `renderMode=csv`, `pageNoLimit=true`, descending ID | Curl download returns 538 data rows / 33,196 bytes, but every ID and State cell is blank despite populated IDs in the visible list. Description, Type and Changed are populated. Exported records lose their identity; ID sort correctness cannot be verified from these blank cells. | Batch-02 `EnumList-apps.csv`, `EnumList-apps.summary.json`, `export-source-notes.md`; shared [ScreenCsv.js](../runtime/components/base/framework/template/screen/ScreenCsv.js) emits link text only for explicit `anchor`/`hidden-form-link`, while Gallery [List.xml](../runtime/components/dev/example/screen/Gallery/List.xml) ID has the default link type and State uses `anchor-button`. | **Fixed and checked 2026-11-22.** The downloaded CSV contained 677 rows with 677 populated IDs in descending order, 677 State values and empty Row Actions cells. The filtered plain-text output also contained UgtCustomTeam and its Flagged state. **2026-11-25:** See SH-B03 for the transfer and browser boundaries. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

### SH-B01 — Initial shell and Gallery review, 2026-11-16

**Scope and phase.** First bounded review batch only: primary sign-in/navigation/appearance, a list/detail journey,
Wizard, standalone Calendar, Handheld and Task, then brief Standard entry. No application or API fixes, triage,
regression/acceptance runs or builds. Supporting subagents prepared the runtime and inspected Gallery source;
only the browser observations below advance activity coverage. All five priorities remain provisional until
consolidated findings triage after the review pass.

**Environment and fixtures.** Source commit `b2295f71b6ea2c62ff17609997c1a80f51300c2e`; existing
`target/release/moqui`, reporting `moqui 0.1.0 maximal`, SHA-256
`f37fab7e583709f3b133f71b04241f73fcb137c6cc29158c107768163e2c10f1`. Fresh embedded PostgreSQL runtime
`/tmp/mr-shared-gallery-review-o3w4sod2`, copied `conf/UiReviewConf.xml`, individual links for framework, tools,
webroot, basic-comms, dev-aide and example; addon/test directories empty and demo/template unloaded. No mutable
source workspace/database linked. HTTP `127.0.0.1:18871`, browser hostname `localhost`; final cluster listener
`127.0.0.1:58900`, other protocol listeners and scheduled jobs disabled. Startup initially exposed the default
cluster bind; setup corrected it to loopback and restarted the same disposable database before the workflow walks.
One broad demo identity only: `john.doe`, `EX_JOHN_DOE`, ADMIN + ADMIN_ADV. This establishes no least-privilege result.
No credentials are recorded here. No mail capture, second-factor/SSO, provider or local-model fixture was prepared.

Runtime date 2026-11-16, review approximately 22:43–22:50 UTC / 16:43–16:50 America/Chicago; fresh seed timestamps
showed 16:43. Current source files override embedded copies. Startup reported four changed embedded copies
(ServerEntities, InstanceServices and the screen/form schemas), plus an unregistered Docker service; no rebuild
was attempted, and this batch is evidence for the recorded binary/source combination. Runtime metadata, copied
configuration and listener excerpt are retained with the evidence.

**Modes and evidence.** In-app browser, Material default color scheme: 1920×1080 light/dark Landing and appearance;
1920×1080 light/compact Wizard and Calendar; 390×844 light Calendar/Handheld/Task, compact then comfortable;
1280×900 light/comfortable launcher/List/Detail/Site Map and Standard launcher/Site Map. The browser viewport
was explicitly set; at 390 the scrolling document reported 375×844 content area, with no wider body overflow.
A transient capture immediately after resizing was replaced after reload; only the settled narrow capture counts.
Keyboard checks were limited to Enter on scan/task fields and Escape in Find Options, not a complete keyboard walk.
No TUI was launched and no quantitative contrast pass was completed.

Evidence filenames below are relative to ignored `plans/ui-refinement-evidence/shared-gallery/batch-01/`.
Screenshots and supporting AX text/JSON are local artifacts, not committed baselines. Reproduction steps in the
findings register remain usable without those files.

| Branch | Observed actions and outcome | Evidence / remaining boundary |
| --- | --- | --- |
| SH-01 sign-in/out | Anonymous protected Gallery URL redirected to Login. Wrong password gave a generic error and cleared the password; correct retry returned to Gallery. At batch end logout returned to Login and reopening protected Gallery still required login. | Initial observations in this record; final `20-signed-out-protected-route.txt`. No MFA/reset/change-password/recent-auth evidence. |
| SH-02/03 shell and appearance | Gallery defaulted to Landing. Account → Dark changed the page and survived reload. Appearance showed Dark selected; changing to Light and Compact with the dialog open worked. Compact remained selected after later reloads; Comfortable was restored and verified through the Task comparison. Desktop app menu, narrow drawer and launcher offered Gallery and the five app families. | `01-gallery-light.jpg`, `02-gallery-dark.jpg`, `03-appearance-light-compact.jpg`. Fonts, policy locks and complete focus/contrast checks still pending. |
| SH-02/04/05 list journey | Launcher → Gallery → Screen Theme Resource Type count (34) → List (`1-20 / 34`) → STRT_APPEARANCE_POLICY Detail. Back restored the filtered URL, label and count; Forward reopened the same detail. Find Options retained the selected type; Escape closed it and focus returned to Find Options. Description `SH-BATCH-01-NO-MATCH` showed No results found with visible filter chips. Reset removed filters and returned `1-20 / 538`. | `12-list-back.txt`, `13-detail-1280.jpg`, `14-list-empty.txt` / `14-list-empty-1280.jpg`, `15-list-reset.txt`. No Saved Find/column preference created and no enumeration changed. Detail mutations/related parts not reviewed. |
| SH-07 wizard | Clearing Customer name and Next showed inline Enter the customer name without advancing. `SH Batch 01 Customer` saved to Options; Back retained it. Direct Review skipped Options, showed empty country/currency, then reported success and reset the draft (SH-UI-001). A separate `SH Batch 01 Complete` draft followed every step, displayed USA/USD/newsletter choices, then finished/reset with the explicit nothing-is-written notice. | `review-observations.md`, `05-wizard-review-incomplete.jpg`, `06-wizard-skipped-options-result.jpg`. The `04`/`06` AX text files contain only unchanged-tree notices; they do not independently capture validation/success text. Default synthetic contact details only. Customer persistence is intentionally unsupported. |
| SH-08 calendar | November displayed a full month with 13 demo events. Selecting Nov 16 opened a dialog with 09:00–10:00; Create with blank title reported a demonstration event and saved nothing. Invalid month `2026-13` produced SH-UI-002; Today recovered. At 390 the grid kept seven columns, one-letter headings and event dots whose accessibility labels described events. | `07-calendar-invalid-month.{jpg,txt}`, `08-calendar-390.jpg`. No real event created; empty-title behavior is a demo observation, not a separately asserted defect. Full keyboard event access/date-range validation remains pending. |
| SH-09 handheld | `NOPE` returned No match with supported types and retained focus/input. `DUP-1` opened a two-match asset/lot chooser; Cancel returned focus to Code. `A-100` resolved Red widget, large and acknowledged a non-writing scan. Bin `B-01-07` required Confirm, then showed the resolved bin and explicit non-writing put-away acknowledgement, cleared the entry and retained focus. | `09-handheld-confirm-390.{jpg,txt}`. No stock data changed. Selecting one ambiguous match, GS1 and failures remain pending. |
| SH-09 task | Enter confirmed line 1 and reloaded that same line, with explicit demo feedback and focus restored to Pick. Six Skip actions reached line 7 with Remaining lines (0); another Skip set `lineIndex=7` but still displayed line 7, as the source clamps the index. Compact clipped the large read-only values (SH-UI-003); Comfortable/reload restored readability. | `10-task-final-line.txt`, `10-task-final-line-390.jpg`, `11-task-comfortable-390.jpg`. No persisted completion exists. The end-of-demo boundary needs later usability judgment; it is not counted as a completed warehouse workflow. |
| SH-02/17 Site Map and Standard | Footer Site Map failed in Material (SH-UI-004), but launcher recovered. Standard link briefly produced a nested Material URL, then normalized back to Material AppList instead of switching (SH-UI-005). Direct `/apps/AppList` rendered Standard successfully; its Site Map reproduced the same render error. Logout remained available. | `16-site-map-error*`, `17-standard-link-result.{json,jpg}`, `18-standard-direct.jpg`, `19-site-map-standard.txt`. No broad Standard walk or cross-app retest claimed. |

**Diagnostics.** `browser-console.json` contains one captured warning: HTTP 500 at 22:44:55 UTC. The server log
at that timestamp records the intentionally empty wizard name validation. This is supporting diagnostic behavior,
not an unexplained extra finding. Site Map's server render errors were visible in both UIs and recorded in the server
log. There is no complete network trace; absence of additional captured console warnings is not a clean-network
claim. No external mail/provider action was attempted.

**Scope reconciliation and next batch.** Source inspection established that the wizard uses only a session draft;
Handheld/Task are non-writing demonstrations; standalone Board/Calendar and Views tree edits also report only.
Views board/calendar changes do persist Example records, with demo dates in October 2026 (EX_100–EX_110), so the
next Views walk must deliberately navigate those dates or prepare current-date disposable rows. SH-07/09 wording
above now reflects these boundaries without treating missing business persistence as a defect. Resume with the
unvisited common forms and remaining SH-04/05/06 branches, then Views/Board and the unfinished wizard/scan
branches. Keep every unrecorded step and mode pending, especially actual TUI, restricted identities, mail, Assist,
files/editors and terminal. Site Map's nested-tree branch is blocked by SH-UI-004; use launcher navigation only as
a named workaround. Do not start refinement or mark the review pass complete.

**Cleanup.** Light/default scheme and Comfortable density restored, no new saved searches or business records,
no grants/providers changed, wizard drafts cleared by final actions, account logged out and protected access
rechecked. Browser viewport override reset. The server exited cleanly after SIGINT at 22:50:34 UTC; HTTP/cluster ports had no listeners and refused
connections, the server process was absent, and the embedded PostgreSQL PID file/processes were gone. See
`runtime-cleanup.json` and `runtime-errors.txt`. Scratch runtime and local evidence are retained for reproducibility.

### SH-B02 — Saved views, shared controls and CSV output, 2026-11-16

**Scope and environment.** Second bounded review batch, alongside System administration, Monitoring and Tools.
Source `83cf05b3b6b069812a6ac2802d95f918db6a9802`; the pulled Rust changes required a current binary. Built with
`cargo build --profile dev --no-default-features --features maximal,devtools -p moqui -j 4` (passed; linker warned
that the large exception frame could affect exception-handling performance). No tests or application fixes.
`target/debug/moqui` SHA-256 `f799a82c681a7c3c9a7973cf5e030a5d1ebf41fa8935280f0d527dbf61bb2004`;
fresh embedded PostgreSQL runtime `/tmp/mr-ui-four-apps-zf5098dh`, six components framework/webroot/tools/basic-comms/
dev-aide/example, no addons/test/demo/template. HTTP loopback `18872`; root browser used `localhost` and the parallel
System browser used `127.0.0.1` to isolate cookies after an initial shared return-URL collision. That collision is
harness context, not a product finding. Telemetry enabled/sample ratio 1 before startup; scheduled jobs and external
protocol listeners disabled. No provider/mail/media fixture; automatic new-device email failed for missing From
address. Actor `john.doe` (`EX_JOHN_DOE`, ADMIN + ADMIN_ADV), no restricted-role evidence. Material light/default,
comfortable, 1920×1080 only. Browser walks ran from 23:49 UTC on Nov 16 into 00:02 UTC Nov 17 (local date Nov 16).
The five SH-B01 findings were not retested and retain their earlier binary/source scope.

**Observed branches.**

1. Landing's ScreenThemeResourceType count opened 34 matching List rows. Saved Find `SH-B02 Theme types` (100000)
   was created and made default. Description `Shell` narrowed to 11; Update to Current and reload retained the
   filter/results. Columns hid Code and moved Description into the preceding ID column using keyboard Return;
   Save Changes and reload retained the configuration. Reset to Default restored columns. Clear Default restored
   all 538 rows. Delete named the saved find; Cancel returned focus to Delete, then confirmed deletion removed it
   from the reopened menu. No saved find from this batch remains.
2. ID descending sort and Next page showed rows 21–40/538 with the expected query parameters. Export → CSV
   displayed raw CSV inside the shell (SH-UI-006). The browser download wait timed out; this alone is not evidence
   of a server export failure. Following the user's transfer preference, curl retrieved the source-backed canonical
   `/apps/gallery/List` output with the same query: HTTP 200, CSV attachment, 538 rows. All ID/State cells were blank
   (SH-UI-007). The copied shell `/qapps` output URL returned zero bytes; it is not the supported canonical output
   route and is not counted as another defect. XLS, print and other output consumers remain pending.
3. Widgets Field rules: Equipment + empty Serial refused submission with Required and focus on Serial. Filling
   `SH-B02-SERIAL` and Save showed “Nothing changed: the gallery writes nothing,” reset the form and refreshed the
   dynamic timestamp. Part hid Detail and showed Note, without required Serial. Server search for Screen returned
   options and selection displayed `Screen [AT_XML_SCREEN]`. Markdown `**SH-B02 preview**` rendered bold preview
   text. These are demo/controller observations, not persistent business writes. Browser locator/clipboard errors
   were recovered with fresh state and semantic controls; they are not recorded as application findings.

**Evidence and limits.** Ignored local `plans/ui-refinement-evidence/shared-gallery/batch-02/` contains runtime
metadata/configuration/build log, `01-saved-find-reloaded`, `02-columns-reloaded`, `03-saved-find-cleanup`,
`04-sort-page`, `05-csv-action`, `06-widget-conditional-success`, `07-widget-search-preview`, canonical CSV bytes,
summary and supporting export source notes. Text captures record controls/data; selected screenshots establish
layout. No dark/narrow/Standard/TUI expansion, complete keyboard walk, provider fixture, full control-family sweep
or prior-finding retest was performed. SH-15 remains pending; the overlapping CSV branch is recorded under SH-04.
Continue with remaining forms/dialogs, records, board/tree/content/files and shell/provider workflows in later
bounded batches; keep review → consolidated triage → refinement separate.

**Cleanup.** Saved Find deleted, default and column preferences restored; no grants or providers changed.
Widget drafts were disposable. Root viewport reset and temporary tab closed; Logout reached confirmation but was
not submitted, so no logout success is claimed. After all four reviews, SIGINT stopped the server at
2026-11-17 00:02:38 UTC with exit 0. Server/HTTP listener and runtime PostgreSQL processes/PID file were absent.
The curl cookie file was removed; scratch runtime/database and local evidence retained. See `runtime-cleanup.json`.

### SH-B03 — Post-cleanup recheck, 2026-11-25

The built-in browser reached the six-component disposable instance on `shell.localhost:18765`. The coordinator identified release build `6d8bb49f8a7c`. Test Login opened the John Doe launcher and Gallery in Material. Evidence is under `plans/ui-refinement-evidence/shared-gallery/batch-03/`.

| Finding | Journey and observed outcome |
| --- | --- |
| SH-UI-001 | **Passed.** Saved customer name `SH-B03 Review`, returned from Options without saving, opened Review directly and chose Create customer. The blank country produced “Choose a country in Options before creating the customer” and preserved the draft. Back → Options → Next → Create acknowledged the named customer and explained that the demo writes nothing. |
| SH-UI-002 | **Passed.** The `2026-13` bookmark showed an invalid-month notice, November 2026, 13 events and date controls spanning November 1 through 30. Next twice traversed December to January 2027 and updated the bookmark to `month=2027-01`. |
| SH-UI-003 | **Passed.** At measured width 390 in Compact, the final task line showed C-11-21 and BRACKET-R · Bracket, right. Location and Product each measured 39 px client height and 39 px scroll height in light and dark. The dark capture shows both complete values and Remaining lines (0). Evidence: `task-dark-390.jpg`. |
| SH-UI-004 | **Passed in both roles.** From each shell's footer, Site Map showed nested Gallery and app routes. Show All expanded Material from 270 to 349 links and Standard from 273 to 352 (whole-page counts include shell navigation). At 1280×900, `sec.none.only` on a separate hostname saw 3/6 links in Material and 6/9 in Standard, with Gallery, Tools and System absent before and after Show All. The fixture uses SEC_NONE_GROUP. |
| SH-UI-005 | **Passed.** The launcher's visible Standard link opened `/apps/AppList` with Standard selected, a Material return link and six available apps. The Standard footer opened its own Site Map. |
| SH-UI-006 | **Partial.** Sorted ID descending, opened page two (21–40 of 678) and expanded Export. CSV exposed the canonical `/apps` URL with the sort, page, no-limit and filename parameters plus the HTML download attribute. Both CSV and XLS transfers succeeded using curl as John Doe. On return, Escape closed the menu with page two and its first ID intact. Native click-to-download behavior is unverified this round because transfers used curl. See `sorted-export-menu.jpg`. |
| SH-UI-007 | **Passed.** The 53,859-byte CSV contained 678 data rows with 678 distinct populated IDs in descending order and 678 populated State cells. Its 21st ID matched the visible VppGeneric. The adjacent 38,228-byte XLSX opened as a valid package with 679 worksheet rows including the header. See `EnumList.csv`, `EnumList.xlsx` and `export-summary.json`. |

The review exercised Material at 1920×1080, Compact light/dark at 390×844, and the restricted navigation sample at 1280×900. Returned the demo account to Light and Comfortable and reset the browser viewport. Existing TUI, unvisited activity branches and original filter-specific output checks retain their previous coverage. No new finding or automated test from this walk.

### Cross-plan triage, 2026-11-25

The nine reviews used the Maximal release binary on checkout `6d8bb49f8a7c` and a disposable database. The initial instance loaded framework, webroot, basic-comms, tools, dev-aide and example. The Dev AIDE walks added three local fixture components. The four first reviews recorded outcomes for 62 numbered activities. Their ledgers distinguish a completed operation from an inspected entry point, a blocked continuation and modes still pending.

The local evidence manifest at `plans/ui-refinement-evidence/round-20261125/runtime.json` records the full checkout and binary SHA-256. All times for this round use America/Chicago unless an evidence record gives a UTC timestamp.

The six registers with new findings contain **16 findings: two P1, ten P2 and four P3**. Across the five prior reviews, 25 of the 50 historical entries passed their original behavior checks, 11 had partial live verification, four remained unverified, and ten recorded proposal decisions. The BasicComms ledger gives the disposition of every original ID.

| Priority | Finding and corrective scope |
| --- | --- |
| P1 | **DASH-UI-003.** Running an analysis returned to the populated form without a result, error or Save panel action. A fresh scoped sign-in reproduced it after loading finished. Restore execution from the Analysis Run control and recheck the result-to-panel handoff. [Dashboards](ui-review-dashboards.md#findings-register) |
| P1 | **AI-UI-002.** A single bounded sync run received HTTP 302 and produced 772 history rows. The runner was stopped. The peer contained zero copies of the selected record. Bound failed retries and make the stopping condition visible in History. The reason for the original redirect remains unresolved. [System AI and integrations](ui-review-system-ai-integrations.md#findings-register) |
| P2 | **DASH-UI-001/002.** Explore selected a service dataset it could not query, and rendered 2026 fixture timestamps as 1970. The exported timestamps and context links provided the comparison for the latter. [Dashboards](ui-review-dashboards.md#findings-register) |
| P2 | **DEVW-UI-001–003.** The new-site wizard accepted a dependency that startup excluded. Standard Home offered a Workspace link that returned 404. The viewer's Workspace tree displayed `[object Object]`. [Dev AIDE workspace](ui-review-dev-aide-workspace.md#findings-register) |
| P2 | **DEVA-UI-001/002.** The Seed data tab opened an empty pane despite a site seed file. Changing a dialog button label in the designer left Save disabled and the source unchanged. [Dev AIDE authoring](ui-review-dev-aide-authoring.md#findings-register) |
| P2 | **AI-UI-001.** New Remote Agent omitted the private-network choice required to register its local peer. A separately prepared record reached the local provider. [System AI and integrations](ui-review-system-ai-integrations.md#findings-register) |
| P2 | **TOOL-UI-003.** Returning from Record discarded the Example Grid filter. **COM-UI-038.** Mail Print reached the corrected mount but returned an empty HTML body, while EML and attachment downloads contained the expected bytes. [Tools](ui-review-tools.md#findings-register), [BasicComms](ui-review-basic-comms.md#findings-register) |
| P3 | **TOOL-UI-004.** Grid displayed “1 rows”. **AI-UI-003.** Six text inputs on Connection Detail shared the accessible name “Value”. The connection editor owns the latter issue across its consumers. [Tools](ui-review-tools.md#findings-register), [System AI and integrations](ui-review-system-ai-integrations.md#findings-register) |
| P3 | **COM-UI-039.** Empty Spam and Trash both announced “Your Inbox is empty” while Inbox contained messages. **DASH-UI-004.** Git synchronization displayed Last push as epoch milliseconds. [BasicComms](ui-review-basic-comms.md#findings-register), [Dashboards](ui-review-dashboards.md#findings-register) |

The two System findings, two Monitor findings and two historical Tools findings passed their selected rechecks. System owns the telemetry readiness message shared with Monitor. Gallery verified six fixes and the transfer URL and contents of the seventh. Its native download click remains unverified under the curl transfer workflow. BasicComms records outcomes for its historical observations, source audits and proposal decisions in [COM-B10](ui-review-basic-comms.md#com-b10--current-build-recheck-2026-11-25). The new empty print result has a separate ID from the earlier wrong-mount finding.

**Closeout.** The dashboard schedule and alert were paused, dashboard Git sync disabled, and the shipped Constitution restored. The primary runtime, local peer, both databases, SMTP sink and model/protocol fixtures were stopped. Listener checks found all six review ports closed. Temporary transfer credentials were removed. The stopped fixture directories and ignored evidence were retained locally. Only the nine review plans changed in the tracked tree. Fixture setup and readback ran during the walks; no acceptance suite ran.

### Refinement, 2026-11-25

All 16 findings are fixed, in commits 62d502416 through 258d3479e. Each register row records its correction, its
regression test and its live recheck on a disposable runtime. Four causes were in shared layers rather than in the
screen where they showed:

- **DASH-UI-003**: `m-form-link` dropped the clicked submit button. This affected every GET form to a screen, not only
  Analysis.
- **DEVW-UI-002**: the framework now sends a qvue-only screen requested in html under `/apps` to the same path under
  `/qapps`.
- **DEVW-UI-003**: View-tier reads with a list or check verb now declare `authz-action="view"`, and the shared
  m-file-tree shows the answer's errors.
- **AI-UI-003**: the shared `NativeConnection` form names each control by its setting, which fixes ConnectionDetail
  and MailRelayDetail together.

Rechecks found two further problems in the same paths, both fixed:

- An ungrouped Analysis count had no row.
- Home reported "Created" even when a component was refused.

Not rechecked live:

- DASH-UI-004, because the runtime had no Git sync fixture.
- The designer canvas for DEVA-UI-002, because the preview was refused for lack of a grant.
- The reviewer's disabled Save for DEVA-UI-002, which was not reproduced.
- The origin of AI-UI-002's 302, which is still unknown; the likely cause is a peer redirecting http to https.

The slow `tools_screens` test passes in full, including its Analysis ungrouped-count assertion (9526ed139). Two of
its earlier checks had fallen behind security rules: the fixture's DataDocument now has an ADMIN user group, so
Datasets offers it (SEC-TLS-92), and the export filter is a JSON object (SEC-TLS-01).

## 7. Decisions

- Nine companion files are indexed in [plans](README.md); this one owns common methodology and shared behavior.
- Primary activity IDs use `SH`; findings use `SH-UI`. Cross-plan links describe handoffs, not duplicate ownership.
- Gallery and common-shell coverage were explicitly added to the original five-app scope. All current Gallery
  screens are included; backend-only endpoints have bounded supporting inspections.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
