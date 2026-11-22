# 11. Desktop

Part of [Agent Code](README.md). T3 sources: `t3:apps/desktop/src/**`, `t3:packages/contracts/src/ipc.ts`
(`DesktopBridge`, `DesktopPreviewBridge`, `LocalApi`), `t3:packages/ssh/src/**`, `t3:scripts/build-desktop-artifact.ts`,
`t3:docs/internals/remote.md`. MR Moqui sources: `crates/moqui-desktop/README.md`,
[docs/ui/Desktop-Client.md](../../docs/ui/Desktop-Client.md), [plans/Mobile-Desktop.md](../Mobile-Desktop.md).

The desktop app is the existing Tauri 3 app on CEF (`crates/moqui-desktop`). Agent Code adds no second desktop app.
All new desktop code goes in `crates/moqui-desktop/src/code/`, one file per area, as T3 has one file per area in
`apps/desktop/src/ipc/methods/`.

## 1. Modes

| T3 | MR Moqui desktop | Agent Code in that mode |
|---|---|---|
| Local backend as a child process (`ELECTRON_RUN_AS_NODE`) | **Sidecar node** (MD1): `moqui` on loopback | The node runs the orchestrator, providers and PTYs. The window shows `/qapps/code` of the node. |
| (none) | **In-process node** (MD2, the default): node in the app process, no port, app scheme | Same, with no port. `/agentws` runs over the in-memory pipe as `/termws` does today. |
| `localEnvironmentEnabled` off (thin client) | **Client mode** (MD3-MD5): accounts on remote servers | No local node. Each account is an environment. The host holds `/agentws` per account (§3). |
| WSL backend (Windows) | **WSL node** (new, phase 4): a Linux `moqui` sidecar in a WSL distro | §8.2. |
| SSH environment | **SSH launch** (new): start or reuse `moqui serve` on a remote host over SSH | Desktop side in §8.1, server side in [13 §7](13-Servers-Linking.md#7-ssh-launch). |

- A local node and client accounts can be on at the same time (T3: primary plus saved environments). The node is the
  `primary` environment. The tray item **Use as a client of a server** turns the node off (thin client).
- `agent_code_home` on the desktop node is the app data directory `agent-code/`
  ([02 §2](02-Orchestration.md#2-thread-creation-and-launch)).
- Login to the local node: the in-process window uses the node's own session (cookie in the host), so approvals stay a
  person's. Agent Code keeps that rule. No login key header on the in-process window.

## 2. Host API

T3 has `DesktopBridge` (IPC) and `LocalApi` (web fallback). MR Moqui has `window.moquiDesktop` commands, granted
only to the exact origin of the node or the account. Agent Code adds `moqui.host.code`, a JS object in
`AgentCode.js` that calls Tauri commands on the desktop, Android commands on the phone, and web APIs in a browser.

### 2.1 Mapping

| T3 `DesktopBridge` area | MR Moqui command (`crates/moqui-desktop/src/code/`) | Browser fallback |
|---|---|---|
| getAppBranding, getClientPlatform, getSystemLocale, getPreferredSystemLanguages | `code_platform` in `platform.rs` | `navigator` |
| getPathForFile, startFileDrag, readMedia | `code_file_path`, `code_file_drag`, `code_read_media` in `files.rs` | file input, download |
| setNotificationBadge, onNotificationBadgeClear | existing `set_counts` (tray and dock badge) with an `agentCode` count | tab title count |
| revealWindow, getWindowFullscreenState, getWindowZoomFactor, onQuitShortcut | `window.rs` | none |
| getLocalEnvironmentBootstraps, get/setLocalEnvironmentEnabled, getHostEnvironmentId, getLocalEnvironmentBearerToken | `code_environments` in `environments.rs` (list of primary node and accounts). No bearer token goes to the page; the host attaches keys. | `/.well-known/moqui/agent-code` |
| getClientSettings, setClientSettings | `code_client_settings` (device settings of [10 §9.2](10-Web-UI.md#9-settings)) in the app config directory | IndexedDB |
| getConnectionCatalog, setConnectionCatalog | existing `client/accounts.json` plus SSH and WSL entries in `code/catalog.json` | none |
| SSH (discoverSshHosts ... resolveSshPasswordPrompt) | `ssh.rs` (§8.1) | none |
| Snap Shot | `snapshot.rs` (§6.1) | none |
| Server exposure, Tailscale | existing tray **Open ports for other devices** plus `exposure.rs` (§8.3) | none |
| WSL | `wsl.rs` (§8.2) | none |
| pickFolder, pickProjectFavicon, pickThemeFiles, setTheme | `pickers.rs` (Tauri dialog) | `showDirectoryPicker` where it exists, else a server path field |
| showContextMenu, onMenuAction, openExternal | `menus.rs`; `open_external` exists (http, https, mailto, tel, sip) | DOM menu, `window.open` |
| receiveProviderAuthCallback, cancelProviderAuthCallback | `provider_auth.rs`: a loopback listener for a provider's OAuth callback (Codex sign-in), as the SSO loopback | the server's own page |
| openSystemSettings, checkSystemPermission | `permissions.rs` (macOS screen capture, microphone, camera, local network) | none |
| probeRemoteEditors, open in editor | `editors.rs`: finds VS Code, Cursor, Zed, JetBrains, opens `path:line`; `vscode-remote://ssh-remote+host` for SSH environments | hidden |
| pasteAsText, getPrimaryPasteEnabled | `clipboard.rs`; Linux primary selection | none |
| Updates | existing `updates.rs` (§9) | none |
| cliCommand (install a `t3` shim) | `cli.rs`: installs a `moqui` shim on the PATH that runs the app's bundled binary | none |
| webLinks, appActivation | existing `os/links.rs`, `os/instance.rs`; adds `moqui://code/...` links | none |
| appleSpeech | `speech.rs` on macOS 26+ (later, [07 §4](07-Secrets-Guest-Voice-Usage.md#4-voice-input)) | server speech |
| preview (`DesktopPreviewBridge`) | `preview/` (§5) | server-run preview tabs |

### 2.2 Rules

- Every command checks the calling origin as `bridge.rs` does now: the node origin or the open account origin.
- A command that takes a path checks it is under a project root of the environment (as the server checks
  `shell.openInEditor`).
- No command returns a credential to the page.
- The commands are part of the host capability list in the descriptor (`{kind: "desktop"}`,
  [09 §6](09-Wire-API.md#6-environment-descriptor)), so the page hides what the host cannot do.

## 3. Sockets and environments

- In-process and sidecar node: the page opens `/agentws` directly (same origin). In-process it runs over the pipe.
- Client mode: a page socket cannot set `login_key`, so the host holds `/agentws` per account and bridges frames, as
  it does for `/notws`, `/callws` and `/termws` (`crates/moqui-client/src/socket.rs`: add `/agentws` to `PATHS`).
  Binary sub-streams go base64 between page and host, as `/termws`.
- The app scheme answers SSE with 503. Agent Code uses no SSE, so this does not matter.
- Several environments in one window ([10 §3](10-Web-UI.md#3-environments-in-one-page)): the host holds one
  `/agentws` per account at the same time. Today switching accounts closes the other account's sockets. For Agent
  Code, accounts marked **Show in Agent Code** stay connected in the background (shell stream only, no app windows).
  This is a change to `client/windows.rs` and `moqui_client::socket`.
- Reconnect: the T3 rules ([13 §3](13-Servers-Linking.md#3-pairing-and-sessions)): jittered backoff to 5 minutes,
  probe on wake, network change and focus. `wake.rs` already tells the host of a wake.
- Offline: drafts and queued messages go to the outbox kind `agent-message`
  ([12 §6](12-Mobile.md#6-offline)). The shell stream snapshot is kept in the account store so the sidebar shows
  when the server is offline.

## 4. Windows, menus, tray and quit

### 4.1 Windows

- The main window shows `/qapps/code` when the user opens Agent Code (a tray item and an app menu item, plus the
  Apps menu).
- **Open in new window** on a thread (T3 has one window; MR Moqui app windows already exist): one window per thread is
  allowed. The window title is the thread title (`screenReaderWindowTitle`).
- Closing the window hides it to the tray (existing rule). Runs go on in the node.

### 4.2 Menus

| T3 menu | MR Moqui |
|---|---|
| App (macOS): About, Check for Updates, Settings, Services, Hide, Quit | Existing macOS menu plus **Agent Code Settings** (`Settings/General`). |
| File: Settings, Close Developer Tools, Close Window / Quit | Adds **New thread** (`chat.new`), **New window**. |
| Edit: Undo ... Paste as Text, Speech | Adds **Paste as Text** (`pasteAsText`). |
| View: Reload, Force Reload, DevTools, zoom, full screen | Existing CEF items. Zoom keys only when the focus is not in the preview (the preview has its own zoom). |
| Window: Minimize, Zoom, Bring All to Front | Same. |
| Help: Check for Updates | Same. |

Menu actions reach the page as the keybinding command names of [10 §8](10-Web-UI.md#8-keybindings).

### 4.3 Tray and badge

T3 has no tray. MR Moqui has one. Agent Code adds:

- A count of threads that wait for the user (approval, question, secret request) on the tray and the dock badge
  (`set_counts`, key `agentCode`).
- Tray menu: **Agent Code** (open), **Waiting for you (n)** (opens the first one), **Pause automations** (sets
  `AgentTrigger` pause for the user for 1 hour; [05 §6](05-Automation.md#6-services-rpcs-and-tools)).
- Native notifications through the existing `notify` command; a click opens the thread link (existing rule).

### 4.4 Quit

- `confirmQuit` (device setting): direct, hold (default; hold `Cmd+Q`/`Ctrl+Q` 1.2 s), double-click (twice within
  500 ms). An overlay shows the hold progress.
- When runs are active, quit asks **Quit and stop N runs?** In-process and sidecar nodes stop their runs as an
  interrupt ([02 §4](02-Orchestration.md#4-messages-dispatch-modes-and-the-queue)). `continueThreadsAfterServerUpdate`
  resumes them on the next start.
- Quit order: stop new turns, send interrupt to providers (5 s), close PTYs (terminal sessions with term-hold keep
  their hold), stop the node, stop Postgres. This keeps the existing order and adds the first two steps.

## 5. Preview view

T3 owns the preview in the Electron main process (`apps/desktop/src/preview/`). MR Moqui uses the CEF runtime that
the desktop already has. Runtime `desktop` of [06 §2](06-Terminal-Preview-Devices.md#2-browser-preview-and-automation).

### 5.1 Parts

| T3 part | MR Moqui (`crates/moqui-desktop/src/code/preview/`) |
|---|---|
| Manager, BrowserSession | `manager.rs`, `session.rs`: one CEF browser per preview tab, placed as a child view over the right panel rectangle the page reports. |
| registerWebview, createTab, closeTab, setShown, setCaptureDrawn | `tabs.rs`. The page sends the panel rectangle on resize and scroll. Hidden tabs keep their browser up to 8 per window, then the oldest unloads. |
| navigate, back, forward, refresh, stop, hardReload, zoom, setColorScheme, setAudioMuted, find | `nav.rs` on CEF browser host calls. |
| Profiles, clearCookies, clearCache | `profiles.rs`: one CEF request context per `AgentBrowserProfile`, cache path under the app data `agent-code/browser/<profileId>/`. Ephemeral profiles use an in-memory context. |
| CdpRelay | `cdp.rs`: a DevTools protocol session per tab. The node's `preview_*` tools reach it over `/agentws` stream `preview.cdp` ([08 §2](08-MCP-Tools.md#2-tool-list)). The PDF printer already uses DevTools on a hidden CEF window, so the access path exists. |
| LoopbackTunnel, port forwards | `tunnel.rs`, `forward.rs` ([06 §2.1](06-Terminal-Preview-Devices.md#21-model)). |
| Pick preload, element picking, annotations, annotation voice | `pick.rs` plus a script injected into the preview frame with CEF `ExecuteJavaScript` on a separate world. The pick result (selector, text, box, screenshot crop) goes to the composer as a context chip. |
| Recording (screencast, input overlay, save) | `record.rs`: CDP `Page.startScreencast` frames to a WebM by the bundled encoder, with key and mouse overlays (`browserRecording*` settings). Saved as a thread attachment. |
| Screenshots, revealArtifact, copyArtifactToClipboard | `artifacts.rs`. |
| Picture-in-picture (mini player) | `pip.rs`: a small always-on-top window with the same browser. `browserAutoShowFloatingPreview`. |
| Passkeys (macOS keychain group) | Later. CEF passkey support on macOS needs the entitlement and a signed app. Until then, passkeys work in a profile only by the system browser. |
| BrowserImport (cookies from local browsers) | Later ([06 §2.5](06-Terminal-Preview-Devices.md#25-other-browser-features)). |
| openDevTools | CEF DevTools window. |

### 5.2 Rules

- The preview browser has no access to `window.moquiDesktop` and no Tauri IPC. It is a separate CEF browser, not an
  app webview.
- A tab of the local node goes direct to loopback. A tab of a remote environment uses the SOCKS5 tunnel
  ([06 §2.1](06-Terminal-Preview-Devices.md#21-model)); only loopback goes through the tunnel.
- `browserLinkTarget` system or app: a link from a thread opens in the system browser or a preview tab.
- Agent control needs `enableAgentBrowserAccess` and the project override. The user sees a pill **Agent is using
  this tab** and can take back control.
- Keys in the preview: `previewFocus` true; `setForwardedShortcuts` sends the keys of
  [10 §8](10-Web-UI.md#8-keybindings) with `previewFocus` back to the page.

## 6. Other desktop features

### 6.1 Snap Shot (window capture)

- Shortcut (default `snapShotShortcut`) captures another app's window and its accessibility tree, plays an animation
  into the composer and adds it as an attachment.
- `snapshot.rs` per platform: macOS ScreenCaptureKit and AX API; Windows Graphics Capture and UI Automation; Linux XDG
  desktop portal (screenshot) for all desktops, and the accessibility tree by AT-SPI. T3's GNOME shell extension,
  KDE, Hyprland and Niri helpers: not ported in phase 1; the portal covers them.
- Settings `snapShot*` (device). Off by default. macOS asks the screen capture permission once.

### 6.2 Files and editors

- Drag a file from a thread to the OS (`startFileDrag`): local node only, or a downloaded copy for a remote one.
- Drag files into the composer: Tauri file drop gives paths; the host reads them and uploads
  (`attachments.createUploadUrl`).
- Open in editor (`mod+o` favorite editor): local node opens the path; SSH environment opens `vscode-remote`; other
  remote environments hide it.

### 6.3 CLI shim

- Settings > General **Install command line tool** puts a `moqui` shim on the PATH (macOS `/usr/local/bin` with a
  prompt, Linux `~/.local/bin`, Windows the user PATH). The shim runs the app's bundled `moqui` with the node's
  `runtime` directory, so `moqui code ...` ([13 §11](13-Servers-Linking.md#11-cli)) talks to the local node.

### 6.4 Accounts and sign-in for providers

- Provider sign-in (Codex `login`, Claude `login`, Cursor) runs on the node host. On the desktop node the host's
  `provider_auth.rs` catches the loopback callback and gives it to the node. On a remote server the sign-in uses the
  device code flow or a pasted token ([03 §1](03-Providers.md#1-drivers-and-instances)).

## 7. Power and sleep

| T3 | MR Moqui |
|---|---|
| `keepAwakeWhileAgentsWork` (server setting, true) | The desktop host holds an OS sleep inhibitor while any run of the local node is `starting` or `running`: macOS `IOPMAssertionCreateWithName` (`PreventUserIdleSystemSleep`), Windows `SetThreadExecutionState(ES_CONTINUOUS \| ES_SYSTEM_REQUIRED)`, Linux `org.freedesktop.login1.Manager.Inhibit("sleep:idle", ..., "block")`. The display can sleep. File `code/power.rs`. |
| Background activity profile: pause when locked or on battery | `power.rs` reports `onBattery`, `locked`, `lowPower` to the node by the host command `code_power_state` and on change. The node's `backgroundActivity` scheduler ([02 §6](02-Orchestration.md#6-usage-limit-continuation-and-background-work)) slows git fetch and provider health checks: battery-saver x4 interval, paused while locked. |
| Wake | `wake.rs` already finds a wake by the wall and monotonic clocks. On wake the host probes each `/agentws` and the node resumes timers. Automations that were due during sleep run once (not once per missed slot, [05 §3](05-Automation.md#3-scheduler)). |
| Laptop lid closed | The inhibitor does not keep a closed laptop awake (OS rule). The node marks runs that were cut by sleep `interrupted` and `autoResumeLimitedThreads`-style resume runs them on wake if `continueThreadsAfterServerUpdate` is true. |

- The inhibitor reason text is "Agent Code: N agents working".
- Client mode: no inhibitor (the work runs on the server).

## 8. SSH, WSL and exposure

### 8.1 SSH launch (desktop side)

- **Add environment > SSH host**: the host reads `~/.ssh/config` (`Host` entries, no wildcards) and lists them
  (`discoverSshHosts`), or the user types `user@host:port`.
- The desktop runs the system `ssh` binary (as T3) so the user's agent, keys, `ProxyJump` and config work. It does not
  use the built-in `moqui_core::sftp` client (that one pins host keys for server jobs and never reads `~/.ssh`).
- Password and passphrase prompts: `SSH_ASKPASS` points at the app's helper, which asks the page
  (`onSshPasswordPrompt`) and answers. The prompt shows the host and the prompt text.
- The remote steps (lock, install, port scan, start, runtime record, owner flag) are in
  [13 §7](13-Servers-Linking.md#7-ssh-launch).
- The desktop forwards the remote port to a local loopback port (`ssh -N -L`), probes every 100 ms up to 90 s, and
  adds the environment to `code/catalog.json` with kind `ssh`.
- Sign-in over the tunnel: a one-use pairing token that the remote launch script prints (as `t3 pair`); the host
  trades it for a device login key ([13 §3](13-Servers-Linking.md#3-pairing-and-sessions)).
- On reconnect the host restores the forward first, then the socket.
- Remove: stops the remote server only when `serverKind` is `managed` (this launcher started it).

### 8.2 WSL node (Windows)

- Settings > General > **WSL**: enable, pick the distro, **WSL only** (no Windows node).
- The host runs the Linux `moqui` binary (bundled for x86_64 and aarch64) inside the distro as a sidecar on a loopback
  port; WSL forwards loopback. `WSLENV` gets the needed variables.
- `toLocalBackendPath` maps `C:\...` to `/mnt/c/...` for file drops and open-in-editor.
- A fatal failure falls back to the Windows node and is kept; a transport failure keeps the WSL choice (T3 rule).

### 8.3 Exposure

- Existing tray item **Open ports for other devices** opens HTTP, SSH and cluster ports. Agent Code adds the
  advertised endpoints list (LAN addresses, Tailscale name) for phone pairing
  ([13 §8](13-Servers-Linking.md#8-relay-and-tailscale)).
- **Tailscale Serve** toggle: runs `tailscale serve --bg --https=443 http://127.0.0.1:<port>` for the sidecar node.
  The in-process node has no port; the toggle switches the node to sidecar first and says so.

## 9. Updates and packaging

| T3 | MR Moqui |
|---|---|
| Channels `latest`, `nightly` | Existing Tauri updater (`updates.rs`) with a channel setting: `stable`, `nightly`. The feed URL gets `/{channel}/`. |
| `installUpdate(expectedVersion)` | The page passes the version it showed; the host installs only that one. |
| Remote-triggered desktop update (`DesktopRemoteUpdates`, prepared update expires in 5 min) | `server.commitDesktopUpdate` on `/agentws` from another device of the same user, for the in-process node: check, download, install, wait, done. Needs a fresh confirm on the desktop when the desktop is not locked; else it waits. |
| Update stops runs | Install stops the node (existing). Runs resume after start (`continueThreadsAfterServerUpdate`). |
| macOS dmg + zip, Linux AppImage + deb, Windows NSIS; signing | Existing packaging. No change. |
| Bundled `t3` server | The bundled `moqui` binary. Provider CLIs are not bundled; `refreshProviders` finds them on the PATH and offers install ([03 §4](03-Providers.md#4-the-process-host)). |
| Rust native helpers per triple | The SSH askpass helper and the Snap Shot helper are part of the app binary (`moqui-desktop --askpass`, `--snapshot-worker`). |
| Open source licenses | The about page lists T3 Code (MIT) and the libraries ported or bundled. |

## 10. Platform differences

| Platform | Agent Code specifics |
|---|---|
| macOS | Sleep assertion; ScreenCaptureKit Snap Shot; permission page for screen capture, microphone, local network; Apple Speech (later); `fastWorktreeCheckout` with APFS clones. |
| Linux | logind inhibitor; XDG portal Snap Shot; primary selection paste; Secret Service for keys (existing). |
| Windows | `SetThreadExecutionState`; WSL node; Windows Graphics Capture; `ConPTY` for terminals (existing in the PTY layer). |

## 11. Tests

See [15](15-Tests.md). Desktop specifics:

- Rust unit tests beside each `code/*.rs` (`*_tests.rs`, as the crate does now): origin checks, path checks, catalog
  read and write, askpass prompt round trip, power state mapping, quit order.
- Preview: CEF is not in the regression suite. One acceptance walk on an installed app opens a preview tab, picks an
  element and runs `preview_snapshot` from a thread.
