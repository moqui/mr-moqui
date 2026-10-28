# Dev AIDE

The assisted IDE of mr-moqui (`plans/design-dev-aide.md`), at `/qapps/dev` on a dev instance:

- **Home** has the site's unpublished changes with Publish, each repository and directory with its branch and changes,
  the reload log, the restart a change needs, and "No site component yet. Create one".
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
- **Settings** has the site components, "Allow editing upstream components", the roots' labels, read-only flags and
  publish branches, more directories, and network git on a shared dev server.

It gives its users code execution as the server's OS user, so it loads only when `instance_purpose` is dev:
`runtime/dev-component` is a dev-only component directory, and its `component.xml` says `purpose="dev"`. Bind the server to
127.0.0.1 (`webapp_http_bind=127.0.0.1`) unless it is a team dev server (`dev-aide shared="true"`).

Who may use it (`data/DevAideSecurityData.xml`):

| Permission | Who has it | What it allows |
|---|---|---|
| `DEV_AIDE_VIEW` | | Browse, search and read files. |
| `DEV_AIDE_EXTEND` | `DEV_AIDE_EXTENDERS` | Write site components and publish them. |
| `DEV_AIDE` | `ADMIN`, `DEV_AIDE_USERS` | Everything: upstream edits with the toggle on, and every git command. |

Its Rust crate is linked into the Dev and Debug packages (the `devtools` feature). In another package the screens load
and the services say they are not built in.

- **Tests:** `./moq test dev-aide` (the screens), `cargo test -p dev-aide` (confinement, the git parsers),
  `moqui test-client runtime/dev-component/dev-aide --offline` (the client), `cargo test -p moqui-host --test it devaide` (saves, the lock and tiers, git and Publish on scratch
  repositories, an entity save's relink), and `cargo test -p moqui-host --test gate3_browser the_workspace` and
  `the_entity_modeler` and `the_screen_builder` (Chromium). `test/vectors/` are the visual views' operations, which the page tests make and the
  crate's tests apply (`MOQUI_REGENERATE_VECTORS=1 cargo test -p dev-aide` writes their outlines and texts again).
- **Screenshots:** `MOQUI_DEV_SHOTS=<dir>` writes them in light, dark and 390 px.
