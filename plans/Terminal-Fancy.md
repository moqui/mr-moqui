# Terminal: sessions, the command line, server shells and floating panels

Nov 11, 2026 · **Built** the same day, P0 to P9 ([§13](#13-phases)), and a second round that evening
([§13.1](#131-the-second-round)): three items of §15, the leftovers, and live runs on this machine. The guide is
[Terminal](../docs/ui/Terminal.md). What is left (the hand checks on installed apps, a phone and a deployed edge node in
[Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md#the-browser-terminal-the-apps-and-the-edge-node), Codex's
hook line, the rest of §15) is in [Remaining-Work](Remaining-Work.md#browser-terminal). The decisions are in
[§4](#4-decisions); those that want the user's word were all answered the same day ([§16](#16-open-questions)); none is
open. Remote access is off by default
(D7); the order of the phases stands with the holder fifth (D14); the command line is behind `SSH_SHELL` and host
sessions behind a new permission of their own on `ADMIN_ADV` (D6); a logout leaves sessions running (D2); the pty
goes through `moqui term-exec` (D8); the feature is `devtools` (D15); and the apps get tabs, splits and floats on
every purpose (D19). One answer widened the plan: floating is the shell's, and this version also undocks the Assist
drawer and the tray's tabs (D12, [§10](#10-floating-panels)).

The Terminal screen (`/qapps/Terminal`) runs one session of the apps in xterm.js and ends it when the page is left.
This plan makes it the terminal form of the development environment: many sessions at once, with command line agents
and TUIs in them, on a dev server that a developer reaches from a browser or from the Moqui Desktop and Mobile apps,
on the same machine, over a network, or through an Edge Node. It expands the user's notes of Nov 11, 2026:

1. **Many sessions**, with what a desktop terminal has (Ghostty, Rex and herdr are the reference, §3): tabs, splits,
   scrollback, search, links, copy and paste, sessions that outlive a page.
2. **The command line.** With the SSH prompt's own permission, `SSH_SHELL` (on `ADMIN_ADV`, which john.doe is in),
   and on a dev instance, the Terminal opens the `moqui>` prompt that SSH gives, where `apps` and the other commands
   are typed.
3. **Server shells.** From that prompt and from the Terminal's own menu, a session on the server's operating system:
   bash, zsh or any other program, behind a permission of its own (new, on `ADMIN_ADV`).
4. **Floating terminals.** A terminal in a window that is dragged around, started from the Terminal screen and kept
   while the user goes to other screens; several of them, with ways to arrange them. The Terminal screen becomes the
   place that lists, arranges and configures sessions.
5. **Floating as a thing of the shell** (added Nov 11, 2026). The windows are not the terminal's own: the Assist
   drawer and the tray panel's tabs undock into them in this version, and other things may later.

Paths: `host/` is `crates/moqui-host/src/`, `tui/` is `crates/moqui-tui/src/`, `fw/` is
`runtime/base-component/framework/`, `webroot/` is `runtime/base-component/webroot/screen/webroot/`.

Contents: [1 today](#1-what-is-there-today) · [2 server sessions](#2-server-side-sessions-what-it-takes) ·
[3 reference terminals](#3-the-reference-terminals) · [4 decisions](#4-decisions) ·
[5 sessions](#5-sessions-on-the-server) · [6 command line](#6-the-command-line) · [7 host sessions](#7-host-sessions) ·
[8 access](#8-access-and-security) · [9 client](#9-the-client) · [10 floating panels](#10-floating-panels) ·
[11 attention](#11-attention-and-agents) · [12 remote](#12-the-apps-phones-and-the-edge-node) ·
[13 phases](#13-phases) · [14 tests and docs](#14-tests-and-docs) · [15 later](#15-later) ·
[16 open questions](#16-open-questions)

---

## 1. What is there today

| Part | State | Where |
| --- | --- | --- |
| The page | One xterm.js terminal that fills the page, one socket, no scrollback (`scrollback: 0`). It follows the app's light and dark mode (the TUI asks by OSC 11), opens OSC 8 links in a new tab, and copies by OSC 52 with a dialog when the browser refuses. Leaving the page ends the session; the close reason shows with Reconnect. Opened from the account menu's Terminal UI, for every signed-in user | `webroot/apps/Terminal.xml`, `Terminal.qvue` |
| xterm.js | 6.0.0 with the fit addon 0.11.0, vendored; no other addon | `webroot/libs/xterm`, `libs/refresh.mjs`, `libs/MANIFEST.json` |
| The socket | `GET /termws`: origin, then the session cookie only (no key header). Binary messages are the terminal's bytes both ways; one text message, `{"type":"resize","cols","rows"}`, and the session starts on the first. A ping each 30 s, the HTTP session checked each minute, a close frame with the reason | `host/termws.rs` |
| The apps session | `term::serve` runs `moqui_tui::run_session` on a thread of its own, as the user, with the request pool behind it. Limits: `terminal` `max-sessions` 16 and `max-sessions-per-user` 4, over SSH and the browser together; 10 minutes without input ends it. A session is its transport's: nothing has an id, nothing is kept when the stream closes | `host/term.rs` |
| The `moqui>` prompt | SSH only, for a key or password login with `SSH_SHELL` (`ADMIN_ADV`, sealed). A loop generic over any byte stream: it echoes, takes backspace, Ctrl+C and Ctrl+D, and has no history, cursor keys or completion (all three since P2). Commands: `help`, `status`, `service`, `entity`, `cache clear`, `js`, `apps`, `exit`, each run as the user | `host/ssh/server.rs` `interactive`, `host/ssh/shell.rs` |
| Sessions kept between connections | Only `agent-apps`: the server holds a session by id between one `ssh` and the next, found by its own user, ended after 10 idle minutes | `host/ssh/agent.rs` |
| Processes | The host starts Chromium, OpenSearch, `git` and an external agent adapter with pipes. Nothing opens a pseudo-terminal | `host/pdf.rs`, `host/search.rs`, `moqui-devkit`, `moqui-core` `service/external_agent.rs` |
| A window that stays across screens | The call dock: the shell's root holds `m-call-dock` in its canvas, over a store (`moqui.call`), so a call's small window stays while the user navigates. `m-dialog` drags by its header, with the mouse only, and is modal | `webroot/js/WebrootCall.js`, `WebrootVue.qvt.js` |
| The Assist drawer | `m-shell-assist`: one right `q-drawer`, mounted on its first opening and kept, with the whole conversation in its template; 420 px (320 to 720 by a handle), pushing the page from 1440 px. Nothing in it is moved, and Assist is kept out of it by the drawer's id | `webroot/js/WebrootAssist.js` |
| The tray panel | `m-shell-tray`: one panel at the right of the canvas, 400 px, with a tab per screen the theme names (Today, Chat, Notifications, Dialer; mr-boss's Commons and Comms panels), each an `m-dynamic-container` loaded once and kept. The root keeps what is open, the tab and each tab's scroll, in `sessionStorage`. It is in Assist's reach while open | `WebrootVue.qvt.js`; mr-boss `plan/design-drawers-commons.md` §1.4 |
| The desktop app | The node in process bridges `/termws` to the page with its binary frames. As a client of a remote server the host bridges `/notws` and `/callws` only, text frames only | `crates/moqui-desktop/src/client/sockets.rs`, `crates/moqui-client/src/bridge.rs` |
| The edge node | Web access forwards WebSockets, `/termws` among them, as HTTP/2 extended CONNECT on the link; at most 768 tunnels and sockets per node. The local node sees the client's address | [Edge-Node](archive/Edge-Node.md) §5.3 |
| The rule | No code comes in from outside and runs on the server, and there is no web eval endpoint. The Dev AIDE is the one exception, on dev instances (its D15). The browser terminal runs the apps only | [design-dev-aide](design-dev-aide.md) §15, `docs/security/Attack-Surface.md` |

---

## 2. Server-side sessions: what it takes

The question asked: what can a server-side terminal session support, how much terminal emulation is there for it, and
is a crate needed for bash or zsh.

**The short answer.** Any shell or program the server's machine has: bash, zsh, fish, vim, tmux, a coding agent's
TUI. Nothing implements a shell and no crate is added for one. The emulation is already whole in the browser, and the
one missing piece on the server is a pseudo-terminal, which the workspace's own `rustix` opens.

| What a server shell needs | There today | To add |
| --- | --- | --- |
| A terminal emulator that draws what the program writes | Yes, in the browser: xterm.js is a full xterm emulator (alternate screen, 256 and true color, mouse reports, bracketed paste, OSC 8 links, OSC 52), and the TUI already uses most of it | Scrollback turned on, and the addons of §9.4 |
| A pseudo-terminal (pty) on the server: what makes a program believe it runs in a terminal (line editing, job control, colors, window size, Ctrl+C as a signal) | No | §7.2: `rustix` (a workspace dependency, locked at 1.1.6) with its `pty`, `termios` and `process` features, on Linux and macOS. No new crate |
| The shell itself | Not ours to have: the machine's | Profiles that name a command (§7.1). The default is the server user's own shell |
| A server-side emulator: a model of the screen kept on the server | No. `moqui-tui` is an application that writes escape sequences through Revue and reads keys (`tui/decode.rs`); it cannot read a shell's output | Not for showing a session. It would matter for two things only, below |

**Where a server-side emulator would matter**, and what stands in for it:

- *Coming back to a session* (a reload, another device, a dropped connection). The server keeps each session's last
  output as bytes and sends them again; the browser's emulator reads the same bytes it would have read the first time,
  so nothing is lost in translation. A small tracker (§5.5) remembers the modes in force, and a full-screen program is
  asked to repaint by a window-size signal.
- *The screen as text*, for an AI agent that drives a shell or for a thumbnail. Not in this plan (§15). When it is
  wanted, the emulator goes behind the tracker's seam.

**How others come back to a session** (read Nov 11, 2026, from each project's source or docs):

| Tool | On reconnect | Flow control |
| --- | --- | --- |
| sshx | A ring of raw bytes, the last 2 MiB per shell, with sequence numbers; a client subscribes from a number | not read |
| terminado (Jupyter) | The last 1000 chunks read from the pty, sent to a new socket | none |
| VS Code | An emulator on the server: each kept process feeds a headless xterm, and a reconnect gets its `serialize()` | The client acknowledges; the pty is paused above 100,000 characters unacknowledged and resumed below 5,000 |
| shpool | Raw bytes while attached; a `vt100` screen kept only to redraw at reattach (`screen`, `lines`, or `simple`: a window-size signal alone) | none |
| dtach, abduco | Nothing kept; dtach asks the program to redraw (Ctrl+L, or a window-size signal) | none |
| ttyd, gotty | A new process per socket; nothing to come back to | ttyd: pause and resume messages |
| Rex | The server sends the pty's raw bytes to each client, which parses them; on attach a state frame first (one secondary source) | not read |

This plan is sshx's ring with VS Code's acknowledgements and dtach's repaint signal (§5.4). The xterm.js project's
own advice for restore is a headless xterm with its serialize addon on the server, which needs Node and is not an
option here.

**The emulator crates**, for that later step (crates.io and the repositories, Nov 11, 2026; to be checked again then):

| Crate | Version, license | What | Fit |
| --- | --- | --- | --- |
| `vte` | 0.15.0, Apache-2.0 or MIT | The escape sequence state machine alone (Alacritty's), no screen | The tracker of §5.5 may use it in place of a scanner of our own |
| `vt100` | 0.18.1, MIT | Parser and screen with the alternate screen; `state_formatted` writes the current screen back as escape sequences. shpool uses a fork | The first choice for a text screen: small, made for this |
| `avt` | 0.20.0, Apache-2.0 | asciinema's virtual terminal; reads OSC 133; its `dump()` leaves out the scrollback | A second choice |
| `alacritty_terminal` | 0.26.0, Apache-2.0 | Alacritty's whole terminal model, with its own pty layer; nothing that writes the state back as sequences | Larger than the need |
| `wezterm-term`, `termwiz` | not published; 0.23.3, MIT | WezTerm's model | Not a library to depend on |
| `libghostty-vt` | C library 1.0.0 (Nov 6, 2026), MIT; Rust binding 0.2.2 | Ghostty's terminal state with scrollback; writes plain text, VT or HTML. Rex and herdr are built on it | The most complete, but its build needs Zig |

**A shell in a crate.** `brush` (0.4.0, MIT) is a bash-compatible shell in Rust that can be embedded on tokio with its
own input and output. It would only matter where the machine has no shell at all. It is not needed: with a pty the
choice of shell is a profile, and "zsh run from bash" needs nothing.

**Windows.** The `moqui>` prompt and the apps work everywhere, since they need no pty. A server shell on Windows needs
ConPTY, through a crate (`portable-pty` 0.9.0 or `winpty-rs` 1.0.8, both MIT) or `windows-sys` calls; it is left for
later (§15). A dev server on Windows would then run `wsl` or Git Bash as a profile.

---

## 3. The reference terminals

The notes name Ghostty, Rex and Herder: these three, as the user confirmed, and as references for ideas and concepts,
not programs to match feature for feature (read Nov 11, 2026, from each one's site, docs and repository):

| | Ghostty | Rex | herdr ("Herder") |
| --- | --- | --- | --- |
| What | A native terminal (1.4.0, MIT), macOS and Linux | Superlogical's terminal and multiplexer in one, on libghostty, in public testing since Oct 5, 2026: a macOS app and a server that owns the sessions | An agent multiplexer (0.9.5, Apache-2.0), a TUI inside any terminal, in Rust on `portable-pty` and libghostty-vt, with a background server |
| Layout | Windows, tabs, splits, a quick terminal; no floating panes | Sessions of tabs of panes; split, zoom, balance; floating and minimized panes | Sessions, workspaces, tabs, panes |
| Sessions kept | No (a window's layout only, on macOS) | Yes: quit and reattach, from another machine too | Yes; after its server restarts the layout comes back and the processes do not, an agent is resumed by its own command (`claude --resume`) |
| For agents | Nothing of its own | A program status protocol (OSC 7501: idle, working, done, blocked with its kind, error, and progress), an agent skill and hooks, a command line to capture, send, wait and run, an activity sidebar | A state per pane (blocked, working, done, idle) rolled up to its tab and workspace, read from the screen by per-agent rules or sent by hooks; git worktrees; a socket API |
| Also | Shell integration for five shells, jump to prompt, notify on command finish, a command palette, search, kitty graphics, paste protection | A command palette and session switcher, jump to prompt, banners and sounds per state, Lua configuration | Toasts and sounds, mouse first, a `ctrl+b` prefix |

What this plan takes from each: from Ghostty the feel of a finished terminal (search, shell integration, paste
protection, the quick terminal); from Rex the server that owns sessions, floating panes and its status protocol,
which this plan reads as it is rather than inventing one; from herdr the status rolled up to where the user is
looking, and resuming an agent by its own command.

What a desktop terminal has, and where this plan puts it. The browser sets two limits that a desktop app does not
have: a few keys belong to the browser (Ctrl+W, Ctrl+T, Ctrl+N; the installed app and the Desktop app give them back),
and a page cannot open a native window of its own (the pop-out of §10.4 is a browser window).

| Feature | Here | Phase |
| --- | --- | --- |
| Tabs and splits, resized by their dividers, a pane zoomed to the whole area | The Terminal screen's layout (§9.3) | P1 |
| Sessions that outlive the window, reattach | The registry (§5.2); across server restarts with the holder (§7.5) | P1, P5 |
| Scrollback and search in it | xterm.js scrollback and the search addon | P3 |
| Links: OSC 8 and plain URLs | OSC 8 today; the web-links addon for plain URLs | P3 |
| Copy and paste, copy on select, paste protection | §9.5 | P3 |
| Quick terminal: one key shows and hides a terminal over whatever is on screen | The floating terminal's toggle key (§10.4) | P4a |
| Floating and tiled windows | §10, for terminals and for Assist and the tray's tabs too | P4a, P4b |
| Shell integration: prompt marks (OSC 133), the working directory (OSC 7), jump to a prompt, "command finished" | The tracker and an opt-in script for bash and zsh (§11.2) | P6 |
| Notifications from a program (bell, OSC 9, OSC 777) and progress (OSC 9;4) | §11.1, through the notification socket and Web Push, so a phone hears it | P6 |
| Which session needs me: a status per session (OSC 7501 where the program sends it), rolled up | The session list, tab dots and the header badge (§11.1) | P6 |
| Themes, fonts, cursor, a command palette | Settings (§9.6); the shell's palette gets the terminal's actions | P1, P9 |
| Wide characters and emoji at the right width | The unicode11 addon | P3 |
| The kitty keyboard protocol (Shift+Enter and the like told apart in an agent's TUI) | Not in xterm.js 6.0.0; its main branch has it behind an option. With the release that has it | later |
| Images (kitty graphics, sixel), ligatures, GPU drawing | Later (§15): the image and WebGL addons | later |
| A command line and API for agents to drive other sessions (capture, send, wait) | Later (§15); capture needs the server-side emulator | later |

---

## 4. Decisions

| # | Decision | Why |
| --- | --- | --- |
| D1 | Three kinds of session, one model: `apps` (the TUI, as today), `command` (the `moqui>` prompt) and `host` (a program on the server's operating system). Each is a program on a session's byte stream, so replay, resume, limits, the list and the windows are written once. | The prompt already runs on any stream, and `apps` at the prompt already takes the stream over. |
| D2 | A session belongs to its user and has an id. The server keeps it when its socket closes: an apps session for two minutes (a reload, a network drop), a command or host session until it ends, is closed, or has had nobody attached for three days. A logout closes the sockets and leaves the sessions. | A terminal that dies on reload is not a development environment; an agent's run must outlive a sleeping laptop and an expired login. The logout rule is confirmed with the user. |
| D3 | One WebSocket per session, on `/termws`, attached by id. Not one multiplexed socket. | Each session gets its own back-pressure, and a busy one cannot hold up another. The path is already exempt from the request deadline and already forwarded by the edge link and the desktop bridge. |
| D4 | Coming back to a session replays its bytes, kept in a ring per session with a running byte count; a client that says how far it got continues from there with nothing repeated. No emulator on the server (§2). An apps session is asked to draw itself again instead. | The same bytes through the same emulator give the same screen, with every feature xterm.js has. A second emulator would be a second set of bugs. |
| D5 | Sessions are opened by a transition and attached by the socket. The transition carries the session token, checks the permission, the purpose and the sign-in, writes the audit row and answers the id. | A WebSocket handshake is a GET with no token. Opening is the privileged step, so it goes where every other write goes. |
| D6 | The command line and host sessions are for a dev instance only (`instance_purpose` dev). The command line is behind `SSH_SHELL`, the permission the SSH prompt already has: sealed, and on `ADMIN_ADV`. Host sessions are behind a permission of their own, `TERMINAL_HOST`: new, sealed, and granted to `ADMIN_ADV` too. john.doe is in `ADMIN_ADV` and has both; a plain `ADMIN` member has neither. On any other purpose a request for one gets a single vague refusal and the page does not offer it. | Decided with the user, in two steps: `SSH_SHELL` is the prompt's permission and stays that, however the prompt is reached; the server's OS account is another thing to hand out, so it has its own. They are the second exception to "no code from outside runs on the server", after the Dev AIDE (its D15), and take its limits: dev only, a permission, an audit. |
| D7 | Local first. From the server's own machine (a loopback peer, the server's own loopback origin, no forwarding header) these sessions need nothing more. From anywhere else they need `terminal` `remote="true"`, a secure request, and a sign-in within the last hour, checked when a session is opened and when one is attached. The lightest way to turn it on is `terminal_remote=true` in `runtime/.env`; a conf does it too (§5.8). | Confirmed with the user. A default dev instance has john.doe with a known password. Without this, a dev server listening on a LAN would hand an OS shell to anyone on it. The developer who sets up remote access turns it on once. |
| D8 | A pty through `rustix` and a small helper mode of the `moqui` binary (`moqui term-exec`) that makes the new process a session leader with the pty as its terminal and then becomes the shell. No new crate, and no new `unsafe`. | The workspace has `rustix`; its `setsid` and `ioctl_tiocsctty` are safe functions when a process calls them on itself, which the helper does. The other way is a `pre_exec` closure, which is `unsafe` ([Practices-Rust](../docs/dev/Practices-Rust.md) §7: add none), or a pty crate that holds the same `unsafe` for us (`pty-process` 0.5.4, MIT, on `rustix` and `tokio` alone). Confirmed with the user. |
| D9 | The server's shell by default, as a login shell; profiles for the rest. A profile is a name, a command with arguments, a directory and variables. The conf gives the instance's profiles, a user adds their own. | bash, zsh and anything else with no code per shell. A login shell, because a server started by systemd, launchd or the Desktop app has no developer's `PATH`. |
| D10 | Host sessions run as the server's OS user, with its environment less the server's own properties. Nothing hides the runtime from them. | That is what a developer's terminal is. Saying it plainly decides who gets `TERMINAL_HOST`. |
| D11 | The terminal's client state is a store of the shell (`moqui.terminal`, as `moqui.call` and `moqui.chat` are). A session's xterm.js instance lives in the store and is shown by whichever view holds it: a pane of the Terminal screen or a floating window. Moving it between them moves its element; nothing reconnects. | It is what keeps a floating terminal alive across screens, and one terminal from being two. |
| D12 | Floating is the shell's, not the terminal's: one layer and one store (`moqui.floats`) that kinds of content register with. This version has three kinds: a terminal, the Assist drawer undocked, and each tab of the tray panel undocked. The windows are not `q-dialog`: not modal, several at once, dragged and resized by pointer events (mouse, touch and pen), with snap, tile, cascade and minimize. Below 600 px nothing floats. | Decided with the user: a long-term design goal, started here rather than built for the terminal and widened afterwards, at the cost of refactoring the Assist drawer and the tray. A dialog traps focus and closes with its page, and Esc belongs to the content. |
| D13 | Settings and profiles are the user's preferences, so they follow the user. Places, sizes, which things float and the Terminal screen's layout are the browser's (`localStorage` under `moqui.scopedKey`), since they fit one display. A load never takes a terminal session that is attached in another tab (§10.9). | A phone and a desk do not share a layout, and two tabs of one browser must not pull sessions from each other. |
| D14 | Host sessions move to a holder process (`moqui term-hold`) in the fifth phase, after the floating windows, so that they outlive a restart of the server. Until then a restart ends them, and the Terminal says so and offers Start again (§7.5). | An agent that rebuilds and restarts the server from a terminal the server owns would end itself. The pty code is the same in both; in process first is the short way to a working terminal. The order is confirmed with the user. |
| D15 | The browser's way to the command line, and host sessions on any transport, are compiled only with the `devtools` feature (Maximal's default build and Debug; not Lite or Enterprise). SSH's prompt stays with `ssh`, as now. | A package made for deployment does not carry them, and the purpose gate holds where they are carried. Confirmed with the user. |
| D16 | What a session's program says about itself (title, directory, bell, notifications, prompt marks, progress) is read on the server by a small tracker, not only in the page. | The list, the badges and a phone's notification must work for a session nobody has open. |
| D17 | Content is moved into a float and never made again: a Vue `<teleport>` for a component, the element itself for a terminal. Docked and floated are two places for one instance. | A chat's draft, an Assist run and a terminal's screen survive the move. It also keeps the refactor small: the Assist body and a tray tab stay where they are in the component tree. |
| D18 | Assist's reach follows the content, not the place: a floated tray tab stays in reach, a floated Assist stays out, and a terminal is never in, docked or floating. | The layer is outside the canvas, so reach has to be said per kind. A terminal on the page must not become a way for an agent to type into a shell without an approval. |
| D19 | The apps get all of it on every purpose: tabs, splits and floats of apps sessions, within today's limits of 16 sessions and 4 a user. | Confirmed with the user. An apps session is the user's own screens under their own authorization, so more of them changes nothing about access. |

---

## 5. Sessions on the server

### 5.1 The model

A **session** is: an id (128 random bits, base 62), its user, its kind, a title, a size in columns and rows, a state
(running, or ended with a status and a reason), an input channel, the output ring with its byte count, the tracker's
state, and the clients attached. It runs one **program** on its stream:

| Kind | Program | Coming back | Who |
| --- | --- | --- | --- |
| `apps` | `term::serve`: the TUI client | A redraw: a new `SessionEvent::Reattach` in `tui/ui/session_loop.rs` writes the terminal setup again (alternate screen, mouse mode, the OSC 11 question) and a whole frame. A resize to the same size does nothing today | Every user, every purpose, as now |
| `command` | The `moqui>` loop (§6) | Replay of the ring. While `apps` or `sh` runs inside it, that program's way | `SSH_SHELL`, dev |
| `host` | A program on a pty (§7) | Replay of the ring, then a repaint signal when the program is on the alternate screen | `TERMINAL_HOST`, dev |

`/termws` with no session named stays what it is today: an apps session that ends with its socket.

### 5.2 The registry and a session's life

`Registry::of(factory)` (a factory extension, as `Sessions::of` is) holds the live sessions by id. A session is found
only by its own user: an id that is another user's answers as one that does not exist.

- **Detached.** When the last socket closes the session goes on. An apps session ends after `detach-seconds` (120)
  without a client, and after 10 minutes without input, as now. A command or host session ends when its program ends,
  when the user ends it, when the server stops, or after `host-detach-hours` (72) with no client.
- **Ended.** An ended command or host session stays listed for 10 minutes with its exit status and its last output, so
  the user sees why a window went quiet; then it is dropped.
- **Logout.** A logout closes the user's sockets within a minute, as now, and leaves their sessions for the next
  sign-in. An administrator ends another user's sessions from System (P9).
- **Typing is use.** The socket's minute check renews its HTTP session when the socket had input since the last
  check. Today nothing does, so a developer in a terminal all afternoon would be logged out at `session-config`
  `timeout` (60 minutes). Output alone renews nothing.
- **One writer.** A second attach to a session takes it: the first client is told "opened elsewhere" and offers Take
  it back. Mirrored viewers are later (§15).
- **Stop.** `before_shutdown` ends every session: SIGHUP to each host session's process group, SIGKILL after 5 s.
- **A cluster.** Sessions are a process's own. A dev server is one node. On a cluster an apps session's id reaches
  only the member that holds it; a client whose reattach finds nothing opens a new session.

### 5.3 The socket

`GET /termws?session=<id>`: origin, the session cookie, and the session must be the cookie's user's; for a command or
host session also the rule of §8.2. Messages:

| From | Message | |
| --- | --- | --- |
| page | `{"type":"attach","cols","rows","have":<count or null>}` | The first message, in place of today's first `resize`. `have` is how many of the session's bytes this client already drew |
| server | `{"type":"attached","kind","title","cwd","cols","rows","from":<count>,"reset":<bool>}` | `reset` false: the bytes that follow continue from `have`. True: clear and take what follows as the screen |
| both | binary | The terminal's bytes, as now |
| page | `{"type":"resize","cols","rows"}` | As now |
| page | `{"type":"ack","count"}` | How far xterm.js has drawn (§5.4) |
| server | `{"type":"state", ...}` | A change of title, directory, attention or progress (§5.5) |
| server | `{"type":"ended","status","reason"}` | Then the close frame, whose reason the page shows as now |

### 5.4 Replay, resume and flow control

- **The ring.** Each command and host session keeps its last `replay-kb` (2048) of output with the count of all bytes
  written. An apps session keeps none.
- **Resume.** A client that reconnects with a `have` still in the ring gets the bytes after it. The terminal on the
  page was never cleared, so a dropped connection shows as a pause.
- **Replay.** A client with no `have`, or one that fell behind the ring, gets `reset`, then the modes in force that
  the kept bytes will not set again, then the ring from the oldest point where the tracker saw the stream at rest (a
  line end outside any escape sequence), so the replay never starts inside a sequence. A host program on the alternate
  screen then gets a size change of one row and back, which makes it repaint.
- **Flow control.** xterm.js draws slower than a pty writes, a WebSocket gives no back-pressure of its own, and
  xterm.js drops what it is given past about 50 MB waiting. So, in the manner of the xterm.js flow control guide, the
  page acknowledges what it has drawn (the callback of `write`), each 100,000 bytes. While the attached client is
  more than 500,000 bytes behind, the server stops reading the program, and reads again below 100,000; `cat` of a
  large file fills the pty's own buffer and waits, as on any terminal, and Ctrl+C still goes straight in. A client
  that acknowledges nothing for 30 s is detached, so a dead page cannot hold a program. With no client attached
  nothing waits: output goes to the ring.
- **Frames.** Output is gathered for up to 8 ms or 64 KiB into one binary message.

### 5.5 The tracker

A state machine over a session's output (`vte`'s parser, or a scanner of our own if the crate is not worth a
dependency; decided in P3 by size). It keeps no screen. It reads:

| Sequence | Kept as |
| --- | --- |
| DEC private modes: alternate screen (47, 1047, 1049), application cursor keys (1), mouse (1000, 1002, 1003, 1006), bracketed paste (2004), cursor shown (25), the keypad mode | The replay's preamble (§5.4) |
| OSC 0 and 2 | The title, unless the user named the session |
| OSC 7 | The working directory |
| BEL; OSC 9, OSC 777 `notify`, OSC 99 | Attention, with the text (§11.1) |
| OSC 133 A, B, C, D | At a prompt, or running a command; the last command's exit status (§11.2) |
| OSC 7501 (Rex's program status protocol) | The program's own state: idle, working, done, blocked with its kind, error; and progress |
| OSC 9;4 | Progress |

The page needs no addon for these: xterm.js passes over an OSC it does not know, and the server's reading is the one
every device shows.

A change is sent to the attached client as `state` and kept for the session list.

### 5.6 Limits

`terminal` `max-sessions` (16) and `max-sessions-per-user` (4) keep counting apps and command sessions, over SSH and
the browser together. Host sessions have their own, `host-max-sessions` (32) and `host-max-sessions-per-user` (16), so
a developer's agents do not take the apps' places. A refusal is the transition's error, or the socket's close reason,
as now. Input is bounded as today (a 1 MiB pipe), a resize to 20 by 10 at least and 500 by 500 at most.

### 5.7 Services, transitions and the audit

`org.moqui.terminal.TerminalServices`, Rust services of the host (registered by an extension, as the load runner's
are), each checking its caller whatever calls it:

| Service | What |
| --- | --- |
| `get#TerminalOptions` | What this user may open from here: the kinds, the host profiles that resolve to a program, the limits, and whether sessions end with the server |
| `find#TerminalSessions` | The user's sessions: id, kind, profile, title, directory, size, state, attention, progress, attached or not, started and last output |
| `open#TerminalSession` | `kind`, `profileName`, `directory`, `title`, `cols`, `rows`; answers the id |
| `update#TerminalSession` | The title |
| `close#TerminalSession` | Ends one of the user's own sessions |

`Terminal.xml` gets the transitions that call them (`options`, `sessions`, `open`, `rename`, `close`), JSON, with the
session token. A refusal there answers a `reasonCode`: `reauthRequired` (which `moqui.reauth.retry` handles) or
`notAvailable`.

**Audit.** A new entity, `moqui.terminal.TerminalSession` (`fw/entity/TerminalEntities.xml`), one row per command and
host session: id, user, kind, profile, command line, directory, the client's address, local or remote, through the
edge node or not, the visit, from and thru dates, the exit status and the reason it ended. Written at open and at end,
with an INFO log line each. What is typed and shown is never recorded. A daily job removes rows older than 90 days.

### 5.8 Conf

```xml
<terminal max-sessions="16" max-sessions-per-user="4" detach-seconds="120" replay-kb="2048"
        command-line="${terminal_command_line}" host-sessions="${terminal_host_sessions}" remote="${terminal_remote}"
        remote-reauth-minutes="60" host-max-sessions="32" host-max-sessions-per-user="16" host-detach-hours="72"
        host-directory="">
    <host-profile name="shell" title="Shell"/>
    <host-profile name="bash" title="bash" command="bash"/>
    <host-profile name="zsh" title="zsh" command="zsh"/>
</terminal>
```

`terminal_command_line` and `terminal_host_sessions` default to `dev`: on for a dev instance. `false` turns one off.
No value turns one on for another purpose. The attributes go in `moqui-conf-3.xsd`, and `MoquiDevConf.xml` needs no
change.

`terminal_remote` defaults to `false` (D7). To turn remote access on, from the lightest touch up
([Properties](../docs/Run-Deploy.md#properties)):

- `terminal_remote=true` in `runtime/.env`: one line, this host only, and it stays through an upgrade;
- the environment, or `-D terminal_remote=true` on the command line, for one run;
- `<default-property name="terminal_remote" value="true"/>` in `MoquiDevConf.xml` or another runtime conf, for
  every host that uses that conf.

### 5.9 Code

`host/term.rs` (658 lines) becomes a module, each file one thing: `term/mod.rs` (the limits, `Terminal`, `Start`),
`apps.rs` (today's `serve`), `registry.rs`, `ring.rs`, `track.rs`, `command.rs` (the prompt's loop, moved out of
`ssh/server.rs`, with `ssh/shell.rs`'s commands), `line.rs` (the line editor), `host.rs` and `exec.rs` (the pty and
the helper, `cfg(unix)`), `access.rs` (§8), `services.rs`; `termws.rs` stays the socket. `command.rs` and `line.rs`
are built with `ssh` or `devtools`, since SSH's prompt is the same loop; a command session in the registry, `host.rs`,
`exec.rs` and `access.rs` are `devtools` alone (D15).

---

## 6. The command line

The `moqui>` prompt of SSH, in the browser. For a user with `SSH_SHELL` on a dev instance it is what the Terminal
opens first; for everyone else the Terminal opens the apps, as now.

- **The same commands**, run as the user under artifact authorization, `service`, `entity` and `js` inside the Tools
  app's authorization as over SSH.
- **`apps`** runs the TUI in the session and comes back to the prompt on Ctrl+Q, as over SSH.
- **`sh [profile]`** (new) runs a host program in the session and comes back to the prompt when it ends. It needs
  `TERMINAL_HOST` beside the prompt's `SSH_SHELL`, and a dev instance, over SSH too (there with the rule of §8.2 by
  the SSH peer's address). `profiles` lists them; `sessions` lists the user's sessions.
- **A line editor** (`line.rs`), since a person now lives here: Left, Right, Home and End, Ctrl+A, E, K, U and W,
  history with Up and Down kept per user (the last 500 lines, a preference), Tab completing commands, service names,
  entity names and profiles, and a paste taken whole (bracketed paste). SSH gets it too.
- **The greeting** names `apps`, `sh` and `help`.

---

## 7. Host sessions

### 7.1 Profiles

A profile: `name`, `title`, `command` and `args` (never a shell string: an argv), `directory`, `login`, `env`
children, `integration` (§11.2), `agent` (§11.1) and `resume-args`: the arguments used in place of `args` when a
session of the profile is started again after the server ended it (`--resume`, `--continue`), which is how an
agent's work comes back before the holder exists (§7.5), as herdr does it.

- **`shell`**, the default: the server user's shell (`SHELL` in the server's environment, else `/bin/bash`, else
  `/bin/sh`), as a login shell.
- **The conf's profiles** are the instance's. One whose command is not on the `PATH` is not offered, so `zsh` shows
  where zsh is installed.
- **A user's own** are in the Terminal's Settings (a preference): an agent (`claude`, `codex`), a log (`tail -F
  runtime/log/moqui.log`), a container (`podman run -it ...`). A user who may open a host session can type any of
  these at a prompt already, so a profile grants nothing.
- **The directory** is the profile's, else `host-directory`, else the directory above the runtime (the checkout that
  holds `moq`). "Open a terminal here" from another screen (the Dev AIDE's repositories, later) passes one.

### 7.2 The pty

1. `rustix::pty::openpt`, `grantpt`, `unlockpt`, `ptsname`, with close-on-exec on the master (a flag of `openpt` on
   Linux; set afterwards on macOS, where it has none, or every later child would hold the master). The slave is opened
   with `O_NOCTTY`, so that it never becomes the server's own terminal, and is the child's stdin, stdout and stderr.
   The server closes its own copy once the child has started, or the program's end would never be seen.
2. The window size set with `rustix::termios::tcsetwinsize` before the start and on each `resize`.
3. `std::process::Command` starts **this binary** as `moqui term-exec -- <argv>` with the slave as its stdin, stdout
   and stderr, the profile's directory and the environment of §7.3. The helper calls `rustix::process::setsid` and
   `ioctl_tiocsctty` on its own stdin, then `exec`s the program (`CommandExt::exec`, with `arg0` `-bash` for a login
   shell). It never returns: the pid the server holds becomes the shell's. If the program cannot start, its message
   is the session's output.
4. The master is read and written on the edge runtime through `tokio::io::unix::AsyncFd`, non-blocking.
5. Ending: SIGHUP to the session's process group (`rustix::process::kill_process_group`), SIGKILL after 5 s, the child
   waited for so nothing is left a zombie. A read of the master after the last slave closes is `EIO` on Linux and 0
   on macOS; both are the end, not an error, and what is still in the pty is read out first.

The helper mode is hidden from `moqui help`, takes no conf, starts no runtime, and refuses to run when its stdin is
not a terminal. Every binary that hosts a server answers to it and to `term-hold` (§7.5) before anything else: the
`moqui` binary, the one `./moq build` generates, and the Desktop app, whose node in process starts its own
executable.

### 7.3 The environment

The server's own environment, so that `PATH`, `HOME`, `CARGO_HOME` and the developer's keys for their tools are there,
with these changes:

- Removed: every variable that names a property of the Moqui conf (the server's settings and secrets set through the
  environment).
- Set: `TERM=xterm-256color`, `COLORTERM=truecolor`, a UTF-8 locale when the server has none, `TERM_PROGRAM=mr-moqui`
  with its version, `MOQUI_TERMINAL_SESSION` (the id), `MOQUI_RUNTIME`, `MOQUI_USER` (the Moqui username).
- No credential of the Moqui user is put in it.

### 7.4 What a host session is

It is the server's OS user, with that user's files, keys and processes: the runtime's `.env`, the database's files,
the git credentials. Every host session of every Moqui user on an instance is that one OS account. On a developer's
own machine that is the point. On a shared dev server it means `TERMINAL_HOST` goes only to people trusted with the
account, while `SSH_SHELL` alone gives the prompt and no OS shell; a per-person OS account is not something this
plan gives (§15).

### 7.5 The holder (P5)

`moqui term-hold`: a process the server starts on the first host session, detached as `--detached` servers are, one
per runtime. It owns the ptys, the rings and the trackers, and listens on a Unix socket in the runtime (`0600`, in a
`0700` directory) for the server: open, list, attach, resize, write, end. The registry's host sessions sit behind a
trait with two implementations, in process and the holder.

- A restart of the server (`./moq run` again, Restart Now, a rebuild) finds the holder and its sessions.
- The holder ends itself a minute after its last session does, and at once when its runtime directory is gone.
- It is the build of the server that started it; the protocol has a version, and a newer server that cannot speak to
  an old holder leaves its sessions listed as "from an earlier build", attachable only to end them.
- Limits and permissions stay the server's: the holder trusts the socket's owner and nothing else.
- Not chosen: handing the ptys' descriptors from the old server to the new one, which herdr has as an experiment. It
  needs both servers alive at once, and a restart on one port has the old one gone first.
- Until the holder is built, a session the server ended shows Start again, which opens the profile in the same
  directory with its `resume-args` (§7.1).

---

## 8. Access and security

### 8.1 Who

| Session | Needs |
| --- | --- |
| `apps` | A signed-in user, as today. Every purpose |
| `command` | `SSH_SHELL`, a dev instance, `command-line` not `false`, and §8.2 |
| `host` | `TERMINAL_HOST`, a dev instance, `host-sessions` not `false`, a Unix server, and §8.2 |

- **`SSH_SHELL`** stays as it is (D6): sealed (`sealed-permissions` in `MoquiDefaultConf.xml`) and on `ADMIN_ADV`
  alone (`fw/data/SecurityTypeData.xml`). Its description, "SSH: admin command shell", gains the browser terminal
  on a dev instance.
- **`TERMINAL_HOST`** is one new row there, "Terminal: programs on the server's operating system, on a dev
  instance", granted to `ADMIN_ADV` and added to `sealed-permissions`, so only a holder grants it.
- john.doe is in `ADMIN_ADV` in the demo data (`WebrootSecurityDemoData.xml`) and has both. A plain `ADMIN` member
  gets the apps, as now. A user with `SSH_SHELL` alone gets the prompt, and `sh` is refused.
- On a purpose other than dev the services answer `notAvailable` whatever the permission, with nothing that tells
  a missing permission from the purpose, and the page's options hold only `apps`.

### 8.2 From where

- **Local**: the connection's peer is a loopback address, the request carries no forwarding or edge-client header,
  and its `Origin` is the server's own and a loopback one (`localhost`, a loopback address or a name under
  `.localhost`). The Desktop app's node in process is local. Nothing more is asked.
- **Remote**: everything else, so a LAN, a VPN, the edge node, and also a tunnel tool or a proxy on the same machine,
  whose requests come from loopback with another origin. A command or host session then needs `remote="true"`, a
  secure request (the HTTPS listener, a believed proxy's https, the edge link), and a sign-in that counts as recent
  ([Recent sign-in](../docs/security/Recent-Auth.md)) within `remote-reauth-minutes`. It is checked when a session is
  opened and when one is attached: an attach to a running shell is worth as much to a thief as a new one. A socket
  that is attached is not asked again.
- **The log.** A dev instance with `remote="true"` logs a WARN at start that names what can be reached.

### 8.3 The rest

- **The origin, strictly.** `/termws` checks its origin as `/notws` does, and that check passes every origin when
  `allow-origins` holds `*`, which `MoquiDevConf.xml` sets, and passes a request with no `Origin` at all. So on a dev
  instance it stops nothing today. A command or host session therefore takes only the server's own origin (scheme,
  host and port), whatever `allow-origins` says; a page on another port of the same machine is another origin, and
  its requests do carry the cookie. A request with no `Origin` is taken only from the app's own bridge (the node in
  process, and a device key in P7). Apps sessions keep the check they have.
- An id is no capability: the cookie's user must own the session. It is still 128 random bits that only the opening
  transition answers, behind the session token.
- Only web links and paths of this site open from a terminal, as now. No window report or clipboard read is
  answered; a program may write the clipboard (OSC 52), as now.
- Paste protection (§9.5).
- Each open and end is an audit row (§5.7).
- Production is unchanged. `docs/security/Attack-Surface.md` gets the new rows, `TERMINAL_HOST` in its list of
  sealed permissions, what `SSH_SHELL` reaches on a dev instance, and the second exception to its rule.

---

## 9. The client

### 9.1 The store

`webroot/js/WebrootTerminal.js`, loaded on `/qapps` for a signed-in user when the server serves the socket, defines
`moqui.terminal` and the components. xterm.js and its addons still load on the first session.

- `state` (reactive): the sessions (id, kind, title, directory, state, attention, progress, where it is shown), the
  screen's layout, the settings, the options the server answered. The floating windows are the layer's
  (`moqui.floats`, §10), not this store's.
- Per session, not reactive: its xterm.js instance, its socket, its element, how many bytes it has drawn.
- `open(kind, profile, options)`, `close(id)`, `focus(id)`, `refresh()`, and `float(id)` and `dock(id)`, which ask
  the layer.
- At page load it restores only when the browser's storage says there were windows or a layout, so a user who never
  opens a terminal costs no request.
- It loads before `WebrootVue.qvt.js`, as `WebrootAssist.js` and the layer do, so its components are precompiled
  with the shell's.

### 9.2 One terminal, many places

`m-terminal-view` shows a session: it puts the session's element into itself when mounted, fits it to its box
(`ResizeObserver`), and takes it out when unmounted without disposing anything. A pane of the Terminal screen and the
body of a floating window are both this component. A session shown nowhere keeps its socket and its xterm.js instance
while the page lives; past 12 live instances the one seen longest ago is let go and replays when it is shown again.

### 9.3 The Terminal screen

`Terminal.qvue` becomes the workspace, in the page's whole height as now:

- **A tab strip**: a tab per arrangement, each a tree of splits; a New menu (Apps, Command line, each host profile);
  Split right, Split down, Zoom, Float, Close on the focused pane; Search; Settings.
- **Panes**, divided by draggable splitters, each an `m-terminal-view` with a thin header (status dot, title,
  directory).
- **Sessions**, a side list of all the user's sessions, here or floating or shown nowhere or opened on another
  device: Show here, Float, Rename, End. An ended one shows its status and its last lines.
- **Settings**, a panel (§9.6).
- With nothing open it opens one session: the command line for a user who has it, else the apps.
- The `tui` and `html` texts of `Terminal.xml` stay as they are.

### 9.4 xterm.js

- Scrollback: 5000 lines for command and host sessions (a setting; it is memory per terminal, and xterm.js's own
  default is 1000), none for the apps.
- Addons vendored through `libs/refresh.mjs`, all MIT: web-links 0.12.0, search 0.16.0 and unicode11 0.9.0 in P3
  (the graphemes addon is still marked experimental); later webgl 0.19.0, image 0.9.0 and web-fonts 0.1.0.
- The DOM renderer. A page may hold many terminals and a browser gives few WebGL contexts.
- xterm.js 6.0.0 has synchronized output (mode 2026), which lets a full-screen program draw without flicker, and
  has no kitty keyboard protocol yet (§3).

### 9.5 Keys, copy and paste

- A focused terminal takes the keys, as now, but for the terminal's own: the floating toggle, the pane keys (new,
  split, close, move focus, search) and Leave the terminal, all `moqui.keymap` actions on Ctrl+Shift chords in the
  manner of desktop terminals, so a user rebinds them. The set is checked against each browser's reserved keys in P1.
- Copy: a selection copies with the copy key or the context menu, or at once with Copy on select.
- Paste: the paste key, Shift+Insert, the context menu. A paste of several lines into a host session whose program
  has not asked for bracketed paste asks first.

### 9.6 Settings

In the user's preferences (`TERMINAL_CONFIG`, `TERMINAL_PROFILES`): font family and size, the colors (follow the app,
or a named palette), cursor, scrollback, bell (none, visual, sound), copy on select, paste protection, what the
Terminal opens first, notifications (§11.1), and the user's host profiles.

---

## 10. Floating panels

The fourth note asks for a floating terminal. The user's answer of Nov 11, 2026 widens it: floating belongs to the
shell, for more than the terminal, and this first version also undocks the Assist drawer and the tray panel's tabs
(Today, Chat, Notifications and Dialer: the Commons and Comms panels of mr-boss's `plan/design-drawers-commons.md`
§1.3 and §1.4). It is a long-term design goal, started here. The word in the code is **float**, since "panel"
already names the tray panel, the record panels, `ext-panel` and the layout panels.

### 10.1 The layer

`webroot/js/WebrootFloats.js` holds the store `moqui.floats` and two components. It loads before `WebrootVue.qvt.js`,
as `WebrootAssist.js` does, so its components are precompiled into the shell's render file.

- **`m-float-layer`**, in the shell's root and outside `#m-canvas`: a fixed layer over the page, the header and the
  drawers, and below dialogs, menus, notices and the call's ring bar. It draws what the store lists and holds no
  content of its own.
- **`m-float`**, one window: the frame of §10.2 around an empty body element.
- **A kind** is what can float: `moqui.floats.register(kind, {title, icon, defaultSize, minSize, dock})`.
  `dock(shown)` puts the content back where it lives, shown there or closed.
- **Content comes to the window; the window never makes it** (D17). Whatever owns a piece of content keeps its
  component where it is in the component tree and moves its element into the float's body: a Vue `<teleport>` that
  targets the float's body while floated and is off while docked, or for a terminal the session's own element
  (§9.2). Nothing is mounted again, so a floated chat keeps its draft, Assist its run, a terminal its screen. A moved
  element loses its scroll position, so each kind puts its own back, as the tray does for a tab today. An `iframe`
  loads again when it is moved, and neither body holds one: Assist's canvas is in the page. The window's body must
  be there before the teleport names it (Vue 3.5's `defer`); P4b starts by proving the move in the precompiled
  build, with a run in progress.
- `moqui.floats.open(kind, key, options)`, `close(id)`, `dock(id)`, `toggle(kind, key)`, `focus(id)` and
  `arrange(mode)`; `state.floats` is reactive: id, kind, key, title, place and size, order, minimized, maximized.

| Kind | Content | Docked, it lives in | In this version |
| --- | --- | --- | --- |
| `terminal` | A session's terminal (§9.2) | A pane of the Terminal screen | Yes (§10.4) |
| `assist` | The Assist conversation; there is one | The right drawer | Yes (§10.5) |
| `tray` | One tab of the tray panel | The tray panel | Yes (§10.6) |
| `screen` | Any screen, loaded in an `m-dynamic-container` | Nowhere: it opens floating | The mechanism, which a tray tab uses; offered for any screen later |
| The live call window, the record panels (Chatter, Files) | | | Later (§15) |

### 10.2 A window

- **Title bar**: the kind's icon or status dot, the title, a count badge where the kind has one; then icon buttons:
  Minimize, Maximize, Dock and Close.
- **Dock** puts the content back where it lives and shows it there: the drawer opens, the tray opens on that tab,
  the session takes a pane.
- **Close docks it closed**: the content goes back to where it lives and is not shown there. Closing a float ends
  nothing.
- **Move and resize** with pointer events and pointer capture (mouse, touch and pen), by the title bar and by eight
  edge and corner handles; by keys too when the title bar has focus. A window stays inside the viewport when the
  browser is resized.
- **Focus**: a click raises the window and gives its content the keys. Esc is the content's, never the window's.
- A window is a non-modal `role="dialog"` with its title as its label.

### 10.3 Arranging

- **Snap**: dragged to an edge a window takes that half, to a corner that quarter, with an outline before the drop; a
  double click on the title bar maximizes and restores.
- **Arrange** (the layer's menu and the palette): Tile, Cascade, Column at the right, Minimize all.
- **Minimized** windows are chips in a strip at the bottom, each with its icon, title and badge.

### 10.4 Terminals

- Float on a pane of the Terminal screen moves the session into a window; Dock puts it back in a pane. Gather, in
  Arrange, docks every terminal.
- Close, by the rule of §10.2, puts the session back in the Terminal screen's list, in no pane, and the session
  goes on. End session is its own item beside Close, since a desktop terminal's close ends its shell and a user may
  expect that.
- Leave the terminal (a key, or a click outside) gives the keys back to the page.
- **The toggle key** shows and hides the last floating terminal on any screen: the quick terminal.
- **The header** gets a terminal button while the user has sessions, with the count of those that want attention; its
  menu is the session list.
- **Pop out**: a session in a browser window of its own (`/qapps/Terminal?session=<id>&solo=true`, the page without
  the shell's chrome). It takes the session; the window it left offers Bring back. In the Desktop app this is a
  native window (later).

### 10.5 Assist undocked

Today `m-shell-assist` is one right `q-drawer` with the whole conversation in its template: 420 px (320 to 720 by
its handle), pushing the page from 1440 px and over it below, the full width below 600 px.

- **The refactor.** What is inside the drawer (the header, the panes, the thread, the cards, the composer) becomes
  `m-assist-body`. `m-shell-assist` keeps the `q-drawer` and holds the body in a teleport.
- **Undock**, a button in the Assist header beside Close, closes the drawer and shows the body in a float (420 by
  640, at the right, to start). The float's Dock button puts it back and opens the drawer; its Close puts it back
  with the drawer shut. The Assist header's own Close is the float's Close while floated.
- The header's Assist button and Mod+J show and hide whichever form is current; a hidden float is still a float.
  The first Esc gives the keys to the page in both; the second closes the drawer, or hides the float.
- The width handle and `ASSIST_WIDTH` are the drawer's; a float is sized by its frame.
- The canvas stays where it is, over the page (`m-assist-canvas`). With Assist floated the drawer no longer pushes
  the page, so a canvas has the page's whole width.
- Follow, approvals, the hand-off and the tasks badge read the store and the page, not the drawer, and do not change.

### 10.6 The tray's tabs undocked

Today the tray is one panel at the right of the canvas, 400 px wide, whose tabs are `m-dynamic-container`s loaded on
their first opening and kept; the root keeps what is open, the tab, what is loaded and each tab's scroll.

- **The refactor.** Each tab's body gets a teleport, and the root's tray state gains which tabs float.
- **Undock**, a button in the tray's strip for the current tab, makes that tab a float (400 by 560) with the tab's
  label and its count badge. It leaves the tray's strip, and each tab floats on its own. The float's Dock button
  puts it back and opens the tray on it; its Close puts it back in the strip and leaves the tray as it was, shut
  or on another tab.
- The header's Today, Chat and Notifications buttons show, raise or hide a floated tab's window in place of opening
  the tray on it. With every tab floated the tray has nothing left to open.
- A floated tab refreshes as a docked one does (`reloadTrayTab`, the `shellCounts` topic and the tab's own).
- The tray closes on Esc, as now. A float does not.
- A tab's links still open their record in the page; the float stays.

### 10.7 Assist's reach

Assist reaches elements by id (`assistReach`: the canvas, the tray while open, the quick-create dialog). The layer is
outside the canvas, so a float is in reach only when its kind says so (D18):

- A floated tray tab is: its body's id joins the list, so Assist reads a floated Chat as it read the docked one.
- A floated Assist is not, as the drawer is not. The check that keeps Assist out of its own drawer names the
  drawer's id today; it becomes a mark on the body, so it holds wherever the body is.
- A terminal never is, in a pane or in a float: its element carries the same mark. An agent that should drive a
  shell gets a tool with approvals (§15), not the page.

### 10.8 Small screens

Below 600 px, and on a `density="handheld"` screen, nothing floats: Float and Undock are not offered, a terminal is
shown full screen with a switcher between sessions, and Assist and the tray are what they are today.

### 10.9 Kept and restored

Places, sizes, order and which things float are kept in the browser's storage, per device (D13). At a load:

- Assist and the tray's tabs come back floated or docked as they were. The tray's own open state stays in
  `sessionStorage`, as now.
- A floating terminal comes back attached, unless its session is attached somewhere else (another tab of the same
  browser): then it comes back as a chip that offers Take it here. Two tabs never take sessions from each other at
  a load.

---

## 11. Attention and agents

### 11.1 Which session needs me

- **A session's status**, in the words Rex's protocol and herdr both use: working, blocked (it wants the user: a
  permission, a question, a sign-in), done, idle, error; and ended, with the exit status. From the best source the
  session gives, in this order:
  1. the program's own word, OSC 7501, where the program or its hooks send it;
  2. a bell or a notification sequence (OSC 9, 777, 99): blocked, with its text;
  3. shell marks (OSC 133): working between a command's start and end, then done or error by its exit status, idle
     at a prompt;
  4. output alone: working while it flows, and for an agent profile done once it has been quiet for a set time. The
     server reads no screen, so it cannot tell a question from an end as herdr's screen rules do; the dot says
     "quiet".
- **Shown** as the dot on a pane, a tab, a window and a chip, in the session list, and as the header's count of
  blocked and done sessions not yet looked at.
- **Notified**: a session that becomes blocked or done while its user is not looking at it sends a notification on a
  `TerminalAttention` topic through the notification socket, so the page, Web Push, the Desktop app and a phone show
  "claude in mr-mantle is waiting". Per session and in Settings: on blocked, on done, on a command that ran longer
  than a set time, on a bell. By default only blocked notifies.
- **Hooks.** The guide gives hook lines for Claude Code and Codex that write OSC 7501 to the terminal when the agent
  stops or asks, for agents that do not send it themselves. Claude Code's were run in the second round (§13.1): a hook
  has no terminal, so a line writes to its agent's, and a failing one would refuse the prompt. Codex's was not run.

### 11.2 Shell integration

A profile with `integration="true"` starts bash or zsh with a small script of ours first, the way Ghostty and kitty
do it: bash in POSIX mode with `ENV` naming the script, which leaves POSIX mode and reads the user's own startup
files as the login shell would have (macOS's old `/bin/bash` is left alone, as Ghostty leaves it); zsh by `ZDOTDIR`,
with the user's own kept and read. The script writes OSC 133 around each prompt and command and OSC 7 on each
directory change. With it: the directory in the title and on new sessions "here", jump to the previous and next
prompt in the scrollback (markers the page sets from the server's `state`, since xterm.js does not read OSC 133
itself), the last command's status on the dot, and "command finished". Shells that already write these (starship
and the like) need no script. zsh's script was not run against a real zsh (none where it was built), by decision: bash's
is tested and zsh's is the same few hooks.

---

## 12. The apps, phones and the edge node

- **The Desktop app with its node in process** works as a browser does: its bridge already carries `/termws` and its
  binary frames, and it is local (§8.2).
- **The Desktop and Mobile apps as clients of a remote dev server** need two things (P7): the client's bridge to
  carry `/termws` with binary frames (`moqui-client` `bridge.rs`, today text only on `/notws` and `/callws`), and
  `/termws` to take the device's login key as `/notws` does. A key login is a program's for Assist approvals, as with
  `moqui tui --server`, and it is never a recent sign-in, so a command or host session from the app first confirms
  the user in an app window (`moqui.reauth`); how that session reaches the host's socket is checked in P7.
- **A phone** (P7): a key bar above the keyboard (Esc, Tab, Ctrl, Alt, the arrows, and the characters a phone hides),
  the terminal fitted to the visual viewport as the keyboard opens, pinch for the font size, a long press to select.
- **The edge node** forwards `/termws` already. What this plan adds is on the local node: such a client is remote
  (§8.2), a dropped link resumes by the byte count (§5.4), and each session is one of the link's 768 forwarded
  sockets. P8 checks it through both modes of web access.
- **A LAN or VPN**: remote, over HTTPS.

---

## 13. Phases

Each phase ends with its focused tests green; the whole framework suite runs once at the end
(`./moq test-framework`).

**Built**, each phase a commit (Nov 11, 2026). Where the build went its own way:

- P1: the session pump runs a `Program` (output, input, resize, redraw, hang-up, kill, end) on any byte stream, so the
  apps, the command line, a host program and a holder's session are one model (D1).
- P2: a request says how it reached the server (`RequestVia`: direct, the node in process, the edge link), set by the
  host on each connection, which §8.2's place reads.
- P3: the tracker is a scanner of our own, not `vte`. `term-exec` takes the program's argv in `MOQUI_TERM_EXEC`, not its
  own arguments, since the Desktop app's binary reads a `--type=` argument as a browser helper.
- P4a: floating terminals and the float layer (P4b with it, built beside the server phases).
- P5: the holder speaks a line of JSON then raw bytes per connection; a holder of an earlier version keeps its own
  socket (`hold-v<n>.sock`), and its sessions are listed "(earlier build)", to be ended only.
- P6: prompt markers are set by the page itself (an OSC 133 handler of xterm.js) rather than from the server's state;
  the page reports whether it looks at a session (`{"type":"view"}`), which decides "nobody looks".
- P7: a device key keeps no session, so a recent sign-in from an app is a ticket of `confirm#TerminalUser` (the
  password and code in one request) that the opening and the attach take.
- P9: palettes are named schemes with their ANSI colors; the administrator's list is System › Terminal Sessions, its End
  an audit row registered for `find#Changes`.

| Phase | What | Done when |
| --- | --- | --- |
| **P0** Seams | `host/term.rs` to `term/`; the prompt's loop and commands out of `ssh/`; `Terminal.qvue` over `moqui.terminal` and `m-terminal-view` with one session. No change a user sees | The termws and SSH tests pass unchanged |
| **P1** Sessions | The registry, ids, detach and reattach (`SessionEvent::Reattach`), the services and transitions, `/termws?session=`; the Terminal screen's tabs, splits, session list and Settings; apps sessions only | Two apps sessions side by side; a reload comes back to both where they were; the limits refuse with their reason |
| **P2** Command line | `SSH_SHELL` in the browser, the dev gate, §8.2 with the recent sign-in and the strict origin, the audit entity, the ring, resume and flow control, the line editor | john.doe on a dev instance gets `moqui>` in the Terminal; a production instance and a user without the permission get the apps and `notAvailable`; a remote client is refused until `remote` is on and the sign-in is recent |
| **P3** Host sessions | The pty and `term-exec`, profiles, the environment, the tracker (modes, title, directory, bell), replay with the repaint, `sh` at the prompt in the browser and over SSH, `TERMINAL_HOST`, scrollback and the addons, paste protection | bash and zsh open for john.doe, and a user with `SSH_SHELL` alone is refused them; vim, htop and an agent's TUI draw and resize; a reload replays; `cat` of a large file does not freeze the page and Ctrl+C stops it; the exit status shows |
| **P4a** Floats and floating terminals | `WebrootFloats.js`: the layer, the window, snap and arrange, minimize, places kept and restored; the `terminal` kind, the toggle key, the header button, pop out, small screens | A floating shell stays through navigation and a reload; four windows tile; nothing floats on a phone |
| **P4b** Assist and the tray undocked | `m-assist-body` out of `m-shell-assist`; the tray's tabs behind teleports; Undock and Dock, the header's buttons, the reach list by kind | Assist floats with a run in progress and loses nothing; a floated Chat keeps its draft and its count; Assist still reads a floated tray tab and never a terminal; docked, both look and behave as they do today |
| **P5** Holder | `term-hold`, the socket and protocol, the registry's second host implementation | A host session's program runs on through a restart of the server and reattaches with its output |
| **P6** Attention | The tracker's marks, notifications and progress, the status, the badge, the notification topic, shell integration for bash and zsh | An agent that stops for input shows on the header and on a phone; jump to prompt works in bash and zsh |
| **P7** Apps and phones | `/termws` through the client's bridge with a device key, the re-authentication, the key bar and touch | The Desktop and Android apps open a shell on a remote dev server; a phone types Esc, Tab and Ctrl+C |
| **P8** Edge | Checks through an edge node in both modes; the docs | A shell through the edge node survives a dropped link |
| **P9** Polish | Palettes and fonts, the palette's actions, an administrator's list of all sessions in System, "Open a terminal here" from the Dev AIDE, file and line links into its Workspace | |

### 13.1 The second round

Nov 11, 2026, the evening. The plan stays here, not archived: §15 waits for a focused effort of its own.

- **§15, three items built.** `attach <id>` at the SSH prompt (`command/attach.rs`; Ctrl+\ detaches; SSH only, so a
  command line never feeds itself); `m-dialog` drags by touch and pen on `moqui.pointer` (MoquiLib.js), the helper the
  floats' bars and grips use too; a float dragged to its kind's home docks, with a dashed Dock outline before the drop
  (`home` of a kind: Assist and a tray tab the right edge, a terminal the Terminal screen's tab strip).
- **Leftovers.** `scripts/check-packages.sh` passes again: imap-only code in `moqui-mail` and a cluster test helper are
  gated as their callers are. The Desktop crate builds with its `terminal_helper` call.
- **Found by the live runs, and fixed.** The guide's Claude Code hook lines wrote to `/dev/tty`, which a hook has not
  got; the failing `UserPromptSubmit` line refused every prompt. They now write to the agent's own terminal and always
  succeed, with `PermissionRequest` for blocked (`Notification` did not run). The Terminal offered a device key's
  client (the Desktop or Android app) the command line and shells that opening then refused: a device key's listing
  is now placed as its opening is.
- **Run here.** The Desktop app's node in process, in a build with `devtools`: the command line, `sh`, a host shell
  through the app's own executable and its holder, a float kept across screens and a reload, Ctrl+C, dock by drag. As a
  client of this machine's server: an apps session over the client's bridge (the command line and shells are remote
  for a device key, by design). Chrome on the moqui-36 emulator, through `adb reverse` (local): the command line and a
  shell, the key bar's Esc, Tab, Ctrl+C and Ctrl+D by touch, the fit above the keyboard, a long press that selects and
  copies the line, pinch in and out. `adb shell input text` doubled characters where taps on the keyboard did not: an
  artifact of the injection, not of the terminal.
- **Not done here**, as hand checks in [Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md#the-browser-terminal-the-apps-and-the-edge-node):
  installed bundles, a real phone and the Android app against a remote dev server over HTTPS, and a deployed edge node.
- **The Desktop app and the dev tools.** Its default build had no `devtools` (`maximal` alone, the server's `maximal,
  devtools`), so its node in process offered only the apps. Decided with the user: the app carries the dev tools and
  the bundle `dev-component`, and the tray's Developer tools (off by default) makes the node a dev instance, with the
  Dev AIDE, the command line and shells (`node-purpose`, `MOQUI_DESKTOP_PURPOSE`).
- **Open.** Claude Code 2.1 asks `OSC 7501 ; ?` itself: answering as Rex does may let it send its status with no
  hooks (§15). A long press on a phone also opens the terminal's context menu beside the copy.

---

## 14. Tests and docs

- **Rust, beside the code**: the ring and resume counts, the tracker on sequences split across reads, the line
  editor, local and remote, the environment, profile resolution, the registry's lifetimes.
- **Host integration** (`crates/moqui-host/tests/it/termws.rs`, each well under 15 s): open, attach, reattach and
  redraw; the limits; the command line refused by purpose, permission and place; a host session's echo, `stty size`
  after a resize, exit status, replay and resume (`cfg(unix)`); `sh` over SSH in `ssh.rs`.
- **Client specs** (`webroot/test/client/`, in a page with xterm.js as `WebrootTerminal.spec.js` is): a session's
  element moves between views with its buffer; the resume protocol against a stand-in socket; the key bar. A new
  `WebrootFloats.spec.js`, with no terminal in it: drag, resize, snap, tile and minimize; places kept and restored;
  a kind's `dock`, shown and closed. `WebrootAssist.spec.js` gains the undocked body (a run, a draft and the scroll kept
  across Undock and Dock; Mod+J and Esc in both forms) and the reach rules of §10.7; a tray spec the same for a tab.
- **Screens**: `Terminal.xml` keeps its one render per mode in the specs that have it. No sweep.
- **Acceptance** (`acceptance/client`, run only when asked): one browser walk, from a new host session through a
  float, another app, a reload and back.
- **Docs when built**: a guide, `docs/ui/Terminal.md`; `docs/ui/TUI.md` "In the browser"; `Attack-Surface.md`;
  `Run-Deploy.md` for the conf; `Qvt-Client.md` for the terminal's store and a section on floats (the layer, the
  kinds and how to register one); `docs/ai/Assist.md` for the undocked form and the reach rules; the Desktop, Mobile
  and Edge docs; the conf XSD. mr-boss's `plan/design-drawers-commons.md` §1.3 and §1.4 and its style guide §2 get
  the undocked forms, in that repository.
- The precompiled render file of the shell (`WebrootVue.qvt.render.js`) is rebuilt when the root's template gains
  the layer (P4a) and when the Assist and tray components change (P4b).

---

## 15. Later

- Windows host sessions (ConPTY).
- A server-side emulator: the screen as text for an Assist or MCP tool that drives a shell, and thumbnails (§2).
- Sessions driven by a program, as Rex's command line and herdr's socket do it: services and a `moqui` command to
  list, send keys, wait for a state and read events; capture with the emulator above.
- The kitty keyboard protocol, with the xterm.js release that has it.
- Mirrored viewers of one session, and a session shown to another user.
- ~~Attaching to a session from SSH (`attach <id>` at the prompt).~~ Built Nov 11, 2026 (§13.1).
- Answering Claude Code's `OSC 7501 ; ?` as Rex does, so the agent sends its status with no hooks.
- A per-person OS account for host sessions on a shared dev server.
- Named workspaces: an arrangement saved with the profiles it starts.
- A git worktree per agent session, with the Dev AIDE's repositories.
- Images, ligatures, WebGL; a bundled font with the symbols agents' TUIs use.
- A native window per terminal in the Desktop app.
- Recording a session.
- More kinds of float: the live call window, the record panels (Chatter, Files), and any screen from its page
  menu ("Open in a float", the `screen` kind of §10.1).
- ~~Docking a float by dragging it to the edge it came from.~~ Built Nov 11, 2026 (§13.1): to its kind's home.
- ~~`m-dialog`'s drag on the layer's pointer helper, so a dialog moves by touch too.~~ Built Nov 11, 2026 (§13.1).

---

## 16. Open questions

**Answered Nov 11, 2026.**

- *Remote by default* (D7): off, until `terminal_remote` is set, in `runtime/.env` for the lightest touch or in
  `MoquiDevConf.xml` or another conf (§5.8).
- *The holder* (D14): fifth, after the floating windows. The order of §13 stands.
- *Permissions* (D6): `SSH_SHELL` is reused for the command line and stays on `ADMIN_ADV`, not `ADMIN`. Host
  sessions have a permission of their own, `TERMINAL_HOST`, sealed and on `ADMIN_ADV` too. The plan first had two
  new permissions on `ADMIN`, then `SSH_SHELL` for both.
- *Sessions and logout* (D2): a logout leaves the user's command and host sessions running.
- *The helper and `unsafe`* (D8): `moqui term-exec`, with no new crate and no `unsafe`.
- *The feature* (D15): `devtools`.
- *Apps sessions for everyone* (D19): yes, tabs, splits and floats on every purpose. With it came the wider scope:
  floating is to become general over time, and this version also undocks the Assist drawer and the Comms and
  Commons drawers (D12, §10), with the refactoring that takes.
- *The reference programs* (§3): Rex and herdr, as references for ideas and concepts.
- *"The Comms and Commons drawers"* are the tray panel's tabs: Today, Chat, Notifications and Dialer (§10.6). The
  left app drawer does not float; the record panels and the live call window come later (§15).
- *A floated tab leaves the tray* (§10.6), each tab on its own. A float has an icon button in its header to dock
  it again, and closing a float docks it closed (§10.2).

**Open.** None.
