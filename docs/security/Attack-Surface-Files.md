# Attack surface: DAV and SFTP

DAV (files, calendars and contacts) and SFTP. The `/resources` file manager and the upload transitions of the root screen are rows in [Attack-Surface-Routes.md](Attack-Surface-Routes.md). This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## DAV

The routes are always registered. There is no `<dav-server>` element in `MoquiDefaultConf.xml`. The defaults are the Rust fallbacks, and `docs/interfaces/Sync.md` states the same numbers.

| Flag | Default | Effect |
| --- | --- | --- |
| `enabled` | true | `false` is 404 `the DAV server is off` |
| `allow-http` | false | otherwise credentials need a secure request: HTTPS, a believed proxy's https, or loopback |
| `account-password` | false | app password only. `true` also accepts the account password |
| `max-file` | 268435456 (256 MiB) | 413 over that body |
| `sftp-drives` | true | SFTP to drives for a user without `SSH_SFTP` |

OPTIONS is answered with no login: `DAV: 1, 2, 3, calendar-access, addressbook, extended-mkcol`, and `Allow` lists `OPTIONS, GET, HEAD, PUT, DELETE, PROPFIND, PROPPATCH, MKCOL, COPY, MOVE, LOCK, UNLOCK, REPORT`.

Every other method is HTTP Basic. No credentials is 401 `WWW-Authenticate: Basic realm="Moqui", charset="UTF-8"`. Plain HTTP while `allow-http` is false is 403 `credentials over plain HTTP are refused; use HTTPS`. A path whose user segment is not the signed-in username (case-insensitive) is 404. Writes go through the file and adapter services as that user. `docs/interfaces/Sync.md` paths: `/dav/`, `/dav/principals/<user>/`, `/dav/files/<user>/<drive>/<path>`, `/dav/calendars/...`, `/dav/addressbooks/...`. The framework keeps no calendar or contact of its own. An application serves them through adapter services, and Mantle sets those adapters.

Projected address books (Mantle's `ContactProjection`, IronStone Sales' My accounts) are read-only collections
`projected-<id>` computed per request by their owner service as the signed-in user: the collection is listed only when the
owner admits the user, its cards are the parties the owner answers for that user (Sales: the contacts of the accounts the
user manages or has deals with, in their Sales scope), and `PUT` and `DELETE` on them answer 403. No `ContactBook` row is
copied.

`login_user` does not call `finish_login` when a second factor is required. DAV requires that login to succeed and requires a non-empty current user id. The wrapper that maps a second-factor result onto the current user was not read.

## SFTP

SFTP is the SSH `sftp` subsystem. Nothing listens for it unless the SSH server is up. `docs/interfaces/Sync.md` "SFTP to the drives" is this subsystem. The outbound SFTP client dials other hosts.

`sftp-drives` defaults to true. `sftp-root` defaults to `runtime://sftp`. `sftp-max-file` defaults to 256 MiB. The user must already be logged in by key, password, or app password. A dialog login is refused. `SSH_SFTP` (seeded on `ADMIN`) selects the root: one resource location (`runtime://`, `file:`, `component://`, or `dbresource://`), paths normalized against `/`, and `..` that leaves the root is refused. Those calls run with authorization off. The permission checked at login is the gate. A user without `SSH_SFTP` gets the drives that user may open, when `sftp-drives` is true. `sftp-drives="false"` gives that user no SFTP. A write is an upload and a new version. A removal goes to the trash. At most 64 handles per channel.
