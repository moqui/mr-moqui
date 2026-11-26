# MR Moqui — UI review: Dev AIDE workspace and collaboration

**Review update — 2026-11-25.** DEVW-B02 records the latest live evidence. The activity ledger identifies the branches still open.

**Status — 2026-11-25, refinement.** All three DEVW-B01 findings are fixed (DEVW-UI-001–003).

**Previous status — 2026-11-25.** First live review (DEVW-B01). The disposable Git fixture reached a verified commit, local push, conflict abort and session-worktree cleanup. Three findings are open. Hosted collaboration and the remaining role, mode and recovery branches are listed below.

Contents: [scope](#1-purpose-scope-and-sources) · [method](#2-review-method-and-refinement-criteria) ·
[inventory](#3-screen-and-surface-inventory) · [activities](#4-end-to-end-activities) ·
[verification](#5-existing-checks-and-future-verification) · [record](#6-coverage-findings-and-running-record) ·
[decisions](#7-decisions).

## 1. Purpose, scope and sources

Review the developer's workspace loop: select the right root/task, edit and recover safely, inspect changes, commit
and collaborate, then verify what reloaded or still needs restart. One Workspace route contains many distinct
workflows, each listed below. Guided entity/service/screen authoring has its own companion file.

Sources: [Dev AIDE README](../runtime/components/dev/dev-aide/README.md),
[design](design-dev-aide.md), [Run and Deploy](../docs/Run-Deploy.md),
[Git hosting](../docs/interfaces/Git-Hosting.md), [Terminal](../docs/ui/Terminal.md),
current DevAide Home/Workspace/Repositories/Tasks/Settings, Workspace QVue/TUI,
`devstatic`, `DevAideSecurityData.xml`, and `org.moqui.devaide` services. Reconcile current implemented group-server
and worktree controls with older design statements before calling a feature absent.

### Ownership and handoffs

| Surface | Owner / handoff |
| --- | --- |
| DevAide root/Home, Workspace, Repositories, Tasks, Settings and WorkspaceTui | This plan |
| Artifacts/Entities/Services/Screens/Eca/Data and GuidedEdit | [Authoring](ui-review-dev-aide-authoring.md); endpoints hosted by Workspace retain their source owner here |
| Shared terminal, Assist drawer and shell page actions | [Shared/Gallery](ui-review-shared-gallery.md); developer change sets, session worktrees and local job handoff here |
| Tools source/entity/service links | [Tools](ui-review-tools.md); cross-app selected artifact context must survive |
| Group-server repository checks/reviews | Local framework hosting fixture; no addon Smithy or real external repository required |

Dev AIDE writes are real writes as the server user and are refused under sim/agent fences. Do not use that as a
reason to exercise them against the actual source checkout. Every mutable root/remote is a disposable fixture.

## 2. Review method and refinement criteria

Apply the [common review protocol](ui-review-shared-gallery.md#common-review-protocol) and
[verification policy](ui-review-shared-gallery.md#verification-policy-for-later-execution). The details below add this
area's fixtures, dependencies and judgment criteria.

### Environment, identities and records

| Proposed actor | Grant / purpose |
| --- | --- |
| `review.dev.viewer` | DEV_AIDE_VIEW plus the prepared Dev AIDE screen grant; read/search, no source/Git mutation |
| `review.dev.extender` | DEV_AIDE_EXTEND, as DEV_AIDE_EXTENDERS; site-owned edits/publish only |
| `review.dev.full` | DEV_AIDE, as DEV_AIDE_USERS; upstream toggle and full local Git on fixture roots |
| `review.dev.host` | Full developer plus TERMINAL_HOST only for local CI/host sessions |
| Local group-server contributor/reviewer | Separate repository rights, token and synthetic contributor-agreement state; instance permission alone does not grant remote write |

Use a devtools-capable binary on a dev instance. Add disposable component/site repositories and a separate local
bare remote; source-linked framework roots are read-only. Prepare clean/dirty/untracked/binary/read-only files,
conflicting disk edits, divergent branches, tags/stashes, signed synthetic collaboration fixtures, one pending/rejected
agreement case, check success/failure/logs, a harmless `ci.xml`, task source and two Assist session worktrees.
Record runtime roots, branches and remote URLs before starting. Group-server setup may alter disposable myaddons,
credentials and `.env`; compare its proposed changes and remove only its owned settings at cleanup. Local CI uses
safe fixture commands and no secrets/container deployment. Do not push/comment/sign on real services.

### Area-specific criteria

- Root, branch, task, file, save/reload and restart state are visible together. A save acknowledges the submitted
  version; stale disk content, dirty editor buffers and Git changes remain distinct.
- Read-only/site-only/full tiers are clear before mutation, and still enforced by the endpoint. Network Git and
  upstream-edit toggles cannot silently bypass root confinement or repository rights.
- Git destructive actions show exact files/hunks/branch/commit and let the user inspect differences. Abort/cancel
  and failure preserve enough context to recover; status must reflect actual repository state.
- Connection credentials, group-server rights and contributor agreement are distinct. Hosted checks and local runs
  are labelled accurately; a session worktree not loaded into the live runtime must not claim live preview behavior.

### Ordered waves

| Wave | Activities | Continuation |
| --- | --- | --- |
| 1. Local edit | DEVW-01–03 | Root/component → edit/reload → disk conflict |
| 2. Git and task context | DEVW-04–08 | Changes/commit → branch/history/conflict → local remote/publish/task |
| 3. Team collaboration | DEVW-09–11 | Connect → merge request/review → checks/local run |
| 4. Assist and boundaries | DEVW-12–13 | Session worktree/change set → access/TUI/remaining surfaces |

Keep separate repositories for destructive Git/conflict, ordinary authoring and collaboration work. Publish/restart
only after authoring's dependent live-preview checks. Remove worktrees after their changes/evidence are retained.

## 3. Screen and surface inventory

This inventory accounts for **8 XML definitions** assigned to this file; screen containers and helper
definitions are included. Routes below are Material paths unless labelled otherwise; app routes have Standard
`/apps` counterparts. A file's physical presence does not prove access or a supported interaction in every mode.
Each row maps to §4; helper inspection does not replace its parent workflow. Counts describe definitions, not user
activities. Dialogs, templates and non-XML surfaces follow the route tables.

### dev-aide: `DevAide`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dev/Home` | [Home.xml](../runtime/components/dev/dev-aide/screen/DevAide/Home.xml) | Inspect the route and its visible controls through the activity. | DEVW-01, DEVW-07 |
| `/qapps/dev/Repositories` | [Repositories.xml](../runtime/components/dev/dev-aide/screen/DevAide/Repositories.xml) | Context: `loadName`. | DEVW-01, DEVW-07 |
| `/qapps/dev/Settings` | [Settings.xml](../runtime/components/dev/dev-aide/screen/DevAide/Settings.xml) | Context: `serverId`. | DEVW-01, DEVW-09, DEVW-13 |
| `/qapps/dev/Tasks` | [Tasks.xml](../runtime/components/dev/dev-aide/screen/DevAide/Tasks.xml) | Inspect the route and its visible controls through the activity. | DEVW-08 |
| `/qapps/dev/Workspace` | [Workspace.xml](../runtime/components/dev/dev-aide/screen/DevAide/Workspace.xml) | Companions: [Workspace.qvue](../runtime/components/dev/dev-aide/screen/DevAide/Workspace.qvue). Authored text: `qvue`, `tui`. Endpoint host also serves guided authoring: DEVA-04–13. Tabs/dialogs are inventoried separately. Context: `rootId`, `path`, `line`, `view`, `tab`, `query`, `regex`, `changePath`, `staged`, `commitId`, `commitPath`, `pageIndex`, `diff`. Declared modes: `qvue,tui`. | DEVW-02, DEVW-03, DEVW-04, DEVW-05, DEVW-06, DEVW-07, DEVW-10, DEVW-11, DEVW-12, DEVW-13 |
| `/qapps/dev/WorkspaceTui` | [WorkspaceTui.xml](../runtime/components/dev/dev-aide/screen/DevAide/WorkspaceTui.xml) | Authored text: `html,qvt`, `tui`. Hidden/helper entry; enter through its parent action and inspect direct access. Context: `rootId`, `path`, `line`, `view`, `tab`, `query`, `regex`, `changePath`, `staged`, `commitId`, `commitPath`, `pageIndex`, `diff`. | DEVW-02, DEVW-03, DEVW-04, DEVW-06, DEVW-07, DEVW-13 |

### dev-aide: `roots`

| Route / surface | XML source | Default / variants / boundary | Activities |
| --- | --- | --- | --- |
| `/qapps/dev` | [DevAide.xml](../runtime/components/dev/dev-aide/screen/DevAide.xml) | Default: `Home`. | DEVW-01, DEVW-13 |
| `/devstatic (supporting assets)` | [devstatic.xml](../runtime/components/dev/dev-aide/screen/devstatic.xml) | Static support route; no independent user workflow. Editors consume its modules. | DEVW-13 |

### Embedded surfaces, dialogs, outputs and reconciliation

| Workspace or supporting surface | Activity / inspection |
| --- | --- |
| Root/file tree, Go to file, search, editor tabs, breadcrumbs, schema checks/completion, presence, file/folder create/rename/delete | DEVW-01–03; distinguish path selection from artifact authoring |
| Changes diff, selected files/hunks stage/unstage/discard, commit/task trailer | DEVW-04; every hunk operation is a separate pending/saved state |
| Branches compare/create/switch/rename/delete/merge, tags and stashes | DEVW-05/06; dialogs include target and dirty-state recovery |
| Commits detail/tree/file-at/blame, cherry-pick/revert/conflict sides/resolve/abort | DEVW-06; only disposable authored fixtures, contribution policy still applies to any real PR |
| Remotes add/change/remove/prune, fetch/pull/push/publish and cancel Git | DEVW-07; local remote only |
| Tasks source/current/create/branch and task links in commit/workspace | DEVW-08; task identity persists through branch and commit |
| Settings group-server sign-in/token/offer/setup/remove, credentials/network Git/upstream/root/site settings | DEVW-09/13; no secret returned in setup preview |
| Merge requests list/checkout/new/propose/fork, files/diff/review threads/gutter/reply/resolve/submit and agreement | DEVW-10; real remote actions excluded; reading unsigned real contributions follows repository policy |
| Commit/check chips, run/job logs/test links/rerun, Run locally and terminal handoff | DEVW-11; CI image/secrets are not copied into a host-local run |
| Sessions worktrees, change-set review/completeReview and Dev Assist page actions | DEVW-12; fence/sim refusal and ordinary user-controlled writes remain distinct |
| WorkspaceTui file/tree/search/problems/artifact links, change/commit/Git dialogs | DEVW-02–08/13; branches/tags/stashes/reviews/blame can delegate to browser rather than fabricate parity |
| Workspace guided endpoints and `devstatic` authoring modules | Owned source here; interactions inventoried in [authoring](ui-review-dev-aide-authoring.md), not duplicated |
| Browser tab/local-draft persistence and `.qvue` Design entry | DEVW-02/03 own tab recovery and disk conflicts; DEVA-12 owns QVue element editing/preview; DEVA-13 owns multi-file guided save and terminal buffer expiry |

Home's restart/new component and Repositories' get/clone/load controls are part of the review. Older statements that
group-server support is merely proposed do not override current Settings/Workspace controls. The archive's eight
scratch-repository journeys are historical coverage only.

## 4. End-to-end activities

Use visible controls and inspect intermediate states, confirmation and the resulting data/output. Unless narrowed
below, each activity has Material light/dark, actual supported TUI and a brief Standard/output branch. Select narrow
and keyboard checks where layout/interaction differs. Record each unattempted branch in §6; a service call or render
alone does not complete the activity. Names beginning `review.` are fixture proposals, not shipped accounts.

### DEVW-01 — Choose a root and create a disposable component

- **Actor/start:** Full developer, extender and viewer; isolated dev runtime with empty site plus existing
  read-only roots.
- **Objective/steps:** Home → inspect roots/branches/reload/pending restart → New site/component → inspect
  created root → Repositories get/clone local fixture/load → Workspace → Settings root labels/read-only/site
  selection.
- **Expected:** Component/root identity and edit tier are clear; generated files land only in the disposable
  location; loaded versus not-loaded roots and restart needs are explicit.
- **Recovery:** Invalid/duplicate name, missing devtools, unavailable local remote, denied creation and canceled
  dialog leave existing roots unchanged.
- **Screens/modes:** Home, Repositories, Settings, Workspace/root chooser; browser/TUI/Standard capability
  boundaries.

### DEVW-02 — Edit a file and observe what reloaded

- **Actor/start:** Extender on site root and viewer; XML/JS/text/binary fixture files and harmless syntax error.
- **Objective/steps:** Workspace tree/Go to file/search → open file → edit → schema completion/check → Save →
  inspect problems/reload → follow artifact to consumer → return; create/rename/move/delete fixture path where
  offered.
- **Expected:** Path/root and saved revision are visible; validation versus save/reload failure are distinct;
  binary/read-only files are not offered a misleading text save.
- **Recovery:** Invalid syntax, denied path, failed write, late response, unsaved navigation and canceled delete
  preserve recoverable buffer and correct selection.
- **Screens/modes:** Workspace editor/search/problem/file dialogs and WorkspaceTui; authoring owns guided edit;
  keyboard and narrow/two-pane focus.
- **Persistence branch:** Leave a dirty browser tab and companion buffer, reload/reopen, then accept or discard
  the Unsaved changes offer on separate fixtures. Repeat after the disk base changes and under a different
  user/root; drafts must identify their origin and cannot silently overwrite the newer file. Browser local drafts
  are distinct from the server-side terminal buffers reviewed in DEVA-13.

### DEVW-03 — Resolve a disk conflict without losing either edit

- **Actor/start:** Two editor sessions on a disposable file with different edits.
- **Objective/steps:** Open file → edit locally → change disk in second session → Save → inspect conflict/diff →
  Take disk on one fixture and Save mine over it on another → reload and inspect retained result.
- **Expected:** Disk and buffer versions are labelled; refusal writes nothing; the explicit choice acts on the
  compared revision and acknowledges only that snapshot.
- **Recovery:** Another disk change during conflict, failed follow-up save, canceled choice and close while
  saving preserve content and truthful dirty state.
- **Screens/modes:** Workspace/WorkspaceTui conflict controls, editor recovery copy and diff; browser/TUI,
  native input/blur behavior.

### DEVW-04 — Review hunks and make a task-linked commit

- **Actor/start:** Developer with staged/unstaged/untracked fixture changes and selected task.
- **Objective/steps:** Changes → inspect file/hunk diff → stage selected hunks → unstage one → cancel then
  discard a disposable hunk → enter commit → inspect resulting Commits/tree and task trailer.
- **Expected:** Index versus worktree is understandable; only selected changes commit; task linkage and author
  match the intended context.
- **Recovery:** Empty commit, invalid identity/message, failed Git command, stale hunk and canceled discard
  retain repository status and recoverable edits.
- **Screens/modes:** Workspace Changes/Commit/Commits and WorkspaceTui subset; no actual application source
  commit.

### DEVW-05 — Keep work through branch, tag and stash operations

- **Actor/start:** Full developer; disposable repository with two branches, dirty changes, tags and stashes.
- **Objective/steps:** Branches compare/create/switch/rename → inspect dirty-state prompt → stash selected work
  → switch → apply/pop → inspect conflict if any → create/delete local tag and branch with confirmation.
- **Expected:** Current branch and retained changes are visible; stash apply/pop/drop and tag versus branch have
  distinguishable effects.
- **Recovery:** Dirty checkout refusal, duplicate name, non-fast-forward branch, stash conflict and canceled
  deletion retain enough evidence to recover.
- **Screens/modes:** Workspace branches/tags/stashes dialogs; TUI supported branch operations and explicit
  browser delegation for unavailable features.

### DEVW-06 — Inspect history and recover a conflict

- **Actor/start:** Full developer; synthetic locally authored commits, divergent branches and conflict fixture.
- **Objective/steps:** Commits → file/tree at revision → blame → compare → cherry-pick/revert/merge disposable
  commit → inspect conflict sides → resolve → continue or abort → inspect final history/status.
- **Expected:** Selected commit/ref/file and conflict sides are clear; abort returns to a defined state; history
  shows the intended operation once.
- **Recovery:** Missing ref, unsigned real contribution boundary, conflict left unresolved, failed continuation
  and canceled destructive action do not silently drop edits.
- **Screens/modes:** Workspace Commits/blame/history/conflict dialogs, WorkspaceTui cherry-pick/revert; actual
  fixture Git status readback, no real PR port.

### DEVW-07 — Synchronize and publish to a local remote

- **Actor/start:** Full developer or permitted site extender; disposable bare remote, divergent local/remote
  change and site publish branch.
- **Objective/steps:** Configure remote → Fetch → inspect comparison → Pull → resolve if needed → Push/Publish →
  inspect result and Home reload/restart state → cancel a separate harmless pending operation.
- **Expected:** Fetch, working-tree update, remote publication and live runtime reload are distinct;
  branch/remote and published commit are visible.
- **Recovery:** Missing network permission, wrong remote, rejected push, dirty tree, failed publish and canceled
  operation leave an accurate repository state.
- **Screens/modes:** Repositories/Home/Workspace remote/Git controls; local remote only, terminal supported
  subset and browser delegation.

### DEVW-08 — Keep task context through a branch and commit

- **Actor/start:** Developer with local task provider, one existing task and quick-create target.
- **Objective/steps:** Tasks → select source/filter → create or select → Set current → branch → Workspace
  edit/commit → inspect task trailer → switch task/source → return.
- **Expected:** Current task, branch and commit link agree; changing source does not silently attach work to a
  stale task.
- **Recovery:** No task provider, invalid task, inaccessible source, duplicate branch and canceled create
  preserve previous context with a clear explanation.
- **Screens/modes:** Tasks forms, Workspace task controls and commit context; browser/TUI/Standard where
  offered.

### DEVW-09 — Connect a local group server and inspect owned setup

- **Actor/start:** Developer and local hosting account; disposable `.env`/component catalog/credential
  configuration, no external account.
- **Objective/steps:** Settings Connect/sign-in or token → inspect offer and proposed setup → apply → inspect
  repository/task/gateway configuration → reconnect/refresh → Remove → compare restored ownership.
- **Expected:** Connection token and gateway-only key remain separate; preview names changed files/settings;
  removing connection retracts only what it installed.
- **Recovery:** Wrong server/state/token, canceled sign-in, partial setup, conflicting user setting and expired
  credential preserve manual configuration and secret privacy.
- **Screens/modes:** Settings group-server/sign-in/setup/root/credential dialogs; local fixture server, browser
  flow with truthful terminal/Standard delegation.

### DEVW-10 — Review a local merge request and its discussion

- **Actor/start:** Two local contributors/reviewer with distinct repository rights and synthetic agreement
  states; fixture commits only.
- **Objective/steps:** Workspace hosted repository → Merge requests → checkout/open/new or propose/fork as
  rights allow → inspect changed file and gutter threads → comment/reply/resolve/submit review → inspect
  agreement warning/sign fixture when permitted.
- **Expected:** Remote repository/branch/revision and rights are clear; review comments attach to intended
  lines/revision; unsigned real contributions never expose a diff for porting.
- **Recovery:** No write right, stale revision, failed comment, unresolved agreement, canceled proposal and
  unavailable server retain local work and accurate remote status.
- **Screens/modes:** Workspace merge-request/diff/review/agreements surfaces; browser-only branches explicitly
  delegated, no real remote messages.

### DEVW-11 — Explain a failed check and run its safe local counterpart

- **Actor/start:** Developer with local hosting check result/log/failed-test fixture; TERMINAL_HOST holder for
  harmless local CI.
- **Objective/steps:** Commit/branch check chip → run/job → inspect logs and failed-test source link → rerun
  fixture check → Run locally from permitted ci.xml → follow terminal session/steps → inspect completion.
- **Expected:** Remote CI and host-local execution are clearly labelled; local command uses intended working
  tree without remote image/secrets; result links preserve commit/task.
- **Recovery:** Missing config, failed job, forbidden rerun/host session, stopped command and malformed log
  preserve diagnostics and do not claim remote CI passed.
- **Screens/modes:** Workspace checks/localJobs/startLocalRun, terminal handoff SH-14; safe local processes only
  and explicit browser/TUI limits.

### DEVW-12 — Review an Assist change in an isolated session worktree

- **Actor/start:** Two Assist sessions with separate disposable worktrees and developer-owned change sets.
- **Objective/steps:** Sessions → create/select worktree → ask local fixture agent → inspect proposed changes →
  accept/reject through user controls → check/commit in that worktree → inspect live-root separation → remove
  after preserving needed result.
- **Expected:** Session/root/branch and proposed versus written changes remain clear; an unloaded worktree never
  claims live reload; rejected changes do not write.
- **Recovery:** Agent/sim fence refusal, conflicting buffer, failed worktree creation/removal, another session
  ownership and canceled acceptance retain source and context.
- **Screens/modes:** Workspace Sessions/changeSet/completeReview, Dev Assist page actions and SH-13 drawer;
  authoring DEVA-13 owns guided buffers.

### DEVW-13 — Reconcile tiers, TUI and hidden endpoints

- **Actor/start:** Viewer/extender/full developer and unrelated user; earlier valid fixture
  roots/files/operations.
- **Objective/steps:** Repeat selected read/write with each relevant tier → disable upstream/network edit →
  inspect server refusal → traverse WorkspaceTui tree/search/changes/commit → follow browser-delegated operation
  → return; inspect remaining endpoints by owning activity.
- **Expected:** Root confinement and tier limits hold at endpoints; terminal actions target correct files;
  delegated browser behavior is explicit; every surface is accounted for.
- **Recovery:** Root disappears, revoked grant, stale file, unsupported feature and disconnected group server
  remain partial/blocked branches without modifying source-linked roots.
- **Screens/modes:** All owned routes, devstatic support and Workspace guided endpoints handed to DEVA; no
  second full sweep or real repository writes.

## 5. Existing checks and future verification

This is an available-test map, not a results ledger. Read assertions and suite setup before selecting a run; static
shape checks, offline component fixtures, live browser walks and service rules provide different evidence.

| Component / suite | Existing specs | Coverage and limits |
| --- | --- | --- |
| dev-aide / server | [WorkspaceTui](../runtime/components/dev/dev-aide/test/WorkspaceTui.spec.js) | File tree, stale-hash save/conflict, terminal change/commit surface and root navigation; not the whole Git collaboration UI. |
| dev-aide / server | [GroupServers](../runtime/components/dev/dev-aide/test/GroupServers.spec.js) | Connection offer/setup state, secret suppression and local-server URL rules; no complete remote review walk. |
| dev-aide / client | [DevAide](../runtime/components/dev/dev-aide/test/client/DevAide.spec.js) | Offline Workspace/controller fixtures; no live repository/network evidence. |
| webroot / client | [WebrootTerminal](../runtime/components/base/webroot/test/client/WebrootTerminal.spec.js), [WebrootFloats](../runtime/components/base/webroot/test/client/WebrootFloats.spec.js), [WebrootAssist](../runtime/components/base/webroot/test/client/WebrootAssist.spec.js) | Shared offline terminal/Assist interactions only; developer root/Git effects remain to review. |

The [component README](../runtime/components/dev/dev-aide/README.md) also identifies Rust integration coverage for
scratch saves/Git/publish/group servers and opt-in browser checks. Those are supporting existing checks, not a reason
to run cargo or the full acceptance suite for these documents. Inspect their registrations before a later selected run.

Future acceptance candidates: **DevAideWorkspaceWalk** (DEVW-01–03), **DevAideGitWalk** (DEVW-04–08),
**DevAideCollaborationWalk** (DEVW-09–11), **DevAideSessionWorktreeWalk** (DEVW-12) and
**DevAideAccessTuiWalk** (DEVW-13, shared with authoring). Reuse existing scratch-root and browser fixtures; no
source-editing test may write a linked framework/addon root. One Dev AIDE sweep covers both companion documents.

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
| DEVW-01 — Choose a root and create a disposable component | exercised · partial | Created both repositories from Home and checked their files, Settings and restart notice. The generated app loaded. The original site failed discovery. Repository clone, duplicate-name and cancellation remain open. | DEVW-UI-001 |
| DEVW-02 — Edit a file and observe what reloaded | exercised · partial | Opened scratch.txt through the tree, saved with the editor and reopened its bytes in another tab. Draft recovery offered Restore and Discard. Binary, XML validation, move/delete, recovery after reload and narrow keyboard branches remain open. | save.jpg |
| DEVW-03 — Resolve a disk conflict without losing either edit | exercised · partial | Two tabs edited the same file. The losing save displayed the disk and buffer side by side. Take the disk’s retained the peer edit in the subsequently committed file. Save mine, third edit and failed follow-up remain open. | conflict.jpg |
| DEVW-04 — Review hunks and make a task-linked commit | exercised · partial | Staged scratch.txt alone and committed 2a2c81b. Changes returned to zero and history named John Doe. Task association, split hunks, unstage/discard and failed commits remain open. | DEVW-B01 |
| DEVW-05 — Keep work through branch, tag and stash operations | exercised · partial | Branch comparison showed compare one commit and one file ahead of its common base. Tags listed fixture-baseline. Stashes showed an empty list. Switch/rename, stash lifecycle and deletion remain open. | DEVW-B01 |
| DEVW-06 — Inspect history and recover a conflict | exercised · partial | Inspected README.md at e5e6c6b and its two-sided diff. Merging compare produced a non-fast-forward prompt and an actual conflict. Abort restored clean main. Resolution/continue, blame, cherry-pick and revert remain open. | merge.jpg |
| DEVW-07 — Synchronize and publish to a local remote | exercised · partial | Fetch, Push and Pull reported operation-specific results. The bare remote contained 2a2c81b and the local branch tracked origin/main. Site Publish, cancellation and rejected push remain open. | DEVW-B01 |
| DEVW-08 — Keep task context through a branch and commit | exercised · partial | Tasks displayed 0 open tasks from No task source. Source selection, quick create and a current-task commit require a populated task provider. | tasks.jpg |
| DEVW-09 — Connect a local group server and inspect owned setup | exercised · partial | Connected to the local group server as John Doe and resolved review-hosted from Workspace Settings. Credential rotation and offer/apply/remove remain open. | DEVW-B01; DEVW-B02 |
| DEVW-10 — Review a local merge request and its discussion | exercised — partial | Opened hosted MR !1 and inspected both README revisions. The peer reported no review threads. Repeat with discussion storage and an independent reviewer. | DEVW-B01; DEVW-B02 |
| DEVW-11 — Explain a failed check and run its safe local counterpart | exercised — partial | Ran the previewed local CI job in a terminal and verified its worktree marker. Supply a failed hosted job to inspect its logs, source and rerun. | DEVW-B02 |
| DEVW-12 — Review an Assist change in an isolated session worktree | exercised · partial | Created agent/review-session and removed its clean worktree. The UI named the retained branch and explained live-root separation. Bound Assist conversations and proposed changes were unavailable. | sessions.jpg |
| DEVW-13 — Reconcile tiers, TUI and hidden endpoints | exercised · partial | Viewer sign-in showed disabled Git/write controls and an unreadable tree error. Browser-hosted Apps TUI opened Dev AIDE Home and the devw-git Workspace tree with its explicit browser delegation. Standard Home’s Workspace link returned 404. The separate review.dev.extender identity edited scratch.txt on the same site root, saved and received acknowledgement; disk readback preserved its line. Endpoint-level refusals, TUI commits and live theme changes remain open. Scoped viewer opened the copied app tree from the Standard entry; also sampled the TUI Workspace controls. | DEVW-UI-002, DEVW-UI-003; tui-workspace.jpg; DEVW-B02 |

### Findings register

Use `DEVW-UI-001` onward. P1 blocks work or essential understanding; P2 causes substantial friction or misleading
presentation; P3 is a localized refinement. Label evidence as observed behavior, static suspicion or design judgment.

| ID / severity | Actor, activity step, route, mode / viewport | Reproduction and impact | Evidence / owning layer | State / correction / retest |
| --- | --- | --- | --- | --- |
| DEVW-UI-001 · P2 | ADMIN, DEVW-01, Material Home | Create site component with Extends = example. Home reports that the site will load after restart and Settings makes it the default. Discovery then omits the site because a component enabled on every instance cannot depend on a dev-only component. Validate the choice before saving or generate a compatible component purpose. | Observed Home/reload log and startup error in DEVW-B01. Dev AIDE site creation. | **Fixed and checked 2026-11-25.** create#SiteComponent applies startup's dependency rule before writing anything. It refuses an Extends component that this instance doesn't load, or that loads only on some purposes, and says why. Home shows "Created" only for a component that was created, and the Extends field's tooltip names the rule. Live: Extends = example was refused with "example loads only on dev instances…". Extends = tools created my-site, which loaded after the restart. workspace.rs unit test. |
| DEVW-UI-002 · P2 | ADMIN, DEVW-13, Standard Home | Choose Open workspace on /apps/dev/Home. The link opens /apps/dev/Workspace and returns Page Not Found (404) with the internal XML path. Root-specific links have the same unsupported target. Offer a working handoff to Material or the terminal. | Observed, standard-link-404.jpg. Home links and Workspace mode handoff. | **Fixed and checked 2026-11-25.** In the framework, a qvue-only screen asked for in html under `/apps` now redirects to the same path and query under `/qapps`, rather than a 404 naming the screen file. This fixes Home's links, the Standard menu item, bookmarks, and the other qvue-only Dev AIDE screens. The tui keeps its transition. Live: `/apps/dev/Workspace?rootId=mr-moqui-base` opened Material's Workspace at that root. Core screen test. |
| DEVW-UI-003 · P2 | review.dev.viewer, DEVW-13, Material Workspace, 1280×720 | Open the devw-git site root as the prepared viewer. Root and branch load, write actions are disabled, and the tree body shows the literal “[object Object]”. Explain the read failure with a usable message. The underlying viewer access decision still needs diagnosis. | Observed, viewer.jpg. Workspace tree error presentation. | **Fixed and checked 2026-11-25.** There were two causes. First, the View-tier reads whose verb is not view, find, get or search (list#Directory, check#File and nine others) were checked for every action, so a VIEW grant refused them. They now declare `authz-action="view"`; plan#EntityDdl, at the Full tier, is unchanged. Second, the shared m-file-tree printed the rejection object; it now shows the answer's errors or HTTP status. WorkspaceReader checks that a View-tier reader lists a root and that a user without the tier is refused. WebrootCode checks the message. A viewer walk on the hosted UI was not repeated. |

### Bounded batch record

For each later batch record:

- Intent, activities and branch limits; runtime/build/components, identities/grants, fixture IDs and dates.
- Observed steps and outcomes; evidence filenames, console/network/output observations and findings.
- Workarounds and blocked/deferred branches, with prerequisites and follow-up.
- In refinement only: decisions, owning files/components, correction, focused verification commands/results and retest.
- Temporary settings/grants/files/jobs restored, remaining cleanup and companion handoffs.

#### DEVW-B01 — 2026-11-25, first live review

- Checkout 6d8bb49f8a7c, coordinator’s isolated maximal runtime on port 18765. Home listed framework, webroot, basic-comms, tools, dev-aide and example. Chicago time. Browser hosts workspace.localhost and viewer.localhost separated the demo ADMIN and DEVW_VIEWER (DEVW_VIEWERS with DEV_AIDE_VIEW and review artifact grants).
- Home created round-review-site and round-review-app at 05:28 Chicago time. The reload log named each component.xml. The restarted runtime omitted the site with “it loads on every instance purpose but depends on component example, which loads only on some”. The generated app loaded. Site and app are independent disposable repositories; their authoring handoff uses the corrected site dependency.
- Git setup used a dedicated devw-git repository, separate from the generated app, and a local bare remote. Compared e5e6c6b on main with 7768f38 on compare. Saved scratch.txt, retained the peer revision after conflict, staged and committed it, then verified 2a2c81b in the bare remote. After the deliberately conflicting merge and Abort, Git reported clean main tracking origin/main. Removing the clean session worktree retained its named branch as the UI promised.
- Material, Standard and the browser-hosted terminal were exercised. The measured default viewport was 1280×720. The requested width overrides did not establish a measured 390px or 1920px workspace. Light/dark and outstanding tier, hosting, task, Assist and recovery branches remain unverified. No automated or acceptance suites ran.
- Local screenshot evidence is under `plans/ui-refinement-evidence/devw-20261125/`. The coordinator owns the isolated instance and retained fixture repositories.

### DEVW-B02 — November 25 continuation

review.dev.viewer, with DEV_AIDE_VIEW and the view grants, opened the copied round-review-app root through Standard. The redirect retained rootId. Material showed the locked root, data, entity, screen, service and test directories, and named files including component.xml. DEVW-UI-002 and the hosted viewer branch of DEVW-UI-003 passed. A separate live TUI session opened Workspace and exposed the Files tree, Go to file and Search controls. Evidence is in `workspace-viewer.json`, `workspace-viewer.jpg` and `tui.txt`.

John Doe connected Workspace to the second local server and found the disposable DB Git repository review-hosted. Workspace listed MR !1 from review-change to main. The README diff showed Base revision and Comparison revision. The review panel reported that the server keeps no review threads. Discussion persistence and an independent reviewer decision remain open. The localJobs and startLocalRun transitions ran the working tree’s review/readme job in a terminal. The job checked README.md and wrote a marker which was independently read. A hosted failed job, log and rerun remain open. See group-server-connected.json, hosted-file.json and ci-local.json.

Local evidence is under `plans/ui-refinement-evidence/closeout-d10e22f/`.

#### Remaining branches

The ledger records the strongest observed outcome. The remaining steps, recovery paths and modes in each section 4 activity are deferred to the fixture and next check below. An explicitly blocked or failed result keeps that status.

| Activity | Required setup and next observation |
|---|---|
| DEVW-01 | Create a site component under a non-admin creator and reopen it after reload. |
| DEVW-02 | Repeat edit/reload with an external file writer and inspect the downstream consumer. |
| DEVW-03 | Cause two edits to the same saved file and inspect the retained alternatives through resolution. |
| DEVW-04 | Stage one hunk as a contributor and verify the commit and linked task independently. |
| DEVW-05 | Recover the same fixture change after a branch switch, tag and conflicting stash application. |
| DEVW-06 | Resolve a diverged merge and verify both commits and resulting file through history. |
| DEVW-07 | Introduce remote divergence between two disposable clones and verify the recovery and push. |
| DEVW-08 | Switch between two task branches and reopen the linked commit from each task. |
| DEVW-09 | Disconnect or expire the local group credential and verify reconnect and repository visibility. |
| DEVW-10 | Use a thread-capable peer and second contributor for comment, reply, resolution and reviewer decision. |
| DEVW-11 | Supply a hosted failed job with a test location, follow its log and source, then rerun it. |
| DEVW-12 | Start an Assist task in a disposable worktree, review its proposed diff and confirm its disposition. |
| DEVW-13 | Repeat the same valid Workspace write across View, Edit and host-local grants through browser and TUI. |

## 7. Decisions

- IDs are `DEVW`; authoring uses `DEVA`. Workspace owns its XML transition host, while guided editor interactions
  belong to the authoring plan and are linked explicitly.
- All actual file/Git/publish/group-server/CI actions await a later review and target disposable fixtures only.
  No real repository merge, contributor agreement or remote message is part of this planning task.
- Live reload and session worktrees are distinct: unloaded worktrees are checked/committed there, not run live.

- Preparing or auditing these plans changes documentation only. Application/API changes, test runs, runtime walks
  and fixture creation belong to separate review/refinement execution.
- Current code, mounts and grants take precedence over older plans. Historical outcomes, permissions and exceptions
  are not inherited. Preserve routes, stored preferences and public contracts in any later refinement.
- Finish a review with explicit branch outcomes and a candid unresolved ledger; missing prerequisites do not become
  passes. Building absent features is outside this review.
