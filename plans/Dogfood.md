# Dogfood: the Moqui project's infrastructure on MR Moqui

Nov 11, 2026 · Research and plan; nothing built. The decisions are in [§0](#0-decisions); the user made or confirmed
most of them the same day, in two rounds (each row says so), and the rest are this plan's choices, each with its
reason. The questions are in [§16](#16-risks-and-open-questions), six of them answered. The page-by-page design of the
new app is the first phase's output (`mr-pass/plan/design-smithy.md`, [§13](#13-phases) DG0), and the Dev AIDE's part
is [design-dev-aide §21](design-dev-aide.md#21-dev-aide-with-a-group-server).

The Moqui project runs on other people's software today: GitHub for the repositories, Issues and pull requests, and
for the `moqui-site` repository that holds the web site; Discourse for forum.moqui.org; Google Groups for the mailing
lists; and almost no CI. This plan moves all of it to mr-moqui, at moqui.org, on our own software and on cloud
infrastructure we run (the provider is not chosen). It expands the user's notes of Nov 11, 2026:

1. **A git hosting app in mr-pass**, with groups and repositories, public and private, as GitHub and GitLab have. It
   is the landing app of moqui.org and it includes the forum's screens, so that a group, and perhaps a repository, has
   its forum.
2. **The `moqui` group** is the server's home group. Other groups host third-party open source components.
3. **The mailing lists** move to the forum of the `moqui` group, and Discourse's forum to the same place.
4. **GitHub becomes a mirror**: the repositories are pushed to it from the git server at moqui.org. Issues and pull
   requests move.
5. **CI** is built and run by us: builds and tests on a push and before a deploy, in an outside environment started
   through an API for containers.
6. **The web site** is no longer only an information site: it is the one place for the project.

Moqui's own needs come first, and the apps are general: a dev shop that runs Moqui uses them inside the company. That
is the **group server** of [Run-Deploy](../docs/Run-Deploy.md#dev-instances-with-a-dev-group-server), a production
instance that hosts a team's repositories, work items and LLM gateway for its dev instances. moqui.org is a public one.

Most of the hosting core is built already ([§2](#2-what-is-there-today)), so this is a plan of gaps
([§3](#3-gaps)), not of a new system.

Paths: `git/` is `crates/moqui-git/src/`, `host/` is `crates/moqui-host/src/`, `core/` is `crates/moqui-core/src/`,
`fw/` is `runtime/base-component/framework/`, `mantle/` is `runtime/component/mr-mantle/`, `pass/` is
`runtime/component/mr-pass/`, `boss/` is `runtime/component/mr-boss/`. Every other path is relative to this
repository.

Contents: [0 decisions](#0-decisions) · [1 what moves](#1-what-moves) · [2 today](#2-what-is-there-today) ·
[3 gaps](#3-gaps) · [4 topology](#4-topology-and-deployment) · [5 Smithy](#5-smithy) ·
[6 forum and lists](#6-the-forum-and-the-mailing-lists) · [7 the site](#7-the-site) · [8 CI](#8-ci) ·
[9 GitHub](#9-github) · [10 contributor agreement](#10-the-contributor-agreement) · [11 Dev AIDE](#11-dev-aide) ·
[12 framework changes](#12-framework-changes) · [13 phases](#13-phases) · [14 tests and docs](#14-tests-and-docs) ·
[15 later](#15-later) · [16 open questions](#16-risks-and-open-questions)

---

## 0. Decisions

| # | Decision | Why |
| --- | --- | --- |
| D1 | moqui.org is one production instance, a cluster later, with mr-mantle, mr-pass and mr-boss: a **public group server**. Smithy's first version runs where the git data is, not as a remote PASS frontend. | A remote PASS host has no `/git` and no git rows. One server is the least to run, and Run-Deploy's group server is this shape already. |
| D2 | The app is **Smithy**, a PASS app in mr-pass. Its rules and entities are in mr-mantle (`mantle.smithy`), on the PASS pattern: registered in `PublicApps.js` and `Apps.js`, pages calling `pass.Facade.call#Operation`, public accounts with roles in data, and 404 for what a caller may not see. It needs Mantle, not BOSS. | Decided with the user, Nov 11, 2026 (the app and its name; "Forge" is overused). Public-facing apps go in mr-pass and internal ones in mr-boss (`boss/plan/impl-projects.md` §9.4). |
| D3 | The framework stays the git host. Anonymous read, repository paths, forks, push-to-review, push mirrors and the CI engine go in `moqui-git` and the framework. Smithy and Projects' Code share what is underneath, and neither depends on the other. | It is [design-dev-aide](design-dev-aide.md#0-decisions) D7's rule. BOSS Projects' Code gets each of them too, and a dev shop with BOSS alone is served. |
| D4 | `repositoryId` stays the key. A unique `repositoryPath` (`moqui/mr-moqui`) is what URLs carry: `/git/<path>.git`, SSH `<path>.git`, and pages at `/<group>/<repo>` with `/-/` before a section (`/moqui/mr-moqui/-/issues/12`). The page URL itself clones. | Rows that name a repository keep their ids, and a rename or a transfer changes one field. `/-/` keeps a repository's name from colliding with a section's, as GitLab does. |
| D5 | Visibility is public, members or private, on a group and on a repository, and a repository is no wider than its group. Roles (Owner, Maintainer, Developer, Reporter, Guest) are membership rows on a group, inherited by its subgroups and repositories, or on one repository. One Mantle check gives a repository's rights from those rows and from project roles. | PASS roles live in data, never in user groups (`pass/plan/design-forum.md` §5.1). The git server and the pages must agree, so there is one check (`mantle.work.GitServices.check#GitRepositoryAuthz` today). |
| D6 | An issue is a Mantle work item: a task of the project linked to the repository or the group. Smithy shows the public projection of it (public items, outward messages). BOSS Projects, when it is loaded, is the maintainers' view of the same rows. An issue may be filed confidential: any account files one, and only the owners of the repository or the group read and edit it. | Decided with the user, Nov 11, 2026, confidential issues with it. The tracker, its links to commits, branches, tags and merge requests, and its import exist; a second tracker would repeat them. |
| D7 | Contributions come by a fork or by push-to-review: a push to `refs/for/<branch>` opens a merge request with no fork. "Merge request" is the one term. | Decided with the user, Nov 11, 2026. People from GitHub expect forks, and a third party wants its own copy. Push-to-review is the least work for one patch, and for an agent. The term is the framework's and Projects'. |
| D8 | The conversation of an issue or a merge request uses the forum's post engine: revisions, reactions, watch levels, mail with reply by email, the review queue, trust levels and the spam rules. Markdown becomes an authoring format beside rich text. | Confirmed by the user, Nov 11, 2026: reuse and share what the forum has. A public tracker needs all of the forum's moderation and mail, and `ForumPostRevision` has `sourceFormat` and `sourceText` already. How a topic belongs to a work item is the design's (question 3). |
| D9 | A site has one forum. A group has a category in it, and a repository may have a subcategory. Forum pages render inside the group's or the repository's shell, by a scope and a link base. A mailing list is a category in mailing-list mode. | Categories are a tree within one site already, and a parent is an access ceiling. One forum keeps one search, one account and one set of mail preferences. |
| D10 | CI is an engine in the framework. A pipeline is a file in the repository, `ci.xml` with an XSD. gitECA rules queue the runs. Results are `GitCommitStatus` rows. A job never runs on the server: it runs in a container or a VM that lasts for the job, started through an `InstanceHostType` driver. | The branch rules' `requiredStatusContexts` gate merges on statuses today, so nothing new gates. An XML file with an XSD gets the Dev AIDE's schema editing and `moqui check`. Code from a merge request is untrusted. |
| D11 | A repository that is a Moqui component and has no `ci.xml` gets a built-in pipeline: a runtime with the framework and the component's dependencies, `./moq test <component> --junit`, and the component's regression budget. | Every component is tested the same way ([Build-Test](../docs/Build-Test.md#the-two-suites)), so a third party's component and a dev shop's are checked with no file written. |
| D12 | The GitHub mirror is our own push client in `moqui-git`, over the pkt-line and pack writing code the server has. Every issue and pull request is imported, open and closed, and GitHub is then closed to new ones. | gitoxide has no push, and a `db` repository has no directory for a spawned `git`. The import was decided with the user, Nov 11, 2026: one searchable record, and an importer a dev shop moving in can use. |
| D13 | The contributor agreement (`AUTHORS.md`, [Contributions](../docs/dev/Contributions.md)) is a computed status of every merge request, which the branch rule requires. A setting of a group, off by default. | The check is done by hand with `gh` and `git log -S` today. A dev shop has no such rule. |
| D14 | Smithy never runs code and has no working tree. It does edit text: a file is edited on its page and saved as a commit, to the branch where the account may push, else as a merge request ([§5.7](#57-editing-a-file-on-its-page)). | Decided with the user, Nov 11, 2026: an edit on the fly with a commit is very useful. The commit goes through the hosting services and their checks, as a push does, so review and CI see it. What runs code is the Dev AIDE on a dev instance and a CI runner (design-dev-aide D22). |
| D15 | The site is Smithy. The home group's page is the landing page of moqui.org, the documentation is rendered from the repositories' `docs/` at a branch or a tag, and news is a forum category until the Publishing app. | The documentation is in the repositories now, and it is versioned with the code. One app, not a site to keep beside it. |
| D16 | moqui.org keeps its repositories as `db` repositories, in Postgres. | Decided with the user, Nov 11, 2026: the data stays in one place, so a backup, a restore and a copy of the instance are the database's. A cluster needs `db` too. |
| D17 | Every post needs an account with a confirmed email address: an issue, a comment, a merge request, a review, a forum post, a push, a message to a list. Nothing is posted for an address that is not an account's. | Decided with the user, Nov 11, 2026. It is the forum's rule for local registration already, and the first defence against spam. |
| D18 | Smithy has no moderation of its own. What a public account writes goes through one moderation layer, the forum's in Mantle today, by a target type and a scope that do not name the forum ([§6.4](#64-moderation-and-spam-one-layer)). What mail has in place joins it here; a wider framework for internal mail and chat is later and not this plan's. | Decided with the user, Nov 11, 2026: the forum's moderation is the start of a larger spam and moderation framework, so it is built to be shared now. |
| D19 | Servers and runners start on the standard Docker and Podman interfaces, with images built from the files in `podman/`. Kubernetes, AWS and other providers' APIs are later. | Decided with the user, Nov 11, 2026. Any machine with Docker or Podman will do, so the choice of provider does not shape the software. |

---

## 1. What moves

| Today | Lands in | The old place |
| --- | --- | --- |
| GitHub repositories of the `moqui` organization (the five mr-* repositories of the [README](../README.md#repositories), the Java ones, `moqui-site`) | Hosted repositories of the `moqui` group ([§5](#5-smithy)) | A push mirror, read only ([§9](#9-github)) |
| GitHub Issues | Work items of each repository's project, with their numbers ([§5.5](#55-issues)) | Closed to new issues, each with a link to its new place |
| GitHub pull requests | Merge requests, with their commits, reviews and line comments ([§5.6](#56-merge-requests-forks-and-push-to-review)) | A pull request template that points to moqui.org |
| GitHub Actions, where a repository still has a workflow | Pipelines ([§8](#8-ci)) | Removed |
| `moqui-site`, the web site | The `moqui` group's home, Docs and Releases ([§7](#7-the-site)) | Old URLs redirect |
| Discourse, forum.moqui.org | The site's forum, by the importer that is built ([§6](#6-the-forum-and-the-mailing-lists)) | Old URLs redirect (`ForumLegacyUrl`) |
| Google Groups mailing lists | Forum categories in mailing-list mode ([§6.3](#63-the-mailing-lists)) | Archived read only, with a last message that names the new address |
| `moqui-board@googlegroups.com`, where security reports go ([Attack-Surface](../docs/security/Attack-Surface.md)) | Confidential issues, and `security@moqui.org` ([§5.5](#55-issues)). moqui.org has no mail server today; DG9 gives it one | The docs name the new address |
| The contributor agreement check, by hand | A status of each merge request ([§10](#10-the-contributor-agreement)) | |

What is in each of these (the repositories and their sizes, the counts of issues and pull requests, the lists and
their members, the Discourse plugins in use, the site's pages and URLs) is DG0's inventory. This plan does not guess
them.

---

## 2. What is there today

| Area | What exists | Where |
| --- | --- | --- |
| Git server | Clone, fetch and push over smart HTTP and the built-in SSH server (protocol v2, shallow clones, LFS), on gitoxide's objects and packs with a protocol layer of our own. Repositories are `local` (bare, on disk), `db` (rows, in the backup) or `remote` (a read-only mirror, fetched on a schedule). | `git/server/` (`upload.rs`, `receive.rs`, `pack_in.rs`, `pack_out.rs`, `pktline.rs`, `lfs.rs`), `host/git_http.rs`, `host/ssh/git.rs`, [Git-Hosting](../docs/interfaces/Git-Hosting.md) |
| Hosting | gitECA rules on pushes and fetches, branch rules (required approvals and status contexts, merge methods), merge requests with reviewers, merge by commit, squash, rebase or fast-forward, compare, archive, webhooks signed as GitHub signs them, and commit statuses in GitHub's shape for outside CI. | `git/hosting.rs`, `merge.rs`, `rules.rs`, `events.rs`, `git/service/hosting.rs`, `fw/entity/ResourceEntities.xml` (`GitRepository`, `GitMergeRequest`, `GitBranchRule`, `GitCommitStatus`, `GitWebhook`) |
| Access | A repository is an artifact, `AT_GIT_REPOSITORY`, with view, create, update and delete. Mantle's check gives the rights by project role. Keys are `UserSshKey` rows and tokens are login keys. | Git-Hosting "Access"; `mantle/service/mantle/work/GitServices.xml` |
| Code screens, internal | Repositories, Files, Commits, Branches, Tags, Activity, Settings, Compare, CommitDetail, merge requests with reviews and line comments, Git access. For internal users, and read only for a client. | `boss/screen/Projects/Code/`, `boss/plan/design-projects.md` §7 |
| Work items | Tasks, milestones, releases, labels (`WorkEffortCategory`), Requests. Commits, branches, tags and merge requests link to them by key, by `Task:` trailers and by `Fixes`. A CSV import with a GitHub preset. | mr-mantle `WorkEffort*`, `mantle/service/Work.gitecas.xml`, `WorkGit.secas.xml`; `boss/plan/design-projects.md` §5 |
| Forum | Categories, topics, tags, Q&A, polls, private conversations, trust levels, a review queue, watch levels, digests and mailing-list mode, reply by email, RSS, and a Discourse importer with redirects of the old URLs. Anonymous reading works without JavaScript. S1 to S3 are built. | `pass/screen/Forum/`, `mantle/service/mantle/forum/`, `pass/plan/design-forum.md`, `implementation-forum.md` |
| Public sites | A `public-site` in the conf binds a host to a site with a mount for each app. Public accounts register, are invited, and sign in with a second factor or through a provider (GitHub among them). | `core/screen/public_site.rs`, `pass/docs/Deployment.md`, `mantle/entity/PublicEntities.xml` |
| Mail | An SMTP listener that can take port 25 itself and check SPF, DKIM, DMARC and ARC; mailbox polling; email ECA rules; signed outbound delivery. | `host/smtp.rs`, [Mail-Inbound](../docs/interfaces/Mail-Inbound.md) |
| Containers | Services that create, start, stop and remove a container through the Docker Remote API, with `InstanceHost` and `InstanceHostType` rows that name a host type's services. A container image of the server. | `fw/service/org/moqui/dev/InstanceServices.xml`, `fw/entity/ServerEntities.xml`, `podman/` |
| Tests | `moqui test --workers N` runs specs in N processes, each with its own database, and writes JUnit. | [Build-Test](../docs/Build-Test.md) |
| Deploy | A component is fetched from git, previewed and deployed live, on one server or a cluster. | [Component-Deploy](../docs/tools/Component-Deploy.md) |
| Dev AIDE | Working trees through the `git` command, Publish, the task panel over a Mantle or a `moqui-rest` source, a shared mode. It has no merge request, review or check screens, and a working tree does not know its hosted repository. | [design-dev-aide](design-dev-aide.md) |
| Group server | A production instance with BOSS that hosts a team's repositories, work items and LLM gateway. | [Run-Deploy](../docs/Run-Deploy.md#dev-instances-with-a-dev-group-server) |

---

## 3. Gaps

**Framework** (each is an item of [§12](#12-framework-changes))

- A request with no credentials always gets 401 (`git/server/mod.rs`), so a public repository cannot be cloned
  anonymously.
- A repository's address is one path segment, its `repositoryId` (`host/git_http.rs`). There are no groups.
- `SSH_GIT` is granted to ADMIN and BOSS's groups. A public account has only the automatic `ALL_USERS`, and a
  permission granted to `ALL_USERS` counts only with a membership row, so a public account cannot use git over SSH.
- No forks: a merge is between branches of one repository.
- No push to another server.
- No CI: statuses are posted by outside CI, and webhooks start it.
- REST has the commit statuses. Merge requests, reviews and threads are services with no REST resource.
- No ACME client: certificates come from outside, into `runtime/cert`.
- Content blobs are in the database only. Build artifacts and caches would be too.
- No code search.

**Mantle**

- No group or repository membership other than project roles.
- No public projection of work items. The planned Client portal defines one for clients
  (`boss/plan/impl-projects.md` §9), not built.
- The forum's `PublicDiscussionLink` knows a product, an article and a case, not an issue or a repository.
- `List-Id` is one per site (`mantle/service/mantle/forum/DeliveryWorker.js`); a list needs its own.
- The forum's moderation knows a post, a topic and an account as targets. A repository or a group cannot be reported
  or hidden.
- A post that comes by mail does not bring the mail gateway's spam verdict to the forum's review.
- No importer of an mbox into the forum, and none of GitHub's issues and pull requests.
- The Discourse importer was rehearsed on a made-up backup only.

**mr-pass**

- No app for code. The Account app has no screen for SSH keys and access tokens.
- Forum pages build every link from the forum's own base, and the header shows the forum's navigation
  (`pass/screen/Shared/View.js`), so they cannot yet be shown inside another app.
- Members write rich text. Markdown is read only by the Discourse import.
- No landing or documentation pages other than a Shop's.

**Dev AIDE**: [§11](#11-dev-aide).

**Operations**: a provider, a server, backups off the machine, mail reputation for a new sending domain, monitoring,
and a restore that has been tried.

---

## 4. Topology and deployment

**The public group server.** One production instance (Maximal) with mr-mantle, mr-pass and mr-boss:

```
 visitors, members   https://moqui.org      Smithy at /, Forum, Account      PASS public site
 git clients         https /git, ssh        hosted repositories              framework
 maintainers         https://moqui.org/qapps  BOSS Projects, System, Monitor  internal accounts
 dev instances       git, /rest, /llm       a group server, as Run-Deploy's
 runners             containers or VMs      started by the server, they call back over https
 GitHub              push mirror            from gitECA, queued
```

It is a new row of Run-Deploy's topology table and a section beside "Dev instances with a dev group server", written
when DG9 builds it. A cluster comes when one server is not enough: Enterprise, `db` repositories, and `/git` and SSH
on every member.

- **Repositories** are `db` repositories (D16): objects, refs and LFS files are rows and content blobs of the content
  database ([Run-Deploy](../docs/Run-Deploy.md#content-database)), with the issues, the forum, run logs and artifacts.
  A copy of the instance is a copy of its database. DG1 measures clone and push of mr-moqui's history against
  Git-Hosting's table, and what a fork costs.
- **Backups.** The database backup, sent off the machine, is all of it. The GitHub mirror is a second copy of the
  code, not of the issues. DG9 restores a backup to a second machine before cutover. Run logs, artifacts and caches
  have a retention time, so they do not grow the database without end.
- **Mail.** moqui.org has no mail server today. The server takes mail itself (`smtp_internet`) or through an edge
  node ([Edge-Node](Edge-Node.md)), and signs what it sends. A list's mail at volume from a new domain needs its
  reputation built (question 11).
- **Certificates.** certbot writes into `runtime/cert`, which the server re-reads, until F10.
- **Runners.** On a machine other than the server's, always ([§8.4](#84-safety)).
- **The machines** run Docker or Podman, and nothing else is asked of a provider's API (D19). The server is the
  image `podman/build.sh` makes from `podman/Containerfile` (the Maximal package on Debian 13), run by the quadlet
  (`podman/quadlet/`) or a compose file, with one data volume; or the systemd unit with no container
  (`podman/README.md`). A runner host is a second machine with the Docker or Podman API open to the server only.
- **The provider** is then any that rents such machines. What it must still have: port 25 open with reverse DNS, IPv6,
  storage that can grow, and a place off the machine for backups. Kubernetes, AWS and others are later
  ([§15](#15-later)).

**A dev shop's group server** is the same software with other settings: registration closed or by invitation, every
group private or members-only, the agreement check off, runners on the shop's own Docker or Podman host, `local` or
`db` repositories as it likes, and no import. Staff
work in BOSS Projects; Smithy serves clients, partners and anyone without a BOSS account. A shop without BOSS runs
Smithy alone on Mantle.

---

## 5. Smithy

The detailed design, page by page as `pass/plan/design-forum.md` is, is `pass/plan/design-smithy.md`, written in DG0.
This section fixes its shape.

### 5.1 Groups, repositories and names

- A **group** has a path segment, a name, a description, an avatar, a visibility, and settings that its repositories
  inherit (the agreement, runner pools, quotas). A group may hold subgroups. A site has a **home group**, whose page is
  the site's home; on moqui.org it is `moqui`.
- A **personal namespace** is an account's handle (`PublicSiteProfile`), for its forks and its own repositories.
  Handles and group paths are one set of names.
- A **repository** in Smithy is a row beside the framework's `GitRepository`: its group or namespace, its slug, its
  visibility, what it has turned on (issues, merge requests, pipelines, a forum category, docs), the project its
  issues are in, and the repository it was forked from.
- **Records** (mr-mantle, `mantle.smithy`; the names are the design's to settle): `SmithyGroup`, `SmithyGroupParty`,
  `SmithyRepository`, `SmithyRepositoryParty`, and the import's run and map rows. Every row carries its `siteId`, as
  PASS rows do.
- **Who may create.** A site setting: operators only, by request and approval, or any verified account. Personal
  repositories and forks likewise. Question 8 asks what moqui.org starts with.
- **Quotas** per group and per account: repository size, LFS bytes, runner minutes. A site sets the defaults.
- **Rename and transfer** change `repositoryPath`, and the old path redirects (pages and git) until it is taken.

### 5.2 Access

| Who | Code | Issues and merge requests | Settings |
| --- | --- | --- | --- |
| Anyone, on a public repository | read, clone | read | |
| An account with a confirmed address, on a public repository | fork, push-to-review, propose an edit of a file | file (confidential too), comment, react, open a merge request | |
| Guest | read a members-only or private one | read | |
| Reporter | | label, assign, close, mark duplicate | |
| Developer | push a branch or a tag, fast-forward | review, approve | |
| Maintainer | force, delete a branch or tag, merge to a protected branch | merge, manage milestones and releases | branch rules, webhooks, mirrors, pipelines' secrets |
| Owner | | read and edit confidential issues | members, visibility, transfer, archive, delete |

- **Every write needs an account with a confirmed email address** (D17), signed in with a password and a second
  factor or through a provider whose address the site trusts. An account without one reads, and is asked to confirm.
- The rows map to `AT_GIT_REPOSITORY`'s actions: view from Guest (or anyone, for a public repository), create and
  update from Developer, delete from Maintainer.
- One Mantle check answers for the git server, the REST API and the pages: the rights of the Smithy membership rows
  and those of project roles, whichever gives more. Branch rules narrow them further, as today.
- What a caller may not see is 404, on a page and in git.
- A sanction (a suspension, a silence) applies to everything the account may write in its scope, Smithy's
  conversations and the forum's alike ([§6.4](#64-moderation-and-spam-one-layer)).

### 5.3 URLs

Smithy may mount at `/`, as Forum and Shop may. Then:

| Path | What |
| --- | --- |
| `/` | the home group's page |
| `/explore` | groups, repositories, components |
| `/<group>` and `/<group>/<subgroup>` | a group |
| `/<group>/<repo>` | a repository, at its default branch |
| `/<group>/<repo>/-/tree/<ref>/<path>`, `/-/blob/…`, `/-/commits/<ref>`, `/-/commit/<id>`, `/-/compare/<a>...<b>` | code |
| `/<group>/<repo>/-/issues`, `/-/issues/<n>` | issues |
| `/<group>/<repo>/-/merge-requests`, `/-/merge-requests/<n>` | merge requests |
| `/<group>/<repo>/-/pipelines`, `/-/pipelines/<n>`, `/-/releases`, `/-/docs/<ref>/<path>`, `/-/forum` | the rest |
| `/<group>/-/issues`, `/-/forum`, `/-/members`, `/-/settings` | a group's sections |
| `/<group>/<repo>.git`, `/git/<group>/<repo>.git` | git |

A group's path may not be one of the reserved first segments (`mantle/service/mantle/public/PublicApps.js`
`reservedPaths`) nor another app's mount (`forum`, `account`, `members`).

### 5.4 Screens

| Screen | What |
| --- | --- |
| Home, GroupDetail | Overview (the group's landing text, pinned repositories, news, releases, activity), Repositories, Issues (across its repositories), Forum, Members, Settings |
| Explore | Groups, repositories and components, with a search |
| RepositoryDetail | Files (tree, README, file view, raw, download, blame, Edit), Commits, Branches, Tags, Issues, Merge requests, Pipelines, Releases, Docs, Forum, Settings |
| CommitDetail, Compare | As Projects' Code has them, with the checks of a commit |
| IssueDetail, NewIssue | [§5.5](#55-issues) |
| MergeRequestDetail | Overview (the conversation, reviewers, checks, the agreement), Commits, Changes (the diff with line comments), Checks |
| PipelineDetail | Jobs, a job's log as it runs, tests that failed, artifacts ([§8](#8-ci)) |
| Releases, ReleaseDetail | [§5.8](#58-releases-and-the-component-directory) |
| Settings | General, Members, Branch rules, Webhooks, Mirrors, Pipelines (pools, secrets, environments), Agreement, Forum, Archive |
| Account › Git access | SSH keys and access tokens, a card of the PASS Account hub |
| Import | GitHub ([§9](#9-github)), for an operator or a group's owner |

- Pages are PASS's: HTML with `.qvue` where a page needs it (the diff with line comments, the composer), at 390 px and
  on the desktop, and readable without JavaScript where they are public. No terminal form, as PASS has none.
- The reads are the framework's hosting services (`org.moqui.git.GitServices`, `GitReviewServices`), behind Mantle
  operations that apply the check of §5.2. The diff, tree and file widgets are the framework's (`m-diff-view`,
  `m-file-tree`, `m-code-editor` read only), which Projects' Code may adopt too.
- The planned Client portal's phase C5, shared repositories (`boss/plan/impl-projects.md` §9.3), uses these
  operations with a client's grant, not a second browser.

### 5.5 Issues

- A repository with issues on names its project (`WorkEffortGitRepository`, which exists). A group may keep one
  project for all of its repositories. An issue is a task of that project.
- **What the public sees**: an item whose visibility is public (`WevGeneral`), with its title, text, state, labels,
  milestone, assignees' handles, linked commits and merge requests, and the outward conversation. Never: Chatter
  notes, time, cost, estimates, allocations, internal members, or an item that is not public. A commit that names
  such an item shows it unnamed. It is the Client portal's rule ("Nothing internal leaves", impl-projects §9.2) with
  the public as the audience, and the two share their Mantle projections.
- **Confidential issues.** Any account files one, as it files any issue, by a mark on the form. Only the Owners of
  the repository or of its group read and edit it; it is in no list, search or feed for anyone else, and its number
  answers 404. The reporter keeps what they filed: their own text and the replies the owners address to them. It is
  how a security report is filed. An owner may make it public later, as it is or as a new issue.
- **`security@moqui.org`** files a confidential issue in the home group's tracker, as the account whose confirmed
  address sent it. Mail from an address that is no account's is not posted (D17): the sender gets one reply with a
  link that confirms the address and files the report, and until then it waits as a held item that the owners see in
  their review queue ([§6.4](#64-moderation-and-spam-one-layer)). A group may have such an address of its own.
- **Numbers.** A work item has its key (`PROJ-12`). An issue also has a number in its repository (`#12`), kept from
  GitHub for an imported one, so `#12` in old commits, comments and links still finds it. `Fixes #12` in a commit to a
  repository completes the item as `Fixes PROJ-12` does. Question 9.
- **Labels, milestones, releases** are Mantle's (`WorkEffortCategory` of type `WcatLabel`, milestones, releases).
- **States.** The public sees open and closed, with the reason. The project's status flow is the maintainers'.
- **The conversation** is forum posts (D8): Markdown or rich text, mentions, reactions, edits with history, and mail
  with reply by email. A topic of the forum becomes an issue (as `create#CaseFromTopic` makes a case), and an issue
  links its discussion.
- **Templates and forms** for a new issue are files of the repository, later.

### 5.6 Merge requests, forks and push-to-review

- **Merge requests** are the framework's (`GitMergeRequest`, reviewers, approvals, required checks, the merge
  methods). Smithy adds the public pages, the conversation (D8), and the agreement ([§10](#10-the-contributor-agreement)).
- **Forks** (F4). A fork is a repository in the forker's namespace, or in a group the forker may create in, that
  names its parent. It shares its parent's objects, so it costs its own refs and its own new objects. A merge request
  names a source repository. A fork of a private repository is no wider than its parent. Maintainers may push to a
  merge request's source branch when its author allows it, later.
- **Push-to-review** (F5). `git push origin HEAD:refs/for/main` from any account that may view opens a merge request
  to `main`, or updates the one of the same author and topic (`-o topic=…`, `-o title=…`). The commits are kept under
  `refs/merge-requests/<n>/head`, which a fetch may ask for by name. Nothing else in the repository is writable to
  that account.
- **Fetching a merge request**: `git fetch origin refs/merge-requests/12/head`, for a fork's and a pushed one alike,
  which is how the Dev AIDE checks one out ([§11](#11-dev-aide)).
- **Review agents.** A triggered agent ([LLM-A2A-Assist](LLM-A2A-Assist.md)) may read a signed merge request and post
  a review as a comment. It runs nothing and writes no code.

### 5.7 Editing a file on its page

A file's page has Edit, for a text file under a size limit that is not kept in LFS (D14).

- **The editor** is the framework's code editor (`m-code-editor`), with a preview for Markdown. New file, Rename,
  Delete and Upload are the same flow.
- **Saving is a commit.** The dialog asks for the message and shows where it goes:
  - to the branch, when the account may push there and the branch rules allow a direct push;
  - to a new branch with a merge request, which is the default on a protected branch;
  - as a proposal, when the account may view and not push: the commit is kept under `refs/merge-requests/<n>/head`
    and the merge request opens, with no fork (F5).
- **The commit** is made by the hosting services (`commit#GitFiles`, which exists and checks the update right). Its
  author is the account, by its name and confirmed address; the committer is the server. It is recorded as any write
  is, so gitECA rules, pipelines, the agreement check and the mirror all follow.
- **A conflict.** The save names the blob it started from. When the file has changed since, the page shows both and
  asks.
- **One file a commit** in the first version. Editing many files, running them and seeing them live is the Dev AIDE's,
  on a dev instance.
- **What it is for**: a fix in the docs, a line in `AUTHORS.md` ([§10](#10-the-contributor-agreement)), a `ci.xml`
  setting, a small patch from someone who will never clone.

### 5.8 Releases and the component directory

- A **release** is Mantle's release with its tag, its published notes, and assets: files a pipeline uploaded for the
  tag (an instance archive from `./moq archive`, a container image's name), with their checksums. Downloads are public
  for a public repository.
- **The component directory.** A repository whose root has a `component.xml` is a Moqui component: Explore lists it
  with its name, version and dependencies, read at the default branch and at each release. A group serves an
  `addons.xml` of its components, so `./moq component-get <name>` takes a component from moqui.org as it takes one from
  a team's group server today ([Run-Deploy](../docs/Run-Deploy.md#dev-instances-with-a-dev-group-server)). Third
  parties' components are found here.

### 5.9 Search and abuse

- **Search** is one box over the public projections of each app (forum topics, issues, merge requests, docs,
  repositories), each keeping its own access rule, as design-forum §13.1 says of the shared search. Code search is
  later (F12).
- **Moderation** is not Smithy's own (D18): [§6.4](#64-moderation-and-spam-one-layer).
- **Repository abuse**: quotas, a report on a repository or a group, and Hide, which makes it 404 to all but its
  owners.
- **Rates.** Anonymous clones, archives, blame and large diffs are capped by client (`host/guard.rs`).

---

## 6. The forum and the mailing lists

### 6.1 The forum inside Smithy

- `pass/screen/Shared/View.js` takes a **scope** (a category and its subtree) and a **link base**, where it builds links
  from the forum's base now, and the route resolver knows `/-/forum` under a group or a repository. The page is the
  forum's own, inside Smithy's shell and navigation. `/forum` stays the whole site's forum.
- A group has a top category, made with it or linked later. A repository may have a subcategory. The category's
  visibility is no wider than the group's or the repository's.
- `PublicDiscussionLink` gains the source types issue, merge request and release: a release has its announcement
  topic, and a topic may be turned into an issue.
- **Markdown** is an authoring format of the composer, with fenced code blocks that are highlighted. It serves the
  forum and Smithy's conversations alike (D8).

### 6.2 Discourse

The importer is built (`mantle/service/mantle/forum/ForumImport.js`, design-forum §18.1). What is left is in
`pass/plan/implementation-forum.md`: a rehearsal on a real backup of forum.moqui.org, a decision for each plugin's
data, and what is not carried (edit history, category watch levels). DG0 gets the backup and DG6 rehearses.

### 6.3 The mailing lists

- A list is a category in mailing-list mode with its own address (`ForumMailRoute`, which exists) and its own
  `List-Id`, `List-Post` and `List-Archive` (the `List-Id` is the site's today).
- **Archives.** An importer reads an mbox, threads it by `Message-ID`, `In-Reply-To` and `References`, and makes topics
  and posts with their dates. An author is an unclaimed profile by address, which the person claims by registering
  with that address verified. It is resumable and safe to run twice, with an import map as the Discourse importer's.
  How Google gives a group's archive to its owners is DG0's check.
- **Members.** Each member of a list is invited by mail, and one click confirms the address, makes the account and
  sets its mail mode to every post for that category. Nobody is subscribed without the click.
- **Posting by mail** needs the account (D17), as the forum requires today. Mail to a list from an address that is no
  account's is not posted: the sender gets one reply with the link that makes the account, and the message is held
  for that long.
- **Cutover** of a list: the archive imported, the members invited, a last message on the old list, the old list made
  read only.

### 6.4 Moderation and spam: one layer

The forum's moderation is the start of a spam and moderation framework that internal mail and chat will share later
(D18). This plan does not build that framework. It makes Smithy the layer's second user in a way that does not have
to be undone, and it joins what is in place.

**What is in place**

| Where | What | Shared? |
| --- | --- | --- |
| Forum (Mantle, `mantle.forum`) | One review queue (held first posts, flags, classifier results, pending accounts) with claims, decisions, appeals and an audit of every action; rules by word, pattern and link; sanctions; trust levels with limits; rate limits; an optional classifier service (`pass_classifier_service`); a file scanner (`pass_scanner_service`). design-forum §10.1 | The layer |
| BOSS Comms › Moderation | The same queue for internal moderators, over the same services (`boss/service/ironstone/comms/ModerationServices.xml`) | Yes, already |
| Mail | The gateway's spam verdict (Rspamd, or the listener's own call to it) puts a message in Junk; a user's Spam and Not spam move it; a mail rule may mark spam ([Mail-Inbound](../docs/interfaces/Mail-Inbound.md)). Filtering in moqui itself is [Edge-Node](Edge-Node.md)'s late phase | A signal, joined here |
| Chat | A person's own block list (`XmppBlockedJid`). Rooms have no moderation ([Comms-Framework](Comms-Framework.md)) | Nothing to join; left out |
| Shop reviews | A review's moderation is a status change with its reason in the audit log | Left as it is |

**The seams**, which Smithy uses and which keep the layer free of the forum's name:

- **A gate.** Every path that writes public content calls one service with who writes, the scope, the kind of content
  and its revision, and gets allow, hold for review, or refuse. It is the forum's shared post gate, which its pages,
  mail in and its API go through (design-forum §13.4); Smithy's operations call the same one. No Smithy write
  reaches a row another way.
- **A target** is a type and an id: post, topic and account today; repository and group are added, for a report and
  for Hide. An issue's and a merge request's conversation are posts (D8), so they are covered with nothing added. An
  issue's title and a repository's name and description are checked by the rules as a post's text is.
- **A scope** is a site and, within it, a category. A group's and a repository's conversations are in their category
  (D9), so a scope names them with no new field, and an owner moderates the scope they own.
- **Signals** come in through one shape, a source and a verdict with its reason: a rule, a flag, the classifier, the
  file scanner, a rate limit, and, new here, the mail gateway's verdict.
- **Reputation and sanctions** are a party's in a scope, not a forum member's: trust earned in the forum counts for
  an account's first issue, and a suspension covers both.
- **Names.** The services and rows keep their `Forum` names in this plan. Renaming them is the framework's work,
  later, and nothing here depends on the names.

**What this plan builds**

- The target types repository and group, with Report and Hide.
- The mail verdict as a signal: a post that comes by mail (a reply, a message to a list, `security@`) whose message
  the gateway marked as spam is held for review and not posted.
- The moderation pages of the forum inside a group's and a repository's shell, by the scope and link base of
  [§6.1](#61-the-forum-inside-smithy), for the owners who are public accounts. Internal moderators use BOSS Comms as
  now.
- The policy's limits by trust level extended to Smithy's kinds: issues, merge requests, forks, repositories and
  pipeline runs in a day.
- Held items shown to the owners of the scope, which is how a report by mail from an unconfirmed sender waits
  ([§5.5](#55-issues)).

**What it leaves** to the later framework: internal mail's Junk and its reports as review items, chat rooms, shared
reputation of senders across mail and posts, and filtering of mail inside moqui.

---

## 7. The site

- **Landing.** The home group's Overview: what Moqui is, how to start, the repositories that matter, the latest
  release, news, and what is happening. Its text is a page of a content repository, edited as any file is.
- **Docs.** A repository's Docs renders its `docs/` (Markdown, through `git://` locations and the Markdown renderer
  the framework has) at the default branch, with a picker of branches and tags, a table of contents from the tree, and
  search. mr-moqui's `docs/README.md` is its index. The documentation of the Java framework that the site has now is
  inventoried in DG0: what of it is kept, where it lives, and what each old URL redirects to
  (`https://moqui.org/m/docs/...` is linked from [Contributions](../docs/dev/Contributions.md) and from each
  `AUTHORS.md`).
- **Downloads** are the releases ([§5.8](#58-releases-and-the-component-directory)).
- **News** is an announcements category of the home group's forum, with its RSS, shown on the landing page. The
  Publishing app (design-forum §13.3) replaces it later.
- **Other pages** (the license, the policy on AI, how to contribute, the board) are pages of the same content
  repository.
- **Redirects**: one table of old paths, of moqui.org and of forum.moqui.org, checked in DG7 against the inventory.

---

## 8. CI

### 8.1 Pipelines

A pipeline is defined in the repository, in `ci.xml` at its root, read at the commit it runs for. A sketch (the XSD,
`git-ci-3.xsd` beside `git-eca-3.xsd`, is F7's):

```xml
<ci>
    <pipeline name="check" on="push merge-request" branches="main release/*">
        <job name="framework" image="moqui-build" pool="linux" timeout-minutes="30">
            <cache key="cargo-${hash:Cargo.lock}" path="target"/>
            <step run="cargo lint"/>
            <step run="./moq test-framework --junit"/>
            <report junit="test-results/*.xml"/>
        </job>
    </pipeline>
    <pipeline name="nightly" on="schedule" cron="0 0 3 * * ?">
        <job name="acceptance" image="moqui-build" pool="linux" timeout-minutes="120">
            <step run="./moq accept-all --junit"/>
        </job>
    </pipeline>
</ci>
```

- **Events**: push, merge request (opened, a new head), tag, schedule, manual, and deploy ([§8.6](#86-deploys)).
- **A job** names an image, a runner pool and a time limit; it has steps (a command each), caches, artifacts, reports,
  service containers beside it (a database), and a matrix. Jobs of a pipeline run in parallel unless one `needs`
  another.
- **Statuses.** Each job writes a `GitCommitStatus` with the context `ci/<pipeline>/<job>`: pending when queued,
  then success, failure or error, with the run's page as its target. A branch rule requires contexts, as today.
- **The built-in pipeline** (D11) is what a Moqui component without a `ci.xml` runs: the framework at the version the
  component names, its dependencies by `component-get`, then `./moq test <component> --junit --workers N`. A spec over
  the regression budget fails it, as it does locally.
- **Outside CI** keeps working: the status REST and the webhooks are unchanged.

### 8.2 Records and the engine

- **Records** (framework, with the hosting rows): a pipeline run (repository, commit, ref, event, who, state, times),
  a job run (pool, image, state, exit, the log's blob, times), a step's result, artifacts and reports (content blobs),
  secrets (encrypted, scoped to protected branches, tags or an environment), environments, and runner pools.
- **Queueing.** The framework's gitECA rules queue runs on `post-receive` and on merge request events, as they
  refresh merge requests and send webhooks today (`fw/service/Git.gitecas.xml`). A job (ServiceJob) starts queued job
  runs up to each pool's limit, and another ends those past their time and removes what they left.
- **Pages.** Projects' Code and Smithy both show runs; System has the pools, the hosts and the queue.

### 8.3 Runners

1. The engine makes a **job token**: good once, for this job run, to read this repository at this commit and to post
   this run's log, results and artifacts. Nothing else.
2. It calls the pool's host type to **start** a container or a VM from the job's image, with the server's URL and the
   token in its environment and with the pool's limits (CPUs, memory, disk, time).
3. Inside, **`moqui ci-run`** (a subcommand of the binary, and a small static program for an image without Moqui)
   clones shallow at the commit, restores caches, runs the steps in order, sends log lines as they are written, and
   uploads the reports, the artifacts and the caches.
4. It posts the result. The engine writes the status, and calls the host type to **remove** the container.

- **Drivers** (D19). An `InstanceHostType` names its services, so a driver is a set of services and the engine does
  not change. The first is the standard container API: the Docker Engine API, which Podman serves on its socket too
  (`podman/README.md`: compose talks to the Podman API socket), over a Unix socket or TLS. The services that exist
  speak it (`init#InstanceDocker` and the rest, API v1.24) over plain HTTP on loopback, and they start a server that
  stays; F8 adds a run that ends, limits, and the socket and TLS. Kubernetes, AWS and other providers' APIs are
  drivers for later. A **registered runner** is a long-lived machine that asks for jobs, for what a container cannot
  be: macOS and Windows, which design-dev-aide's "Still open" says run nowhere today.
- **Images** come from the files in `podman/`. The server's image is the one `podman/build.sh` makes today. The
  runner's, `moqui-build`, is a second Containerfile beside `podman/Containerfile`, on the same Debian base: the
  pinned Rust toolchain (`rust-toolchain.toml`), the embedded Postgres payload, and optionally Chromium and
  OpenSearch; `build.sh` builds both. Whether a component's built-in pipeline (D11) can run in the server's image
  with the component added, needing the binary and not the toolchain, is DG5's check. The engine does not build
  images in the first version; a pool lists the images it allows.
- **Logs** are content blobs, written as they arrive and tailed to the page over the notification socket. JUnit is
  parsed: a run's page and the merge request list the tests that failed, with their files and lines.
- **Caches** are keyed by the job's `key`, per repository and per protected or unprotected ref. `MOQ_SCCACHE=1` with
  a cache backend is the build's own.

### 8.4 Safety

- A runner is never the server, nor on its machine or its network.
- A run for a fork, for a pushed-to-review merge request, or for an account without Developer has **no secrets**, a
  read-only cache, and a pool of its own. The first run of a new contributor waits for a maintainer's "Approve and
  run".
- Containers run without root and without privilege, with no host socket, with limits, and with egress rules a pool
  sets.
- A secret is given only to a job on a protected branch or tag, or in an environment; it is masked in logs.
- A job token cannot push, and dies with the job.
- Runner minutes are a quota ([§5.1](#51-groups-repositories-and-names)); question 10 is the cost and the abuse of
  open CI.

### 8.5 mr-moqui's own pipelines

What a run needs is in [Build-Test](../docs/Build-Test.md): the toolchain, bash, the embedded Postgres archive, and
optionally Chromium and OpenSearch. Node is not needed.

| When | What runs |
| --- | --- |
| A push or a merge request to mr-moqui | `cargo lint`, `scripts/check-packages.sh`, `./moq test-framework` |
| A push or a merge request to a component | the built-in pipeline: `./moq test <component>` |
| A merge to main | `./moq test-all` |
| Nightly, and before a release | `./moq accept-all --junit` |
| A tag | the release's archives and image, uploaded as its assets |
| macOS and Windows | the checks of design-dev-aide §19 "Platforms", on registered runners |

The suites' rules hold in CI as on a laptop: the regression suite on every change, acceptance only nightly and before
a release, budgets not raised.

### 8.6 Deploys

- An **environment** names a target instance (its URL and a token), the pipeline that must have passed for a commit,
  and who approves.
- A **deploy** event runs a pipeline for the commit to be deployed (the pre-deploy build and tests), then the deploy
  step: Component-Deploy's fetch, preview and deploy on the target, or a new archive or image for a framework change.
- **The gate**: an instance set to require it refuses to deploy a commit that has no passed run of the environment's
  pipeline.
- On moqui.org: a demo instance deploys main after every green merge; moqui.org itself deploys after a pass and a
  maintainer's approval.

---

## 9. GitHub

- **Before cutover**, moqui.org mirrors GitHub: `remote` repositories fetched on a schedule, which exist. DG1 shows
  them publicly, read only, so the site is real before anything moves.
- **The push mirror** (F6). A repository has mirror rows: a URL, a credential, which refs, the last result. A gitECA
  rule on `post-receive` queues a push; it is retried with a back-off, and a failure tells the maintainers. The mirror
  is forced to match: nothing is pushed to GitHub by hand after cutover.
- **The importer** (Mantle, for an operator or a group's owner, through GitHub's API with a token):
  - issues: number, title, text (Markdown), state and reason, labels, milestone, assignees, comments, reactions,
    dates;
  - pull requests, as merge requests in their final state, with their commits fetched from `refs/pull/<n>/head` to
    `refs/merge-requests/<n>/head`, their reviews and their line comments (`CommunicationEventGitAnchor`);
  - people: an author is a placeholder account by GitHub login, which the person claims by signing in with GitHub
    (the provider is built); until then the name shows with a mark that it was imported;
  - numbers are kept, and an old GitHub URL of an issue or a pull request has its redirect;
  - it resumes, can be run again without duplicates (an import map, as the forum's), waits on the API's rate limit,
    and reports what it could not carry.
- **Rehearsal**: one small repository first, then mr-moqui, on a staging instance, checked against GitHub's counts.
- **Cutover** of a repository: freeze (a notice, branch protection on GitHub), the last import, the hosted repository
  made from the mirror, the push mirror turned on, issues closed to new entries on GitHub, the README and the pull
  request template pointing to moqui.org.
- **Back out**: until issues are closed on GitHub, a repository goes back by turning the mirror around. After that,
  moqui.org is the record and its backups are what protects it.
- **Not built**: a sync in both directions. A pull request opened on GitHub after cutover is answered by hand with a
  link.

---

## 10. The contributor agreement

The rule is [Contributions](../docs/dev/Contributions.md): code is accepted only from people who have signed the
repository's `AUTHORS.md`, under both lists, each by a commit of their own account.

- **The check** is a status of the merge request, `agreement/authors`, computed when it opens and at each new head:
  the author of the merge request and the author of every commit in it have a line in both lists of `AUTHORS.md` at
  the target branch or in the merge request itself, and each line was added by a commit of that account. An account's
  identities are its verified addresses and, for history from GitHub, its linked login.
- **The branch rule** requires the context, so an unsigned merge request cannot be merged.
- **Merging** keeps a signature commit's author: a squash that would fold a signature into another person's commit is
  refused, with merge commit and rebase offered.
- **Signing on the page.** `AUTHORS.md` is edited on its page ([§5.7](#57-editing-a-file-on-its-page)): the account
  adds its line under both lists, and the save is a commit of its own, by that account, in the merge request or as a
  new one. Nobody needs a clone to sign.
- **Unsigned.** The page says what is missing and links to the Contributing section. The tools of agents (MCP, A2A,
  Assist) refuse the diff of an unsigned merge request and give its description: "work from its description, never
  from its diff".
- **Bots.** An account marked as a bot counts as signed when its operator has signed and says so on the merge request,
  which a maintainer confirms.
- **A setting** of a group or a repository: the file, the rule, on or off. Off by default.

---

## 11. Dev AIDE

The Dev AIDE's additions are designed in [design-dev-aide §21](design-dev-aide.md#21-dev-aide-with-a-group-server),
with their decisions (D20 to D22), framework changes (F17 to F19) and phases (P11 to P13). In short:

- **A group server is a connection**: one record gives the token, the addons repository, the task source and the
  gateway, where Run-Deploy lists four steps by hand.
- **A working tree knows its hosted repository**, by its origin's URL.
- **Merge requests in the Workspace**: Publish opens one; push-to-review when the user may not push; Fork; check out a
  merge request; review threads in the diff and in the editor's gutter; approve.
- **Checks**: statuses on Commits and Branches, a run's log, failed tests as links to file and line, re-run, and
  "Run locally".
- **Issues** in the task panel, and `Fixes #12`.
- **The agreement**: a warning before a commit by someone who has not signed, and "Sign".
- **Agents**: a `hosting` tool, and a git worktree for an agent's session.

What stays out of the Dev AIDE: anything that runs in production. The Dev AIDE loads on a dev instance only, so
Smithy, the CI engine and review agents on the server are not its. Smithy's edit of one text file on its page
([§5.7](#57-editing-a-file-on-its-page)) is a commit and not an IDE; design-dev-aide D22 records the line between
them.

---

## 12. Framework changes

| # | Change | Where |
| --- | --- | --- |
| F1 | Anonymous read: a request with no credentials is checked as the anonymous user, and gets 401 only when it may not view, so git still asks for a login on a private repository. Caps by client on anonymous clones, archives and diffs. | `git/server/mod.rs`, `host/git_http.rs`, `host/guard.rs` |
| F2 | `GitRepository.repositoryPath`, unique, resolved over HTTP and SSH, with the old one-segment ids still found; clone URLs from it; on a public-site host, git requests at a page's URL (`…/info/refs`, `git-upload-pack`, `git-receive-pack`, `info/lfs`) go to the git handler; redirects after a rename. | `host/git_http.rs`, `host/ssh/git.rs`, `core/screen/public_site.rs`, `fw/entity/ResourceEntities.xml` |
| F3 | Git over SSH by key for any account, when the conf says so, with the repository's check deciding the rest. Git access in PASS's Account app. | `host/ssh/server.rs`, `pass/screen/Account/` |
| F4 | Forks: `forkOfRepositoryId`; objects shared with the parent (a `db` fork reads its network's rows, a `local` one its parent's objects); collection that keeps what any fork of the network reaches; `GitMergeRequest.sourceRepositoryId`, and merges across the two. | `git/db.rs`, `fs.rs`, `gc.rs`, `hosting.rs`, `merge.rs` |
| F5 | Push-to-review: `refs/for/*` accepted from an account that may view, `refs/merge-requests/*` kept and fetched by name, push options for the title and the topic. The same for a commit made by the services, so a page's edit can be a proposal ([§5.7](#57-editing-a-file-on-its-page)). | `git/server/receive.rs`, `upload.rs`, `git/service/hosting.rs` |
| F6 | `push#GitRepository`: a push client over HTTPS, and over SSH on `russh`; mirror rows; a queue and retries. | `git/`, `fw/service/Git.gitecas.xml` |
| F7 | The CI engine: `ci.xml` and `git-ci-3.xsd`, the records, the queue, job tokens, the runner's REST, logs and artifacts, `moqui ci-run`. | `git/`, `host/`, `fw/` |
| F8 | Runner drivers: host type services for a run that ends (start with limits and an environment, remove, clean up); the Docker Engine API on Docker and on Podman, over a socket or TLS; the `moqui-build` image in `podman/`; registered runners. | `fw/service/org/moqui/dev/InstanceServices.xml`, `fw/entity/ServerEntities.xml` |
| F9 | REST for repositories, merge requests, reviews, threads and pipeline runs, in OpenAPI, for the Dev AIDE and for anyone's tools. | `runtime/base-component/tools/service/moqui.rest.xml` |
| F10 | An ACME client, for moqui.org and for a site's own host names; [Edge-Node](Edge-Node.md) notes its absence too. | `host/tls.rs` |
| F11 | A file system or S3-like store behind content blobs, for artifacts, caches and LFS. In [Remaining-Work](Remaining-Work.md#git-and-git-hosting) already. moqui.org keeps them in the database (D16), with retention, so this waits until their size asks for it. | `crates/moqui-entity/src/resource/blob.rs` |
| F12 | Later: code search over default branches. | |

F1 to F3 are small and DG1 and DG2 need them. F4 and F7 are the large ones.

---

## 13. Phases

Each phase ends usable. "Done" for a phase is its rules' regression specs in the repositories it touched, their
suites within budget, and its **Built** note here; the acceptance test a phase names is written with it and run then
([§14](#14-tests-and-docs)).

| Phase | What | Repositories | Done when |
| --- | --- | --- | --- |
| DG0 | The inventories of §1; a real Discourse backup; how each list's archive is exported; `pass/plan/design-smithy.md`; the questions of §16 that block DG1 to DG4 answered | docs only | the design is reviewed, and each inventory is a table in it |
| DG1 | Public read: F1, F2; Smithy's groups, repositories, visibility and membership; Home, Explore, GroupDetail, the code pages; over `remote` mirrors of GitHub | mr-moqui, mr-mantle, mr-pass | a visitor browses and clones `moqui/mr-moqui` on a staging host with no login; a private repository is 404 in the browser and in git |
| DG2 | Accounts and push: F3, Git access, roles, creating groups and repositories by the site's policy, Settings, branch rules, webhooks, quotas | the same | a new account adds a key, makes a repository and pushes to it; a Reporter's push is refused |
| DG3 | Issues: the projections, the pages, numbers, labels, milestones, confidential issues, Markdown, the conversation with mail and reply by email | mr-mantle, mr-pass, mr-boss | an issue filed in Smithy is worked in Projects, answered by mail, and closed by a commit; nothing internal shows; a confidential issue is 404 to all but its owners and its reporter; an account with no confirmed address cannot post |
| DG4 | Merge requests: the pages with line comments, F4, F5, editing a file on its page, the agreement check | all | an outside account forks, pushes, is told to sign, signs on the page, and is merged; another pushes to `refs/for/main`; a third fixes a line of the docs on its page and the merge request opens |
| DG5 | CI: F7, F8 with the container driver on Docker and on Podman, the `moqui-build` image, the built-in pipeline, mr-moqui's pipelines, approvals, secrets, quotas | mr-moqui, then the apps' pages | a push runs `test-framework` in a container on another machine, and a red run blocks the merge |
| DG6 | The forum in Smithy and the lists: scope and link base, categories, `List-Id`, the mbox importer, the invitations; the moderation layer's part of [§6.4](#64-moderation-and-spam-one-layer) | mr-mantle, mr-pass | a list's archive and a real Discourse backup import on staging; a reply by mail to a list message is a post; mail the gateway marked as spam is held; a reported repository is hidden by its group's owner |
| DG7 | The site: landing, Docs, Releases, the component directory, search, redirects | mr-pass, mr-mantle | the old links of moqui.org and forum.moqui.org in the inventory land where they should; `component-get` takes a component from the staging host |
| DG8 | GitHub: F6 and the importer, rehearsed | mr-moqui, mr-mantle | every issue and pull request of one repository is on staging with its number, and its mirror on GitHub is current after each push |
| DG9 | Production and cutover: the server from the `podman/` image, backups and a restore, a mail server for moqui.org (it has none today) with `security@`, certificates, monitoring; then the forum, the lists, and each repository | operations, docs | GitHub's issues are closed to new entries, the mirrors run, and the old forum and lists are read only |
| DG10 | Deploy environments and the gate | mr-moqui | moqui.org deploys itself from a passed pipeline, with an approval |

- DG3, DG5 and DG6 do not depend on each other, and DG8's importer needs DG3 and DG4.
- The Dev AIDE's P11 follows DG2 and DG4, and P12 follows DG5.
- DG1 alone is worth having: a public, read-only moqui.org over mirrors, before anything moves.
- F10 and F11 are taken up when DG9 and DG5 need them, not before.

---

## 14. Tests and docs

**Tests**, by [the testing skill](../.agents/skills/testing/SKILL.md) and the two suites:

- Regression specs for each rule, in the component that has the rule: Mantle's for access, projections, numbers and
  the agreement; the framework's for anonymous read, paths, forks, push-to-review, the push client and the engine.
- One access spec for the app, `SmithyAccess`, as PASS's other apps have one: every operation against anonymous, a
  signed-in stranger, and each role, on a public, a members-only and a private repository.
- Git protocol tests with the real `git` client, as the hosting tests run it (askpass, credential helpers and global
  config off).
- The engine against a fake host type in regression; the container driver against a real Docker and a real Podman
  is an opt-in test.
- Importers against recorded fixtures (a small mbox, a recorded set of API responses); a run against the real
  services is a rehearsal by hand, on staging.
- Acceptance walks, written with their phase: an outside contribution end to end (DG4), a pipeline end to end (DG5).
  No sweep of Smithy's screens in regression.

**Docs** that change as phases are built:

- [Run-Deploy](../docs/Run-Deploy.md): the public group server as a topology and a section; the dev group section,
  whose "Not built" list shrinks.
- [Git-Hosting](../docs/interfaces/Git-Hosting.md): anonymous read, paths, forks, push-to-review, mirrors, and its
  Limits.
- A new guide for pipelines and runners; [Build-Test](../docs/Build-Test.md) links to it.
- [Contributions](../docs/dev/Contributions.md): the check in place of the `gh` commands, and the guide's URL.
- `pass/docs/Deployment.md`: Smithy as an app; `pass/README.md`.
- The [README](../README.md)'s repository links, and the forum badges of the components' READMEs, at cutover.
- [Remaining-Work](Remaining-Work.md): each item built leaves it.

---

## 15. Later

- Runner and server drivers for Kubernetes, AWS and other providers' APIs (D19).
- The wider spam and moderation framework, with internal mail and chat in it
  ([§6.4](#64-moderation-and-spam-one-layer)).
- Code search (F12), and search inside a repository at a ref.
- Moqui as an identity provider, so a dev instance and other tools sign in with moqui.org. Run-Deploy lists it as not
  built.
- Passkeys for accounts.
- The Publishing app, in place of the announcements category.
- Security advisories: a confidential issue that is published with its fix.
- A merge queue; a preview instance for a merge request, started as a runner is.
- Registries: container images, and whatever a component's release needs beyond a file.
- Issue forms and merge request templates as files of a repository.
- Remote PASS frontends for Smithy, for sites that keep no business data on the public machine.
- Signed commits and tags shown as verified; SHA-256 repositories; partial clone.
- Federation between Smithy servers.

---

## 16. Risks and open questions

Each has what this plan proposes. An answer is added under its question with its date.

1. **The hosting provider.** Not chosen. Proposed: DG0 compares three by [§4](#4-topology-and-deployment)'s list and
   by price, and DG5's first driver is `docker` on a rented machine, so the choice does not hold up CI.
   - Answered Nov 11, 2026: start with the standard Docker and Podman interfaces, and build the images from the
     existing `podman/` files; Kubernetes, AWS and the like come over time, later (D19, [§4](#4-topology-and-deployment),
     [§8.3](#83-runners)). The provider is then any that rents a machine, and no comparison is needed first.
2. **`local` or `db` repositories on the first server.** `local` clones fastest and is the kind the docs name for
   code; `db` is in the database backup and is what a cluster needs. Proposed: `db`, so there is one thing to back up
   and no move later, after DG1 measures clone and push of mr-moqui's history against Git-Hosting's table.
   - Answered Nov 11, 2026: `db` repositories in Postgres for Moqui itself, partly to keep the data in one place for
     backup and for copying an instance (D16). DG1 still measures.
3. **D8 against Projects' Chatter.** An issue's outward conversation as forum posts, and its internal notes as
   Chatter: two kinds of message on one work item. Proposed: yes, as the Client portal separates the messages for the
   client from Chatter; DG0 checks that a forum topic can belong to a work item without a category of its own showing
   in the forum.
   - Answered Nov 11, 2026: yes, reuse and share what the forum has (D8, D18). What is left is the design's: where
     an issue's topic lives so that its scope is the repository's category and it does not show in the forum's lists.
4. **Posting by mail without an account.** Google Groups allows it; the forum requires a verified account. Proposed:
   every subscriber is an account, made by one click in the invitation; mail from an unknown address is answered once
   with that link, and held.
   - Answered Nov 11, 2026: every post needs an account with a confirmed email address, by mail as on the pages (D17,
     [§6.3](#63-the-mailing-lists)).
5. **A text edit in Smithy.** Fixing a typo in the docs from the page is common on GitHub. D14 says Smithy never edits
   code. Proposed: allow it as a commit to a new branch (or to `refs/for/`) that opens a merge request, through the
   hosting services' own write, for any file under a size: nothing runs, and review is where it is checked.
   - Answered Nov 11, 2026: yes, include it; an edit on the fly with a commit is very useful (D14,
     [§5.7](#57-editing-a-file-on-its-page)). It also commits to the branch itself where the account may push.
6. **The pipeline file.** `ci.xml` with an XSD is Moqui's way and gets the editor's help; YAML is what people know
   from elsewhere. Proposed: `ci.xml` only.
7. **Security reports.** Today a Google Group. Proposed: a confidential issue ([§5.5](#55-issues)), and
   `security@moqui.org` as an address that files one from any sender, the one address that takes mail from a
   non-member.
   - Answered Nov 11, 2026: confidential issues, filed with an account as any issue is, and read and edited only by
     the owners of the project or group; and a `security@moqui.org` address, once moqui.org has a mail server, which
     it does not today (D6, [§5.5](#55-issues)). With D17 the address does not post for a sender without an account.
     Confirmed the same day: the reporter keeps seeing their own report and the owners' replies to them, and mail
     from an unconfirmed sender waits as a held item the owners can read.
8. **Who may create groups on moqui.org.** Proposed: personal repositories and forks for any verified account, with
   small quotas; a group by request, approved by an operator, for open source Moqui components.
9. **Issue numbers.** A number per repository beside the work item's key means two names for one item. Proposed: keep
   both; the number is what the public sees and the key what Projects shows, and each finds the item.
10. **Open CI's cost and abuse.** Anyone's merge request can ask for minutes. Proposed: approval for a first
    contribution, a pool without secrets for outside runs, quotas, and no runs at all for a personal repository
    without a quota an operator gave.
11. **Mail reputation.** A new domain that starts sending list mail at volume is throttled. Proposed: move the lists
    one at a time, smallest first, after the forum's own notifications have been sent from the domain for some weeks.
12. **One person, two accounts.** A maintainer is an internal account in BOSS and a member in Smithy. Whether an
    internal account signs in to a PASS site as itself is DG0's check of design-forum §5.1; the plan assumes a
    maintainer's role in Smithy comes from the same party.
13. **Trust in our own host.** A project hosted on its own software loses its tools when that software fails.
    Proposed: the GitHub mirror stays for the code, backups are restored on a schedule, and the runners and the
    server are on different machines.
14. **Everything in the database** (D16). Repositories, LFS files, run logs, artifacts and caches in Postgres make
    backup and copying simple and make the database large. Proposed: retention times for logs, artifacts and caches
    from the start, the size watched in Monitor, and F11 when it is needed.
