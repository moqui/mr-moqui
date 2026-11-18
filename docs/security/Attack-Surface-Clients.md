# Attack surface: clients

The desktop and mobile apps and the other clients: the commands a page can call in the apps (Tauri IPC), the apps' URL schemes and OS links, the node the desktop app runs, the device keys and the offline store, and how the browser shell puts untrusted content into a page. This is one file of the attack surface map. The index, how to read a row, the defaults and the lock-down advice are in [Attack-Surface.md](Attack-Surface.md).

## The desktop app's own node

A node the desktop app runs in its own process may have no HTTP or cluster port (`moqui_host::start_embedded_with`, `crates/moqui-host/src/in_process.rs`). Its window's requests and sockets reach the router in process, through the same checks as a listener's, from the loopback address, as a sidecar window's do over loopback HTTP. The app keeps the node's session cookie for the window, sets `Host` and `Cookie` itself, and does not pass on the page's `Cookie`, `Host`, `login_key`, `api_key`, `Forwarded`, `X-Real-IP`, `X-Forwarded-*`, `X-Moqui-Edge*` and `X-Moqui-Tunnel` headers (`HOST_HEADERS` and `passed_on` in `crates/moqui-desktop/src/node/scheme.rs`). Nothing outside the app's process reaches that path.

The node in process answers a request that another site started (`Origin` not `http://moqui-node.localhost`, or `Sec-Fetch-Site` not `same-origin` or `none`) only when it is a `GET` or `HEAD` for a document; any other is answered 403 before it reaches the router (`cross_site`, `navigation`). The cookie jar is the file `node-cookies` in the app config directory, opened with mode `0600` (`crates/moqui-desktop/src/node/cookies.rs`). A `Content-Security-Policy` of the node with no `script-src`, `script-src-elem` or `default-src` is not passed to the window (`has_script_policy`). An attachment or a large file a window opens is saved by the host and the page gets 204 (`crates/moqui-desktop/src/download.rs`, not read).

## The desktop app's modes

The desktop app (`crates/moqui-desktop`, identifier `org.moqui.desktop`, Chromium through `tauri-runtime-cef`) runs in one of three modes, kept in the file `node-mode` in the app config directory and switched from the tray menu (`crates/moqui-desktop/src/node/mod.rs`, `crates/moqui-desktop/src/lib.rs`).

| Mode | What listens | Window origin |
|---|---|---|
| Node in process (the default) | Nothing, unless the tray's ports item wrote `on` to the file `node-ports`; then HTTP on `127.0.0.1:<port>` | `http://moqui-node.localhost` (scheme `moqui-node`) |
| Sidecar (the `moqui` binary as a child process) | HTTP on `127.0.0.1:<port>`; the cluster listener on `127.0.0.1` at a port the kernel picks | `http://127.0.0.1:<port>` |
| Client of a remote server | Nothing | `http://moqui-shell.localhost` (main window), `http://moqui-app.localhost` (windows `remote-<n>`) |

Both node modes set `moqui_conf__webapp_list__webapp__WEBROOT__http_bind=127.0.0.1`, `moqui_conf__webapp_list__webapp__WEBROOT__http_port=<port>`, `moqui_conf__webapp_list__webapp__WEBROOT__https_listen_port=off`, `cluster_bind_address=127.0.0.1`, `cluster_https_port=0` and `moqui_conf__search_facade__cluster__run=false` (`listen_properties`); the sidecar gets them as `-D name=value` arguments with `moqui_stop_on_stdin_close=true`. The port is the one in the file `node-port` when it is free, else one the kernel picks (`choose_port`). The loopback listener is plain HTTP and is the node's ordinary web listener: every local process and every local user of the machine reaches the same routes a remote caller of a server reaches, the sign-in and the first-administrator form (`createInitialAdminAccount` in `runtime/components/base/webroot/screen/webroot/Login.xml`) among them; the first administrator is taken only from this machine or with the setup secret of the startup log ([Login lockout and passwords](Attack-Surface-Web.md#login-lockout-and-passwords)). Both node modes set `webapp-list/webapp[webroot]@allowed-hosts` to `127.0.0.1:<port>,localhost:<port>`, so a request whose `Host` names anything else (a page that rebinds a DNS name to loopback) is answered 421 (`serve.rs` `host_allowed`; plans/Security-Review.md SEC-CLI-01, fixed). The tray's "Developer tools" item writes the file `node-purpose` and starts the node with `instance_purpose=dev` (`purpose_properties`); see [Attack-Surface-Dev.md](Attack-Surface-Dev.md) for what a dev instance adds. `tauri.conf.json` of both apps has `"csp": null`, `"withGlobalTauri": false` and no windows; the windows are built in code.

