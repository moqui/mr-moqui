# 04. Git, pull requests and issues

Part of [Agent Code](README.md). T3 sources: `t3:packages/source-control-*`, `t3:packages/issue-tracker-linear`,
`t3:packages/contracts/src/{git,vcs,review,sourceControl,pullRequest,pullRequestReviewGuide,threadPullRequest,issue}.ts`,
`t3:docs/user/source-control.md`, `t3:docs/user/triage.md`.

## 1. Code hosts

Code goes in `crates/moqui-code/src/scm/`, one module per host, behind the traits of §2.

| T3 package | Host | Auth order (as T3) | MR Moqui module |
|---|---|---|---|
| source-control-github | GitHub, GitHub Enterprise | 1. token in Settings (secret); 2. `GH_TOKEN` (`GH_ENTERPRISE_TOKEN` + `GH_HOST`); 3. `gh` CLI 2.81.0+. Per-host account pick; a host can be off. | `scm/github` (REST and GraphQL over the Rust HTTP client; `gh` only for its stored token) |
| source-control-gitlab | GitLab merge requests | `glab auth login` | `scm/gitlab` |
| source-control-forgejo | Forgejo, Gitea, Codeberg | `fj` first, `tea` 0.16+; subpath URLs use `tea` | `scm/forgejo` |
| source-control-bitbucket | Bitbucket | Access token, or Atlassian email + API token (`read:user:bitbucket`, `read:workspace:bitbucket`); env `MOQUI_BITBUCKET_ACCESS_TOKEN` or email + token | `scm/bitbucket` |
| source-control-azure-devops | Azure DevOps | `az login` with `azure-devops` | `scm/azure` |
| source-control-gitcafe | GitCafe | `cafe auth login`, token, or `CAFE_TOKEN` / `CAFE_HOST` | `scm/gitcafe` |
| source-control-tanstack-source | TanStack Source (private beta) | flag; `src auth login --web`, token, `SRC_TOKEN` | `scm/tanstack` (behind a Labs flag) |
| source-control-cursor-origin | Cursor Origin | `origin auth login` | `scm/cursor_origin` |
| issue-tracker-linear | Linear | API key; teams bound per project | `scm/linear` |
| (new) | **MR Moqui Git hosting** on this server or a linked group server | the user's session here, or a `DevAideCredential` / host account token there | `scm/moqui` on `GitRepository`, `GitMergeRequest`, `GitCommitStatus`, `GitPipelineRun` and `GitRestServices` |

- Accounts: `AgentCodeHostAccount` per user and host. Tokens are secret refs. CLI-held tokens are read through the
  CLI at use time and never stored.
- **Rescan** runs discovery again (`discover#SourceControl`).
- Contract enums kept as T3: auth status `authenticated`, `unauthenticated`, `unknown`; clone protocol `auto`, `ssh`,
  `https`; visibility `private`, `public`; publish status `pushed`, `remote_added`; discovery `available`,
  `missing`; app access `covered`, `not-covered`, `signed-out`; change request state `open`, `closed`, `merged`.

## 2. Provider traits

Ports of `PullRequestProvider.ts`, `SourceControlProvider.ts` and `IssueTracker.ts`.

| Trait | Members (optional ones with `?`) |
|---|---|
| `PullRequestProvider` identity | `kind`, `capabilities`, `unavailable_requirement?`, `merge_message_rewrite?`, `repository_key?`, `reads_public_host_without_checkout?`, `batches_reads?`, `serves_host?`, `with_verified_credential?`, `routing_identity?`, `viewer` |
| lists | `list_change_requests`, `list_change_requests_across?`, `list_change_request_stats?` |
| read one | `change_request`, `preview?`, `summary?`, `watch_fingerprint?`, `events?`, `stack?`, `activity`, `review_thread_comments?`, `viewer_permissions`, `forget_reads?` |
| checks | `checks?`, `check_logs?` (tail at most 32,000 chars, `tail_log_lines`), `rerun_checks?` |
| diff | `diff`, `diff_file_paths?`, `diff_file_contents?`, `file_stats?`, `file_revisions?` |
| viewed files | `files_viewed?`, `set_files_viewed?` |
| write | `run_action`, `update_change_request?`, `comment`, `update_comment?`, `submit_review`, `reply_to_thread`, `set_reaction`, `set_thread_resolution` |
| reviewers and labels | `reviewer_candidates`, `set_reviewer_request`, `label_candidates?`, `repository_labels?`, `search_authors?`, `search_references?`, `set_labels?` |
| `SourceControlProvider` | repository identity from remotes, `head_branch_probe`, `read_change_request_template`, `repository_name_from_remote_url`, `refine_repository_identity` |
| `IssueTracker` | `kind`, `capabilities`, `viewer`, `list_issues` (state, limit, query, labels, assignee, sort, cursor, refresh), `issue`, `issue_link?`, `comments`, `create?`, `comment?`, `update?`, `set_state?`, `set_reaction?`, `templates?`, `candidates?`; standalone trackers add `parse_issue_url`, `probe_credential` |

