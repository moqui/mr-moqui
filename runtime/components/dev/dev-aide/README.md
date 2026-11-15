# Dev AIDE

The assisted IDE of mr-moqui (`plans/design-dev-aide.md`), at `/qapps/dev` on a dev instance:

- **Home** has the site's unpublished changes with Publish, each repository and directory with its branch and changes,
  the reload log, the restart a change needs, "No site component yet. Create one", and New component, which makes an
  app from the component template (`runtime/components/dev/template`) as `./moq component-create` does.
- **Workspace** is the IDE: the roots (the repositories and directories of the loaded components), the file tree with
  git status, files in the CodeMirror editor with schema completion and checks, Save that reloads what the file defines,
  Changes (stage, unstage, discard, commit with the task trailer), Commits, Fetch, Pull, Push and Publish.
- **Entities** is the entity modeler: an entity as the server merged it, with each field's origin, its edits going to the
  site's `extend-entity` when it is upstream; Add field by kind (Picklist and Status write their seed records too), the
  relationships, the incoming ones, indexes, a view entity's members and aliases, the table against the fields, and for
  the full tier the changes the table check never makes, behind a typed confirm. Saving relinks the model and adds the
  columns, no restart. A data file's records show as tables, with Load this file.
- **Screens** is the screen builder: the screen tree as the screen facade mounts it, and a screen on the design canvas,
  its real render in a frame from the unsaved buffers with each widget stamped with its source, an upstream screen in
  extension mode (every edit into the site's `screen-extend`), Add column and Add field (placed by `after=` in the layout
  or the columns), inserts at a named anchor or the nearest one, the Extension points, the Outline on the block tree and
  Properties from the schema digest. "Open in screen builder" on any app page opens it on that page.
- **Services** is the service writer: a service's settings and parameters, its actions in the logic composer (blocks as
  sentences, chips that edit an attribute, the palette, the generated JavaScript), its SECA rules, and Try in sim with the
  rows it would write, Save as spec and Run spec (in the sim). **ECA Rules** lists the active rules of each kind with their
  files; a rules file opens in the ECA view.
- **Browser** (Artifacts › Browser) is an artifact in the center with what uses it and what it uses, by kind: a
  pan-and-zoom canvas in the browser, and in the terminal Miller columns (`BrowseTui.xml`, the `artifact-browser`
  component) where Enter re-centers on a row, Backspace and `]` walk the trail, `k`, `h` and `x` filter, and the
  center's links have keys ([docs/ui/TUI.md](../../../../docs/ui/TUI.md#components)).
- **Docs** (Artifacts › Docs) is the Markdown doc of any artifact, package or tree, made from its template as the server
  has it now, with Mermaid diagrams (a click opens one in a zoom dialog) and a button that copies it; the entity modeler and service writer have a Doc tab,
  the screen builder a Doc panel. **Export docs** writes a tree of them into a repository (`autodoc/` by default), as
  `moqui autodoc` does ([docs/dev/Autodoc.md](../../../../docs/dev/Autodoc.md)).
- **Settings** has the site components, "Allow editing upstream components", the roots' labels, read-only flags and
  publish branches, more directories, network git on a shared dev server, and the **group servers**
  ([plans/design-dev-aide.md](../../../../plans/design-dev-aide.md) §21): Connect signs in at a team's server (or
  moqui.org) as an installed app does, with this instance's own address as the redirect, or takes a pasted token, and
  then sets what you confirm: the `myaddons.xml` repository, the task source, the LLM gateway's `.env` lines (with a
  key for the gateway alone) and git's credential; Remove takes back only what it set. The Workspace then knows each
  working tree's hosted repository: a Merge requests tab (Check out, New, Review with the server's threads: reply,
  resolve, a comment on a line, a review), open threads in the editor's gutter, check chips on Commits and Branches
  (the runs, a job's log, failed tests as links, Re-run), the contributor agreement's warning and Sign, Propose (push
  to review) for a repository you may only view, Fork where the server makes forks, and Push or Publish that opens a
  merge request. None of Dev AIDE's own writes (a file, a git command that changes a repository, a call of a group
  server) runs under an agent's fence or in a sim: `try_service` refuses its write services, and the writes refuse
  themselves. Dev Assist's `hosting` tool reads the same (merge requests, threads, checks, a job's log and failed tests,
  an issue) as untrusted data, opens a merge request or comments only by your approval, never approves, merges,
  re-runs, resolves or signs, and is refused the diff of a merge request whose authors have not signed. A commit's
  checks have **Run locally**: a job of the working tree's `ci.xml` in a host session of the Terminal, its steps and
  variables but not its image or secrets. The **Sessions** tab makes a worktree of a root for an Assist session
  (`runtime/worktrees`, a branch of its own), where that session's file and git tools work; a worktree is not loaded,
  so its changes are checked and committed there, not run live. The server's REST decides, as you there: the framework's
  ([Git-Hosting](../../../../docs/interfaces/Git-Hosting.md#rest)), or the REST of the application that keeps the
  repository (Smithy). Steps: [Run-Deploy](../../../../docs/Run-Deploy.md#dev-instances-with-a-dev-group-server).

It gives its users code execution as the server's OS user, so it loads only when `instance_purpose` is dev:
`runtime/components/dev` is a dev-only component directory, and its `component.xml` says `purpose="dev"`. Bind the server to
127.0.0.1 (`webapp_http_bind=127.0.0.1`) unless it is a team dev server (`dev-aide shared="true"`).

Who may use it (`data/DevAideSecurityData.xml`):

| Permission | Who has it | What it allows |
|---|---|---|
| `DEV_AIDE_VIEW` | | Browse, search and read files. |
| `DEV_AIDE_EXTEND` | `DEV_AIDE_EXTENDERS` | Write site components and publish them. |
| `DEV_AIDE` | `ADMIN`, `DEV_AIDE_USERS` | Everything: upstream edits with the toggle on, and every git command. |

Its Rust crate is linked into the Dev and Debug packages (the `devtools` feature). In another package the screens load
and the services say they are not built in.

- **Tests** ([the two suites](../../../../docs/Build-Test.md#the-two-suites)): the regression suite is `./moq test
  dev-aide` (the screens, the docs), `cargo test -p dev-aide` (confinement, the git parsers, the Browser's sections),
  `moqui test-client runtime/components/dev/dev-aide --offline` (the client), `cargo test -p moqui-host --test it devaide` (saves, the lock and tiers, git and Publish on scratch
  repositories, an entity save's relink, a docs export; `devaide_group`: the instance as its own group server, Connect
  through the app sign-in, merge requests, push to review, check out, review threads, statuses, the agreement and
  Sign, the `hosting` tool, two sessions' worktrees, a `ci.xml`'s local jobs). The Chromium smokes are acceptance tests, run by
  `./moq accept-all`: `cargo test -p moqui-host --test gate3_browser the_workspace` and
  `the_entity_modeler`, `the_screen_builder` and `the_service_writer`. `test/vectors/` are the visual views' operations, which the page tests make and the
  crate's tests apply (`MOQUI_REGENERATE_VECTORS=1 cargo test -p dev-aide` writes their outlines and texts again).
