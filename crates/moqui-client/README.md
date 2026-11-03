# moqui-client

The client of a remote MR Moqui server, for the desktop app's client mode and later the mobile app (configuration 3 of
[Mobile-Desktop](../../plans/Mobile-Desktop.md), MD3). It has no Tauri, no framework and no async API (reqwest's blocking
client, so callers run on a plain thread), and builds for Android as well as the desktops. TLS is checked against the
OS trust store; on Android that needs `rustls_platform_verifier`'s JVM init.

Each installed client is a device with its own login key. The host keeps the key and adds it to every request as the
`login_key` header; the page never holds it.

- `account`: the servers and users signed in on this device (`accounts.json` in a directory the host gives), and the
  current one. Plain `http://` only to this machine.
- `keys`: where the keys are kept. `KeyStore` is filled by the host with the OS key store; `FileKeyStore`, a 0600 file,
  is the fallback where there is none.
- `login`: password sign-in (REST `login` with `createLoginKey=true` and `deviceName`), the second factor's code,
  sign-out that revokes the key (`/rest/logout` with `revokeLoginKey=true`), and the server's `/qapps/clientConfig`.
- `proxy`: a page's request forwarded to the same path on the server with the key. Only plain request headers pass;
  cookies are dropped from the answer and a redirect to the server is pointed back at the app scheme.
- `socket`: `/notws` and `/callws` held by the host for a page, connecting again after a drop, but not after a close
  for authentication.
- `cache`: the last answers of the shell's settings, apps and counts, per account, for a start with no network.

The patterns follow `moqui tui --server` (`crates/moqui-host/src/tui_remote.rs`).

## Test

```sh
cargo test -p moqui-client
```
