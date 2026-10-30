# demo

Locks a server down for a public demo. The apps stay fully usable, writes included, with the well-known `john.doe` /
`moqui` login (the Login screen's Test Login button). What is taken away: running code or SQL, bulk import and export,
the file manager, writes in Tools and System, the servers a visitor does not need, and changes that would lock other
visitors out of the shared login. It is off by default.

This is not production policy. A production server loads no demo users and does not publish Tools
([Attack-Surface](../../../docs/security/Attack-Surface.md#heavy-lock-down)).

## Turning it on

- **A demo server** runs on the production purpose with `runtime/conf/MoquiDemoConf.xml`, which names this component.
  A conf that names a component loads it on any instance purpose, even when its `component.xml` is disabled:

  ```sh
  ./moq load -c conf/MoquiDemoConf.xml              # all data: the demo users and the lock-down
  ./moq run-production -c conf/MoquiDemoConf.xml
  ```

  A first start on an empty database loads all data too (`entity_empty_db_load=all` in that conf).
- **A dev instance**: `./moq run -D demo_disabled=false`, to try it. Never run a public demo as a dev instance: the Dev
  AIDE loads there, and `moqui tui --user` logs in with no password.

`component.xml` has `disabled="${demo_disabled ?: 'true'}"`. The property is read before `runtime/.env` and the conf's
default properties, so set it with `-D` or in the environment.

## What it does

| Where | What |
| --- | --- |
| `MoquiConf.xml` | Puts `screen/Disabled.xml` in place of Data Import, Data Export, Data Snapshot, Speed Test, SQL Runner, SQL Script Runner, Service Run, Service Load Runner and the Resource Finder (ElFinder), and leaves them out of the menus. Speed Test and the Data Import check run as the screen renders, so VIEW-only would not stop them. |
| `MoquiConf.xml` | Turns off SSH (`ssh-server enabled="false"`), WebDAV, the MCP server and client, A2A both ways and the LLM gateway, whatever their properties say. Mail connects only to `localhost`, so no one syncs a real mailbox into Comms, where every visitor would read it. |
| `data/DemoSecurityData.xml` (seed) | ADMIN is VIEW-only on Tools, System, Monitor, the `/moqui` REST API, EntitySync and SystemMessage services, git repositories (no push or mirrors), report git sync and report webhooks. ADMIN loses the search and dashboard proxies, `ADMIN_PASSWORD` (another user's password without the old one), SFTP and git over SSH, telemetry ingest, and report files written on the server. |
| `data/DemoUserData.xml` (demo) | John Doe leaves ADMIN_ADV: no login as another user, SQL Runner, SSH admin shell or service load runner. ADMIN_ADV is a privileged group, so a member of ADMIN cannot add him back. |
| `service/Demo.secas.xml` | Before a password change or reset, a sign-in factor, or a change of username, disabled or require-password-change, `demo.DemoServices.refuse#SharedAccountChange` stops it for a user in `demo_shared_user_ids` (default `EX_JOHN_DOE`). |

The security data is `seed`, not `demo` as in Moqui Java's `moqui-demo`: each seed load runs the tools and framework
seed and then this, so the lock-down holds. The John Doe row is `demo` because a seed load without the demo users would
fail its foreign key.

Still open: every app, Tools views (AutoFind and Data Edit show every entity), the System views, Dashboards and reports,
Assist, and the browser terminal (`/termws`, the same screens checked as the user). `/notws` and PDF stay on.

## Running a demo server

- Restore a saved database on a schedule (`./moq save-db demo` once after the load, then `./moq reload-save demo` with
  the server stopped): visitors change the demo data.
- Every visitor is John Doe, so the `ALL_SCREENS` tarpit (120 screens a minute per user) is shared.
- Keep real keys and passwords out of the database: Data Edit shows every entity to a VIEW-only user.
- Leave `llm_openai_api_key` empty, or accept the `llm-facade` `user-daily-cost` as the cap for every visitor together.
- To undo the lock-down on a database, load seed, seed-initial and demo data again with the component off.

## Tests

```sh
./moq test -D demo_disabled=false demo
```

`test/Demo.spec.js` renders each stubbed screen in qvt and tui, checks the security rows and what John Doe may still do,
calls each guarded account service, and checks that WebDAV and SFTP are off. `./moq test-all` runs it.