The DevTools protocol listens on `127.0.0.1:<MOQUI_DESKTOP_DEVTOOLS_PORT>` only in a development run (`tauri::is_dev()`) with that variable set (`cef()` in `crates/moqui-desktop/src/lib.rs`). Media autoplay needs no gesture in every window. Camera and microphone requests are allowed for a page on the node's origin or a client's shell or app origin and denied for any other page (`on_permission`, `media_allowed` in `crates/moqui-desktop/src/bridge.rs`).

### Where a window may go

| Window | Rule | Code |
|---|---|---|
| Main window, node modes | Stays on the app's own pages (`tauri.localhost`, the development server), and the node's origin. An `http` or `https` URL elsewhere, and a `mailto:`, `tel:` or `sip:` link, is handed to `xdg-open` (`open` on macOS) and the window stays. A URL of any other scheme is not refused by this check and is left to Chromium. A new window follows the same rule. | `stays_in_app`, `is_external`, `crates/moqui-desktop/src/lib.rs` |
| Main window, client mode | Only `/` and `/index.html` on the shell's origin, and `about:`, `data:` and `blob:` URLs. Another path on the shell's origin or a URL on the app origin opens in an app window. `http`, `https`, `mailto`, `tel` and `sip` go to `xdg-open`. Anything else is refused. A new window is always denied. | `goes`, `stays_on`, `crates/moqui-desktop/src/client/windows.rs` |
| App window `remote-<n>`, client mode | Stays on the app origin, and `about:`, `data:` and `blob:` URLs. The shell's origin is refused. The web goes to `xdg-open`. | same |
| The one webview of the mobile app | The shell's page only when the page shown is the shell or the host itself shows the shell; the app origin only with an account signed in; `about:blank`. `http` and `https` to a host that is not `localhost` or `*.localhost`, and `mailto`, `tel`, `sip`, open outside the app. Anything else is refused. | `goes`, `crates/moqui-mobile/src/host.rs`; `navigation`, `crates/moqui-mobile/src/lib.rs` |

A page's navigation to a web URL, or to a `mailto:`, `tel:` or `sip:` link, starts the OS handler with no question to the user; the URL is one argument of the opener.

## Commands a page can call (Tauri IPC)

A page calls a command through Tauri's IPC. Which page may call which command is set by capabilities: `crates/moqui-desktop/capabilities/splash.json`, `crates/moqui-mobile/capabilities/main.json`, and three the desktop app adds at run time (`capability` in `crates/moqui-desktop/src/bridge.rs`, `add_capabilities` in `crates/moqui-desktop/src/client/windows.rs`). Tauri counts a page of the app's own schemes (`moqui-node`, `moqui-shell`, `moqui-app`) as local, so a local capability names a window, and some commands check the calling page themselves.

