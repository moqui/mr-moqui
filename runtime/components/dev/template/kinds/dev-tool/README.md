# @component@

@Title@, a dev tool: mock services and a test data screen, loaded only on a dev instance. It lives in
`runtime/components/dev`, a component directory with `purpose="dev"`, and its `component.xml` says `purpose="dev"`
too, so a copy elsewhere stays out of production; `./moq archive` leaves it out unless `--with-dev`.

| Path | What is here |
| --- | --- |
| `component.xml` | The component: its name, version and `purpose="dev"`. |
| `MoquiConf.xml` | Mounts the tool under the apps root, at `/qapps/@component@`. |
| `screen/@Class@.xml` | The tool: Load test data, the test records, and a form that tries the mock. |
| `service/@package@/@Class@MockServices.xml` | `get#Rate`, a mock of an outside service, and `load#TestData`. |
| `data/@Class@TestData.xml` | Test records, of a data type of their own (`@package@-test`): a load of chosen types (`./moq load seed demo`) leaves them out. |
| `data/@Class@SecurityData.xml` | Seed: the tool's screen for the ADMIN group. |
| `test/@Class@Mock.spec.js` | The regression suite: the mock's answer and the test data load. |

## Run and test

- `./moq run` loads it on the next start (a dev instance), at `/qapps/@component@` once signed in.
- `./moq test @component@` runs its regression specs.
