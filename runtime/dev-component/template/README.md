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
| `test/*.spec.js` | Specs of the services, the screens and the REST API. |

A component made with `./moq component-create --rest` has no screens, and one made with `--screens` has no REST API.

## Run and test

- `./moq run` loads it on the next start, at `/qapps/@component@` once signed in; `./moq load` loads its seed and demo
  data.
- `./moq test @component@` runs its specs.

Practices for screens, services and specs: `docs/dev/Practices-App-Dev.md` in mr-moqui.
