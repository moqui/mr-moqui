# Shared devices

A shared device is a browser with no person signed in that may open one registered group of screens: a wall display
that plays dashboards, later a kiosk. An administrator registers it, enters a code on it once, and can revoke it. The
code is in `crates/moqui-core/src/user/device.rs` (the credential and the route gate),
`org.moqui.admin.DeviceServices` (`service/org/moqui/admin/Devices.js`) and the `/Device` page
(`webroot/screen/webroot/Device.xml`). The design is IronStone's CX-DEVICE (mr-boss `plan/design-crosscut.md` §1.5,
`plan/design-admin.md` §6 Devices); the threats are in [Attack-Surface](Attack-Surface.md).

## The parts

- **Route group** (`moqui.security.DeviceRouteGroup`): seed data of the app that owns the screens. It names an artifact
  group, which is the device's allow-list and grants nothing: no `ArtifactAuthz` needs to name it. A member of type
  `AT_XML_SCREEN` admits that screen's render, and with `inheritAuthz` `Y` also its transitions and the screens below it;
  a member of type `AT_XML_SCREEN_TRANS` (`<screen location>/<transition>`) admits that transition. The row also has the
  path a device opens once registered (`homePath`), the one request parameter a registration fixes (`parameterName`, a
  wall display's playlist) and the default idle timeout and lifetime.
- **Display account** (`UserAccount.displayAccount` `Y`): the account a device's requests run as. A device identifies no
  person, but what it shows is read as someone: the display account's groups and, in Mantle, its party's organizations
  decide that. Nothing signs a display account in: a password, a login or API key, an app password, an SSH key, an
  identity provider, a second factor, a reset password and `Login as` are all refused (`user/login.rs`: the password
  login and every login that ends in `internal_login`). The one exception is work a device's request queued (an async
  service call, a service's commit rules), which a worker runs as the account with no client attached. A display
  account may not be in a privileged group or hold a sealed permission (`user-facade` `authz`), may not have a password
  or an external identity, and must not be disabled or past its end date. The same check runs when a device is
  registered and on every request of the device, so a change holds from the device's next request.
- **Device** (`moqui.security.DeviceCredential`): its name, route group, fixed parameter, display account, site, the
  hashes below, who registered it and when, when and from which address it was last seen, its idle timeout and end
  date, and when it was revoked or stopped and why.

## Registering

`register#Device` (`ADMIN_ACCESS_MANAGE`) checks the route group, the display account (above) and that every group the
display account is in is one the registrar could assign by `AccessServices`' delegation rules (a registered bundle or a
custom group, its delegable permission, a privileged group only by a member, no sealed grant), so registering a device
gives nothing the registrar could not give a person. The framework knows no organization: a caller that scopes
identities checks the display account first (Mantle's `register#SharedDevice`).

The registration code is 20 letters and digits (about 99 bits) made by the framework, returned once and kept only as its
single-use hash. `register#Device` returns it; with `issueCode` false it makes none, and `issue#DeviceCode` makes it at
the moment it is shown (once, to the registrar, within fifteen minutes), so a screen carries no code from its write to
its render. The code is never logged, listed or written to the audit log.

## Redeeming

The device's browser opens `/Device` and posts the code (`redeem#Device`, with the page's session token). The code is
good once and for `codeMinutes` (15 by default, 5 to 60): the write that redeems it holds only while the registration is
unredeemed, so two posts at once make one device. Every refusal gets the same answer, is counted by the web protection
(which slows and blocks an address that guesses) and, when the code was a registration's, leaves a `deviceRefused` row
in the audit log. Redeeming signs out any user of that browser's session.

The response sets the cookie `moqui.device=<deviceId>.<secret>`: `HttpOnly`, `SameSite=Strict`, `Path=/`, `Secure` on
https, lasting until the registration's end date. The secret is 43 random URL-safe characters; only its SHA-256 is kept.
No script, service or log sees it.

## A device's request

A request with a device cookie is the device's, whatever else it carries (`screen::handle`, and a screen test's render
with the cookie among its headers):

1. The device row is read (no cache) and the secret compared. An unknown device or secret is refused and counted as an
   authentication failure. Then: revoked, stopped, past its end date, not seen for longer than its idle timeout, an
   address outside `ipAllowed`, a display account that fails its check, a missing or disabled route group.
