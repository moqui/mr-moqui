# Terminal 2: the screen as text, driven sessions, viewers, recording, OS accounts, threads and more floats

Status 2026-11-27: plan, not built; checked against the code the same day, and the parts that assumed what is not
there corrected (D13 to D17 and the notes in each phase). It follows [Terminal-Fancy](archive/Terminal-Fancy.md), built Nov 11, 2026 (P0 to
P9 and a second round) and archived the same day as this plan was written. The guide is
[Terminal](../docs/ui/Terminal.md). This plan takes the items of Terminal-Fancy §13.1 and §15 that the user chose on
Nov 27, 2026, and the framework part of [Agent Code](agent-code/06-Terminal-Preview-Devices.md)'s thread terminals, so
that Agent Code builds on it rather than beside it. The decisions are in [§3](#3-decisions); the phases TF1 to TF8 in
[§4](#4-phases). What neither plan builds stays in [Remaining-Work](Remaining-Work.md#browser-terminal).

Paths: `term/` is `crates/moqui-host/src/term/`, `command/` is `crates/moqui-host/src/command/`, `js/` is
`runtime/components/base/webroot/screen/webroot/js/`, `fw/` is `runtime/components/base/framework/`.

Sections: [1 goals](#1-goals) · [2 what is there](#2-what-is-there) · [3 decisions](#3-decisions) ·
[4 phases](#4-phases) · [5 tests](#5-tests) · [6 documents to change](#6-documents-to-change) ·
[7 not in scope](#7-not-in-scope)

---

## 1. Goals

1. **One long press** on a phone: it selects the line and opens the terminal's menu, once (Terminal-Fancy §13.1).
2. **The screen as text on the server**: a model of what a session shows, for a program or an agent to read, and for
   a preview in the session list.
3. **Sessions driven by a program**: list, read the screen, send keys, wait for a state or a text, read events; from
   services, a `moqui term` command, the `moqui>` prompt, and an agent tool with approvals.
4. **Mirrored viewers**: any number of read-only views of one session, the owner's own other windows and devices, and
   other users the owner shares it with. Agent Code's `observe` is this.
5. **Recording**: a session's output kept as an asciicast file, played back in the Terminal and downloaded; and Agent
   Code's per-thread history as a bounded form of it.
6. **OS accounts**: a host session as the person's own OS account on a shared dev server, and Agent Code's `run-as`
   account, by one mechanism.
7. **The `thread` kind**: the framework part of Agent Code's thread terminals, with the policy left to the component.
8. **More kinds of float**: the live call window, any screen, and the record panels (Chatter, Files).

## 2. What is there

From the code on Nov 27, 2026:

| Part | Now | Where |
| --- | --- | --- |
| Kinds | `enum Kind { Apps, Command, Host }`; access by kind in `built_with`, `offered`, `check` | `term/session.rs:64`, `term/access.rs:294-350` |
| A session | `Session` (owner, kind, `Info`, command and input channels) and its `Pump`: tracker, ring, the byte count, **one** `client` | `term/session.rs:183`, `:427` |
| Program | A struct of boxed parts (output, input, resize, redraw, hang-up, kill, ended), built for apps, command, host in process, host in the holder | `term/session.rs:93`, `term/registry.rs:228,448`, `term/host.rs:419`, `term/held.rs:208` |
| Attach | A second attach closes the first with "opened elsewhere"; replay by `have` or from a rest point with the modes' preamble | `Pump::attach` `term/session.rs:792`, `replay` `:850` |
| Flow control | Pause the program above 500,000 unacknowledged bytes, resume below 100,000; let a client go after 30 s with no ack | `term/session.rs:44-48`, `flush` `:703` |
| Tracker | Modes, title, directory, bell and notifications, OSC 133 marks, OSC 7501, progress; keeps no screen | `term/track.rs` |
| Holder | `moqui term-hold`, protocol 1: a JSON line then raw bytes, one subscriber per session; `hold-v<n>.sock`; the earlier-build loop is empty | `term/hold.rs:50,239`, `term/held.rs:320` |
| Starting a program | `moqui term-exec` (`MOQUI_TERM_EXEC`): `setsid`, `TIOCSCTTY`, `exec`; the server's environment less `MOQUI_*` | `term/exec.rs:74-213`, `term/host.rs:274` |
| Services | `get#TerminalOptions`, `find#TerminalSessions`, `open#TerminalSession` (with `input`), `update#`, `close#`, `confirm#TerminalUser`, `find#AllTerminalSessions`, `end#TerminalSessionOf` (Rust); `clean#TerminalSessions` (a script). All `allow-remote="false"`, reached through the Terminal screen's transitions only: no REST resource | `term/services.rs`, `fw/service/org/moqui/terminal/TerminalServices.xml`, `apps/Terminal.xml`, `TerminalWeb.js` |
| Build | `exec`, `held`, `hold`, `host`, `audit`, `notice` are `cfg(all(feature = "devtools", unix))`; a component crate depends on moqui-core, never on moqui-host | `term/mod.rs:39-50`, [Practices-Rust](../docs/dev/Practices-Rust.md) |
| Adoption | The adopted session's pump starts its count at 0 and takes the holder's replay as output, so a page's `have` from before the restart is in another numbering | `term/registry.rs:395`, `term/held.rs:310` |
| Audit | `moqui.terminal.TerminalSession`, a row per command and host session; "what is typed and shown is never recorded" | `fw/entity/TerminalEntities.xml`, `term/audit.rs:16` |
| SSH attach | `attach <id>` at the prompt: an ordinary single client, Ctrl+\ detaches | `command/attach.rs` |
| Client | `moqui.terminal`; `m-terminal-view` with its `q-menu context-menu touch-position`; a long press of 550 ms copies a line; a float holds the bare terminal element, not the view | `js/WebrootTerminal.js:452,1136,1351` |
| Floats | `moqui.floats.register(kind, def)`; kinds `terminal`, `assist`, `tray`; no `screen` kind yet; Close and Dock both end the window, with no hook of the kind's for Close | `js/WebrootFloats.js:98,188`, `js/WebrootAssist.js:1675`, `js/WebrootVue.qvt.js:10595` |
| The call window | `m-call-dock`'s small window: a fixed card at the bottom right, not moved or resized | `js/WebrootCall.js:3188-3203` |
| Record panels | `m-chatter` and `m-record-files`, an `m-dynamic-container` on `moqui.recordPanelUrl`, inline in the screen | `js/WebrootVue.qvt.js:8497-8577` |
| Agent tools | The dev tools' agent tools, with approvals (`moqui_llm::approval`): each approval is used once and bound to a hash of its arguments; an MCP write call is a run of its own | `runtime/components/dev/dev-aide/src/agent/`, `crates/moqui-llm/src/approval/mod.rs`, `crates/moqui-llm/src/mcp/tools.rs:520` |
| Precompiled templates | `m-terminal-view`, `m-float` and the shell in `WebrootVue.qvt.render.js`; the call's in `WebrootCall.render.js` (`UPDATE_GOLDEN=1 cargo test -p moqui-core --lib client_components_are_precompiled`, `call_components_are_precompiled`) | `crates/moqui-core/src/screen/tests/qvt.rs:1107,1150` |

Nothing in the repository switches OS user, keeps a screen, records output or lets two clients see one session.

---

## 3. Decisions

| # | Decision | Why |
| --- | --- | --- |
| D1 | **OS accounts by one sudo-gated supervisor.** The server starts `sudo -n <moqui> term-exec` under a sudoers line the administrator installs. `term-exec`, as root, checks the account against a root-owned file, gives it the pty, drops to it for the program and stays as a small parent that ends it. The same path serves a person's account and Agent Code's `run-as` account. No setuid binary of ours. | Decided with the user, Nov 27, 2026. sudo is on every Linux and macOS server, its rule is the administrator's, and the server is not trusted to say which accounts it may become. |
| D2 | **Recording is opt-in, and the conf may force it.** The owner starts and stops it per session or per profile; `terminal@recording` is `offer` (the default), `always` (every host and thread session, which the owner cannot stop) or `off`. It always shows. Only the owner plays a recording; an administrator deletes. Recordings expire. | Decided with the user. It is the one exception to the audit's "what is shown is never recorded", so it is visible and bounded. |
| D3 | **Sharing is read-only, write only with the kind's permission.** The owner shares a session with named users. They watch; the owner may let one type only if that user holds the kind's own permission (`TERMINAL_HOST` for a shell). Every share is audited and revocable. | Decided with the user. A shell's output is the owner's to show; typing into it is running code, which has its own permission. |
| D4 | **An agent drives a session with an approval per session per run.** The agent's first read of or keystroke into a session is an approval card that names it; then the run may read and type there until it ends. Ending a session asks each time. | Decided with the user. A terminal stays out of Assist's page reach (Terminal-Fancy D18); the tool is the way in. |
| D5 | Replay stays bytes to xterm.js (Terminal-Fancy D4). The server's screen model is for reading text, never for drawing. | The same bytes through the same emulator give the same screen. |
| D6 | The screen model is made on first use and kept in the server's pump only, rebuilt from the ring after an adoption. | A session nobody reads costs nothing, and the holder does not change for it. |
| D7 | A second writer no longer ends the first: the first becomes a viewer, with Take it back. | Agent Code's "a second window no longer takes over an observer", and a dropped laptop tab is not a dead window. |
| D8 | A phase that changes the holder's protocol raises `PROTOCOL`. An older holder's sessions are listed "(earlier build)" and can only be ended; the empty loop in `adopt` is filled in for version 1. | Each server starts a holder of its own version, so an old holder never receives a field it does not know. |
| D9 | The `thread` kind is the framework's; who may open one is a policy the Agent Code component registers. With none registered, `thread` is not available. | The pty, ring, holder, history, viewers and audit are written once; Agent Code supplies its permission, its work roots and its run-as. |
| D10 | Agent Code's history is a bounded recording (TF5's history mode), not a store of its own. | One writer, one format, one place for the query filter. |
| D11 | A `screen` float loads its screen anew: the one exception to Terminal-Fancy D17. | A screen has no other place to be moved from. |
| D12 | One phase a commit, with its focused tests and the pre-commit checks (`cargo lint`, `cargo lint-min`, `./moq test-framework`; `scripts/check-packages.sh` for a dependency). | AGENTS.md: CI runs them on every push. |
| D13 | **The `thread` kind needs a build with the dev tools**, as `host` does: the pty, holder and audit code stay under `devtools`. It does not need a dev *instance* (`host-sessions="dev"`); its policy says who may open one. | Decided with the user, Nov 27, 2026. |
| D14 | **A session's token reaches by its kind.** A command or host session's token reaches its user's command and host sessions; a thread session's only the thread sessions of its own conversation. | Decided with the user. A thread terminal may run as Agent Code's `run-as` account; its agent must not type into the person's own shell. |
| D15 | Shares are the entity's: `TerminalSessionShare` rows, read again when a held session is adopted. The holder's `meta` is written once at `open` and has no request to change it. | No protocol change for TF4. |
| D16 | The `thread` policy is a trait in moqui-core that a crate gives through an `Extension` field, as `chat_bridge_adapters` does (`crates/moqui-core/src/extension.rs:55`). The dev tools' `terminal` tool calls TF3's services. | A component crate cannot reach moqui-host. |
| D17 | TF5's protocol change also carries the count: the holder's `attach` answers the count where its replay starts, and the adopted pump starts there. | A page that comes back after a restart then resumes or resets rightly, and viewers and event numbers keep one numbering. |

---

## 4. Phases

| Phase | What | Done when |
| --- | --- | --- |
| **TF1** Long press and the float's view | §4.1 | A long press on a phone opens the menu once, with the line selected; a floating terminal has the menu, find and banners |
| **TF2** Screen model | §4.2 | A session's visible screen and scrollback read as text after output, a resize, the alternate screen and an adoption |
| **TF3** Driven sessions | §4.3 | A program lists, reads, sends to and waits on a session; an agent does the same after one approval |
| **TF4** Viewers and sharing | §4.4 | Two windows of the owner and one of another user watch while one types; a share is revoked |
| **TF5** Recording and history | §4.5 | A recorded session plays back and downloads; a forced recording cannot be stopped; a held session records through a server restart |
| **TF6** OS accounts | §4.6 | On a server set up for it, a person's host session runs as their account, with that account's environment, and ends cleanly |
| **TF7** The `thread` kind | §4.7 | With a test policy, a thread terminal opens idempotently, restarts with its history, clears, and is refused without the policy |
| **TF8** More floats | §4.8 | A call, a screen and a record panel float, restore at a load and dock |

TF2 comes before TF3, and TF5 before TF7's history. TF1 and TF8 are client only and may go first.

### 4.1 TF1: one long press, and the full terminal view in a float

**The cause.** On touch, Quasar's `context-menu` opens `m-terminal-view`'s q-menu after 300 ms of holding
(`js/WebrootTerminal.js:1351`; Quasar binds its touch hold on every platform, and Android's own `contextmenu` too),
and the terminal's own timer selects and copies the line at 550 ms (`touchGestures`, `:452-486`). Both run, and on
iOS the copy from a timer is refused, so the "did not allow the copy" dialog opens as well.

- The q-menu opens only by its `v-model`: a right click (`contextmenu` from a mouse) opens it at the pointer, and the
  550 ms handler selects the line under the finger and opens it at the finger. Touch is told by the event (a
  `touchstart`, or a `contextmenu` that follows one), not by the view's `touch` computed, which misses a touch laptop.
  Copy is first, then Paste, Select all, Find and Clear. Nothing is copied until Copy is tapped: the tap is the user
  gesture a clipboard write wants, on iOS too.
- `-webkit-touch-callout: none` on the terminal's element and its helper textarea, so iOS shows no callout of its own.
- **A float shows the whole view.** `showFloating` (`:1136`) puts the bare terminal element in the float, so a floating
  terminal has no menu, find bar, closed or taken banner, or key bar. A component that is always mounted while the
  terminal is on (beside `m-terminal-copy`) holds a teleport for each floating session into `floats.target`, as Assist
  (`js/WebrootAssist.js:2807`) and the tray do, each with an `m-terminal-view`; the view's own mount attaches the
  element. `showFloating`, the kind's `focus` and `dock` no longer move the element. TF4's viewer banner needs it.
- `WebrootVue.qvt.render.js` holds these templates and is rebuilt.
- Tests (`webroot/test/client/pages/WebrootTerminal.page.js`): a long press of synthetic touch events opens the menu
  once, with the line selected and the clipboard untouched; Copy copies it; a floating terminal's body has the menu.
  The spec loads only the template compiler today; these need Vue and Quasar loaded, as `WebrootFloats.spec.js` does,
  and are the first to synthesize a `TouchEvent`. The floating-element check (`:232`) changes with the view.

### 4.2 TF2: the screen model

- **The crate.** `vt100` (0.18.1, MIT, checked on crates.io Nov 27, 2026; it brings `vte` 0.15, and `unicode-width`
  and `itoa`, already in the tree): a parser and screen with the alternate screen, scrollback, and the screen written
  back as escape sequences. `term/screen.rs` wraps it.
- **Made on first use.** A session gets a screen the first time something reads text (TF3, the list's preview). It is
  built by feeding the replay a new client would get (`Ring::replay_from_rest`, `term/ring.rs:80`, after the modes'
  preamble), then fed in `Pump::output` (`term/session.rs:606`) beside the tracker. It follows each resize. It is
  dropped after 10 minutes with no reader, and made again when asked.
- **Held sessions.** The screen is in the server's pump only; after an adoption it is rebuilt from the holder's replay
  (D6). An apps session has no ring and no screen.
- **Rebuilt.** A screen made from the ring was written across the session's resizes, so a full-screen program's may
  be partial until it draws again; `get#TerminalScreen` says `rebuilt` until a resize or a clear of the screen.
- **The build.** `vt100` is an optional dependency under `devtools`, the feature the sessions with a ring are built
  with (`scripts/check-packages.sh`).
- **What it answers.** The visible rows, the scrollback (up to `terminal@screen-scrollback`, 1000 lines), the last
  *n* lines, the cursor, whether the alternate screen is on, and the screen as escape sequences for a preview.
- **The list's preview.** The session list shows the last three lines of a session on hover, read through TF3's
  service, which makes the screen.
- **Memory.** A cell is 32 bytes, so a screen of 200 by 50 with its 1000 lines of scrollback full is about 6.7 MB
  (measured Nov 27, 2026), held only while something reads it (D6) and dropped after 10 minutes unread.

### 4.3 TF3: sessions driven by a program

**Services**, Rust services of `term/services.rs` (registered with the others, `:46-56`) in `TerminalServices.xml`:

| Service | Parameters | Answers |
| --- | --- | --- |
| `get#TerminalScreen` | `sessionId`, `scope` (`visible`, `scrollback`, `last`), `lines`, `format` (`text`, `ansi`) | the lines, the cursor, the size, the title, the status, the byte count |
| `send#TerminalInput` | `sessionId`, `text`, `keys` (named: `Enter`, `Tab`, `Esc`, `C-c`, `Up`, …) | the byte count after the write |
| `wait#TerminalSession` | `sessionId`, and one of: `status` in a list, `pattern` (a regex on the screen), `prompt` (OSC 133 A), `quietMs`, `ended`; `timeoutSeconds` (300 at most) | what matched, or the timeout, with the screen's last lines |
| `find#TerminalEvents` | `sessionId`, `after` (an event number) | the events after it |

- **Keys.** A table maps each named key to its bytes, honoring application cursor mode from the tracker. Text with a
  newline is sent as a bracketed paste when the program set mode 2004. At most 64 KiB in one call, and no NUL.
- **Events.** Each session keeps its last 256 events, numbered, in a list of their own beside `Info` (which is cloned
  for every list and state message): a change of title, directory or status, a prompt mark, a command's end with its
  exit status, a bell or notification, an attach or detach, the end. The program's come from the tracker's `Changes`
  (`term/track.rs:74`); attach, detach and the end are the pump's.
- **Waiting.** A Rust service holds its worker thread while it waits, so a user has at most 4 waits at once, and
  `moqui term wait` waits in calls of 25 s at most, which proxies let through.
- **Who.** Command, host and thread sessions, not apps sessions. The checks are an attach's: the owner, or a share
  holder (TF4) with write for `send#TerminalInput`; `access::check` for the kind and the place (`term/access.rs:318`,
  and `may_attach` in `termws.rs:316`). A caller by login key from a remote place brings a ticket of
  `confirm#TerminalUser` for the recent sign-in, as a socket attached by key does (`termws.rs:337`). Input a program
  sends never renews the HTTP session.
- **Reached.** The screen's transitions (`apps/Terminal.xml`, `TerminalWeb.js`) for the page; for a program, a REST
  resource `terminal` in `moqui.rest.xml` with these four services and `find#DrivableTerminalSessions` (what
  `moqui term list` and the tool's `list` read, under a token too), `key-scope="terminal"` (a new scope in
  `KEY_SCOPES`, `moqui-core/src/user/login.rs:1399`). The paths ask for no sign-in, since a token's program has none;
  each service decides its caller. A GET only reads and is taken as the request's own origin, as listing is; a call
  with no request (the agent tool's) is this machine. A token is taken from a loopback peer on the server's own
  listener with no forwarding header, whatever its origin (`Facts::loopback_direct`).
- **Patterns** of a wait match at each line's ends (`^`, `$` multi-line), at most 1024 characters.

**`moqui term`**, a command of the `moqui` binary: `moqui term list`, `capture <id>`, `send <id> [--keys …] [text]`,
`wait <id> …`, `events <id>`.

- Against a server, it signs in with a login key, as `moqui tui --server` does, and finds it as
  `devtools_remote::Remote::connect` does (`MOQUI_LOGIN_KEY` or the tui key file).
- Inside a host session it finds `MOQUI_TERMINAL_SESSION`, `MOQUI_TERMINAL_URL` and a new `MOQUI_TERMINAL_TOKEN`: an
  HMAC of the session id under a key generated into `runtime/.env` (as `git_server@lfs-token-key` is), so a held
  session's token still holds after the server restarts, with no row. It is good only for these four services, from
  the loopback address only, while the session runs, and reaches by the session's kind (D14). It is set in
  `host::environment` (`term/host.rs:274`). An agent run in one terminal can then read and drive the user's others.
- The `moqui>` prompt gets `capture`, `send` and `wait` beside `sessions` and `attach` (`command/mod.rs:40`).

**The agent tool `terminal`**, in the dev tools (`runtime/components/dev/dev-aide/src/agent/terminal.rs`, beside
`hosting.rs`): `list`, `read`, `send`, `wait`, `events`, by TF3's services (D16).

- The first `read` or `send` on a session in a run is an approval (`moqui_llm::approval::ApprovalRequest`) whose card
  names the session: its title, kind, profile and directory, with the target `terminal:<sessionId>`. An approval is
  used once and bound to its arguments, so the tool looks first for an executed `AgentApproval` of the same run and
  target: found, the run reads and sends on that session without asking. Ending a session is an approval every time
  (D4).
- What the screen shows reaches the model as untrusted data, never instructions (`moqui_llm::prompt::untrusted`).
- MCP clients reach it through the dev tools' catalog (`spec.mcp`, `Classification::Read` with `may_pause`), under
  the same approvals. An MCP write call is a run of its own, so there each `send` asks.

### 4.4 TF4: mirrored viewers and sharing

**In the pump** (`term/session.rs`):

- `client` stays the one writer; beside it `observers`, each with its own sent and acknowledged counts. A frame goes to
  each window (a copy each: watchers are few). An apps session, with no ring, keeps one window: a second still closes
  the first.
- A viewer never pauses the program and never resizes it, and what it types is dropped. A viewer more than 500,000
  bytes behind is let go with "behind", and its page comes back by its `have`.
- A second writer's attach makes the first a viewer, told by `{"type":"role","role":"viewer"}` rather than a close,
  with Take it back (D7). A viewer's Take it back is a writer's attach.
- Only the owner's own viewer keeps the session from its detach timeout; another user watching does not.
- An ended session's kept output (`keep_ended`) answers a viewer as it answers a writer.
- Viewers are for command, host and thread sessions: an apps session has no ring to come back by.

**The socket.** `/termws?session=<id>&observe=1` attaches a viewer. `state` gains the session's `cols` and `rows`; a
viewer's terminal takes the session's size and scrolls in its box instead of fitting it. A viewer's `view` counts as
"the owner is looking" only when the viewer is the owner.

**Sharing:**

| Service | Parameters |
| --- | --- |
| `share#TerminalSession` | `sessionId`, `username`, `write` (`N` by default) |
| `revoke#TerminalSessionShare` | `sessionId`, `username` |

- A share is a row of a new entity, `moqui.terminal.TerminalSessionShare` (the session, the user, who shared it,
  `write`, from and thru), and kept on the session while it runs; an adoption reads the open rows again (D15).
- `Registry::find_for` beside `find` (`term/registry.rs:105`) admits the owner or a user with a share;
  `find#TerminalSessions` lists the sessions shared with the user, with their owner.
- A user with a share passes the same rules as the owner for the place (a remote client needs `terminal@remote` and a
  recent sign-in) and the purpose (a dev instance for command and host sessions). Write needs the kind's permission:
  `SSH_SHELL` for a command line, `TERMINAL_HOST` for a host session (D3).
- Ending the session ends its shares; revoking one closes that user's sockets.

**The clients.** The pane's header shows an eye with the count of viewers, and the list of them with Revoke; the
session list has Share. A viewer's terminal shows "Watching *owner*'s session", and the sessions shared with the user
are listed apart. SSH gets `attach --view <id>` (`command/attach.rs`).

### 4.5 TF5: recording, and Agent Code's history

- **The format.** asciicast v2 (`.cast`): a header line, then `"o"` events for output and `"r"` for a resize, each
  with its time. What is typed is never recorded. The terminal's queries (device attributes, cursor reports,
  `OSC 10` and `11 ?`) are left out, so a playback answers nothing; TF5 checks whether the ring's replay needs the
  same filter and gives it one if so.
- **The writer.** `term/record.rs`, used by the server's pump for a session in process and by the holder's pump
  (`term/hold.rs:395-427`) for a held one, so a restart of the server loses nothing. The holder's `open` gains
  `record` (a path and a size limit), a new request `record` starts and stops it, and `attach` answers the count
  where its replay starts (D17): `PROTOCOL` becomes 2 (D8), and `adopt`'s loop over earlier versions
  (`term/held.rs:320`) runs for version 1.
- **The conf.** `<terminal recording="offer" recording-days="30" recording-max-mb="256">`, in `mr-moqui-conf.xsd` and
  the conf schema; a profile's `record="true"` starts its sessions recorded. At its size limit a recording stops,
  with a note in the session.
- **The files.** `runtime/terminal/recordings/<username>/<sessionId>-<n>.cast`, the directory 0700 and the files 0600,
  owned by the server's OS user, whatever account the session runs as (TF6). A new entity,
  `moqui.terminal.TerminalRecording`: the session, the user, the location, from and thru, the bytes, and whether it
  was forced. A new Rust service, `clean#TerminalRecordings`, removes recordings past `recording-days`, file and row;
  `clean#TerminalSessions`, a script, calls it.
- **Shown.** A REC mark on the pane, the float's title and the session list (`state.recording`), and a `recording`
  event when it starts or stops. No line is written into the session's output: it would break a full-screen program's
  screen.
- **Played.** The Terminal's Recordings list, the owner's alone: a player (xterm.js fed by the events' times, with
  play, pause, speed and seek by feeding again from the start) and Download `.cast`, which asciinema plays too. System
  › Terminal Sessions lists every recording's row and deletes them; it does not play them (D2).
- **The rule changes.** The audit's comment (`term/audit.rs:16`), the entity's comment and Attack-Surface say what is
  recorded, when, and who reads it.
- **History mode**, for the `thread` kind (D10): rolling segments of at most 8 MiB of output in all (counted in the
  output's bytes, not the escaped `.cast` text), at code-point edges, not
  listed under Recordings and removed with the session's row. When a thread terminal is started again after its
  program is gone, its history is shown first.

### 4.6 TF6: OS accounts, per person and Agent Code's run-as

**The conf.** `<terminal run-as="server" run-as-sudo="sudo">`.

- `server` (the default) is today's: every host session is the server's OS user (Terminal-Fancy §7.4). With
  `person`, a host session runs as its user's OS account.
- The mapping is a new entity, `moqui.terminal.TerminalOsAccount` (the user, the OS username, from and thru), kept on
  System › Terminal Sessions › Accounts by an administrator who holds `TERMINAL_HOST`. A user with no account is
  refused host sessions with the reason `noOsAccount`.
- Agent Code's `agent-code@run-as` names one account for its harnesses, scripts and thread terminals, and starts them
  by the same path.

**How a program starts as the account** (`term/exec.rs`):

1. `start` (`:162`) runs `sudo -n -- <moqui> term-exec --spec <base64 JSON>` in place of `term-exec` alone. The spec
   is an argument because sudo resets the environment. The administrator's sudoers line lets the server's user run
   only `<moqui> term-exec *` as root, with `!use_pty` so sudo puts no pty of its own between.
2. `term-exec`, running as root, reads `/etc/moqui/run-as.conf`: the group whose members may be used, any accounts
   named one by one, and the server's own account. It refuses when the file is not root's or is writable by anyone
   else. The server is not trusted to name the group.
3. It checks the account: it exists, its uid is not 0, it is not the server's account (`SUDO_UID`), and it is in the
   group or named.
4. It makes the account the owner of the pty's slave (mode 0620), so programs that open `/dev/tty` again (ssh, sudo,
   gpg) work.
5. It starts itself again in a child mode, which drops to the account (supplementary groups, gid, uid), then
   `setsid` and `TIOCSCTTY` (`term/exec.rs:93-96`) and `exec`s the program.
6. The root parent stays as the supervisor: a SIGHUP or SIGTERM, which sudo relays from the server, goes to the
   program's process group; SIGKILL follows 5 s later; it exits with the program's status. The server signals sudo's
   pid instead of the program's group, which it may not signal across accounts. The session's kill, 5 s after its
   hang-up, is a SIGTERM to sudo for such a session, never a SIGKILL: a killed sudo would leave the supervisor and the
   program running, and sudo cannot relay a SIGKILL.

- **No new `unsafe`** (Practices-Rust §7). The child is one thread, so rustix's thread-level credential calls drop it
  whole; the supervisor's signals are tokio's. TF6 proves this first. Linux only: rustix has these calls on Linux
  alone.
- **The holder** needs no change: the server builds the whole helper line (`sudo -n -- <moqui> term-exec --run-as
  <account> --spec <b64>`, the spec in the arguments since sudo clears the environment) and the holder runs it as it
  runs any helper. `exec::signal` signals the pid itself when the program has no group of its own (sudo's), and a
  SIGKILL there is a SIGTERM, so a held session's kill needs no new request either. `PROTOCOL` stays 2.
- **The environment** is not the server's: `HOME`, `USER`, `LOGNAME`, `SHELL` and a `PATH` from the account's passwd
  entry, `TERM` and the others of Terminal-Fancy §7.3, and the profile's env; a login shell, so the account's profile
  does the rest.
- **The directory** is the profile's, else the account's home.
- **Shell integration** scripts (`term/host.rs:323`) are written readable by all: they hold nothing secret.
- **The holder** stays the server's user and keeps `peer_is_own_user`.
- **Setting it up.** `moqui term-exec --print-setup` prints the sudoers line and a `run-as.conf` for the
  administrator to install. Run-Deploy and the Terminal guide say how; Attack-Surface says what a server compromise
  then reaches: any account the file allows, never root.

### 4.7 TF7: the `thread` kind

- **`Kind::Thread`** (`term/session.rs:64`): a host session in every way (a pty, the ring, the holder, the tracker,
  viewers, the audit), and also opened, attached and listed by its conversation. It touches `access::built_with`,
  `offered` and `check` (`term/access.rs:294-329`), the dispatch in `open#TerminalSession` (`term/services.rs:240`),
  and the audit. It is built where `host` is (D13).
- **The policy** (D9, D16). A trait in moqui-core, given by a crate through an `Extension` field: who may open one
  (Agent Code: `AgentCodeHost`, the user's conversation, its work root), the directory check (inside the work root),
  the run-as account (TF6), the env it adds, and whether a dev instance is needed (no). With no policy given
  `thread` answers `notAvailable`. TF7's tests give one of their own.
- **One id, a new program.** A restart runs a new session under the same id, so `Registry::remove` removes only the
  session it is given (`Arc::ptr_eq`): the ended one's pump must not take the new one out of the list.
- **`open#TerminalSession` gains:**
  - `conversationId` and `clientTerminalId` (128 characters at most). A live session of the same user, conversation
    and client id is answered instead of a new one, so an open is idempotent.
  - `label`; `workRootId`, `providerInstanceId` and `stopOnSettle`, kept for the component.
  - `env`: at most 128 names, each value at most 8192 characters; names of the Moqui conf's properties removed, as
    for any host session.
- **Sizes.** The top goes to 1000 by 500 for every kind (`dimension`, `term/services.rs:103`, and the session's
  resize).
- **Attach.** The attach message gains `restart` (start an ended thread terminal's program again under the same id,
  its history first) and `maxHistory` (the most replay wanted; `have` is resume). The server sends `cleared` and
  `restarted`. New services `clear#TerminalSession` (empties the ring and the history; the page clears) and
  `restart#TerminalSession`.
- **The entity.** `TerminalSession` gains `conversationId` (indexed), `clientTerminalId`, `label`, `workRootId`,
  `providerInstanceId`, `stopOnSettle` and `historyLocation`.

### 4.8 TF8: more kinds of float

**`call`.** Where floats can be (600 px and wider), the live call's small window (`js/WebrootCall.js:3188-3203`)
becomes a `call` float holding the compact `m-call` by teleport. TF8 first proves that a moved `<video>` keeps playing
(a `play()` after the move). Its home is the right edge: Dock opens the side panel when the page is wide, else the Call
screen. Close minimizes and the call goes on, through a new `close(key)` of the kind's def (Close and Dock both end
the window today); Leave is a title-bar action. Not in Assist's reach. Picture-in-picture is unchanged. Below 600 px
the small card stays as it is. `m-call-compact` sets its height for the side panel (`js/WebrootCall.js:3632`); the
float has a variant of its own. `WebrootCall.render.js` is rebuilt.

**`screen`.** `m-float-screen`, a component of the layer (`js/WebrootFloats.js`), holds an `m-dynamic-container`
(`js/WebrootVue.qvt.js:1880`) of its own, keyed by the screen's URL and restored at a load. It loads its screen anew
(D11). Its links open in the page, as a tray tab's do. In Assist's reach. Offered as "Open in a window" in the screen
title menu (`#m-app-screens`) and in the palette.

**Record panels.** Chatter and Files (`recordPanel`, `js/WebrootVue.qvt.js:8525`) get a header button that opens a
`screen` float on the panel's URL (`moqui.recordPanelUrl`) with the record's name as its title. The float provides
`mRecordPanel`, so mr-boss's `Shared/Chatter.qvue` and `RecordFiles.qvue` work unchanged. The panel in the page stays.

The shell's precompiled render file (`WebrootVue.qvt.render.js`) is rebuilt with TF8.

---

## 5. Tests

- **Rust, beside the code**: the screen model on split sequences, a resize and the alternate screen; the key table;
  the event ring; viewers' counts and let-go; shares and their checks; the recorder's file, its filter, its limit and
  its history segments; `run-as.conf`'s parse and every refusal of §4.6 (no root needed); the thread policy and the
  idempotent open.
- **Host integration** (`crates/moqui-host/tests/it/termws.rs`, each well under 15 s): drive a shell through the
  services; a viewer and a writer on one session; a share and its revoke; a recording read back; a held session
  recorded through an adoption; `thread` with a test policy.
- **OS accounts, live**: only where `sudo -n true` works and a test account exists, as an opt-in test
  (`#[ignore = "…"]` or its own `[[test]]`), never in the regression run.
- **The agent tool**: the dev-aide's tests, with the approval asked once per session per run.
- **Client specs** (`webroot/test/client/`): TF1's long press and the float's view; a viewer's banner and size; the
  player; `WebrootFloats.page.js` for a screen float (it loads, restores at a load, its links go to the page) and a
  record panel's button; a call float in the call's spec.
- **Screens.** No sweep; a screen renders once per mode where a spec already has it. An acceptance walk only when the
  user asks.

## 6. Documents to change

- [Terminal](../docs/ui/Terminal.md): each phase's part, the conf, setting up OS accounts.
- [Qvt-Client](../docs/ui/Qvt-Client.md): the floats' new kinds and `m-float-screen`.
- [Assist](../docs/ai/Assist.md): the dev tools' `terminal` tool and its approvals.
- [Attack-Surface](../docs/security/Attack-Surface.md) and [Attack-Surface-SSH](../docs/security/Attack-Surface-SSH.md):
  driving, the token, viewers and shares, recording, OS accounts.
- [Run-Deploy](../docs/Run-Deploy.md): the new `terminal` settings and the sudoers setup.
- The conf XSD and the conf schema for every new attribute.
- The Agent Code plan, already pointed here: [06](agent-code/06-Terminal-Preview-Devices.md),
  [14](agent-code/14-Security.md), [16](agent-code/16-Phases.md), [01](agent-code/01-Data-Mapping.md),
  [08](agent-code/08-MCP-Tools.md), [09](agent-code/09-Wire-API.md).

## 7. Not in scope

Left in [Remaining-Work](Remaining-Work.md#browser-terminal): Windows host sessions (ConPTY); the kitty keyboard
protocol; named workspaces; images, ligatures and WebGL; a native window per terminal in the Desktop app; answering
Claude Code's `OSC 7501 ; ?`; Codex's hook line; the hand checks of
[Mobile-Desktop-Exit-Checks](Mobile-Desktop-Exit-Checks.md#the-browser-terminal-the-apps-and-the-edge-node). Sessions
reached on another node of a cluster or a linked instance stay Agent Code's ([09](agent-code/09-Wire-API.md),
[13](agent-code/13-Servers-Linking.md)).