Detail fields: `merge_capabilities`, `viewer_permissions`, `base_comparison`, `behind_by`, `auto_merge_enabled`,
`auto_merge_method`, `workflow_approvals_required`, `merge_queue_removal`, `review_threads`, `overview_threads`,
`commits`, `reactions`. Shared code: merge message rewrite (remove agent credits), check helpers, rate-limit
tracking, diff file revisions.

## 3. Enums

Kept as T3 in `crates/moqui-code/src/scm/types.rs` and as `Enumeration` rows where a field stores them:

| Enum | Values |
|---|---|
| Involvement | all, reviewing, authored |
| List state | all, open, closed, merged |
| Filters | draft only/hide; review approved, changes-requested, review-required, none; checks passing/failing |
| Checks state | passing, failing, pending |
| Mergeability | mergeable, conflicting, unknown |
| Merge method | merge, squash, rebase, fast-forward |
| Action | merge, ready, draft, close, reopen, delete-branch, update-branch, enable-auto-merge, disable-auto-merge, revert, approve-workflows |
| Update method | merge, rebase |
| Base comparison | up-to-date, behind, unknown |
| Check status | pending, action-required, success, failure, skipped, neutral, cancelled |
| Review verdict | comment, approve, request-changes |
| Diff side | left, right |
| Reviewer kind | user, team |
| Viewed files store | host, environment |
| Reference sigil | `#`, `!` |
| Filter option kind | labels, authors, references |
| Change type | change, rename-pure, rename-changed, new, deleted |

MR Moqui hosting mapping: `GitMergeRequest.statusId` → state; `mergeableEnumId` → mergeability;
`mergeMethodEnumId` → merge method; `GitCommitStatus.state` and `GitPipelineRun.statusId` → check status.

## 4. VCS

| T3 | MR Moqui |
|---|---|
| `VcsDriverKind` git, jj, unknown | git through `moqui-git` (and the `git` CLI where the crate has a gap); jj through the `jj` CLI. |
| Ignore classifier native or git-compatible fallback | Same. |
| Stacked actions `commit`, `push`, `create_pr`, `commit_push`, `commit_push_pr`; phases branch, commit, push, pr; progress kinds action_started, phase_started, hook_started, hook_output, hook_finished, action_finished, action_failed; branch step created or skipped_not_requested; PR step created, opened_existing, skipped_not_requested | Service `run#StackedAction` as an effect; progress on the `/agentws` stream `vcs.action`. Git hooks stream stdout and stderr. |
| `git.preparePullRequestThread` (local or worktree) | `prepare#PrThread`: open a thread on a PR branch. |
| `git.resolvePullRequest` | `resolve#PullRequest` (branch or URL). |
| pull result `pulled` or `skipped_up_to_date`; ref listing `all`, `local`, `remote` | Same. |
| `vcs.pull`, `refreshStatus`, `listRefs`, `createWorktree`, `removeWorktree`, `createRef`, `switchRef`, `init`, `refreshBranch`; streams `subscribeVcsStatus`, `subscribeVcsBranch`, `subscribeWorktreeSetup` + `worktreeSetup.cancel` | Services in `VcsServices.xml` of the component and `/agentws` methods of the same names ([09](09-Wire-API.md)). Dev AIDE's `VcsServices` (dev only) stay; shared Rust code moves to `moqui-code` so both use it. |
| Review diff `getBranchCommits`, `getDiffPreview`, `getDiffFileContents`; sources working-tree, branch-range, commit | Services `get#BranchCommits`, `get#DiffPreview`, `get#DiffFileContents`. |

Turn and branch review rules (as T3):

- A turn's diff shows only the turn's own work. Files brought in by pull, merge or rebase are left out unless the
  turn edited, committed or fixed a conflict in them.
- Branch comparison shows everything against the base.
- Multi-repo projects group changed files by repository; a picker narrows the diff. Thread details list each changed
  repository with branch, uncommitted changes and commit, push, PR actions.
