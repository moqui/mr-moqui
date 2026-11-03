# Mobile-Desktop exit checks

The checks on installed apps that close MD1, MD2, MD3 and MD4 of [Mobile-Desktop](Mobile-Desktop.md) (§8, "Exit evidence"). They
are run by hand on real machines, not in the test suites: several take a sleep, a second machine or minutes of
waiting. Each says what to set up, what to do, and what to record. Record the results in §8 of the plan (a line per
check: date, OS and version, app version, pass or what failed), and file what fails as a fix.

Build the bundles with `./moq desktop build` (signed and with updates when their variables are set,
[Build-Test](../docs/Build-Test.md#desktop-app)) and install them as a user would: the `.dmg` on macOS, the `.deb` or the
AppImage on Linux. A development run (`./moq desktop`) is not an installed app: it uses the repository's runtime and
never opens the DevTools protocol in a bundle.

Where a check names the app's config directory, it is `~/Library/Application Support/org.moqui.desktop` on macOS and
`~/.config/org.moqui.desktop` on Linux; the runtime is under the app data directory
(`~/Library/Application Support/org.moqui.desktop/runtime`, `~/.local/share/org.moqui.desktop/runtime`).

## Setup

- A clean machine or user account for each OS: macOS (the version of MD0's matrix) and Linux (Ubuntu 26.04, GNOME).
- An edge node (`node_role=edge`) and an all-in-one node (`node_role=all-in-one`) reachable from the internet, each with
  a name for the local node's web access, mail (MX) and, for the call check, calls and TURN set up
  ([Edge-Node](Edge-Node.md)).
- A remote server with BOSS for the client checks, with a user who has a second factor, and one identity provider for
  SSO.
- A second device (a phone or another computer) with a browser.

## MD1 and MD2: the node on this computer

The node runs in process with no port by default (MD2); switch to the sidecar from the tray (the node item) for the
checks marked both, and run them in each mode.

1. **First run** (both). Install and open the app on the clean machine.
   - The splash page shows, then the node's login page asks for the initial admin. Create it.
   - The attach wizard opens once (`/qapps/system/EdgeNode/Pair`).
   - Record: the time from opening to the login page (cold start), and that the wizard opened.
2. **No port** (in process). With the app running, `lsof -nP -iTCP -sTCP:LISTEN` (macOS) or `ss -ltnp` (Linux).
   - Record: no HTTP or cluster port of the app; only listeners the runtime's properties ask for (SSH, SMTP, calls).
   - Turn on "Open ports for other devices" in the tray: after the restart the node also answers on its loopback port
     (`node-port` in the config directory), and the window stays on the app scheme. Turn it off again.
3. **Pairing with an edge node** (both). Pair through the wizard with the edge node's token.
   - From the second device, open the local node's web access name on the edge node and sign in.
   - Record: the pairing succeeded, the site answers through the edge, and the edge's status page shows the link up.
4. **Pairing with an all-in-one node** (both). Unpair, then pair with the all-in-one node; as in 3.
   - Also send a mail to an address of the local node and one of the server's: each lands with its owner.
5. **Mail across a sleep** (both). With the app paired and running, put the computer to sleep for at least ten minutes.
   From elsewhere, send three numbered mails to the local node's address during the sleep.
   - The edge node's offline page shows for the local node's web access while it sleeps.
   - Wake the computer. Record: the link comes back by itself (the app's log: the wake and `/health` ready), and the
     three mails arrive in order, once each.
6. **A call with media on the edge** (both). From the second device, call a user of the local node through the edge;
   answer in the app.
   - The first call asks for the microphone (macOS asks once, for the app).
   - Record: audio both ways, an encrypted call where both ends support it (the call's lock), and that the media went
     through the edge node (its call status). On Linux, the call runs in the app's window (CEF); record it the same.
7. **Mail and call links** (both). "Make default for email and calls" in the tray; click a `mailto:` link and a `tel:`
   link in another app.
   - Record: Compose opens filled in, and the Dialer with the number, in the Comms app of the node (BOSS's Comms when
     BOSS is installed, also with the sidecar).
8. **A large download** (both). Export a large report or download an attachment over 16 MB.
   - The save dialog asks where; a notification says when it is saved. Record: the file is whole, and the app's memory
     (Activity Monitor or `ps`) did not grow by the file's size while it downloaded.
9. **PDF** (in process). Print a document to PDF (a screen's PDF button, or `/fop/...`) on a machine with no Chromium
   installed and none configured.
   - Record: the PDF opens and looks as on a server; the app's log says "PDF on the app's own Chromium (CEF)".
   - Check the bundle's `notices/chromium-credits.html` is in the app's resources.
10. **Backup and restore** (both). Back up now from the tray; change a record; Restore from that backup.
    - Record: a `-before-restore` dump is written first, the node comes back with the record as it was, and the window
      signs in again.
    - Leave the app running overnight with the nightly backup on: record that a dump is in the backups folder.
11. **An upgrade that keeps the data** (both). Install a newer build over the installed one (or take an update from
    the feed, with updates on).
    - Record: the app restarts, the window is still signed in (the node's cookie in `node-cookies`, in process), and
      the data and the pairing are as before. Then upgrade an install in sidecar mode to one in process: the same data
      directory, no new initial admin.
12. **Quit and crash** (both). Quit from the tray: no `moqui`, `postgres` or app process is left. Then end the app
    with `kill -9`, and open it again.
    - Record: the next start takes over the orphaned Postgres (its log says so) and stops it on quit.
13. **Startup and memory** (both). With the node paired and idle for five minutes, record the resident memory of the
    app, its helpers, `moqui` (sidecar) and Postgres together; then after ten minutes of use (mail, chat, a report, a
    PDF) again. Record the cold start of check 1 and a warm start (quit and open again). Compare the two modes, and
    the web client in a browser against the same node.
14. **Linux** (both). Run checks 1 to 13 on Linux, from the `.deb` and from the AppImage. Also: the tray, the
    notifications, Start at login (an XDG autostart entry), and the backup dialogs (the desktop portal, or zenity or
    kdialog).

## MD3: the client of a remote server

Switch to the client from the tray ("Use as a client of a server").

1. **No network.** Sign in to the remote server, then turn off the network and quit and open the app.
   - Record: the shell opens with its last apps and counts, marked offline; it says so in the bar.
   - Turn the network on: the shell reloads with the server's data once its socket opens.
2. **Sign-in with a second factor**, and **SSO** through the identity provider in the system browser.
   - Record: both end signed in; the device list (the account menu's Apps and devices, and BOSS's Account settings)
     shows this computer once for each.
3. **Revocation.** From BOSS on another device, sign this computer out (revoke its device).
   - Record: within a minute the socket closes and the shell shows the sign-in; the key is gone from the key store.
4. **Key renewal.** On a test server, set `login-key/@device-expire-hours` short (say 3 hours) and sign in; wait past
   two-thirds of it with the app running (the renewal looks every six hours and at start: quit and open the app to
   check sooner).
   - Record: the device list shows the same device with a new expiry, the app is still signed in, and its sockets
     opened again; the old key is refused ten minutes later.
5. **An older or newer server.** Against a server whose `clientConfig` has a newer `server.clientApi` than the app
   knows, the shell says to update the app.
6. **Account switching.** Add a second account on another server; switch between them.
   - Record: each shows its own apps, windows and sockets close on a switch, and a page's browser storage is the
     account's own (a chat draft in one does not show in the other).
7. **Links, downloads, PDF.** In an app window, follow a link to another site (it opens in the system browser), download
   a large attachment (as check 8 above), and open a PDF.
8. **Capability boundaries.** In an app window (a server page), from the DevTools of a development run only, check that
   `window.moquiDesktop` has only `socket` and `storageScope`, and that calling another command (`client_state`) is
   refused. In the shell, check that a server page or script cannot load on the shell's origin (it opens in an app
   window).
9. **Camera and microphone.** Start a call from the main window (a room's Call, the Dialer): it is granted (and the OS
   asks once); a page of another origin is never shown in the app, so there is nothing else to grant.
10. **Calls in the main window.** The main window owns the account's calls.
    - From a browser, call this user while the app is hidden to the tray, and again while it shows but another app has
      the focus. Record: a native notification each time (a click shows the call's screen in the main window), and the
      ringtone heard without first clicking in the window (CEF's `--autoplay-policy=no-user-gesture-required`).
    - Answer an end-to-end encrypted call there, from a browser on another computer. Record: audio and video both ways,
      the call's lock, and that the frames decrypt (no "frames do not decrypt" on the roster; the SFrame worker runs
      under the shell's CSP with no CSP error in the DevTools console of a development run).
    - In an app window, open that call's screen (Calls, History): it offers Open in the app's Calls and joins nothing;
      a call started there (a plain one) is joined in the main window; its ring bar does not ring aloud.
    - `tel:` link with the Dialer already shown: the second number fills it.
    - Turn the network off: a call's screen and the Dialer say they need a connection; on again, they come back.
    - Switch account or sign out in a call: it asks first, and the other people see this user leave.

## MD4: offline data and bundled Comms

In the client, against the remote server with BOSS. A personal account is one signed in with "This is my device", a
shared one with "Shared device" (the build's `<client-conf device-mode>` is `choose`). The account's store is
`client/data/<account>.db` in the app's config directory.

1. **Personal and shared sign-in.** Sign in to one account as personal and another (another user or server) as shared.
   - Record: the sign-in asked for the mode; `accounts.json` shows each account's `deviceMode`; each has its store.
   - Sign in to the personal account again choosing shared: it stays personal and the shell says why.
2. **Offline restart.** With the personal account, open a few rooms (one encrypted), a mail folder and a thread, and
   start a chat draft and a Compose draft. Turn the network off (or stop the server), quit and open the app.
   - Record: the shell opens offline; Chat shows the rooms and their messages to where they were read or mirrored, then
     the end of kept history; the encrypted room's messages decrypt; the Inbox shows the folder and the thread; both
     drafts are there.
3. **Queued sends.** Still offline: send a chat message, one with an attachment, mark a room read, send a mail with an
   attachment, and archive a thread. Quit and open the app again, still offline.
   - Record: each shows as queued (the Inbox's Outbox for the mail); the room shows read and the thread archived at once.
   - Turn the network on. Record: each is sent once (the server's room and Sent folder, and a browser of the other user,
     show one message each, the attachments once); the queued rows go.
4. **A reopened draft.** Online, save a Compose draft; go offline, reopen it from Drafts, change it and send.
   - Record: it is queued; once online the one draft is sent, not a second copy (one message in Sent, none left in
     Drafts).
5. **Discarded.** Offline, queue a chat message and a mail, and Discard each from its row; go online. Record: nothing
   is sent, and the store's files of the mail's attachment are gone.
6. **Sign-out by mode.** Sign the shared account out with an item still queued.
   - Record: the shell lists the unsent item and asks first; after it, the account's entry, its store file and its
     files are gone, and signing in again shows no kept data (an app window's browser storage of it is cleared too).
   - Sign the personal account out: its store is still there; signed in again and then offline, it reads as before.
   - Revoke the shared account's device from BOSS on another device: its data is deleted as on sign-out; revoke the
     personal one: its data is kept until the account is removed from the account menu.
7. **The outbox moved from an MD3 build.** Install an MD3 build, sign in, go offline and queue two chat messages and a
   chat draft (kept in the page's `localStorage`). Install the MD4 build over it and open it online.
   - Record: both messages are sent once and the draft is in Chat; the browser storage no longer holds them.
8. **Encrypted room offline.** Restart the app offline after reading an encrypted room. Record: its messages decrypt
   (the store key from the key store); in an app window an encrypted room offers Open in Chat, and the device list
   shows one device for this computer, "MR Moqui desktop on <host>".
9. **Mirror bounds.** On a test build set `mirror-days` short (say 2), `prefetch-rooms` 3 and `prefetch-mail` 10.
   - Record: a personal account's store holds rooms and mail back about two days and no further (older pages need the
     server); a shared account's the three latest rooms and the inbox's ten latest threads.
10. **Store cap.** On a test build set `store-max-mb` small (say 5) and read rooms and mail past it, with a queued send
    and a draft.
    - Record: the store file stays near the cap, the least recently read rooms and threads go first, and the queued
      send and the draft are kept and sent.
