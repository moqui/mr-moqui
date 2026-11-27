# Location sharing: explicitly shared live location

Status 2026-11-27: LL0 to LL3 built (§4); LL4 (chat) and LL5 (XMPP) next. It carries the "explicitly shared person-location feeds" item of
[Maps §9, Later research](archive/Maps.md#9-delivery-and-acceptance) forward as work for now, on the user's decisions of
November 26, 2026: a person shares where they are, for a time they choose, with people they choose (users, a group,
their XMPP contacts, or a guest link for one record such as a delivery or a service visit); the Android app keeps
sharing in the background through a visible foreground service; the server keeps the latest position only, unless the
sharer keeps a trail for that share. Offline maps are [Maps-Offline](Maps-Offline.md).

## 1. Rules

From [Maps §8](archive/Maps.md#8-access-publication-and-reliability), mr-home
([§1.1](../runtime/components/addon/mr-home/plan/initial-design.md#11-design-guideline-child-autonomy-enablement-and-safety),
[§3.9](../runtime/components/addon/mr-home/plan/initial-design.md#39-maps-places-and-geographic-information) and its
"Child records and location") and
[Comms-Messaging](Comms-Messaging.md) (the location message):

- **Only by the person's act.** A share starts when its owner starts it, on a device they hold, and nothing else
  starts one: not an administrator, an organizer of a household, a manager, a record or a schedule. No position is
  inferred from an address, a photo, a check-in or a login.
- **Visible to everyone it concerns.** The owner sees every active share, its audience and its end, wherever they are
  in the app (a status chip in the app bar on every screen, Maps, the Android notification; the user's decision of
  November 27, 2026, since the shell's tray takes no app item: a generic shell indicator hook), and can stop any of them at once. Each person in an audience
  is told who shares with them and until when.
- **Ended by time.** Every share has an end, at most `location@max-share-hours` (24) after its start; extending it is
  the owner's act again. Revoking or ending a share stops the feed at once; a device still sending is told to stop.
- **Nothing kept by default.** The server holds a share's latest position only, and deletes it when the share ends.
  A trail is kept only when the owner chooses it for that share, at most `location@trail-max-days` (30) after it
  ends, and the owner can delete it at any time.
- **No other reader.** No grant reads another person's shares: not ADMIN, not a household organizer, not a group's
  owner. Monitor shows counts only. Positions never go to telemetry, logs or shared caches
  ([Maps §8](archive/Maps.md#8-access-publication-and-reliability)).
- **Approximate when chosen.** A share can be exact or approximate (rounded on the server to about 1 km, before it is
  stored or sent).

## 2. What it builds on

| Piece | Today |
| --- | --- |
| A one-shot location | The chat location message (`ChatLocationServices.xml` `send#Location`, `Chat.qvue:1086` `chat-location-card`), sealed in encrypted rooms; live location and Check In left "after Maps" ([Comms-Messaging](Comms-Messaging.md), the feature table and phase 20's "Left out") |
| Live push | `/notws` (`moqui-host` `notws.rs`): subscribe by topic, fan-out only to a message's recipient userIds (:827), rechecked sessions; `ec.makeNotificationMessage()` with `directOnly` and `persistOnSend(false)`, sent on commit |
| Sharing with expiry | `ResourceShare` (users, groups, link tokens by hash, `expireDate`, `revokedDate`); Mantle `ExternalAccessCapability` (a token shown once, scopes, thru date, revocation, failed tries), PASS's guest-link model |
| Devices | `UserLoginKey` (the apps' device keys, `deviceName`), `WebPushSubscription` (FCM, UnifiedPush, web), `ChatDeviceKey` (per-device curve25519 keys) |
| Android | Location foreground-only today (`AndroidManifest.xml:32`, "never in the background"); foreground services for calls and screen share; FCM and UnifiedPush sealed to account keys |
| XMPP | PEP with any node (`moqui-xmpp` `session/pep.rs`); the `geoloc` node is refused until this plan maps it (LL0) |
| Retention jobs | `clean#ChatRetention`, `clean#ChatChanges`, the telemetry day partitions |
| Maps | `m-map`'s Locate me (`WebrootMap.js:1409`, one position, never kept or sent) and its marker source; record layers by `MapAdapter` with bounded viewport reads |

## 3. Design

### 3.1 Model

In the framework (`entity/MapEntities.xml`, package `moqui.maps`):

- **`LocationShare`**: `shareId`, `ownerUserId`, the device it is sent from (`userLoginKeyId`, or none for a browser
  page), `label` (what the audience sees: the owner's name by default), `purposeText`, `fromDate`, `thruDate`
  (required), `revokedDate`, `precisionEnumId` (`LspExact`, `LspApproximate`), `keepTrail` and `trailKeepDays`, and for
  a guest share the record it is for (`recordAdapterKey`, `recordKey`, as `MapAdapter` names records).
- The device a share is sent from (`userLoginKeyId`) waits for LL3: no host member yet tells a script which login key
  signed the request (`ec.user.loginKeyKind` tells only its kind), so LL1 binds a share to its owner, not a device.
- **`LocationShareAudience`**: `shareId`, `audienceSeqId`, `audienceTypeEnumId` (`LsaUser`, `LsaUserGroup`, `LsaXmpp`,
  `LsaLink`), `userId`, `userGroupId`, `jid`, and for a link `tokenDigest`, `failedTries`, `lastViewedDate`.
- **`LocationShareAudienceMember`** (built in LL1 after review): a group entry's snapshot, the members the owner's act
  reached (less those the chat directory keeps the owner from) and the membership row that made each a member; a user
  counts only while that same row is current, so a membership written later by anyone, with any dates or stamp, reaches
  nobody until the owner adds the group again (a snapshot member's own row ended and reopened brings that same person
  back: accepted).
- **`LocationLatest`**: one row a share (`shareId`), `latitude`, `longitude`, `accuracy` (metres, as the device gives
  it), `heading`, `speed`, `observedDate`, `receivedDate`. Not cached.
- **`LocationTrailPoint`**: `shareId`, `observedDate`, the same values, indexed by share and date. A share's trail is
  read in bounded pages. Each point keeps the server's `receivedDate` (LL3 review), and a viewer sees only points
  received after they were reached.
- Settings on a new `location` element of `MoquiDefaultConf.xml` (each with its XSD attribute and `conf_schema.rs`
  entry): `max-share-hours` (24), `trail-max-days` (30), `min-interval-seconds` (5), `link-max-views-per-minute` (30).

### 3.2 Services

`org.moqui.maps.LocationServices` (`framework/service/org/moqui/maps/LocationServices.xml`, logic in
`LocationShares.js`; the loops in Rust only if a measurement says so):

- `create#Share` (audience, until, precision, trail): the caller is the owner; the end within `max-share-hours`; an
  audience of users the owner may message (the chat directory's rule), groups the owner belongs to, XMPP contacts of
  the owner's roster, or a link. Each audience user is notified ("X shares their location with you until 15:30"). A
  link's token is returned once.
- `update#Share` (extend, change audience), `stop#Share`, `revoke#Audience` (one person or the link), all the owner's
  only; each change notified to the owner's other devices and to the audience it concerns.
- `report#Position`: from the owner only, for an active share, from the share's device when it has one; values
  checked (ranges, accuracy, a date not in the future nor older than the latest); at most one a `min-interval-seconds`
  a share (later ones answer 429 and are dropped); approximate shares rounded here. It writes `LocationLatest` (and a
  trail point when kept) and sends `locationShare:<shareId>` to the audience's current userIds. A report for an ended
  or revoked share answers 410, which stops the device.
- `find#MyShares`, `find#SharedWithMe`, `get#Share` (the latest position, and the trail when kept, in pages): every read
  rechecks the audience (a group's current members, a user still in it); a viewer removed from a group stops seeing it
  at the next read and gets no more pushes.
- `clean#LocationShares` (an hourly `ServiceJob`): ends expired shares, deletes the latest row of every ended share,
  and the trail points past `trailKeepDays`; the owner's `delete#Trail` deletes them at once.
- Start, extend, stop and revoke are written to the audit log (who, when, the audience; never a position).

### 3.3 In Maps

A client sends a share's audience only for an explicit "add the group again": any `update#Share` naming an existing
group takes a new snapshot of it (LL1 review).

- The effective configuration carries `locationShares` (the viewer's own active shares and those shared with them).
  `m-map` draws them in a live layer: a marker with the label and the position's age, an accuracy circle, a stale style
  after a few minutes without a report, the trail when the share keeps one; it subscribes to each share's topic while it
  shows it.
- Browse gets **Shared with me** (each person, the age, Show on map, Directions in the phone's maps app by `geo:`) and
  **Share my location** (audience, until, exact or approximate, keep a trail), and **Who sees my location** (each share
  with its audience and end, Stop, Extend, Delete trail). An app-bar chip on every screen (the shell indicator hook) shows the user's own active shares and opens My location
  shares (`Maps/MyShares.xml`, granted to every signed-in user), where Stop and an hour more are, with Open in Maps
  for a Maps user.
- In a browser, a share sends while its page is open (`watchPosition`), with a banner on the page saying so; when the
  page closes the share stays active but goes stale, and the audience sees its age.
- The terminal lists shares with their ages and coordinates, and Stop.

### 3.4 The Android app

- `LocationShareService.kt`: a foreground service of type `location`, started only from the page's Share action while
  the app is in the foreground (so no `ACCESS_BACKGROUND_LOCATION`), with a permanent notification ("Sharing your
  location with Sam and 2 others until 15:30", with Stop) for as long as it runs. It reads Android's `LocationManager`
  (no Google Play services dependency), at an interval chosen from the share (a few seconds moving, longer still), and
  hands each fix to the host command `report_location`, which keeps only the newest unsent point and sends it; it stops
  at the share's end, on a 410, at sign-out or on Stop.
- The manifest adds `FOREGROUND_SERVICE_LOCATION`; its comment, [Mobile-Client](../docs/ui/Mobile-Client.md),
  [Privacy](../docs/Privacy.md) and the Play data-safety text say location is sent while a share the person started is
  on, and to whom.
- The desktop shares while its page is open, as a browser does.

### 3.5 In chat

Decided in LL4: live location only in unencrypted rooms (the phone's background sender cannot seal to a room's keys;
sealing per report is later); someone removed from the room by another, or blocked by the owner, loses that room's
live shares, while someone who leaves keeps them until the owner removes them.

"Share live location" beside the location message's Share: a share whose audience is the room's members (as users),
and a `content.location` message with its `liveShareId`. The card shows the live position and its age while the share
is on, and "Live location ended" after. In an encrypted room the server must not see the position: LL4 decides between
sealing each report to the room's `ChatDeviceKey`s (the server holding only ciphertext, and `LocationLatest` then
empty for that share) and offering live location only in unencrypted rooms, saying so in the picker. Check In ("tell
them when I arrive") stays later, built on this.

### 3.6 XMPP (XEP-0080)

Decided November 27, 2026 (the user's choice, after LL5's review): XMPP positions feed a share only when its owner
ticked "also use my XMPP app's location" for it (audited, raising the audience version), and go only to that share's
XMPP contacts, never to its users, groups or guest link.

The `geoloc` PEP node is refused today (LL0). LL5 maps it: a publish from the user's XMPP client updates the user's
active share whose audience includes XMPP contacts, and is refused with no such share (so a client's setting alone never
starts sharing); a share with XMPP audience entries sends each report as a `geoloc` notification to those JIDs only, and
an ended share an empty `geoloc` item (the XEP's "stopped"). Approximate shares are sent rounded, as everywhere.

### 3.7 Guest links for a record

- In the framework: the `LsaLink` audience and a public, token-only screen `Maps/SharedLocation.xml` (no session; the
  token in the URL fragment, posted by the page, so it is never in a server log or a `Referer`). It shows only that
  share's label, latest position and age, and its purpose text (an ETA a sender wrote), on the outline map or a basemap
  the installation marks public; it answers nothing else, at most `link-max-views-per-minute` reads, and refuses a token
  after its share ends or after too many failed tries. Its `.tui.json` is not needed (no terminal guest).
- In Mantle (`mantle.other.LocationShareServices`): `create#RecordGuestShare` for a service visit (a WorkEffort a
  technician is assigned to) or a delivery, checking that the caller may act on the record (Mantle's own checks) and
  that the share's end is no later than the record's planned end; an EECA ends the share when the record completes or
  is cancelled. The link goes to the record's contact by the record's own Comms (email or SMS template).
- In BOSS: "Share my location with the customer" on the technician's work order (Maintenance), and on a delivery once
  the warehouse trips (WH-TRIP) give a delivery its driver and status.

## 4. Phases

| Phase | Deliverable |
| --- | --- |
| LL0 — the geoloc gap | The XMPP PEP `geoloc` node refused, nothing stored or pushed (built first, with the Maps leftovers). Built November 26, 2026: refused in the XMPP session and in `XmppServices` (`get#PepNodeRefused`), a trimmed, case-insensitive name, `forbidden` on every path |
| LL1 — model and services | §3.1 and §3.2, the `location` conf element. Regression `framework/test/LocationShares.spec.js`: seen by its audience only; a group member removed loses it; an expired share shows nothing and its latest row is cleaned; a trail kept only when chosen, and purged; a non-owner's report refused; the rate; approximate rounding; no ADMIN read; the audit rows hold no position. Built November 26, 2026, after five review rounds: six entities (`LocationShareAudienceMember`, the group snapshot; `LocationShareOwner`, the owner's lock row; all in the audit class, so no generic tool, REST, agent or import reaches them; the agent `data` tool now refuses restricted entities), one fixed grid with hysteresis held per person, minute-rounded times and a 60 s interval for approximate shares, caps on active shares and audience, an ordered, time-bounded clean; operator routes that can still see a share are stated in Maps and Privacy (login-as, the SQL runner, `NotificationCapture`, the database) |
| LL2 — Maps | §3.3: the live layer, Browse's panels and dialog, the tray, the terminal list. A page test of the live layer (pushes moving a marker, stale, ended). Built November 27, 2026, after five review rounds: `locationShares` in the effective configuration only when asked (Browse and the terminal), m-map's live layer (positions stamped once at read, stale after 300 s, removed only by an ended or revoked push), Browse's Shared with me, Share my location, Who sees my location and Add the group again (the only resend of a group), browser sending with a banner per sending share, fresh fixes and a keep-alive, a failed read kept apart and said, the sending list in sessionStorage per account and session hash, no watch after close; the shell indicator hook (`moqui.shellIndicators`, `beforeLogoutHooks`, a conf-level `shell-indicator` provider) with the Sharing location chip opening My location shares (`Maps/MyShares.xml`, every signed-in user) |
| LL3 — Android | §3.4. Kotlin unit tests of the service's stop rules; the host command's tests in `moqui-client` (newest point only, a 410 stopping). Built November 27, 2026, after seven review rounds: `ec.user.loginKeyId` and a share bound to its device (`OTHER_DEVICE`, a takeover, a renewed key's 10-minute grace); `/qapps/reportClientLocation`, `stopClientLocation` and `clientLocationShare` for any signed-in owner; the app's own confirmation dialog as the control (the server's words, a weekday end, Send delayed and guarded from overlays and obscured touches), never the page's gesture; an `audienceVersion` the phone's reports carry, refused with `AUDIENCE_CHANGED` before anything is written, which pauses the share until the person confirms; a viewer sees only positions received after they were reached; `LocationShareService` (foreground `location`, its own thread, 15 s posts, coarse-only for approximate shares, quality by precision), durable stops through `LocationStopWorker`, the notification kept; revoking a device key stops its shares (force-new, never rolling the revoke back). Not run on a device yet (LL7) |
| LL4 — chat | §3.5 with the encrypted-room decision recorded here |
| LL5 — XMPP | §3.6. Tests in `moqui-xmpp` and the XMPP spec |
| LL6 — guest links | §3.7: the framework screen and link audience (regression: a token read, refused after the end, rate-limited, a wrong token counted), Mantle's record shares and EECA (regression in mr-mantle), BOSS's work order action |
| LL7 — finish | Acceptance `LocationShareWalk` (two browsers and the emulator: a share started, seen, extended, a member removed, stopped; the Android notification's Stop). Guides: Maps "Live location", Comms, XMPP, Privacy, Mobile-Client |
