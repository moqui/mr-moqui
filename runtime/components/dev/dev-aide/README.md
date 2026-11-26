# Dev AIDE

The assisted IDE of mr-moqui, at `/qapps/dev` on a dev instance: the Workspace (roots, the editor, git, live reload),
the artifact index and Browser, the entity modeler, the service writer and ECA rules, the screen builder, tasks and
group servers, and Dev Assist. How to use it is in the guide,
[docs/tools/Dev-AIDE.md](../../../../docs/tools/Dev-AIDE.md); the design and its build record are in
[plans/design-dev-aide.md](../../../../plans/design-dev-aide.md).

It gives its users code execution as the server's OS user, so it loads only when `instance@purpose` is dev
(`runtime/components/dev` is a dev-only component directory, and its `component.xml` says `purpose="dev"`). Its Rust
crate is linked into the builds with the `devtools` feature: Maximal (the default) and Debug, and Lite built with
`--features devtools`. Run it with `./moq run` on a dev runtime and open `https://localhost:8443/qapps/dev`.

What is here:

- `src/`: the crate: the workspace files and their confinement (`confine.rs`), the git layer (`git.rs`, `vcs.rs`), the
  views' services, the data load, hosted roots, Run locally, worktrees, the group server client (`hosting/`) and the
  dev agent's tools (`agent/`).
- `service/org/moqui/devaide/`: the services, and `GuidedEdit.js` and `GuidedOps.js` for the terminal's guided editors.
- `screen/DevAide/`: the screens; `screen/devstatic/`: the browser client of the Workspace and its views.
- `entity/`, `data/` (access, the dev agent, the worktree sweep job), `skill/` (the dev agent's skills and recipes),
  `template/site` (the Site template; the other kinds are `runtime/components/dev/template`).
- **Libraries** in Screens and Services lists the active edit component's exports. Use on this screen stages the qvt
  declaration in the same buffer as other edits; for a server library, copy the module call into an action. A
  full-tier editor of the component can search npm, choose a release and target, review the lock, notices and bundle
  sizes, and apply them. The dev instance needs the
  [build prerequisites](../../../../docs/Build-Test.md#add-a-component-library).

## Tests

See [the two suites](../../../../docs/Build-Test.md#the-two-suites). The regression suite:

- `./moq test dev-aide`: the screens, the Workspace's terminal form on a scratch root, the docs, the terminal's guided
  editors against a throw-away root, the three-way data load, a hosted root on a scratch repository, Replace, the
  editor settings, the rule fixtures and the push contexts.
- `cargo test -p dev-aide`: confinement, the git parsers, the Browser's sections, the edit buffers, the data load, the
  local runs and the worktrees. The opt-in real-engine run of Run locally needs Docker or Podman.
- `moqui test-client runtime/components/dev/dev-aide --offline`: the client in headless Chromium. `test/vectors/` are
  the visual views' operations, which the page tests make and the crate's tests apply
  (`MOQUI_REGENERATE_VECTORS=1 cargo test -p dev-aide` writes their outlines and texts again).
- `cargo test -p moqui-host --test it devaide`: saves, the lock and tiers, git and Publish on scratch repositories, an
  entity save's relink, a docs export; `devaide_group` runs the instance as its own group server (Connect, the scoped
  keys, merge requests, review threads, statuses, the agreement, the `hosting` tool, worktrees, local jobs).

The Chromium smokes are acceptance tests (`./moq accept-all`), each run alone with
`cargo test -p moqui-host --test gate3_browser <name>`: `the_workspace`, `the_artifact_browser`, `the_entity_modeler`,
`the_screen_builder`, `the_service_writer`, `a_dev_agent_change_set`, `a_group_server_merge_request`,
`an_assist_session_worktree` and `the_workspace_editor_features`. The dev agent's evaluation tasks run with
`moqui test dev-aide --agents --replay runtime/components/dev/dev-aide/test/agent/replay`.
