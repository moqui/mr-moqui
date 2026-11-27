# Dev AIDE

The Dev AIDE (assisted IDE) is the `/qapps` app for changing and creating Moqui components while the server runs them:
a workspace of the components' repositories with an editor, git and live reload; an artifact index with search and a
browser; an entity modeler; a service writer with a logic composer and ECA rule tools; a screen builder; a task panel;
and Dev Assist, the dev agent, with tools for all of it. It is at `/qapps/dev` (menu Dev AIDE) on a dev instance. The
design and its build record are in [plans/archive/design-dev-aide.md](../../plans/archive/design-dev-aide.md).

The code: the component `runtime/components/dev/dev-aide` (its crate in `src/`, services in
`service/org/moqui/devaide/`, screens in `screen/DevAide/`, the browser client in `screen/devstatic/`, the dev agent's
skills in `skill/`), and the framework's development kit in `crates/moqui-devkit/src/` (the artifact index, the
reference rules, reload, the watcher, the schema digest). The command-line tools are in `crates/moqui-host/src/`
(`devtools_cmd.rs`, `lsp.rs`). What it may reach, and how each write is checked, is in
[Attack-Surface-Dev](../security/Attack-Surface-Dev.md#dev-aide).

## Loading and access

The Dev AIDE gives its users code execution as the server's OS user, so it loads only on a dev instance
(`instance@purpose` dev). `runtime/components/components.xml` lists `runtime/components/dev` as a set with
`purpose="dev"`, and the component's own `component.xml` says `purpose="dev"`; on any other purpose each is left out
with a WARN naming the mark that left it out. Bind a dev server to 127.0.0.1 (open `https://localhost:8443`;
`moqui_conf__webapp_list__webapp__WEBROOT__http_bind=127.0.0.1`) unless it is a team dev server (below).

Its Rust crate is in the builds with the `devtools` feature: Maximal (the default `./moq build`) and Debug, and Lite
built with `./moq build --lite -- --features devtools` ([Build-Test, Packages](../Build-Test.md#packages)). In another
build the screens load and the services answer that they are not built in.

**Tiers.** Every Dev AIDE service and dev tool checks one of three permissions (`data/DevAideSecurityData.xml`):

| Permission | Who has it | What it allows |
| --- | --- | --- |
| `DEV_AIDE_VIEW` | | Browse, search and read files; Try in sim of saved services |
| `DEV_AIDE_EXTEND` | `DEV_AIDE_EXTENDERS` | Everything of view, and writes inside the site components only, additive DDL only, Publish to the site's remote, Try over unsaved text and its trace |
| `DEV_AIDE` | `ADMIN`, `DEV_AIDE_USERS` | Everything: upstream edits with the toggle on, destructive DDL, every git command, the conf, Rebuild, Convert |

An agent works with its user's tier. A request signed in by a header (an API key) reads, and never writes, connects
or comments.

**The site component.** Settings › Site components names the components the site owns, the first the default target of
an extension (the conf's `dev-aide@site-component`). Every other component is *upstream*: shown, searchable and read
only, with a lock chip, and every gesture that adds something to an upstream artifact writes an extension in the site
component: an `extend-entity` for a field or relationship, a `screen-extend` for a form field, a list column or a
widget at a named anchor, a new SECA or EECA file for a rule on an upstream service or entity. Home offers "No site
component yet" with Create site component when there is none. Settings › Upstream components › Allow editing upstream
components turns the lock off for the full tier, for working on the framework or an app itself; while it is on,
`moqui check` still reports an in-place edit of an upstream component as a problem.

## The guided tools and the Workspace

The same buffers, Save, checks and approvals serve two arrangements.

**Guided**, for extending an application:

- **Home** has the site's customizations ("Site extensions: n, m broken"), its unpublished changes with **Publish**
  (stage the site component's changes, commit with a message Assist drafts and the current task's trailer, push), each
  root with its branch and changes, problems, the reload log, the restart a change waits for, and New component.
- On any app page of a dev instance, the Assist drawer's header has **Customize this screen** (Dev Assist with the
  recipes for the page) and **Open in screen builder** (the builder on that page's screen, with its parameters).
- **Entities**, **Services** and **Screens** list their artifacts; each opens in its visual view alone
  (`dev/Entities/Edit?artifact=moqui.example.Example`, `dev/Services/Edit`, `dev/Screens/Design?page=`), with the
  file status strip, Save, Text a click away, Problems inline and the Assist drawer. Open in Workspace moves the tab
  there.

**The Workspace** (`dev/Workspace`) is the full IDE. It takes `rootId` (a root's id), `path` (relative to the root),
`line`, `view` and `artifact`, so a link opens a file at its line in a view. The shell's app drawer is closed while it
is the page.

- The **roots** are the repositories and directories of the loaded components, and any directory added in Settings ›
  Add a directory. A root that is not a git repository has the same layout with version control off and a "Not a git
  repository" chip. A root may be read only (Settings › Roots).
- The rail switches the side panel: **Files** (the tree with each file's git status), **Search** and **Tasks**.
- **Tabs**: a file opens in Text (CodeMirror, with schema completion, artifact completion and checks) or in its file
  kind's visual view (Entity, Service, Screen, Rules, Design for a `.qvue`). Text and the view are one buffer with one
  undo history (Mod+Z in a view undoes its last operation) and one Save. A view may hold companion buffers (an entity's
  site extension and seed data file, a `.qvue` screen's XML) that its tab saves with it.
- **Save** writes the file, reloads what it defines, checks it, and reports what it reloaded and its problems. A file
  changed on disk since it was opened opens Compare: keep mine, or take the disk's. Unsaved text is kept in the browser
  (IndexedDB) by root and path and offered back when the file is opened again.
- **Go to file** (Mod+P), **Go to artifact** (Mod+Shift+O) and **Search in files** (Mod+Shift+F). In the editor F12 or
  Mod+click goes to a definition and Shift+F12 lists the references.
- **Search in files** takes text or a regular expression (the Rust `regex` dialect, over each file's whole text, so
  `\n` matches across lines), case and a path filter; it is `git grep` for a text in a repository and a bounded scan
  otherwise. **Replace** takes a replacement (`$1`, `${name}`) and shows a preview of each file's matches with its line
  before and after; choose the files and the matches, then Replace writes each file as Save writes it, keeping its line
  endings. A file changed since its preview, one you may not write, or one with unsaved text in an open buffer is
  refused and named, and the others are written.
- **Problems** lists the checks of the open files: schema problems are warnings, references, expressions and parse
  failures errors, each naming its rule and the skill section that has the rule.

### The Workspace with git

The git commands are the developer's own `git` (`dev-aide@git-command`), with its config, hooks, signing and
credential helpers. A read (status, a diff, a file at a commit, the log, blame) runs nothing a repository names: no
lazy fetch from a partial clone's remote, no `core.fsmonitor` and no hooks; writes and network commands keep the
repository's config. The tabs of a git root:

- **Changes**: the staged and unstaged files with their diffs; Stage, Unstage and Discard by file or by hunk (the hunks
  as chips over the merge view); the commit form with the current task's trailer, Amend, Skip hooks, and Commit and
  Push.
- **Commits**: a page of 100 at a time, or a file's History; a commit has Copy id, Browse files, Branch here, Tag here,
  Cherry-pick and Revert, and check chips when the repository is on a group server.
- The bar has the branch (Switch, New branch), ahead and behind, **Fetch**, **Pull** (offering a merge commit when it
  cannot fast-forward) and **Push**, each network command with its progress line and Cancel.
- **Advanced** adds **Branches** (Switch, Merge into the current, Compare, Rename, Delete; Force push with lease, the
  remote branch typed back, for the full tier), **Tags** and **Stashes**.
- **Conflicts**: a merge, cherry-pick, revert or stash that stops at conflicts shows a banner; a conflicted file opens
  ours, the result and theirs side by side, with Ours, Theirs and Both on each conflict, Mark resolved once no markers
  remain, and Abort.
- **Settings** (a tab of the root): remotes (Open on the server, Prune, Set URL, Remove, Add remote), the root's components,
  where Publish pushes, and `.gitignore`. A root's **publish branch** (Settings › Roots, or per user the
  `DEV_AIDE_PUBLISH_BRANCH` preference an admin sets on System › User) makes Publish push to a branch of its own and
  open a merge request, optionally required.
- **Merge requests** and **Sessions**: below, with group servers and Dev Assist.

**Repositories** (`dev/Repositories`) lists every root with its kind, components, branch and upstream, ahead and
behind, changes and last fetch; **Fetch all** and **Pull all** go root by root, a failure a message for its root while
the others go on. **Get a component** runs `./moq component-get` (where the runtime has `moq`), **Clone a URL** clones
into `runtime/components/addon`, `dev` or `test`, and a component that needs no restart is offered **Load now**.

## Live reload, restart and rebuild

On a dev instance a Save reloads exactly what the file defines: its services (the service registry rebuilt), its
screen, the ECA rule set of its kind, its REST API or skill, and its entities (the model relinked, the missing columns
added: no restart). A file that does not parse keeps its previous definitions loaded and shows as a problem. A change
from outside the app (another editor, an agent in a terminal) reloads the same way through the framework's file watcher
(`dev-reload`, `enabled` and `debounce-ms` in the conf), so the open tabs take it.

Some changes wait for a restart: a part of the conf not applied live, a `component.xml`, a component not loaded live, a
Rust change. Home's and the Workspace's banner then has **Restart** (`restart#Server`, for `DEV_AIDE` or `ADMIN`): the
server runs itself again with its arguments, the open pages reconnect and the Workspace comes back to its tabs.
Sessions and the sim overlay are lost. A Rust change (a native component, a new one included) needs a build first:
**Rebuild** (`rebuild#Server`, the full tier, on Unix where the runtime has `moq` beside it) opens a host session of
the [Terminal](../ui/Terminal.md) typed with `exec ./moq build` for the package this server runs; the banner follows
it, says how it ended, and offers Restart when it succeeded. The Terminal decides who may open a host session
(`TERMINAL_HOST`).

## The artifact index

The framework keeps an index of every artifact the loaded components define, with its file and line, and the
references between them read from the XML and from the string literals of scripts, and on a dev instance the calls
observed on the artifact stack (`moqui.devtools` tables). It holds entities and their fields, relationships, indexes,
view members and aliases (`alias-all` included); services and their parameters (`auto-parameters` and `implements`
merged as the runtime merges them); screens, subscreens, transitions, forms and form fields; REST methods with their
service's parameters; ECA rules; data records and seed data; artifact groups with their members (a pattern resolved to
the artifacts it matches, as authorization matches it); and what each extension extends. A `moqui run` server builds
it in the background at start, and each reload or pull updates the files it changed.

- **Search** (`dev/Artifacts/Search`, the shell palette's Artifacts group, Go to artifact) finds by name, short name or
  segment; a result opens in its view, in Text at its line, or in the Browser.
- **References** (`dev/Artifacts/Refs`) lists what uses an artifact and what it uses, as a list.
- **Browser** (`dev/Artifacts/Browse`) draws an artifact in the center, what uses it in a column to its left and what
  it uses to its right, grouped by kind in cards; a row expands into the next column. Filters: kinds, framework
  artifacts, static and observed links; the center has Open in Text, Try, Data, Preview, the List view and Copy name.
  It pans and zooms (drag, wheel, pinch, the arrow keys, + and - and 0 to fit).
- **Docs** (`dev/Artifacts/Doc`) is the Markdown doc of an artifact, package or tree as the server has it now, and
  **Export docs** writes a tree of them into a repository, as `moqui autodoc` does ([Autodoc](../dev/Autodoc.md)).
- **Customizations** (`dev/Artifacts/Customizations`) is every extension the site components make, by kind and file,
  with its anchors' state (OK or Broken); a pull that breaks one says so. **DbForm changes** lists the DbForms that
  change a screen form. **Convert** (the full tier) shows the form merge it writes into the site's `screen-extend` of
  that screen, the DbForm's rows that go and what is not carried (user groups, the print layout); Accept writes the file
  and deletes the rows in one transaction, one way only. A dated DbForm, one with responses, or one that changes no
  screen form stays a DbForm.

## Entities and data files

The **entity view** shows an entity as the server merged it, each field with its origin (the defining component, each
extension, a DbForm). Edits of an upstream entity go into the site's `extend-entity`. **Add field** by kind; Picklist
and Status write the field, its relationship and their seed records (into the site's seed data file, loaded on Save).
The view has the relationships, the incoming ones, indexes, a view entity's members (a diagram, or a table) and
aliases with its condition, order and Run (the SQL a find makes and its first rows), the table against the fields, a
label in another language, and a Doc tab. Saving relinks the model and adds the columns. The full tier also has the
changes the table check never makes (drop a column, change a type, copy a renamed field, recreate the table), each
after the table's name is typed back and audited.

**Data** (`dev/Data`) lists the data files of the loaded components by component and type. A data file opens with
its records as tables per entity (Edit, Remove, Add row) and the actions:

- **Load this file**: the data loader, with the file's own type.
- **Check against the database**: which records differ. It is a restricted check: a password or encrypted field is
  not compared, and an entity you may not read through the Dev AIDE is not checked.
- **Export records**: a find over an entity written into the file, at most a thousand records.
- **Load the changed records** compares each record by entity and key three ways: the base (the text the last load of
  this file committed, else the last commit's), the file and the database. Unchanged in the file: nothing. Changed or
  added, with the database still the base's: written. The database already the file's: applied. Changed in both, or
  added where the database has another: a **conflict**, settled per record by Keep database or Take file. Removed from
  the file: deleted only when you choose. The plan shows before anything is written; Load writes it in one transaction
  through `store#<entity>` (its SECA rules run), and only then moves the base. A record passed over, a removal or a
  difference nothing was chosen for keeps the old base, so the next load offers it again. A change to a password or
  encrypted field alone is not picked up. The base keeps the time zone of its load, so a user in another zone does
  not see a time stamp as changed.

## Services and ECA rules

The **service view** has the header's settings, the **Parameters** (nested: Edit and Remove on each, Add child in a
Map, List, Set, Collection or Node; the parameters `auto-parameters` or an interface add shown after them, not
editable), the implementation (inline actions in the composer; a script service's file opens in the Workspace), its
SECA rules with Add a SECA rule in the site, what uses it, the last spec run's calls, and a Doc tab.

The **logic composer** edits XML Actions as blocks: each element a sentence whose chips edit one attribute (entity and
service names completed from the index), containers nesting their blocks, a palette on `/` or the + between blocks,
Alt+arrows to move a block, and Generated, the JavaScript the framework runs for them. It edits services' and ECA
rules' actions, and in the screen view's Actions panel a screen's and its transitions' actions.

**Try** runs the service with a form of its in-parameters:

- **Try in sim** (the default) runs it as you against an overlay of the database (embedded Postgres) under a fence
  against outside effects, and answers its out-parameters, messages, the rows it would write (before and after), its
  log lines and its time, keeping nothing. At the extend tier it runs the service as the Workspace's unsaved service and
  script buffers define it, a service they define anew included, and asks for a **trace**: the composer then marks each
  block run (its count as a badge, the values it set on hover), not run, or failed, and the Try panel names the failing
  block. A buffer may not define a Rust or native service anew or change one's type.
- **Run for real** runs the saved service behind a confirm, audited.
- **Snippet** runs XML Actions or JavaScript in the sim as a scratch service.
- **Save as spec** appends a feature to `test/<Services file>.spec.js` of the service's component (or the site's for an
  upstream service): the call, its out-parameters, and a `checkData` of the rows the sim wrote, for you to trim. **Run
  spec** runs the file inside the sim.

**ECA Rules** (`dev/Eca`) lists the active SECA, EECA, EmECA and gitECA rules with their files, in load order, a
replaced rule under the one that replaced it. A rules file opens in the **ECA view**: its rules with their attributes,
condition and actions (in the composer); Add a rule, Remove, Move. **Try the condition** runs a rule's condition in the
sim against a context: its service's last Try, any JSON object (a record of an EECA's entity), or for a gitECA rule
**Against a push**: the hosted repositories its `repository-pattern` takes, their latest changes from the push log, and
a change's contexts as its events had them, each marked when the rule would not run on it. A context is kept with
**Save as fixture** (`test/fixtures/<name>.json` of the rules file's component, or the site's). **Run the trigger in
sim** runs the rule's service call or entity operation with it and marks the rule's actions; **Save as spec** writes a
feature into `test/<RulesFile>.spec.js` that runs the trigger with the fixture and checks the messages and rows it made
(a gitECA rule's condition only, as a spec replays no push; an EmECA rule has none).

## The screen builder

**Screens** (`dev/Screens`) is the screen tree as the screen facade mounts it, a level at a time, each screen with how
it is mounted, its menu index, default and modes; Set default and menu index, Move (each file naming the old location
saved naming the new one), Remove (refused naming what references it), and New subscreen from a gallery archetype, with
a live preview of each.

A screen opens on the **design canvas**: its real render in a frame, from the tab's unsaved buffers, each widget stamped
with its source. A click selects the element; the Outline (the block tree) and Properties (from the schema digest) edit
it. An upstream screen opens in **extension mode**: every edit goes into the site's `screen-extend` of it (form merges
with `after=`, widgets after, before or inside a named container, Replace section, Override transition, Actions before
and after), and the Extension points list names its anchors; an unnamed anchor is reported with the nearest named one,
and a full-tier user with upstream editing on gets Add an anchor here. The intents: Add column, Add field (placed in the
form's `field-layout` or columns), Add filter, Add section, Add button (a transition calling a service and its link) and
Add row action. A double click edits a label's or link's text in place.

A **`.qvue` screen** is one tab of its two files: Design (the page framed, rendered from both unsaved buffers, an
element selected by its stamp and edited, moved, removed or inserted beside), Text .qvue and Text .xml, saved together.

## New components

Home's **New component**, and `./moq component-create`, make a component from the template
`runtime/components/dev/template` the same way ([the component template](../../runtime/components/dev/README.md)):

| Kind | `--kind` | What it holds |
| --- | --- | --- |
| App | `app` (the default) | Screens and a REST API (`--screens`, `--rest` for one alone), entities, services, security, demo data and specs |
| Data only | `data` | Seed and demo data |
| Native | `native` | `Cargo.toml` and `src/` with `submit_component!` and a Rust service, built by `./moq build` |
| Theme | `theme` | A theme of the `/qapps` shell and its style sheet |
| Skill pack | `skills` | A skill, a suggested prompt and retrieval cases for agents |
| Dev tool | `dev-tool` | Mock services and a test data screen, `purpose="dev"`, in `runtime/components/dev` |

A **Git remote** (`--git-remote`) starts the repository with that remote as origin: https with no user part, ssh,
`git://` or `user@host:path`, never a credential or a local path. A name whose package is another component's (`org`,
`moqui`, `mantle`), or a theme named as a webroot path, is refused. A new component loads after a restart; a native one
after Rebuild. Home's Create site component makes the site component (`extend-entity`, `screen-extend`, seed data and
SECA files ready) and names it in Settings.

## Dev Assist

Dev Assist is the `dev` agent of the Assist drawer, picked by default on Dev AIDE pages, with its skills and recipes
(`skill/`, `skill/recipes/`: add a field and show it, a list column, a filter, a picklist, a status flow, a related
section, a button that calls a service, a list and detail, a spec from a Try). Its tools
([Assist, Development tools](../ai/Assist.md#development-tools)):

- `artifact`, `source` and `check` read the index, a file or element, and the checks of a file or of proposed text.
- **`propose_edit`** is the one write: a **change set** of files (XML operations, exact replacements or a whole text),
  each checked before it is shown. Its card in the drawer reads as artifacts ("Entity `Example`: + field `dueDate`"),
  with titles you may reword, and **Preview** (the app page rendered with the proposal in place, under a read fence),
  **Accept all**, **Review** and **Reject**. Review opens the change set in the Workspace, each file a merge of the
  disk's text and the proposal with Accept and Reject on each chunk; Save accepted saves them. An upstream edit while
  the lock is on comes back as a problem with nothing proposed.
- **`try_service`** runs a service in the sim, as it is saved, as a change set of yours defines it, or as files it
  names.
- `vcs` (git reads and writes, each write an approval) and **`hosting`**: a connected group server's merge requests,
  review threads, checks, a job's log and failed tests, and an issue, as untrusted data; it opens a merge request,
  comments and replies only by your approval, never approves, merges, re-runs, resolves or signs, and is refused the
  diff of a merge request whose authors have not signed.

**Sessions** (a Workspace tab) makes a git **worktree** of a root for an Assist conversation, on a branch of its own
under `runtime/worktrees`, so two sessions do not share uncommitted changes; that conversation's file and git tools work
in it. A worktree is not loaded: its files are checked and committed there, not run live, which its banner says. When
the conversation is archived (Assist's History › Archive) or deleted, a clean worktree is removed (its branch stays) and
one with changes is kept, with a notice; a conversation whose run is still at work keeps its worktree until a sweep job
ends it.

Dev AIDE's own writes (a file, a git command that changes a repository, a call of a group server) never run under an
agent's fence or in a sim: `try_service` refuses them, and the writes refuse themselves.

Agents outside the app edit files on disk, the watcher makes that live, and they read the index and the checks through
`moqui check`, `moqui refs`, `moqui lsp` and the MCP tools `artifact`, `source` and `check` ([MCP](../ai/MCP.md)); the
coding-agent skill (`runtime/components/base/framework/skill/moqui-coding-agent.md`) says how.

## Tasks

**Tasks** (`dev/Tasks`, and the Workspace rail's Tasks) lists your open tasks from a task source: `mantle` (this
instance's Mantle work efforts assigned to you, when Mantle is loaded) or another Moqui instance over REST (Another
instance's tasks: a base URL, your user name and an access token there, kept encrypted); Tasks from and Open in pick
the source and the task app (IronStone Projects and MarbleERP by the conf's `task-app` elements, those a group server
offers, and those added in Settings › **Task apps**: a title, and a project's and a task's path on this server with
`{projectId}` and `{taskId}`; the extend tier adds one, and who added it or the full tier removes it). A task has Open (in the task app), Copy key, Use in commit, **Set current** (the root's commit
messages get its trailer) and **Branch** (`KEY-first-words`). Quick create makes a task on this instance through
Mantle. A task key is one short line; one that is not is refused.

## Group servers

A **group server** is a production instance a team's dev instances share: it hosts the repositories, the work items and
the LLM gateway (a team's BOSS Projects server, or moqui.org's Smithy). Its setup is in
[Run-Deploy](../Run-Deploy.md#dev-instances-with-a-dev-group-server).

**Connect** (Settings › Group servers): give its address, choose how to sign in (the server's identity providers, or its
own login), sign in and confirm there, and the dev instance gets keys back through the installed-app handoff with a
loopback redirect. It keeps a key for git alone, one for tasks alone and, where the server runs a gateway, one for the
gateway, never the account's key (for a Smithy server, whose REST takes an installed app's key, that key is kept and
sent only to Smithy's REST). **Paste an access token** made on the server works too. Connect then shows what it sets,
each confirmed: the `myaddons.xml` repository for the server's git URL, the task source and the task app, the gateway's
`.env` lines, and git's credential for that host. **Remove** takes back only what it set and ends its keys.

Each working tree whose remote is on a connected server then knows its hosted repository (its rights, default branch,
fork, merge request settings, agreement), and the Workspace has:

- **Merge requests** (a tab): mine, asking for my review, and all open, each with its state and checks; **Check out**
  (into a local branch; it warns that the code runs here); **New merge request** (from a fork's remote when the branch
  is there); **Review**: the changes in the merge view with the server's threads on their lines, Reply, Resolve, a
  comment on a line, and a review (approve, comment, request changes). Open threads of the checked-out merge request
  mark the editor's gutter. Merging stays on the server's page.
- **Push** and **Publish** open a merge request when the branch rule or the publish branch asks; **Propose** pushes to
  review (`refs/for/<branch>`) when you may view and not push; **Fork** makes your fork where the server makes forks.
- **Checks**: chips on Commits and Branches; a commit's checks (statuses, runs, jobs), a job's log followed while it
  runs, its failed tests as links to the file and line, Re-run and Retry.
- **Run locally** runs one job of the working tree's `ci.xml` here: in a host session of the Terminal (its steps and
  variables, not its image or secrets; the dialog shows them first), or, where Docker or Podman answers on this
  machine, **in the job's image** on a copy of the working tree's files (no `.git`, links not followed out of the tree),
  deleted when the run ends; nothing comes back to the tree, and the run's output is its log in the dialog. A run in
  the image needs `TERMINAL_HOST` and a root you may write that does not hold the runtime, has the conf's memory,
  process and time limits, and has no network on a shared dev server.
- **Open on the server** for the current file at its line, a commit and a branch, where the server offers those pages:
  the pages of the application that keeps the repository (Smithy's), or for a repository the framework's hosting keeps
  those an application has for it (IronStone Projects' Code, where mr-boss is loaded). The framework has no pages of
  its own, so on a server with no such application the button is not shown. The same label opens a merge request, a
  status and a remote on their server.
- **The contributor agreement**: where it is on, Changes warns when the commit's author has not signed, and **Sign**
  commits the line in `AUTHORS.md` alone, as you.
- **Issues** in the task panel: the hosted repository's open issues, the tasks of its linked projects first; Use in
  commit writes `Fixes #12` where issues have numbers; **New issue** files one; Branch names a branch for one.

The server decides everything as you: a right you lack there is not gained here. The REST it answers is the
framework's ([Git-Hosting, REST](../interfaces/Git-Hosting.md#rest)) or that of the application that keeps the
repository (Smithy).

## Hosted roots

Repositories › **Add hosted root** adds a repository of this server's git hosting at a branch as a root with no working
tree. Its tree and files are read as you, so the repository's view right decides what you see, and a user who may not
view it does not see the root at all. Each **Save** (its commit message asked), New file, New folder, move and delete is
one commit to the branch as you, which the repository's update right and branch rules allow or refuse as they would a
push; a branch moved past the file answers with Compare. Nothing of it is loaded or run, and status, staging, stashes
and Replace across files are off. Remove takes the root away (who added it, or the full tier).

## A shared dev server

With `dev-aide shared="true"`, one dev instance serves a team:

- commits carry the signed-in user as their author;
- fetch, pull and push are off until an admin turns on Settings › Shared dev server › Network git commands; Settings ›
  Git tokens then stores a root's deploy token or a user's own token, encrypted, given to git only for the host it was
  stored for;
- **presence**: a tab shows who else has the file open ("being edited by"), and a save by another user tells the open
  tabs, whose clean ones take the new text;
- Save is single-writer by hash: a file saved by another since you opened it opens Compare;
- **co-editing**: the users with one file open that they may write edit it together, each seeing the others' carets
  and selections by name and the "co-editing with" chip. The file's room is seeded from the file when its first editor
  opens it, kept in memory only and dropped when the last tab closes; undo is your own edits only. Save is still each
  user's, by hash: a save by one moves the others' base, so the next save of the shared text is no conflict. A room is
  one server's: on a server with cluster peers nothing is co-edited. The bounds are `dev-aide@coedit-max-rooms` (files
  a user co-edits at once, 20) and `dev-aide@coedit-max-total-bytes` (every room's state together, 64 MiB).

## Editor settings and keymaps

Settings › **Editor** sets, for you only (the `DEV_AIDE_EDITOR` preference): the font size, the tab size (and indent;
empty is CodeMirror's), wrap, white space shown, the keymap (CodeMirror's, VS Code's or Vim, whose library loads when
you pick it) and **Check script types**, off by default: a script file under a component's `script/` is checked by
TypeScript in a worker of the browser against the project's `moqui types` declarations, as the function body it runs
as, its problems in the editor and Problems (the compiler, 3.6 MB, loads on the first script tab). The shell, Groovy,
properties, TOML, Dockerfile and nginx files are highlighted by a second bundle of languages, loaded when one opens.
Settings › **Editor keys** has a row for each of the
code editor's commands (find, toggle comment, completion, undo, redo, fold all, unfold all) with its default and your
keys, in CodeMirror's form (`Mod-Shift-f`; Mod is Ctrl, or Cmd on a Mac); empty sets the default back, and a key another
command has is refused. The Workspace's own keys (Go to file, Go to artifact, Search in files, Save) are the shell's key
map actions `devGotoFile`, `devGotoArtifact`, `devSearchFiles` and `devSave`: the shell's `?` lists them, and the
`KEYMAP` preference rebinds them (`{"dev-aide": {"devGotoFile": "Mod+E"}}`), in Text from the next page load.

## In the terminal

The lists (Home, Repositories, Search, References, Customizations, Entities, Services, Screens, ECA Rules, Data, Tasks)
are XML screens with terminal forms, so `moqui tui` and the browser Terminal reach them, and `moqui tui --agent` lets a
coding agent drive them. The canvases have terminal twins of their own, keyboard first:

- **WorkspaceTui** (`dev/Workspace` in the terminal): the tree, Go to file, Search, one file in the `code` editor with
  Save, its problems and References and Browse for the artifacts it defines, Changes with the commit form, Commits with
  Cherry-pick and Revert, and a Git menu (Fetch, Pull, Push, branches); merge requests, review and blame open in the
  browser.
- **BrowseTui** (Artifacts › Browser): Miller columns, where Enter re-centers on a row, Backspace and `]` walk the
  trail, `k`, `h` and `x` filter ([TUI, components](../ui/TUI.md#components)).
- **EditTui** of Entities and Services, and **DesignTui** of Screens: forms, lists and dialogs in place of the
  canvases. Their edits are staged in your edit buffer, one per artifact, kept two hours since the last edit, and the
  header says how many wait; **Save** (Ctrl+S) writes them in kind order (entities, services, ECA rules, screens, data)
  with the diff, and "Save mine over it" or "Take the disk's" when a file changed on disk; Discard drops them. The
  entity twin edits the site seed file's records as rows; the service twin has Try, the settings, nested parameters,
  the actions read only and the SECA rules; the screen twin has the outline, the selected element's properties and
  intents, an extension mode of its own, and a **Preview**, the designed screen's own terminal form rendered from the
  buffer. A data file's menu has Load the changed records when nothing needs a choice; conflicts go to the browser.

## The command line

These need a build with `devtools`. Each reads the files into a memory index on a model-only boot (no database), and
takes `--server URL` to ask a running dev instance instead (its MCP tools, with the login key `moqui tui --server`
keeps, or `MOQUI_LOGIN_KEY`) and `--project DIR` or `--java DIR` for another Moqui project.

- **`moqui check [paths]`** checks Moqui XML files (with no paths, those git shows as changed): schema, references and
  expressions, each problem as `path:line:col: severity: message [rule, skill#section]`; `--json` prints one object a
  line; `--extensions` checks every site extension against what it extends. It exits 1 on an error, so it runs as a
  git or agent hook. `moqui check --agreement` asks the group server (`MOQUI_GROUP_SERVER`, `MOQUI_GROUP_TOKEN`)
  whether the commit's author has signed.
- **`moqui refs <name>`** lists what uses an artifact (`--in`, the default) or what it uses (`--out`), `--kind` to keep
  one kind of the other side, `--json`.
- **`moqui lsp`** is a language server over stdio: diagnostics, go to definition and references, hover, completion,
  document and workspace symbols. **Rename** of an entity field, a service, a form field or a parameter edits every
  definition and resolved reference and what takes the name from it (aliases, `auto-parameters`, form fields, key
  maps, data records); it is refused, naming the place, where a reference does not resolve, something reads the name
  under another, an edit would land in an upstream component under the lock, or under `--server`. Its last gate is a
  strict plain-text scan of every component for the old name, so renaming a common name practically always refuses.
  **Code actions** offer the nearest known names for an unknown entity, service, field or parameter, and **Extend in
  my-site** for an in-place edit of an upstream component while the lock is on: the open text against the file at git's
  `HEAD` is reported as `upstream.edit`, and where the edit is mechanical (fields added to an upstream entity, a field
  added to an upstream form with its layout ref, a column added to an upstream list with a column of its own, each
  after another field) the quick fix is one edit that puts the upstream file back as committed and writes the fields
  as the entity modeler and the screen builder do: an `extend-entity` in the site component's entity file, or a form
  merge with `after=` in its `screen-extend` of the screen, the file made when there is none (offered to a client whose
  edits take `documentChanges` with a `create`, the open file's edit naming its version). The site component is the
  first of the conf's `dev-aide@site-component`; with none there nothing is upstream and nothing is offered (Settings ›
  Site components is the database's, which the command line does not read).

## Settings

The `dev-aide` conf element (the component's `MoquiConf.xml`; Settings stores some of them over these):

| Attribute | Default | What |
| --- | --- | --- |
| `site-component` | | The site components, comma separated |
| `allow-upstream-edits` | false | The default of Allow editing upstream components (`DEV_AIDE_UPSTREAM_EDITS`) |
| `shared` | false | A shared dev server |
| `git-command` | git | The git command |
| `git-read-timeout`, `git-network-timeout`, `git-write-timeout` | 15, 300, 120 | Seconds |
| `max-edit-bytes` | 2097152 | The largest file the editor and Replace write |
| `local-run-memory-mb`, `local-run-pids` | 4096, 1024 | Each run in a job's image |
| `local-run-timeout-minutes` | 60 | After which a run in the image is stopped |
| `local-run-limit` | 2 | Runs in the image a user may have going |
| `local-run-copy-mb` | 1024 | The most the copy of a working tree may hold |
| `hosted-max-deleted` | 2000 | The most files a delete of a hosted root's folder removes in its one commit; a folder with more is refused |

The framework's `dev-reload` element (`MoquiDefaultConf.xml`) has the dev tools' own: `index-writers` (3), the
connections the artifact index writes a build's rows on at once, a hundred files to a transaction, at most one more
than the pool's connections the request and service workers leave.

Its `task-app` elements name the task apps' project and task paths; Settings › Task apps adds more (`DevAideTaskApp`
rows). The preferences: `DEV_AIDE_EDITOR`,
`DEV_AIDE_UPSTREAM_EDITS`, `DEV_AIDE_PUBLISH_BRANCH` (an admin's, on System › User), `DEV_AIDE_TASK_SOURCE`,
`DEV_AIDE_TASK_APP` and `DEV_AIDE_CURRENT_TASK.<root>`.

## Tests

The regression suite is `./moq test dev-aide`, `cargo test -p dev-aide`,
`moqui test-client runtime/components/dev/dev-aide --offline` and `cargo test -p moqui-host --test it devaide`
(`devaide_group` runs the instance as its own group server); the component's
[README](../../runtime/components/dev/dev-aide/README.md#tests) says what each covers. The Chromium smokes in `gate3_browser.rs` are acceptance tests
([Build-Test, the two suites](../Build-Test.md#the-two-suites)). The dev agent's evaluation tasks run with
`moqui test dev-aide --agents --replay runtime/components/dev/dev-aide/test/agent/replay`, or against a model with
`--llm-profile`.
