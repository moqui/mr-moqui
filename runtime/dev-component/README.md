# dev-component

Components that load only on a dev instance (`instance_purpose` is `dev`, as with `./moq run` and `./moq test`):
`MoquiDefaultConf.xml` lists this directory as a `component-dir` with `purpose="dev"`. The framework's are in the
mr-moqui repository (each has its own `.gitignore` exception); a developer's own sit beside them in repositories of
their own (`./moq component-get --dev`, `./moq component-create --dev`).

| Directory | What it is |
| --- | --- |
| `demo/` | Locks a server down for a public demo: the apps with the john.doe login, not the sharp tools. Off by default; a demo server names it in its conf (`runtime/conf/MoquiDemoConf.xml`), which loads it on a production instance. |
| `dev-aide/` | Dev AIDE, the assisted IDE at `/qapps/dev` ([plans/design-dev-aide.md](../../plans/design-dev-aide.md)). |
| `example/` | Rust in a component, and the screen archetype gallery at `/qapps/gallery`. |
| `template/` | The component template that `./moq component-create` and the Dev AIDE's New component copy. Not a component: its `component.xml` is `disabled="true"`, so it never loads. |

A crate here (a `Cargo.toml` beside the `component.xml`) is linked only into the packages with the dev tools (Dev and
Debug); dev-aide's and example's are members of the workspace.

## The component template

A new component is a copy of `template/` with the placeholders in its file names and contents filled in from the
component's name, and the template's `disabled="true"` taken out. For `my-shop`:

| Placeholder | Value | Where |
| --- | --- | --- |
| `@component@` | `my-shop` | The component name, its mount (`/qapps/my-shop`) and REST resource (`/rest/s1/my-shop`) |
| `@Title@` | `My Shop` | Menu and screen titles |
| `@Class@` | `MyShop` | File names, the app's root screen, the entity |
| `@package@` | `myshop` | The entity and service package |
| `@ID@` | `MY_SHOP` | Artifact group and demo record ids |

A name starts with a letter and has letters, digits, `-` and `_`. Made REST API only (`--rest`), a component has no
`screen/`, `MoquiConf.xml`, `data/@Class@SecurityData.xml` or `test/@Class@Screens.spec.js`; made screens only
(`--screens`), no `service/@component@.rest.xml`, `data/@Class@ApiSecurityData.xml` or `test/@Class@Api.spec.js`.
`moq` and the Dev AIDE (`dev-aide/src/template.rs`) follow these same rules, and a dev-aide test checks that they make
the same files.

The placeholders make some files not well-formed XML (the demo data's `<@package@.@Class@Item>`), so the tools that
scan components (`moqui test`, `moqui lint --screens`, the XML corpus test) leave the template out. To check a change
to the template, make a component from it and run its specs:

```sh
./moq component-create tpl-check --no-git && ./moq test tpl-check && rm -rf runtime/component/tpl-check
```
