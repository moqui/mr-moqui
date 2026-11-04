# moqui-client

The client of a remote MR Moqui server, for the desktop app's client mode and the mobile app (configuration 3 of
[Mobile-Desktop](../../plans/Mobile-Desktop.md), MD3 and MD4; how the desktop app uses it is
[docs/ui/Desktop-Client.md](../../docs/ui/Desktop-Client.md)). It has no Tauri, no framework and no async API (reqwest's blocking
client, so callers run on a plain thread), and builds for Android as well as the desktops. TLS is checked against the
OS trust store; on Android that needs the app to bundle `rustls-platform-verifier`'s Android component (its Maven
artifact) and call `rustls_platform_verifier::android::init_with_env` before the first request (as
[moqui-mobile](../moqui-mobile/README.md) does).

Each installed client is a device with its own login key. The host keeps the key and adds it to every request as the
`login_key` header; the page never holds it.

- `account`: the servers and users signed in on this device (`accounts.json` in a directory the host gives), the
  current one, and each one's device mode (personal or shared, set when it is added and never lowered). Plain
  `http://` only to this machine.
- `keys`: where the keys are kept. `KeyStore` is filled by the host with the OS key store; `FileKeyStore`, a 0600 file,
  is the fallback where there is none.
- `login`: password sign-in (REST `login` with `createLoginKey=true` and `deviceName`), the second factor's code,
  sign-out that revokes the key (`/rest/logout` with `revokeLoginKey=true`), the server's `/qapps/clientConfig` and
  the `clientApi` this client knows (`CLIENT_API`, 4) and what a server supports of the sync (`ServerSync`: its
  `clientApi` and `server.features`), and the key's renewal: its life from the device list
  (`key_period`), renewed when less than a third is left (`renew_due`, `/qapps/renewClientDevice`).
- `proxy`: a page's request forwarded to the same path on the server with the key. Only plain request headers pass;
  cookies are dropped from the answer and a redirect to the server is pointed back at the app scheme.
- `socket`: `/notws` and `/callws` held by the host for a page, connecting again after a drop, but not after a close
  for authentication.
- `cache`: the shell's settings, apps and counts as MD3 kept them in files, moved once into the account's store.
- `store`: an account's data for offline use, one SQLite file per account (`data/<account>.db`, owner-only, WAL,
  migrated by `user_version`): cached answers by path and sorted query; chat rooms and events, with the seq ranges
  held whole so an offline page stops at a gap; mail folders, threads and messages, replaced when re-read; the outbox
  of operations by `clientRef` (claimed atomically, ordered per target, recovered after a crash; a refusal's status and
  a conflict's server state kept); drafts; and their files beside it (`data/<account>.files/`); the cursors of the
  server's changes feeds and the progress of chunked uploads (MD5). Past its size cap the least recently read cached
  reads go; never the outbox, drafts, files, folders or cursors. Synchronous, one connection: the host shares a `Store`
  in a `Mutex`.
  `store::forget` removes an account's.
- `conf`: `ClientConf`, the client's settings from the `<client-conf>` element of the Moqui XML conf, which the host
  hands over: `device-mode` (choose, personal, shared), `mirror-days`, `prefetch-rooms`, `prefetch-mail` and
  `store-max-mb`; a bad value is logged and its default used.
- `sso`: sign-in through the server's identity providers in the system browser (RFC 8252): a PKCE verifier and state,
  the server's `/sso/appLogin`, the hand-back's state checked, and the code taken with the verifier for a device key.

The client's core, what the desktop app's client mode and the mobile app run
([Desktop-Client](../../docs/ui/Desktop-Client.md)):

- `app`: `Client`, the state while the app runs as a client (the accounts, `keys::Keys`, the stores, the conf read
  once through the host, the comms app of each account, the mirror, the outbox's sender, the proxy and the pages'
  sockets), held by the host in an `Arc`; and `ClientHost`, what the host does for it: which account a page shows,
  whether a page is the shell, telling an account's pages of an outbox change, handing a page its socket's events,
  and checking a key the server answered 401 to. `keys::Keys` keeps the keys in memory over the host's `KeyStore`,
  with each account's chat store passphrases. `Client::shutdown` ends its threads and closes the pages' sockets (which
  hold it weakly), so a host that makes a new `Client` lets the old one and its stores go.
- `bridge`: the pages' sockets by id and page (`Sockets`, any `PageSocket`), `/notws` and `/callws` opened on the
  account's server (`open_remote`), and a client's `/notws` frames kept in the store, the heads marked stale on a drop,
  the mirror started and the outbox woken when the shell's opens.
- `stores`: each account's store, opened when first used and only while the account is kept; `offline`: the comms
  transitions' answers and rows kept, and answered when the server cannot be reached.
- `sync`: `push` (the chat pushes folded into the store, a gap in their numbers noted), `outbox` (the operations and
  drafts, their checks and limits), `flush` (the outbox sent from a thread of its own, with WebrootChat's retry rules,
  conflicts and chunked uploads), `mirror` (the current account's chat and mail read ahead) and `feeds` (the server's
  chat and mail changes feeds folded into the store, with a server at `clientApi` 4).
- `modes` (what each device mode keeps), `renewal` (each key renewed on a thread before it runs out) and `assets` (the
  shell's own files and their CSP: on disk for the desktop, or embedded in the mobile app, `Assets::embedded`).

The threads are plain threads with blocking requests. The patterns follow `moqui tui --server`
(`crates/moqui-host/src/tui_remote.rs`).

## Test

```sh
cargo test -p moqui-client
scripts/check-android.sh   # clippy on this crate and moqui-mobile for both Android targets (an NDK, ~/Android/Sdk)
                           # (ANDROID_NDK_HOME, an SDK's, or brew install --cask android-ndk)
```
