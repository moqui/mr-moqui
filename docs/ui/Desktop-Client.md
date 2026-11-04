# The desktop client

The desktop app's `client` mode: a bundled shell that is a client of a remote MR Moqui server, with no node on this
computer (configuration 3 of [Mobile-Desktop](../../plans/Mobile-Desktop.md), MD3 and MD4). Each account is a server and a
user with a device key the host keeps. The shell opens with no network, keeps each account's data in a store on this
computer, answers from it offline, and sends what was queued once the server can be reached.

- **Host:** `crates/moqui-desktop/src/client/` (the Tauri host: the schemes' hooks, the windows, the OS key store
  and the conf's reading) and `shell/` ([moqui-desktop](../../crates/moqui-desktop/README.md), which has how to run
  and test it), on [moqui-client-tauri](../../crates/moqui-client-tauri/README.md) (the shell's commands, the outbox
  and drafts commands and the schemes' answer, shared with the mobile app) and
  [moqui-client](../../crates/moqui-client/README.md) (accounts, keys, sign-in, the proxy, sockets, the store, the
  conf, and the client's core: its state behind a host trait, the offline answers, the pushes, the outbox and its
  sender, the mirror, the renewal and the device modes), which has no Tauri or framework and builds for Android. The
  same client on Android is [Mobile-Client](Mobile-Client.md).
- **Pages:** the webroot's own scripts and BasicComms' `.qvue` screens; what a page sees of the host
  (`window.moquiDesktop`, `moqui.drafts`, `moqui.connection`) is in
  [Qvt-Client, In the desktop app](Qvt-Client.md#in-the-desktop-app).
- **Server:** nothing of its own but `/qapps/clientConfig` (with `server.clientApi`, 4 since MD5), the device keys
  (`createLoginKey` with `deviceName`, `renewClientDevice`, `/qapps/clientDevices`), the mobile app's FCM registration
  (`/qapps/registerClientPush`, [Qvt-Client, Native apps](Qvt-Client.md#install-and-web-push)), the SSO app
  hand-back, Compose's `clientRef` ([Comms](../tools/Comms.md)) and the sync contracts of MD5 ([below](#the-servers-sync-contracts)).

## Windows and schemes

Two origins, each an app scheme of the host:

- **The shell**, `moqui-shell` (`http://moqui-shell.localhost`), in the main window. It serves the shell's files
  (`shell/`, and the webroot's `js/`, `libs/` and `css/` named in `shell/libs.txt`; a bundle's staged copy in
  `shell-assets/`), and nothing else under its own paths (a 404, never forwarded). Its CSP has no `unsafe-eval`: only
  `wasm-unsafe-eval`, for the crypto device's module, and one worker, an encrypted call's staged SFrame transform,
  answered with its own policy (`moqui_client::assets`). It forwards the server's data to the current account's server:
  its JSON, images, media and downloads, never its pages, frames or scripts. Its query parameters are read only on a
  load the host started.
- **App windows**, `moqui-app` (`http://moqui-app.localhost`, windows `remote-<n>`), show the server's own `/qapps`;
  the scheme forwards everything.

Each request carries the key in the `login_key` header; the pages hold no credential and send no cookie. The host's own
502, when the server is not reached, is marked `X-Moqui-Offline: unreachable`; an answer from the store is marked
`X-Moqui-Offline: 1`. How the schemes work around Chromium's handling of their HTML is in
[Mobile-Desktop](../../plans/Mobile-Desktop.md#one-transport-seam).

**Commands.** The main window has the shell's commands, the page bridge's, the sockets, the outbox and the drafts; an
app window has the sockets, the outbox and the drafts of its own account, and `open_in_shell`. Tauri counts the app's
scheme pages as local, so the capabilities name the windows. Each window's navigation stays on its origin: the web
opens in the system browser, a server page from the shell in an app window.

**Sockets.** A page's `/notws` or `/callws` (`moquiDesktop.socket`, which `moqui.openSocket` uses) is held by the host
with the key, and its frames are handed to the page.

**Downloads.** An attachment or a large file is saved by the host, streamed, as for the node in process; a small one is
Chromium's. An event stream is answered 503.

## Accounts, keys and sign-in

- **Accounts:** `client/accounts.json` in the app's config directory. Switching closes the other account's windows and
  sockets. The pages' browser storage of a server's data is under the account (`moquiDesktop.storageScope`, MoquiLib
  `scopedKey`).
- **Sign-in:** the shell's Sign in (`shell/screens/SignIn.qvue`) has the host post the password once to the server's
  REST login with `createLoginKey` and a `deviceName`, with the second factor's code when it asks.
- **Single sign-on:** the sign-in's "Single sign-on" lists the server's identity providers; one opens the system browser
  at the server's `/sso/appLogin`, where the user signs in and lets the app in. The server hands the browser back to the
  app's own scheme, `org.moqui.desktop:/sso`, with a code the app takes with its PKCE verifier for a device key. The
  scheme is registered with the OS as the app's (beside `mailto:` and the others; on Linux by an AppImage or development
  run when it starts a sign-in; on macOS by the app's bundle, checked on the installed app only) and is not claimed by
  "Make default". A hand-back the app is not waiting for is passed over.
- **Sign-out and revocation:** Sign out revokes the key (`/rest/logout` with `revokeLoginKey`). A 401, or `/notws`
  closing for authentication, has the host check the key and, if it no longer signs in, forget it and show the
  sign-in. What is kept then depends on the device mode (below).
- **Renewal:** at start and every six hours, a key with less than a third of its life left is renewed
  (`moqui_client::renewal`, `/qapps/renewClientDevice`), and its pages' sockets open again with the new one; the server
  ends the old key ten minutes later. The shell says to update the app when the server's `clientApi` is newer than it
  knows.
- **Apps and devices:** the account menu lists the account's apps and devices signed in by a key of their own
  (`/qapps/clientDevices`), each to sign out; so does My Account's Account page in BOSS.
- **Key store:** the macOS Keychain or the Secret Service (`keyring`), else a file in `client/keys/` that only the user
  can read, which the shell's account menu says. A development run uses the file unless `MOQUI_DESKTOP_KEY_STORE=os`
  (each build is a new unsigned binary the Keychain would ask about).

## Device modes and client-conf

Whose device this is decides how much is kept and for how long. A build sets it in its runtime conf, read by the host
without booting a node (`client/conf.rs`: `MoquiDefaultConf.xml`, each component's `MoquiConf.xml`, then the runtime
conf, merged as the framework would); the server does not read it.

```xml
<client-conf device-mode="choose" mirror-days="90" prefetch-rooms="20" prefetch-mail="100" store-max-mb="512"/>
```

| Attribute | Default | What |
|---|---|---|
| `device-mode` | `choose` | `personal` (a person's own computer or phone), `shared` (a hot desk, a packing terminal, a point of sale), or `choose`, where the sign-in asks |
| `mirror-days` | 90 | how far back a personal device mirrors chat rooms and mail folders, in days |
| `prefetch-rooms` | 20 | how many of the latest rooms a shared device reads ahead (each room's latest page); 0 for none |
| `prefetch-mail` | 100 | how many of the inbox's latest threads a shared device reads ahead; 0 for none |
| `store-max-mb` | 512 | the size cap of an account's store; past it the least recently read cached data goes, never unsent items or drafts |

A bad value is logged and its default used; a runtime with no framework conf, or one that does not parse, gives the
defaults. The element is in `moqui-conf-3.xsd`; the parsing is `moqui_client::conf`.

- **Kept with the account.** The mode is set when the account is created here, from the build's mode or the sign-in's
  choice (password and SSO alike). An account already personal stays personal when shared is chosen: nothing is
  trimmed, and the shell says the account already has a personal device's mirrored data. An account with none follows
  the build's mode.
- **Shared:** sign-out or a revocation deletes the account's store, its chat store keys and its entry in
  `accounts.json`, and the pages clear its scoped browser storage and IndexedDB (the shell as it loads, the next app
  window once). Before a shared sign-out the shell lists what is unsent and asks. The next sign-in creates the account
  again with whatever mode is chosen then.
- **Personal:** sign-out and revocation keep everything; removing the account deletes it.
- **Removing an account** always deletes its data, going on past a failed save (`moqui_client::modes`).

## The store and offline answers

Each account's data is in its store (`moqui_client::store`, `client/data/<account>.db`, one SQLite file, owner-only,
WAL, migrated by `user_version`), opened only while the account is listed (`moqui_client::stores`), with the cap of
`store-max-mb` (the cached reads go least recently read first, and always keep a quarter of the cap). The answers MD3 kept in `client/cache/` move into it once. It is plain SQLite: the OS's disk encryption
protects it at rest, and the keys are in the key store, not in it.

- **Kept:** the shell's `/qapps/clientConfig`, `/shellData` and `/shellCounts`, and the comms app's Chat (`chatData`,
  `rooms`, `roomView`, `room`, `timeline`), Mail (`inboxData`, `threads`, `thread`) and Compose (`composeData`) 200 JSON
  answers, keyed by path and sorted query less `_`, `moquiSessionToken` and `pageTraceId` (`moqui_client::offline`).
- **Rows:** besides the answers, the store keeps structured rows: timeline pages with the `eventSeq` ranges held whole
  per room and thread, rooms, mail folders, a folder's thread pages by cursor (BasicComms' one store, or mr-mantle's
  several) and threads.
- **Offline:** when the server cannot be reached the same transitions are answered from the store, marked
  `X-Moqui-Offline: 1`. Timeline pages, a room view, the room list, a folder's thread pages and a thread are built from
  the rows in the server's shape, so a page never asked for whole still answers and the pushes show. A page in a gap
  (never fetched) needs the server (502); `offline.gapBefore` and `gapAfter` say where the held stretch ends, and
  `hasMoreBefore`/`hasMoreAfter` stay false there.
- **Compose offline:** `composeData` answers from the store in `get#ComposeData`'s shape. The new message's answer (no
  parameters, which the mirror reads) gives the identities, signatures and preferences; with none kept, those of any
  other kept `composeData` do.
  - A reply or forward is the kept answer of the same query (not a forward's: its draft was the server's then), else
    built from the kept message: its identity, recipients, "Re:"/"Fwd:" and the quote, cleaned as the server cleans it.
    There is no Reply-To. A forward's files are left out, as only the server copies them into its draft, and
    `offline.forwardFilesMissing` says how many. A message not kept gives a new message answering it, with
    `offline.messageMissing`.
  - A draft follows its kept message: one not a draft any more (sent or deleted) needs the server. A kept draft is read
    from its row, the rest from its kept answer or the draft Compose kept here; with no message kept, the kept answer,
    else that draft.
  - Mailto-style values fill the new message. An internal reply or a mailbox post needs the server.
  - With nothing kept, Compose starts a blank email in the desktop client, with Send off until a mail account is known;
    a draft says it needs a connection. The web shows its error as before.
- **The chat store passphrase** (`/ChatKeys/storeKey`) is kept in the key store, not the store, and answered offline to
  the shell only; a revoked device's is dropped, and it goes with the account.

**Pushes.** The chat pushes on the account's `/notws` (`chatRoom:*`, `chatUser:*`) are written into the store as
`get#Timeline` shows them (`moqui_client::sync::push`): edits, reactions and redactions folded into their message,
encrypted rooms too, the same push from two sockets once. A drop marks the timelines' heads stale. With a server at
`clientApi` 4 a push's number in the changes feeds moves the store's cursor, or has the feeds read
([below](#the-clients-sync)).

**The mirror.** When the shell's `/notws` opens, `moqui_client::sync::mirror` reads ahead on a thread of its own: a
personal device all rooms and mail folders back to `mirror-days`, a shared one the `prefetch-rooms` latest rooms and the
inbox's first `prefetch-mail` threads. Both read the new message's `composeData`, so mail can be written offline. A run
is bounded, cancelled on a switch or sign-out, and stops on a 401 or when offline. After a drop, each room's latest page
is read again; with a server at `clientApi` 4 the run reads the changes feeds first, and after a drop those alone,
which read again only the rooms with something new.

## The outbox and drafts

What a page sends offline is kept in the account's store and sent by the host (`moqui_client::sync::outbox` and
`flush`; the commands in `moqui_client_tauri::outbox`), on a thread of its own for every account with a key here, so it goes
while the window is hidden.

| Operation | What |
|---|---|
| `chat-send` | a chat message (a plain `/apps` or `/qapps` Chat send path), done by its answer or its pushed echo's `clientRef` |
| `chat-read` | a mark-read, coalesced per room; the room shows read in the stored rows at once |
| `mail-send` | a compose: the draft saved, its files uploaded and the mail sent, each once by its `clientRef` (by id for a draft whose ref is another's) |
| `mail-state` | a thread's read, flag, archive, move or trash, as absolute values, in order; shown in the stored rows at once; with `seen` at `clientApi` 4 |
| `chat-edit`, `chat-redact`, `chat-react`, `poll-vote` | a message's edit, redaction, reaction or poll vote, once by its `clientRef`, the four of one message in order (`clientApi` 4) |
| `internal-send` | an internal message's send, once by its `clientRef`, where the server's `server.features.internalSendRef` says so (a `mail-send` of one becomes it); no files |

- **WebrootChat's rules:** no answer, 408, 429 and 5xx are tried again after 2, 5, 15 and 60 seconds; 401 and 403 hold
  until a sign-in or a renewed key, and an operation held three times in a row fails; another 4xx fails with the
  server's message. Operations of one target go in order. A discarded or deleted operation is not sent.
- **Limits:** 50 waiting, 64 KB each; a chat message with files fails after 20 hours. The drafts are capped at 500 and
  16 MB of text, their files and the sends' at a quarter of `store-max-mb`. Files no operation or draft refers to are
  collected.
- **Not queued:** an internal message, unless the server takes its send once by its `clientRef` (`internal-send`):
  Compose keeps it as a draft until there is a connection. A send with an undo hold is not queued either.
- **Conflicts and rejections (`clientApi` 4):** an operation the server met changed (a thread's `seen`, a draft's
  `baseStamp`) is `conflict`, with the server's state in `current`; a mail send's own text is then kept as a copy, a
  new Compose draft (`current.copyKey`, kept as Compose keeps a draft written offline under the `clientRef`
  `current.copyRef`), and the server's draft stays. A refusal (another 4xx) is `failed` with its
  `status`, listed as `rejected`. Neither holds up another operation; Retry of a conflict sends the user's change
  anyway (no `seen` or `baseStamp`, new references), Discard drops it. `outbox.kinds()` says what the server takes.
- **Moved from the browser:** WebrootChat uses the host's outbox when there is one (its own flush off) and moves the
  outbox it kept in scoped `localStorage` into it item by item, each taken off the browser's list once the host has it
  (one refused for now stays to try again). Chat's and Compose's drafts move the same way, the newer copy kept.
- **Compose offline:** a draft is kept here when the server cannot be reached (an email draft by its message), files are
  queued with it, and the send is queued. `composeData` answers a draft's `clientRef`, so a draft reopened offline is
  sent by the ref the server keeps.

The pages reach these through `moquiDesktop.outbox` and `moquiDesktop.drafts`, for the calling window's account
([Qvt-Client](Qvt-Client.md#in-the-desktop-app)); the shell's `outbox.unsent()` lists what a shared sign-out would delete.

## The server's sync contracts

What `clientApi` 4 adds on the server for a client that keeps data offline (MD5 stage 2; the client's use of them is
stage 3 of the plan). Each answers only the user's own data, and each is a JSON transition of the BasicComms screens.

**The mail change cursor.** `Inbox/Mail/mailChanges?afterSeq=&limit=` (through `InboxServices.run#Operation`,
`EmailClientServices.get#MailChanges`) answers `{changes, lastSeq, more, reset, uncoveredStores}`, or
`{unsupported: true}` when an app's inbox source answers the Inbox without a cursor (the client then reads its folders
each time). An answer that is not `unsupported` covers every store of the Inbox but those `uncoveredStores` names
(IronStone: `internal`, whose messages have no change log): the client reads those stores' threads again each time.

**What an app adds.** `/qapps/clientConfig`'s `server.features` has what each inbox source's `features` operation
answers (`ProviderInterfaces.inbox#Source`; none for BasicComms alone): `internalSendRef` (an internal message's send is
made once by its `clientRef`, so a client may queue it) and `mailChangesUncovered` (the stores the mail cursor does not
cover, as `uncoveredStores`). IronStone answers both.

- Each change is one thread, the newest change of it: `{changeSeq, kind, threadId, emailAccountId, emailMessageId,
  emailFolderId, changeDate, removedMessageIds, deleted, messages}`. `messages` are the thread's messages as they are when
  read (`{emailMessageId, threadId, emailAccountId, emailFolderId, statusId, isRead, isFlagged, isAnswered, archived,
  archivedDate, snoozedThruDate, messageDate, subject, fromName, fromAddress, snippet, hasAttachment, labelIds}`), so the
  client replaces what it holds of the thread; `removedMessageIds` are the tombstones (deleted, out of the account, or in
  another thread now), and `deleted` is true when nothing of the thread is left. `kind` (`new`, `state`, `move`,
  `delete`) is a hint.
- `more` says to read again from `lastSeq` at once. `reset` true, with no changes, says to read the folders again and
  go on from `lastSeq`: no `afterSeq`, the user's first read (which starts the log), a cursor older than the log kept
  (`mail_change_keep_days`, 30), past the last change, or after the user's accounts changed (a member added or removed).
- Rules on `EmailMessage` and `EmailMessageLabel` (`framework/entity/Email.eecas.xml`) note every change, wherever it
  comes from (sync arrivals, flag scans, moves and expunges, delivery, sends, drafts, the Inbox's actions), for each
  user of the account who has a cursor (`EmailUserState`); a user with none keeps no log. The changes are numbered at
  the user's next read under the lock of that row, so the numbers follow the order they committed, one row per thread.
  A note never fails the write it comes from (it logs and leaves no error).
- A sequence never starts again: a reset (accounts changed) uses up a number, so even a caught-up cursor answers
  `reset`; a cursor not read for `mail_change_keep_days` goes dormant (its log removed, nothing noted) and its next read
  answers `reset` and goes on from its last number. For ten minutes after a cursor starts or resets, each read also
  notes the threads of messages changed since then, for a writer whose transaction began before. A page ends before a
  thread that would take it past about 20000 messages (`more`), so no thread is cut.

**A request once by its `clientRef`** (`^[A-Za-z0-9_.:-]{1,64}$`, the user's own per request kind; `ClientRequestRef`,
kept `client_request_keep_days`, 30). The first request runs and its answer is kept in the same transaction; one sent
again answers it with `duplicate: true` and does nothing. Chat's `edit`, `redact`, `react`, `votePoll` and `markRead`
(`ClientSyncServices.call#ClientRequest`), and the Inbox's `state` and `viewAction` (`run#Operation`, so an app's inbox
source is covered too). An app's own mutation uses `claim#ClientRequest` and `store#ClientRequest`. A ref is keyed by
user and request kind alone, not by its parameters: a client gives each request its own ref, and the same ref with
other parameters answers the first request's result. Sent again after 30 days, the request runs again.

**Conflicts.**

- Compose's `saveDraft` and `send` take `baseStamp`, the `draftStamp` of the copy the client edited (answered by
  `composeData` and each save; the time of the draft's last content save, which attachments alone, the sync and flags do
  not move). When the server's draft was saved since, nothing is saved or sent: `{conflict: true, compose}` with the
  server's draft, and the client keeps its own as a copy (a new draft with a new `clientRef`). A save sent again after
  a lost answer is not a conflict with itself: the draft holds what it brings, or its `saveRef` (the client's reference
  of that one save) was the draft's last. Stamps compare in milliseconds, or in seconds on a database that keeps no more.
- Each target of the Inbox's `state` takes `seen`, the state the client saw for the fields its action changes
  (`isRead`, `isFlagged`, `archived`, `snoozedThruDate`, `emailFolderId`, `labelIds`, and `latestMessageId`, the newest
  message the client saw, so a read or archive does not take a message that came since; for a thread: every message
  read, any flagged, every one archived, the latest snooze, one message in the folder; a flag's on the message it flags,
  labels only those the action adds or removes). When the server's differs, the target
  is left and is in `failedList` with `reason: "conflict"` and `current`, the state now; the server's change wins.

**Files in chunks.** Compose's and Chat's `uploadChunk` (multipart: `uploadId`, the client's own name of the upload;
`offset`; `total`, at most 25 MB; `fileName`; `contentType`; the bytes as the part `chunk`) answer
`{uploadId, received, total, accepted, complete}`. A chunk is taken only at the offset of the bytes held: one sent again,
or past a gap, answers `accepted: false` with `received`, where to go on. `uploadStatus?uploadId=` answers
`{found, received, total, complete}` after a restart (`found: false`: start again). The chunk that ends at `total` joins
the file; then Compose's `upload` and Chat's `upload` take it by `uploadId` in place of the file. An upload is its user's
only, and goes `client_upload_expire_hours` (24) after its last chunk. A chunk but the last is at least
`client_upload_min_chunk_bytes` (256 KB); a user has at most `client_upload_max_count` (20) unfinished uploads of
`client_upload_max_mb` (100) MB in all.

The jobs `clean_MailChanges_daily`, `clean_ClientRequests_daily` and `clean_ClientUploads_hourly` clean them up, each user
or upload on its own.

## The client's sync

With a server at `clientApi` 4 (`moqui_client::login::ServerSync`, from the kept client config) the client uses those
contracts (MD5 stage 3); at 3 it keeps MD4's behavior. The store's step 2 keeps each feed's cursor (`sync_cursor`: the
user's chat feed and its `headsSince`, each room's message changes, the mail feed or `unsupported`), each chunked
upload's progress (`upload`), and an operation's refusal `status` and conflict `current`.

**The feeds** (`moqui_client::sync::feeds`) run when a page's `/notws` opens (in the mirror's run, or a run of their
own), when a push's `changeSeq` or `roomChangeSeq` is past the next number, names a change the push does not carry
whole (a room joined), or does not fold (chat only, then), and in the mirror; a push that is the next number and
carries its change moves the cursor itself. A first read (no cursor) only takes the cursor; a `reset` of a cursor drops
what its feed covers, with the new cursor in one unit, and the mirror reads it again (after the run under way, if
one is). The mirror and the feeds of an account run one at a time, so an older page never lands over a newer change.

What a change leaves to read (the room list, a room, a room's latest page, a thread), and a change that does not
fold, is kept in the store as a pending read before the cursor moves past it, and read at the start and end of each
run until it answers (five tries), so a stop or a failed read loses nothing.

- **The user's chat feed:** a room left (`member` false) is forgotten with its cursor and answers; a new room or a room
  changed reads the room list (and the room); settings go on the room's row; a read marker clears the counts when it
  reaches the last message (else the list is read for them); retention deletes the events up to `eventSeq`, the ranges
  below them and the room's cached timeline answers. The heads (all of them after a drop, else those since
  `headsSince`) restore a stale head whose stretch reaches the room's newest event, or read that room's latest page.
- **Room feeds:** for each room held whose `lastChangeSeq` is past its cursor: a redaction folded as the push folds it,
  another change's message read again (`timeline` by `eventSeq`, a thread reply's root too, an event gone removed),
  in order, the room's cursor stopping at the first change not done; past 50 messages, or on a reset, the room's held
  events go and its latest page is read. A room with nothing held only takes its cursor.
- **The mail feed:** each change makes the thread match: `deleted` removes it everywhere, `removedMessageIds` and the
  messages no longer in it go, the kept messages and the thread's rows take its state (read, flags, archived,
  folder, label, newest message), a row of a folder none of its messages is in leaves it, and a thread with a message
  not kept is read again. A folder that lists messages has each message's row changed; a thread new to a list the
  store keeps (the inbox, its folder) gets a row there. The stores it does not cover have their kept threads read
  again, 50 a run, going round them.

**The outbox** at 4: every operation goes with its `clientRef` (`duplicate` is done); `mail-state` sends `seen` (the
server's state as the stored thread showed it before the changes still queued, but for a field one of those makes,
and the newest message of the page's row), and keeps the rows it changed, which a conflict or a refusal puts back
(the server's state stands) with the threads read again; `mail-send` sends `baseStamp` and `saveRef`, and the send the
stamp its save left; a retry of its conflict removes the kept copy, so the text is not sent twice. A file over 256 KB
goes by `uploadChunk` in 1 MB chunks, its progress kept, and after a restart or a retry goes on from what
`uploadStatus` says, then `upload` takes it by `uploadId`.

## Bundled screens and app windows

`shell/bundled-screens.txt` lists the screens bundled with the app: the shell's own Root and SignIn, and BasicComms'
chat panel, Chat, Inbox (Mail), Compose, Calls/Call and Calls/Dialer, from their `.qvue` files, the web's own source.
They are precompiled into `shell/js/bundled-screens.js` with WebrootCall.js's components in `WebrootCall.render.js`,
and the libraries they load (tiptap, the crypto library) are in `libs.txt`; a golden test checks both
(`UPDATE_GOLDEN=1 cargo test -p moqui-core --lib bundled_screens_are_precompiled`, `call_components_are_precompiled`).

- **In the shell:** Root routes those screens with its own history under the account's comms app (`calls.appRoot` of the
  client config). A link to them from the OS or a notification (`mailto:`, `tel:`, `sip:`, a chat or ring notification)
  is kept by the host until the shell takes it, across a sign-in or a load (`client/windows.rs` `open_page`).
- **In app windows:** every other screen, the server's own `/qapps`: Calls' History, Voicemail and Recordings; Settings,
  Settings/Calls too (its device choices are per-origin `localStorage`, so the app window's; the call lobby's pickers set
  the shell's); Home and the notifications; MessageDetail; the quick creates; an attachment opened to view; anything not
  bundled.
- **An app that extends them** (BOSS, at `/qapps/Comms`): the shell keeps its `screen-extend` transitions, its
  `ext-action` items (`m-ext-actions`, `/extItems`) and its shell providers; its own screens open in app windows.
- **Versions:** the bundled screens are the app build's. A server with newer screens shows them only in app windows
  until the app is updated.
- **In the shell, not on the web:** mail's remote images are not offered (the shell's CSP); Chat's "Copy link" copies a
  shell address.

## Chat crypto

The shell owns the account's chat crypto device, named for the computer ("MR Moqui desktop on <host>",
`moquiDesktop.deviceName`). Its store passphrase is kept in the key store so cached ciphertext decrypts offline. An app
window makes and opens no device (`window.__moquiCryptoInShell`): `moqui.crypto.ensureDevice` and each call that needs
the device refuse with `elsewhere`, and an encrypted room there offers Open in Chat, which shows it in the shell for the
window's account while that account is current (`open_in_shell`).

A browser device of the same user is a device of its own; the shell's is made new, with no migration from a store of
an app window or a browser, and its older room keys come from the key backup ([Chat keys](../interfaces/Chat-Keys.md)).

## Calls

The shell owns the account's calls, so each call has one owner of its media.

- It rings: a native notification whenever the main window is not focused or is hidden (the host tells the page,
  `bridge::tell_focus`), with sound and no gesture first (CEF's `--autoplay-policy=no-user-gesture-required`).
- It shows a call (the ring bar, the LiveCall window, Calls/Call) from a room, the Dialer, a notification or a
  `tel:`/`sip:` link; a second number fills the Dialer already shown. The camera and microphone are granted as to the
  app windows, and an encrypted call's keys go over the shell's crypto device, its SFrame worker the one the CSP allows.
- An app window rings silently. Its Answer, Join and Return open the call in the shell (`open_in_shell`), and it never
  ends the shell's call. A plain call started there is created there and joined in the shell; an encrypted room's call
  opens the room in the shell (`moqui.callKeys.prepare` refuses with `elsewhere`).
- Offline, a call's screen and the Dialer say they need a connection, and read again when it is back. Switching account
  or signing out in a call asks first and leaves it.

## Offline in the screens

From `moqui.connection`, inert on the web: offline is the host's own unreachable answer
(`X-Moqui-Offline: unreachable`) or no answer at all. The screens show queued, sending and failed messages and mail with
Retry and Discard, the Inbox's Outbox, the end of kept history, "needs a connection" on what only the server can do, a
shared device's hint of what is kept, and the sign-out's list of what is unsent.

**What the server did not take (MD5).** With a server at `clientApi` 4 the screens follow what the host's outbox takes
(`outbox.kinds()`), and show its conflict and refused states where they show what waits, each with Retry and Discard:

- **Chat:** a message's edit, deletion, reaction and poll vote go through the outbox when `kinds()` has them
  (`moqui.chat.queueChange`; a vote taken back, with no option, is asked of the server), offline too, so they no
  longer need a connection. Each shows under its message (`.m-chat-change`) as waiting, sending, held ("sign in again
  to send it") or not made with the server's reason, and the message shows the change when its answer (the host's
  `sent`, which the page hears as `changed`) or its push brings it; a reaction shows at once, as before, and is read
  again from the store when its refusal is discarded. A chat send refused shows its error, as in MD4. After a
  reconnect the room reads the messages it shows again (up to 200), so edits and deletions the changes feeds brought
  into the store, or a reset of them, show.
- **Mail:** the Inbox's Outbox lists a thread change the server met changed ("Changed on the server: the thread shows
  its state there", with the server's reason) or refused ("Not made"), apart from the changes still waiting, which it
  counts; the list and the open thread are read again when one arrives, so the rows show the server's state. Retry
  sends the change anyway; Discard leaves the thread as the server has it. An internal message waiting is listed with
  the mail.
- **Compose:** a send whose draft changed elsewhere says "Changed elsewhere: your version is kept as a copy" in the
  Outbox, with Open the copy: Compose opened with `copyRef` reads the copy the host kept
  (`ironstone.compose.email.<copyRef>`, `moqui.drafts.fetch`), says it is the user's version kept as a copy, and saves
  it on the server as a new draft by that `clientRef`; the server's draft stays. Retry sends the user's version and the
  host deletes the copy; Discard keeps it, listed in the Outbox (Open, Delete) until it is opened or deleted. A
  queued send of a server draft carries `baseStamp`, the `draftStamp` Compose last saw, so the conflict is found. An
  internal message offline waits in the outbox when `kinds().internalSend` is true, else it is kept as a draft as in MD4.

## Known gaps

- Device-dependent features refuse with `elsewhere` in app windows (My devices' actions, the key backup, verification),
  and their Settings screens are not bundled in the shell yet, so they cannot be used in the client.
- A shared sign-out does not clear the webview's HTTP cache. The pages' queue of scoped storage to clear is in memory,
  so a page that closes before it runs leaves it to the next one.
- A kept link that arrives while signed out stays for the next sign-in, which may be another account's.
- The store's cap keeps no running total of its size: each check measures the rows and files again, a cost that
  grows with the store.
- A reply or forward built offline quotes with English labels and this computer's time zone, not the user's locale and
  zone as the server does.
- MD5: a queued message change is not shown in the stored rows until its push or the feed brings it (the screen
  shows it as waiting). Compose's saves while online send no `baseStamp`, so a draft edited online in two places is
  the last save's; only a queued send carries it. A chat message's files are uploaded by the page before it is queued,
  not in chunks by the host ([Mobile-Desktop §6](../../plans/Mobile-Desktop.md#6-offline-behavior-and-data-ownership)).

## Tests

- `cargo test -p moqui-client`: accounts, keys, sign-in, proxy, sockets, the store and the conf;
  `scripts/check-android.sh` for the Android build.
- `cd crates/moqui-desktop && CARGO_TARGET_DIR=../../target cargo test`: the schemes, offline answers, pushes, the
  mirror, modes, the outbox and flusher, windows.
- `moqui test-client webroot --offline`: MoquiLib's drafts and connection, WebrootChat's host outbox and import.
- `moqui test-client basic-comms --offline`: the bundled screens in the shell (`ShellComms`, `ShellCalls`), and
  MD5's states in them (`ShellSync`: a lost answer then a retry, a duplicate push, a reconnect after a feed reset, a
  thread change's and a send's conflict with the copy opened, a refusal with Retry and Discard, a revoked key
  mid-queue).
- `moqui test basic-comms`: Compose by `clientRef` (`BasicCommsComposeRef`); the mail change cursor, requests once by
  `clientRef`, conflicts, chunked uploads and their clean-up (`BasicCommsSync`); `cargo test -p moqui-mail`: a sync's
  arrivals, flag scan and expunges on the change cursor.
- The checks on installed apps are in [Mobile-Desktop-Exit-Checks](../../plans/Mobile-Desktop-Exit-Checks.md).
