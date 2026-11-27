# Privacy in the MR Moqui apps

What the MR Moqui desktop and mobile apps keep, and what they send where. The apps are clients of an MR Moqui server
that you, or your organization, choose and sign in to: what that server keeps and does is its operator's, under their
own policy. The apps send nothing to the project that makes them, and they have no analytics or crash reporting of
their own.

## Both apps

- **Your server.** Everything you do in the apps goes to the server of the account you use, over TLS (HTTPS), and
  nowhere else of the apps' own choosing: the pages you open, the messages, mail, files and calls you make, and the
  name of this device in your device list.
- **On this device.** Each account's list entry, and its device key (the credential that signs the app in, in place of
  your password), sealed by the system's key store. A store of each account's mail, chat, drafts and outbox, for use
  with no network, in the app's own files. Signing out of a shared account removes its store; a personal account keeps
  its store until you remove the account, or sign out and choose to delete it.
- **Offline maps.** A map region you download (an offline map package, built on your server from what your account
  could see then: the basemap tiles, your places, the overlays you could read, and the region's postal codes, Overture
  places and addresses) is kept in the app's own files, readable only by the app, until you delete it in the app; a
  newer build of it replaces it. It stays on the device after the server revokes your access to what is in it. Your
  server records which device downloaded which build of it. Browsing and searching it sends nothing; a place you save
  while offline waits on the device and goes to your server when it can be reached. A shared account's packages go
  when it signs out; a personal account's stay until you delete them or remove the account.
- **The device's name.** The app names this device in your device list and as your chat encryption device, which the
  people who share a room or a call with you see: on the phone its model, on the desktop "MR Moqui desktop on" its
  operating system. You can rename it.
- **Calls.** A call's audio and video go to your server and the other people in the call, only while it lasts.
- **Location.** Only when you share a place in a chat (the place you choose, sent to your server as a message), and while a
  live share you started in Maps has its page open in the app or a browser: that page's position, at most every few
  seconds (every minute for an approximate share), sent to your server for that share. Closing the page stops it.
- **Live location on the server.** While a share you started lasts (at most a day unless you extend it), your server
  keeps only its latest position, read by no one once the share ends and deleted then (when you stop it) or up to an
  hour later (when its time runs out), and a trail only when you choose one for that share, for at most 30 days after it
  ends (`location@trail-max-days`) or until you delete it. The server's operator is not kept out: an administrator who
  signs in as you, a holder of the Tools SQL runner, any server script that turns on the `NotificationCapture` tool
  (which sees the live positions as they are sent), the database's own tools and its backups can see what the server
  holds, and its audit history keeps who you shared with and until when, never where you were.
- **Photos and videos.** Only those you take or pick to send, sent to your server; a photo or video taken in the app
  is kept in the app's cache, not in your gallery, and cleared at its next start.
- **Logs.** The apps log what they do on this device (the phone's system log, the desktop's log files); a failed
  request's line names its server, not the address.

## The mobile app (Android)

- **Permissions**, each with a reason: the microphone and notifications at the first start after you sign in (to ring
  and answer a call on a locked phone), the camera when you first take a photo or a video, your contacts when you first
  search them in the dialer (read on the phone, never sent), and your location when you first share a place. Calls use Android's own calling support; the app may ask to run unrestricted
  in the background so an incoming call rings.
- **Push.** A build with Firebase gets a token from Google's Firebase Cloud Messaging once an account is signed in, and
  sends it to your server; with a UnifiedPush distributor app (such as ntfy), that app gets an endpoint for each
  account under a random name. Either way the messages are sealed to the account's own keys on this phone, so neither
  Google nor the distributor can read them.
- **Android's WebView** shows the server's pages. It sends Google no usage metrics of this app. Its Safe Browsing
  check, which Android keeps on, compares the addresses the pages load against a list on the phone that Google updates.
- **No backup.** The app's data is not in Android's cloud backup. A phone with no screen lock lets whoever holds it
  open your accounts in the app, which tells you so, and a shared-device sign-in needs one. The passphrases of your
  chats' offline store open only while the phone is unlocked.

## The desktop app

- **The node.** Run as a node, the app is itself an MR Moqui server, for your own use: all its data is in the app's
  data folder on this computer. Each night it backs the whole database up, encrypted with a backup key kept in the
  system's key store, to a folder (by default `MR-Moqui Backups` in your Documents, which your own backup or a cloud
  service may copy); it tells you so the first time, and the tray's Backups menu chooses the folder, turns the nightly
  backup off, and saves the backup key to a file, which restoring on another computer needs. Backups taken before the
  key are not encrypted.
- **As a client** of a server, it keeps what both apps keep (above), its device keys in the system's key store (the
  macOS Keychain, the Windows Credential Manager, the Secret Service on Linux) or, where there is none, in a file only
  you can read.
- **Chromium**, which shows the pages, reaches Google for two things: its component updater, which keeps the
  certificate revocation and Certificate Transparency lists current, and a spelling dictionary the first time a
  language is checked. Its Safe Browsing is off.
- **Updates**, where a build has an update feed: the app asks that feed for a newer version when you check, and
  installs only one signed by the project's key.

## Removing your data

Signing out revokes the device key on your server, or as soon as the server can be reached. Removing an account from
the app, or signing out with "Also delete the mail, chat and drafts kept for it", deletes its store, its offline map
packages and its keys on this device. Uninstalling the app removes the rest of its data on this device,
but not the desktop node's backups in a folder of yours. What your server keeps, ask its operator.
