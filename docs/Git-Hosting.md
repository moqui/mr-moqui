# Git hosting

MR Moqui hosts the `moqui.resource.GitRepository` repositories of [Revision-Management](Revision-Management.md) for the
git command line and any git client: clone, fetch and push over HTTPS (smart HTTP) and over the built-in SSH server,
shallow clones for CI, branches and tags, and Git LFS. A push or a mirror fetch runs **gitECA rules**, the git
counterpart of email ECA rules, and Mantle uses them to tie commits, branches and tags to projects and tasks. gitoxide
has no server side; the protocol is ours (`crates/moqui-core/src/resource/git/server/`) over gitoxide's objects and
packs.

## Clone URLs

| | URL |
|---|---|
| HTTPS | `https://<host>/git/<repositoryId>.git` (`.git` is optional) |
| SSH | `ssh://<username>@<host>:<ssh_port>/<repositoryId>.git`, or `<username>@<host>:<repositoryId>.git` on port 22 |

`org.moqui.impl.GitServices.get#GitCloneUrls` gives both, from `git_public_url` (else the webapp host and port
properties), and `ssh_public_host` and `ssh_public_port` (else the webapp host and `ssh_port`). The SSH username is
the Moqui username. The kinds of repository behave as they do for `git://` locations:

- **`local`**: a bare repository on disk, the one for code. Clones reuse the deltas of its packs.
- **`db`**: objects in `GitObject` rows, kept with the database backup, compressed and as deltas of each other as a
  pack holds them; a clone copies them into its pack as they are ([Storage](#storage-of-db-repositories)). Large files
  are best kept in [Git LFS](#large-files-git-lfs).
- **`remote`**: a read-only mirror; it can be cloned and fetched, a push is refused. A fetch of the mirror takes every
  branch and tag and drops the ones the remote dropped.

A repository is created on the Git Repositories tools screen (System, Resource Finder), by
`mantle.work.GitServices.create#ProjectRepository` from a project's Repositories tab, or as a `GitRepository` row.
An empty repository can be cloned; the first push to one whose row names no branch makes `main` (or `master`, or the
first branch pushed) its default branch.

## Logging in

- **HTTPS**: HTTP Basic with the username and the password, or an **access token** or an **app password** (docs/Sync.md)
  as the password, which accounts with a second factor need (the password alone cannot complete a login there). `Authorization: Bearer <token>` and
  an `api_key` header work too. Credentials are taken only over HTTPS (the `webapp_https_listen_port` listener, or
  `X-Forwarded-Proto: https` behind the proxy that `webapp_client_ip_header` names), from loopback, or with
  `git-server` `allow-http="true"`. A request with no credentials gets 401, so git asks for them.
- **SSH**: a key (`moqui.security.UserSshKey`, matched by its SHA-256 fingerprint) or the password. The user needs the
  `SSH_GIT` permission (granted to ADMIN; grant it to the groups of developers). A user whose only SSH permission is
  `SSH_GIT` logs in by key unless `ssh-server` `git-password-auth="true"`. `git -c protocol.version=2` sends `GIT_PROTOCOL`,
  which the server reads from the channel's `env`.

My Account, Git Access (`MyAccount/User/GitAccess.xml` of SimpleScreens) adds and removes SSH keys and creates and
revokes access tokens, through `org.moqui.impl.UserServices` `create#UserSshKey`, `delete#UserSshKey`,
`create#UserAccessToken` (a `UserLoginKey` with a description; the token is shown once) and `revoke#UserAccessToken`.

## Access

Each repository is an artifact of type `AT_GIT_REPOSITORY`, named by its `repositoryId`, in the usual ArtifactGroup
and ArtifactAuthz rows:

| Action | Allows |
|---|---|
| `AUTHZA_VIEW` | clone, fetch, the `get#Git*` and `find#Git*` services, trial merges, reviews of merge requests |
| `AUTHZA_CREATE` | push a new branch or tag, `create#GitRef` |
| `AUTHZA_UPDATE` | push a fast-forward, `commit#GitFiles`, `merge#GitRefs`, `merge#MergeRequest` |
| `AUTHZA_DELETE` | force a push (a branch that does not fast-forward, a moved tag), delete a branch or tag, the repository's settings, branch rules and webhooks, archive |

Posting a commit status needs any of create, update or delete (a push right).

The seed data has the `GitRepositories` group (every repository, `.*`) with ALWAYS ALL for ADMIN. Mantle adds an
ALLOW ALL for ALL_USERS on it whose `authzServiceName` is `mantle.work.GitServices.check#GitRepositoryAuthz`, by the
user's party's current roles on the projects a repository is linked to: an internal member may view, create and update;
a lead (`ProjectManager`, `Manager`, `ProgrammerLead`, `SysArchitect`) also deletes; a client contact (`Customer`,
`CustomerBillTo`, `ClientManager`, `ClientBilling`) only views, and only when the project shares its repositories
(`WorkEffortSetting` `WstShareRepositories` Y). A user who may not view a repository gets 404 as if it were not there;
one who may view and not push gets 403 on a push.

A repository's rights are its own records' (`crate::user::authorize_own`): an inheritable authorization of the screen or
service that asks (an app root's ALLOW for its users) is not taken, so the code screens and services grant no more than
a clone does. The server
refuses to serve at all when `<artifact-execution type="AT_GIT_REPOSITORY" authz-enabled="true"/>` is not in the conf,
as every check would pass then. Reading and writing `git://` locations is not checked against this type.

The default branch cannot be deleted. Only `refs/heads/` and `refs/tags/` can be pushed. Branch rules
([below](#branch-rules)) protect refs further, and an [archived](#archive) repository takes no push.

A repository that holds a drive (`ResourceDrive.repositoryId`, the `DRIVE_<driveId>` repositories of version-controlled
drives) also takes the drive's own rule (`FileServices.get#DriveAccess`), for users with no `AT_GIT_REPOSITORY` grant: view
fetches, edit pushes new refs and fast-forwards on a drive that is not read only, manage forces a push or deletes a ref. The
`get#Git*` services follow it, so Files' drive settings show the clone addresses to every member.

## Protocol

- **Fetch**: protocol v2 (`ls-refs`, `fetch`) and v0/v1 (`multi_ack_detailed`, `no-done`, `side-band-64k`), stateful
  over SSH and stateless over HTTP; `shallow`, `deepen`, `deepen-relative` (`--deepen`), `deepen-since`
  (`--shallow-since`), `deepen-not` (`--shallow-exclude`) and `--unshallow`; `include-tag`; a want of any object the
  repository has (`git fetch --depth=1 origin <sha>`). No partial clone (`filter`) yet.
- **Push**: v0/v1 (v2 has no push), `report-status`, `side-band-64k`, `atomic`, `delete-refs`, `push-options`. The
  pack is indexed into a quarantine directory, completed from the repository when it is thin, and every object it
  refers to must be in it or the repository; only an accepted push moves it in (a `local` repository keeps the pack in
  `objects/pack`, a `db` one writes each object as a row). Each ref is then moved by compare-and-set: a ref another
  push moved first is refused with `stale info`.
- Clones and pushes run on a pool of their own (`git-server` `max-concurrent`, 4, and `git-server` `max-queued` waiting), not on the
  request workers of the screens, and are exempt from `webapp_request_timeout`; `git-server` `idle-timeout` (120 seconds) of
  silence in a request body ends it. Limits: `git-server` `max-request` (64 MB) for a fetch request, `git-server` `max-push-size` (2 GB),
  `git-server` `max-object-size` (512 MB) for one pushed object; `git-server` `pack-threads` (all cores when empty) compress and index.

## gitECA rules

A `*.gitecas.xml` file in a component's `service` directory (schema `framework/xsd/git-eca-3.xsd`):

```xml
<gitecas>
    <giteca rule-name="NoWip" event="pre-receive">
        <condition><expression>newCommits.some(c => c.summary.startsWith('WIP'))</expression></condition>
        <actions><message error="true">WIP commits are not accepted</message></actions>
    </giteca>
    <giteca rule-name="ProtectMain" event="pre-ref-update" ref-pattern="refs/heads/main">
        <condition><expression>changeType == 'force'</expression></condition>
        <actions><message error="true">main cannot be forced</message></actions>
    </giteca>
    <giteca rule-name="LinkCommitTasks" event="commit">
        <actions><service-call name="mantle.work.GitServices.link#CommitReferences" in-map="context"/></actions>
    </giteca>
</gitecas>
```

| Event | Runs | An error |
|---|---|---|
| `pre-receive` | once per push, before any ref changes, with `newCommits` | refuses the push |
| `pre-ref-update` | once per ref of a push, before it changes | refuses that ref (every ref of an `atomic` push) |
| `ref-update` | once per ref that changed, with `newCommitIds` (the commits it gained) | is recorded |
| `commit` | once per commit new to the repository, oldest first, with the ref it came on | is recorded |
| `tag` | once per tag created | is recorded |
| `post-receive` | once per change, after the others | is recorded |

| Attribute | |
|---|---|
| `source` | `push`, `fetch` (of a `remote` mirror), `import`, `write` (a commit through `git://` or `commit#GitFiles`, such as a wiki save, and a ref written by `create#GitRef`, `delete#GitRef` or a merge); `push,fetch,import` when empty |
| `repository-pattern`, `ref-pattern` | regular expressions the whole repositoryId, or ref name (`refs/heads/main`), must match |
| `ref-type` | `branch` or `tag` |
| `default-branch-only` | the repository's default branch only (a commit event: a commit that came on it) |
| `priority`, `id`, `run-on-error` | as for service ECA rules: lower first; a later rule with the same id replaces this one; run even after a rule of the same change failed |

The context of every event has `repositoryId`, `source`, `transport` (`http`, `ssh`, `internal`), `userId`,
`updateId`, `pushOptions` and `refUpdates` (each `refName`, `refType`, `shortName`, `oldId`, `newId`, `changeType` of
`create`, `update` (a fast-forward), `force` or `delete`, `isDefaultBranch`); a per-ref event those fields of its ref;
a commit event `commit` (`commitId`, `shortId`, `summary`, `body`, `message`, `trailers` by name, the author and
committer and their dates, `parentIds`, `moquiUserId`); a tag event `tag` (`tagName`, `targetId`, `commitId`,
`annotated`, and an annotated tag's `message` and tagger). `org.moqui.GitServices.process#GitEca` is the interface.

The pre events run in the push, against its quarantined objects: their messages, and the reasons of a refusal, show
on the client as `remote:` lines. The post events run after the change, on a job of the git pool, from its
`moqui.resource.GitUpdate` row (with a `GitUpdateRef` per ref), each rule in a transaction of its own; one that fails
is logged and recorded on the row (`ecaStatus` failed, `ecaError`), and the others still run. The updates of one
repository run one at a time, in order. Every push is recorded, accepted or not, so the rows are also its push log
(the tools screen of the repository lists them, and runs a done or failed one again). A change's commit events stop at
`git-server` `eca-max-commits` (10,000) new commits. The `process_PendingGitUpdates_frequent` job runs what is left pending (a
full pool, a restart) and finishes a push a restart cut short. Rules run with authorization off: they are a
component's own code.

The framework has rules of its own (`framework/service/Git.gitecas.xml`, ids `FrameworkRefreshMergeRequests` and
`FrameworkWebhookPush`, on every source, run even after another rule failed): merge requests follow their branches, and
webhooks get pushes and tags ([Hosting through Moqui](#hosting-through-moqui)).

## Hosting through Moqui

The services of `org.moqui.impl.GitServices` (Rust: `crates/moqui-core/src/service/git_hosting.rs` over
`resource/git/hosting.rs`, `merge.rs` and `rules.rs`) and `org.moqui.impl.GitReviewServices` serve the code screens of
an application (IronStone Projects' Code). Each checks the repository's rights as the git server does; the rows they
keep (`GitCommitStatus`, `GitWebhook`, `GitBranchRule`, `GitMergeRequest`, `GitMergeRequestReviewer`) are `moqui.*`
entities that entity filters do not reach, so a screen reads them only through these services. All of it is
Mantle-free: Mantle links merge requests to work items and comments to lines.

### Browsing

| Service | |
|---|---|
| `find#GitRepositories` | the repositories the user may view, paged, by id list, text and kind; archived ones only with `includeArchived` or `archivedOnly`; each with its open merge requests |
| `find#GitUpdates` | a repository's push log, newest first: every push (refused ones too), mirror fetch, import and write, its refs and the state of its gitECA rules |
| `get#GitTree` | a directory at a revision, directories first, each entry with its size (a file in LFS: its content's) and last commit, from the path index of the branch the revision names (or of the commit) |
| `get#GitRefs`, `get#GitLog`, `get#GitCommit`, `get#GitCommitDiff`, `get#GitBlob` | as before ([Revision-Management](Revision-Management.md)) |

### Refs, compare and merge

`create#GitRef` makes a branch, or a tag (annotated by the current user when given a message), at a revision;
`delete#GitRef` deletes one. `get#GitCompare(base, head, withDiffs, checkMergeable)` answers as GitHub's compare: the
merge base, the commits in head and not in base (oldest first, at most 250), ahead and behind, the files changed from
the merge base to head with their line counts, and with `withDiffs` each file's diff (at most 300 files) as the
`moqui-diff` html and as hunks of numbered lines (`oldNo`, `newNo`) for a review to anchor to.

`merge#GitRefs(source, target, method, message)` merges a revision into a branch as the current user, by gix-merge
(gix's `merge` feature: git's `merge-ort` rules, renames followed, both sides' changes to one file merged by lines):

| `method` | Makes |
|---|---|
| `GmmMerge` (default) | a merge commit, its parents the target and the source, also when a fast-forward was possible |
| `GmmFastForward` | the target moved to the source; refused when the target has commits the source lacks |
| `GmmSquash` | one commit on the target with the merged tree, its author the merge request's author when merged from one |
| `GmmRebase` | each commit of the source not in the target replayed on it, oldest first, keeping its author and message (a merge commit of the source is not replayed; a commit whose changes the target has is dropped) |

A merge is refused on a conflict, listing the paths (`conflictPaths`); `checkOnly` is a trial merge that changes
nothing. The merged objects are made apart from the repository and written only when the target moves.

Every ref written through Moqui (`create#GitRef`, `delete#GitRef`, `merge#GitRefs`, `merge#MergeRequest`) moves under
the repository's lock as a push does: the branch rules, the pre events of [gitECA rules](#giteca-rules) of source
`write` (an error refuses it), a `GitUpdate` in the push log, and the post events after it (so Mantle's link rules and
the framework's own rules follow).

### Branch rules

A `GitBranchRule` protects refs of a repository: `refPattern` (one that starts with `refs/` is matched against the full
ref name, any other against a branch's name; `*` is any run of characters), `allowForcePush`, `allowDelete`,
`requireMergeRequest`, `requiredApprovals`, `dismissStaleApprovals` (a new head of the source sets its approvals back
to requested), `requiredStatusContexts`, `allowedMergeMethods` and `pushUserGroupId` (only its members push directly).
The git server enforces them before a pushed ref moves (the refusal shows on the client, `protected branch: main changes
through a merge request`), and so do the ref services: they are built in, not a gitECA rule a component could remove.
Every rule that names a ref applies. A write through `git://` (a wiki save, release notes) by a user with the update
right passes and is logged. `store#`, `delete#GitBranchRule` and `get#GitBranchRules` (what the rules of a ref ask
together, and whether the user may push to it directly).

### Archive

`archive#GitRepository` (delete right) sets `archivedDate`: the repository is read only (pushes are refused with 403,
ref writes, merges and `git://` writes with an error), its open merge requests are closed, and `find#GitRepositories`
leaves it out unless asked. `unarchive#GitRepository` makes it writable again. `update#GitRepository` changes the
description, the default branch and the default merge method (`defaultMergeMethodEnumId`, `GmmMerge` or `GmmSquash`).

### Commit statuses

Outside CI posts the result of a check of a commit, in GitHub's status shape:

```
POST /rest/s1/moqui/git/{repositoryId}/commits/{commitId}/statuses
Authorization: Bearer <access token>
{"state": "success", "target_url": "https://ci.example.com/run/7", "description": "Build", "context": "ci/build"}
```

`state` is pending, success, failure or error; the commit must be in the repository and the user (a CI account with an
access token) needs a push right on it. The REST path is open to every user (`MOQUI_API_GIT`); the service checks the
repository. `GET` on the same path, and `get#GitCommitStatuses` (one commit or many), answer the latest status of each
context and the combined state (failure when any failed or erred, pending when any is pending, success when all
succeeded).

### Webhooks

A `GitWebhook` of a repository (delete right: `store#`, `delete#`, `find#GitWebhooks`) calls an outside URL on its
events: `push` (a branch changed), `tag`, `merge_request` (opened, updated, ready, reviewed, synchronize, closed,
reopened, merged), all when none is named. The body is JSON (`event`, `ref`, `before`, `after`, `created`, `deleted`,
`forced`, `repository`, `pusher`, `commits` with the last 20 and `total_commits`; a merge request's `action` and
`merge_request`); the headers `X-Moqui-Event`, `X-Moqui-Delivery` and, with a secret (kept encrypted),
`X-Hub-Signature-256: sha256=<the HMAC-SHA256 of the body>`, as GitHub signs, so CI servers that check GitHub's
signature check it unchanged. Each delivery is a SystemMessage of type `GitWebhook` sent at once
(`send#GitWebhookDelivery`); a status that is not 2xx, or no answer in 10 seconds, leaves it to
`retry#GitWebhookDeliveries` (the `retry_GitWebhookDeliveries_frequent` job, each minute), which tries again after 1, 2,
4 ... minutes and gives up after 8 failures. `find#GitWebhookDeliveries` is the delivery log, each attempt's error kept
(`SystemMessageError`); `test#GitWebhook` sends a `ping` now. A webhook URL that names, or resolves to, a loopback,
private or link-local address is refused unless `git-server` `webhook-private-network="true"` (a CI server on the same
network). Pushes and tags are queued by the framework's post-receive rule (`framework/service/Git.gitecas.xml`), merge
request events by the review services.

### Merge requests

`org.moqui.impl.GitReviewServices` keeps a merge request of a source branch into a target branch of one repository
(`moqui.resource.GitMergeRequest`, numbered per repository from 1, statuses `GmrDraft`, `GmrOpen`, `GmrMerged`,
`GmrClosed`) and its reviewers (`GitMergeRequestReviewer`: `GmrrRequested`, `GmrrApproved`, `GmrrChangesRequested`,
`GmrrCommented`, each with the head it was given on):

| Service | |
|---|---|
| `create#MergeRequest` | by a user who may push; one open request per source and target; head, merge base and mergeability read now |
| `update#MergeRequest` | title, description, target, draft (marked ready), merge method, delete the source after; by the author or a user with update |
| `set#MergeRequestReviewers` | replace, add or remove reviewers (not the author) |
| `submit#MergeRequestReview` | approve, request changes or comment, at the current head; only a user who may push (update) approves, a viewer comments or requests changes; the author does not approve their own |
| `close#`, `reopen#MergeRequest` | |
| `merge#MergeRequest` | by its method (given, its own, else the repository's default) once the target's branch rules are met: the approvals (of the current head when stale ones are dismissed; only those of users who may push now count, each decided as its reviewer), the required checks successful on the head, a method they allow; update right; the source branch deleted when asked |
| `get#MergeRequest` | the request, its reviewers, the head's checks, the target's rules and what keeps it from merging now (`missingList`), and what the user may do |
| `find#MergeRequests` | of the repositories the user may view: by repository, status, author, reviewer, waiting for a user's review, target, title |
| `get#MergeRequestApprovals` | the approvals that count toward the branch rules, as `merge#MergeRequest` counts them |
| `get#MergeRequestDiff` | its commits and diffs as `get#GitCompare` gives them |

`refresh#MergeRequests`, the framework's ref-update rule (`Git.gitecas.xml`), keeps them current on every change of a
branch, from any source: a source's new head is recorded (its approvals dismissed when the target's rules say so;
webhooks get `synchronize`), each request of the branch is checked for mergeability again (`GmaMergeable`,
`GmaConflict` with the paths, `GmaUnchecked` while a branch is gone), and a request whose head the target holds is
merged (as when it was merged by a push).

## Projects and tasks

In Mantle (`mantle.work.GitServices`, the rules of `mantle-usl/service/Work.gitecas.xml`, and the screens of
SimpleScreens), a project's Repositories tab links repositories to it (`WorkEffortGitRepository`), or makes a new one.
In a repository linked to a project:

| | Links |
|---|---|
| A task ID of the project as a word of a commit message (`PROJ-100023`) | the commit to the task (`WorkEffortGitCommit`). A word is looked up, so `UTF-8` never matches |
| A `Task: <workEffortId>` trailer | the commit to that task, in any project |
| `Fixes`, `Closes` or `Resolves` before a task ID | completes and resolves the task (`WeComplete`, `WerCompleted`) when the commit reaches the default branch: only commits made after the repository was linked, and only where the task's status flow has a way to Complete |
| A branch named `PROJ-100023` or `PROJ-100023-login-fix` (its last path segment, or the part of it before a `-`) | the branch to the task (`WorkEffortGitRef`) |
| A tag named `<workEffortId>` or `<workEffortId>/<anything>`: `PROJ/v1.2.0`, `PROJ-MS-01/rc1`, `PROJ-100023/review` | the tag to that project, milestone or task; an annotated tag's message is read for task IDs as a commit's is |
| A tag named as a release's version, or `v` and the version (`1.4.0`, `v1.4.0`) | the tag to that release (`WetRelease` named so) of a linked project |
| A merge request whose source branch is named for a task, or whose title or description names one | the merge request to the task (`WorkEffortGitMergeRequest`); `Closes PROJ-12` there, or the project's `WstMergeRequestMoves`, completes it when the merge request merges |

Deleting a branch or tag ends its links (`thruDate`). The rules run on pushes, mirror fetches (so a repository hosted
elsewhere and mirrored here links too), imports and commits made through Moqui. Merge requests follow the rules of
`mantle-usl/service/WorkGit.secas.xml`, after the framework's review services: opened and edited ones link their
tasks (`link#MergeRequestItems`), an opened or ready one moves them to In Review when the project's
`WstMergeRequestMoves` is Y (`move#MergeRequestItemsToReview`), a merged one completes them (`complete#MergeRequestItems`), reviewers and authors are told (topics `MergeRequestReview`,
`MergeRequestUpdate`), and a failed check tells the commit's author (`CommitStatusFailed`). `create#TaskBranch` makes a
task's branch by the project's `WstBranchPattern`; review comments on a line are `CommunicationEvent`s with a
`CommunicationEventGitAnchor` (`create#ReviewComment`, `resolve#ReviewThread`, `get#ReviewThreads`). A task's Source tab links commits,
branches and tags by hand; its summary shows them, and the project summary the repositories and tags.
`Project/GitCommit` shows a commit: its message, people, parents, linked tasks, and each changed file's diff.

## Storage of db repositories

A `GitObject` row holds an object as a pack does (`storage`):

| `storage` | `objectData` |
|---|---|
| `zlib` | the zlib stream of the content |
| `delta` | the zlib stream of git delta instructions that make the content from `deltaBaseId`, an object of the same repository, at most `git-server` `db-delta-depth` (50) deltas from a whole one |
| `blob` | nothing: an object over `git-server` `db-large-object` bytes (8 MiB) is the [content blob](#content-blobs) `blobId` |
| empty | the content itself, as rows written before this format were; still read, and compressed by the next collection |

A push keeps the deltas the client sent, and its entries' compressed bytes, as they are (a commit or tag, and a delta
deeper than the limit, is stored whole); a large object is inflated into its content blob as the pack is read. A clone
or fetch copies the rows into its pack: a delta as `OFS_DELTA` when its base is in the pack (or `REF_DELTA` without
`ofs-delta`), as a delta against an object the client has in a thin pack, and applied otherwise; a content blob is
compressed into the pack a window at a time. Reads go a batch of rows at a time, a delta chain a level of bases per
query, and each object is checked against its id as it is decoded; `git-server` `db-cache-bytes` (64 MiB) of decoded objects
are kept for all repositories. `import#GitRepository` and `export#GitRepository` go through a pack as a push does.
`git-server` `db-write-format="raw"` writes rows in the old form, for a database an older version also reads.

`gc#GitRepositories` (the `gc_GitRepositories_daily` job, 03:30) collects each `db` repository, holding its lease
(`gcLeaseOwner`) so only one run rewrites its rows:

1. It marks what the refs reach, and what may still be needed: the tips of a change whose gitECA events have not run,
   the old and new ids of the changes of the last `git-server` `gc-reflog-days` (30) days (a force push can be undone), and every
   row written or referenced again in the last `git-server` `gc-prune-days` (14) days. A ref that reaches a missing object stops
   the run: a damaged repository is never swept (`fsck#GitRepository` names what is wrong).
2. It deletes the other rows, deepest delta first, each only while it is still that old: a push that reuses an old
   object while the run goes refreshes its stamp, and keeps it.
3. It repacks while its time (`timeoutMinutes`, 60) lasts: the versions of each path, newest first, the newest whole
   and each older one a delta of a newer one when that saves a quarter of its bytes; rows of the old form compressed;
   chains deeper than the limit made shorter. Each new delta is applied and checked against the object's id first.
4. It lets go of the LFS objects no kept pointer names, uploaded before the cutoff, and records `objectCount`,
   `storedBytes`, `lastGcDate` and `lastGcResult` on the repository. `dryRun` counts and changes nothing.

The tools screen of a `db` repository shows its storage and runs a collection (a dry run by default) and the check.

Measured on embedded Postgres (debug build, `cargo test -p moqui-host --test it -- --ignored git_storage
--nocapture`):

| | Objects | Content | git's pack | Stored, imported | After a collection | Clone |
|---|---|---|---|---|---|---|
| mr-mantle-usl's history | 13,435 | 195 MiB | 8.25 MiB | 8.37 MiB (1.02x) | 7.88 MiB (0.96x) | 3 to 5 s |
| mr-moqui's history | 11,404 | 160 MiB | 10.3 MiB | 25.6 MiB | 10.4 MiB (1.01x) | 2 to 3 s |
| 1000 `git://` saves of a 50 KB page | about 3,000 | about 49 MiB | | 2.72 MiB | 0.30 MiB | |

Before this format the rows held the content (the Content column). A 1 GiB file is stored as it is read (55 MiB/s)
and sent at about 270 MiB/s, the process's peak memory rising about 180 MiB; a 64 KiB range of it takes about 50 ms.

## Content blobs

Large content is a `moqui.resource.ContentBlob`, by the SHA-256 of its bytes, in `ContentBlobChunk` rows of
`resource-facade` `blob-chunk-size` (1 MiB) written a chunk per transaction: Git LFS objects, the large objects of `db` repositories, and
`dbresource://` files over 64 KiB. The same bytes are stored once; two uploads of them at once, or a crash halfway
(`ContentBlobUpload`), leave one blob and no stray chunks. A row refers to a blob by a relationship to `ContentBlob`,
and `gc#ContentBlobs` (`org.moqui.impl.ResourceServices`, the `gc_ContentBlobs_daily` job, 04:30) deletes the blobs no
row refers to and that were not written or referenced again in the last `resource-facade` `blob-gc-prune-days` (14) days.

File responses stream (`sendResourceResponse`, a screen's static files): a file over 256 KiB is read a window at a time
as the client takes it, on a thread of its own (at most `webapp` `transfer-workers`, 16, reading the database at once), so
a slow download holds no request worker. One `Range` is answered from the bytes it names, with `ETag` (a git file's
blob id, a content blob's SHA-256) and `If-Range`. Only text types are compressed. An upload is read as it arrives: a
file part over 256 KiB goes to a file in `runtime/tmp/upload`, and from there to its storage (a content blob, a git
object) as it is read; `webapp_max_upload` (2 GiB) bounds it and `webapp_upload_idle_timeout` (60 seconds) of silence
ends it.

## Large files (Git LFS)

git-lfs clients push and fetch large files at `/git/<repositoryId>.git/info/lfs` (the Batch API with the basic
transfer, and the locks API): `git lfs install`, `git lfs track "*.psd"`, then push and clone as usual. Over SSH,
git-lfs runs `git-lfs-authenticate`, which gives it that HTTPS endpoint and a token for an hour (`git-lfs-transfer` is
refused, so it does). An object is the content blob of its SHA-256, so the same file in many repositories is stored once, but a
repository has an object (`moqui.resource.GitLfsObject`) only once it was uploaded to it: knowing an oid does not reach
another repository's file. Uploads need the push rights of the repository, downloads view; a lock needs push rights,
and force unlocking another user's lock `delete`. The batch's actions carry a token signed with `git_lfs_token_key`
(generated into `runtime/.env`; `git_lfs_token_key_previous` still reads while it is changed) that says who the user
is, for 15 minutes, and each request is authorized again. Objects are at most `git-server` `lfs-max-object-size` (5 GiB).

Moqui itself writes LFS files through `git://` locations: a file the `.gitattributes` of the branch head keep in LFS
(`filter=lfs`) is stored as a content blob, linked to the repository, and committed as its pointer; a read gives the
content, streamed for a response, and its size is the content's. `setup#GitLfs(repositoryId, patterns)` adds the
patterns to `.gitattributes` (images, video, audio, PDFs and archives when empty); a content repository gets them when
it is made (`init#GitRepository` with `contentRepository`, the tools screen's Content option, and the Content option of
a project's new repository). A web site's images and their variants (`convert#Image` writes an image and its sizes in
one commit) can so live in a `db` repository, with their history, in the database.

## Limits

- Merges are of branches of one repository: no forks. A merge reads no `.gitattributes` (no merge drivers, every file
  merged as text unless it is binary); revisions are names and ids, not `main~1`.
- The framework's rules make a `GitUpdate` of every `git://` write (a wiki save), to keep merge requests and webhooks
  current.

- No push from a repository to another server; publish a `db` repository with `export#GitRepository`.
- A push made directly to a `local` repository's directory on the host runs no gitECA rules.
- Each push to a `local` repository adds a pack; there is no repack of those yet (run `git repack -ad` in the
  directory). `gc#GitRepositories` is for `db` repositories.
- `local` and `remote` repositories are on one node's disk: in a cluster send `/git` and SSH to that node, or share
  the directory. `db` repositories and content blobs are in the database, for every node.
- A `remote` mirror does not fetch the LFS objects of the repository it mirrors.
- SHA-1 only; no partial clone, signed pushes (`push-cert`), `report-status-v2`, the dumb HTTP protocol, or LFS
  transfer adapters other than basic.
