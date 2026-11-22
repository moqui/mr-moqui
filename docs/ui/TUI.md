# The Terminal UI

`moqui tui` shows the apps in a terminal. It draws the same XML Screens the browser does, through the `tui` render mode:
the server writes each screen as a JSON document of nodes, and the client in `crates/moqui-tui` draws it with
[Revue](https://github.com/hawk90/revue). There are no TUI-only screens. The design is in
[plans/archive/Screen-TUI-Impl.md](../../plans/archive/Screen-TUI-Impl.md).

It is in every package, Default (`lite`) included (the `tui` feature, [Packaging](../Build-Test.md#packages)). `apps` over SSH also
needs the `ssh` feature, which Default leaves out.

## Starting it

```
moqui tui                                   the apps on this runtime, after a login
moqui tui --user john.doe                   log in as john.doe (an instance@purpose of dev only)
moqui tui --path /apps/gallery/List         start on a screen
moqui tui --serve                           also open the HTTP, SSH and cluster ports, for the browser beside it
moqui tui --snapshot /apps/gallery/Detail --size 100x40     print one screen as text and exit
moqui tui --server https://erp.example.com  the apps of a running server, over HTTP
moqui tui --keys classic                    the Classic keys for this run (Keys)
```

The client runs in this process on this runtime. It opens no port unless `--serve`, and it writes nothing to stderr: the
log goes to the log files and to the log pane (Alt+L). If a server already answers on the runtime's HTTP port, `moqui
tui` says so and stops: stop the server first, or reach it with `moqui tui --server https://localhost:8443`.

Each screen is rendered as the logged-in user with artifact authorization on, as the browser's requests are. The client
holds no permission logic: what the user may not do is not in the document.

**The login dialog** asks for the username and password, and the code of an account with a second factor (an
authenticator app, a single-use code, or one sent by email or SMS). A right password moves the dialog to Code, says
which factors the account has, and keeps the password for the next try only; Alt+S sends a code to the first factor
that takes one, when there is one. Esc closes the dialog and forgets what was typed.

On a system with no user accounts yet (after a production data load; the seed accounts nobody logs in as do not count),
the dialog creates the first administrator, as the browser's `/Login` page does. As it opens it asks the server
(`UserServices.get#InitialAdminNeeded`, over HTTP the Login screen's `initialAdminNeeded`), and shows the login until
the answer comes. On a system with no accounts the dialog becomes "Welcome to your new system: create the first
administrator", with Username, Password, Password again, Full name and Email, and the password rules ("at least 12
characters, 1 digit, 1 other"). From another machine than the server it has a Setup secret field too, with where the
secret is (the server's startup log). The last field, Also join ADMIN_ADV, is a check, off unless chosen: Space flips
it, `y` and `n` set it, and a note under the rules says what the group holds. Tab, Down, Up and Shift+Tab move between
the fields; Enter goes to the next field and sends from the last, and Ctrl+S or Alt+Enter sends from any. The client checks that every field is filled and the two
passwords match before it sends. The account is made by `create#InitialAdminAccount`, in the `ADMIN` group (and
`ADMIN_ADV` with the check), and the
dialog then logs in with it, as a login does (with `--server` the login key is kept). A refusal (the password rules,
an email that is not one) shows in the dialog and clears both passwords. If an account exists by then, someone else
made the first one: the dialog is the login again, with the username kept. Over SSH a refused creation counts as a
refused login, and the session still closes without a login within 2 minutes ("Over SSH"). In `--agent` the overlay is
`initial admin`, its fields are the elements (`set Full name Ada Admin`; `do login.joinAdvanced` sets the check), and
`do Create and log in` sends it. While `user-facade/login@initial-admin-enabled` is off (the default conf), the server answers that no setup
is needed, so the dialog stays the login.
`GET /Login?renderMode=tui` (and `--snapshot /Login`) shows the same form while no account exists.

## A running server (`--server`)

`moqui tui --server <url>` is the same client for a server that is already running, here or elsewhere. It boots no
runtime: each screen is a `GET <path>?renderMode=tui`, each form and transition asks for JSON, and every request
carries a login key. `--agent`, `--snapshot`, `--ascii` and `--no-color` work with it as they do on the runtime.
The connection keeps HTTP session cookies in memory so one-time values survive redirects; the login key is checked on
each request. An explicit `api_key` or `login_key` takes precedence over cookie or Basic authentication, and a key for
another user is refused against an existing session. Session cookies are not written to the credentials file.

- **Login.** The client asks for the username and password once and posts them to the REST `login` with
  `createLoginKey=true` and a `deviceName` (`moqui tui on <host>`), the sign-in the desktop client makes too
  (`moqui_lib::remote_login`). The server answers with a device's login key (`login-key/@device-expire-hours`), listed
  in the user's devices where it can be revoked, which the client keeps in
  `~/.config/moqui/tui/<host>_<port>.toml` (or under `XDG_CONFIG_HOME`), readable by the user only. The password is not
  kept. The next run uses the key; when the server stops taking it (it expired, or was revoked) the login dialog comes
  back. A sign-in the server answers with no key is refused. `moqui tui --server <url> --forget` removes the key. An account with a second factor
  gets its code asked for in the dialog ("The login dialog", above).
- **https.** Plain `http://` is refused except to this machine (`localhost`, `127.0.0.1`, `::1`), since the login
  would cross the network in the clear.
- **Files.** Exports and attachment links are saved to `XDG_DOWNLOAD_DIR` (else `~/Downloads`) on the user's machine,
  as in local mode, including `--agent`. An attachment leaves the current screen and focus in place; its saved path or
  write error appears on the message line. The server's attachment response is kept as bytes, even for JSON files.
- **Counters.** The client opens `/notws` with its key, so the counters change when a notification comes, and the
  notification's title shows on the message line. A dropped socket connects again after a wait that doubles up to a minute, at a random point
  in the upper half of each step, so the clients of a restarted server do not all connect at once. Without the socket it reads them each minute, as before.
- **Build.** The client alone builds without a database driver or the HTTP server:
  `cargo build -p moqui-host --no-default-features --features tui` (docs/Build-Test.md).

## Over SSH

When the built-in SSH server runs (`ssh-server@port`, see `crates/moqui-host/src/ssh/server.rs`), the same client runs on an
SSH channel, in the server's process, for any user, as the browser is for any user:

```
ssh -p <ssh-server@port> <user>@<host>             the apps; with SSH_SHELL and a key or password login, the admin shell first
ssh -t -p <ssh-server@port> <user>@<host> apps     the apps, for any account
moqui> apps                                 from the admin shell; quitting the apps (Ctrl+Q) comes back to the prompt
moqui> attach <id>                          one of your kept terminal sessions (sessions lists them); Ctrl+\ detaches
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
  the System app and read the `otel_logs` dataset, both granted with `SYSTEM_APP` (Monitor › Logs reads the same log).
  Others are told the log is not offered.
- **Background.** The client asks the terminal for its background color (OSC 11) and takes the dark or light colors
  to match; a terminal that does not answer keeps what `COLORFGBG` says, else dark.

## In the browser

The account menu (the user's name in the header) has **Terminal UI**, for every signed in user: `/qapps/Terminal`, the
same client in xterm.js, on the `/termws` WebSocket (`crates/moqui-host/src/termws.rs`). It runs as the browser's user
with no login of its own, and the apps alone, as a dialog login over SSH does. The socket is checked by origin and
session cookie as `/notws` is. The limits, the idle close, the files and the log are as over SSH. `enabled="false"` on
the webapp's `/termws` `endpoint` in the conf turns the socket and the menu item off. While the terminal has focus it
takes the keys, except those a browser keeps (Ctrl+W, Ctrl+T, Ctrl+N, and in some browsers Ctrl+Q) and the terminal's
own Ctrl+Shift chords. A link the client writes (`o`) opens in a new tab.

The Terminal is a workspace of sessions in tabs and splits that outlive the page, with the `moqui>` command line and
shells on the server for a user who may on a dev instance, floating terminals and a status per session: an apps
session left by a reload or a dropped connection is kept two minutes and draws itself again for the next window, and a
logout leaves it. See [Terminal](Terminal.md).

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
| `type <text>` | characters as typed; `\r` is Enter, `\t` is Tab, `\s` a space; the text is trimmed, so a space at its start or end is `\s` (or `\ ` at the start) |
| `go <path[?query]>` | go to a screen |
| `focus <element>` | move the focus |
| `set <element> <value>` | replace a field's value; a drop-down takes a choice's value or label and chooses it as a person does, so its dependent fields follow. `set <element>` alone, or with `""` as its value, empties it (over ssh, quote it from the local shell: `'""'`) |
| `do <element>` | focus and press Enter: a button, link, dialog button, menu (a submit button by its text); a subscreen tab by its title (one past the ninth too, which has no number key); with a confirm open, `do Yes` answers it |
| `do Find [list]` | open a list's find fields (as `f` on the list), and with the find open, find: the list again with the fields set, each with its own operator, and none of an empty field's flags |
| `find <text>` | move to the text on the screen, a list or tree row's included (it becomes the current row), and a button menu by its items; then `key Right` follows the row's link (an agent's keys are Classic unless `--keys modern`). A subscreen tab is no stop: `find` names it and how it opens |
| `elements` | the focusable elements: number, kind, id, label, value, and required, changed or error marks |
| `size <cols>x<rows>` | the drawing size; a tall one shows a long list whole (the page size follows the rows) |
| `format text\|json` | the answers' form; `--json` on one command for that one |
| `help`, `close` | the commands and the screen's keys; end the session |

An element is its id, its number from `elements` (`#3`), or its label: the same label first, then the one label that
holds the text as whole words, ignoring case (`No` does not name `Notify`; words held by two labels name neither, and
the answer lists them). `do` on a check or radio is refused: set it. `set` takes the longest run of leading words that names an element, so `set Thru Date 2026-10-21
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
agent-apps hFY74uac go /apps/system/Operations/JobDetail?jobName=clean_ServiceJobRun_daily
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

There are two key schemes. **Modern**, the default, has the keys of today's terminal apps and desktop editors: the
arrows move, Enter follows, Backspace goes back, Esc opens the palette when nothing is open, and the vi keys that clash
with nothing work too (`j`, `k`, `/`, `n`, `:`). **Classic** is the 5250 green screen with Lynx link travel: F3 goes
back, F12 cancels, Right follows a link and Left goes back.

- **Choosing.** The palette's **Keys: switch to …** row changes the scheme and keeps it as the user's `TUI_KEYS`
  preference. **Terminal keys** in the browser's Appearance dialog does the same.
- **No choice yet.** A user who has not chosen gets the theme's `STRT_TUI_KEY_SCHEME` (Green Screen's is Classic),
  else Modern.
- **One run.** `moqui tui --keys modern|classic` uses a scheme for one run and keeps nothing.
- **Agents.** An agent (`--agent`, `agent-apps`, Assist's screen tools) uses Classic unless `--keys` says otherwise.

F1 names the scheme and lists the keys in use, a theme's included. Single-letter keys act only when focus is not in a
text field; function keys and Ctrl and Alt combinations work everywhere. Ctrl+C copies and never stops the client;
Ctrl+Q quits. No default uses Ctrl+B, Ctrl+\ or Ctrl+], and none uses Ctrl+Z outside a text field, so tmux, job
control and ssh escapes are left alone. Classic leaves Ctrl+A alone too, for GNU screen.

The keys that differ:

| Action | Modern | Classic |
| --- | --- | --- |
| Next / previous field, link, button or list | Tab, Down, Right, `j` / Shift+Tab, Up, Left, `k` | Tab, Down / Shift+Tab, Up |
| Follow, press, open (see Enter below) | Enter | Enter, Right |
| Back to the screen before | Backspace (not in a field), Alt+Left | F3, Alt+Left, Left and Backspace (not in a field) |
| Esc | Close the top dialog, list or row editor; with nothing open, the palette | Close the top dialog or list; else clear the message; else leave the edits, after asking (F12 too) |
| Previous / next page | PgUp / PgDn | F7 / F8, PgUp / PgDn |
| Prompt: a drop-down's list, a date's picker, a text find's operator | F4, Alt+Down | F4 |
| The palette | Ctrl+K, Ctrl+P, `:` | Ctrl+K, `:` |
| Go to a path (`/apps/...`) or a menu title | `g`, Ctrl+G | `g` |
| Find text on the screen, then the next and previous match | Ctrl+F, `/`; F3, `n`; Shift+F3, `N` | `/`; `n`; `N` |
| Quick create | `+` | F6, `+` |
| Select all rows | Ctrl+A, `*` | `*` |

**Enter.** In a field it submits the field's form (a find form finds). On a link it follows the link, and on a button it
presses it. On a menu button it opens the items, and on a box title it opens or closes the box. In a text area it is a
new line. On a list row:

- in Modern, Enter follows the row's link, and edits the row when it has no link;
- in Classic, Enter edits the row when it has fields, and opens its link otherwise. Right follows the link.

The row's link is the link of the field the list names in `row-open-field`, when the row has one (Monitor › Logs opens
a line's Surrounding entries, not its user's account); else its first link to another screen. A link back to the screen
shown (a filter on a value, or a toggle that stages a change, as on Access Capabilities) opens the row only when it has
no link to another screen, as a list beside its detail selects a row with one. A card of a board, an event of a
calendar and a row of a tree open the same way: the link the server gives each (`_open`) is its `row-open-field` link,
else its first link to another screen, else its first back to this one.
The `row-open-field` link of a list row may post (BOSS's notification center reads a notification as it opens its
record): Enter posts it, after its confirmation, and goes where it answers. Any other link that posts or writes never
opens the row; it stays on F10 and on numbered links. A row with no link to open says "Nothing to open on this row" (and points to F10 when it
has other links). The show-total row opens nothing. In both schemes `e` edits the row.

**Lists and trees.** In both schemes:

- On a tree, Right opens a node and Left closes it. Classic's Right also follows a leaf.
- On a radio, Left and Right change the choice.
- In the confirm buttons, the tray and the date picker, Left and Right move as they always have.

In Modern, Left and Right move as Up and Down do in menus and the drawer, and `j`/`k` move through a list's or a
tree's rows.

The keys both schemes share:

| Key | Action |
| --- | --- |
| Home, End | A list's first and last row of the page, else the first and last thing to focus; on a tree its first and last rows |
| PgUp, PgDn on a tree | Ten rows up and down |
| Alt+Enter, Ctrl+S | Submit the focused form from anywhere, a text area included |
| F1, `?` | The keys, the focused field's or component's keys, and the screen's documents: a document's number opens it |
| F5, Ctrl+R | Refresh the screen |
| Shift+Left / Shift+Right | Scroll a list wider than the screen; its first column stays |
| F9 | Show the drawer and focus it; again to hide it. The choice is kept for the user (`TUI_DRAWER_OPEN`) |
| F10 | Actions: the screen's button menus, then the focused list's row actions, the row's links as `column ▸ text`, Edit row, Find, Saved finds, Columns and exports |
| Space | On a list: select the row. On a check box: toggle |
| `f`, `s`, `e` | On a list: the find dialog of its header fields, cycle the sort, edit the row |
| `v` | On a list with `views`, or its board or calendar: the next view (list, board, calendar), kept for the user |
| Ctrl+C | Copy: in a text field the selection (else all of it), on a list the current row's cells, on a link its text |
| Ctrl+PgUp / Ctrl+PgDn | The previous / next subscreen tab |
| Alt+1…Alt+9, 1…9 | Subscreen tab N. Only the first nine tabs show a number (`1:Contact`); the others are reached with Ctrl+PgDn or a click |
| Ctrl+N / Ctrl+P | Down and up in a drop-down's list, a menu and the palette |
| Alt+B, Alt+I, Alt+K | In a Markdown text area: bold, italic or link the word at the cursor |
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

**A text field** edits as a desktop editor does, in both schemes:

| Key | In a text field or text area |
| --- | --- |
| Shift+Left, Right, Home, End (Up, Down in a text area) | Select; Ctrl+Shift+Left and Right select a word |
| Ctrl+Left, Ctrl+Right; Ctrl+Home, Ctrl+End | A word left and right; the text's start and end |
| Ctrl+C, Ctrl+X | Copy, cut: the selection, else all of the field. It goes on the terminal's clipboard (OSC 52) and is kept for Ctrl+V |
| Ctrl+V | Paste what was copied in the client (a field's, a row's, the code editor's), through the field's length and mask. The terminal's own paste (Ctrl+Shift+V) brings the system clipboard in |
| Ctrl+Z, Ctrl+Y | Undo, redo; a run of typing is one step |
| Alt+Backspace, Ctrl+Backspace; Ctrl+Delete | Delete the word before the cursor; the word after it |
| Ctrl+U; Ctrl+E | Delete back to the line's start; go to the line's end |
| Ctrl+A | Select all of the field (Modern) |
| Esc | Drop the selection; with none, Esc is the screen's |

Typing, Backspace, Delete and a paste take the place of the selection. The selection is drawn underlined in the
selection color. Many terminals send Ctrl+Backspace as Backspace; Alt+Backspace works in all of them.

In a drop-down list, typing narrows it, Up and Down move, Enter picks and Esc closes; a search drop-down (an
`ac-transition` text line, or `server-search`) asks the server for what is typed once the typing stops. A `scan-input` is a
search drop-down over its screen's `scanResolve` transition: the code typed or scanned is the search, its matches the choices,
and the form posts the id with a scan id the screen made when it rendered ([Qvt-Client](Qvt-Client.md#scan-input)). In the date
picker the arrows move a day or a week, PgUp and PgDn a month, Home is today and Enter picks.

A list draws its column titles unless it has `skip-header`; a column of submit buttons has none, as in the browser. A
column whose field has `align="right"`, or that shows numbers, is right-aligned, its title too; `align="left"` keeps a
number column left. The paging footer names the find's values (`Find: Status = Approved`), a drop-down's, radio's or
check's by its option's label. A column wider than the room left is drawn cut to that room
(at least 8 columns), and Shift+Right brings the columns after it; a row takes the lines of its tallest cell among the
columns drawn. A cell's row actions (an icon or button link, a link that posts, a dialog, a button) share one line. The
`show-total` row is drawn bold under the rows, with only the totals' columns filled, no checkbox and nothing to open; it
is not counted in the footer. A `pre` text in a dialog wraps at the dialog's edge rather than being cut, and a dialog
blanks a column and a row around its frame.

A list row with fields (a list with a `transition`) opens in the row editor with Enter or `e`: its fields under their
column titles; Enter or Ctrl+S saves the row, or every row of a `multi` list. A row action (`row-selection` `action`)
with a `dialog` is on F10; one without is its form after the list. Either form posts the rows selected with Space. F10 also offers the current row's dialogs and write links,
including those inside a cell container; each keeps its own row's values and confirmation. It then lists the row's
navigation links as `column ▸ text` (a dashboard row's Name, Receivable, Payable and Settings), since Enter and Right
open only one of them; they also remain on numbered links. Labels and links in a cell's containers show in the order the screen
lists them, each under its own name. In the Columns dialog Space shows or hides a field, Alt+Up and Alt+Down move it,
Enter saves and `r` puts back the list's own columns. An export saves a file to `XDG_DOWNLOAD_DIR` (else
`~/Downloads`), with its path on the message line; the text export shows in a dialog.

The title line shows the counters (`Today 3  Chat 1  Notif 2`) when the server has a count service. They change when a
notification comes for the user (in process, and over `--server` through `/notws`), with its title on the message
line, and are read again every ten minutes besides; a client that cannot be told reads them each minute. Below the md
width, or when the long form would cut the breadcrumb, the middle form with three-letter words (`Tdy 3 Cht 1 Ntf 2`: a
word's first letter and the two consonants after it) takes over, then the short form `T3 C1 N2`, then the user and the
clock give way. The breadcrumb names each level by its menu title (`Bills of Materials`, not `BillsOfMaterials`), the
envelope's `pathTitles` beside `pathList`; a level whose title is empty or does not expand, and an extra path's name (a
wiki page), keeps its name, which the screen URL decoded (`Getting started`). A level named as the one after it (a screen whose
default subscreen is its own list) is one crumb, as in the browser.

A list with `views` (`view-board`, `view-calendar`, `view-tree`; style guide §8.2) opens in the view the user chose last
(the `LIST_VIEW_` preference the browser keeps), else the first the form names. The board shows the page's rows as cards in
their status columns, and `m` moves a card through the board's transition as a drag does in the browser; the calendar
is the agenda of the rows' dates; the tree is the page's rows as an indented outline under their parents, `-` before an
open row with children and `+` before a closed one, its roll-ups right-aligned (Left and Right close and open a row; the
browser's levels fetched on open and its moves are not in the terminal yet). `v` goes to the next view.

An `editor-type="md"` text area shows its Markdown source styled as it is typed: headings and links in the primary
color, bold and italic, code in the info color, and the markers (`#`, `**`, backticks, a link's brackets and url, list
bullets) muted. The source is the value, so nothing is converted and nothing is lost. F4 shows the preview and the
source again. Rich-text fields (`html`, `rich`, `rich-light`) stay read-only: their HTML is shown as its text, a block a
line, with a line under it to edit it in the browser. A text area longer than its
rows scrolls with its cursor, and PgUp, PgDn and the wheel move it by its rows; `▲` and `▼` on its bar say there is
more above or below. The preview scrolls the same way.

A row of buttons wider than the screen goes on as many lines as it needs. In a container whose style has the `row`
class (`row:true` in the document) a section is a grouping, not a block: the buttons of its sections (a
`section-iterate` of one link each, Product 360's tabs) flow on the container's line as its own buttons do. Elsewhere
each section starts its own line.

Leaving a changed form (a link, Back, Ctrl+Q) asks: Stay, Discard or Save. A button or link with a `confirmation` asks
No or Yes, starting at No.

A wizard's Next (and PgDn on a step with no list) posts the step's form, changed or not, so its validation runs, and
the next step follows the answer; a step with no form goes on at once. The step's form is the first form inside the
wizard that is not a list's, a row action's or in a dialog, as the browser's `stepForm`.

After a refused post each field the server names is marked with its message; the errors the server lists too (the same
message, or Java's form of it, `Enter a name(for field Name`) are not repeated on the message line, as the browser
leaves them out. A dialog among a button menu's items is listed by its button text, else its title. A link button with
no `btn-type` is drawn as a plain one and a `primary` one bold, so a row of link buttons that marks the current one
`primary` (Product 360's tabs) shows it without color; a link's check icon (`check`, `done`, `check_circle`,
`task_alt`) is drawn as `✓` before its text (Cases' current view). An action its author shows disabled for a reason
(`disable-condition` and `disable-message` on a link or a container-dialog) is drawn `[ text ]` dimmed and takes focus:
the hint line gives its text and the reason, and Enter gives the reason as the message and does nothing. A link disabled
with no reason (its transition out of the user's reach) is left out, as before.

A successful write and F5 refresh the current screen and its cached parent screens, so a record's header and actions
reflect child-screen changes. Moving between child tabs, or resizing the terminal, reuses those parents. Each parent
screen is asked for naming the screen below it it is loaded for (`X-Moqui-Page-Path`): one the user may not open alone,
above a screen they may open (a self-service area under an app's area), answers as that screen's frame, its tab bar of
the subscreens they may open and nothing else of its own ([Attack-Surface](../security/Attack-Surface-Web.md#parent-frames)).
A parent that renders as itself takes the menu items' `inPath` (`sri.getMenuItems()`, its tab bar's active tab) from
that screen's path, when it is on the parent's screens below it, as a render of the whole path does: a screen that marks
its current tab by `tab.inPath` (Product 360) marks it in the terminal too. Internal
links under `/qapps` use the terminal's `/apps` mount, a download reached by following a link included; explicitly
opening a link in the browser preserves its browser address, as do external links. A form's or transition's own
JSON request keeps its authored route.
An explicit `renderMode=html-print` link also opens its print view in the browser, keeping its filters and the current
terminal screen.
An authored `target-window="_blank"` GET link does the same, including an inline document in a list row. Download links
without that browser target keep the terminal's file-saving behavior; write links keep their transition and confirmation.

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
hides it at any width. In it `▾` marks an app with its screens under it, `•` the screen shown (bold), and `◂ Home`, the
last row, is the link to all the apps. As the browser's drawer, it opens the current path below the app's top-level
screens: each screen on the path that has subscreens lists them under it, a level more indented (from the screen's
menu data, `menuData`), so Payroll's runs and settings are there though Payroll has no tab bar; a screen whose
subscreen on the path has its own title gives way to it. The arrows walk every row. Where rows share a screen, as a saved find (`?formListFindId=`) does its Find
screen, the one whose parameters the screen shown holds is marked, and the plain row only when none is. A box narrower
than 40 columns goes on a line of its own rather than squeeze, where it stands among the others (a label column stays
above its value). A word too long for its line, a URL, is broken across lines rather than cut. A list's page size
follows the rows of the terminal (10 to 100).

## Themes

A screen theme (the user's `STT_INTERNAL_QUASAR` theme, else `DEFAULT_QUASAR`) can set the client's values through
`ScreenThemeResource` rows, each `name = value`:

| Resource type | Names | Example |
| --- | --- | --- |
| `STRT_TUI_BREAKPOINTS` | `sm`, `md`, `lg`, `xl`, `drawer` (columns) | `md = 110` |
| `STRT_TUI_COLORS` | `text`, `primary`, `secondary`, `positive`, `negative`, `warning`, `info`, `muted`, `field`, `focus`, `changed`, `selection`, `border` | `primary = #1976d2` |
| `STRT_TUI_KEYS` | an action name: `next`, `prev`, `activate`, `submit`, `back`, `cancel`, `help`, `prompt`, `refresh`, `page-prev`, `page-next`, `drawer`, `select`, `select-all`, `find`, `sort`, `view`, `number-links`, `info`, `open-browser`, `redraw`, `log`, `palette`, `go-to`, `quick-create`, `actions`, `find-text`, `find-next`, `find-prev`, `messages`, `tray`, `edit-row`, `assist`, `assist-stop`, `quit`, `first`, `last`, `tab-next`, `tab-prev`, `copy`, `tab-1` to `tab-9`; with `modern.` or `classic.` before it (`classic.back`), for that key scheme only | `back = f3 \| alt+left` |
| `STRT_TUI_KEY_SCHEME` | `modern` or `classic`: the scheme of a user who has not chosen one (Keys) | `classic` |

A value that does not read is logged and the built-in one stays. Esc, Enter and Tab keep what they do whatever a theme
binds. A `STRT_TUI_KEYS` row replaces the action's chords in the scheme in use. A row for one scheme (`modern.find-next
= f7`) wins over the same action's row without one, and a row for the other scheme is left out.

- **Brand colors.** The theme's browser brand colors (`STRT_QUASAR_CONFIG`, `{"brand": {"primary": …}}`) are the
  terminal's `primary`, `secondary`, `positive`, `negative`, `warning` and `info` too; a `STRT_TUI_COLORS` row wins.
- **Dark and light.** A color name may start `dark.` or `light.` (`dark.primary = #64a0ff`) to apply on that
  background only; it wins over the same name without one. The background is the terminal's answer to OSC 11 over
  SSH, else `COLORFGBG`, else dark. In local mode only `COLORFGBG` is read: an answer that came late would arrive as
  keys.
- **No color, ASCII.** Color is never the only signal: every cue has a glyph or text too. `NO_COLOR` (or `--no-color`)
  draws in reverse and underline only, and `--ascii` (also a `TERM` of `linux` or `vt100`, or a locale that is not
  UTF-8) draws borders, symbols and the client's own marks (`…`, `–`, `·`) in plain ASCII. The Gallery snapshots check
  both on every screen (`crates/moqui-host/tests/tui_screens.rs`), an acceptance test of the framework that
  `./moq accept-all` runs ([the two suites](../Build-Test.md#the-two-suites)).

## For screen authors

A screen needs nothing extra for the terminal: forms, lists, boxes, rows, dialogs, links and the find widgets draw as
they are. Where a screen has gone custom for the browser (a `.qvue` screen, or a `render-mode` text of type `qvt`), it
needs a `tui` text of its own beside it to show in the terminal; until it has one, the terminal shows a placeholder with
the browser address. `moqui lint --screens` lists those parts under "TUI worklist".

In a `form-single`, a link in a field with a `title` is the field's value, with the title as its label; a link in an
untitled field is one of the buttons under the form. A field with `title=""` has no label (not its field name), as in
the browser.

A list whose row has no single record to open, where its first link to another screen is not what the row is about,
names the field whose link opens it: `<form-list row-open-field="context">` (the XSD documents it). Enter and Right on
the row, and a card or event of its views, open that link; when the field has no link on a row, the row opens as
without it. The browser does not open rows by a link and ignores it. A label with a `condition` beside such a link (the text shown when
there is nothing to link to) adds no line when its condition fails.

A screen's `menu-group` (or its `default-menu-group`) is a heading in the drawer above the first of each run of menu
items that share it, as the caption in the browser's drawer ([Qvt-Client](Qvt-Client.md), Menu groups).

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

The code inside a `${}` is raw JavaScript, not JSON text: it is not JSON-escaped, so a string in it is written as in
a script (`'it\'s'`), and it may hold `//` and `/* */` comments and regular expressions
([Screen-Output, Templates](Screen-Output.md#templates)). Only the text outside the `${}` is the JSON.

The node types and their properties are documented with the other client contracts in
[Qvt-Client](Qvt-Client.md) ("The tui screen model"); the serde types in
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
| `artifact-browser` | A reference graph as Miller columns: what uses the center, the center (its lines and its links, each with its key), what it uses, each side in sections by kind; `source` answers `{center, usedBy, uses, kinds, filter}` and is asked again (`Act::Call`) for each new center or filter. A row on the trail is marked `↺`. Narrower than 100 columns the panes stack and one shows at a time. Dev AIDE's Browser twin (`Artifacts/BrowseTui`) | Up/Down, PgUp/PgDn, Home/End in a column; Left/Right the column (or pane); Enter makes the row the center; Backspace and `]` back and forward on the trail; `k` the next kind; `h` hide framework; `x` hide observed-only; each link's own key (`t` Text, `y` Try, `d` Data, `p` Preview, `r` Refs, `D` Doc) |
| `board` | Cards in columns by group from lg, else grouped under headings; `source` answers `{groups, rows}`, or the props hold them (a list's board view) | Up/Down, Left/Right (wide), `m` moves a card through `move`, Enter opens `open` |
| `calendar` | An agenda by day, with the month grid from lg; `source` answers `{month, events}`, or the props hold the events; an event's `_open` is its screen | Up/Down, PgUp/PgDn months, `t` today |
| `calcgrid` | A CalcGrid workbook's sheet ([Database](../tools/Database.md#calc-mode)): the formula bar, the column letters, the cells that fit (frozen rows and columns kept, numbers right, a text running on into empty cells, bold and underline), the sheet tabs, and who else has it open and where; `room` is what `open#CalcDocument` answered when the screen drew, and the screen's `calc*` transitions answer the rest | Arrows, PgUp/PgDn, Home/End, Shift to select; Enter or `e` edits a cell, `=` or a digit starts one (Tab takes a completion); Delete; `u`/`U` undo and redo; `b` `i` `_`; `[` `]` sheets; `<` `>` width; `F` freeze; `d` fill down; `c` charts; `z` kiosk; `R` refresh; `m` the rest |
| `chart` | A table of the datasets (`labels`, `series`) with a bar for the first series | `r` reads again |
| `code` | An editor for one text: line numbers with problem marks (`!` an error, `·` a warning), light highlighting, long lines scrolled sideways, and a status line (Ln/Col, the language, `●` when changed, the problem at the cursor); below. Dev AIDE's twins, System › Conf's Edit (`Conf/EditTui`) and Tools › SQL (`SqlTui`) | Arrows, Home/End, PgUp/PgDn, Ctrl+Home/End; Ctrl+Left/Right by word; Shift to select; Ctrl+C/X/V; Tab/Shift+Tab indent and outdent; Esc lets go; Ctrl+Z/Y; Ctrl+G; Ctrl+F, F3/Shift+F3; Ctrl+Space; F8/Shift+F8 |
| `timeline` | Entries (`items`) in order, each its time, title and text | Up/Down, Enter |

The `code` component's props:

- `text`; `language` (`xml`, `js`, `json`, `sql`, `md`, else plain text); `readOnly`; `cursorLine` (1-based), where
  the cursor starts; `rows`, the lines shown (else what fits the terminal).
- `form` and `field` name a field of the form the component is in, usually a `hidden` one. Each edit sets that field as
  typing in a field does, so the form's own submit posts the text: Ctrl+S or Alt+Enter with the editor focused submits
  the form `form` names.
- `problems`, `[{line, column, severity, message}]`, 1-based, severity `error`, `warning` or `info`.
- `check`: a transition posted with `text` and `language` 800 ms after the typing stops. Its answer's `problems`
  take the place of the props'.
- `complete`: a transition for Ctrl+Space. For XML it answers a schema digest (`{digest}`, as `get#SchemaDigest` does
  for the `rootElement` posted). The component keeps the digest and offers from it the elements the enclosing element
  takes, the attributes the tag does not have yet, or an attribute's values. For another language it answers a list of
  choices (`items`, `completions`, or the list itself; each a string or `{label, insert, detail}`), asked for with
  `text`, `line`, `column`, `prefix` and `language`. The list opens under the cursor's line: typing narrows it, Up and
  Down choose, Enter or Tab takes one, Esc closes it.
- `parameters`: an object of parameters posted with `check` and `complete` as well, such as a root and a path. A
  query string on either url is posted too.

A focused component sees each key before the screen's keymap, so the editor keeps Tab, Shift+Tab, Esc, PgUp/PgDn, F3
and F8. Esc lets go of the focus: then Tab and Shift+Tab move to the next field and a second Esc is the screen's (a
dialog closes), and any other key takes the focus back. Ctrl+F opens a find line under the status line; F3 and
Shift+F3 go to the next and previous match, and F3 opens the find line when nothing has been searched. Ctrl+G asks for a
line number. Ctrl+C and Ctrl+X put the selection, or the cursor's line when nothing is selected, on the terminal's
clipboard (OSC 52, as a drag does) and keep it for Ctrl+V. The terminal does not hand its clipboard to a program, so
text copied elsewhere comes in with the terminal's own paste (Ctrl+Shift+V, or Cmd+V), which the editor takes as one
edit. Undo keeps the last 200 edits; typed characters on one line undo together. A read-only text refuses edits with a
message; it does not hold Tab, so Tab moves on from it. The keys the editor does not use go on to the screen's: F1, F5,
Ctrl+K, Ctrl+S, the Alt keys. Ctrl+Z and Ctrl+Y act only in the editor and in text fields; no screen key uses them.

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
post, follow, a message), and its help. Its state's `rows` is the terminal's rows, for a component that loads what
fits on screen; `Act::Call` posts to a transition that answers JSON and gives the answer to `receive`, as a read's is
(a sheet's edit); `Act::SetFormField` sets a field of a form; `Act::Copy` puts text on the terminal's clipboard;
`paste` takes text pasted while it has the focus; `poll_millis` and `tick` run it on a timer (the editor's check after the
typing stops); and a `receive` that sets the state's
`reload_pending` asks for its source again at once (the next read of a component whose `source` changes with what it
received).

## Assist

F2 (or Alt+A) opens the Assist pane, the terminal's copy of the browser's Assist drawer (docs/ai/Assist.md,
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
client's (its id and page version, docs/ai/Assist.md "Follow") and follow is on: the session goes to the screen
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
the agent's next task. The server takes a decision only from a person's channel (docs/ai/Assist.md `POST decide`): the
in-process terminal (local, over SSH, in the browser terminal) renders as `Terminal`, so its decisions count; over
`--server` the link signs in with a key, a header login a program could use, so a decision (and a hand-off's Skip) is
refused with 403 and is made in the browser instead. A question card lists its options (the recommended one marked), Enter offers them and "Other:
type an answer" when the question takes text.

**Not in the terminal.** Canvases are not drawn (the canvas line and `o` open the browser), nor Markdown in the answers
(they show as written), record chips, attachments, dictation, suggested prompts, response actions, thumbs, the approval's
argument editor, Preview effects and the context meter; History and Tasks open a conversation but do not search,
rename or delete. The mouse does nothing in the pane.

## Not yet

A user's own key for one action (only whole schemes are chosen per user), vi and emacs key schemes, a two-way rich-text
editor, and a `diagram` component wait; plans/Remaining-Work.md lists them.
