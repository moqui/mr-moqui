# 06. Terminal, browser preview, HTML renders, MCP Apps and devices

Part of [Agent Code](README.md). T3 sources: `t3:docs/user/terminal.md`, `t3:docs/internals/terminal-runtime.md`,
`t3:docs/user/remote-access.md`, `t3:docs/internals/remote.md`, `t3:docs/user/html-renders.md`,
`t3:docs/user/devices.md`, `t3:docs/internals/devices.md`, `t3:packages/contracts/src/{terminal,preview,
previewAutomation,browserProfile,device}.ts`.

## 1. Terminal

### 1.1 What is reused

The MR Moqui terminal is built ([docs/ui/Terminal.md](../../docs/ui/Terminal.md)): `/termws`, `TerminalSession`,
`moqui term-hold` (sessions survive a server restart), `moqui term-exec`, attention tracking (OSC 7501, OSC 133,
bell), flow control and the phone key bar. Agent Code uses it. It does not add a second PTY stack.

What thread terminals need beyond that is the framework's, planned in [Terminal-Fancy2](../Terminal-Fancy2.md) (Nov 27,
2026): the `thread` kind with a policy this component registers (TF7), viewers (TF4), history as a bounded recording
(TF5), the screen as text and driven sessions (TF2, TF3), and OS accounts (TF6). The rows below say which part gives
each.

### 1.2 New: thread terminals

