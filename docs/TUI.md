# The Terminal UI

`moqui tui` shows the apps in a terminal. It draws the same XML Screens the browser does, through the `tui` render mode:
the server writes each screen as a JSON document of nodes, and the client in `crates/moqui-tui` draws it with
[Revue](https://github.com/hawk90/revue). There are no TUI-only screens. The design is in
[plans/archive/Screen-TUI-Impl.md](../plans/archive/Screen-TUI-Impl.md).

It is in every package, Default (`lite`) included (the `tui` feature, [Packaging](Packaging.md)). `apps` over SSH also
needs the `ssh` feature, which Default leaves out.

## Starting it

```
moqui tui                                   the apps on this runtime, after a login
moqui tui --user john.doe                   log in as john.doe (an instance_purpose of dev only)
moqui tui --path /apps/gallery/List         start on a screen
moqui tui --serve                           also open the HTTP, SSH and cluster ports, for the browser beside it
moqui tui --snapshot /apps/gallery/Detail --size 100x40     print one screen as text and exit
moqui tui --server https://erp.example.com  the apps of a running server, over HTTP
```

The client runs in this process on this runtime. It opens no port unless `--serve`, and it writes nothing to stderr: the
log goes to the log files and to the log pane (Alt+L). If a server already answers on the runtime's HTTP port, `moqui
tui` says so and stops: stop the server first, or reach it with `moqui tui --server http://localhost:8080`.

Each screen is rendered as the logged-in user with artifact authorization on, as the browser's requests are. The client
holds no permission logic: what the user may not do is not in the document.

**The login dialog** asks for the username and password, and the code of an account with a second factor (an
authenticator app, a single-use code, or one sent by email or SMS). A right password moves the dialog to Code, says
which factors the account has, and keeps the password for the next try only; Alt+S sends a code to the first factor
that takes one, when there is one. Esc closes the dialog and forgets what was typed.

## A running server (`--server`)

`moqui tui --server <url>` is the same client for a server that is already running, here or elsewhere. It boots no
runtime: each screen is a `GET <path>?renderMode=tui`, each form and transition asks for JSON, and every request
carries a login key. `--agent`, `--snapshot`, `--ascii` and `--no-color` work with it as they do on the runtime.

- **Login.** The client asks for the username and password once and posts them to the REST `login` with
  `createLoginKey=true`. The server answers with a login key (`login_key_expire_hours`), which the client keeps in
  `~/.config/moqui/tui/<host>_<port>.toml` (or under `XDG_CONFIG_HOME`), readable by the user only. The password is not
  kept. The next run uses the key; when the server stops taking it (it expired, or its `UserLoginKey` row was removed)
  the login dialog comes back. `moqui tui --server <url> --forget` removes the key. An account with a second factor
  gets its code asked for in the dialog ("The login dialog", above).
- **https.** Plain `http://` is refused except to this machine (`localhost`, `127.0.0.1`, `::1`), since the login
  would cross the network in the clear.
- **Files.** Exports are saved to `XDG_DOWNLOAD_DIR` (else `~/Downloads`) on the user's machine, as in local mode.
- **Counters.** The client opens `/notws` with its key, so the counters change when a notification comes, and the
  notification's title shows on the message line. Without the socket it reads them each minute, as before.
- **Build.** The client alone builds without a database driver or the HTTP server:
  `cargo build -p moqui-host --no-default-features --features tui` (docs/Packaging.md).

## Over SSH

When the built-in SSH server runs (`ssh_port`, see `crates/moqui-host/src/ssh/server.rs`), the same client runs on an
SSH channel, in the server's process, for any user, as the browser is for any user:

```
ssh -p <ssh_port> <user>@<host>             the apps; with SSH_SHELL and a key or password login, the admin shell first
ssh -t -p <ssh_port> <user>@<host> apps     the apps, for any account
moqui> apps                                 from the admin shell; quitting the apps (Ctrl+Q) comes back to the prompt
```

- **Login.** A key (an active `UserSshKey`) logs in at the SSH level and goes straight in. With no key, the client is
  let in with no user (keyboard-interactive, with no questions, which OpenSSH tries after its keys) and the apps open
  with the same login dialog `moqui tui` shows, the SSH name filled in. Such a session closes after 2 minutes without a
  login, or after 5 refused ones, and gets only the apps: no commands, `agent-apps`, SFTP or git. To log in by password
  at the SSH level instead (for the admin shell without a key), use `ssh -o PreferredAuthentications=password`.
  `ssh-server` `login-dialog="false"` turns the dialog off.
  An account with a second factor logs in through the dialog ("The login dialog");
  at the SSH level a password alone does not let it in. A dialog login is checked against the account's and its
  groups' `ipAllowed` with the SSH client's address, as an SSH password login is.
- **Permission.** None is needed for the apps: every screen is still rendered as the user with artifact authorization
  on, as in the browser. `SSH_SHELL` adds the admin shell's `moqui>` prompt, only for a key or password login; a
  dialog login is the apps alone, and quitting them ends the connection.
- **Terminal.** It needs a pty (`ssh -t`, which plain `ssh user@host` asks for). The client takes the pty's `TERM` and
  window size, and follows window changes. Colors follow `TERM` (`*-256color` is 256 colors). Send
  `COLORTERM=truecolor` for true color, or `NO_COLOR=1` for none, with `SendEnv COLORTERM NO_COLOR LANG` in
  `~/.ssh/config`. A locale that is set and is not UTF-8 draws in ASCII.
- **Limits.** At most `terminal` `max-sessions` (16) terminal sessions run at once, over SSH and in the browser together, and
  `terminal` `max-sessions-per-user` (4) of them for one user; a session in its login dialog counts toward the first only.
  `ssh-server` `max-sessions` counts connections, one let in for the dialog too. A session ends after 10 minutes without input.
- **Files.** Exports are not downloaded, because the client runs on the server; the text export still shows in a
  dialog. Uploads are off.
- **Log.** Alt+L opens the log pane, with the WARN and ERROR lines from the session's start, for a user who may view
  the System app (whose Log Viewer reads the same log). Others are told the log is not offered.
- **Background.** The client asks the terminal for its background color (OSC 11) and takes the dark or light colors
  to match; a terminal that does not answer keeps what `COLORFGBG` says, else dark.

## In the browser

The account menu (the user's name in the header) has **Terminal UI**, for every signed in user: `/qapps/Terminal`, the
same client in xterm.js, on the `/termws` WebSocket (`crates/moqui-host/src/termws.rs`). It runs as the browser's user
with no login of its own, and the apps alone, as a dialog login over SSH does. The socket is checked by origin and
session cookie as `/notws` is; a logout closes it within a minute. The limits, the idle close, the files and the log are
as over SSH. When the socket closes, its reason shows with Reconnect. `enabled="false"` on the webapp's `/termws` `endpoint` in
the conf turns the socket and the menu item off. While the terminal has focus it takes the keys, except those a browser keeps (Ctrl+W, Ctrl+T, Ctrl+N,
and in some browsers Ctrl+Q); leaving the page ends the session as Ctrl+Q does. A link the client writes (`o`) opens
in a new tab.

## For agents

`agent-apps` is the same client for a program, an AI agent above all: commands in, the screen as text out. Each
command answers once the screen has settled (its requests done, a search's wait for typing fired at once), so there is
no timing to guess and no escape sequence to read. It needs a key or password login and no pty.

```
ssh -p 2222 john.doe@localhost agent-apps open [path] [--size 120x40] [--json]     a session; answers its id
ssh -p 2222 john.doe@localhost agent-apps <id> <command>                            one command in it
ssh -p 2222 john.doe@localhost agent-apps                                           line mode: a command per line on stdin
moqui tui --agent --user john.doe                                                   line mode on this runtime (dev)
```

The server keeps an opened session between commands, so each step is one `ssh`, and the agent holds no connection. A
session is found only by its own user. It ends on `close`, or after 10 minutes without a command, and counts in
`terminal` `max-sessions` and `terminal` `max-sessions-per-user` as `apps` does. The exit status is 0 for an answer that ends
`--- ok`, 1 for `--- error`, and 2 when there is no such session or none may start.

| Command | |
| --- | --- |
| `look` | the screen again |
| `key <chord>...` | keys, as `F10`, `Enter`, `Esc`, `Tab`, `Shift+Tab`, `Down`, `Ctrl+K`, `Alt+M`, `F4` |
| `type <text>` | characters as typed; `\r` is Enter, `\t` is Tab |
| `go <path[?query]>` | go to a screen |
| `focus <element>` | move the focus |
| `set <element> <value>` | replace a field's value; a drop-down takes a choice's value or label and chooses it as a person does, so its dependent fields follow |
| `do <element>` | focus and press Enter: a button, link, dialog button, menu; with a confirm open, `do Yes` answers it |
| `do Find [list]` | open a list's find fields (as `f` on the list), and with the find open, find: the list again with the fields set, each with its own operator, and none of an empty field's flags |
| `find <text>` | move to the text on the screen, a list row's included; then `key Right` follows the row's link |
| `elements` | the focusable elements: number, kind, id, label, value, and required, changed or error marks |
| `size <cols>x<rows>` | the drawing size; a tall one shows a long list whole (the page size follows the rows) |
| `format text\|json` | the answers' form; `--json` on one command for that one |
| `help`, `close` | the commands and the screen's keys; end the session |

An element is its id, its number from `elements` (`#3`), or its label: the same label first, then one that holds the
text, ignoring case. `set` takes the longest run of leading words that names an element, so `set Thru Date 2026-10-21
00:53` needs no quotes. An answer is:

```
path /apps/system/Security/UserAccount/UserAccountDetail?userId=EX_JOHN_DOE
title system ▸ Security ▸ UserAccount ▸ User Account Detail
focus dialog AddGroupMemberDialog "Add Group"
overlay none                                  (a dialog, a confirm with its choices, a drop-down, the palette, ...)
message Saved                                 (or: error ..., or no line)
<the screen as text; the focused element between [[ and ]]; long runs of empty rows as one counted line>
--- ok                                        (or: --- error <why>)
```

`--json` gives `{ok, reason, session, path, parameters, title, focus, overlay, message, messageError, screen, elements}`,
with every row of the screen. An element that runs a transition which is not read-only (a link to one, a form's submit
button) has `writes: true`. A name that finds no element answers the nearest labels, as `--- error no element named
"Svae"; did you mean: Save (button)?`.

The Assist agents use the same client in process, one session per conversation (plans/LLM-A2A-Assist.md 4.7): there
each render is fenced and every write is held for the user's approval, so the answers mark such elements `[needs
approval]` and add a `[needs approval] Save, Delete` line. `agent-apps` writes with its own credentials and is not
gated.

A worked example, the Tools task, abridged (the whole of it took 20 commands on Oct 21, 2026):

```
agent-apps open                                      session hFY74uac ...
agent-apps hFY74uac go /apps/system/Security/UserAccount/UserAccountList
agent-apps hFY74uac key f                            the list's find dialog
agent-apps hFY74uac set Username john
agent-apps hFY74uac key Enter
agent-apps hFY74uac find EX_JOHN_DOE                 the row
agent-apps hFY74uac key Right                        its account
agent-apps hFY74uac do Add Group
agent-apps hFY74uac set Group My Account Users
agent-apps hFY74uac do Add                           message Saved
agent-apps hFY74uac go /apps/system/ServiceJob/Jobs/ServiceJobDetail?jobName=clean_ServiceJobRun_daily
agent-apps hFY74uac do Run Job                       overlay confirm "Run job now ...", choices No|Yes
agent-apps hFY74uac do Yes                           message Done
agent-apps hFY74uac close
```

With the login dialog open (`moqui tui --agent --server`, or a session that lost its login), its fields are the
elements: `set Username john.doe`, `set Password ...`, `set Code 123456`, then `do Log in`. The password is never in an
answer, as text or JSON. An answer of `--- error Enter the authentication code ...` means the password was right and
the account wants its code.

**Logging in.** Each step is its own `ssh`, so let it not ask for a password every time:

- Give the agent a key: a `UserSshKey` row with the key's fingerprint, for the agent's user.
- Or keep one connection open and run every step over it, with this in `~/.ssh/config` (the first `ssh` logs in, the
  rest reuse it):

  ```
  Host moqui-agent
      HostName localhost
      Port 2222
      User john.doe
      ControlMaster auto
      ControlPath /tmp/moqui-agent-%C
      ControlPersist 30m
  ```

  A control socket path must be short: a Unix socket path holds at most 108 bytes.
- Where a desktop is around, `ssh` may ask for a password in a window (`SSH_ASKPASS`). Set `SSH_ASKPASS_REQUIRE=never`
  so it asks on the terminal, or not at all.

## Keys

The scheme follows 5250 function keys and Lynx link travel. Single-letter keys act only when focus is not in a text
field; function keys and Alt combinations work everywhere. No default uses Ctrl+A, Ctrl+B, Ctrl+Z, Ctrl+\ or Ctrl+], so
tmux, screen, job control and ssh escapes are left alone. F1 lists the keys in use, a theme's included.

| Key | Action |
| --- | --- |
| Tab, Down / Shift+Tab, Up | Next / previous field, link, button or list |
| Enter | In a field: submit its form (a find form finds). On a link: follow it. On a button: press it. On a list: edit the row when it has fields, else open its first link. On a menu button: its items. On a box title: open or close it. In a text area: a new line |
| Right / Left | Not in a field: follow / back (on a list row, its first link). In a field: move the cursor. On a tree: open / close a node |
| Alt+Enter, Ctrl+S | Submit the focused form from anywhere, a text area included |
| Esc, F12 | Close the top dialog or list; else clear the message; else leave the edits, after asking |
| F1, `?` | The keys, the focused component's keys, and the screen's documents: a document's number opens it |
| F3, Alt+Left, Backspace (not in a field) | Back to the screen before |
| F4 | Prompt: a drop-down's list, a date's picker, a text find's operator |
| F5, Ctrl+R | Refresh the screen |
| F6, `+` | Quick create: the apps' create forms, each in a dialog |
| F7 / F8, PgUp / PgDn | Previous / next page of the focused list (else the first list); on a wizard with no list, PgDn is Next |
| Shift+Left / Shift+Right | Scroll a list wider than the screen; its first column stays |
| F9 | Show the drawer and focus it; again to hide it. The choice is kept for the user (`TUI_DRAWER_OPEN`) |
| F10 | Actions: the screen's button menus, then the focused list's row actions, Edit row, Find, Saved finds, Columns and exports |
| Ctrl+K, `:` | The palette: apps, recent screens, actions, and what the server's search finds as you type |
| `g` | Go to a path (`/apps/...`) or a menu title |
| `/`, `n`, `N` | Find text on the screen (links, fields, list rows), then the next and previous match |
| Space, `*` | On a list: select the row, select all. On a check box: toggle |
| `f`, `s`, `e` | On a list: the find dialog of its header fields, cycle the sort, edit the row |
| `v` | On a list with `views`, or its board or calendar: the next view (list, board, calendar), kept for the user |
| Alt+B, Alt+I, Alt+K | In a Markdown text area: bold, italic or link the word at the cursor |
| Alt+1…Alt+9, 1…9 | Subscreen tab N |
| `#` | Number the links; then digits and Enter follow link N |
| `=` | Info: the screen, its address, the focused link's target |
| `o` | Open the screen or the focused link in the browser (its address on the message line) |
| Alt+L | The log pane (in local mode; over SSH for a user who may view the System app) |
| Alt+M | The last 25 messages |
| Alt+T | The tray: Today, Chat and Notifications; Left and Right change the tab |
| Alt+P | The mouse off (the terminal's own selection and menu back) or on again, kept for the user (`TUI_MOUSE`) |
| F2, Alt+A | The Assist pane: show and focus it; again, close it ("Assist" below) |
| Alt+X | Stop Assist's run |
| Ctrl+L | Redraw |
| Ctrl+Q | Quit, after asking if a form changed |

In a drop-down list, typing narrows it, Up and Down move, Enter picks and Esc closes; a search drop-down (an
`ac-transition` text line, or `server-search`) asks the server for what is typed once the typing stops. In the date
picker the arrows move a day or a week, PgUp and PgDn a month, Home is today and Enter picks.

A list row with fields (a list with a `transition`) opens in the row editor with Enter or `e`: its fields under their
column titles; Enter or Ctrl+S saves the row, or every row of a `multi` list. A row action (`row-selection` `action`)
is on F10, and its form posts the rows selected with Space. In the Columns dialog Space shows or hides a field,
Alt+Up and Alt+Down move it, Enter saves and `r` puts back the list's own columns. An export saves a file to
`XDG_DOWNLOAD_DIR` (else `~/Downloads`), with its path on the message line; the text export shows in a dialog.

The title line shows the counters (`Today 3  Chat 1  Notif 2`) when the server has a count service. They change when a
notification comes for the user (in process, and over `--server` through `/notws`), with its title on the message
line, and are read again every ten minutes besides; a client that cannot be told reads them each minute. The short form
`T3 C1 N2` takes over below the md width or when the long one would cut the breadcrumb, then the user and the clock give
way. The breadcrumb ends with the screen's title in place of its name.

A list with `views` (`view-board`, `view-calendar`, `view-tree`; style guide §8.2) opens in the view the user chose last
(the `LIST_VIEW_` preference the browser keeps), else the first the form names. The board shows the page's rows as cards in
their status columns, and `m` moves a card through the board's transition as a drag does in the browser; the calendar
is the agenda of the rows' dates; the tree is the page's rows as an indented outline under their parents, `-` before an
open row with children and `+` before a closed one, its roll-ups right-aligned (Left and Right close and open a row; the
browser's levels fetched on open and its moves are not in the terminal yet). `v` goes to the next view.

An `editor-type="md"` text area shows its Markdown source styled as it is typed: headings and links in the primary
color, bold and italic, code in the info color, and the markers (`#`, `**`, backticks, a link's brackets and url, list
bullets) muted. The source is the value, so nothing is converted and nothing is lost. F4 shows the preview and the
source again. Rich-text fields (`html`, `rich`) stay read-only, with "open in browser". A text area longer than its
rows scrolls with its cursor, and PgUp, PgDn and the wheel move it by its rows; `▲` and `▼` on its bar say there is
more above or below. The preview scrolls the same way.

A row of buttons wider than the screen goes on as many lines as it needs.

Leaving a changed form (a link, F3, Ctrl+Q) asks: Stay, Discard or Save. A button or link with a `confirmation` asks
No or Yes, starting at No.

## The mouse

The keyboard does everything; the mouse is a second way in. It is on in every channel (local, `--server`, SSH and the
browser terminal) until the user turns it off with Alt+P, which is kept as the `TUI_MOUSE` preference.

- **Click.** A click does what Enter does on what is under it: a link follows, a button presses, a tab opens, a box
  title opens or closes, a menu item, a drop-down choice, a palette row, a confirm's choice or a date in the picker is
  taken. On a field it only takes the focus; a check box toggles and a drop-down opens its list. A click on a list row
  makes it the current row, and a double click is Enter on it; its first column, or a link in a cell, follows. A
  column title with a sort sorts by it, ascending, then descending; the selection box `[ ]` selects. The title line's
  crumbs open the palette, the `≡` of a hidden drawer opens the drawer, and a key line entry presses its key. While a
  drop-down list, a menu, the palette, a confirm or the login dialog is open, a click elsewhere does nothing, and a
  click beside a dialog does not close it, so no edits are lost to a stray click.
- **The wheel** scrolls the screen three rows, until the next key or click; over a text area it scrolls the text, and
  over an open list or menu it moves the highlight. It never turns a list's page (PgDn does).
- **Drag to copy.** A drag selects the screen's text in reading order, or a rectangle with Alt held (a column of a
  list); a double click on plain text takes a word, a triple click the row. On release the text goes to the clipboard
  by OSC 52 (up to 64 KiB) and the message line says how many characters. kitty, WezTerm, Alacritty, foot, Windows
  Terminal and xterm (with its window ops allowed) take OSC 52, iTerm2 when allowed, tmux with `set-clipboard on`.
  GNOME Terminal and the other VTE terminals do not: there, Shift+drag (Option+drag on macOS) is the terminal's own
  selection, which every terminal keeps while an app has the mouse, or Alt+P turns the mouse off. A password field is
  drawn as `•`, so it is never copied. In the browser terminal the page puts the text on the clipboard, or shows it to
  copy by hand when the browser refuses.

## Widths

The client measures the canvas: the terminal less a docked drawer. The width classes keep the browser's names, so
`row-col` and `field-col` sizes apply as written.

| Class | Canvas columns | What changes |
| --- | --- | --- |
| xs | under 80 | Labels sit above values; dialogs are full screen; the mobile column set is asked for |
| sm | 80–119 | One column of boxes; a `field-row` keeps its cells side by side while each has its label and 24 columns |
| md | 120–179 | Two columns: `row-col md` and `lg` sizes apply; panels and splits show their sides |
| lg | 180–239 | As md, three columns where `row-col lg="4"` asks |
| xl | 240 and up | As lg |

The 26-column drawer docks while the canvas left beside it is still md, from a terminal of 146 columns; F9 shows or
hides it at any width. A box narrower than 40 columns goes on a line of its own rather than squeeze. A list's page size
follows the rows of the terminal (10 to 100).

## Themes

A screen theme (the user's `STT_INTERNAL_QUASAR` theme, else `DEFAULT_QUASAR`) can set the client's values through
`ScreenThemeResource` rows, each `name = value`:

| Resource type | Names | Example |
| --- | --- | --- |
| `STRT_TUI_BREAKPOINTS` | `sm`, `md`, `lg`, `xl`, `drawer` (columns) | `md = 110` |
| `STRT_TUI_COLORS` | `text`, `primary`, `secondary`, `positive`, `negative`, `warning`, `info`, `muted`, `field`, `focus`, `changed`, `selection`, `border` | `primary = #1976d2` |
| `STRT_TUI_KEYS` | an action name: `next`, `prev`, `activate`, `submit`, `back`, `cancel`, `help`, `prompt`, `refresh`, `page-prev`, `page-next`, `drawer`, `select`, `select-all`, `find`, `sort`, `view`, `number-links`, `info`, `open-browser`, `redraw`, `log`, `palette`, `go-to`, `quick-create`, `actions`, `find-text`, `find-next`, `find-prev`, `messages`, `tray`, `edit-row`, `assist`, `assist-stop`, `quit`, `tab-1` to `tab-9` | `back = f3 \| alt+left` |

A value that does not read is logged and the built-in one stays. Esc, Enter and Tab keep what they do whatever a theme
binds.

- **Brand colors.** The theme's browser brand colors (`STRT_QUASAR_CONFIG`, `{"brand": {"primary": …}}`) are the
  terminal's `primary`, `secondary`, `positive`, `negative`, `warning` and `info` too; a `STRT_TUI_COLORS` row wins.
- **Dark and light.** A color name may start `dark.` or `light.` (`dark.primary = #64a0ff`) to apply on that
  background only; it wins over the same name without one. The background is the terminal's answer to OSC 11 over
  SSH, else `COLORFGBG`, else dark. In local mode only `COLORFGBG` is read: an answer that came late would arrive as
  keys.
- **No color, ASCII.** Color is never the only signal: every cue has a glyph or text too. `NO_COLOR` (or `--no-color`)
  draws in reverse and underline only, and `--ascii` (also a `TERM` of `linux` or `vt100`, or a locale that is not
  UTF-8) draws borders, symbols and the client's own marks (`…`, `–`, `·`) in plain ASCII. The Gallery snapshots check
  both on every screen (`crates/moqui-host/tests/tui_screens.rs`).

## For screen authors

A screen needs nothing extra for the terminal: forms, lists, boxes, rows, dialogs, links and the find widgets draw as
they are. Where a screen has gone custom for the browser (a `.qvue` screen, or a `render-mode` text of type `qvt`), it
needs a `tui` text of its own beside it to show in the terminal; until it has one, the terminal shows a placeholder with
the browser address. `moqui lint --screens` lists those parts under "TUI worklist".

A `tui` text is one node or an array of nodes of the model, in JSON:

```xml
<render-mode>
    <text type="qvt"><![CDATA[<m-chart :data="..."/>]]></text>
    <text type="tui"><![CDATA[{"t":"text","style":"h3","text":${JSON.stringify("Total " + total)}}]]></text>
</render-mode>
```

Every value goes through `JSON.stringify`: a bare `${}` can write a quote that breaks the JSON, as a bare `${}` in qvt
text is an HTML injection. The lint reports a `${` outside `JSON.stringify(`. A text that is not JSON, or not nodes,
becomes an error node naming the screen, and the rest of the page is kept. A static file uses `template="false"`.

The node types and their properties are documented with the other client contracts in
`runtime/base-component/webroot/screen/webroot/js/README.md` ("The tui screen model"); the serde types in
`crates/moqui-tui/src/model.rs` are the schema.

### Components

A `.qvue` screen's twin is usually one `component` node: a client widget registered by name. The screen adds `tui` to its
`render-modes` and puts the text beside its `qvue` text; the server sets the component's `base` to the screen's path, so
the component's transitions are the screen's own, the ones the `.qvue` file calls:

```xml
<screen render-modes="vue,qvue,tui">
    <widgets><render-mode>
        <text type="qvue" location="component://example/screen/Gallery/Board.qvue" template="false"/>
        <text type="tui" location="component://example/screen/Gallery/Board.tui.json" template="false"/>
    </render-mode></widgets>
</screen>
```

```json
{"t": "component", "name": "board", "props": {"source": "boardRows", "groups": "stages", "groupOf": "stage",
 "fields": ["amount", "owner", "next"], "move": "moveCard", "moveGroup": "stage", "open": "../Detail?opportunityId={id}"}}
```

The framework's components, with their props documented at the top of each file in `crates/moqui-tui/src/components/`:

| Name | Shows | Keys |
| --- | --- | --- |
| `board` | Cards in columns by group from lg, else grouped under headings; `source` answers `{groups, rows}`, or the props hold them (a list's board view) | Up/Down, Left/Right (wide), `m` moves a card through `move`, Enter opens `open` |
| `calendar` | An agenda by day, with the month grid from lg; `source` answers `{month, events}`, or the props hold the events; an event's `_open` is its screen | Up/Down, PgUp/PgDn months, `t` today |
| `chart` | A table of the datasets (`labels`, `series`) with a bar for the first series | `r` reads again |
| `timeline` | Entries (`items`) in order, each its time, title and text | Up/Down, Enter |

A name the client does not know draws a placeholder naming it, and `=` lists the components the client has: a remote
client knows only those it was built with. An add-on crate registers its own behind a `tui` feature of its crate, which
`./moq build` turns on when the binary has the TUI:

```rust
#[cfg(feature = "tui")]
inventory::submit! { moqui_tui::Registered(&MyInbox) }
```

There is no `diagram` component: Revue's Mermaid widget was tried and does not draw a status flow usably, so a
Mermaid diagram's twin is text (Tools' Status Flows lists its transitions).

A component implements `moqui_tui::Component`: where its data comes from (a transition under the screen's path), its
lines at a width (spans in theme roles, so it needs no Revue), its keys (moves, and what it asks of the session: reload,
post, follow, a message), and its help.

## Assist

F2 (or Alt+A) opens the Assist pane, the terminal's copy of the browser's Assist drawer (docs/Assist.md,
plans/LLM-A2A-Assist.md 4.12): the user asks in the pane, the agent works the user's own screens as the user, this
session follows it, and every write waits for the user's approval. It speaks the drawer's `/assist/*` calls through the
same JSON lane the palette uses, so it works in process, over SSH, in the browser terminal and over `--server`. The
code is `crates/moqui-tui/src/assist/` (`state.rs` the reducer, `draw.rs` the pane) and `src/session/assist.rs` (its
keys, calls and follow); the tests are `tests/assist.rs` with the `assist-*` snapshots.

**Where.** A 48-column right column while the canvas beside it stays md: from 168 columns with the app drawer hidden,
194 with it (with the pane open the drawer docks only when both fit, or when the user keeps it open with F9, which then
needs 194). Narrower, the pane covers the right of the screen while it has the keys, all of it below 80 columns; Esc
gives the keys back and hides it, F2 brings it back as it was. From the top: the conversation's title, the thread (the
user's messages, the answers, a step row per tool call such as `· screen /apps/tasks/EditTask ok`, a note for each
decision, the done line `✓ Done · 2 steps · 940 tokens · 3 s`), the cards waiting, the status line (Working, Waiting
for your approval, …) and the composer.

| Key | In the pane |
| --- | --- |
| Enter | In the composer: send. On a card: review it |
| F4 | The composer in a box of several lines; Alt+Enter (or Ctrl+S) sends, Esc puts the text back in the composer |
| Tab, Shift+Tab | Composer, cards (when any wait), transcript |
| Up, Down, PgUp, PgDn | On the cards: the next card. In the transcript: scroll |
| `o` | The conversation's canvas in the browser (the screen's address with `?assist=`) |
| Alt+X | Stop the run; what it wrote so far stays |
| F10 | New conversation, History, Tasks (what waits for the user across surfaces), Agent, Follow on or off |
| Esc | The keys back to the screen; F2 again closes the pane |

A message is a turn with `surface=tui`, a client id made once per session, a message id per message, the page version
and the screen's path and parameters as `state`. While a run works the pane reads its stored events
(`GET /assist/events?runId=&after=`) about once a second, at once after a decision, an answer or a stop, and when an
`AgentRun` notification comes (the in-process link and `/notws` carry the notification's payload as `Push.message`);
it stops while the run waits for the user. Events apply in sequence; one that skips a sequence reads again from the
last applied, and text that cannot be placed reads the conversation again. An error answer
(`{errorCode, errors, code}`) shows its `errors` on the message line.

**Follow.** The agent's `ui.navigate`, `ui.fill`, `ui.dialog` and `ui.focus` apply to this session when they are this
client's (its id and page version, docs/Assist.md "Follow") and follow is on: the session goes to the screen
(`Session::navigate`), and a fill sets the field (`Session::fill`, by the event's `tuiId`, else by its form and field
name), marks it changed and tags it `Assist` while it holds that value, with "Assist filled Status" on the message
line. A fill waits until the screen it follows has come; a password, file, hidden or display field is never filled. A
navigation while the user has unsaved edits is not followed. Events of another client, an older page version, or
with follow off show as notes ("Assist opened Edit Task (not followed here)"). The user's own navigation raises the
page version and posts `context`, so the agent goes on from where the user went and its older page events are
dropped. A hand-off (Edit in screen) fills the form and then focuses its Save with "Assist filled Edit Task — review,
then Enter on Save"; the keys go to the screen.

**Approvals.** An approval is a card in the pane: `! Approval · write`, its summary, a `Field: current → proposed`
line per change, and Approve, Edit in screen and Reject. Enter on the card opens the confirmation with the same lines;
only Enter on a choice decides (no letter does, and Esc leaves it pending). Reject asks for the feedback, which becomes
the agent's next task. The server takes a decision only from a person's channel (docs/Assist.md `POST decide`): the
in-process terminal (local, over SSH, in the browser terminal) renders as `Terminal`, so its decisions count; over
`--server` the link signs in with a key, a header login a program could use, so a decision (and a hand-off's Skip) is
refused with 403 and is made in the browser instead. A question card lists its options (the recommended one marked), Enter offers them and "Other:
type an answer" when the question takes text.

**Not in the terminal.** Canvases are not drawn (the canvas line and `o` open the browser), nor Markdown in the answers
(they show as written), record chips, attachments, dictation, suggested prompts, response actions, thumbs, the approval's
argument editor, Preview effects and the context meter; History and Tasks open a conversation but do not search,
rename or delete. The mouse does nothing in the pane.

## Not yet

Per-user key bindings, a two-way rich-text editor, and a `diagram` component wait; plans/Remaining-Work.md lists them.
