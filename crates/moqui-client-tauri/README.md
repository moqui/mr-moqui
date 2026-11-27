# moqui-client-tauri

The Tauri glue of the client of a remote server that the desktop app ([moqui-desktop](../moqui-desktop/README.md), on
CEF) and the mobile app ([moqui-mobile](../moqui-mobile/README.md), on wry) share: what needs Tauri but no webview
runtime and no window layout. Tauri 3 runs both on its type-erased runtime (`tauri::DynRuntime`), so the commands and
handlers are the same. [moqui-client](../moqui-client/README.md) stays Tauri-free; this crate is outside the workspace,
as the apps are, so Tauri stays out of the server builds.

- `ClientState`, managed on the app: the client's core, the app as its `ClientHost`, what the app does with its pages
  (`ShellHost`: show the shell, open a server page, the device name, the SSO redirect, which page is the shell, whether
  requests may be made) and the shell's files (`ShellFiles`).
- `scheme`: the app schemes' answer (the shell's files, the rest forwarded with the key, kept and answered offline), the
  worker threads, and `SchemeHooks` for what an app does with an answer (the desktop saves downloads, Android takes no
  redirect status).
- `vault`: the page's `moquiDesktop.vault` (`vault_call`, `vault_memory`), the vault module run natively for the calling
  page, which must be a vault screen's document (the schemes see each page's loads); the schemes refuse the server's
  copy of the module.
- `packages`: the offline map packages' commands (`packages_list`, `packages_download`, `packages_cancel`,
  `packages_delete`, `packages_status`, the shell's only; plans/Maps-Offline.md §2.4); a download runs as
  `ShellHost::start_package_download` says (a thread by default, Android's WorkManager worker on the phone), and the
  schemes answer `/__moqui-app/maps/<packageId>/<path>` from the device to the shell's origin only.
- `commands` (the shell's: accounts, sign-in and SSO, sign-out), `outbox` (the outbox and drafts), `sockets` (a page's
  server sockets, the mobile app's; the desktop has its own with the node's in process). The commands are `pub` at the
  crate root, where `generate_handler!` finds them: `moqui_client_tauri::client_state`.

How the client works is [docs/ui/Desktop-Client.md](../../docs/ui/Desktop-Client.md) and
[docs/ui/Mobile-Client.md](../../docs/ui/Mobile-Client.md).

## Test

```sh
./moq test-clients tauri    # or: cd crates/moqui-client-tauri && cargo test (its Cargo.lock is committed)
scripts/check-android.sh    # clippy for both Android targets, this crate among them
```