| T3 rule | MR Moqui |
|---|---|
| The environment server owns PTYs and output. Every client attaches through the connection. | Same: `/termws` from web, desktop, phone and peer links. |
| Terminal ids are chosen by the client. Key `{threadId, terminalId}`, at most 128 chars. | Kind `thread` (Terminal-Fancy2 TF7) with `conversationId` + `clientTerminalId` (unique together among a user's live sessions: an open of an existing one answers it). `open#TerminalSession` gains `conversationId`, `clientTerminalId`, `label`, `env`, `workRootId`, `providerInstanceId`, `stopOnSettle`; the worktree path is its `directory`, checked by this component's policy. |
| `cols` 1-1000, `rows` 1-500 | TF7 raises the top from 500x500 to 1000x500 for all kinds and keeps the bottom (20x10). |
| `env` at most 128 keys, value at most 8,192 chars | TF7's check in `open#TerminalSession`. Names of Moqui conf properties are still removed. |
| Attach: `restartIfNotRunning?`, `resume?`, `maxHistoryBytes?` | TF7: the attach frame gains `restart` and `maxHistory`; `resume` is the attach's existing `have`. |
| Observe (read-only watch) | TF4: `/termws?session=...&observe=1`, any number of viewers, input dropped, never paused or resized by them. A second writer makes the first a viewer instead of closing it, and nothing takes over a viewer. TF4 also shares a session with other users (read-only, write with the kind's permission). |
| Status starting, running, exited, error | Map from the session state. `TerminalSession.exitStatus` and `endReason` hold the end. |
| Events started, output, exited, closed, error, cleared, restarted, activity; metadata stream snapshot, upsert, remove | `/termws` text frames `state`, `ended` exist; TF7 adds `cleared`, `restarted`, and TF3's `find#TerminalEvents` keeps the last events. The metadata stream is `/agentws` `terminal.metadata` ([09](09-Wire-API.md)). |
| Retention 5,000 lines and 8 MiB UTF-8 per terminal; oldest evicted on code-point edges; client buffer 512 KiB | TF5's history mode: a bounded recording (`historyLocation`) of 8 MiB at code-point edges; 5,000 lines is the client's scrollback. Other kinds keep `replay-kb` (2 MiB). |
| Coalesced persistence; clear, restart and close drain first; restore skips an incomplete UTF-8 prefix | In TF5's recorder (`term/record.rs`), written by the holder for a held session; no separate `history.rs`. |
| Terminal query and response traffic stripped from history | TF5's recorder leaves the terminal's queries out. |
| Shell from `T3CODE_TERMINAL_SHELL`, with fallbacks | Conf setting `agent-code@terminal-shell` (empty by default), else the user's login shell, else `/bin/bash`, else `/bin/sh`. Not used for provider commands or setup scripts. |
| fish starts with `no-query-term` | Same flag for fish. |
| Keys Shift+PageUp/PageDown, Ctrl(Cmd)+Shift+Home/End, Select all, **Jump to latest** | Add to `moqui.keymap` defaults for the terminal view. |
| iPhone/iPad long-press paste or **Select Text** | Terminal-Fancy2 TF1: a long press selects the line and opens the terminal's menu (Copy, Paste, Select all, Find). |
| Renderer `libghostty-vt` (WASM on web) | Keep the MR Moqui terminal renderer. A switch to `libghostty-vt` is open decision O8 ([16](16-Phases.md)). The server's screen model (TF2, `vt100`) is for reading text, not drawing. |

### 1.3 Access

- A thread terminal is a host session in every way but its policy. Host sessions need `TERMINAL_HOST` and a dev
  instance.
- New rule, the `thread` policy this component registers (Terminal-Fancy2 TF7): a thread terminal needs
  `AgentCodeHost` ([14](14-Security.md)) and the project's work root. It does not need a dev instance. It runs as the
  account of `agent-code@run-as` (14 §5) when that is set, through Terminal-Fancy2 TF6's sudo-gated `term-exec`
  supervisor, not as the server user.
- Remote use keeps the remote rules: HTTPS, `terminal@remote` (`moqui_conf__terminal__remote=true`), and a recent
  sign-in ticket. The desktop and phone
  apps ask **Confirm it is you** as today.
- The directory is always inside the thread's work root. `cwd` outside it is refused.
- Each thread terminal writes a `TerminalSession` audit row, as other host sessions.
- Driving: an agent reads and types in a thread terminal through Terminal-Fancy2 TF3's services and `terminal` tool,
  with one approval per session per run; another of the user's windows watches as a TF4 viewer.

### 1.4 Thread links

- `stopOnSettle` ends the terminal when the thread settles.
- Project scripts with `runOnWorktreeCreate` or a manual run open a thread terminal named after the script.
- A terminal's output can be added to the composer as context (`terminal context` chip, as T3).
- `file:line` links open in the thread's file viewer.
- Attention: a blocked thread terminal raises the thread's attention badge, besides `TerminalAttention`.

## 2. Browser preview and automation

### 2.1 Model

| T3 | MR Moqui |
|---|---|
| Tabs belong to the environment; user and agents see the same tabs from any device; agents use tabs with no device connected; `localhost` resolves on the host | Same. `AgentPreviewTab` rows. |
| The host runs headless Chrome (about 120 MB, downloaded once); agent tabs have separate storage and share one process | Server runtime: Chromium found by `crates/moqui-host/src/chromium.rs` (`moqui_chromium_path`, `./moq chromium-download`, or the `embed-chromium` build). A new `crates/moqui-code/src/preview/` drives it over the DevTools protocol, with one browser context per profile. |
| Desktop shows its own environment's tabs directly | The Tauri desktop already runs CEF (`tauri-runtime-cef`). Runtime `desktop`: a CEF child view in the right panel ([11 §5](11-Desktop.md#5-preview-view)). |
| Other devices stream tabs from the host (`/api/preview-stream`) | `/agentws` stream `preview.frames`: CDP screencast JPEG frames plus input events back. Phone and web use it. |
| Desktop tab for a remote environment loads locally and reaches the host's loopback through `/api/preview/tunnel` (one WebSocket per TCP connection); SOCKS5 with bypass `*,<-loopback>,169.254.0.0/16,fe80::/10`; media tickets; Linux checks the socket owner in `/proc/net/tcp` | Route `/preview/tunnel` in `moqui-host`. The desktop runs a local SOCKS5 listener in `crates/moqui-desktop/src/code/tunnel.rs` and gives CEF the same bypass list. The host connects only to loopback ports owned by the work root's processes (Linux: `/proc/net/tcp` owner UID check). |
| Port forwarding (desktop): host port to local loopback, same port unless taken; **Forwarded ports** in the palette | `crates/moqui-desktop/src/code/forward.rs` over `/preview/tunnel`. Edge-linked servers: one forwarded socket of the link per TCP connection. SSH-launched servers: the tunnel goes over the SSH-forwarded `/agentws` port ([13 §7](13-Servers-Linking.md#7-ssh-launch)). |
| Phone **Open on this device** opens a host `localhost` page in the phone's web view through the tunnel; floating preview can go picture-in-picture | Android: a WebView with a proxy override to a local tunnel listener in the Kotlin host. PiP on Android 8+. iOS with MD7. |
| Setting **Preview tunnel** (`enablePreviewTunnel`, default true); policy `allowsPreviewTunnel` | Same names in `AgentCodeSetting` and `AgentCodePolicy`. |
| **Take control** / release: a human takes control before typing in an agent tab; read-only connections watch only; with control, clipboard, file picker and downloads go to the user's device | `AgentPreviewTab.controlUserId`. Input frames from others are dropped. Downloads go to the controlling device through `/agentws` `preview.download`. |
| **Local servers** list: ports held by terminal and agent processes; project script **Preview URL**; `T3CODE_PORT_DISCOVERY=0` off; stream `subscribeDiscoveredLocalServers` | `crates/moqui-code/src/preview/ports.rs` reads listening sockets of the process tree of each session (Linux `/proc`, macOS `lsof -p`). Conf setting `agent-code@port-discovery` (default true). Stream `preview.localServers`. |
| Linux host setup `sudo t3 browser setup` (AppArmor, libraries, fonts); `T3CODE_SERVER_BROWSER_SANDBOX=0` | `moqui code browser setup` prints and runs the same steps. Conf setting `agent-code@browser-sandbox` (default true; false needed as root, with a warning). |

### 2.2 Contracts (kept as T3)

- `PreviewRuntime` desktop, server. Host needs sandbox, libraries.
- `BrowserProfileKind` persistent, incognito. Profile clear cookies, cache.
- Color scheme system, light, dark.
- Tab owner agent, human, unclaimed.
- Act actions click, type, hover, select, drag, upload, press, scroll. Buttons left, right, middle. Modifiers Alt,
  Control, Meta, Shift. Targets by `locator` or legacy `selector`, or `x`/`y`.
- Navigate readiness load, domContentLoaded, none. Resize mode fill, freeform, preset. Orientation portrait,
  landscape.
- Snapshot stages metrics, frames, page state, screenshot, accessibility tree.
- Control errors agentMismatch, unclaimedTab, humanControl, tabRequired, closed, interrupted, dialogPending, tabLimit.
- Recording status running, succeeded, failed, interrupted. Video in `ContentBlob` (`AgentPreviewRecording`).
- Private-network resolution direct, direct-private-network.

### 2.3 Limits

- Tabs per environment: 16. Tabs per thread: 6. Above that, `tabLimit`.
- An agent tab with no activity for 30 minutes closes.
- One Chromium process per host. It starts on first use and stops 10 minutes after the last tab closes.
- Cluster: tabs live on the host of the thread's work root (`hostInstanceId`). Calls from other nodes go to that node
  over the cluster channel.

### 2.4 Services, RPCs and tools

- RPCs `preview.open`, `resize`, `adjust`, `close`, `list`, `clearProfile`, `reportProfiles`, `reportStatus`,
  `stopLocalServer`; stream `subscribePreviewEvents`. Same names on `/agentws`; services in `PreviewServices.xml`.
- Agent tools `preview_open`, `preview_navigate`, `preview_act`, `preview_snapshot`, `preview_evaluate`,
  `preview_wait_for`, `preview_resize`, `preview_set_appearance`, `preview_dialog`, `preview_recording_start`,
  `preview_recording_stop`, `preview_status`, `code_preview_list`, `code_preview_close` ([08](08-MCP-Tools.md)).
- Settings `enableAgentBrowserAccess` (default true), **Auto-show floating preview**.

### 2.5 Other browser features

| T3 | MR Moqui |
|---|---|
| **Use T3 Code as default browser** | Desktop only. The desktop app registers as an `http`/`https` handler on request. Links open as human preview tabs. Later phase. |
| **Import browser sessions** (cookies; policy can block) | `import#BrowserCookies` from local Chrome, Edge, Firefox profiles on the desktop, into a persistent profile. Policy `allowsBrowserImport`. Later phase. |

## 3. HTML renders (visual replies)

| T3 | MR Moqui |
|---|---|
| An agent builds a self-contained HTML page shown in the thread; themed, sandboxed | `AgentArtifact` artifactKind `html`. Shown in a sandboxed iframe (`sandbox="allow-scripts"`, no same origin) served from `/agentws` media tickets with a strict CSP. |
| Local images embedded at publish | `html_render` reads file paths in the page, inside the work root only, and inlines them as data URLs. |
| Checked with screenshots from the headless browser | `html_render` renders once in the server Chromium and returns a screenshot to the agent. |
| Private-network names blocked | CSP `default-src 'none'` plus `img-src data:`. No network from the page. |
| Tools `html_render`, `html_preview` | Same names ([08](08-MCP-Tools.md)). `html_preview` checks a page without posting. |
| Asked for an image to post elsewhere: a `.png` file | `html_render` with `format=png` writes a PNG to the work root. |

The existing OpenUI canvas of Assist stays for native runs. An HTML render is a separate kind.

## 4. MCP Apps

- Interactive UIs returned by MCP tools (MCP Apps standard) render below the tool call.
- They can call tools on their own server. The user is asked first unless the tool is read-only.
- They can post messages. The user is always asked.
- RPCs `mcpApps.callTool`, `mcpApps.toolInfo`, `mcpApps.readResource`, `mcpApps.updateModelContext` on `/agentws`.
- The MCP client in `moqui-llm` (`AgentMcpServer`) reads `ui://` resources. The view is a sandboxed iframe as §3.
- SEC-AI-02 (a one-click write in the OpenUI canvas) must not repeat here: every write from an app is a separate
  approval.

## 5. Devices (simulators and emulators)

### 5.1 Panel

- Device panel in the right panel. Tabs per device. Only the visible tab streams. **Float device over chat**.
  **3D view** for foldables. Toolbar Home, Back, Recents (Android), rotate (iOS), power off.
- **Tools** drawer: foreground app, light/dark, text size, accessibility toggles, element frame overlay, fake
  location, app permissions. iOS adds Liquid Glass, color filters, VoiceOver, test push. Android adds orientation and
  network toggle.
- Setup steps: start the device hub, check iOS and Android support, choose agent control. Nothing downloads before
  the user agrees.

### 5.2 Hosts

| T3 host | MR Moqui `AgentDeviceHost.kindEnumId` | Notes |
|---|---|---|
| Local: `expo-device-hub` streams, `agent-device` drives; npm-installed at pinned versions; supervised | `local` | With T3's hub: Node 22.12+ on that host, for devices only; open decision O9 ([16 §3](16-Phases.md#3-open-decisions)) weighs that against O3's no Node at runtime and a Rust device hub. Installed into `<agent-code@home>/devices/` at pinned versions. Run by the process host of [03 §4](03-Providers.md#4-the-process-host). |
| iOS needs macOS + Xcode; Android needs Platform-Tools, Emulator, Command-line Tools, an AVD, `ANDROID_HOME`; video needs `ffmpeg` with libx264 | same | Checks in `check#DeviceHost`. |
| **SSH device host**: alias or `user@host`, identity file, port; no password prompts; macOS or Linux; Node 22.12+; **Test connection**; both endpoints forwarded to server loopback | `ssh` with `AgentSshHost` | Needs a local port forward. `russh` has no `direct-tcpip` client use in MR Moqui yet: add it in `crates/moqui-code/src/ssh/forward.rs` (client side only; the SSH server stays without forwarding). |
| **Expo host**: billed Expo Simulators sessions with limits | `expo` | Later phase. Token in the secret store. |
| **Simlock pool** (alpha, Labs flag) | `simlock` | Later phase, behind Labs. |
| HTTP proxy `/api/device-hub/*` | `/device-hub/*` route in `moqui-host`, authenticated, to the host's hub | Streams go through the environment server, so they work over LAN, Tailscale and the edge link. Live video needs a secure origin. |

### 5.3 RPCs, tools and settings

- RPCs `device.configure`, `list`, `testHost`, `easAccount`, `simlockIdentity`, `open`, `close`, `shutdown`, `detail`,
  `action`; stream `subscribeDeviceState`. Services `DeviceServices.xml`.
- Tools `device_list`, `device_open`, `device_screenshot`, `device_close` ([08](08-MCP-Tools.md)). `device_open`
  returns an `agent-device` command bound to the device. Agent-opened devices float over chat.
- Settings **Use devices**, **Agents can use devices**. `agent-device` installs only after the second is on.
- `AgentDeviceSession` claims a device for one thread. Another thread cannot drive it while claimed.