2. The display account becomes the user of that request only. It is held in the execution context and never written to
   the web session: the session row, the live session and the session cookie stay those of a browser with nobody signed
   in. So nothing that reads a session's user (the notification socket, the terminal socket, the Assistant's event
   stream) ever sees a display account, there is no signed-in session of it to list, steal or end, and a revocation
   needs no session to be found on another server.
3. `api_key` and `login_key` headers, Basic credentials and `authUsername` are not read, and no login succeeds during the
   request.
4. The route gate (`check_route`, before anything of the target runs) refuses every screen, transition, REST and service
   path, token route and script servlet that the route group does not admit, with 403. Admitted: the route group's members, `/Device`
   itself (it says what the browser is registered as), and static files that need no sign-in. What is admitted is then
   authorized as the display account in the usual way, so a route group does not widen what that account may read.
5. The registration's fixed parameter replaces whatever the request sent under that name.
6. `lastSeenDate`, the address and the user agent are written at most once a minute.

A refused device gets 403; when the refusal is final (unknown, revoked, stopped) the response also removes the cookie, so
the browser is an ordinary one again. A browser's page (an `Accept` of `text/html`: a wall display, which has no one to read
a JSON error) gets a plain page instead of the JSON error: one sentence (not registered, revoked, ended, refused for now, or
for a request the route group does not admit or that failed, "This page is not part of this display" or "cannot show this
page now") and a link to `/Device`, with nothing of the instance (no host, title, theme, logo, script, style sheet, route
group, device or error text) and `Cache-Control: private, no-store` (`user::device::refusal_page`, `screen::handle` and
`error_page`). An API client keeps the JSON error.

A page a device plays should reload itself when one of its own requests is refused (401 or 403), so that the server's
plain page replaces it: the report playlist does this in kiosk mode (`m-report-playlist`'s `kiosk`, `WebrootReport.js`), and
Insights' wall display (mr-boss `screen/Insights/WallDisplay.xml`) uses it.

## One browser

Nothing binds a cookie to hardware, so "tied to one browser" means this, and no more:

- The secret exists only in the cookie of the browser that redeemed the code, is not readable by script and is sent
  only on same-site requests over https.
- The secret is replaced once a day by the first request that presents the current one and whose answer can carry the
  new cookie: any answer the server builds whole, a refusal or an error included, but not one already streaming nor
  one a shared cache may keep (`Cache-Control: public`); otherwise the next request does it. The secret before it works for
  ten more minutes (requests under way). After that, a request presenting it can only come from a copy: the device is
  stopped (`stopReason` `copied`), for the copy and the original alike, with a `deviceStopped` audit row, and is
  registered again. A copy taken and used within one day of the copying is not detected by this; the address and client
  shown in the device list, and `ipAllowed`, are what limit it.
- `ipAllowed` holds a device to an address pattern, as `UserAccount.ipAllowed`, against the client address the server
  trusts (`client_ip`, the configured proxy header); a request with no known address is refused.
- If the browser loses the response that carried a new secret, it keeps the old one and is stopped after the grace: a
  device that stops this way is registered again.

## Ending

- `revoke#Device` (`ADMIN_ACCESS_MANAGE`): the next request of the device is refused, on every server.
- The idle timeout (`idleTimeoutMinutes`, the route group's default or a week; 5 minutes to 30 days): a device not seen
  for longer stops. A wall display asks for data all day, so it is never idle while it is on; the timeout ends the
  credential of a display that was unplugged, lost or replaced.
- The end date (`expireDate`, 365 days by default, at most 730): then it is registered again.
- A display account that is disabled, loses what a device needs or gains what a device may not have: its devices are
  refused until that is put right (not stopped for good).

## Evidence

What happens to a device is written to the audit log on its display account (`UserAccount`, the device as the
secondary key): `deviceRegistered` (and the showing of its code), `deviceRedeemed`, `deviceRevoked`, `deviceStopped`
and `deviceRefused` (a code presented again, expired or revoked; a request refused for its address or its account, at
most every ten minutes). An owner that knows organizations registers the audit projections for them (Mantle's
`PartyAuditData.xml`).

## Tests

- `cargo test -p moqui-core --lib user::device`: the code once, nothing stored in plain text, the request-only
  identity, the daily replacement and the copy, the idle stop, the sign-ins a display account never passes, the account
  and address refusals.
- `./moq test framework --spec DeviceCredential`: registering and its refusals, the code through `/Device`, the route
  gate, revocation.
- `./moq test-client framework --serve --spec SecurityDevice`: the same over HTTP, with the plain page of a refused
  display and of a page outside its route group.
