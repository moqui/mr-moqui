# Sync: CalDAV, CardDAV, WebDAV and SFTP

Phones and desktop apps reach a user's calendars, tasks, contact books and drives over the standard protocols, each logged
in with an **app password** of its own: the DAV server at `/dav` (WebDAV over drives, CalDAV, CardDAV) and SFTP to the
drives on the built-in SSH server. The framework keeps no calendar or contact of its own: an application serves them
through adapter services, and Mantle has them. As of Oct 21, 2026.

The code is `crates/moqui-core/src/dav/` (the protocol: `mod.rs`, `files.rs`, `pim.rs`, `locks.rs`, `xml.rs`),
`crates/moqui-host/src/dav_http.rs` (the routes) and `crates/moqui-host/src/ssh/sftp_fs.rs` (SFTP). Tests:
`crates/moqui-core/src/dav/tests.rs`, `crates/moqui-host/tests/it/ssh.rs` and `collab.rs` (the routes), and
mr-mantle `test/Dav.spec.js` (CalDAV and CardDAV on Mantle, through `org.moqui.impl.DavServices.serve#DavRequest`).
IronStone's screen is Commons, Settings, Sync.

## App passwords

`moqui.security.UserAppPassword`: one per device or app, made by `org.moqui.impl.UserServices.create#UserAppPassword`
(`name`; the password is in the out parameters only, 20 letters and digits in groups of four, about 99 bits) and ended by
`revoke#UserAppPassword`; `get#UserAppPasswords` lists them with the last use (written at most once a minute). Only the
SHA-256 of the password without its dashes, in lower case, is stored, as for a login key. It logs in with the username
(`moqui_core::user::login_app_password`); the account's own checks apply (disabled, password change, address).

| Taken by | |
|---|---|
| The DAV server | always; the account password only with `dav-server` `account-password="true"`, and never for an account with a second factor |
| SFTP and the other SSH channels | as the SSH password, tried first when it has the form of an app password |
| git over HTTPS | as the password, as an access token is |
| The web login | never |

A refused app password counts for web protection; it is not an account failure, so guessing cannot lock the account.

## The DAV server

| Path | |
|---|---|
| `/.well-known/caldav`, `/.well-known/carddav` | 301 to `/dav/` |
| `/dav/` | the root: `current-user-principal` |
| `/dav/principals/<username>/` | `calendar-home-set`, `addressbook-home-set`, `calendar-user-address-set` (the user's email) |
| `/dav/files/<username>/<drive>/<path>` | WebDAV over the drives |
| `/dav/calendars/<username>/<collection>/<name>.ics` | CalDAV |
| `/dav/addressbooks/<username>/<collection>/<name>.vcf` | CardDAV |

Every request logs in by HTTP Basic (devices send it each time). Credentials are taken over HTTPS, from the trusted proxy's
`X-Forwarded-Proto: https`, from loopback, or with `dav-server` `allow-http="true"`. A path under another user's name is
not found. OPTIONS answers without a login, with `DAV: 1, 2, 3, calendar-access, addressbook`. The body is read whole, up to
`dav-server` `max-file` (256 MiB), and the request runs on a request worker in a fresh context, as the user with
authorization on: the drive services are granted to all users through the seed group `DriveDeviceServices`, and each checks
the drive's members and shares itself, as for the Files screens.

| `dav-server` | Default | |
|---|---|---|
| `enabled` | true | false answers 404 under `/dav` |
| `public-url` | the webapp host and port | the base the Sync settings show devices (also the `dav_public_url` property) |
| `calendar-services`, `contact-services` | empty | the adapter classes; Mantle's `MoquiConf.xml` sets `mantle.work.CalendarDavServices` and `mantle.party.ContactDavServices` |
| `account-password` | false | take the account password too |
| `allow-http` | false | take credentials over plain HTTP |
| `max-file` | 268435456 | bytes of a request body |
| `sftp-drives` | true | SFTP to the drives for a user without `SSH_SFTP` |

`org.moqui.impl.DavServices.get#SyncAddresses` gives a user's addresses (with `driveId`, that drive's WebDAV folder and SFTP
path), and `serve#DavRequest` serves one request without a socket, in a context of its own, for a spec.

### WebDAV over drives

`/dav/files/<username>/` holds a folder per drive the user may open (`FileServices.get#UserDrives`), named by the drive (a
`/` in it as `-`; two of one name get the drive id after it), and below it the drive's files, through the file services as
the user:

