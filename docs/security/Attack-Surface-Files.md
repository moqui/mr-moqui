# Attack surface: DAV and SFTP

DAV (files, calendars and contacts) and SFTP. The `/resources` file manager and the upload transitions of the root screen are rows in [Attack-Surface-Routes.md](Attack-Surface-Routes.md). This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## DAV

The routes are always registered. There is no `<dav-server>` element in `MoquiDefaultConf.xml`. The defaults are the Rust fallbacks, and `docs/interfaces/Sync.md` states the same numbers.

| Flag | Default | Effect |
| --- | --- | --- |
| `enabled` | true | `false` is 404 `the DAV server is off` for every method but OPTIONS; OPTIONS and the `/.well-known/` redirects still answer, and the request body is still read up to `max-file` |
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

### Request handling

The host reads the whole request body into memory, up to `max-file`, within `webapp_body_timeout`, before `moqui_dav::dav::serve` checks credentials (`crates/moqui-host/src/dav_http.rs`). A declared `Content-Length` over the limit is 413 before the body is read. A failed login is recorded for web protection (`Event::AuthFailure`). After login, a path segment that decodes to `.`, `..` or holds `/` is 400 (`crates/moqui-dav/src/dav/mod.rs`). The request body is parsed as XML for every method but PUT.

A drive is named by its folder name (case ignored) or its `driveId`, from the list `FileServices.get#UserDrives` returns for the user. GET of a file sends `Content-Type` by the file's name with `ETag`, `Accept-Ranges` and `Last-Modified`, and no `Content-Disposition`. GET of a folder is an HTML list of escaped names. PUT calls `upload#Resource` with `conflictPolicy` `replace`. DELETE calls `trash#Resource`. COPY and MOVE take a `Destination` under `/dav/files/<the same user>/`, else 502, and call `copy#Resource` or `move#Resource`. File content is opened with authorization off after `get#ResourceInfo` admitted the path (`open_file` in `crates/moqui-dav/src/dav/files.rs`).

### Calendars and contacts

`crates/moqui-dav/src/dav/pim.rs` holds no calendar or contact logic of its own. It calls four services of the class named by `dav-server` `calendar-services` or `contact-services`, as the signed-in user with authorization on (`sync_checked`): `get#DavCollections`, `get#DavObjects`, `put#DavObject` and `delete#DavObject`. mr-mantle's `MoquiConf.xml` sets `mantle.work.CalendarDavServices` and `mantle.party.ContactDavServices`. A PUT or DELETE on a collection whose row has `readOnly` is 403 `need-privileges` before `put#DavObject` or `delete#DavObject` is called.

Calendars (`runtime/components/addon/mr-mantle/service/mantle/work/CalendarDavServices.xml`): the collections are the calendars `CalendarServices.get#UserCalendars` answers with `CalpView`, `CalpEdit` or `CalpManage`, a `tasks` collection, and each `projected-<id>` collection whose owner service admits the user. `get#DavObjects` calls `check#CalendarAccess` for a calendar. `put#DavObject` and `delete#DavObject` need `CalpEdit` or `CalpManage` on a calendar that is not a subscription (else 403), and `EventServices.check#EventAccess` for an existing event. An event is stored through `CalendarServices.store#CalendarFeedEvents`; a task is made with `TaskServices.create#SimpleTask` assigned to the user's own party, or changed with `update#Task` and `set#TaskStatus`. `put#DavObject` itself calls no mail service. The object text is parsed by the `ICalendar` tool; the `data` parameter is `allow-html="any"`.

Contacts (`mantle/party/ContactDavServices.xml`): `get#DavObjects` calls `ContactBookServices.check#ContactBookAccess` with `CbpView`. `put#DavObject` and `delete#DavObject` answer 403 unless the user is the book's owner and the book is `CbvPersonal`. `get#ContactObjectTags` with a `partyIdList` (the projected books) finds those `mantle.party.Party` rows with authorization off.

