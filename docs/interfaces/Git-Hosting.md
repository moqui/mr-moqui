# Git hosting

MR Moqui hosts the `moqui.resource.GitRepository` repositories of [Revision-Management](../data/Revision-Management.md) for the
git command line and any git client: clone, fetch and push over HTTPS (smart HTTP) and over the built-in SSH server,
shallow clones for CI, branches and tags, and Git LFS. A push or a mirror fetch runs **gitECA rules**, the git
counterpart of email ECA rules, and Mantle uses them to tie commits, branches and tags to projects and tasks. gitoxide
has no server side; the protocol is ours (`crates/moqui-git/src/server/`) over gitoxide's objects and
packs.

## Clone URLs

| | URL |
|---|---|
| HTTPS | `https://<host>/git/<path>.git` (`.git` is optional) |
| HTTPS, on a public-site host | `https://<host>/<path>` or `https://<host>/<path>.git`: the page's URL |
| SSH | `ssh://<username>@<host>:<ssh_port>/<path>.git`, or `<username>@<host>:<path>.git` on port 22 |

`<path>` is the repository's `repositoryPath` (`moqui/mr-moqui`), or its `repositoryId`, which always works too and is
the only address of a repository with no path. `org.moqui.git.GitServices.get#GitCloneUrls` gives the HTTPS and SSH
URLs with the path (and the path itself, `repositoryPath`), from `git_public_url` (else the webapp host and port
properties), and `ssh_public_host` and `ssh_public_port` (else the webapp host and `ssh_port`). The SSH username is
the Moqui username.

### Repository paths

`repositoryId` stays the key of every row that names a repository; the path is what URLs carry, so a rename or a move
changes one field.

