# Terminal

The browser terminal at `/qapps/Terminal` (the account menu's **Terminal UI**) is the terminal of the development
environment: many sessions at once in tabs and splits, sessions that outlive the page, the `moqui>` command line, shells
and other programs on the server's operating system, floating terminals that stay across screens, and a status for each
session that tells you when an agent in it needs you. The plan and its decisions are in
[Terminal-Fancy](../../plans/Terminal-Fancy.md); the terminal client itself (the apps in a terminal) is [TUI](TUI.md).

Contents: [kinds](#kinds-of-session) · [the screen](#the-terminal-screen) · [keys](#keys) ·
[floating](#floating-terminals) · [sessions](#sessions-that-outlive-the-page) · [host sessions](#host-sessions) ·
[attention](#attention-and-agents) · [who and from where](#who-may-open-what-and-from-where) · [conf](#conf) ·
[apps and phones](#the-desktop-and-mobile-apps-and-phones) · [protocol](#the-socket) · [code and tests](#code-and-tests)

## Kinds of session

| Kind | What | Who |
| --- | --- | --- |
| Apps | The terminal client of the apps (`moqui tui`'s), as the signed-in user | Every user, every purpose |
| Command line | The `moqui>` prompt of SSH's admin shell: `service`, `entity`, `js`, `cache clear`, `status`, `apps`, `sh`, `profiles`, `sessions`, and over SSH `attach` | `SSH_SHELL`, on a dev instance |
| Host | A program on the server's operating system on a pseudo-terminal: the server user's shell, bash, zsh, an agent, a log | `TERMINAL_HOST`, on a dev instance |

With nothing open, the Terminal opens the command line for a user who has it, else the apps (Settings › Opens first
changes it).

## The Terminal screen

- **Tabs**, each a tree of splits. **New** opens a session in a new tab: the command line, the apps, and each host
  profile. Each pane has Split right, Split down, Zoom, Float, Close pane (the session goes on) and End session.
- **The session list** (the list button) has every session of yours, here, floating, shown nowhere or open in another
  window: Show here, the session's notifications (as in Settings, always, off), Rename and End.
- **Settings**: colors (follow the app, or a named palette), the font and its size, scrollback (5000 lines for command
  and host sessions; the apps keep none), what opens first, the bell (a flash, a sound, nothing), copy on select, paste
  protection, notifications, and your own programs (host profiles). They are your preferences (`TERMINAL_CONFIG`,
  `TERMINAL_PROFILES`), so they follow you; the layout is this browser's.
- Find in the scrollback (Ctrl+Shift+F, or the context menu), copy (Ctrl+Shift+C, or select with copy on select),
  paste (Ctrl+Shift+V, Shift+Insert, the context menu). Several lines pasted into a shell whose program has not asked
  for bracketed paste ask first.
- Links: OSC 8 links and plain URLs open in a new tab (web links and paths of this site only). In a shell's output a
  file with its line (`src/term/mod.rs:42`) opens in the Dev AIDE's Workspace.
- Below 600 px one pane is shown at a time, with no splits; the tabs switch sessions.

## Keys

A focused terminal takes the keys, but for the terminal's own, which are Ctrl+Shift chords (`moqui.keymap` actions, so
the `KEYMAP` preference rebinds them; none is one a browser keeps):

| Keys | Action |
| --- | --- |
| Ctrl+Shift+Enter | New session |
| Ctrl+Shift+D, Ctrl+Shift+E | Split right, split down |
| Ctrl+Shift+X | Close the pane |
| Ctrl+Shift+Z | Zoom the pane |
| Ctrl+Shift+Left, Right | The previous and next pane |
| Ctrl+Shift+Up, Down | The previous and next prompt in the scrollback (with the shell integration) |
| Ctrl+Shift+F | Find |
| Ctrl+Shift+C, V | Copy, paste |
| Ctrl+Shift+G | Leave the terminal: the keys go back to the page |
| Ctrl+\` | The quick terminal, anywhere |

The command palette (Mod+K) has the terminal's actions too: a new command line, apps or shell, the quick terminal,
Gather terminals, Tile and Cascade.

## Floating terminals

Float on a pane moves its session into a window of the shell's float layer ([Qvt-Client](Qvt-Client.md#floats)) that
stays while you go to other screens and comes back after a reload. Dock, or on the Terminal screen dragging the window
onto its tab strip, puts it back in a pane; Close puts it back in
the session list and ends nothing; End session and Pop out are on its title bar. Arrange › Gather terminals docks them
all. Ctrl+\` is the quick terminal: the last floating terminal shown or hidden, else one floated, else a new one. The
header's terminal button, while you have sessions, lists them, with a count of those that want you. Pop out opens the
session in a browser window of its own with no shell around it. A terminal is never in Assist's reach, in a pane or a
window. Nothing floats below 600 px.

## Sessions that outlive the page

A session is the server's and belongs to its user; it has an id, and the page attaches to it (`/termws?session=`).
Leaving the page, a reload, a dropped connection or another device leaves it running: an apps session for two minutes
with no window (`detach-seconds`), a command or host session until it ends or has had no window for three days
(`host-detach-hours`). Coming back, a window that says how far it got goes on with nothing repeated, and one that does
not gets the session's last output again (`replay-kb`, 2 MiB) with the modes in force; an apps session draws itself
again. A second window takes a session, and the first offers Take it back. A logout closes the windows and leaves the
sessions. Typing in a terminal keeps your HTTP session from timing out.

Host sessions are held by `moqui term-hold`, a process the server starts beside itself, so they outlive a restart of
the server (`./moq run` again, Restart Now, a rebuild): the restarted server lists them again. The holder ends a minute
after its last session, and at once when its runtime directory is gone. A session of a holder of an earlier build is
listed with "(earlier build)" and can only be ended. When no holder starts, host sessions end with the server, and a
pane of one offers Start again: its profile in the same directory with its `resume-args` (an agent's `--resume`).

An administrator sees every user's sessions in System › Terminal Sessions, and may end one; ending another user's is
an audit row.

## Host sessions

A host session is the server's operating system user, with that user's files, keys and processes: the runtime's
`.env`, the database's files, the git credentials. Every Moqui user's host sessions on an instance are that one OS
account, so `TERMINAL_HOST` goes only to people trusted with it.

- **Profiles.** `shell` is the server user's shell (`SHELL`, else bash, else sh) as a login shell. The conf's
  `terminal` `host-profile` elements are the instance's (bash and zsh by default), offered only when their command is
  on the `PATH`. Your own are in Settings: an agent (`claude`, `codex`), a log (`tail -F runtime/log/moqui.log`), a
  container. A profile grants nothing a shell could not run.
- **Directory.** The profile's, else `host-directory`, else the checkout above the runtime. The Dev AIDE's Repositories
  open a terminal in a root's directory.
- **Environment.** The server's, less every variable that names a property of the Moqui conf, with `TERM`, `COLORTERM`,
  a UTF-8 locale, `TERM_PROGRAM=mr-moqui`, `MOQUI_TERMINAL_SESSION`, `MOQUI_RUNTIME` and `MOQUI_USER` set. No credential
  of the Moqui user is in it.
- **At the prompt.** `sh [profile]` runs a host program on the command line's own stream and the prompt comes back
  when it ends, in the browser and over SSH; `profiles` lists them and `sessions` your sessions. Over SSH, `attach
  <id>` (the id or its start; Tab completes it) makes the SSH window the session's, as a page's would be: the session's
  output is replayed, the window it was shown in is told "opened elsewhere", and Ctrl+\ detaches with the session
  still running. A command line or a host session asks what an attach from the browser asks, by the SSH peer's
  address; another user's id is no such session.
- **The pty.** `moqui term-exec` (a hidden mode of every server binary, the Desktop app's too) makes itself the session
  leader with the pty as its terminal and then becomes the program. Windows servers have no host sessions yet.

## Attention and agents

Each session has a status, shown as a dot on its pane, tab, window and list row: **working**, **blocked** (it wants
you: a permission, a question), **done**, **idle**, **error**, or ended. It comes from the best source the session gives:

1. the program's own word, OSC 7501 (Rex's program status protocol): `ESC ] 7501 ; state=blocked ; kind=permission BEL`;
2. a bell or a notification (OSC 9, OSC 777 `notify`, OSC 99): blocked, with its text, until you type;
3. shell marks (OSC 133, with the shell integration): working while a command runs, then done or error by its exit
   status;
4. the output alone: working while it flows, idle when quiet, and for an agent profile done ("quiet") after five quiet
   seconds.

A session that becomes blocked while nobody looks at it (no window shows it, or the page is hidden) sends a
notification on the `TerminalAttention` topic, which the page, Web Push, the Desktop app and a phone show: "claude in
mr-mantle is waiting". Settings › Notifications turns on done, a bell and a command longer than a set time; each
session can be set to always or off from the session list.

**Shell integration.** A profile with `integration="true"` (the default shell, bash and zsh profiles) starts bash in
POSIX mode with `ENV` naming a script of ours that leaves POSIX mode and reads your own startup files as the login shell
would, or zsh with `ZDOTDIR` naming a `.zshenv` that puts your own `ZDOTDIR` back. The script writes OSC 133 around each
prompt and command and OSC 7 with the directory, which gives jump to prompt, the directory in the title, the last
command's status on the dot and "command finished". macOS's own `/bin/bash` is left alone. A shell whose prompt already
writes these (starship and the like) needs no script.

**Hooks for agents** that do not send OSC 7501 themselves, writing to the session's terminal (not their own output):

```jsonc
// Claude Code: .claude/settings.json
{"hooks": {
  "Notification": [{"hooks": [{"type": "command", "command": "printf '\\033]7501;state=blocked;kind=permission\\007' > /dev/tty"}]}],
  "Stop": [{"hooks": [{"type": "command", "command": "printf '\\033]7501;state=done\\007' > /dev/tty"}]}],
  "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "printf '\\033]7501;state=working\\007' > /dev/tty"}]}]
}}
```

```toml
# Codex: ~/.codex/config.toml (notify runs at the end of each turn)
notify = ["sh", "-c", "printf '\\033]7501;state=done\\007' > /dev/tty"]
```

These hook lines follow each agent's documented hook settings; they were not run against the agents here.

## Who may open what, and from where

- The command line needs `SSH_SHELL` (sealed, on `ADMIN_ADV`), host sessions `TERMINAL_HOST` (sealed, on `ADMIN_ADV`),
  both on a dev instance (`instance_purpose` dev), and a build with the dev tools (Maximal, Debug; not Lite or
  Enterprise). john.doe has both in the demo data; a plain `ADMIN` member gets the apps.
- **From this machine** (a loopback peer, no forwarding header, the server's own loopback origin; or the Desktop app's
  node in process) nothing more is asked.
- **From anywhere else** (a LAN, a VPN, the edge node, a tunnel tool or a proxy on this machine) the command line and
  host sessions need `terminal_remote=true`, HTTPS, and a sign-in within the last hour, asked when a session is opened
  and when a window attaches to one. A dev server with remote on says so in its log at start.
- They take only the server's own origin, whatever `allow-origins` says.
- Elsewhere the Terminal offers the apps only, and a request for more gets one vague refusal.
- Each command line and host session is an audit row (`moqui.terminal.TerminalSession`: who, what, where from, when,
  how it ended), kept 90 days. What is typed and shown is never recorded.

## Conf

```xml
<terminal max-sessions="16" max-sessions-per-user="4" detach-seconds="120" replay-kb="2048"
        command-line="${terminal_command_line}" host-sessions="${terminal_host_sessions}" remote="${terminal_remote}"
        remote-reauth-minutes="60" host-max-sessions="32" host-max-sessions-per-user="16" host-detach-hours="72"
        host-directory="">
    <host-profile name="shell" title="Shell" integration="true"/>
    <host-profile name="bash" title="bash" command="bash" login="true" integration="true"/>
    <host-profile name="zsh" title="zsh" command="zsh" login="true" integration="true"/>
</terminal>
```

`terminal_command_line` and `terminal_host_sessions` are `dev` by default (on for a dev instance), `false` turns one
off, and empty turns it on for any purpose. To reach them from another machine, `terminal_remote=true` in
`runtime/.env` (this host only), the environment for one run, or a `default-property` in a runtime conf
([Run-Deploy](../Run-Deploy.md#properties)). A `host-profile` takes `command`, `args` (words, or a JSON list),
`resume-args`, `directory`, `login`, `integration`, `agent` and `env` children.

## The Desktop and Mobile apps, and phones

- The Desktop app's node in process carries the terminal as a browser does, and is local.
- As a client of a remote server, the Desktop and Android apps' host carries `/termws` with binary frames, signed in by
  the device's login key. A key is never a recent sign-in, so a command line or a shell from another machine first asks
  for your password (and code) in the app: Confirm it is you, which answers a ticket for a few minutes.
- On a phone the terminal has a key bar above the keyboard (Esc, Tab, Ctrl and Alt for the next key, the arrows, and
  `| ~ / - \``), fits above the keyboard as it opens, takes a pinch for the font size, and a long press copies the line.
- Through an edge node a terminal is from another machine, so the remote rules apply; a dropped link resumes by the
  count of what was drawn. Each session is one of the link's forwarded sockets.

## The socket

`GET /termws?session=<id>[&ticket=]`, by session cookie or a device's `login_key`; binary messages are the terminal's
bytes both ways, text messages JSON:

| From | Message |
| --- | --- |
| page | `{"type":"attach","cols","rows","have"}`: the first, with how many of the session's bytes it drew (or null) |
| server | `{"type":"attached","kind","title","cwd","cols","rows","from","reset"}` |
| page | `{"type":"resize","cols","rows"}`, `{"type":"ack","count"}` (each 100,000 bytes drawn), `{"type":"view","visible"}` |
| server | `{"type":"state","title","cwd","status","statusText","progress","unseen"}`, `{"type":"ended","status","reason"}` |

The server stops reading a program while the page is more than 500,000 bytes behind what it acknowledged, and reads
again below 100,000; a page that acknowledges nothing for 30 s while behind is let go. Sessions are opened, listed,
renamed and closed by the Terminal screen's JSON transitions (`options`, `sessions`, `open`, `rename`, `close`,
`confirm`) over `org.moqui.terminal.TerminalServices`. `/termws` with no `session` runs an apps session that ends with
its socket, as before.

## Code and tests

- Server: `crates/moqui-host/src/term/` (the registry, a session's pump, the ring, the tracker, attention, access,
  the pty and `term-exec`, host profiles, the holder, notifications, the services), `termws.rs`, `command/` (the
  `moqui>` loop, its line editor, its commands and `sh`); the client of a remote server in `crates/moqui-client`.
- Client: `js/WebrootTerminal.js` (`moqui.terminal`, `m-terminal-view`, `m-terminal-node`, the key bar), `apps/Terminal.qvue`.
- Tests: `cargo test -p moqui-host --lib -- term:: command::`, `cargo test -p moqui-host --test it -- termws ssh::
  edge_web`, `cargo test -p moqui-client --lib socket`, and `./moq test-client webroot --spec WebrootTerminal`.
