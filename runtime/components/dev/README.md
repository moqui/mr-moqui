# The dev set

Components that load only on a dev instance (`instance@purpose` is `dev`, as with `./moq run` and `./moq test`):
`../components.xml` lists this directory as a `component-dir` with `purpose="dev"`. `./moq archive` leaves the set out
unless `--with-dev`. The framework's are in the
mr-moqui repository (each has its own `.gitignore` exception); a developer's own sit beside them in repositories of
their own (`./moq component-get --dev`, `./moq component-create --dev`).

| Directory | What it is |
| --- | --- |
| `demo/` | Locks a server down for a public demo: the apps with the john.doe login, not the sharp tools. Off by default; a demo server names it in its conf (`runtime/conf/MoquiDemoConf.xml`), which loads it on a production instance. |
| `dev-aide/` | Dev AIDE, the assisted IDE at `/qapps/dev` ([plans/design-dev-aide.md](../../../plans/design-dev-aide.md)). |
| `example/` | Rust in a component, and the screen archetype gallery at `/qapps/gallery`. |
| `template/` | The component template that `./moq component-create` and the Dev AIDE's New component copy. Not a component: its `component.xml` is `disabled="true"`, so it never loads. |

A crate here (a `Cargo.toml` beside the `component.xml`) is linked only into the packages with the dev tools (Dev and
Debug); dev-aide's and example's are members of the workspace.

## The component template

A new component is a copy of `template/` (an App), or of one of its other kinds in `template/kinds/<kind>`, with the
placeholders in its file names and contents filled in from the component's name, and the template's `disabled="true"`
taken out. For `my-shop`:

| Placeholder | Value | Where |
| --- | --- | --- |
| `@component@` | `my-shop` | The component name, its mount (`/qapps/my-shop`) and REST resource (`/rest/s1/my-shop`) |
| `@Title@` | `My Shop` | Menu and screen titles |
| `@Class@` | `MyShop` | File names, the app's root screen, the entity |
| `@package@` | `myshop` | The entity and service package |
| `@ID@` | `MY_SHOP` | Artifact group and demo record ids |

A name starts with a letter and has letters, digits, `-` and `_`. The kinds (`./moq component-create --kind`, Home's
New component), each with a `README.md` that lists its files:

| Kind | From | What it starts with |
| --- | --- | --- |
| `app` (the default) | `template/` | An entity, services, a REST API, screens, security and demo data, regression and acceptance specs. Made REST API only (`--rest`) it has no `screen/`, `MoquiConf.xml`, `data/@Class@SecurityData.xml` or screen specs; made screens only (`--screens`), no `service/@component@.rest.xml`, `data/@Class@ApiSecurityData.xml` or `test/@Class@Api.spec.js`. It never has `kinds/`. |
| `data` | `kinds/data/` | Seed and demo data files and a spec; no screens, services or code. |
| `native` | `kinds/native/` | `Cargo.toml` and `src/lib.rs` with `submit_component!` and a `type="rust"` service, which `./moq build` links into the binary ([Native-Components](../../../docs/dev/Native-Components.md)). |
| `theme` | `kinds/theme/` | A `ScreenTheme` of the `/qapps` shell built on the Default theme, its style sheet served by `MoquiConf.xml`, a body class and a color scheme ([Qvt-Client, Themes](../../../docs/ui/Qvt-Client.md#themes)). |
| `skills` | `kinds/skills/` | A skill (`skill/@package@.md`), a suggested prompt (`AgentSuggestedPrompt` seed) and the retrieval cases; no code. |
| `dev-tool` | `kinds/dev-tool/` | Mock services and a test data screen, `purpose="dev"`, made in `runtime/components/dev` always. |

`--git-remote URL` (Home: Git remote) starts the repository with that remote as `origin`: an `https` URL with no user
part (or `http` to 127.0.0.1), `ssh://`, `git://` or `user@host:path`; a URL with a credential in it, a local path,
`file:` or git's `<transport>::` is refused before anything is made. `moq` and the Dev AIDE (`dev-aide/src/template.rs`,
the remote check in `dev-aide/src/vcs.rs`) follow these same rules, and a dev-aide test checks that they make the same
files for each kind and part and refuse the same remotes.

The placeholders make some files not well-formed XML (the demo data's `<@package@.@Class@Item>`), so the tools that
scan components (`moqui test`, `moqui lint --screens`, the XML corpus test) leave the template out. To check a change
to the template, make a component from it and run its regression specs, and its acceptance specs when the change is
to one ([the two suites](../../../docs/Build-Test.md#the-two-suites)):

```sh
./moq component-create tpl-check --no-git && ./moq test tpl-check && ./moq accept tpl-check \
  && rm -rf runtime/components/addon/tpl-check
```

For another kind, `--kind <kind>` the same way (a dev tool is made in `runtime/components/dev`); a native one needs
`./moq build` before its specs run.