- **A path** is one to eight segments of letters, digits, `.`, `_` and `-`, at most 255 characters. No segment starts
  with `.` or `-` (so never `.` or `..`, and never the `-` that comes before a section in a page's URL) or ends in
  `.git` or `.lock`. A request whose path is not one names no repository, and is answered as one that is not there.
- **Case.** Paths that differ only in case are one path. `repositoryPath` is kept as it was written, for URLs and
  pages; `repositoryPathKey`, the path in lower case, is what is unique and what a request is looked up by, so
  `git clone https://host/moqui/PopCommerce` finds `moqui/popcommerce`, and no repository can take `Moqui/mr-moqui`
  beside `moqui/mr-moqui`. Ids are exact.
- **`set#GitRepositoryPath(repositoryId, repositoryPath)`** sets it, or clears it with an empty path, and is the only
  writer of the two fields (the Git Repositories tools screen does not write them; a data file that sets
  `repositoryPath` sets `repositoryPathKey`, its lower case, too). It needs the update right. Refused: a path that is
  not one, another repository's path in any case, and a path of one segment other than the repository's own id.
  `find#GitRepositories` gives each `repositoryPath` and searches it; webhook payloads carry it as `full_name` (its
  last segment as `name`) and in `clone_url`.
- **One segment is an id's.** A request of one segment is first the repository with exactly that id. As no repository
  has a one-segment path other than its own id, no path can stand in front of an id, of a repository there now or
  made later (a drive's `DRIVE_<driveId>`, say). An application's paths have two or more segments.
- **Looking a path up**, over HTTP, SSH and LFS alike: for one segment, the repository with exactly that id; else the
  repository whose `repositoryPathKey` is the path in lower case; else the repository that had that path before
  (`moqui.resource.GitRepositoryPathRedirect`, by `oldPathKey`). Each is one indexed read per request, with no cache,
  so a rename shows on every node at once.
- **After a rename** the old path still reaches the repository: `set#GitRepositoryPath` keeps it as a redirect row
  (`oldPathKey`, `oldPath`, `repositoryId`, `createdDate`). The server answers a request at an old path with the
  repository itself, not an HTTP redirect: git follows a redirect only on its first request, and SSH has none. The
  repository's own rights decide, so an old path of a repository the caller may not view answers as a path that names
  nothing. **A redirect ends when another repository takes the old path**: the path is that repository's from then
  on, and clones that still use it reach the new repository. Rows are not removed by time.
- **No namespaces here.** The framework knows paths, not groups or owners: any user with the update right on a
  repository may give it any free path, under any first segment, and a path is free the moment its repository leaves
  it. Who may rename or move a repository, which first segments an account may use, and whether or when a path that
  was left may be taken again are the application's rules (Mantle's and Smithy's), checked before it calls the
  service.
- **A request under `/git/`** is split at the first `.git/`; without `.git`, before `/info/refs`, `/git-upload-pack`,
  `/git-receive-pack` or `/info/lfs/`.
- **At a page's URL.** On a host of a `<public-site>` route (mr-pass's sites), a request is git's, and is served as
  the same request under `/git/`, when its path is `<path>/info/refs` with `?service=git-upload-pack` or
  `git-receive-pack` (a GET), `<path>/git-upload-pack` or `<path>/git-receive-pack` (a POST), or has
  `<path>.git/info/lfs/` (git-lfs adds the `.git`), where `<path>` is a path as above (with or without `.git`) whose
  first segment is not `git` or another framework route (`apps`, `rest`, `Login` and the rest). So
  `git clone https://moqui.org/moqui/mr-moqui` works, and every other request of the host is the site's page. A
  repository that is not there is answered as under `/git/` (401 or 404), not with the site's page. On any other
  host these paths are not git. A page request pays three suffix tests for this. Web protection checks such a request
  by the path it came with, and its path rules again by the `/git/...` path it is served at, so a rule that denies
  `/git/**` stops it too.
- **Ids that git URLs no longer reach.** An id is reached by its own name only when it is a path of one segment. A
  repository whose id has another character (a space, `+`, `@`, `:`, a letter outside ASCII), starts with `-` or
  `.`, or ends in `.git` or `.lock` is not reached by a git URL any more (before, any id without a `/` that did not
  start with `.` was). Give it a path of two or more segments; the `get#Git*` services and `git://` locations still
  take its id.

The kinds of repository behave as they do for `git://` locations:

- **`local`**: a bare repository on disk, the one for code. Clones reuse the deltas of its packs.
- **`db`**: objects in `GitObject` rows, kept with the database backup, compressed and as deltas of each other as a
  pack holds them; a clone copies them into its pack as they are ([Storage](#storage-of-db-repositories)). Large files
  are best kept in [Git LFS](#large-files-git-lfs).
- **`remote`**: a read-only mirror; it can be cloned and fetched, a push is refused. A fetch of the mirror takes every
  branch and tag and drops the ones the remote dropped.

A repository is created on the Git Repositories tools screen (System, Resources), by
`mantle.work.GitServices.create#ProjectRepository` from a project's Repositories tab, or as a `GitRepository` row.
An empty repository can be cloned; the first push to one whose row names no branch makes `main` (or `master`, or the
first branch pushed) its default branch.

## Logging in

- **HTTPS**: HTTP Basic with the username and the password, or an **access token** or an **app password** (docs/interfaces/Sync.md)
  as the password, which accounts with a second factor need (the password alone cannot complete a login there). `Authorization: Bearer <token>` and
  an `api_key` header work too. Credentials are taken only over HTTPS (the `webapp_https_listen_port` listener, or
  `X-Forwarded-Proto: https` behind the proxy that `webapp_client_ip_header` names), from loopback, or with
  `git-server` `allow-http="true"`. A request with no credentials is served what anyone may view
  ([Access](#access)) and gets 401 for everything else, so git asks for them.
- **SSH**: a key (`moqui.security.UserSshKey`, matched by its SHA-256 fingerprint) or the password. The user needs the
  `SSH_GIT` permission (granted to ADMIN; grant it to the groups of developers). A user whose only SSH permission is
  `SSH_GIT` logs in by key unless `ssh-server` `git-password-auth="true"`. `git -c protocol.version=2` sends `GIT_PROTOCOL`,
  which the server reads from the channel's `env`.
- **SSH for any account**: with `ssh-server` `git-any-account="true"` (off by default) an account that logs in by key
  uses git over SSH without `SSH_GIT`: a public site's accounts, which are in no group and so can hold no
  permission. Nothing else changes: each repository's own check ([Access](#access)) decides what the account
  clones and pushes, so one it may not view is "not found"; a password login still needs `SSH_GIT` and
  `git-password-auth`; the shell still needs `SSH_SHELL` and the SFTP root `SSH_SFTP`. (SFTP to the drives an
  account may open is `dav-server` `sftp-drives`, on by default and apart from this.) An account adds its key with
  `create#UserSshKey`, so "who may add a key" is who may sign in.
- **SSH sessions.** At most `ssh-server` `max-sessions` (8) connections are logged in at once; of those one user
  logged in by key or password holds at most `max-sessions-per-user` (4) and one address (an IPv6 /64)
  `max-sessions-per-source` (8), so one account, or one host, cannot hold them all; 0 is no share. A git command
  holds its connection only while it runs. A server that takes git over SSH from the public raises `max-sessions`
  to what it should serve at once (hundreds), keeps `max-sessions-per-user` small, and raises
  `max-sessions-per-source` where many users come from one address (an office, a CI provider); the git pool
  (`git-server` `max-concurrent`) still bounds the work done at once.

An account page adds and removes SSH keys and creates and revokes access tokens through
`org.moqui.impl.UserServices` (My Account, Git Access of mr-marble; BOSS's Git Access): `create#UserSshKey`,
`delete#UserSshKey`, `create#UserAccessToken` (a `UserLoginKey` with a description; the token is shown once) and
`revoke#UserAccessToken`, and lists them with `find#UserSshKeys` (each `sshKeyId`, `description`, `keyType`,
`fingerprint`, `fromDate`, `thruDate`; when a key was last used is not kept) and `find#UserAccessTokens` (each
`tokenId`, `description`, `fromDate`, `thruDate`). Both list the current user's rows only, the ones in date unless
`includeEnded`, and never a public key, a token or a token's stored hash: `tokenId` is a digest of that hash, which
`revoke#UserAccessToken` takes.

## Access

Each repository is an artifact of type `AT_GIT_REPOSITORY`, named by its `repositoryId`, in the usual ArtifactGroup
and ArtifactAuthz rows:

| Action | Allows |
|---|---|
| `AUTHZA_VIEW` | clone, fetch, the `get#Git*` and `find#Git*` services, trial merges, reviews of merge requests, a push to `refs/for/<branch>` and a proposal of `commit#GitFiles` ([Push to review](#push-to-review)) |
| `AUTHZA_CREATE` | push a new branch or tag, `create#GitRef` |
| `AUTHZA_UPDATE` | push a fast-forward, `commit#GitFiles` to a branch, `merge#GitRefs`, `merge#MergeRequest`, a merge request from a branch of the repository into another repository of its fork network, the whole push log and commits by id that no ref reaches ([Reachability](#reachability)) |
| `AUTHZA_DELETE` | force a push (a branch that does not fast-forward, a moved tag), delete a branch or tag, the repository's settings, branch rules and webhooks, archive, `set#MergeRequestHidden`, `detach#MergeRequestSource` |

Posting a commit status needs any of create, update or delete (a push right); a context a hosting application
reserves (`git-server` `reserved-status-contexts`, none by default) is posted with authorization disabled only
([Commit statuses](#commit-statuses)).

The seed data has the `GitRepositories` group (every repository, `.*`) with ALWAYS ALL for ADMIN. Mantle adds an
ALLOW ALL for ALL_USERS on it whose `authzServiceName` is `mantle.work.GitServices.check#GitRepositoryAuthz`, by the
user's party's current roles on the projects a repository is linked to: an internal member may view, create and update;
a lead (`ProjectManager`, `Manager`, `ProgrammerLead`, `SysArchitect`) also deletes; a client contact (`Customer`,
`CustomerBillTo`, `ClientManager`, `ClientBilling`) only views, and only when the project shares its repositories
(`WorkEffortSetting` `WstShareRepositories` Y). A user who may not view a repository gets 404 as if it were not there;
one who may view and not push is answered, and each ref they push is refused (`permission denied`) but
`refs/for/<branch>` ([Push to review](#push-to-review)); with `git-server` `review-push="false"`, and for a drive's
viewer, such a push gets 403 as before.

A repository's rights are its own records' (`crate::user::authorize_own`): an inheritable authorization of the screen or
service that asks (an app root's ALLOW for its users) is not taken, so the code screens and services grant no more than
a clone does. The server
refuses to serve at all when `<artifact-execution type="AT_GIT_REPOSITORY" authz-enabled="true"/>` is not in the conf,
as every check would pass then. Reading and writing `git://` locations is not checked against this type.

### No credentials

An HTTP request with no credentials (no `Authorization`, `api_key` or `login_key` header) is not refused before the
repository is looked at: it is checked as no user, against the same records.

- **What is asked.** With no user the only group is `ALL_USERS`, so only its `ArtifactAuthz` records apply. One with an
  `authzServiceName` has its service called as for a user, with `userId` empty, `authzActionEnumId`,
  `artifactTypeEnumId` (`AT_GIT_REPOSITORY`) and `artifactName` (the `repositoryId`); the `authzTypeEnumId` it answers
  takes the place of the record's (`AUTHZT_ALLOW` to grant, `AUTHZT_NONE` to say nothing, `AUTHZT_DENY`). The service
  runs with authorization off and no user, so it needs no `authenticate` attribute. This is how an application makes a
  repository public: Mantle's `check#GitRepositoryAuthz`, on the ALLOW ALL record for `ALL_USERS` above, answers
  `AUTHZT_ALLOW` for `AUTHZA_VIEW` on a public repository whoever asks. The framework's own data grants `ALL_USERS`
  nothing, so without such a record no request without credentials views anything. A drive's rule is by member, so it
  gives no user nothing.
- **What is served.** A repository the request may view: `info/refs` and `git-upload-pack` (clone, fetch), and LFS
  downloads (the batch answers actions with no token; each download is checked the same way).
- **What is not.** Everything else is `401` with `WWW-Authenticate: Basic` (LFS: `LFS-Authenticate` too), so git asks
  for a login and sends the request again: a repository the request may not view, a path that names none or is not a
  path, and every write and every answer that names users, even on a public repository: `git-receive-pack`, an LFS
  upload, verify, and every locks request (listing them too). The server refuses a push with no user itself, before
  any record is asked, so a record that lets everyone do everything still lets nobody push without a login. The answer
  is the same for a repository that is there and one that is not, status, headers and body alike, so it says nothing
  of what exists. With credentials the answers are as before: 404 for a repository the user may not view, 403 for a
  push the user may not make.
- **Not a failure.** A 401 for no credentials is not an `auth-failure` event (git sends every first request without
  credentials), and records no `ArtifactAuthzFailure`. Credentials that are wrong are both, as before.
- **A header that is not credentials** is not "no credentials". An `Authorization` header that is neither Bearer nor
  Basic that decodes to `user:password` (another scheme, an empty one), an empty `api_key`, and an LFS token that is
  not good are answered 401 and count as an `auth-failure`, on a public repository too. The anonymous rate limit below
  goes by the headers alone, so a made-up header buys nothing: it reads nothing and counts toward the ban.
- **Caps, by client.** With web protection's per-client parts on ([Run and Deploy](../Run-Deploy.md#behind-a-proxy-or-load-balancer)),
  the `git-anonymous` rate limit counts each client's git requests that carry no credentials: 120 a minute sustained,
  240 at once (a clone is three or more requests), under `/git/` and at a page's URL; past it the answer is 429 with
  `Retry-After`. It is a `<rate-limit>` with `anonymous="true"`, which counts only requests with no `Authorization`,
  `api_key` or `login_key` header; the headers are not checked there, and a session cookie is not looked at (git has
  none). A component changes it by its name (`<rate-limit name="git-anonymous" requests="..."/>`, or
  `enabled="false"`). A signed-in client is not counted by it.
- **Caps, in all.** Requests with no credentials run at most `git-server` `anonymous-max-concurrent` git jobs at once
  (half of `max-concurrent` when it is empty, at least 1: 2 of the default 4), so they cannot hold the whole git pool
  and signed-in users always have workers left. One more is `503` with `Retry-After: 5`. Only a request that is about
  to be served counts: one that is answered 401, as every signed-in client's first request is, takes no slot.
- SSH always has a user: the SSH server logs one in before git runs.

The default branch cannot be deleted. Only `refs/heads/`, `refs/tags/` and `refs/for/` can be pushed. Branch rules
([below](#branch-rules)) protect refs further, and an [archived](#archive) repository takes no push.

A repository that holds a drive (`ResourceDrive.repositoryId`, the `DRIVE_<driveId>` repositories of version-controlled
drives) also takes the drive's own rule (`FileServices.get#DriveAccess`), for users with no `AT_GIT_REPOSITORY` grant: view
fetches, edit pushes new refs and fast-forwards on a drive that is not read only, manage forces a push or deletes a ref. The
`get#Git*` services follow it, so Files' drive settings show the clone addresses to every member.

## Protocol

- **Fetch**: protocol v2 (`ls-refs`, `fetch`) and v0/v1 (`multi_ack_detailed`, `no-done`, `side-band-64k`), stateful
  over SSH and stateless over HTTP; `shallow`, `deepen`, `deepen-relative` (`--deepen`), `deepen-since`
  (`--shallow-since`), `deepen-not` (`--shallow-exclude`) and `--unshallow`; `include-tag`; a want by id of a commit
  a branch, a tag or a merge request's head reaches (`git fetch --depth=1 origin <sha>`), and of nothing else the
  store has ([Reachability](#reachability)). The advertisement is the branches and tags; the head of a merge request
  is listed by protocol v2 for a prefix that names it ([Merge request refs](#merge-request-refs)). No partial clone
  (`filter`) yet.
- **Push**: v0/v1 (v2 has no push), `report-status`, `side-band-64k`, `atomic`, `delete-refs`, `push-options`. The
  pack is indexed into a quarantine directory, completed from the repository when it is thin, and every object it
  refers to must be in it or the repository, a parent commit and a ref's new value among the commits the
  repository's refs reach ([Reachability](#reachability)); only an accepted push moves it in (a `local` repository keeps the pack in
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
`create`, `update` (a fast-forward), `force` or `delete`, `isDefaultBranch`, and `forReview` with `targetBranch` for
a push to `refs/for/<branch>`); a per-ref event those fields of its ref;
a commit event `commit` (`commitId`, `shortId`, `summary`, `body`, `message`, `trailers` by name, the author and
committer and their dates, `parentIds`, `moquiUserId`); a tag event `tag` (`tagName`, `targetId`, `commitId`,
`annotated`, and an annotated tag's `message` and tagger). `org.moqui.GitServices.process#GitEca` is the interface.

**A quota by pre-receive.** The pre events of a push also have `packBytes`: the bytes of the pack the push sent, as
it is kept in quarantine (a thin pack completed from the repository), so what the push would add to a `local`
repository and close to what it adds to a `db` one; and `forReview`, true when a ref of the push is
`refs/for/<branch>` (that ref's `refName` is `refs/for/<branch>`, its `targetBranch` the branch), so a rule decides
who may push for review and how much, beside the server's own caps ([Push to review](#push-to-review)). It is 0 for a push that only deletes and for a write through
Moqui. With `userId`, `refUpdates` and [`get#GitRepositoryUsage`](#usage-and-delete) a rule refuses a push that
would pass a limit, and its message is what the client sees:

```xml
<giteca rule-name="PushQuota" event="pre-receive">
    <actions><service-call name="mantle.smithy.SmithyGitServices.check#Push" in-map="context"/></actions>
</giteca>
```

A push is bounded before any rule runs by `git-server` `max-push-size`. LFS uploads do not pass pre-receive: see
[Large files](#large-files-git-lfs).

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

The services of `org.moqui.git.GitServices` (Rust: `crates/moqui-git/src/service/hosting.rs` over
`resource/git/hosting.rs`, `merge.rs` and `rules.rs`) and `org.moqui.git.GitReviewServices` serve the code screens of
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
| `get#GitRefs`, `get#GitLog`, `get#GitCommit`, `get#GitCommitDiff`, `get#GitBlob` | as before ([Revision-Management](../data/Revision-Management.md)) |
| `get#GitBlame` | for each line of a file at a revision, the commit that last changed it ([below](#blame-and-archive)) |
| `get#GitArchive` | a tree at a revision as a zip or tar.gz file to serve ([below](#blame-and-archive)) |
| `get#GitCommitRefs` | the branches and tags that contain a commit ([below](#reachability)) |
| `get#GitCommitPushers` | who brought commits to the repository ([below](#reachability)) |

### Blame and archive

`get#GitBlame(repositoryId, revision, path, withLines)` answers `commitId` (the commit the revision names),
`lineCount` and `hunkList`: runs of lines that one commit last changed, in line order, each `startLine` (from 1),
`lineCount`, `commitId`, `shortId`, `summary`, `authorName`, `authorEmail`, `authorDate`, and with `withLines` its
`lines`. A commit passes the lines it did not change to the parent that has them (the first parent whose file is the
same takes them all; a merge passes each parent its lines) and keeps the rest. Renames are not followed: a file is
blamed as far back as it has its path. Nothing is answered for a revision that names nothing or a path that is not a
file there. A binary file and a file kept in Git LFS are refused.

A blame is refused past any of its `git-server` limits, which bound what one costs:

| Attribute | Default | Bounds |
|---|---|---|
| `blame-max-bytes` | 1 MiB | the file, and each older version compared with it |
| `blame-max-lines` | 50,000 | the same, in lines |
| `blame-max-commits` | 10,000 | the commits read; one reached again is counted again |
| `blame-max-diffs` | 2,000 | the comparisons of two versions; each parent of a merge is one |
| `blame-max-seconds` | 10 | the whole blame; each comparison is cut off at it, and the blame refused, not answered worse |

Within them a blame of a large file with a long history is still seconds of one service thread. An older version
past the byte or line limit shares no lines: the commit after it keeps them.

`get#GitArchive(repositoryId, revision, format, path)` gives the tree at a revision (or one directory of it) as
`zip` (the default) or `tar.gz`, every entry under one directory named for the repository and the commit
(`mr-moqui-0123456789/`), and answers `archiveLocation`, `fileName`, `contentType`, `archiveSize`, `contentBytes`,
`fileCount`, `commitId` and `skippedList`. Entries carry the commit's time; executable files stay executable; a
submodule is its empty directory; files kept in LFS are archived with their content.

- **`archiveLocation`** is the absolute path of a file under `runtime/tmp/git-archive` on the node that answered.
  There is one file for each (repository, commit, path, format), named by a digest of those, written beside its
  name and renamed into place: every caller that asks for that archive is given the same file, whole. Serve it
  (`ec.web.sendResourceResponse(archiveLocation, false, fileName)`); never delete, move or write it. It is kept six
  hours from its last use (a call that reuses it marks it used) and removed by a later call or at start, as is what
  a writer that died left.
- **Names are not trusted.** A tree is what someone pushed. An entry whose name is empty, `.`, `..`, `.git` in any
  case, has `/`, `\` or NUL, or is not UTF-8, and a name twice in one directory, refuses the archive: unpacked, such
  a name could write outside the archive's directory or over another entry. git's own clients make none of them.
- **Links.** A link is a link entry with its target as it is stored; the server never follows it. It is kept only
  when the target is relative and, by its `..` segments, stays inside the archive. One with an absolute target, a
  `\`, or a target that climbs out is left out and its path listed in `skippedList`: no archive from here holds a
  link that points outside what was unpacked.
- **Bounds**, refused before anything is written: `git-server` `archive-max-bytes` (256 MiB) of content;
  `archive-max-files` (50,000) entries, counting files, links, submodules and every tree read, so a tree that names
  one subtree many times is not read without end; 64 directories deep (the walk uses no stack, whatever the
  depth); `archive-max-concurrent` (2) archives measured and written at once on a node, one more being refused; and
  `archive-max-disk-bytes` (2 GiB) of archive files kept, a new archive that would pass it being refused until
  older ones are removed. An LFS file and a large object of a `db` repository are read a window at a time; a large
  file of a `local` repository is held whole while it is written, within `archive-max-bytes`.

Both services need view, and are the application's to offer or not to a visitor with no account: a page that does
puts its route under a rate limit of its own, as the `git-anonymous` limit counts git requests only.

### Reachability

A repository's store may hold objects its readers may not read: a fork network keeps one set of objects
([Forks](#forks)), and the head of a merge request pushed for review is kept in its target before anyone has looked
at it. So that an object is in the store never decides a read.

**Who reads a commit by its id.**

| Caller | Reads by id |
|---|---|
| anyone who may view | a commit that a branch, a tag, or the head of a merge request that is not hidden (`refs/merge-requests/<n>/head`) reaches |
| a user with the update right, in a repository that is in no fork network | also any other commit of its store (a deleted or forced branch's, which the commit pages and links of who works in the repository show), but not what the head of a hidden merge request reaches |
| a user with the update right, in a repository of a fork network | as anyone: the store is other repositories' too |
| a service called with authorization disabled | as a user with the update right; with `includeHidden`, a hidden head's commits too |
| the git protocol (a fetch by id), a `git://` location's version, a checkout of a network's repository | only what the refs reach, whoever asks |

Any other id is answered as one that names nothing.

- **Where.** Every revision a service takes (`get#GitCommit`, `get#GitCommitDiff`, `get#GitBlob`, `get#GitTree`,
  `get#GitLog`, `get#GitCompare`, `get#GitBlame`, `get#GitArchive`, `get#GitCommitStatuses`, `get#GitCommitRefs`,
  `get#GitCommitPushers`, `create#GitRef`, `merge#GitRefs`, `create#GitCommitStatus`) that is an object id or the
  start of one; a name (a branch, a tag, `refs/merge-requests/<n>/head`) is the ref itself. The start of an id is
  looked up among the commits the refs reach, so neither its answer nor "names more than one commit" says anything
  of the rest of the store. A version of a `git://` location named by a commit id (read, or restored:
  `restore_version`) and a checkout of a revision are of such commits too. A fetch that wants an object by id
  (`git fetch origin <sha>`): what a ref it may read names, or a commit one reaches, else `not our ref`.
- **What a push names.** A push names objects it does not bring: the new value of a ref, the parents of its
  commits, the targets of its tags, the entries of its trees, and the bases its thin pack's deltas are made from.
  Each must be the pusher's to read, else the push is refused (`missing necessary objects`), so nobody makes a
  branch of a commit they only know the id of, builds on one, or reads a file back through a tree entry or a delta:
  - a commit (a ref's value, a parent, a tag's target, also a parent the client says is where its shallow history
    is cut): one the readable refs reach;
  - a tree, a blob or a delta's base: a file or directory of a commit the pusher reads, which for a user with a
    push right is what any readable ref points to or a commit the push builds on, and for a user who may only push
    for review is the branch the push is for, the heads of the user's own open pushes for review, or a commit the
    push builds on. These are the objects a git client leaves out of a push; the files of older commits it sends
    again. The walk reads the trees of those commits until what was named is found, at most 1,000,000 trees.
- **The push log.** `find#GitUpdates` gives a user without the update right no `remoteAddress`, no object id that
  no readable ref reaches now (the old value of a forced or deleted branch, a replaced head), and no row of a push
  to `refs/for/` whose head is not read (a hidden merge request's).
- **Hidden.** `GitReviewServices.set#MergeRequestHidden(mergeRequestId, hidden)` (with authorization disabled, else
  the delete right) sets `GitMergeRequest.hiddenDate` while an application holds a merge request's text: its head is
  then fetched by nobody, by name or by id, its commits are not read by id, and `get#MergeRequest`,
  `get#MergeRequestDiff` and `find#MergeRequests` (`includeHidden`) answer for it only with authorization disabled,
  as the application decides who sees it. It does not merge while hidden. A read service takes `includeHidden`,
  honoured with authorization disabled only, to read a hidden head's commits for those viewers.
- **Cost.** The commits each repository's refs reach are kept in memory on the node that is asked, as a set
  (about 50 bytes a commit), for at most 128 repositories (one more lets go of the set used longest ago). When the
  refs changed, the new values are walked back to the commits already known: a push that moves a branch forward,
  or adds one, costs its new commits and nothing else. Only a ref whose old value no commit of the set has as a
  parent after that (a branch deleted before it was merged, a forced one) makes the set again: one read of the
  history's commits, a batch of rows at a time. A hit is answered from the set for two seconds without reading
  the refs; a miss reads them first, so a commit pushed a moment ago is found. The first read by id, fetch by id or
  push of a repository on a node after a start pays one read of its history; a clone and a fetch of branches and
  tags ask nothing. `git-server` `reach-max-commits` (1,000,000) bounds a repository's set: in a larger history the
  commits past it are not read by id (by a ref's name they are), a warning is logged, and each change of the refs
  walks to the bound again.
- **`get#GitCommitRefs(repositoryId, commitId, limit)`** answers the branches and tags that contain a commit (it is
  their head or an ancestor of it): `refList`, each `refName`, `refType`, `shortName` and `isDefaultBranch`, the
  default branch first. At most `limit` refs (50; 500 at most) are asked, and at most `git-server`
  `contains-max-commits` (200,000) commits are read in one call, each once; `truncated` says that either ran out.
- **`get#GitCommitPushers(repositoryId, commitIdList)`** answers who brought commits (at most 500 ids) to the
  repository, for the commits the caller reads by id: `pusherList`, each `commitId`, `userId`, `username`,
  `updateId` and `pushDate`, from `moqui.resource.GitCommitPush` by its key. A row is the user of the first push or
  write (a `GitUpdate` with a user) whose refs reached the commit, written with the change's events, for at most
  `eca-max-commits` commits a change. A commit that came by a mirror fetch or an import, or before this was kept,
  has none.

### Refs, compare and merge

`create#GitRef` makes a branch, or a tag (annotated by the current user when given a message), at a revision;
`delete#GitRef` deletes one. `get#GitCompare(base, head, headRepositoryId, withDiffs, checkMergeable)` (`head` a
revision of `headRepositoryId`, another repository of the fork network that the user may view too, when it is given: a
fork's branch against its parent's) answers as GitHub's compare: the
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

### Commits through Moqui

`commit#GitFiles(repositoryId, message, ...)` makes one commit of files as the current user, for a page that edits a
file: no clone, no working tree. Where the commit goes:

| | Needs | Does |
|---|---|---|
| nothing, or `branch` | update | a commit on the default branch, or on `branch` |
| `newBranch` | create | makes `newBranch` at the head of `branch` (the default branch when empty) and commits there; the application then opens the merge request |
| `asProposal` | view | no branch: the commit, made on the head of `branch`, is the head of the user's merge request into it, opened or updated as by a push to `refs/for/<branch>` ([Push to review](#push-to-review)); `mergeRequestId`, `topic`, `title`, `description` and `draft` are that merge request's |

- **Files.** `fileMap` is path to text or bytes; `removePaths` the paths deleted; `renameMap` old path to new (a file
  or a directory), with edits of the renamed file under its new path in the same commit. `fileLocationMap`, path to
  a resource location whose bytes are the content (a staged upload), is read as the server, so it is taken with
  authorization disabled only. A file the branch's `.gitattributes` keep in LFS is stored there, for a user who may
  push.
- **The edit's base.** `expectedBlobIds` is path to the blob the edit began from (`get#GitTree`'s `objectId`; empty
  text for "no file there"). When the path has another blob at the branch's head nothing is committed, and the
  answer is not an error: `conflict` true, `errorCode` `GitFileChanged` and `conflictList`, each `path` with
  `expectedBlobId` and `currentBlobId`, so the page shows the text that is there beside the user's.
- **The author** is the current user, as the committer is. With `authorName` and `authorEmail`, which are taken
  with authorization disabled only (the application gives an account's display name and confirmed address; a
  caller does not say who a commit is by), the author is that and the committer is the server (`git-server`
  `committer-name` and `committer-email`, else `Moqui <moqui@localhost>`). Either way the message gets the
  `Moqui-User-Id` trailer.
- **As a push.** A branch moves through the same checks: the [branch rules](#branch-rules) (a protected branch
  refuses, `GitProtectedBranch`), the pre events of gitECA rules of source `write`, the push log, and the post events
  after it (a wiki save through a `git://` location is not this service and is not checked against the rules). An
  application's own write to a protected branch (release notes) passes the rules with `bypassBranchRules`, honoured
  with authorization disabled only. A new branch's name must be one git takes (`GitBadRefName`), as every ref a
  service writes.
- **A proposal** is a push for review made by a service: the same caps, and the pre events of gitECA rules of
  source `push` (transport `internal`) with the ref `refs/for/<branch>`, `forReview` and `packBytes` (the bytes of
  the files), so a rule that gates pushes for review gates proposals; it is in the push log as that push.
- **Out**: `commitId` (empty when nothing changed), `refName` (the branch, or the merge request's head ref), and for
  a proposal `mergeRequestId`, `mergeRequestNum` and `mergeRequestCreated`.

### Error codes

A service error has no code of its own in the message facade, so a refusal an application must tell from the others
starts with one, then `: ` and the text for a person (`GitProtectedBranch: protected branch: main changes through a
merge request`). Match the start of the error; the text after it may change.

| Code | From | When |
|---|---|---|
| `GitProtectedBranch` | `commit#GitFiles`, `create#GitRef`, `delete#GitRef`, `merge#GitRefs`, `merge#MergeRequest`, a push | the branch rules refuse the change |
| `GitRefExists` | `create#GitRef`, `commit#GitFiles` (`newBranch`) | the branch or tag is there |
| `GitBadRefName` | `commit#GitFiles` (`newBranch`), the ref writes | not a name git takes for a branch or tag |
| `GitDefaultBranch` | `delete#GitRef` | the default branch cannot be deleted |
| `GitArchived` | every write, `create#MergeRequest`, `fork#GitRepository` | the repository is archived (or being deleted) |
| `GitNoSuchRef` | `delete#GitRef`, `commit#GitFiles`, `create#MergeRequest`, `merge#MergeRequest`, a push to `refs/for/` | no such branch or tag |
| `GitStaleRef` | the ref writes | the ref moved since it was read |
| `GitFileChanged` | `commit#GitFiles` | `errorCode`, with no error: a path's blob is not the expected one |
| `GitReservedContext` | `create#GitCommitStatus` | a context the system keeps |
| `GitHasForks` | `delete#GitRepository` | other repositories were forked from it |
| `GitRepositoryExists` | `fork#GitRepository` | the new id is in use |
| `GitNotInNetwork` | `create#MergeRequest`, `get#GitCompare` | the two repositories are not forks of one |
| `GitMergeRequestExists`, `GitNumberTaken` | `create#MergeRequest` | one is open for the source and target; the number given is in use |
| `GitReviewPushTooLarge`, `GitReviewQuota` | a push to `refs/for/`, `commit#GitFiles` (`asProposal`) | over the caps for a user with no push right |
| `KeyInUse` | `UserServices.create#UserSshKey` | the key is an account's already |

Over git the same texts are a ref's reason (`! [remote rejected] main -> main (GitProtectedBranch: protected branch:
...)`) or a `remote:` line.

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

### Usage and delete

`get#GitRepositoryUsage(repositoryId, repositoryIdList)` answers what repositories store now, for an application's
quotas, so it reads no content-group rows itself: `usageList`, in the order asked, each `repositoryId`,
`repositoryKind`, `storedBytes`, `objectCount`, `lfsBytes` and `lfsObjectCount`, with `totalStoredBytes` and
`totalLfsBytes`. A repository the user may not view, or that is not there, is left out; at most 1000 ids. A caller
with authorization off (a gitECA rule's actions, the LFS upload check) is answered for every repository that is
there, so a quota can count a namespace's repositories whoever pushes. It is for an application's own quota code,
not a visitor's route: each `db` repository asked costs a grouped read of its object rows, each `local` or `remote`
one a walk of its directory.

- `db`: `storedBytes` is the object rows as they are stored (compressed, deltas as deltas) plus the content blobs of
  its large objects, and `objectCount` its rows, from one grouped query. For a fork these are the objects it added
  (what its pushes and writes stored first); what it shares counts to the network's root ([Forks](#forks)). It is current right after a push. It falls
  when `gc#GitRepositories` deletes and repacks, so a quota should allow for what a collection has not yet let go.
- `local` and `remote`: `storedBytes` is the files of the repository's directory (packs and loose objects, with the
  few files beside them), read when asked, so it is current after a push too; `objectCount` is empty.
- `lfsBytes` is the sizes of the LFS objects uploaded to the repository. The same content in two repositories is
  stored once and counted in each.
- `GitRepository.storedBytes` and `objectCount` are not these: they are what `gc#GitRepositories` recorded at its last
  run of a `db` repository.

`delete#GitRepository(repositoryId)` (delete right) removes a repository and everything the framework keeps of it:
its refs and objects (the rows of a `db` kind, the directory of a `local` or `remote` kind), its LFS objects and
locks, its push log, commit statuses, webhooks, branch rules, merge requests and their reviewers, the redirects of
paths it had, and its row. It cannot be undone.

- **Who.** The delete right is also what forces a push and changes a repository's settings, so an application
  that lets fewer users delete (an Owner, a typed path to confirm) checks its own rule and offers its own service,
  not this one. Called with authorization off it deletes for any caller.
- **In order.** The repository is first marked as going: its `archivedDate` is set in a transaction that commits
  at once. Every push and write reads the row again, under the repository's lock, before it stores anything (on
  every node, whatever it has open), so from then nothing is written to it. The rows are then deleted in the
  service's transaction. Once that commits the directory is removed and the rows of the id are deleted a second
  time, for what a push that was already storing when the mark was set left behind. A rollback keeps the rows and
  the directory and sets the mark back. When the caller's own transaction already holds the repository's row (it
  wrote it before calling), the mark cannot be committed and is skipped; the second pass still runs.
- **The directory** is removed only when all of this holds, read from the disk with links and `..` followed, not
  from how the path is written: it is under the runtime directory and is not that directory; it looks like a bare
  repository (a `HEAD` file and an `objects` directory); and no other repository's row names it, a directory in
  it, or a directory it is in. Otherwise it stays and is named in `directoryKept`: a `localPath` can name anything,
  a developer's own clone too.
- **Left**: the content blobs of its large and LFS objects (`gc#ContentBlobs` collects them when nothing else has
  them), and webhook deliveries already queued.
- **Refused**: a repository that holds a drive (delete the drive), one another component's row still refers to
  by a foreign key, and one other repositories were forked from (`GitHasForks`: [Forks](#forks)). The application
  removes its own rows first (Mantle's project links, a Smithy repository) and decides whether a repository with
  open merge requests or release assets may go: the framework asks nothing of that.
- After it the id and its paths name nothing, for git and for the services; a path it had is free at once.
- The node that deleted it forgets it at once (the open repository, its cached objects and path indexes), so an id
  used again starts empty there. In a cluster a git request reads the repository's row each time, so every node
  answers "not found" at once; another node that had the repository open may still answer a `get#Git*` service as
  for an empty repository, and keeps its cached objects, until it restarts. Use a new id for a new repository, never
  a deleted one's.

### Commit statuses

Outside CI posts the result of a check of a commit, in GitHub's status shape:

```
POST /rest/s1/moqui/git/{repositoryId}/commits/{commitId}/statuses
Authorization: Bearer <access token>
{"state": "success", "target_url": "https://ci.example.com/run/7", "description": "Build", "context": "ci/build"}
```

**Reserved contexts.** A hosting application that writes checks of its own (a contributor agreement check, its CI
engine) names their prefixes in `git-server` `reserved-status-contexts` (`agreement/,ci/`): such a context is then
refused from a user, whatever their rights (`GitReservedContext`), and written with authorization disabled only, so
a branch rule that requires it cannot be met by someone who may push posting it themselves. None is reserved by
default: an outside CI posts `ci/...` with a user's token, as above.

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

### Push mirrors

A repository with a `pushMirrorRemoteId` is pushed to another server (GitHub, GitLab, another Moqui) after each push,
fetch, import or write, by the framework's post-receive rule (`push#GitMirror`; Push Mirror Now on the repository's
screen). The `SystemMessageRemote` holds where and how: `sendUrl` the SSH URL (`git@github.com:owner/repo.git` or
`ssh://git@host:port/owner/repo.git`), `privateKey` and `publicKey` (a deploy key with write access), and
`sshHostKeyFingerprint` to pin the host key, else it is trusted on first use (`runtime/conf/ssh_known_hosts`), as the
backup's SFTP mirror does. `pushMirrorRefs` names the refs as `from:to` pairs (`master:mirror` pushes master to the
mirror's `mirror` branch; `refs/tags/*:refs/tags/*`), every branch and tag when empty. Each is forced to the local one,
and nothing is deleted. The rule runs the push async, outside the change's events, so a slow or unreachable mirror
holds up no other rule; pushes of one repository take turns, and each sends the refs as they are when it runs.
`lastPushMirrorDate` and `lastPushMirrorError` are on the row (a failure does not fail the `GitUpdate`); the next
change, or Push Mirror Now, pushes again. A push ends as git's does: it waits for the mirror's `git-receive-pack` to
end, so the mirror's own post-receive hooks finish.

### Merge requests

`org.moqui.git.GitReviewServices` keeps a merge request into a target branch of a repository
(`moqui.resource.GitMergeRequest`, numbered per repository from 1, statuses `GmrDraft`, `GmrOpen`, `GmrMerged`,
`GmrClosed`): of a source branch of the same repository, of a branch of another repository of its fork network
(`sourceRepositoryId`, [Forks](#forks)), or of commits pushed for review with no branch (`pushedForReview`,
[Push to review](#push-to-review)); and its reviewers (`GitMergeRequestReviewer`: `GmrrRequested`, `GmrrApproved`, `GmrrChangesRequested`,
`GmrrCommented`, each with the head it was given on):

| Service | |
|---|---|
| `create#MergeRequest` | of a branch of the repository by a user who may push; with `sourceRepositoryId`, of a fork's branch by a user who may view the target and has the update right on the fork; one open request per source and target; head, merge base and mergeability read now. With authorization disabled only: `mergeRequestNum`, a free number (an import) |
| `update#MergeRequest` | title, description, target, draft (marked ready), merge method, delete the source after; by the author or a user with update |
| `set#MergeRequestReviewers` | replace, add or remove reviewers (not the author) |
| `submit#MergeRequestReview` | approve, request changes or comment, at the current head; only a user who may push (update) approves, a viewer comments or requests changes; the author does not approve their own |
| `close#`, `reopen#MergeRequest` | |
| `merge#MergeRequest` | by its method (given, its own, else the repository's default) once the target's branch rules are met: the approvals (of the current head when stale ones are dismissed; only those of users who may push now count, each decided as its reviewer), the required checks successful on the head, a method they allow; update right (an application with roles of its own checks them before it calls). What merges is the head the approvals and checks were counted for, by its id: a source that moves meanwhile is not merged. It writes the target only; the source branch is deleted when asked, in a fork only by a user with the delete right there |
| `get#MergeRequest` | the request, its reviewers, the head's checks, the target's rules and what keeps it from merging now (`missingList`), and what the user may do |
| `find#MergeRequests` | of the repositories the user may view: by repository, status, author, reviewer, waiting for a user's review, target, title |
| `get#MergeRequestApprovals` | the approvals that count toward the branch rules, as `merge#MergeRequest` counts them |
| `get#MergeRequestDiff` | its commits and diffs as `get#GitCompare` gives them |

#### Merge request refs

Every merge request has its head at `refs/merge-requests/<mergeRequestNum>/head` of its repository, set when it is
opened and at each new head: a branch's, a fork's (whose objects the repository does not have are copied to it then)
and a pushed one's alike. The ref is the merge request's: no push writes it, it makes no `GitUpdate` and runs no
rule, and `get#GitRefs` does not list it.

- **Fetched by name**: `git fetch origin refs/merge-requests/12/head`, by anyone who may view the repository, which
  is how a reviewer or a tool checks one out. The refs are not in the advertisement, so a clone and `git ls-remote`
  do not see them; protocol v2 (git's default) lists one for a prefix that names it. A v0 client fetches the head
  by its id (`headCommitId`).
- **Hidden** ones are not fetched ([Reachability](#reachability)).
- A merge request from a fork or pushed for review is compared and merged from this ref, in its repository, so a
  merge never reads another repository. One of a branch of the same repository follows the branch, as before (it
  is unchecked while the branch is gone).
- The ref of a merged merge request stays, so its commits are still read and fetched. One closed without merging
  keeps its ref for `git-server` `review-ref-retention-days` (30; 0 for ever): `gc#GitRepositories` then deletes it,
  so what was pushed for review and refused does not stay in the repository, and a later collection lets go of the
  objects. A reopened one of a branch gets its ref back; one pushed for review is pushed again. While it is
  hidden the ref of a closed one is there and not fetched.
- **Merge requests from before these refs.** One that has no ref gets it when it is next read or refreshed
  (`sync#MergeRequestHead`): an open one from its source branch, a merged one (and a closed one within the
  retention time) from its recorded `headCommitId` while that commit is in the store, so `get#MergeRequestDiff`
  and the checks of its head still answer.

`refresh#MergeRequests`, the framework's ref-update rule (`Git.gitecas.xml`), keeps them current on every change of a
branch, from any source, in the branch's repository and for the merge requests of other repositories of the network
whose source it is (`sync#MergeRequest`): a source's new head is recorded (its approvals dismissed when the target's rules say so;
webhooks get `synchronize`), each request of the branch is checked for mergeability again (`GmaMergeable`,
`GmaConflict` with the paths, `GmaUnchecked` while a branch is gone), and a request whose head the target holds is
merged (as when it was merged by a push).

### Forks

`GitServices.fork#GitRepository(sourceRepositoryId, repositoryId, description)` makes a fork: a new repository of
the source's kind (`local` or `db`; a `remote` mirror is not forked), with the source's branches and tags as they
are now as its own refs, its default branch, and its LFS objects (rows; the content is stored once). It needs view
of the source and checks nothing else: who may fork, into which id and `repositoryPath`, how many forks an account
has and who sees the fork are the application's, checked before the call and set after it.

- **The network.** `GitRepository.forkOfRepositoryId` names the parent; `forkNetworkId` is the root of the fork
  network, on every fork and on the root itself once it has one (a repository with no fork has neither). A fork of
  a fork is in the same network.
- **`db`: one set of rows.** The objects of a network are the `GitObject` rows of the root's id; a fork has refs of
  its own (`GitRef`) and no object rows. A repository's own rows are therefore its network's from the moment it is
  first forked, with no row changed and nothing to migrate, and an object is stored once however many forks have
  it. What a fork's pushes and writes add is marked with it (`GitObject.addedRepositoryId`), which is what
  [`get#GitRepositoryUsage`](#usage-and-delete) counts as the fork's.
- **`local`: the parent's directory.** A fork's bare repository names its parent's `objects` directory in
  `objects/info/alternates`, by a path relative to its own `objects` so the two move together, and keeps its new
  objects in its own directory (`runtime/git/<repositoryId>`, or the `localPath` an application gives with
  authorization disabled). Both must be on the node's disk; moving one of them, or deleting the parent's directory
  by hand, breaks the fork.
- **Reads.** What a repository answers is decided by its refs, not by its store ([Reachability](#reachability)): a
  private fork's commits are not read or fetched through its parent or a sibling, by id or otherwise, and the
  parent's later commits are not read through the fork.
- **Merge requests.** `create#MergeRequest` with `sourceRepositoryId` opens one from a fork's branch into a branch
  of another repository of the network, by a user who may view the target and has the update right on the fork
  (the branch is followed from then on, so only who may write there offers it); its head is kept in the target
  ([Merge request refs](#merge-request-refs)), so opening it is what shows those commits to the target's readers,
  and with `review-start-hidden` it starts hidden as a push for review does. A push to the fork's branch moves it
  while the author still has the update right on the fork. It stops following and keeps its head
  (`sourceDetachedDate`) when the author has lost that right, when the fork's owner detaches it
  (`detach#MergeRequestSource`: the delete right on the fork, or the author), and when the fork is deleted (so a
  repository made later with its id feeds nothing). The LFS objects of the files it adds become the target's too,
  through the target's upload check (`lfs-upload-check-service`), so the merged files read as their content and the
  target's quota holds. `get#GitCompare` with `headRepositoryId` compares before one is opened.
- **Collection.** `gc#GitRepositories` collects a `db` network with its root: the run keeps what the refs of every
  repository of the network reach (and their changes in progress and of the reflog window), and passes over the
  forks themselves. A `local` parent's run walks each fork's refs through the fork's own store, so what a fork's
  own commits reach in the parent's directory stays (a branch the parent deleted and a fork built on); a fork's run
  packs only what its own directory holds. A network one of whose repositories cannot be opened is not collected.
- **Delete.** A fork is deleted alone: its refs and rows go; a `db` fork's object rows stay in the network as
  nobody's (they count in no repository's usage) until a collection lets go of those no ref reaches, and a `local`
  fork's directory goes with it (what a merge request brought to another repository was copied there). A repository
  others were forked from is not deleted (`GitHasForks`): delete its forks first, or archive it. The objects are not
  made the forks' own. The forks are looked for once the repository is marked as going, and a fork is not made of a
  repository that is archived or going, so one node does not fork what it is deleting.

### Push to review

`git push origin HEAD:refs/for/main` opens a merge request into `main` with no branch and no fork, from any account
that may view the repository (`git-server` `review-push`, on by default):

```
$ git push origin HEAD:refs/for/main
remote: merge request !12 opened for main (100042)
remote:   https://moqui.org/moqui/mr-moqui/-/merge-requests/12
 * [new reference]   HEAD -> refs/for/main
```

- **What it does.** The pushed commits are stored, and `propose#MergeRequest` gives them to the open merge request
  of the same author, target branch and topic (the one with no topic when the push names none) as its new head, as
  a push to a source branch does; when there is none it opens one. Its head is
  `refs/merge-requests/<n>/head` ([Merge request refs](#merge-request-refs)), `pushedForReview` is Y, `sourceRef`
  that ref and `reviewTopic` the topic. No ref named `refs/for/...` is ever made.
- **Push options**: `-o topic=<name>` (another merge request for the same branch), `-o title=<text>`,
  `-o description=<text>`, `-o draft`. With no title a new merge request has its first commit's summary, and that
  commit's message body as its text.
- **Nothing else.** A user with no push right is refused every other ref of the push (`permission denied`), and
  uploads no LFS objects. A branch that is not there (`GitNoSuchRef`), and a push that brings nothing the branch does
  not have, are refused.
- **Caps**, for a user with no push right (a user who may push is not capped: they could push a branch): the pack
  of one push is at most `git-server` `review-max-push-bytes` (50 MiB, `GitReviewPushTooLarge`), and no more than
  that is read from such a user's connection, over HTTP and SSH alike; the packs of an account's pushes for review
  in the last day, over every repository, are at most `review-max-account-bytes` (200 MiB, `GitReviewQuota`); 0 is
  no cap. `GitUpdate.packBytes` and `reviewPush` keep what was counted. A pre-receive rule sees `forReview`, the
  ref's `targetBranch`, `userId` and `packBytes`, and refuses by the application's own rules (a confirmed address,
  a limit a day); its message is the client's.
- **The answer** names the merge request by number and id, and its page when `git-server` `merge-request-url` is set
  (`/{repositoryPath}/-/merge-requests/{mergeRequestNum}`; also `{repositoryId}` and `{mergeRequestId}`; after the
  public URL when it starts with `/`).
- **Events.** The push is in the push log under `refs/for/<branch>`, and its commit, ref-update and post-receive
  events run as any push's (the ref is of type `other`). The merge request's own events are the same as for one
  opened by the service: `create#MergeRequest` runs, so the service ECA rules on it do, and webhooks get `opened`,
  then `synchronize` for each later push.
- **Through a service**: `commit#GitFiles` with `asProposal` ([Commits through Moqui](#commits-through-moqui)), and
  `propose#MergeRequest` itself, which is called with authorization disabled only and trusts that `headCommitId` is
  the caller's own commit.
- **Held text.** An application that holds what a new account sends sets `git-server` `review-start-hidden`: a
  merge request opened by a push for review or a proposal then starts hidden ([Reachability](#reachability)), so
  its commits are never read before the application has looked, and the application shows it
  (`set#MergeRequestHidden` with `hidden` false) from its handler of the `opened` event, at once or when a
  moderator lets it through. Without the setting the application may hide one from that handler instead.

## Projects and tasks

In Mantle (`mantle.work.GitServices`, the rules of `mr-mantle/service/Work.gitecas.xml`, and the screens of
mr-marble), a project's Repositories tab links repositories to it (`WorkEffortGitRepository`), or makes a new one.
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
`mr-mantle/service/WorkGit.secas.xml`, after the framework's review services: opened and edited ones link their
tasks (`link#MergeRequestItems`), an opened or ready one moves them to In Review when the project's
`WstMergeRequestMoves` is Y (`move#MergeRequestItemsToReview`), a merged one completes them (`complete#MergeRequestItems`), reviewers and authors are told (topics `MergeRequestReview`,
`MergeRequestUpdate`), and a failed check tells the commit's author (`CommitStatusFailed`). `create#TaskBranch` makes a
task's branch by the project's `WstBranchPattern`; review comments on a line are `CommunicationEvent`s with a
`CommunicationEventGitAnchor` (`create#ReviewComment`, `resolve#ReviewThread`, `get#ReviewThreads`). A task's Source tab links commits,
branches and tags by hand; its summary shows them, and the project summary the repositories and tags.
`Project/GitCommit` shows a commit: its message, people, parents, linked tasks, and each changed file's diff.

## Storage of db repositories

The `GitObject`, `GitRef` and `GitLfsObject` rows, and the content blobs, are in the content database
([Run-Deploy](../Run-Deploy.md#content-database)). The `GitRepository` row and the rest of the hosting rows (updates,
merge requests, branch rules, LFS locks) are with the business rows, so a repository row and its objects are read with
queries of their own.

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

The same job repacks each `local` repository on disk, where each push leaves a pack and each `git://` write loose
objects, as `git repack -a -d` does. One with one pack and fewer than 100 loose objects is passed over. Under the
repository's writer lock it reads the refs and the roots above and lists its packs and loose objects; then, without the
lock, it writes what those roots reach into one new pack (packed deltas copied as they are) and moves it in, pack before
index. An unreachable object written in the last `gc-prune-days` is kept loose with what it reaches; an object in a
pack is as old as its pack (git's rule: git has no time for each packed object), so one that is kept is written loose
with its pack's time and ages from there. Under the lock again it deletes only the packs and loose objects it listed
(an index before its pack; one that cannot be deleted is logged and the rest go on): a push or write since then
stays, and a ref that moved onto an object it would delete stops the deletes. A pack with a `.keep` file is left alone.
The open repository is then forgotten, so the next use opens the packs as they are.

The tools screen of a `db` repository shows its storage and runs a collection (a dry run by default) and the check.

Measured on embedded Postgres (debug build, `cargo test -p moqui-host --test it -- --ignored git_storage
--nocapture`):

| | Objects | Content | git's pack | Stored, imported | After a collection | Clone |
|---|---|---|---|---|---|---|
| mr-mantle's history | 13,435 | 195 MiB | 8.25 MiB | 8.37 MiB (1.02x) | 7.88 MiB (0.96x) | 3 to 5 s |
| mr-moqui's history | 11,404 | 160 MiB | 10.3 MiB | 25.6 MiB | 10.4 MiB (1.01x) | 2 to 3 s |
| 1000 `git://` saves of a 50 KB page | about 3,000 | about 49 MiB | | 2.72 MiB | 0.30 MiB | |

Before this format the rows held the content (the Content column). A 1 GiB file is stored as it is read (55 MiB/s)
and sent at about 270 MiB/s, the process's peak memory rising about 180 MiB; a 64 KiB range of it takes about 50 ms.

## Content blobs

Large content is a `moqui.resource.ContentBlob`, by the SHA-256 of its bytes, in `ContentBlobChunk` rows of
`resource-facade` `blob-chunk-size` (1 MiB) written a chunk per transaction: Git LFS objects, the large objects of `db` repositories, and
`dbresource://` files over 64 KiB. The same bytes are stored once; two uploads of them at once, or a crash halfway
(`ContentBlobUpload`), leave one blob and no stray chunks. A row refers to a blob by a relationship to `ContentBlob`,
and `gc#ContentBlobs` (`org.moqui.resource.ResourceServices`, the `gc_ContentBlobs_daily` job, 04:30) deletes the blobs no
row refers to and that were not written or referenced again in the last `resource-facade` `blob-gc-prune-days` (14) days.

File responses stream (`sendResourceResponse`, a screen's static files): a file over 256 KiB is read a window at a time
as the client takes it, on a thread of its own (at most `webapp` `transfer-workers`, 4, reading the database at once, fewer when the content pool cannot hold them), so
a slow download holds no request worker. One `Range` is answered from the bytes it names, with `ETag` (a git file's
blob id, a content blob's SHA-256) and `If-Range`. Only text types are compressed. An upload is read as it arrives: a
file part over 256 KiB goes to a file in `runtime/tmp/upload`, and from there to its storage (a content blob, a git
object) as it is read; `webapp_max_upload` (2 GiB) bounds it and `webapp_upload_idle_timeout` (60 seconds) of silence
ends it.

## Large files (Git LFS)

git-lfs clients push and fetch large files at `/git/<path>.git/info/lfs` (the Batch API with the basic
transfer, and the locks API): `git lfs install`, `git lfs track "*.psd"`, then push and clone as usual. Over SSH,
git-lfs runs `git-lfs-authenticate`, which gives it that HTTPS endpoint and a token for an hour (`git-lfs-transfer` is
refused, so it does). An object is the content blob of its SHA-256, so the same file in many repositories is stored once, but a
repository has an object (`moqui.resource.GitLfsObject`) only once it was uploaded to it: knowing an oid does not reach
another repository's file. Uploads need the push rights of the repository, downloads view (with no credentials, on a
repository anyone may view: [No credentials](#no-credentials)); a lock needs push rights (and listing locks a
login),
and force unlocking another user's lock `delete`. The batch's actions carry a token signed with `git_lfs_token_key`
(generated into `runtime/.env`; `git_lfs_token_key_previous` still reads while it is changed) that says who the user
is, for 15 minutes, and each request is authorized again. Objects are at most `git-server` `lfs-max-object-size` (5 GiB).

**A quota for uploads.** An LFS upload is not a push, so no pre-receive rule sees it. `git-server`
`lfs-upload-check-service` names a service (none by default; the interface is
`org.moqui.git.GitServices.check#GitLfsUpload`) that is asked before objects new to a repository are accepted, with
`repositoryId`, `userId`, `uploadBytes`, `objectCount`, `objectList` (each `oid` and `size`) and `stored`:

- by the batch API, once for a batch, with every object the batch would add (one already in the repository, and one
  over `lfs-max-object-size`, is not counted);
- by each upload, with that object, so a client that skips the batch is checked too;
- by a write of an LFS-tracked file through Moqui (a `git://` location, `commit#GitFiles`), with that file.

It runs as the user with authorization off, whatever errors the caller had before. An error it adds refuses the
request with 403 and the error's text (`upload declined: ...`), which git-lfs shows; for a batch the whole batch is
refused; a write through Moqui fails with it. A service that fails refuses as well. With a service named, an upload
needs a `Content-Length` (411 without), as git-lfs sends.

Uploads that run at once are each within a limit when they ask and can be past it together, so the service is asked a
second time once an object is the repository's, with `stored` true and `uploadBytes` 0 (the object is in what
`get#GitRepositoryUsage` counts by then); a refusal then takes the object back and answers 403. What a repository
keeps therefore never passes what the service allows, however many uploads run at once, and a service that adds
`uploadBytes` to the usage needs no change for it. The bound while uploads are in flight: each may hold up to
`lfs-max-object-size` of content not yet counted, and a refused upload's content stays, as no repository's, until
`gc#ContentBlobs`.

Moqui itself writes LFS files through `git://` locations: a file the `.gitattributes` of the branch head keep in LFS
(`filter=lfs`) is stored as a content blob, linked to the repository, and committed as its pointer; a read gives the
content, streamed for a response, and its size is the content's. `setup#GitLfs(repositoryId, patterns)` adds the
patterns to `.gitattributes` (images, video, audio, PDFs and archives when empty); a content repository gets them when
it is made (`init#GitRepository` with `contentRepository`, the tools screen's Content option, and the Content option of
a project's new repository). A web site's images and their variants (`convert#Image` writes an image and its sizes in
one commit) can so live in a `db` repository, with their history, in the database.

## Limits

- `git-any-account` is one switch for the server: it cannot be on for one host or one repository. An account that
  may view nothing still logs in over SSH (to the terminal client, as every account does). The session shares are
  per node.
- Blame follows no renames and reads no `.gitattributes`; it holds a service thread for up to `blame-max-seconds`.
  An archive is built whole before it is served and is a file on one node; names that differ only in case are not
  refused, and unpack over each other on a file system that ignores case. Neither is capped by client: the page
  that offers them rate limits its route.
- A push or write that had read the repository's row and was storing when `delete#GitRepository` marked it can
  leave rows after the delete's second pass, on another node; they are of an id that names nothing, and go when
  the delete is run on a repository made with that id, which is one more reason not to reuse ids.
- `packBytes` is the pack's size, not what a `db` repository's rows will take (close: the rows keep the pack's
  compressed entries). `delete#GitRepository` on MySQL deletes a `db` repository's objects a delta depth at a time.
- Usage of a `local` repository walks its directory on each call; ask for many of them rarely.
- Anonymous read is over HTTP only, and is fetch and LFS download only; the `get#Git*` services and an application's
  pages decide for themselves what a visitor with no account sees (they ask the same check).
- A repository's id must itself be a path of one segment to be reached by a git URL
  ([Repository paths](#repository-paths)); one that is not needs a `repositoryPath`.
- Paths have no owners in the framework: the application decides who may rename and when a left path may be taken.
- `anonymous-max-concurrent` is per node, as the git pool is, and is read when the node first serves such a request.
- A merge reads no `.gitattributes` (no merge drivers, every file merged as text unless it is binary); revisions
  are names and ids, not `main~1`.
- Forks: a `remote` mirror is not forked, an archived repository is not forked, and a fork is of its parent's
  kind. A parent with forks is not deleted, and its objects are never made a fork's own. A `local` fork needs its
  parent's directory where it was relative to its own; git itself does not follow a relative alternate of an
  alternate, so a fork of a `local` fork is read by the server and not by `git` run in its directory. A `db`
  fork's LFS rows are not swept by a collection. A detached merge request is not attached again. The fork and
  delete checks are under one node's lock: two nodes can still fork and delete one repository at once.
- The read services that take a revision also take `includeHidden`, honoured with authorization disabled only:
  `get#GitLog`, `get#GitCommit`, `get#GitCommitDiff`, `get#GitBlob`, `get#GitTree`, `get#GitCompare` and
  `merge#GitRefs`; blame and archive do not.
- Reachability: what a push may name is the files of the commits it builds on and of the ref values it may read,
  not of their history, as a git client sends; a client that leaves out an object only an older commit has is
  refused. A fetch negotiation still says whether an object id a client offers as a `have` is in the store. The set
  of reached commits is per node; after `set#MergeRequestHidden` another node may answer from its set for two
  seconds. In a repository in no fork network a user with the update right reads the replaced heads of a hidden
  merge request that its current head does not reach.
- Push to review: a v0 client does not see `refs/merge-requests/*` by name. The caps count packs as sent, per node's
  clock, and a push refused after its pack came in still counted nothing. A push recorded and cut short by a
  restart is logged as not made, though its merge request may have its head. `review-push` is one switch for the
  server. A proposal of `commit#GitFiles` is always made on the branch's head, replacing the merge request's
  earlier head rather than adding to it.
- `get#GitCommitPushers` knows the first user whose change brought a commit to the repository, not who wrote it;
  a commit a fork already had is the user's who brought it to this repository (opened the merge request, or pushed).
- Error codes are a prefix of the error text ([Error codes](#error-codes)); refusals with no code in the table have
  none.
- The framework's rules make a `GitUpdate` of every `git://` write (a wiki save), to keep merge requests and webhooks
  current.

- A push mirror pushes over SSH only, deletes no ref on the mirror, and sends no LFS objects.
- A push made directly to a `local` repository's directory on the host runs no gitECA rules.
- `gc#GitRepositories` repacks a `local` repository into one pack without delta compression of its own: deltas the
  pushes sent are kept, and objects written through Moqui are packed whole (run `git gc --aggressive` in the
  directory for more). It does not let go of a `local` repository's LFS objects.
- `local` and `remote` repositories are on one node's disk: in a cluster send `/git` and SSH to that node, or share
  the directory. `db` repositories and content blobs are in the database, for every node.
- A `remote` mirror does not fetch the LFS objects of the repository it mirrors.
- SHA-1 only; no partial clone, signed pushes (`push-cert`), `report-status-v2`, the dumb HTTP protocol, or LFS
  transfer adapters other than basic.