The parsers are in `crates/moqui-dav/src/pim`. A recurrence rule expands to at most 100000 instances (1000 for an open-ended rule) over at most 2000000 periods (`rrule.rs`). No other count or size constant was found in `ics.rs`, `vcard.rs` or `line.rs` by search; the request body limit (`max-file`) is the bound on an object's size.

`login_user` does not call `finish_login` when a second factor is required. DAV requires that login to succeed and requires a non-empty current user id. The wrapper that maps a second-factor result onto the current user was not read.

## SFTP

SFTP is the SSH `sftp` subsystem. Nothing listens for it unless the SSH server is up. `docs/interfaces/Sync.md` "SFTP to the drives" is this subsystem. The outbound SFTP client dials other hosts.

`sftp-drives` defaults to true. `sftp-root` defaults to `runtime://sftp`. `sftp-max-file` defaults to 256 MiB. The user must already be logged in by key, password, or app password. A dialog login is refused. `SSH_SFTP` (seeded on `ADMIN`) selects the root: one resource location (`runtime://`, `file:`, `component://`, or `dbresource://`), paths normalized against `/`, and `..` that leaves the root is refused. Those calls run with authorization off. The permission checked at login is the gate. A user without `SSH_SFTP` gets the drives that user may open, when `sftp-drives` is true. `sftp-drives="false"` gives that user no SFTP. A write is an upload and a new version. A removal goes to the trash. At most 64 handles per channel.

### The SFTP handler

`crates/moqui-host/src/ssh/sftp_fs.rs` implements `realpath`, `stat`, `lstat`, `fstat`, `setstat`, `fsetstat`, `opendir`, `readdir`, `open`, `read`, `write`, `close`, `remove`, `mkdir`, `rmdir` and `rename`. It has no `symlink` or `readlink` handler. A client path is split on `/`; `.` and empty parts are dropped, `..` removes the part before it, and a `..` with nothing before it is `PermissionDenied` `outside the SFTP root`.

With `SSH_SFTP` the target is the `sftp-root` text followed by the normalized path, and each operation runs on a worker with authorization off through the resource facade (`stat`, `list`, `get`, `open`, `write_bytes`, `delete`, `make_directory`, `rename`). Which roots take writes is the facade's rule ([Resource facade schemes](#resource-facade-schemes)). Without `SSH_SFTP` each operation runs after `internal_login_user` of the channel's user through `moqui_dav::dav::files::tree`, the drive services as that user.

A file opened for writing is held in memory until it is closed. The open files of one channel hold at most `sftp-max-file` bytes. The handler accepts every session channel a logged-in connection opens (`channel_open_session` in `ssh/server.rs` keeps no count), and each channel may start the `sftp` subsystem.

## Drives and public shares

### Public shares