| Method | |
|---|---|
| PROPFIND | `list#Folder` and `get#ResourceInfo`: depth 0 or 1 (infinity is 1); size, type, modified, an entity tag of the version, the locks, the user's privileges |
| GET, HEAD | the file (one byte range), `If-None-Match`; a folder as a page of links |
| PUT | `upload#Resource`, replacing: a new version of a file that is there (one commit on a version-controlled drive), the quota checked; `If-Match` and `If-None-Match: *` |
| DELETE | `trash#Resource`: the drive's trash, restored from Files |
| MKCOL | `create#Folder` |
| MOVE, COPY | `rename#Resource`, `move#Resource`, `copy#Resource` (across drives a copy and a trash); `Overwrite: F` refuses a target that is there, else it goes to the trash first |
| PROPPATCH | answered and not kept, so Finder and Windows go on; the server's own properties are refused |
| LOCK, UNLOCK | exclusive write locks in memory, up to an hour, on the file or folder (its resource location, `resource/locks.rs`), not the href: another user is refused (423), the owner is not. Every other writer keeps the lock too: the Files screens, SFTP drives and the drive services fail that one file ("locked by another user") and write the rest; a delete or move of a folder fails when something under it is locked |

`.DS_Store`, `._name`, `Thumbs.db` and `desktop.ini` are taken and not kept. A records drive is read below its managers, by
the record visibility of its folder map.

### CalDAV and CardDAV

The adapter class has four services, called as the user, which check access themselves:

| Service | In | Out |
|---|---|---|
| `get#DavCollections` | `collectionId` (only that one) | `collectionList`: `collectionId`, `name`, `description`, `color`, `order`, `readOnly`, `ctag`, `components` (`VEVENT`, `VTODO`) |
| `get#DavObjects` | `collectionId`, `objectNameList`, `withData`, `componentType`, `start`, `end` (epoch millis) | `objectList`: `name`, `etag`, `data`, `componentType` |
| `put#DavObject` | `collectionId`, `objectName`, `data`, `ifMatch`, `ifNoneMatch` | `etag`, `created`, `status` (a refusal: 400, 403, 409, 412) |
| `delete#DavObject` | `collectionId`, `objectName`, `ifMatch` | `status` |

The server answers PROPFIND (with `getctag` and `getetag`), GET, PUT, DELETE and the REPORTs `calendar-multiget`,
`calendar-query` (a component and a time range), `addressbook-multiget` and `addressbook-query`; a client keeps in step by
the ctag and the entity tags (sync-collection is not served, and not offered). MKCALENDAR and a DELETE of a collection are
refused: collections are made in the app.

**Mantle's adapters.** CalDAV (`mantle.work.CalendarDavServices`): each calendar the user may view, not free/busy only (read
only below edit permission and for a subscription); an event series is one object with its changed instances, written as a
calendar object (no METHOD, the user's reminders as alarms) by the ICalendar tool from `get#EventIcsMaps`; a PUT goes
through `store#CalendarFeedEvents` by its UID, the name the client used kept in `CalendarWorkEffort.davName`; a DELETE
cancels the event, or only takes it out of the calendar when it is in another. The **Tasks** collection (`tasks`, VTODO) is
the user's assigned `WetTask` work efforts (completed ones for 30 days): a new to-do is `create#SimpleTask` assigned to the
user, a change sets the name, description, status, priority, percent, start, due and completion, and a delete ends the user's
assignment. Not carried: attendees a client writes, and the instances a client removes from a series. CardDAV
(`mantle.party.ContactDavServices`): each contact book the user may view, as vCard 3.0 from `export#Contacts`; a personal
book is written (a new card a personal contact through `import#ContactRow`, a changed card replaces the names and the
emails, phones, addresses and websites, a deleted card leaves the book and is deleted when it is in no other of the user's
books), a shared book is read only; the card's UID and name are kept in `ContactBookParty.davUid` and `davName`. The entity
tags hash the rows each text is made of; the ctag the tags.

## SFTP to the drives

A user with `SSH_SFTP` gets `ssh-server` `sftp-root` as before. Any other user who logs in to the SSH server (an app password,
a key, or the password of an account with no second factor) gets the drives they may open, one folder each, named as in
WebDAV (`moqui_core::dav::files::tree`): a write is an upload and a new version, a removal goes to the trash, a rename across
folders or drives is a move. Each call logs the user in on a request worker. `dav-server` `sftp-drives="false"` gives such a
user no SFTP.
