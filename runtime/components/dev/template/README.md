# @component@

@Title@, an MR Moqui component. It starts with one record type, the item, and what each layer of an app does with it.

| Path | What is here |
| --- | --- |
| `component.xml` | The component: its name and version, and `depends-on` for the components it uses. |
| `MoquiConf.xml` | Mounts the app under the apps root, at `/qapps/@component@`. |
| `entity/@Class@Entities.xml` | `@package@.@Class@Item`, the item. |
| `service/@package@/@Class@Services.xml` | `create#Item` and `update#Item`. |
| `service/@component@.rest.xml` | The REST API: `/rest/s1/@component@/items`. |
| `screen/@Class@.xml`, `screen/@Class@/*.xml` | The app: Items (find, list and a New dialog) and the item's detail. |
| `data/@Class@SecurityData.xml` | Seed: the app's screens for the ADMIN group. |
| `data/@Class@ApiSecurityData.xml` | Seed: the REST API for the ADMIN group. |
| `data/@Class@DemoData.xml` | Demo items. |
| `test/*.spec.js` | The regression suite: specs of the services' rules and the REST API. |
| `test/acceptance/*.spec.js` | The acceptance suite: a sweep of the app's screens. |

A component made with `./moq component-create --rest` has no screens, and one made with `--screens` has no REST API.

## Run and test

- `./moq run` loads it on the next start, at `/qapps/@component@` once signed in; `./moq load` loads its seed and demo
  data.
- `./moq test @component@` runs its regression specs, while working and (with `./moq test-apps`) before a commit.
- `./moq accept @component@` runs its acceptance specs: on a schedule, before a release, when asked, or after adding or
  changing one.

Practices for screens, services and specs: `docs/dev/Practices-App-Dev.md` in mr-moqui; the two suites:
`docs/Build-Test.md#the-two-suites`.
