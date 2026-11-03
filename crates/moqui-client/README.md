# moqui-client

The client of a remote MR Moqui server, for the desktop app's client mode and later the mobile app (configuration 3 of
[Mobile-Desktop](../../plans/Mobile-Desktop.md), MD3). It has no Tauri, no framework and no async API (reqwest's blocking
client, so callers run on a plain thread), and builds for Android as well as the desktops. TLS is checked against the
OS trust store; on Android that needs the app to bundle `rustls-platform-verifier`'s Android component (its Maven
artifact) and call `rustls_platform_verifier::android::init_hosted` before the first request.

Each installed client is a device with its own login key. The host keeps the key and adds it to every request as the
`login_key` header; the page never holds it.

- `account`: the servers and users signed in on this device (`accounts.json` in a directory the host gives), the
  current one, and each one's device mode (personal or shared, set when it is added and never lowered). Plain
  `http://` only to this machine.
- `keys`: where the keys are kept. `KeyStore` is filled by the host with the OS key store; `FileKeyStore`, a 0600 file,
  is the fallback where there is none.
- `login`: password sign-in (REST `login` with `createLoginKey=true` and `deviceName`), the second factor's code,
  sign-out that revokes the key (`/rest/logout` with `revokeLoginKey=true`), the server's `/qapps/clientConfig` and
  the `clientApi` this client knows (`CLIENT_API`), and the key's renewal: its life from the device list
  (`key_period`), renewed when less than a third is left (`renew_due`, `/qapps/renewClientDevice`).
- `proxy`: a page's request forwarded to the same path on the server with the key. Only plain request headers pass;
  cookies are dropped from the answer and a redirect to the server is pointed back at the app scheme.
- `socket`: `/notws` and `/callws` held by the host for a page, connecting again after a drop, but not after a close
  for authentication.
- `cache`: the last answers of the shell's settings, apps and counts, per account, for a start with no network.
- `store`: an account's data for offline use (MD4), one SQLite file per account (`data/<account>.db`, owner-only, WAL,
  migrated by `user_version`): cached answers by path and sorted query; chat rooms and events, with the seq ranges
  held whole so an offline page stops at a gap; mail folders, threads and messages, replaced when re-read; the outbox
  of operations by `clientRef` (claimed atomically, ordered per target, recovered after a crash); drafts; and their
  files beside it (`data/<account>.files/`). Past its size cap the least recently read cached reads go; never the
  outbox, drafts, files or folders. Synchronous, one connection: the host shares a `Store` in a `Mutex`.
  `store::forget` removes an account's.
- `conf`: `ClientConf`, the client's settings from the `<client-conf>` element of the Moqui XML conf, which the host
  hands over: `device-mode` (choose, personal, shared), `mirror-days`, `prefetch-rooms`, `prefetch-mail` and
  `store-max-mb`; a bad value is logged and its default used.
- `sso`: sign-in through the server's identity providers in the system browser (RFC 8252): a PKCE verifier and state,
  the server's `/sso/appLogin`, the hand-back's state checked, and the code taken with the verifier for a device key.

The patterns follow `moqui tui --server` (`crates/moqui-host/src/tui_remote.rs`).

## Test

```sh
cargo test -p moqui-client
scripts/check-android.sh   # clippy for aarch64-linux-android: rustup target add aarch64-linux-android, and an NDK
                           # (ANDROID_NDK_HOME, an SDK's, or brew install --cask android-ndk)
```