Public shares are five services of `org.moqui.resource.FileServices`, each `authenticate="anonymous-all"`: `check#PublicShareToken`, `open#PublicShare`, `list#PublicShare`, `download#PublicShare`, `upload#PublicShare`. mr-boss serves them at `/s/<token>` through the `PublicShare.xml` transitions `open`, `files`, `download` and `upload`; `open` and `upload` are `require-session-token="false"` and name no method ([CSRF](Attack-Surface-Web.md#csrf)).

A link is a `moqui.resource.ResourceShare` row with `isLink` `Y`, made by `share#Resource`, which needs edit level on the path. The token is `ec.user.randomString(32)`: 32 characters of URL-safe Base64 from 32 random bytes. Only `singleUseCodeHash(token)` is stored, in `linkTokenHash`. A token must match `^[A-Za-z0-9_-]{16,64}$`. A revoked or expired link is not valid. The services do not check again that the link's maker still has access to the path.

A link's password is stored as a password hash and checked by `open#PublicShare`, which returns a grant: `<expiry millis>.<HMAC-SHA256>` keyed by the link's token hash and password hash, valid 3600 seconds, compared in constant time (`check_grant` in `crates/moqui-core/src/script/host/files.rs`). `list#`, `download#` and `upload#PublicShare` ask for the grant only when the link has a password. `open#PublicShare` keeps no count of wrong passwords of its own.

A path with a `..` segment is "Not found", and a link to a whole drive does not reach `.trash`. `download#PublicShare` sends a file itself with `ec.web.sendResourceResponse`; inline is asked only for PNG, JPEG, GIF, WebP, BMP and PDF. A folder is zipped to `runtime://tmp/drive-zip/<16 random characters>.zip` (at most 20000 entries, depth 32).

`upload#PublicShare` takes files on a folder link with `RspUpload` or `RspEdit`, and calls `upload#Resource` with authorization off and `asInternal` true, `conflictPolicy` `keepBoth`. The only size limit in the service is the drive's `quotaBytes`, which is empty unless set. The uploader's name (100 characters) goes into the version note. No type or content scan is done.

### Drive services

`org.moqui.resource.FileServices` serves drives (`moqui.resource.ResourceDrive`) over the resource facade. The `DriveDeviceServices` artifact group grants `get#UserDrives`, `list#Folder`, `get#ResourceInfo`, `upload#Resource`, `create#Folder`, `rename#Resource`, `move#Resource`, `copy#Resource` and `trash#Resource` to `ALL_USERS`, `AUTHZT_ALWAYS`, with `inheritAuthz` `Y` (`runtime/components/base/framework/data/SecurityTypeData.xml`). The other services are reached through an application's screens.

Each service calls `get#DriveAccess`. The owner is level 3. A current `ResourceDriveMember` row of the user or one of their groups gives its level. A `ResourceShare` to the user, a group or their party gives view or edit on its path and below. Another user's access is answered only with authorization off. A path with a `..` segment is level 0. A mirror and an archived drive are read only. On a records drive a user below manage level reaches only mapped folders of records the visibility hook admits, and a user with `EXTERNAL_USER` reaches none.

`create#Drive` refuses `mirror` storage, a records drive and an explicit `rootLocation` unless the caller is in `ADMIN` or authorization is off. `transfer#DriveOwner` and `find#OwnedDrives` need the owner or `DRIVE_OWNER_CHANGE`. Three parameters name a server path and are not checked in the service: `create#Drive` `localPath` (stored on the `GitRepository` row), and `export#Drive` `targetLocation` and `targetPath`. The mr-boss wrappers (`DriveServices.create#SharedDrive`, `DriveSettings.xml`) do not pass them.

### Chunked uploads

`org.moqui.edge.ClientSyncServices.upload#Chunk` (the `uploadChunk` transition that Compose and Chat include) takes one chunk of a file of the signed-in user. `uploadId` must match `^[A-Za-z0-9_.:-]{1,64}$`. A file is at most 25 MB. A chunk other than the last is at least `client_upload_min_chunk_bytes` (262144). A user holds at most `client_upload_max_count` (20) unfinished uploads and `client_upload_max_mb` (100) MB in all. Chunks are stored under `dbresource://upload/<userId>/<token>` and rows are found by `userId` and `uploadId`.

### The drive picker

`org.moqui.comms.FilePickerServices` (`get#PickerData`, `list#PickerFolder`, `find#PickerFiles`) lists drives, a folder and search results for Compose and the Inbox through the `pickerData`, `pickerFolder` and `pickerSearch` transitions (GET, `read-only`). Every read goes through `FileServices.get#UserDrives`, `list#Folder`, `find#Recent` and `find#Resources` as the user. `pickerData` without `checkOnly` makes the user's personal drive.

## The `/resources` file manager (elFinder)

The route is a row in [Attack-Surface-Routes.md](Attack-Surface-Routes.md). `ElFinderServices.run#Command` (`crates/moqui-dav/src/elfinder.rs`) implements `file`, `open`, `tree`, `parents`, `ls`, `get`, `mkdir`, `mkfile`, `rm`, `rename`, `upload` and `put`. `resourceRoot` and `volumeId` come from the request; with no `resourceRoot` the transition uses the `mantle.content.root` preference, else `dbresource://mantle/content`. A target is the volume id (3 characters) and the URL-safe Base64 of the path under the root. A path with a `..` segment or a segment holding `:` is `errFileNotFound`. `rm`, `rename` and `put` refuse the root. `rm` of a folder removes everything under it.

In production (`instance_purpose` `production`) the write commands answer "Write not allowed for this resource root" unless the root starts with `dbresource://`, `content://` or `git://`; the read commands are not restricted by the root's scheme. A `content://` root is an error unless a component installs a provider for that scheme, so in production only `dbresource://` and `git://` roots take writes. The `resources` transition is `read-only="true"`, and it runs the same commands for GET and POST ([CSRF](Attack-Surface-Web.md#csrf)).

## Resource facade schemes

`ResourceFacade::get` (`crates/moqui-core/src/context/resource.rs`) serves `dbresource`, `file`, `runtime`, `component`, `classpath`, `http` and `https`, and any scheme a provider registered (`git`, and the CI artifact scheme of `moqui-git`). `content` is an error unless a provider is installed for it. `write_bytes` writes `dbresource` rows, provider locations, and files under `file`, `runtime` and `component`; other schemes are not writable. The facade does not reject `..`: `runtime://` and `component://` join the given path onto their directory. Callers check paths.

## Archive extraction and image decoding

The script host function `resource_extract_archive` (`crates/moqui-core/src/script/host/archive.rs`) reads a tar, gunzipped when its first bytes are the gzip magic. It writes regular files only; a link, a device or any other entry type is skipped as `not a regular file`. An entry path with a root, a prefix or a `..` component is skipped as `an absolute or parent path`. Each file is written to `<target>/<path>` through the resource facade. The expanded stream is cut at `maxBytes` (4 GiB when the script gives none) and files past `maxEntries` (1000000 when the script gives none) are skipped. `resource_gunzip` also cuts its expanded output at a byte limit.

The image services (`crates/moqui-core/src/service/images.rs`) decode with the format guessed from the bytes and refuse an input wider or taller than 30000 pixels. A resize is refused past 16383 pixels a side or 50000000 pixels in all. A rich text image is at most 8 MiB.

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- The DAV XML reader's refusal of a DOCTYPE is stated in the module comment of `crates/moqui-dav/src/dav/xml.rs`; `moqui_lib::node` was not read.
- The bodies of `FileServices` `list#Folder`, `find#Resources`, `move#Resource`, `copy#Resource`, `trash#Resource`, the trash and version services, `download#Zip`, `get#Preview`, `clean#DriveTempFiles` and `index#DriveFiles` were not read past their `get#DriveAccess` calls.
- Every caller of `create#Drive` with `localPath` and of `export#Drive` with `targetLocation` or `targetPath` was not traced, and the path checks of `GitServices.init#GitRepository` and `export#GitRepository` were not read.
- The SFTP client (`crates/moqui-core/src/sftp.rs`, host key pinning) was not read.
- Whether the resource facade follows a symbolic link that is on disk under a `file:`, `runtime://` or `component://` SFTP root was not read.
- The `image` crate's default allocation limit and the image services' concurrency slot (`image_slot`) were not read.
- `moqui_dav::dav::files::tree` (the SFTP drives backend) was not read.
- Whether `CalendarServices.store#CalendarFeedEvents` or `EventServices.check#ResourceBookings` sends mail or a notification to an address taken from a client's calendar object was not traced. The adapter's header comment says attendees and organizer written by a client are not carried; the code that drops them was not read.
- What reaches `ContactDavServices.get#ContactObjectTags` with a `partyIdList` besides the projected address books was not traced.
- `dav/locks.rs` (LOCK limits), PROPFIND depth, free-busy and the REPORT handlers were not read. `pim/ics.rs`, `vcard.rs`, `line.rs` and `zones.rs` were searched for limits, not read.