- The Commits source shows the newest 100 commits; merge commits diff against the first parent.
- Commit links from a code host open in the Diff pane when the commit is local.

## 5. Projects: new, clone, publish

See [02 §11](02-Orchestration.md#11-projects). RPCs: `projects.createNew`, `projectClone.start|cancel|retry`, stream
`projectClones`, `sourceControl.lookupRepository`, `cloneRepository`, `publishRepository`,
`signInStart|signInWait|signInCancel`, `appAccess`, `starred`, `star`. Branch or file page URLs of GitHub, GitLab,
GitCafe, Codeberg and Bitbucket clone the default branch.

## 6. Create, review and merge

| T3 | MR Moqui |
|---|---|
| Git actions commit, push and create PRs; text from changes and thread work | Stacked actions plus text generation ([03 §8](03-Providers.md#8-text-generation)). |
| **Commits & pull requests** setting: Repository conventions or Custom instructions | `sourceControlWritingStyle` with project override. |
| Pull requests page: review changes and comments, request reviewers, check out, merge, edit titles and bodies and own comments, tick checkboxes, saved Views (local), deep links to a comment, review, line range, checks or commit, hover previews, Shift quick actions, drag-to-close many rows | `/qapps/code/PullRequests` ([10 §6](10-Web-UI.md#6-diff-and-pull-request-pages)). Views in `UserPreference`. |
| **Remove agent credits when merging** (off; project override) | `removeAgentCreditsOnMerge`. Removes agent co-author lines from merge and squash messages. |
| Auto-merge on GitHub, GitLab, Azure; GitHub also approve fork workflows and revert | Same per host capability. MR Moqui hosting: auto-merge when checks pass (needs `GitMergeRequest` auto-merge fields; add them in the hosting component). |
| PR sharing per environment: Read PRs, or Read and act; another connected environment with the same account on the same host can answer; verified credentials usable for routing 10 min in an outage; uncertain actions never retried elsewhere | `AgentEnvironmentLink.prSharingEnumId`; service `route#PrRequest`; RPCs `pullRequests.routing`, `routingIdentity`. |
| Mobile: PR list across environments; line comments stay drafts until submit; **Work on PR** prepares a worktree on first send; offline cache of lists, details, guides and diff pages | [12-Mobile §3](12-Mobile.md#3-screens). |
| An agent merge is refused if the head moved since the agent last read it | The PR tools keep the read head SHA per run and compare at merge. |

## 7. Review guides

- Beta, setting **Review guides**. Adds a **Guide** tab.
- A guide is a tour of stops in reading order. Every changed file is somewhere. Mechanical changes go last. Resolved
  conversations are left out. In a stack, only that layer.
- **Write guide** runs the review guide model in a temporary checkout. It changes nothing. Stored per server for all
  users (`AgentReviewGuide`). **Rewrite guide** in the PR menu.
- Select lines in an excerpt to add a review comment or ask an agent. **Ask about this stop** opens a thread with the
  stop context. **Finish review** opens the review with the collected comments.
- **Review guide instructions** setting with project override.
- Data: `{headSha, generatedAt, modelSelection, stops[]}`; stop `title`, `summary`,
  `attention {kind: tradeoff|contract|product|risk|accessibility, note} | null`,
  `excerpts[] {path, side new|old, start, end, caption}`, `files[] {path, note?}`, `notes[] {path, line, title, body}`,
  `questions[]`. Run state Running, Failed and others in `statusId`.
- Services `get#ReviewGuide`, `generate#ReviewGuide`; tools `read_pull_request_guide`, `write_pull_request_guides`.

## 8. Files viewed

- Tick a file in the Code tab; it collapses; a count shows. A tick belongs to the PR. A file changed after the tick
  shows **Changed**.
- GitHub uses its own viewed marks (store `host`). Others are kept on the server (`AgentReviewFile`, store
  `environment`, label "viewed in Agent Code"). MR Moqui hosting stores them in `AgentReviewFile` too.
- Services `get#FilesViewed`, `set#FilesViewed`.

## 9. Linked pull requests and watch

| Rule (as T3) | MR Moqui |
|---|---|
| A thread holds several PRs, also from another repository on the same host | `AgentConversationLink` rows. |
| Link sources manual, created, agent, stack, stack-dismissed; by palette, Linked items panel, right-click a PR link, Git actions, or an agent running `gh pr create` / `glab mr create` | The command mirror (§2.1 of [03](03-Providers.md)) scans command tool calls for a created PR URL and links it with source `agent`. |
| Snapshot: state, title, headBranch, remarksThrough, remarkIds, conflicting, rebaseRequired, mergeQueueRemovalId | `snapshotJson`. |
| Refresh: open PRs on the server; closed ones periodically; merged on request; settling refreshes once | Job `sync#LinkedPullRequests` (open: every 2 min while the thread is active, else every 30 min; closed: daily). |
| Watch: poll every 2 min while active | `AgentPrWatch`; job `poll#PrWatches` each minute picks due rows. |
| Wakes when: a check fails or waits for maintainer approval; required checks pass; other checks finish after a failure; someone else comments or reviews; the branch starts to conflict; on github.com the merge queue drops the PR | Same rules in `crates/moqui-code/src/scm/watch.rs`. A wake is a message into the thread (`senderConversationId` null, kind pr-watch). |
| Threads in a project that watch the same PR share one check; GitHub asks first by fingerprint | Shared poll keyed by PR key. |
| Own-account comments do not wake | Same. |
| Ends on merge or close (one wake unless the agent did it), after 10 comment-only wakes in a row, after 8 failed reads in a row, on Stop, settle or archive; rate limits pause it | Same counters in `AgentPrWatch`. |
| `until` checks (default) or merged; snapshot `wakes`, `awaitingBaseline`, `checksAwaitedSince`, `checksPending` | Same fields. |
| A watched thread is Working while checks run and never auto-settles; subagents cannot watch | Same. |
| RPCs `pullRequests.linkedThreads`, `subscribeRefreshes`, `subscribeLinkedThreadRefreshes`, `invalidate`, `reportState` | `/agentws` methods with the same names. |

MR Moqui hosting watches need no polling: `GitCommitStatus`, `GitPipelineRun` and `GitMergeRequest` changes raise
the wake through an entity ECA in the component.

## 10. Stacks, CI logs and reruns

- PR page shows stack position. **Merge stack** submits the PR and every unmerged layer below it, with branch rules and
  merge queues. **Rebase stack** updates remote branches bottom-up without touching the local checkout. GitCafe also
  merges stacks. Service `get#PrStack`.
- `check_logs` returns `{name, url, log | null, truncated, detail}` per check, tail at most 32,000 chars.
  `rerun_checks` returns `{name, rerun, detail}`. Implemented for GitHub, GitCafe, Bitbucket, TanStack and MR Moqui
  pipelines (`GitPipelineRun` job logs, `retryOfRunId`).
- Agents read, open, update, reply on and merge PRs through the tools of [08](08-MCP-Tools.md).

## 11. Issues

| T3 | MR Moqui |
|---|---|
| Issues page: every project whose remote is GitHub, GitLab, Azure or Forgejo, grouped per tracker account; filters open/closed, involvement (all, assigned, authored, mentioned), tracker, projects; sort updated or created; server search; Load more per group; saved Views | `/qapps/code/Issues`. Also MR Moqui hosting issues and Mantle `WorkEffort` tasks through the `scm/moqui` tracker. |
| Detail: description, comments, state changes, linked PRs, labels, assignees, reactions; comment, edit, labels, assignees, react, close/reopen with reason; read-only through a project that does not own the repo (`not-found`, `read-only`, `rate-limited`) | Same. |
| Link issue to thread: manual or agent; tool `link_issue` | `AgentConversationLink` kind issue. |
| Linear: API key; one or more teams per project; agents file in the first bound team; `issueTrackers.list`, `connect`, `bindProject` | `AgentCodeHostAccount` hostKind linear; `AgentProjectTracker`. |
| Writes over a remote connection need scope `source-control:write` | Scope `code:scm-write` ([14](14-Security.md)). |
| RPCs `issues.list`, `detail`, `comments`, `templates`, `candidates`, `create`, `comment`, `update`, `setState`, `react`, `linkedThreads` | Services and `/agentws` methods of the same names. |

## 12. Triage

- Palette **Triage an issue** opens a project-less thread with a triage playbook. The agent reads logs and state, gets
  the MR Moqui source for the server's version, finds known issues and helps write an issue. It asks before it fixes
  or posts.
- Runs on the machine of the open thread, else this server.
- Terminal: `moqui code triage [--agent <driver>] [--model <id>]`. Service `prepare#Triage`.
- The playbook text is a skill (`AgentSkill` `code-triage`). The issue target is the MR Moqui repository on GitHub.
  The playbook tells the agent never to put private data or private repository names in a public issue.