| Capability | Granted to | Commands |
|---|---|---|
| `splash` (desktop, file) | Local pages of window `main` | `restart_node` |
| `node-bridge` (desktop, sidecar) | Remote origin `http://127.0.0.1:<port>` in window `main`, not local pages | `notify`, `set_counts`, `set_presence`, `open_external`, `set_comms_root`, `notification:allow-is-permission-granted` |
| `node-bridge` (desktop, node in process) | Local pages of window `main` | the same, and `socket_open`, `socket_send`, `socket_close` |
| `client-shell` (desktop, client mode) | Local pages of window `main` | the twelve `client_*` commands, `socket_*`, `outbox_*`, `draft_*`, `file_save`, `notify`, `set_counts`, `set_presence`, `open_external` |
| `client-app` (desktop, client mode) | Local pages of windows `remote-*` | `socket_*`, `outbox_add`, `outbox_list`, `outbox_kinds`, `outbox_retry`, `outbox_discard`, `draft_*`, `file_save`, `open_in_shell` |
| `main` (mobile, file) | Local pages of the one webview `main` (the shell and the server's pages alike) | every command of the mobile app |

What each command does and what it checks:

| Command | Arguments from the page | Effect outside the page | Check in the command |
|---|---|---|---|
| `restart_node` (`splash.rs`) | none | Stops and starts the node | The webview's URL is the splash page's origin or `tauri.localhost` |
| `notify` (desktop `bridge.rs`, mobile `commands.rs`) | `title`, `body`, `link`, `tag` | An OS notification; title cut to 200 characters and body to 1000, control characters removed. On the desktop a click opens `link` only when it is on the node's (or app) origin | none on the caller |
| `set_counts`, `set_presence` (desktop) | numbers; one of four presence names | Tray title, tooltip and badge | none |
| `notify_clear` (desktop, mobile) | `tag` | Removes the app's notification of that tag of the current account (an id hashed from both; `chat-*` on the phone: every chat notification of the account, Mark all read); only the app's own notifications | none |
| `chat_room_shown` (mobile) | `room` (a room id, or none) | The room the shell shows now: its pushes show no notification while the app is in front, and its notification goes | shell page (a server's page cannot hide a room's notifications) |
| `push_check`, `push_check_fix`, `push_test` (mobile; desktop `push_check`, `push_test`) | `what` (fix: `notifications`, `channels`, `battery`, `fullScreen`, `distributor`, `firebase`, `callingAccounts`, `contacts`) | The notification check: the device's notification permission, channels, battery optimization, full-screen intent, whether the server has this device's push registration (no token or key is answered), the last push's time and ack id, the calling account's state and whether the contacts are allowed; a fix opens the system setting, asks the system's battery or contacts question, or opens Calling accounts; a test asks the server for one push to this device (`/qapps/testClientPush`) or, on the desktop, shows one | mobile: shell page |
| `open_external` (desktop, mobile) | `url`, at most 8192 bytes | Starts the OS handler for an `http` or `https` URL with a host that is not the node's or the app's own, or a `mailto:`, `tel:` or `sip:` URL. Other schemes are refused | none on the caller |
| `set_comms_root` (desktop) | `root` | Writes the file `comms-root` in the app config directory; it is the path prefix OS links open under | `/qapps/` then letters, digits, `_`, `-`, `/`, at most 200 characters |
| `client_state`, `client_take_open` | `nonce` | Answers the account list (server, username, display name, signed in or not, device mode) and no key | shell page (see below) |
| `client_sign_in`, `client_send_code` | `server`, `username`, `password`, `code`, `device_mode` | Posts the password to `server` and keeps the device key it returns; the account becomes current | shell page; `server` is `https`, or `http` only to `localhost`, `127.0.0.1` or `[::1]` (`base_url`, `crates/moqui-client/src/account.rs`) |
| `client_sso_providers`, `client_sso_start`, `client_sso_cancel` | `server`, `auth_flow_id`, `device_mode` | Opens the system browser at the server's `/sso/appLogin` | shell page; `base_url` |
| `client_sign_out`, `client_remove_account`, `client_switch_account`, `client_add_account` | account `id` | Revokes a key on its server, deletes the key, the account and its store; makes another account current | shell page |
| `client_open_remote` | `path` | Opens a server page of the current account (an app window on the desktop) | shell page; `path` starts with one `/`, no `\` or control character |
| `socket_open`, `socket_send`, `socket_close` | `path`, frame text of at most 1 MiB | A WebSocket to the server of the calling page's account with its device key (`moqui_client::bridge::open_remote`, not read), or to the node in process | the socket belongs to the calling webview's label |
| `outbox_*`, `draft_*` | operation `kind` and `payload`, draft `key` and `value`, file names | Reads and writes the outbox and drafts of the calling page's account in its offline store | the account is the one of the calling webview (`caller`, `crates/moqui-client-tauri/src/outbox.rs`) |
| `file_save` | `name`, `data` (base64) | Writes a file `<16 hex>-<name>` into the account's store directory `data/<account>.files`, mode `0600`; the name keeps its last 80 characters and every character but letters, digits, `.`, `-`, `_` becomes `_` (`crates/moqui-client/src/store/attach.rs`) | the calling webview's account; the store's size cap |
| `open_in_shell` (desktop `client/windows.rs`, mobile `commands.rs`) | `path` | Keeps a path for the shell to show and shows the main window | the path is a comms screen under the account's comms app; on the desktop the window's account is the current one |
| `client_show_shell` (mobile) | none | The webview goes to the shell | none |
| `call_state`, `call_take_answer` (mobile) | `nonce`, `call_session_id`, `state` (`joined`, `left`, `held`, `screen`, `offScreen`, `video`, `voice`), `name` (with `joined`, the call's name a car or a watch shows; with `screen` and `offScreen`, the call screen's mount token; one line, at most 80 characters) | Tells the native call code a call's state: a joined call is placed in Telecom as a self-managed call, `held` holds it there; answers once whether the call was answered on the phone's call screen | shell page |
| `call_audio`, `call_audio_route` (mobile) | `nonce`, `route` (`earpiece`, `speaker`, `wired`, `bluetooth`) | The call's audio route and routes in Telecom, and the user's choice of one for the call the app is in; nothing without a call in Telecom | shell page |
| `call_dial_account` (mobile) | `nonce`, `can_call` | Whether the current account has a number to call from; the app's managed `tel` calling account (`DialAccount.kt`) is registered with Telecom while an account here has one (`client/dial.json` lists them), and taken away when none has. Registering it does not turn it on: only the user does, in Android's Calling accounts | shell page |
| `call_dial_choice`, `call_dial_from` (mobile) | `nonce`, `account` | A number from the phone's dialer waiting for the user's pick of the account that calls it (the current account cannot, several others can): the number and the accounts (`{id, label}`), for two minutes and for the account it was shown on; the pick makes that account current and shows its Dialer with the number filled in. Neither places a call | shell page; `account` one of the choice's |
| `contacts_search`, `contact_names` (mobile) | `nonce`, `term` (2 to 100 characters), `numbers` (E.164 only, at most 50) | The phone's contacts (`PhoneBook.kt`, `READ_CONTACTS`): up to 10 `{name, number}` whose name or number has the term, and the names of the numbers. Read on the phone for the shell's page only; nothing is sent to a server or kept. The first search asks for the permission (the reason first); the names never ask. The page asks the server's `phoneNames` for every number of its phone rooms, not only those the phone does not know, so the server cannot infer which numbers are in the phone's contacts | shell page |
| `scheme_fetch` (mobile `scheme.rs`) | `method`, `url`, `headers`, `body` (base64) | Makes the request the scheme would have answered, for a request with a body | the webview's URL is on the shell or app origin and `url` is on that same origin |

No command takes a path on the user's file system, reads a file outside the offline store, returns a device key, or starts a process other than the OS link opener.

The shell-page check differs by app. On the desktop `is_shell_page` is the trait default and returns true; the `client-shell` capability and the main window's navigation rule are the check. On the mobile app the one webview shows server pages too, so `is_shell_page` requires the webview's URL to be on the shell's origin and the `nonce` argument to equal the nonce of the shell's current load. The host makes a new nonce when a load of the shell finishes, evaluates it into the document, and clears it when any navigation or load starts (`PageState` in `crates/moqui-mobile/src/host.rs`). On the mobile app `client_show_shell`, `open_in_shell`, `notify`, `open_external`, `scheme_fetch`, the sockets, the outbox and the drafts have no shell check: a server's page calls them.

## The app schemes and what reaches the server

In client mode the pages are on two schemes registered by the app (`crates/moqui-client-tauri/src/scheme.rs`, `crates/moqui-client/src/proxy.rs`).

- `moqui-shell` (`http://moqui-shell.localhost`): the shell's own files (`/`, `/shell/`, `/js/`, `/libs/`, `/css/`) are answered from the app's staged files or 404, never from the server. The shell's page gets the shell's `Content-Security-Policy` (`assets::configured_shell_csp`, not read). Any other path is forwarded to the current account's server only when `Sec-Fetch-Dest` is `empty`, `image`, `audio`, `video`, `track` or `font` (or absent on a request that is not a document); a document, frame, script, style or worker of the server is answered 403. A forwarded answer carries `X-Content-Type-Options: nosniff`.
- `moqui-app` (`http://moqui-app.localhost`): every request is forwarded to the server of the window's account. The server's HTML and scripts run on this origin.

The proxy adds the account's device key as the `login_key` request header; the page never receives the key. Of the page's request headers only fifteen pass: `accept`, `accept-language`, `content-type`, `range`, `if-range`, `if-none-match`, `if-modified-since`, `x-requested-with` and seven `x-moqui-*` headers (`REQUEST_HEADERS`); a page cannot send `Cookie`, `Authorization`, `Origin`, `login_key`, `api_key` or an edge link header. A request whose `Origin` is not the scheme's origin, or whose `Sec-Fetch-Site` is neither `same-origin` nor `none`, is refused before the key is added (`cross_site`). The path must start with one `/` and hold no `\`, `#` or control character, so the target is always the account's server (`upstream_url`). From the answer `Set-Cookie`, `WWW-Authenticate`, `Strict-Transport-Security` and every `Access-Control-*` header are dropped. The request signs in from its `login_key` header and carries no cookie, and the server checks no session token for a request that signs in from a key header ([CSRF](Attack-Surface-Web.md#csrf)); the proxy's cross-site refusal is the check. On the mobile app `scheme_fetch` takes the request's headers from the page, `Origin` and `Sec-Fetch-*` among them.

A `POST` to `/ChatKeys/storeKey` returns the passphrase of the page's chat store. The host keeps the server's answer in the key store, and when the server cannot be reached it answers the kept passphrase to a request on the shell's origin only.

## Links from the OS

The desktop app registers the schemes `mailto`, `tel`, `sip` and `org.moqui.desktop` (`plugins.deep-link` in `crates/moqui-desktop/tauri.conf.json`); `mailto`, `tel` and `sip` become the user's default only after the tray item "Make default for email and calls", which runs `xdg-mime default` with fixed arguments (`crates/moqui-desktop/src/os/handlers.rs`). Any local program or web page that opens such a link reaches the app.

| Link | What the app does | Code |
|---|---|---|
| `mailto:` | Opens `<comms root>/Inbox/Compose` with `to`, `cc`, `bcc`, `subject` and `body` from the link as query parameters. Other headers of the link (`attach`, `in-reply-to`) are dropped | `route`, `mail_params`, `crates/moqui-desktop/src/os/links.rs` |
| `tel:`, `sip:` | Opens `<comms root>/Calls/Dialer?number=<number>` | `route`, `call_number` |
| `org.moqui.desktop:/sso?code=…&state=…` | Taken only when an SSO sign-in is waiting and the link's `state` equals its state; otherwise logged and dropped | `sso_returned`, `crates/moqui-client-tauri/src/commands.rs` |
| `org.moqui.mobile:/sso?…` (mobile) | The same | `on_event`, `crates/moqui-mobile/src/lib.rs` |
| A call link (mobile) | Taken only when it carries a nonce the native code made for the user's Answer or Hang up, once | `calls::link_of`, `native::take_link` (not read) |
| A dial link (mobile, `org.moqui.mobile:/dial?nonce=…`) | Taken only when it carries a nonce the native code made, once and within two minutes, for a number that came through Telecom to the app's calling account (`DialAccount.dialed`, from Telecom's binding of `CallConnectionService`, which only Telecom may bind: `BIND_TELECOM_CONNECTION_SERVICE`). The nonce stops a link sent to the activity by itself; it does not stop another app from choosing the number, since any app may place a call with that account (`placeCall`, `ACTION_CALL`). So the number (digits, `+`, `*`, `#` only) is only filled in on the Dialer, and the user presses Call once, which places it through the server's `place#PhoneCall` and its checks (for cost and fraud safety). The current account's Dialer, or the one account that can call; when several can and the current one cannot, the user picks which (`call_dial_choice`), and no account is switched without that pick. Numbers Android treats as emergency numbers never reach it (Telecom keeps them on the SIM) | `dial::nonce_of`, `native::take_dial_link`, `host::open_dial` |
| A room link (mobile, `org.moqui.mobile:/room?account=…&chatRoomId=…&eventSeq=…&sig=…`, a chat notification's tap) | Taken only when its `sig` is this app's: an HMAC-SHA256 of the account, room and message with a 32-byte key the app keeps in its key store (`app.link-key`), compared in constant time. It then opens the comms Chat screen at the room for an account signed in here (switching to it); another app's link opens nothing | `calls::room_link_of`, `native::room_link_taken`, `host::open_room_link` |

The comms root is, in order: a client account's server's; the file `comms-root` a node page wrote with `set_comms_root`; `comms_app_root`; `/qapps/BasicComms`. The page opens on the node's origin or, in client mode, in the shell or an app window of the current account.

A second launch of the desktop app sends its links to the first over the Unix socket `<config>/org.moqui.desktop/instance.sock`. The first instance sets the socket to mode `0600` after `bind`. A message is at most 64 KiB, must start with the line `moqui-desktop launch 1`, and each other line is used only when it parses as a URL of one of the four schemes (`crates/moqui-desktop/src/os/instance.rs`). What a local process of the same user can send is therefore the same links the OS hands over.

## Sign-in of an installed app

Password sign-in: the shell calls `client_sign_in`; the app posts to the server and keeps the device key (`moqui_client::login::sign_in`, not read). SSO: `client_sso_start` makes a 48-byte random PKCE verifier and a 24-byte random state, opens the system browser at the server's `/sso/appLogin` with the S256 challenge, the state and the redirect `org.moqui.desktop:/sso` or `org.moqui.mobile:/sso`, and keeps one pending sign-in. The hand-back link is matched on its state, refused when stale, and its code is sent with the verifier to `/sso/appToken` (`crates/moqui-client/src/sso.rs`). The verifier stays in the app. The server's `/sso/appLogin` and `/sso/appToken` routes are in [Attack-Surface-Routes.md](Attack-Surface-Routes.md).

The server address the user types decides where the password and the device key go. `base_url` accepts `https` to any host and `http` only to `localhost`, `127.0.0.1` and `[::1]`.

## Device keys and the offline store

| What | Where | Protection |
|---|---|---|
| Device key, desktop | The OS key store (Keychain, Credential Manager, Secret Service), one entry per account under `org.moqui.desktop`. When the OS store does not answer, with `MOQUI_DESKTOP_KEY_STORE=file`, and in a development run: `<dir>/<account>.key` | The file is mode `0600` in a directory made `0700` (`crates/moqui-client/src/keys.rs`, `crates/moqui-client/src/files.rs`, `crates/moqui-desktop/src/client/keystore.rs`) |
| Device key, Android | `client/keys/<account>.sealed` in the app's private files, sealed with an AES-GCM key of the Android Keystore; when the Keystore refuses, the plain file store in the same directory | `crates/moqui-mobile/src/keystore.rs` (`KeyVault.kt` not read) |
| Chat store passphrase | The key store, by account and crypto device | as the device key |
| Account list | `accounts.json` | mode `0600` |
| Offline store | `data/<account>.db` (SQLite: kept answers, chat rooms and events, mail folders, threads and messages, the outbox, drafts, sync cursors, uploads) and `data/<account>.files` | The directory is made `0700` and the database `0600`. The data is not encrypted by the app (`crates/moqui-client/src/store/mod.rs`) |
| Node session cookie, desktop node in process | `node-cookies` | mode `0600` |

The store keeps `GET` answers with a JSON content type and status 200 for the shell's data and the comms app's `/Chat/*`, `/Inbox/Mail/*` and `/Inbox/Compose/composeData` paths (`classify` in `crates/moqui-client/src/offline.rs`). A sign-out revokes the key on the server when it answers and deletes it here. A shared-mode account's store, chat store keys and account entry are deleted at sign-out; a personal account's data stays until the account is removed (`signed_out`, `remove_account`). A 401 to a request with the key starts a check of the key, and a key that no longer signs in signs the account out here (`check_key`). Keys with less than a third of their life left are renewed at start and every six hours through `/qapps/renewClientDevice` (`crates/moqui-client/src/renewal.rs`).

## Helper processes of the desktop app

| Process | Arguments | From untrusted input |
|---|---|---|
| `xdg-open` or `open` | one URL | The URL of a page's navigation or of `open_external`: `http`, `https`, `mailto`, `tel`, `sip` only, parsed as a URL first |
| `xdg-mime default`, `update-desktop-database` | the app's `.desktop` file, fixed MIME types, a directory | no |
| `hostname` | none | no |
| The sidecar `moqui` | `-D name=value` pairs the app builds | no |
| `moqui save-db`, `load-save` and the folder opener (`crates/moqui-desktop/src/backup.rs`) | not read | not read |

## The mobile app on Android

`crates/moqui-mobile/gen/android/app/src/main/AndroidManifest.xml` declares these components.

| Component | Exported | Reached by |
|---|---|---|
| `.MainActivity` (`singleTask`) | yes | The launcher, a `VIEW` intent for `org.moqui.mobile:/sso` (browsable: any app or web page can send one), and one for `/room` (not browsable: any app can still send one). The app takes an SSO link only for a waiting sign-in of the same state, a call link or a dial link only with a nonce the native code made, and a room link only with the app's own signature (`on_event`, `crates/moqui-mobile/src/lib.rs`) |
| `.CallConnectionService` | yes | Only a holder of `android.permission.BIND_TELECOM_CONNECTION_SERVICE` (the system). It has the self-managed calls and the managed `tel` calling account (`DialAccount.kt`): a number dialed with that account is handed to the app by a dial link and never called by Telecom |
| `.IncomingCallActivity`, `.CallService`, `.CallActionReceiver`, `.ChatActionReceiver` (a chat notification's Reply, Mark read and Mute, through `PendingIntent`s of the app; the account, room and message are the intent's data, which a Reply's mutable intent cannot have filled in), the `FileProvider` | no | The app |
| `.PushService` (the `fcm` source set) | no | Firebase messaging events |
| `.UnifiedPushService` | no | The UnifiedPush connector's events (an account's endpoint, a message), handed over by the connector's own receiver `org.unifiedpush.android.connector.internal.MessagingReceiverImpl` (exported, as a distributor app must reach it). The connector's receiver tries its default key manager, which has none of the account's keys, so the message comes sealed; `UnifiedPushService` opens it in Rust (`Native.openFor`) with the push keys of the account its instance names, and drops one that does not open or that the connector says it opened (sealed to the connector's own keys): only a push sealed to that account's keys, which only its server was given, shows |
| `org.unifiedpush.android.connector.internal.RaiseToForegroundService`, `.LinkActivity` (the connector's) | the service yes | A distributor raising the app to the foreground to deliver; the link activity picks a distributor |
| `.DebugRingReceiver` (the `debug` source set only) | yes | Only a holder of `android.permission.DUMP` (adb) |

The app asks for `INTERNET`, `POST_NOTIFICATIONS`, `USE_FULL_SCREEN_INTENT`, `MANAGE_OWN_CALLS`, `FOREGROUND_SERVICE`, `FOREGROUND_SERVICE_PHONE_CALL`, `FOREGROUND_SERVICE_MICROPHONE` and `FOREGROUND_SERVICE_CAMERA` (a call's service keeps its media when the app is left, taken only while the app is in a call), `WAKE_LOCK` (the screen off at the ear), `ACCESS_NETWORK_STATE`, `READ_CONTACTS` (the phone's contacts' names and the new-chat search, asked at the first search, read on the phone only), `RECORD_AUDIO`, `CAMERA`, `MODIFY_AUDIO_SETTINGS` and `REQUEST_IGNORE_BATTERY_OPTIMIZATIONS` (the notification check's battery fix, which the system asks the user). `android:allowBackup` is `false`. `android:usesCleartextTraffic` is a build placeholder: `false` by default and `true` in the `debug` build type (`crates/moqui-mobile/gen/android/app/build.gradle.kts`, lines 45 and 58).

## The desktop updater

The updater plugin is registered only in a build whose config has a `plugins.updater` section with a `pubkey` and at least one endpoint, every endpoint an `https://` URL (`has_feed`); `./moq desktop build` adds it when `MOQUI_DESKTOP_UPDATER_PUBKEY` and `MOQUI_DESKTOP_UPDATER_ENDPOINT` are set (`crates/moqui-desktop/src/updates.rs`, lines 85-100). A check runs only from the tray item, and choosing "Install update" is the confirmation. No capability grants the plugin's commands to a page.

## The browser shell and untrusted content

The shell scripts are in `runtime/components/base/webroot/screen/webroot/js/`. The server's default `Content-Security-Policy` for a screen render is `frame-ancestors 'none'; form-action 'self';`, with `X-Frame-Options: sameorigin`, `X-Content-Type-Options: nosniff` and `X-XSS-Protection: 1; mode=block` (`crates/moqui-core/src/context/factory.rs`, lines 85-98). That policy names no script source, so it does not limit inline script or `eval` in a page; what keeps markup from data out of a page is the escaping and the sanitizers below. A render for the screen builder or a preview overlay sets `frame-ancestors 'self'` instead (`crates/moqui-core/src/screen/render.rs`). In the desktop and mobile apps that default policy has no script directive and is therefore not passed to the page at all (`has_script_policy`). The client shell of the apps has its own policy: `default-src 'self'`, `script-src` limited to the shell's `/shell/`, `/js/` and `/libs/` paths with `'wasm-unsafe-eval'`, no inline script, `frame-src 'none'` unless `moqui_shell_frame_origins` lists origins, `object-src 'none'`, `base-uri 'none'` (`SHELL_CSP`, `crates/moqui-client/src/assets.rs`).

Places where HTML from data is put into the page (`v-html`, `innerHTML`):

| Where | Source of the HTML | Cleaning |
|---|---|---|
| `m-markdown` (`WebrootVue.qvt.js`, near 7627 and 7650) | Markdown text rendered by `marked` | DOMPurify with `ADD_ATTR: ["target"]`, forbidding `style`, `form`, `input`, `button`, `textarea`, `select` tags and the `style` attribute |
| Markdown editor preview (near 3380) | The text being edited | DOMPurify with its defaults |
| Assist thread and canvas Markdown (`WebrootAssist.js`, near 319 and 358) | Model output | DOMPurify forbidding `style`, form controls, `video`, `audio`, `source`, `picture`, `iframe`, `object`, `embed`, `svg`, `math` and the `style`, `srcset`, `poster`, `background` attributes; then `cleanTree` replaces every image that is not a path of this site with a placeholder. In strict mode (a canvas) no image is kept |
| `m-html-view` (`WebrootVue.qvt.js`, 8171-8560) | An email's HTML body | DOMPurify HTML profile, no `data-` attributes, forbidding `style`, forms, frames, `object`, `embed`, `base`, `link`, `meta`, media, `script`, `template` and `srcset`, `poster`, `ping`, `formaction`, `action`, `srcdoc`. The result is shown in an `iframe` with `sandbox="allow-same-origin allow-popups allow-popups-to-escape-sandbox"` (no `allow-scripts`) and `referrerpolicy="no-referrer"`. An `http` or `https` image is dropped until the user chooses "Show images"; a `data:image/` source and a `cid:` source of one of the message's parts are kept. A link keeps only an `http`, `https`, `mailto`, `tel` or `#` target |
| Encrypted chat message (`WebrootChat.js`, 664-675) | HTML decrypted in the page | DOMPurify with a fixed list of allowed tags and attributes |
| `m-editable` display (`WebrootVue.qvt.js`, 1762 and 1817) | The `label-value` attribute the server wrote; after a save, the new text through `moqui.htmlEncode` | The saved text is encoded, and the server encodes `label-value` unless the screen sets `encode="false"` (SEC-SCR-08) |
| Code view lines (near 7802 and 7821) | `hljs.highlight` output split into lines | highlight.js output |
| Shell footer (`shell.footerHtml`, near 9564) | The shell configuration from the server | none in the page |
| `moqui-html.js` (lines 288, 392, 760) | HTML the server rendered for the plain HTML render mode | none in the page |
| `chat.plainText` (`WebrootChat.js`, 649) | The sender's own composer HTML, set on a detached element to read its text | none; its only caller is `sealMessage` |

Message handlers: `MoquiLib.js` `oauthPopup` accepts a `message` event only from the page's own origin and from the popup it opened, and only with the sign-in id it started; the same data is accepted on the `BroadcastChannel` `moqui-email-oauth`, which only same-origin pages reach. `WebrootAssistSandbox.js` (inside the model-output frame) accepts one `message` from `window.parent` carrying the frame's nonce and a port, and talks over that port after; `WebrootAssistCanvas.js` posts that port with target `*` and the nonce. The frame's document and its policy are in [Attack-Surface-AI.md](Attack-Surface-AI.md). `WebrootCrypto.js` and the session token use a `BroadcastChannel` among same-origin tabs.

The Dialer screen (`runtime/components/base/basic-comms/screen/BasicComms/Calls/Dialer.qvue`) puts a `number` URL parameter into its field and searches; it places no call until the user chooses Call.

### The service worker

`/sw.js` (`runtime/components/base/webroot/screen/webroot/sw.js`) answers a `GET` on its own origin with no `Authorization` header from the cache `moqui-shell-v2` first, and otherwise fetches and keeps the answer, when the path starts with `/libs/`, or is a static file by its extension (script, style, map, font, image, wasm) with a `v` query parameter outside `/rest/`, `/rpc/`, `/apps/`, `/qapps/`, `/dav/`, `/git/`, `/xmpp-upload/`, `/s/`, `/u/` and `/r/` (`cacheable`). It keeps an answer only when it is a 200 of type `basic` whose `Cache-Control` names `public` and none of `private`, `no-store` or `no-cache` (`keepable`; SEC-CLI-02). Activating the worker deletes every other cache, `moqui-shell-v1` among them. A screen render carries `no-cache, no-store, must-revalidate, private` and is not kept. A kept answer is not asked for again. A push shows a notification; a click on it focuses or opens a window at the notification's link when that link is on the worker's origin, else at `/qapps`.

## Open questions

These are gaps in this pass. A line that was read is stated above and is not repeated here.

- Whether a frame of another origin inside a window (an embedded page in a server's page) can call a command was not traced: it rests on how Tauri and `tauri-runtime-cef` decide the calling origin, which was not read.
- `request_own_origins` (`crates/moqui-core/src/web/origin.rs`) counts the request's own `Host` as an allowed `Origin`; on the node's listener that `Host` is one of the two allowed names.
- Whether a desktop node starts with user accounts (so that the first-administrator form is closed) was not traced.
- What Chromium does with a navigation of the main window to a `file:`, `data:` or `chrome:` URL that `is_external` lets through was not tested.
- `crates/moqui-desktop/src/bridge.js`, `download.rs`, `pdf.rs`, `wake.rs`, `backup.rs` (but for `tool_args`), `os/autostart.rs`, `os/entries.rs`, `os/first_run.rs` and `runtime_dir.rs` were not read.
- `crates/moqui-mobile/src/native.rs`, `calls.rs`, `android.rs`, the Kotlin sources (`KeyVault.kt`, `PushService`, the WebView settings) and what an FCM data message makes the app do were not read.
- `crates/moqui-client`: `login.rs`, `socket.rs`, `http.rs` (TLS settings), `sync/` (the outbox kinds and what `outbox_add` may send), `cache.rs`, `conf.rs` and the terminal client were not read. The path check of `socket_open` was not read.
- `crates/moqui-browser` and `crates/moqui-embedded` were not read.
- Which signed-in `GET` answers carry no `private` or `no-store` and can be kept by the service worker through a `v` parameter was not traced. Whether the cache is cleared at sign-out was not traced.
- The updater's endpoint handling, signature check and version comparison are the plugin's and were not read.
- The server's encoding of `m-editable`'s `label-value`, the plain (not encrypted) chat room's message path, the rich text editor, Web Push registration (`WebrootPush.js`), the manifest, client-side routing redirects and third-party script sources were not read.
