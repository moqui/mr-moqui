# Screen TUI implementation plan

> Archived 2026-10-21: Phases 1 to 5 are built, with the findings under each in section 5. The open parts (live checks in real terminals and browsers, per-user key bindings, a two-way rich-text editor, a `diagram` component, board sums and calendar create, the upstream Revue asks) are in [Remaining-Work](../Remaining-Work.md). The user guide is [docs/TUI.md](../../docs/TUI.md).

Oct 21, 2026 · Phases 1 to 5 built (findings under each in section 5): the browser terminal (4.4) included, SSH opened to every user (4.2, "Changed Oct 21"), the leftovers of Phase 4 (a second factor in the login dialog, the client address, text areas that scroll, a dialog per list row) and the mouse with drag to select and copy. What is still open is in [Remaining-Work](../Remaining-Work.md). This plan implements [archive/Screen-TUI-Revue.md](Screen-TUI-Revue.md). Revue was read at 3.10.1 (commit `011741e`, the clone at `~/work/moquiwork/3p_other/revue`). Revue paths below are relative to that clone. Every other path is relative to this repository.

The proposal sets the architecture: a `tui` render mode emits a JSON screen model, and one Revue client interprets it and reaches the server over several channels. This plan adds four things:

- the terminal UI design: a TUI variant of every widget the `qvt` mode uses, the shell frame, the width classes and the keyboard scheme;
- the server work;
- the client crate;
- the channels.

It ends with phases, tests and the open questions, updated from what reading Revue showed. Code appears only where it shows a nuance.

---

## 0. Decisions this plan takes

Most of these are beyond what the proposal settles.

| # | Decision | Why |
| --- | --- | --- |
| D1 | `tui` is a JS macro module, `ScreenTui.js`. It is registered like the other modes and emits JSON through `sri.write`. There is no Rust model builder. | Every mode is built this way (`crates/moqui-core/src/screen/generate.rs:46` `MODE_MODULES`). The JS macro walk already handles field chains, conditional fields, layouts and form assembly (`ScreenSupport.js`), and a module can be overridden from a resource (`generate.rs:445` `mode_source`). |
| D2 | `tui` is **always-standalone**: each screen level is its own request, as in `qvt`. | This is the qvt protocol (`runtime/base-component/webroot/screen/webroot/js/README.md:130`, "A level is its own screen"). The client reloads only from the first level that changed. Parent levels such as a record header above tabs stay as they are. Pre-actions start at index 2, as qvt's do (`crates/moqui-core/src/screen/render.rs:781-784`). |
| D3 | The client is a new crate, `crates/moqui-tui`. Its `model` module has no Revue import and is usable alone through a `model` feature. Revue is an optional dependency behind the `ui` feature (3.1). | This is the proposal's single adapter layer. It also lets `moqui-core` tests deserialize the model without pulling in Revue. A Ratatui client stays possible. |
| D4 | `moqui-tui` does no I/O of its own. It defines a `ScreenLink` trait, and `moqui-host` implements it (in-process and HTTP) behind a new `tui` feature. | `crates/moqui-host/Cargo.toml:9` says only `moqui-host` may depend on Tokio, axum, russh and the like. Revue is used with `default-features = false`, so its `async` feature does not pull in Tokio. |
| D5 | Revue is a **render pipeline and widget kit**, not the app framework. The interpreter owns focus, key routing, navigation and the form state. | Revue routes no keys to widgets. The app does that (`docs/refactor/findings-event-dispatch.md`, and `src/core/app/event_loop.rs:171`). Its three focus systems are not connected. `Form`/`FormFieldWidget` only render (`src/widget/form/form/mod.rs`). None of the examples uses its routers. |
| D6 | The SSH and browser channels run a **custom event loop**, in process, one per session. A child process on a pseudo-terminal is the fallback if the Phase 1 spike fails. **Built (Phase 3b) as route 4 of 3.5:** our own frame loop, which draws into a Revue `Buffer` and writes the diff itself, on Revue's public API. | `App::run` is tied to the process: it hardcodes stdout and crossterm's `poll`/`read` (`event_loop.rs:76,94`), installs signal handlers (`src/core/app/signals.rs`) and installs a global panic hook (`src/runtime/render/terminal/panic_hook.rs:76`). None of that can run per session in a server. `Terminal<W: Write>` itself is generic (`src/runtime/render/terminal/core.rs:23`), and `PipelineHarness` already drives the real draw path into memory (`src/testing/pipeline.rs:42`). |
| D7 | Width classes are measured in **canvas columns**, which is the terminal width less a docked drawer. The second column starts at **120**. | Section 1.3. On the user's 180-column terminal the drawer docks and the canvas is 153 columns, which gives two columns of boxes holding forms. |
| D8 | The keyboard scheme follows **5250/3270 function keys plus Lynx link travel**, and every action has a key. | Section 1.5. This settles the proposal's "Keyboard conventions" open question. |
| D9 | Every string from the server is stripped of control characters before it is drawn. | A field value holding ESC or OSC bytes would otherwise drive the user's terminal. Revue writes cell characters as given. |
| D10 | **A screen that goes custom in one render mode needs its own implementation in each other mode it should work in.** A `.qvue` screen, or inline `render-mode` text of type `qvt`, gets a `tui` text beside it: a model fragment that may name a client component. Until it has one, it shows a placeholder with a browser link. | This is the user's rule. Custom presentation does not translate between modes, but the screen's transitions and data are shared, so only the presentation is written twice (section 2.9). |
| D11 | Key bindings are a **theme value**, with the scheme of section 1.5 as the default. Per-user bindings and a UI for them wait. | One scheme per deployment keeps the help, the key line and `docs/TUI.md` true for everyone. |
| D12 | Rich-text fields (`editor-type="html"`, `rich`, `rich-light`) are read-only in the TUI, with "open in browser". Markdown fields (`editor-type="md"`) are edited as source with a preview. A two-way Markdown editor that looks like `markdown-view` comes later (section 3.8). | No terminal editor today edits rich text, or Markdown in place, without losing content (section 3.8). |

---

## 1. UI design

### 1.1 Principles

- **Green screen first.** Fixed regions, dense forms with dot-leader labels, a message line, and a function-key line that names what each key does on this screen. Nothing needs a mouse. Revue gives no mouse for free, since the client paints its own cells. The mouse came in Phase 5: a click does what Enter does on what is under it, the wheel scrolls, and a drag selects text and copies it.
- **Lynx for links.** Up and Down move through links and fields in reading order. Right or Enter follows a link, and Left goes back. `/` finds text on the screen. `#` numbers the links (`[12]Acme Supply Co`), so typing `12` then Enter follows that link. `=` shows where the current link goes and the browser URL of the screen. HTML content gets a numbered References list at its end.
- **Same screens, same terms.** The TUI renders the XML Screens as written. There are no TUI-only screens and no TUI attributes in the XSD for the first release. Where a screen has gone custom for the browser (a `.qvue` file, inline `qvt` text), it adds a `tui` text of its own (D10, section 2.9). It does not get a separate TUI screen. The BOSS style guide's widget rules (`runtime/component/ironstone-boss/plan/style-guide.md` §4) are the visual source: one primary action per form, status as a chip, numbers right-aligned, a list's first column links to the record.
- **Color is never the only signal.** Every color cue has a glyph or text cue. `NO_COLOR` and monochrome terminals stay fully usable, and an ASCII mode replaces box drawing and symbol glyphs.

### 1.2 The frame

The frame has the same regions as the qvt shell (`m-qapps-root`, `WebrootVue.qvt.js:6498`; style guide §2.1). Each is mapped to terminal rows and columns.

| Region | Terminal form | qvt counterpart |
| --- | --- | --- |
| Title line (row 1) | `≡` when the drawer is hidden, then app ▸ breadcrumbs ▸ screen title on the left. On the right: counts (Today, Chat, Notif), user, clock, and a busy spinner while a request runs. | `q-header`: launcher, app title, breadcrumbs, counters, account |
| Drawer | Left, 26 columns. The app menu tree (two levels under the app root), pinned saved finds, then Settings and Home. Docked from 146 terminal columns, an overlay (F9) below that. | `q-drawer` 260 px, `m-menu-nav-item` |
| Canvas | The screen levels, stacked from the app root down. It scrolls vertically. A form-list can scroll horizontally on its own. | `q-page` > `m-subscreens-active` |
| Message line (row −2) | The last message. An error stays until Esc or the next action. Alt+M opens the history (last 25). | `$q.notify` and `notifyHistoryList` |
| Key line (row −1) | The function keys that apply to the focused element, then the global ones, cut to the width. Revue's `StatusBar` with `KeyHint`s (`src/widget/feedback/statusbar/`). | none (green-screen convention) |
| Overlays | Palette, dialogs, drop-down lists, help, confirmations, toasts | `m-shell-palette`, `m-dialog`, `q-menu` |
| Log pane (local only) | A bottom split of 10 rows, toggled with Alt+L | none |
| Tray | Today / Chat / Notifications as a right panel 48 columns wide when the canvas left beside it is still ≥120 (1.3), otherwise a modal | `m-shell-tray` |

The frame at 180 columns, drawer docked, with a canvas of 153 columns (md, two columns). Links are numbered (`#` on).

```
 Moqui ▸ Sales ▸ Orders ▸ SO10023 · Order Detail                                                                        Today 3  Chat 1  Notif 2   john.doe   Tue 14:32
──────────────────────────┬─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 ▾ Sales                  │ ┌─ SO10023  Acme Supply Co ───────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
   Dashboard              │ ┃ Status: [Approved]  ● Placed ─ ● Approved ─ ○ Shipped ─ ○ Completed      < Ship >  [ Hold ]  [ Cancel ]  [ More ▾ ]                                 │
 ▸ Customers              │ ┃ 1:Detail  2:Items  3:Shipments  4:Payments  5:Notes                                                                                                 │
 ▾ Orders                 │ └─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
   ▸ Find Orders          │ ┌─ Order ─────────────────────────────────────────────────────────────────┐ ┌─ Customer ──────────────────────────────────────────────────────────────┐
   ▶ Order Detail         │ │ Order ID . . . . . : SO10023                                            │ │ Customer . . . . . : [12]Acme Supply Co                                 │
   ▸ Returns              │ │ Order date . . . . : [2026-10-20 ]  F4                                  │ │ Contact  . . . . . : Dana Ortiz                                         │
 ▸ Quotes                 │ │ Status . . . . . . : [Approved            ▾]                            │ │ Email  . . . . . . : [dana@acme.example         ]                       │
 ▸ Reports                │ │ Ship by  . . . . . : [2026-10-27 ]  F4                                  │ │ Bill to  . . . . . : 400 Main St, Springfield                           │
                          │ │ Priority . . . . . : (•) Normal  ( ) Rush                               │ │ Ship to  . . . . . : [Warehouse dock B     ▾]                           │
 Saved finds              │ │ * Sales rep  . . . : [Jo Lin              ▾]                            │ │ Terms  . . . . . . : Net 30                                             │
   Open > $10k            │ │                                                                         │ │                                                                         │
   My approvals           │ │ < Save >  [ Reset ]                                                     │ │ [x] Notify on ship                                                      │
                          │ └─────────────────────────────────────────────────────────────────────────┘ └─────────────────────────────────────────────────────────────────────────┘
 Settings                 │ ┌─ Items · 3 of 3 ────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┐
 Home                     │ │   Seq  Product                      Description                                     Qty  Unit price       Total   Status                            │
                          │ │   ───  ──────────────────────────── ──────────────────────────────────────────── ────── ─────────── ───────────   ────────                          │
                          │ │ ▌ 01   [13]WID-100 Widget, blue     Blue anodized widget, 10 mm                      40       12.50      500.00   Approved                          │
                          │ │   02   [14]WID-220 Bracket          Steel mounting bracket                           40        3.75      150.00   Approved                          │
                          │ │   03   [15]SVC-010 Install          Installation service, per hour                    6       85.00      510.00   Pending                           │
                          │ │                                                                                                     ───────────                                     │
                          │ │                                                                                               Total    1,160.00                                     │
                          │ │ Rows 1–3 of 3 · Page 1/1                                         a=Add item  e=Edit row  Space=Select  F7/F8=Page                                   │
                          │ └─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────┘
──────────────────────────┴─────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────────
 Order SO10023 updated.
 F1=Help  F3=Back  F4=Prompt  F5=Refresh  F6=Create  F9=Menu  F10=Actions  F12=Cancel   Ctrl+K=Go to   →/Enter=Follow  ←=Back   /=Find   #=Number links
```

The same screen at 100 columns (sm: drawer hidden, one column, short labels in the title line):

```
 ≡ Sales ▸ Orders ▸ SO10023                                     T3 C1 N2  john.doe
────────────────────────────────────────────────────────────────────────────────────────────────────
 ┌─ SO10023  Acme Supply Co ──────────────────────────────────────────────────────────────────────┐
 ┃ Status: [Approved]   < Ship >  [ Hold ]  [ More ▾ ]                                            │
 ┃ 1:Detail  2:Items  3:Shipments  4:Payments  5:Notes                                            │
 └────────────────────────────────────────────────────────────────────────────────────────────────┘
 ┌─ Order ────────────────────────────────────────────────────────────────────────────────────────┐
 │ Order ID . . . . : SO10023                                                                     │
 │ Order date . . . : [2026-10-20 ]  F4                                                           │
 │ Status . . . . . : [Approved            ▾]                                                     │
 │ …                                                                                              │
 └────────────────────────────────────────────────────────────────────────────────────────────────┘
 ┌─ Customer ─────────────────────────────────────────────────────────────────────────────────────┐
 │ Customer . . . . : [12]Acme Supply Co                                                          │
 │ …                                                                                              │
 └────────────────────────────────────────────────────────────────────────────────────────────────┘
────────────────────────────────────────────────────────────────────────────────────────────────────
 Order SO10023 updated.
 F1=Help F3=Back F4=Prompt F5=Refresh F9=Menu F10=Actions F12=Cancel
```

And at 72 columns (xs: labels above values, the mobile column set of the list, horizontal scroll marked `▸`):

```
 ≡ Orders ▸ SO10023                               N2  john.doe
────────────────────────────────────────────────────────────────────────
 ┌─ Order ────────────────────────────────────────────────────────────┐
 │ Order ID                                                           │
 │   SO10023                                                          │
 │ Order date                                                         │
 │   [2026-10-20 ]  F4                                                │
 │ Status                                                             │
 │   [Approved            ▾]                                          │
 │ …                                                                  │
 └────────────────────────────────────────────────────────────────────┘
 ┌─ Items · 3 ────────────────────────────────────────────────────────┐
 │ Seq Product                 Qty     Total ▸                        │
 │ 01  [13]WID-100 Widget       40    500.00                          │
 │ 02  [14]WID-220 Bracket      40    150.00                          │
 │ …                                                                  │
 └────────────────────────────────────────────────────────────────────┘
────────────────────────────────────────────────────────────────────────
 F1=Help F3=Back F4=Prompt F9=Menu F12=Cancel
```

### 1.3 Width classes and how layouts reduce

The client measures the canvas on every `Event::Resize`. Revue has no CSS media queries (`src/runtime/style/parser/parse.rs:72-80`). Its `Breakpoints` and `ResponsiveValue` (`src/runtime/layout/responsive/breakpoint.rs:52,155`) can be driven from that canvas width, or the interpreter keeps its own small table. The classes keep the qvt names, so `row-col`/`field-col` sizes map directly. At about 8.5 px per column they also line up with Quasar's pixel breakpoints (style guide §7).

| Class | Canvas columns | Quasar | What changes |
| --- | --- | --- | --- |
| xs | < 80 | < 600 px | Labels sit above values. The `columns type="mobile"` set is used (the client sends `_uiType=mobile`, as qvt does at `WebrootVue.qvt.js:277`; `ScreenSupport.js:290`). Dialogs are full screen. The title line shows the screen title and counts only. |
| sm | 80–119 | 600–1023 | One column of boxes. A `field-row` keeps up to 2 cells side by side. Dialogs are 90% wide. |
| md | 120–179 | 1024–1439 | **Two columns**: `row-col md`/`lg` sizes apply. A `field-row` keeps up to 4 cells. A `container-split` shows side and main together. |
| lg | 180–239 | 1440–1919 | As md, plus three columns where `row-col lg="4"` asks for them. |
| xl | ≥ 240 | ≥ 1920 | As lg |

Rules:

- **Docking the drawer.** The drawer docks only when the canvas left over is still at least 120 columns, which with the 26-column drawer means a terminal of 146 columns or more. So docking never pushes a screen from md down to sm. F9 toggles it at any width, and the choice is kept as a user preference (the `QUASAR_LEFT_OPEN` counterpart, `TUI_DRAWER_OPEN`).
- **Docking the tray.** The same rule: the 48-column tray docks only when the canvas left beside it is still at least 120 columns, so from a canvas of 168. The width class is always that of the canvas left over. So docking the tray on a canvas of 180–227 moves the screen from lg to md, and a three-column `row-col lg="4"` row shows two columns while the tray is open. Neither panel ever pushes a screen below md.
- **`row-col`.** The 12-column grid works as `colOpen` does (`runtime/base-component/framework/template/screen/ScreenQvt.js:1107-1122`). When a smaller class has no size, it is 12. A column's width is the canvas width × n/12, less a one-column gutter. A box narrower than 40 columns falls to the next row rather than squeezing.
- **`field-row`.** This is width-driven rather than the fixed qvt rule (`ScreenQvt.js:2033`). Cells stay side by side while each cell gets at least the form's label width plus 24 columns. Otherwise they stack.
- **Labels.** Each form column pads its labels to the longest title in that column, capped at 24 columns (18 in sm), with dot leaders as in the mockup. A longer title ends in `…`, and its full text is on the key line while the field has focus.
- **`form-list`.**
  - Column widths come first from `print-width`/`print-width-type` through `form_list_column_char_widths` (`crates/moqui-core/src/screen/sri/layout.rs:27`), which the text mode already uses. Otherwise from the widest value on the page, between the title width and 40 columns.
  - When the total is wider than the canvas, the first column stays fixed and the rest scroll sideways, with `◂`/`▸` marks.
  - The page size follows the visible rows of the table area, from 10 to 100. The client sends `pageSize` when the area changes, which the Lynx-style full-screen pages call for.
- **Breakpoints are settings.** The breakpoints and drawer width are theme values with these defaults. One deployment can move the second-column line without code.

### 1.4 Visual vocabulary

| Thing | Look | Notes |
| --- | --- | --- |
| Editable field | `[value______]` in the field color, underscores filling the width (`size`, `maxlength`, or a default of 30) | 5250 input field. ASCII and color modes look the same. |
| Focused field | Reverse video, terminal cursor at the insert point | The terminal cursor stays on the field, so screen readers and `tmux` copy mode follow it. |
| Changed field | Changed-field color, plus `•` after the bracket | The qvt blue tint (style guide §2.6) |
| Required | `*` before the label | |
| Field error | Error color, `!` after the bracket, the message on the message line while focused, and a count in the form footer | qvt marks the field and toasts |
| Read-only value | Plain text, no brackets | style guide §4.5: a display never looks like an input |
| Drop-down | `[Approved      ▾]`. F4 or Enter opens the list. | F4=Prompt is the 5250 key |
| Date / time | `[2026-10-20 ]` with F4 for the picker | |
| Check / radio | `[x] Label`, `(•) A  ( ) B` | |
| Primary button | `< Save >`. Dim while the form is unchanged, bold and primary color once a field changes. | The outline-until-changed rule |
| Secondary (tonal) button | `[ Hold ]`. Danger types use the danger color plus a confirmation. | |
| Text link | Underlined link color. `[n]` prefix when numbering is on. | Lynx |
| Disabled | Dim, and skipped by the focus order unless `=` asks why | |
| Box | Single-line border with the title in the top edge. `type` gives a heavy left edge (`┃`) in the type color. | `m-container-box` |
| Status chip | `[Approved]` in the status color (from `StatusItem.statusColor` / the status color map) | style guide §5.2 |
| Badge | `Notif 2`, or a `(3)` suffix on a button | |
| Selected row | `▌` in the left gutter plus the selection color. Checked rows show `[x]` in a selection column. | |
| Sort | `▲`/`▼` after the column title, the active column in the primary color | |

**Theme.** Revue's widgets use fixed constants from `src/widget/theme.rs` and do not read its theme signal (`src/runtime/style/theme_signal.rs:86`). So the client defines its own `Theme` of roles: primary, secondary, positive, negative, warning, info, muted, field, focus, changed and selection. Each role maps to a Revue `Style`, and the adapter applies those to every widget it draws. The roles take their values from the server theme where it has them (brand primary and status colors) and from a built-in dark/light pair otherwise. Color depth is detected (truecolor, 256, 16). `NO_COLOR` gives monochrome, with reverse and underline only. `--ascii`, or a `TERM` of `linux`/`vt100`, uses the `ascii` border preset (`src/widget/layout/border/mod.rs:158-200`) and `v`, `*`, `>` in place of the symbol glyphs.

### 1.5 Keyboard scheme

This is one scheme for every screen. It is documented in a new `docs/TUI.md` and shown by F1. It follows three rules:

1. Single-letter keys work only when focus is not in a text field, as in Lynx.
2. Function keys and Alt combinations work everywhere.
3. No default binding uses Ctrl+A, Ctrl+B, Ctrl+Z, Ctrl+\\ or Ctrl+], so tmux, screen, job control and ssh escapes are left alone. Ctrl+S is safe because raw mode turns IXON off, but every binding that uses it also has an F-key or Alt alternative.

| Key | Action | Source |
| --- | --- | --- |
| Tab / Shift+Tab | Next / previous field or button in the form | |
| Down / Up | Next / previous focusable in reading order (fields, links, rows) | Lynx |
| Enter | On a single-line field: submit its form (a find form runs the find). On a link: follow it. On a button: press it. On a row: open its first link. | 5250 Enter, Lynx |
| Right / Left | Not in a field: follow the link / go back. In a field: move the cursor. | Lynx |
| Alt+Enter, Ctrl+S | Submit the focused form from anywhere, a text area included | |
| Esc / F12 | Close the topmost overlay. Otherwise clear the message line. Otherwise leave the form's edits, after asking if anything changed. | 5250 F12=Cancel |
| F1, `?` | Help: keys for the focused element, the screen's documents (`screenDoc`) | |
| F3, Alt+Left, Backspace (not in a field) | Back, through history | 5250 F3=Exit |
| F4 | Prompt: the list of a drop-down, the picker of a date, the operator menu of a text-find | 5250 F4=Prompt |
| F5, Ctrl+R | Refresh the screen (re-request from the current level) | |
| F6, `+` | Quick create (the shell providers' `quick-create` items) | |
| F7 / F8, PgUp / PgDn on a list | Previous / next page of the focused list | 5250 roll keys |
| F9 | Show or hide the drawer, and focus it | |
| F10 | Actions: one menu of the screen's `button-menu`s, title-row links and the focused list's row actions | |
| Ctrl+K, `:` | Palette: screens, records, saved finds, provider actions, recent screens, apps | `m-shell-palette` |
| `g` | Go to a path (`/apps/...` or a menu title) | Lynx `g` |
| Alt+1…9 or `1`…`9` (not in a field, links unnumbered) | Subscreen tab N | |
| Alt+letter | The button or tab whose mnemonic letter is underlined | |
| `/`, `n`, `N` | Find text on the screen, next, previous | Lynx |
| `#` | Number the links on or off. With numbers on, digits then Enter follow link N. | Lynx |
| `=` | Info: screen path, browser URL, the current link's target and parameters | Lynx `=` |
| `o` | Open the screen (or the focused link) in the browser: an OSC 8 hyperlink plus a copy to the clipboard where available | |
| Space, `*` (list) | Toggle the row's selection, select all | `row-selection` |
| `f` (list) | Find options dialog of the focused list (its header fields) | |
| `s` (list column) | Cycle the sort of the focused column | |
| Alt+M | Message history | |
| Alt+L | Log pane (local mode) | |
| Ctrl+L | Redraw | |
| Ctrl+Q | Quit, after asking if any form has changed | |

Revue's `KeymapConfig` and its chord parser (`src/utils/keymap/mod.rs:97`, `parse.rs:12`) hold the table. The layered priority pattern of `src/state/patterns/keys.rs:130` (modal, then confirm, then popup, then view) is the dispatch order.

**Bindings are per theme (D11).** The table above is the built-in default. A theme may override entries by action name through the `tui` theme values (section 2.6), as chord strings in Revue's syntax, for example `back = "f3 | alt+left"`. The client merges the theme's entries over the default when the shell data loads. An entry that does not parse, or that names an unknown action, is logged and skipped, and the default for that action stays.

These bindings stay fixed whatever the theme says: Esc, Enter and Tab, and the typing keys inside a text field. A binding that would take them is refused, so no theme can leave a user stuck in a dialog. F1 help and the key line are built from the merged keymap, so they always show the theme's keys. Per-user bindings, and a screen to edit them, come later; they would be one more layer over the theme's.

### 1.6 Widget catalog

There is a TUI variant for each element the qvt mode writes. The qvt sources are the macro table (`ScreenQvt.js:2498-2575`) and the wrappers in `WebrootVue.qvt.js`, which are listed by line in `webroot/js/README.md`. "Revue base" is what the adapter builds on. "Own" means a widget written in `moqui-tui/src/ui/widgets/`.

#### Shell and navigation

| Element / qvt widget | TUI variant | Revue base | Notes |
| --- | --- | --- | --- |
| Apps menu, launcher | Palette group "Apps", and the AppList screen | `CommandPalette` (`src/widget/command_palette/`) | From `shellData` (`crates/moqui-core/src/screen/shell.rs:712`) |
| App drawer (`m-menu-nav-item`) | Tree: `▾`/`▸` levels, `▶` active screen, saved finds | `Sidebar` (`src/widget/layout/sidebar/`) | From `menuData` (`crates/moqui-core/src/screen/menu.rs:150`) |
| Breadcrumbs | Title line. A crumb with siblings opens a menu on Enter when the title line has focus (F9 then Up). | `Breadcrumb` | |
| `subscreens-panel type="tab"`, `m-subscreens-tabs` | Tab bar `1:Detail  2:Items …`, active tab reversed, then the active level | `Tabs` (bar only) | Shown only with more than one tab, as in qvt |
| `subscreens-panel type="popup"` | Nothing in the canvas; the siblings are in the drawer and the breadcrumb menu | | |
| `subscreens-panel type="wizard"` | Step header `Step 2 of 4 · Options  ● ● ○ ○`, Back and Next buttons, PgDn = Next after the step form submits | `Stepper` | Follows the `m-subscreens-wizard` submit protocol (`WebrootVue.qvt.js:3659`) |
| `subscreens-panel type="stack"` | Breadcrumb stack over the active level. F3 pops. | `Breadcrumb` | |
| `button-menu` | `[ More ▾ ]`; Enter or F10 opens a menu list with badges | `ContextMenu` (`src/widget/feedback/menu/`) | |
| `tree` | Indented tree. Right expands (children fetched), Left collapses, Enter follows the node link. | `Tree` (`src/widget/data/tree/`) | Revue's tree has no lazy-load hook. The own wrapper fills children when the fetch returns. |
| Palette (`m-shell-palette`) | Top overlay with an input and grouped results | `CommandPalette` | Results from `shellSearch` (`shell.rs:952`) |
| Quick create (`m-shell-quick-create`) | F6 menu, then the create form in a dialog | `ContextMenu`, own dialog | Providers from `shell_providers` (`shell.rs:767`) |
| Counters | Title line `Today 3  Chat 1  Notif 2` | | Read from `shellCounts` at each navigation and every 60 s (Phase 4: push) |
| Tray (`m-shell-tray`) | Right panel when the canvas left beside it is still md (1.3), a modal otherwise. Tabs Today, Chat, Notifications. | `TabView` | Its screens are theme values. A `.qvue` tray screen needs its `tui` text (D10). |
| Keys overlay (`m-shell-keys`) | F1 | `Modal` | |
| Account menu | Palette group "Account": status, preferences, log out | | |

#### Containers and layout

| Element / qvt widget | TUI variant | Revue base | Notes |
| --- | --- | --- | --- |
| `container` | No border; children in a vertical stack. `row`/`col` classes in `style` are ignored. | `vstack` | |
| `container-row` / `row-col` | 12-column grid, by width class (section 1.3) | `Grid` with `TrackSize::Fr` (`src/widget/layout/grid/types.rs:7`) | CSS `grid-*` properties are parsed but not applied (`docs/guides/styling.md:182-200`), so the layout is built in code |
| `container-box` (+ header, toolbar, body) | Bordered box, title in the edge, toolbar right-aligned on the first line, type as a heavy left edge. Collapsible with Enter on the title; `initial=closed` starts collapsed. | `Border` (`src/widget/layout/border/mod.rs`) | |
| `container-panel` | Fixed side columns (`size` px ÷ 8), the center between them. Stacked below md. | `hstack` with `child_sized` | qvt never stacks panels; the TUI does, since a terminal cannot scroll the page sideways |
| `container-split` | Side and main from md, side width px ÷ 8. Below md: the side list, then main with F3 back. The selected id stays in the parameters. | `SplitView` (`src/widget/layout/split_view/`) | `m-container-split` (`WebrootVue.qvt.js:3801`) |
| `container-dialog` | Button, then a centered modal with a title and a scrolling body. The body is in the model and hidden until opened. | `Modal` (`src/widget/feedback/modal/`), `show_with_focus_trap` | Full screen at xs, as qvt is below `sm` |
| `dynamic-dialog` | Button, then a modal whose body is a TUI render of the transition URL | `Modal` + a nested level | `submit-reload-id`/`submit-hide-id` reload or close by id, as in `m-form` |
| `dynamic-container` | A nested level loaded by URL, reloadable by id | | |
| `section`, `section-iterate`, `section-include` | Resolved on the server; the client sees only the output | | `section-iterate paginate` gets the list paging footer |
| `include-screen` | Inline, as the server renders it | | |
| `dashboard` | Grid of cards, columns = canvas ÷ (`min-width` px ÷ 8) | `Grid` | |
| `number-card` | Small box: title in the edge, value bold, delta `▲ 12%` in the good/bad color, Enter follows its link | `Border` + `Text` | `m-number-card` (`WebrootVue.qvt.js:5255`) |
| `status-bar` | `● Placed ─ ● Approved ─ ○ Shipped`, then the transition buttons, each its own form | own `StatusFlow` | `m-status-bar` (`WebrootVue.qvt.js:3988`) |
| `context` | Not drawn. Kept as the screen context for quick create and the palette, as `m-screen-context` does. | | |

#### `form-single`

| Element | TUI variant | Notes |
| --- | --- | --- |
| `form-single` | A focus group. Fields in field-layout order. Enter submits. The footer holds the submit row and an error count. | Submits as `m-form` does: all fields as form parameters, plus the model's hidden fields |
| `field-layout` > `field-row` | Side by side while each cell fits (section 1.3) | |
| `field-row-big` | One wrapping line (find rows, action rows) | |
| `field-col-row` / `field-col` | Grid by width class, `label-cols` ignored (labels are padded per column) | |
| `field-group` | Subtitle line. `box="true"` draws a box. | |
| `field-accordion` | Each group a collapsible line `▸ Shipping options`; Enter toggles | Revue `Accordion` (`src/widget/layout/accordion/`) |
| `fields-not-referenced` | The remaining fields, in order | |
| `visible-when` | Re-evaluated in the client when the named field changes | Same rule data as qvt |
| `focus-field` | Initial focus | |
| `submit` / `reset` | `< Save >` / `[ Reset ]`. `confirmation` asks in a yes/no modal first. | |

#### `form-list`

| Feature | TUI variant | Notes |
| --- | --- | --- |
| Table | Header row, rule, rows, totals. Numbers right-aligned. First column fixed when scrolling sideways. | Revue `DataGrid` (`src/widget/data/datagrid/`): in-memory rows, virtual scroll of visible rows (`core.rs:301`), frozen columns, column types. The client fills it one server page at a time and turns its local sort and filter off. |
| Paging (`paginate`, `show-page-size`, `show-all-button`) | Footer `Rows 21–40 of 312 · Page 2/16`; F7/F8; `g` on the footer for a page number | Paging and sort are re-requests of the level with `pageIndex`, `pageSize` and `orderByField`, so the URL holds the state, as in Lynx |
| `header-field` find | `f` opens the Find dialog: a form of the header fields. A filter summary line appears above the table (`Status = Approved ×`) with a Clear link. | An inline filter row is too tight in a terminal, so the dialog is always used, as `header-dialog="true"` does in qvt. The summary follows qvt's (`ScreenQvt.js:1229-1240`). |
| Sort (`show-order-by`) | `s` on the focused column; `▲`/`▼` on the title | |
| `row-selection`, `multi` | Selection column `[ ]`. Space toggles, Shift+Up/Down extend, `*` selects all. F10 lists the actions; each opens its dialog or form with the selected ids. | `m-checkbox-set` semantics (`WebrootVue.qvt.js:1607`) |
| Per-row links | Text links in the cell. Enter on the row follows the first. | |
| Row forms (each row its own `m-form`) | A row is a focus group. Editing a cell marks the row changed. Enter submits that row. | |
| `multi="true"` editable rows | Inline cell editing on the focused row. Ctrl+S submits all changed rows as the multi form (`_rowSubmit_N`, `field_N`). | `DataGrid` `editable` (`input/editing.rs`) has no validation callback, so the own wrapper owns the edit buffer and validation. Fallback is the proposal's row edit modal. |
| `editable` cell (`m-editable`) | Enter opens a one-field popup and saves to the editable URL | `WebrootVue.qvt.js:1376` |
| `first-row-field`, `second-row-field`, `last-row-field` | A create row above the rows (`a` focuses it), totals and last row below | |
| `columns` / `form-list-column` | Several fields stacked in one cell (multi-line rows) | `DataGrid` rows are one line, so stacked columns need the own row renderer. Phase 3. |
| `select-columns` | Columns dialog: a check list; Alt+Up/Down reorders | `SelectionList` (`src/widget/input/input_widgets/selection_list/`) |
| `saved-finds` | Saved Finds dialog, and pinned finds in the drawer | |
| Exports (`show-csv-button`, xlsx, text, pdf) | Local and remote: saved to the downloads directory, path on the message line. SSH and browser: "View as text" only, the `text` render mode in a pager. | No file leaves the server through an SSH TUI session |
| `views` (board, calendar) | Board: groups as columns at lg and up (cards with title, fields and badge), otherwise a grouped list. `m` moves a card to another group through the board transition. Calendar: an agenda list by day; month grid at lg. | Phase 4. `m-list-views` (`WebrootVue.qvt.js:4368`) |
| Empty list | Header kept, one row "No orders match · Clear filters" or "No orders yet · New" | style guide §6 |

#### Fields

| Element / qvt wrapper | TUI variant | Revue base | Gaps and nuances |
| --- | --- | --- | --- |
| `text-line` (`m-text-line`) | `[____]`, `maxlength` enforced, `input-type="number"` right-aligned | `Input` (`src/widget/input/input_widgets/input/`) | `Input` has no max length or mask, so the own wrapper filters keys. `mask` is enforced as a pattern on input (literal characters typed for the user). `default-transition` fills a value on blur. |
| `text-line ac-transition` | Input with a suggestion list below it, fetched after 250 ms idle | `Autocomplete` (`set_suggestions`, `min_chars`) | Request runs on the worker (section 3.6). The label/value pair is kept in the wrapper. |
| `password` | `[••••••]` | `MaskedInput` (`src/widget/form/masked_input/`) | The value is never in the model |
| `text-area` (`m-text-area`) | Bordered multi-line box, `rows` high. Enter is a newline; Alt+Enter submits. | `TextArea` | `editor-type="md"`: the source is edited, and F4 toggles a preview drawn by the same renderer as `markdown-view`. `html`, `rich` and `rich-light`: the content is shown read-only, converted as `html-view` is, with `o` "Edit in browser" opening the screen there (D12). The field is left out of the form submit, so the stored HTML is never overwritten from the terminal. The later two-way Markdown editor is section 3.8. |
| `drop-down` (`m-drop-down`) | `[Label ▾]`, F4/Enter opens a list that filters as you type | Own `DropDown` over `OptionList` + `Input` | Revue `Select` holds labels only, with no values (`select/mod.rs`). `Combobox` has label/value but is builder-only, with no `set_options`. Options of 300 or fewer come in the model; above that, or `server-search`, they load from the options URL. `allow-multiple`: `MultiSelect` tags. `combo-box`: free text allowed. `allow-empty`: a blank first entry. `submit-on-select`: submits. `show-not` (find forms): `!` toggles Not. |
| `drop-down` with `dynamic-options` / `depends-on` | As above. A change of the named field re-fetches, and a value no longer in the list is cleared. | | The URL and parameter names come from the model, as `options-url`/`options-parameters` do |
| `date-time` (`m-date-time`) | `[2026-10-20 14:30]`, F4 picker. Typed text is checked against the format. | `DateTimePicker` (`src/widget/datetime_picker/`) | Revue's `Date::today()` is fixed at 2025-01-01 (`src/widget/data/calendar/date.rs:21`) and `Time::now()` at 12:00, and it has no string parser. The wrapper parses with `jiff` in the user's time zone and seeds the picker with today. |
| `date-find`, `date-period` (`m-date-period`) | From / thru pair; period mode as two drop-downs (period, offset) with `p` toggling the mode | `RangePicker` presets are wrong until the today bug is fixed | |
| `range-find` | From / thru number pair | `NumberInput` | |
| `text-find` (`m-text-find`) | Input plus an operator chip `[contains]`; F4 opens operator, Not, ignore case | | |
| `check` | `[x] Label` per option, Space toggles; one option with `[x]` alone in a find form | `Checkbox`, `SelectionList` | |
| `radio` | `(•) A  ( ) B`, Left/Right moves | `RadioGroup` | |
| `file` (`m-file`) | `[ choose… ]`. Local and remote: a file picker on the user's machine. SSH and browser: disabled, with a note. | `FilePicker` | **In SSH and browser the client runs on the server, so a picker would browse the server's disk.** Upload is off there. |
| `display`, `display-entity` (`m-display`) | Plain text; `format`, currency and `also-hidden` as on the server; `value-url` re-fetched on `depends-on` changes | `Text` | `Text` does not wrap words on its own; the wrapper uses `src/text/wrap.rs` |
| `hidden`, `ignored` | Not drawn; hidden values are in the form's parameters | | |
| `label` | `h1`–`h3` bold with a rule, `h4`–`h6` bold, `p`/`div` wrapped text, `pre`/`code` unwrapped in the code color, `strong`/`i` as bold/italic where the terminal has them | `Text`, `RichText` | |
| `image` | `▣ alt-text`, a link to open in the browser | | Kitty/sixel images (Revue `image` feature) are later and only for local mode |
| `link` (screen, transition, external, anchor, button, hidden-form) | Text link or button by the style guide's hierarchy (§4.6). `confirmation` asks. `badge` as `(n)`. External URLs are OSC 8 hyperlinks opened by the user's terminal. | `Link` (`src/widget/link.rs`), `Button` | A transition link is a JSON transition call (section 2.5). An external link is never fetched by the client. |
| `markdown-view` (`m-markdown`) | Rendered Markdown with wiki links as links | `Markdown` (feature `markdown`, pulldown-cmark already in `Cargo.lock`) | |
| `html-view` (`m-html-view`) | HTML to text on the server (blocks, lists, quotes collapsed), links numbered with a References list at the end | | No HTML-to-text helper exists in the crates; one is added (section 2.3) |
| `render-mode` `text type="tui"` | The model fragment it holds, in place (section 2.9) | | The TUI counterpart of a `qvt` text |
| `render-mode` `text` for other types (`qvt`, `qvue`, `html`) | Skipped. An `any` text in the same element is used instead, converted to text. When the element has neither a `tui` nor an `any` text, a placeholder `▣ <screen> has a browser-only part · o=Open in browser` is drawn instead, so the gap is visible. | | The lint reports each one (section 2.9) |
| Charts (`m-chart`), diagrams (`m-mermaid`) in qvt text | The screen's `tui` text names the `chart` or `diagram` component with the same data expression the qvt text uses. Until then, the placeholder. | Own `chart` component: a table of the datasets plus bars for one series. `mermaid` (`src/widget/mermaid/`) for diagrams, to evaluate. | |
| `chatter`, `record-files` | A nested level of the theme screen. The BOSS ones are `.qvue`, so they work once their screens add `tui` texts (D10). | | |
| `.qvue` screens | With `tui` in `render-modes` and a `tui` text: the fragment, usually one component node. Without: a box "This screen has no terminal view yet", the browser URL, and `o` to open it. | | The TUI checks `renderModes` in the menu data (`menu.rs:259`) before requesting, so a screen without `tui` costs no request |

### 1.7 States and feedback

These follow the BOSS style guide §6.

- **Loading.** The spinner in the title line. The old content stays, dimmed, until the new level arrives. Input is not blocked except the submit that is running.
- **Transition errors.** The message line in the error color. The form keeps its values, fields named in `validationErrors` are marked, and focus moves to the first of them.
- **Success.** The `submit-message`, or "Saved", on the message line for 3 s.
- **Confirmations.** A yes/no modal whose text names the record, defaulting to No.
- **Unsaved changes.** Leaving a changed form (navigation, F3, Ctrl+Q) asks: Save, Discard or Stay.
- **Permissions.** Actions the user may not run are absent, as the server leaves them out of the model. A screen the user may not view gives the server's 403 as a box with the way back.
- **Session.** A 401 opens the login dialog (username, password, OTP if required). After login the same request is retried.

---

## 2. Server: the `tui` render mode

### 2.1 Registration

- `runtime/base-component/framework/conf/MoquiDefaultConf.xml:671-698` gets a `screen-text-output` entry: type `tui`, mime type `application/json`, `always-standalone="true"`, module `framework/template/screen/ScreenTui.js`.
- `crates/moqui-core/src/screen/cache.rs:143` `BUILT_IN_MODES` grows to 9 entries.
- `generate.rs:46` `MODE_MODULES` gets the `include_str!`, and `generate_xml`/`generate_unit_source` (`generate.rs:518,549`) get the mode name.
- `ScreenSupport.js:33-56` lists every element a module must name, so the module is complete by construction. The golden test fails on an unnamed element.
- The menu data reports `renderModes` per screen (`menu.rs:259,308`). A screen with `render-modes="all"` (the default) includes `tui`.

### 2.2 `ScreenTui.js`

The module follows `ScreenXml.js` and `ScreenText.js`: a `prelude` with the runtime helpers, `macros` per element, and `opaque` for the elements whose children it walks itself. It uses `ScreenSupport.js` (`fieldChain` :125, `displayValueExpr` :248, `formListColumns` :293) the way the other modes do. The widget values come from the same `sri` calls the qvt mode makes:

- `getFieldValueString` (`crates/moqui-core/src/screen/sri/fields.rs:94`);
- `getFieldOptions` (`sri/options.rs:33`);
- `openFormList` / `startFormListRow` (`sri/forms.rs:194,311`);
- the URL helpers (`crates/moqui-core/src/screen/urls.rs:256` `build_url`);
- `field_validation_js_rules` (`sri/fields.rs:412`) for the client checks.

**Nuance: JSON across nested renders.** A screen is several units: the root section, named sections, `section-iterate` entries, includes and dynamic forms. Each writes through `sri.write` into one output, and a nested render writes in the middle of its parent's output. So each unit writes **one complete JSON array**, with its comma state local to the unit. A parent writes a nested render only in a value position:

```js
// in the section macro: the nested unit fills the "c" value with its own [ ... ]
$node({t: "section", id: "s3"}, () => sri.renderSection("OrderHeader"));
// $node writes {"t":"section","id":"s3","c": then runs the body, then }
```

A unit that writes nothing must still write `[]`. The golden fixtures cover an empty section, a failing section, and a section inside a `section-iterate`.

**Nuance: errors inside a section.** `guarded` (`crates/moqui-core/src/screen/sri/sections.rs:44`) writes an error as HTML text where the section would have been. For qvt it first truncates what the failed section wrote (`web::truncate_output`). For `tui` it must also truncate and then write `[{"t":"error","text":…}]`, so the document stays valid JSON. `render_single` (`sections.rs:292`), which writes action errors in place of the widgets, needs the same treatment. Rather than another string test on the mode name, `TextOutput` (`cache.rs:95`) gets an output kind (text, html, json). `guarded` and `render_single` pick the error form from that kind.

**Ids.** Every node that holds state gets an `id` that is stable across renders, so the client keeps focus, scroll, collapsed state and unsent edits:

- the XML `id` where there is one;
- forms by form name, fields by form plus field name;
- rows by the list's `id-field` (or row-selection `id-field`), else by index on the page;
- anything else by its element path within the screen (`s0.3.1`).

### 2.3 The model

The response is one JSON object. `run_job` writes the envelope around the level's widgets when the mode's output kind is json (in `render_body`, `render.rs:842`).

| Envelope key | Content | Source |
| --- | --- | --- |
| `v` | Model version, starting at 1. The client refuses a major version it does not know. | |
| `screen` | `path`, `pathList`, `title`, `renderModes`, `standalone` level index | `ScreenUrlInfo` (`crates/moqui-core/src/screen/url.rs:38`) |
| `parameters` | Current parameters, credentials stripped | `strip_credential_parameters` (`crates/moqui-core/src/web/mod.rs:1516`) |
| `messages` | `{info[], errors[], validationErrors[]}` from the flash of the last transition | `user::session::flash_messages` (`crates/moqui-core/src/user/session.rs:764`) |
| `focus` | The `focus-field` of the first form, if any | |
| `c` | The level's nodes | |

Node types (`t`), each with the properties the client needs and nothing about how qvt draws them:

| Group | Types |
| --- | --- |
| Structure | `section`, `container`, `box` (title, toolbar, `type`, `initial`), `row` / `col` (`xs`/`sm`/`md`/`lg` sizes), `panel` (header/left/center/right/footer, sizes), `split`, `dialog` (button + body), `dynamicDialog` / `dynamicContainer` (button, URL, id), `dashboard`, `subscreens` (kind: tab/popup/wizard/stack; items with title, URL, active, disabled), `active` (the placeholder for the next level) |
| Content | `text` (style h1…dd, wrap), `image` (alt, URL), `markdown`, `html` (already converted: paragraphs, list items, quotes, link references), `numberCard`, `statusBar`, `tree` (root nodes, children URL), `placeholder` (no TUI view: browser URL), `error` |
| Custom (section 2.9) | `fragment` (the nodes of a `tui` text), `component` (name, `props`, the screen's base path for its transitions) |
| Actions | `link` (kind screen/transition/external/anchor/hiddenForm, URL, method, parameters, confirm, role primary/secondary/flat/text, `btnType`, icon as a word, badge, disabled with reason), `buttonMenu` (items) |
| Forms | `form` (name, kind single/link, transition URL, method, hidden parameters, `background-*` ids, `submit-message`, layout tree of field refs), `formList` (columns with title/align/width/sort/header field, rows of cells, first/second/last rows, totals, paging `{index,size,count,maxIndex,rangeLow,rangeHigh}`, filter summary, row selection with actions, tools: find/columns/savedFinds/exports, views) |
| Fields | `field` (name, title, tooltip, required, rules, `visibleWhen`, error) with one `w` widget of kind `textLine`, `textArea`, `password`, `dropDown` (options or `optionsUrl` + `optionsParameters`, `dependsOn`, `multiple`, `combo`, `serverSearch`, `allowEmpty`, `submitOnSelect`, `showNot`), `dateTime` (type, format, min, max), `datePeriod`, `dateFind`, `rangeFind`, `textFind`, `check`, `radio`, `file`, `display` (text, `alsoHidden`, `valueUrl`), `hidden`, `submit`, `reset`, `link`, `label`, `image`, `editable` (URL, widget) |

Icons are sent as their Material Symbols names. The client maps the common ones (add, edit, delete, search, save, close, more) to a glyph or a short word, and drops the rest. HTML content (`html-view`, and the untyped `render-mode` text a few screens use) is converted to the `html` node shape on the server by a new helper module in `moqui-core`, with an allow list of block elements, link collection and the control-character strip of D9. It uses `html5ever`, already in the lock (through `ammonia`), rather than adding a parser.

The schema is written once, as the serde types in `crates/moqui-tui/src/model/`. A section of `webroot/js/README.md` documents it as the contract, as that file documents the qvt JSON today.

### 2.4 Options, dependent options and autocomplete

- **Static options** are inline, from `getFieldOptions`.
- **`dynamic-options`, `ac-transition`, server-search and `depends-on`** are sent as a URL plus parameter names, never pre-fetched beyond the initial value's label. The server already labels the current value (`field_transition_value`, `sri/options.rs:227`).
- The client calls the transition with `Accept: application/json` and reads the same list or `{options:[…]}` shape that `dynamic_options` reads (`sri/options.rs:99`), using the model's `label-field`/`value-field`.
- `default-autocomplete-rows` (`MoquiDefaultConf.xml:671`) caps the suggestion lists.

### 2.5 Transitions and responses

- A form submit, a transition link or a row action is a request to the transition URL with `Accept: application/json`.
- `run_transition` (`render.rs:1106`) and `wants_json` (`render.rs:1432`) already answer with `send_json_redirect` (`render.rs:1640`): `screenUrl`, `screenPathList`, `messageInfos`, `validationErrors`, `currentParameters`.
- The client shows the messages, marks the fields, and then navigates to `screenUrl` in `tui`, or reloads or hides the container named by `background-reload-id`/`background-hide-id`.
- Transitions that answer through `ec.web.sendJsonResponse` (`web/mod.rs:611`) give `messages`/`errors`, used as-is. A `type="none"` response gives nothing, as in Java.
- Downloads (exports, `transition` responses with a file) come back as the body with its content type. The client saves them to disk in local and remote mode, and refuses them over SSH.
- **CSRF.** The HTTP link authenticates with the `login_key` header, which skips the session-token check (`render.rs:1937` `csrf_rejected`, `crates/moqui-core/src/screen/mod.rs:485` `authenticate`). The in-process link renders without `enforce_request_checks`, as the SSH shell does today.

### 2.6 Shell data

The client uses the endpoints the qvt shell uses, unchanged:

- `menuData` (`webroot.xml:39`) for the drawer, the tab bars' sibling lists and `renderModes`;
- `shellData` and `shellSearch` (`webroot.xml:50,59`) for the apps, providers and the palette;
- `shellCounts` for the badges (`crates/moqui-core/src/service/shell.rs:97`).

The one addition is a `tui` value in the theme of the shell data: breakpoints, drawer width, color roles and the key binding overrides (D11). So the settings of sections 1.3 to 1.5 live with the theme. They are `ScreenThemeResource` values of new resource types `STRT_TUI_BREAKPOINTS`, `STRT_TUI_COLORS` and `STRT_TUI_KEYS`. These are Enumeration rows beside `STRT_TRAY_TODAY` in `runtime/base-component/framework/entity/ScreenEntities.xml:157`.

They are read in `shell_data` (`crates/moqui-core/src/screen/shell.rs:712`) from the user's qapps theme, which is how the shell's other values are read: `sri::user_theme_id` with `QUASAR_THEME_TYPE`, falling back to `DEFAULT_QUASAR`. They are not read through `get_theme_values` (`sri/theme.rs:158`). That function takes the theme type from the request's render defs, and `/shellData` is a `webroot.xml` transition with none, so it would fall back to `STT_INTERNAL` and never find them.

`STRT_TUI_KEYS` holds one `action = chords` entry per value. `Shell::theme_value` (`shell.rs:404`) keeps only the first value, which is right for the one-value types but would drop every key override but one. So the keys are read with `sri::theme_resource_values` (`sri/theme.rs:135`), which returns all of them in sequence.

### 2.7 Security

- **Server authz, no client rights.** Each request is rendered as the logged-in user with artifact authorization on, in a fresh context on the request pool. This is the pattern of the SSH shell (`crates/moqui-host/src/ssh/shell.rs:80` `run`, `crates/moqui-host/src/ssh/mod.rs:35` `on_pool`, then `internal_login_user` at `crates/moqui-core/src/user/login.rs:231`). The client holds no permission logic. What the user may not do is not in the model.
- **No secrets in the model.** Password widgets carry no value, and credential parameters are stripped (2.3).
- **Terminal safety (D9).** The model writer (JSON escaping) and the client (before drawing) both drop C0/C1 controls except tab and newline, and the client also drops bidi override characters. OSC 8 links are written only for `http`/`https`/`mailto` URLs.
- **Remote.** The remote client refuses `http://` except to loopback. It stores a login key, never a password (section 4.3).

### 2.8 Server tests

- `crates/moqui-core/src/screen/tests/golden.rs`: a `tui` entry for each fixture in `FIXTURES`, with expected units in `crates/moqui-core/src/script/testdata/screen/tui/`. The existing checks (repeatable, parses, no globals) apply.
- A new `crates/moqui-core/src/screen/tests/tui.rs`: renders fixtures to JSON and compares with `testdata/screen/tui/render/*.json`, with `UPDATE_GOLDEN=1` like the others. Every output is also deserialized into the `moqui-tui` model types (dev-dependency with `default-features = false` and only the `model` feature, 3.1). The fixtures are the Render fixtures of `tests/widgets.rs` plus cases for an empty section, a failing section, iterate, a dialog, depends-on and a multi list.
- `runtime/base-component/example/test/Gallery.spec.js`: render every Gallery screen in `tui` as well, so every widget in the gallery (`Gallery/Widgets.xml` above all) produces a valid model. Run with `./moq test example`.
- Fragment fixtures: a valid `tui` text, one that is not JSON (an `error` node, the rest of the page intact), an `any` text alone, and a `qvt` text alone (the placeholder).
- A render test that a password value and a credential parameter never appear in the JSON, and one that a value holding `\u001b]52;…` comes out without the control bytes.

### 2.9 Custom screens: a `tui` implementation per mode

D10's rule: when a screen goes custom in one render mode, it is implemented again for each other mode it should work in. For the TUI that means a `render-mode` `text type="tui"` wherever the screen has a `qvt` text or is a `.qvue` screen. The screen's actions, transitions and data stay shared. Only the presentation is written twice.

**Two forms.**

| Form | For | How it looks in the screen |
| --- | --- | --- |
| Inline fragment | A small custom part inside an XML screen: a chart, a diagram, a hand-made card row, a banner | A `text type="tui"` in the same `render-mode` element as the `qvt` text. Its content is the JSON of one node or an array of nodes of 2.3. |
| Component screen | The counterpart of a `.qvue` screen: a board, a calendar, a chat, a mail reader | The screen's `render-modes` adds `tui` (`vue,qvue,tui`), and a `text type="tui" location="component://…/Name.tui.json"` sits beside the `qvue` text. The file is usually one `component` node with its props. |

**Which text is used.** The XSD's rule holds (`runtime/base-component/framework/xsd/xml-screen-3.xsd`, the `type` attribute of `text`, default `any`). A `tui` text is used in the `tui` mode. An `any` text is used when no text of the element matches. An `any` text is written for HTML in practice, so its output is converted to an `html` node by the helper of 2.3. A `qvt`, `qvue` or `html` text alone gets the placeholder.

**Template expansion.** The text is expanded like any other `render-mode` text (`render_text`, `crates/moqui-core/src/screen/sri/sections.rs:430`). A static file uses `template="false"`, and its component fetches its own data, as a `.qvue` does. An inline fragment usually expands values from the context.

Nuance: a bare expansion in JSON is an injection, just as a bare `${}` in qvt text is an HTML injection. Every value goes through `JSON.stringify`, which the template's JavaScript expressions already have:

```
{"t":"component","name":"chart","props":{"type":"bar","labels":${JSON.stringify(monthLabels)},"series":${JSON.stringify(totals)}}}
```

**How the macro checks the text.**

- `ScreenTui.js` writes the expanded text as a `fragment` node, so the comma rule of 2.2 holds.
- It parses the text first. A text that is not valid JSON, or not one node or a node array, becomes an `error` node naming the screen and the text's location. This follows the failure-tolerance rule: one bad fragment does not lose the page.
- The control-character strip of 2.7 applies to the parsed strings.

**Components.**

- **The registry.** A `component` node names a client widget registered in `moqui-tui`. The registry uses `inventory`, as native components do: `inventory::collect!` in `moqui-tui`, `inventory::submit!` in a component crate (`crates/moqui-core/src/native.rs:42,23`). A registration gives a name, a constructor from `props` plus a `ScreenLink` and the screen's base path, and the widget's key help for F1.
- **The framework set** lives in `moqui-tui/src/ui/components/`. It starts with what the Gallery and the qvt texts in the framework need: `board`, `calendar` (agenda list, and a month grid from lg), `chart`, `diagram` and `timeline`.
- **Add-on components.** An add-on crate (BOSS, for its chat, inbox and chatter) registers its own behind a `tui` feature of its crate. The generated app crate in `moq` forwards `tui` to every component crate that declares the feature, as it forwards the package features.
- **Data.** A component gets its data from the screen's own transitions through the link's JSON call, the same `sendJsonResponse` transitions its `.qvue` twin calls. That is what keeps the two variants to presentation only.
- **Two failure cases, both logged and drawn rather than fatal.**
  - An unknown component name draws a placeholder naming it.
  - A remote `tui-client` binary knows only the components built into it, so a server whose add-ons it was not built with shows those placeholders. The client's `=` info lists the components it knows.

**Finding the work.** `lint_screens` (`crates/moqui-core/src/script/screen_lint.rs:461`) gets two findings:

1. A screen whose `render-modes` include `tui` (explicitly or as `all`), with a `qvt`/`qvue` text that has no `tui` text in the same `render-mode` element. Also a `.qvue` screen without `tui` in its modes. This is reported as information, not an error, since a browser-only screen is allowed.
2. A `tui` text with a `${` that is not inside `JSON.stringify(`.

**The worklist today.**

- `runtime/component/ironstone-boss/screen`: 25 screens have inline `qvt` text and 18 screens use `qvue`.
- `runtime/base-component` and the `mr-*`/`Mr*` components: 14 more screens have inline `qvt` text.

`moqui lint` lists them by path. The framework's own screens are done with the framework components in Phase 3. The Gallery's `Board` and `Calendar` are the reference pair: `Board.tui.json` and `Calendar.tui.json` beside their `.qvue` files. BOSS's screens are BOSS's work, tracked in its plan.

---

## 3. Client: `crates/moqui-tui`

### 3.1 Crate and dependencies

| Module | Content |
| --- | --- |
| `model` | Serde types of 2.3, version check. Behind feature `model` (default on, and the only feature `moqui-core` enables). |
| `link` | `ScreenLink` trait: render a level, call a transition, fetch options or JSON, fetch the shell data. Plus a `FakeLink` over fixture files for tests. |
| `nav` | The level stack, history, the first-changed-level reload rule, URL ↔ path and parameters |
| `state` | Per-form edit buffers, dirty tracking, client validation, selection, scroll and focus by node id |
| `layout` | Width classes, the grid, the field-row and label rules of 1.3 |
| `ui` | Revue adapter: frame, regions, overlays, the widget adapters, own widgets in `ui/widgets/`, the component registry and the framework components in `ui/components/` (section 2.9) |
| `keys` | The keymap of 1.5 and the layered dispatch |
| `theme` | Roles, server theme mapping, color depth, NO_COLOR, ASCII |
| `run` | Two loops (section 3.5): `run_local` on `App::run`, and `run_session` on a writer and an event source |
| `logpane` | The log pane widget fed by a channel |

**Dependencies.**

- `revue = { version = "=3.10.1", default-features = false, features = ["markdown"], optional = true }`. The version is exact, as the proposal says to pin it, and is upgraded deliberately.
- **Features.** `model` has no dependencies beyond serde. `ui` (default on, and it implies `model`) enables `dep:revue` and gates the `ui`, `keys`, `theme`, `run` and `logpane` modules. `link`, `nav`, `state` and `layout` build with `model` alone. `moqui-core` takes `moqui-tui` with `default-features = false, features = ["model"]`, so its test build never compiles Revue or crossterm (D3).
- `serde`, `serde_json`, `jiff` and `unicode-width` are already in the workspace.
- `regex` (already in the lock) checks the client rules.
- No Tokio, no reqwest (D4).

**New crates in the lock.** crossterm 0.29, unic-emoji-char and textwrap are new. Each needs a line in `plans/archive/License-Review.md` and must pass `deny.toml`: Revue is MIT, crossterm MIT, textwrap MIT, unic-* MIT/Apache.

**Workspace.** `Cargo.toml` members get `crates/moqui-tui`, and `[workspace.dependencies]` gets `moqui-tui` and `revue`. The workspace comment's toolchain note becomes "Revue 3.x requires Rust 1.95". The local toolchain is 1.99, and the repository pins none.

### 3.2 Interpreting the model

- **Rebuilt every frame from our own data.** Revue calls `View::render(&self)` every frame and rebuilds the tree (`src/widget/traits/view.rs:121`). The interpreter keeps the model, plus the per-id state of 3.1, in one `TuiApp` struct, and its `render` builds the view tree from them each frame. Stateful Revue widgets (`Input`, `TextArea`, `DataGrid`, `Tree`) live in that struct, keyed by node id. They are drawn with `ctx.render_child(&widget, rect)` (`src/widget/traits/render_context/mod.rs:225`), because `Stack::child` wants `'static` children.
- **A new model for a level replaces that level's nodes only.** Widget state whose id survives is kept. When a transition reloads the screen, the submitted form's buffers are dropped and other forms' unsent edits are kept, with their dirty marks.
- **Focus order** is the model's reading order, computed once per model change. The focus list is ours. Revue's DOM `:focus` is set to match only so its CSS hover and focus styles apply. Tab navigation in the builder stays off (`src/core/app/builder.rs:177`).
- **Visible-when and depends-on** are evaluated in `state` on each change, and an options fetch is queued when needed.

### 3.3 Key dispatch

Each key event goes through the layers in order: modal, then confirm, then open popup (a drop-down list, the palette), then the focused widget's adapter, then the screen keymap, then the global keymap. The first layer that handles it stops it. An adapter's `handle_key` calls the Revue widget's own handler where the widget has one (`Interactive`, `src/widget/traits/interactive.rs:60`; `Modal` returns the button index at `src/widget/feedback/modal/key_handling.rs:30`). Otherwise it implements the keys itself, as `examples/form_validation.rs:135` does.

**Paste.** Revue's `Terminal::init` does not enable bracketed paste (`EnableBracketedPaste` is only in the unused `CrosstermBackend`, `src/runtime/render/backend/crossterm.rs:97`), so `Event::Paste` likely never fires. The local loop enables it itself after init and turns it off on exit. The session loop's input decoder reports pasted text as one event, so a paste with newlines does not submit a form.

### 3.4 Layout

`layout` turns the model plus the width class into a tree of boxes with fixed cell sizes, with no Revue import. `ui` builds the Revue `Stack`/`Grid`/`Border` nodes from that tree. Revue's CSS applies width/height/margin/gap but not padding, flex or grid properties (`docs/guides/styling.md:182-200`), so all layout is computed in code. CSS is used for colors and borders only, through `AppBuilder::css` (`builder.rs:226`), with the role styles generated from `theme`. A resize recomputes the class and the layout. The model is not re-requested, except when a list's page size or `_uiType` changes.

### 3.5 Event loops

**Local (`run_local`).** `App::builder()` with mouse capture on, then `App::run(view, handler)` (`src/core/app/event_loop.rs:62`). The handler is the dispatch of 3.3. On `Event::Tick` it drains finished link requests and returns true when one changed the state. That is the pattern of `examples/worker_basic.rs:282`. Revue polls every 16 ms (`src/runtime/event/reader.rs:42`), which is fine for one local user.

**Session (`run_session`, for SSH and the browser terminal).** This loop takes:

- a `Write` for output, wrapped in `Terminal::with_size` (`src/runtime/render/terminal/core.rs:34`);
- a byte source for input;
- the initial size;
- a resize signal.

It must not touch the process terminal. It skips `Terminal::init`'s raw mode and terminal size query, the signal handlers and the panic hook. It writes the alternate screen, cursor and mouse-mode sequences itself. Input bytes go through a small ANSI decoder into Revue `Event`s: keys with modifiers, SGR mouse, bracketed paste, focus. The decoder is either an own parser over the `vte` crate, or crossterm's parser if the spike finds a public entry. The loop blocks on "input bytes, resize, request done or the next deadline" rather than polling. The deadline is the earliest of the timed things the local loop gets from its 16 ms tick:

- the success message's 3 s expiry (1.7);
- the 60 s counter read (1.6);
- the title-line clock's next minute;
- the spinner's next frame while a request runs.

So an idle session wakes about once a minute, and SSH and browser sessions behave as local mode does.

**Drawing.** The loop needs the same draw path `App::run` uses: DOM, CSS, layout, diff and dirty-rect output. Today that path is reachable from outside the crate only through `PipelineHarness` (`src/testing/pipeline.rs:42`, `send`/`send_with` at :252), which draws into a `Vec<u8>`. The spike (Phase 1) was to settle which route the loop uses; Phase 3b took the fourth:

1. An upstream change to Revue: a public `App::run_on(terminal, events)`, or making the harness's draw step public. **Preferred.**
2. Driving `PipelineHarness`, then copying its byte buffer to the writer after each frame. Workable, but it leans on a testing API.
3. The fallback of D6: spawn `moqui tui --attach <socket>` on a server-side pseudo-terminal (`portable-pty` or `rustix` openpty), with the parent bridging bytes and window size. The child reaches the parent through the HTTP link over a Unix socket with a one-time login key, since an in-process link is impossible across processes. This costs a process per session and a fork of a large binary, which is why it is the fallback.
4. **Chosen (Phase 3b): our own frame loop on Revue's public API.** The spike assumed the client would draw through Revue's DOM and CSS, which only `App` drives. Phase 2 moved all drawing to the client's own painter (`crates/moqui-tui/src/ui/paint.rs`), which sets cells through `RenderContext`. So the loop needs only what is public in 3.10.1:
   - `Buffer::new`, `RenderContext::new` and `View::render` (`src/widget/traits/render_context/mod.rs:165`);
   - `render::diff` (`src/runtime/render/diff.rs:41`), which compares hyperlinks by URL;
   - every `Cell` field and the buffer's hyperlink registry.

   `crates/moqui-tui/src/ui/encode.rs` writes the changes. `Terminal::render` is not used, for two reasons: its `current` buffer has no hyperlink registry, so every linked cell would be rewritten each frame, and it always writes true color. Nothing upstream is needed.

### 3.6 Background requests

Every `ScreenLink` call runs off the UI thread. The local loop uses Revue's `WorkerHandle::spawn_blocking` (`src/state/worker/handle.rs:41`), which needs no `async` feature. The session loop uses the host's request pool through the in-process link. Results come back on a channel, and the loop marks the state dirty and redraws.

- **Superseding requests.** A request carries a sequence number per purpose (level, options for field X, suggestions), and a late answer to a superseded request is dropped. So fast typing in an autocomplete, or paging twice, shows only the last answer.
- **One in flight per purpose.** Dropping late answers is not enough on its own, since every request sent still holds a worker and a queue slot. The host's request pool is shared with HTTP: 8 workers and a queue of 64 by default (`crates/moqui-core/src/service/request_pool.rs:30-31`), and it refuses work with "request queue is full" once the queue is full. A held PgDn or F5 at key-repeat speed would fill it and refuse browser users. So each purpose has at most one request in flight and holds only the newest pending one, sent when the running one returns. The requests in between are never sent.
- **Request errors.** A network or 5xx error is a message-line error and leaves the screen as it was. A 401 opens the login dialog (1.7).

### 3.7 Own widgets

These are written because Revue lacks them or gets them wrong, as found in the survey.

| Widget | Why | Built on |
| --- | --- | --- |
| `DropDown` | `Select` has labels only; `Combobox` cannot change options after build | `OptionList`, `Input`, `VirtualList` |
| `FieldInput` | max length, mask, number alignment, changed and error marks, bracket look | `Input` |
| `DateField` | today and now are fixed in Revue; it has no string parsing; it uses its own `Date` type | `DateTimePicker`, `jiff` |
| `ListTable` | server paging, our sort/filter, row forms, selection column, stacked columns, sub-rows | `DataGrid` (single-line rows), own row renderer for stacked columns |
| `LazyTree` | expand fetches children | `Tree` |
| `StatusFlow` | the status-bar stages | `Text` |
| `FieldLabel` | dot leaders, `*`, truncation | `Text` |
| `LogPane` | levels, filter, follow | `LogViewer` (`src/widget/data/log_viewer/`) |

Each own widget that fixes a Revue bug gets an upstream issue, and the fix is dropped from the wrapper when a pinned upgrade carries it (section 7).

### 3.8 Markdown editing: now and later

**Now (D12).**

- `editor-type="md"` text areas edit the Markdown source in a `TextArea`, with F4 toggling a preview drawn by the `markdown-view` renderer.
- `html`, `rich` and `rich-light` fields are read-only, converted to text, with "Edit in browser".
- Rich text never round-trips through the terminal, because each conversion between HTML and Markdown loses something: attributes, spans, table layouts, mentions and record links.

**What exists to build on** (surveyed Oct 20, 2026).

- **Revue's `RichTextEditor`** (`src/widget/form/rich_text_editor/`) is the nearest fit.
  - It has block types (headings, quote, code block, bullet and numbered lists, rule), inline formats (bold, italic, underline, strike, code), a toolbar, Markdown shortcuts, a preview mode and undo.
  - It converts with `from_markdown` / `to_markdown` (`core.rs:26,53`).
  - Its import is one block per line (`markdown.rs`). Inline markup on import stays literal text, so `**bold**` is kept but not drawn bold, and the round trip is close to lossless in text but styled at block level only.
  - Tables, nested lists and reference links would need a fidelity check.
- **Elsewhere.** The Ratatui ecosystem has Markdown renderers ([ratatui-markdown](https://docs.rs/crate/ratatui-markdown/latest), [ratkit](https://docs.rs/crate/ratkit/latest)) and editors with a split preview or inline syntax highlighting ([mdt / mdtui](https://docs.rs/mdtui/0.3.0/mdt), [smarkdown](https://docs.rs/crate/smarkdown/0.2.0), [midetor](https://rust-digger.code-maven.com/crates/midetor)). None edits in place WYSIWYG, and none is a Revue widget.

**Later: a styled-source editor.** The likely good-enough two-way editor is not WYSIWYG but **styled source**:

- The Markdown text is the value and is edited directly, so nothing is ever converted and nothing can be lost.
- Each line is drawn with the same styles as `markdown-view`: headings bold and in the heading color; `**bold**`, `_italic_` and `` `code` `` styled, with their markers dimmed; links underlined; list bullets and quote bars in the muted color; fenced code in the code color.
- The styling comes from `pulldown-cmark`'s source offsets over the visible lines. pulldown-cmark is already a dependency through Revue's `markdown` feature, so no tree-sitter grammar is needed.
- Toolbar keys wrap the selection (Alt+B, Alt+I, Alt+K for a link), and F4 still toggles the full preview.

It would be an own widget over `TextArea`, or a contribution to Revue's `RichTextEditor` as a "source" view mode. Before choosing, Phase 4 checks `RichTextEditor`'s round trip on the Gallery's Markdown samples and the wiki pages. If its output equals its input for all of them, it may serve as is. If the HTML fields later get a lossless Markdown form on the server, they can use the same editor.

---

## 4. Channels

### 4.1 `moqui tui` (local)

- **CLI.**
  - A match arm in `dispatch` (`crates/moqui-host/src/lib.rs:281`).
  - The name in `usage_error` (`lib.rs:380`) and in `is_known_command` (`crates/moqui-host/src/usage.rs:307`). Without the last, `moqui tui --server=https://…` is taken for server options, because it holds `=` (`looks_like_server_option`, `usage.rs:326`).
  - Help text in `usage.rs` beside the other commands.
- **Start.** A new `start_local` in `lib.rs` boots the runtime with no listeners. It returns a handle whose `factory()` the in-process link uses, and dropping it shuts down on exit.
  - `start_server` (`lib.rs:196`) cannot do this. It always binds the plain HTTP listener and the cluster listener (`serve_thread`, `lib.rs:885`), and `boot_server_factory` (`lib.rs:985`) itself starts the SSH server, the cluster edge listener (8444 by default) and the SMTP listener. On a machine where a server already runs, the TUI would fail on a port before it reached the DB lock check below.
  - So `boot_server_factory` takes a choice of listeners. With none, it skips those three, and `start_local` binds no HTTP port.
  - On drop, the handle does what `serve_thread` does after its listener stops (`lib.rs:942-960`): flush the artifact hits and the sessions, and stop log shipping. Then it drops the process.
  - `--serve` uses `start_server_in` with the configured address instead.
- **Logging.** Before the boot, logging is installed with stderr off, since any stderr line would tear the screen.
  - `install_with_console(None)` (`crates/moqui-host/src/logging.rs:84`) does the opposite. `None` means stderr gets every line at `moqui_log_level` (`logging.rs:105`), and no `Level` turns stderr off, since `event` writes whenever `level <= console` (`logging.rs:546`).
  - So `logging.rs` gets `install_without_console()`. Its `LineSubscriber` holds `console: Option<Level>`, and `event` skips stderr when it is `None`.
  - It has to be the first install in the process, because the `OnceLock` makes any later install a no-op.
  - The log files are written as usual, and the log pane below shows the lines.
- **Log pane.** `LineSubscriber::event` (`logging.rs:506,528`) gets a third sink: a static list of bounded channels that `logging::tap()` registers. `moqui-tui` gets the receiver. The subscriber is hand-written and has no layers, and `tracing-subscriber` is not in the lock, so this is a tap rather than a layer, as the proposal assumed.
- **Login.** The TUI's login dialog, through `login_user`. On an instance whose purpose is dev, `--user <name>` logs in without a password, as the test fixtures do.
- **Flags.** `--path <screen path>` for the first screen, `--ascii`, `--no-color`, and the HTTP/SSH listeners off by default (`--serve` turns them on, so a developer can have the browser beside the TUI).
- **Already running.** If the runtime directory is already served by a running instance (the DB lock), the command says so and suggests `--server`.

### 4.2 SSH `apps`

- **Command.** `apps` is a command in `crates/moqui-host/src/ssh/shell.rs`, in the `match` and the `HELP` text, and the whole session of a user without `SSH_SHELL`. Every screen is checked as that user. (The first design gated it on a new `SSH_APPS` permission at every place that gated on the shell; that was built in Phase 3b and removed Oct 21, below.)
- **Pty.** `pty_request` (`server.rs:489`) keeps only that a pty was asked for and drops the size and `TERM`. It must store `term`, columns and rows on the `Connection`. A `window_change_request` handler is added (there is none today) and forwards the size to the session loop. Without a pty, `apps` refuses with a one-line message.
- **Stream.** The session takes over the channel stream that `interactive` (`server.rs:691`) reads line by line today. That loop drops escape bytes, so `apps` hands the stream to `run_session` and gets it back on exit, returning to the `moqui>` prompt when the user came from the shell.
- **Sessions.** One `run_session` per channel, on its own thread (the loop is synchronous). Requests go through `on_pool` with `internal_login_user` per request, as the shell's commands do.
- **Session limit.** `ssh_max_sessions` (`server.rs:142`) does not bound the threads. It counts logged-in connections (`log_in`, `server.rs:219`), and `channel_open_session` (`server.rs:473`) accepts any number of channels on one connection. So one login could open hundreds of `apps` channels, each a thread, or a process under route 3.
  - A new `tui_max_sessions` property bounds the live `run_session`s, in total and per user, with small defaults.
  - SSH channels and `/termws` sockets (4.4) take slots from the same count, and a slot is freed when its session ends.
  - Over the limit, `apps` refuses with a one-line message, and the socket closes with a reason.
- **Downloads and uploads** are off over SSH (1.6, `file` and exports). The log pane is off, unless the user has the permission to read logs, in which case it shows only lines at WARN and above. The proposal leaves this to permissions, and it is Phase 4.
- **As built (Phase 3b).** `crates/moqui-host/src/ssh/apps.rs`, behind `ssh` and `tui`.
  - `apps` runs as the `exec` command, as the whole shell session of a user without `SSH_SHELL`, and as a command at the `moqui>` prompt.
  - `env_request` also keeps `COLORTERM`, `NO_COLOR`, `COLORFGBG`, `LANG`, `LC_ALL` and `LC_CTYPE` for the theme. A locale that is set and is not UTF-8 draws in ASCII.
  - The window size is a `watch` per pty channel, so a window change while the user is still at the prompt is not lost.
  - `tui_max_sessions` (16) and `tui_max_sessions_per_user` (4).
  - **Idle.** russh resets its inactivity timer after any loop turn that is not a keepalive, sent data included (`server/session.rs:834-841` in russh 0.64.1). So the client's minute clock would keep an idle session open forever. An `apps` session therefore ends itself after `IDLE` (600 s) without input, with a one-line note.
- **Changed Oct 21, 2026 (user): SSH for every user.** `SSH_APPS` is gone, as SSH became like the HTTP side.
  - Any user may connect, and gets the apps; `SSH_SHELL` adds the `moqui>` prompt first, only for a login by key or
    password at the SSH level. `accept` refuses no one for lack of a permission, and the git-only password rule moved
    to the git exec: git on a password connection needs `ssh_git_password_auth`.
  - A client with no key is let in with no user by keyboard-interactive with no questions ("none" is not offered, so
    OpenSSH tries its keys first). Its `apps` starts with the link's login dialog, the SSH name filled in and the
    password next (`SessionOptions::login_name`). Such a connection gets `apps` on a pty and nothing else; it takes an
    `ssh_max_sessions` slot, and is disconnected when its dialog has not logged in within `LOGIN_WAIT` (120 s). A dialog
    login is the apps alone, with no prompt after. `ssh_login_dialog=false` turns it off.
  - The session code moved from `ssh/apps.rs` to `crates/moqui-host/src/term.rs` (feature `tui`), for SSH and `/termws`
    alike. `Sessions` is now one per factory (an extension), so SSH and browser sessions share `tui_max_sessions`. A
    session with no user counts toward the total only; the dialog login moves it to the user (`SessionSlot::assign`),
    refused when the user's sessions are full. A host-side `ScreenLink` wrapper (`GatedLink`) does that, decides the log
    pane after the login (`SessionEvent::LogOffered`), and closes the session after `MAX_LOGIN_FAILURES` (5).
- **`agent-apps` (Phase 3c).** The same client for a program, on an `exec` channel with no pty (`crates/moqui-host/src/ssh/agent.rs`). Sessions are held by the server between one-command execs, and there is a line mode. It needs a user the SSH connection logged in (a key or a password; not a dialog login) and takes the same slots. docs/TUI.md "For agents".

### 4.3 Remote client (`moqui tui --server <url>`)

- **Link.** The HTTP link lives in `moqui-host`, which already depends on blocking `reqwest` (workspace `Cargo.toml:78`). It sends:
  - `GET <url><path>?renderMode=tui` for a level (`pick_render_mode`, `render.rs:863`, reads the `renderMode` parameter);
  - transitions with `Accept: application/json`;
  - the `login_key` header (`crates/moqui-core/src/user/login.rs:348` `apply_header_keys`).
- **No runtime.** The remote client boots no factory. It runs `run_local` with the HTTP link only. A client-only build (`--no-default-features --features tui-client`) needs no database or server pieces; the gating of `moqui-host`'s modules allows it (`lib.rs:23,28-66`).
- **Login.** The client prompts for username and password (and OTP through `sendOtp`/`verifyOtp`, `crates/moqui-core/src/screen/dispatch.rs:54-55`). It posts them once to the `login` REST transition, then asks for a login key (`get_login_key`, `user/login.rs:305`, with the `login-key` expiry of `login_key_expire_hours`, `user/policy.rs:67`). The key is stored in `~/.config/moqui/tui/<host>.toml`, mode 0600, with its expiry, and the password is forgotten. `--forget` removes it. A missing server-side transition to mint a key is the one server addition this needs, if `login` does not already return one.
- **Downloads** go to `~/Downloads` (or `XDG_DOWNLOAD_DIR`). The file picker browses the user's machine, and uploads are multipart posts.

### 4.4 Browser terminal

Built Oct 21, 2026, once SSH was open to every user (4.2): for every signed in user, from the account menu's Terminal UI.
As built, below the first sketch.

- **Server.** A `/termws` WebSocket route beside `/notws` (`crates/moqui-host/src/http.rs:211`), exempt from the request timeout as `/notws` is (`http.rs:429`). It is authenticated by the session cookie as `notws::upgrade` is (`crates/moqui-host/src/notws.rs:85`). Its bytes feed `run_session` exactly as SSH does. Because it is exempt from the request deadline, each socket takes a slot of `tui_max_sessions` (4.2) before its session starts.
- **Page.** A small page with xterm.js, vendored through `libs/refresh.mjs` with a manifest line and a license review entry (MIT).
- **Placement.** Phase 4, only on demand, as the proposal says.
- **As built.**
  - `crates/moqui-host/src/termws.rs`, behind `axum` and `tui` (so `lite` has it). No permission: the user of the
    session cookie, apps only, every screen checked as that user. The origin check and cookie lookup are `notws.rs`'s.
  - Binary messages are the terminal's bytes both ways; a text message `{"type":"resize","cols":N,"rows":N}` is the
    size, and the first one starts the session. A `tokio::io::duplex` pipe feeds `term::serve`. The server closes with a
    reason (the limits, idle, logged out, stopping), which the page shows with Reconnect. The HTTP session is read
    again each minute, so a logout closes the socket.
  - The page is `webroot/apps/Terminal.xml` with `Terminal.qvue` (`/qapps/Terminal`), viewable by every user through
    the `SCREEN_TREE` artifact group (`WebrootSecurityData.xml`), as AppList is, xterm.js 6.0.0 and the fit
    addon 0.11.0 vendored in `libs/xterm`. xterm.js answers the client's OSC 11, and OSC 8 links open in a new tab.
  - The account menu item is static in the shell (`WebrootVue.qvt.js`), shown when `qapps.xml` gives `terminalUrl`,
    which it does while the host sets `webapp_terminal_endpoint` (`webapp_terminal`, default true).

### 4.5 Headless

The same interpreter draws through `PipelineHarness` at a given size with a `FakeLink` or the in-process link. This gives the client snapshot tests (section 6) and a `moqui tui --snapshot <path> --size 180x50` command that prints the screen text. That command is useful for CI and for checking a design without a terminal.

### 4.6 Packaging

- **The `tui` feature.** A new `tui` feature in `crates/moqui-host/Cargo.toml` enables `dep:moqui-tui`. `enterprise` and `maximal` include it. **Changed 2026-10-20 (user):** `lite` includes it too, for a quick terminal dev and test loop. `lite` has no `ssh`, so it has `moqui tui` but not `apps`.
- **The `tui-client` feature.** Not made: `tui` alone is the remote client's build (Phase 4 findings).
- **Forwarding.** `crates/moqui/Cargo.toml` forwards both features.
- **The generated app crate.** The template in `moq` (the `[features]` block near `moq:1067-1074`, `package_features` at `moq:941`) gets them too, so `./moq build` of an add-on set carries the feature.
- **Checks and docs.** `scripts/check-packages.sh` checks the new sets, and `docs/Packaging.md:18-42` gets the rows.

---

## 5. Phases

These are the proposal's four phases, each ending in something usable. No dates are set.

### Phase 1 — Spike (go or no-go on Revue)

1. **Crate and model.** Create `crates/moqui-tui` with the pinned Revue, and the `model` types for: box, text, link, form-single with text-line, drop-down and date-time, and form-list with paging.
2. **Hand-built screens.** Write two fixture models: the Gallery's List screen (find form plus list) and its Detail screen (two boxes plus a list). Write them by hand from the screens' qvt output. Draw them with `run_local` and a `FakeLink` at 72, 100 and 180 columns, and check the layouts of 1.2 and 1.3.
3. **The session loop.** Build a `run_session` prototype over russh in a throwaway host test, then settle the route (3.5, routes 1–3): open an issue or PR upstream for a public run-on-writer entry, and measure route 2 with a 180×50 screen. Check over SSH: resize, mouse, paste, F-keys in xterm, kitty, GNOME Terminal, tmux and the Linux console (Alt and F-key sequences differ).
4. **Unicode.** Confirm Revue's width handling with CJK and emoji in a list cell (`src/utils/unicode.rs:191`).

**Exit:** both fixture screens usable at all three widths by keyboard alone, the SSH route chosen, and a short findings note added to this plan. **No-go** if the session loop needs a fork of Revue we would carry. Then the Ratatui + rat-salsa fallback of the proposal starts from the same `model` and `link` modules.

#### Phase 1 findings

Recorded 2026-10-20. Decision: **go**. Route 3 is available without carrying a fork of Revue. The spike added no fork, no host feature, and no server mode.

**Route.** Route 2 is unfit as the session loop. On a debug build, the Gallery list at 180×50, three frames (initial draw, Tab, one typed character): 9529 bytes in about 3 ms. `PipelineHarness::terminal_output` returns `&[u8]` (`src/testing/pipeline.rs:191`), so the history cannot be drained, and the three-frame buffer was longer than the one-frame buffer. `send` (line 252) does not draw. Width and height are fixed in `new` (line 52); a resize is a new harness. Route 1 is the one Phase 3 should use, and only from a published Revue that contains it. The request is [hawk90/revue#1153](https://github.com/hawk90/revue/issues/1153): a public `App::run_on(Terminal<W>, event source)` that does not call `enable_raw_mode`, `install_panic_hook`, or `ShutdownSignals::listen`, and that can resize and drain. The `pub(crate)` draw path is `App::draw` (`src/core/app/draw.rs:9`) and `Terminal::draw_changes` (`src/runtime/render/terminal/render.rs:19`). `App::run` (`src/core/app/event_loop.rs:62`) calls `ShutdownSignals::listen` (`src/core/app/signals.rs:136`) and `Terminal::init_with_mouse` (`src/runtime/render/terminal/core.rs:52`), which enables raw mode (line 53) and installs the process panic hook (line 57). The pin stays `revue =3.10.1`. Route 3 (a `moqui tui` child on a pty) is the Phase 3 fallback if that release still has no `run_on`. This phase did not spawn a pty and did not add `portable-pty`. No pull request was opened.

**What the spike checked.** Section 1.3 width classes, on the canvas. The shell frame (drawer, title, message, key line) stays Phase 2. The fixtures are reduced Gallery screens: a find form, a paged list, two boxes, and one screen link each. Page size is fixed at 10 rows. Frames are ASCII `+ title`. Below 120 columns the two detail boxes stack. At 180 they share a row. `Select` and `DateTimePicker` are unused. A drop-down is an `OptionList`. A date is the date string in an `Input`. The line painter draws the brackets. Painting `Input` over those brackets left the closing bracket on the screen, so the spike uses `Input` for keys and leaves the brackets to the line painter. `cargo run -p moqui-tui --example gallery -- --size` printed one harness frame for the detail screen at 72, the list at 100, and the detail screen at 180. `App::run` was not run on a tty in this pass.

**Unicode.** Layout width matches `revue::utils::display_width`. `東京` is 4, `🎉` is 2, `café` (`cafe` + U+0301) is 4, and the ZWJ family `👨‍👩‍👧` is 6. `unicode-width` 0.2.2 reports 2 for that family and agrees on the other three. Layout keeps Revue's numbers. `draw_text` skips zero-width characters, so the painted family is `👨👩👧` with the joiners omitted, and the painted café is `cafe` with the combining acute omitted. The second cell of a wide glyph is a NUL spacer. Snapshot text drops those NULs.

**Decoder.** crossterm 0.29 has no public stream parser, so `crates/moqui-tui/src/decode.rs` is our own incremental parser and `vte` was not added. Unit tests cover xterm and GNOME Terminal CSI and SS3 function keys, kitty CSI-u, the Linux console `ESC[[A` sequence, tmux and xterm Alt (`ESC` plus a character, and `CSI 1;3A`), bracketed paste with a newline as one event, a CSI split across reads, and `CSI 8;rows;colst` (columns, then rows). There was no live five-terminal run. The Linux console sequence was checked as bytes; this machine did not run the Linux console.

**Workspace.** A full workspace check builds `moqui-tui` and needs Rust 1.95. `rust-version` stays unset. `cargo check -p moqui` finished without compiling Revue, and `cargo tree -p moqui` does not contain it. The `model` feature of `moqui-tui` is serde only. `revue-macros` locked at 3.10.2. `cargo deny check`: licenses, bans, and sources passed. Advisories failed on `ammonia` 4.2.3, `rustls` 0.23.45, and `zip` 8.6.0, which are already in the `moqui` graph and were left at the locked versions. The new crate names are in `plans/archive/License-Review.md`.

### Phase 2 — Core: browse and edit records in one real app

1. **Server.** `ScreenTui.js` for the elements of 1.6 minus views, trees, charts and wizard/stack. The registration of 2.1, the error kinds of 2.2 and the envelope of 2.3. `text type="tui"` fragments with their JSON check, and the placeholder where a `qvt` text has no `tui` twin (2.9). The two lint findings of 2.9, and the worklist they print. The goldens and render tests of 2.8, and the Gallery spec in `tui`.
2. **Client.** `nav`, `state`, `layout`, `keys` (with the theme's `STRT_TUI_KEYS` overrides) and `theme`. The frame with title, drawer, message and key lines. Forms with validation. Lists with paging, sort, the find dialog and selection. Drop-down with inline and URL options. Date fields. Dialogs. Transitions with messages and errors.
3. **Local channel.** `moqui tui` with the in-process link, login, and the log pane with its logging tap.
4. **Docs.** `docs/TUI.md` first version: start, keys, widths.

**Exit:** in the example app and one Mantle app, a user finds, opens, edits and saves records, uses dialogs and dependent drop-downs, and pages lists, all by keyboard, at 100 and 180 columns. Every Gallery screen renders a valid model.

#### Phase 2 findings

Recorded 2026-10-20, after a review of the first Phase 2 commit against this section and a second pass that closed the gaps it found.

**Exit evidence.** `crates/moqui-host/tests/tui_screens.rs` (run by `scripts/check-packages.sh`) drives the in-process link on embedded Postgres with Mantle and SimpleScreens. It logs in through the client's dialog after a 401 (a wrong password stays), and renders all 21 Gallery screens with no error node anywhere in their documents. On Gallery List it finds, pages (the footer drawn at 100 and 180 columns), sorts, opens the detail and submits a dialog. On SimpleScreens Party it uses the dependent country and state drop-downs, gets a client mark for an empty required name, creates a person, edits and saves, and reads the record back from the server. `Gallery.spec.js` renders every Gallery screen in `tui` too (`./moq test example`, 53 features).

**Server.** The review found three bugs, all fixed with render tests in `screen/tests/tui.rs`:

- URLs dropped their parameters: links use `getUrlWithParams`, and options send their fixed parameters and `{parameter: field}` for `depends-on`, as qvt's do.
- A `text` from a location was written straight into the JSON: it now comes back through `sri.renderTextValue` and is checked first.
- Several nested renders could leave `"c":}`, which sent the whole page to one error node: `json_value` writes `[]` for a render that wrote nothing.

Other server changes:

- Node ids are unique through `sri.uniqueId` (`id~2` for a repeat).
- Validation rules come as data (`ClientRule`, `sri.getFieldValidationRules`), since the client does not run JavaScript.
- HTML is converted by `screen/html_text.rs` on the html5ever tokenizer.
- The theme's `tui` values are in the shell data (`STRT_TUI_BREAKPOINTS`, `STRT_TUI_COLORS`, `STRT_TUI_KEYS`).
- The two lint findings are in `moqui lint --screens`. The worklist on this date is 87 screens: 44 in ironstone-boss, 24 in MrSimpleScreens, 11 in example, 7 in tools and 1 in webroot. No `tui` text has an unsafe `${`.
- `make_render` gained `history(true)`, so TUI visits go on the screen history.

**Client structure.** `crates/moqui-tui` was rewritten after the review:

- The session is sans-IO. Keys queue `LinkCall`s; the loop runs them on worker threads and hands the answers back. Per-purpose `Lanes` keep one request in flight and only the newest waiting (3.6).
- The modules are `forms` (an index of the levels, posting and client checks), `edit` (the line editor: cursor, `maxlength`, mask), `state`, `keys`, `nav` (parent levels by `.tui` path), `layout` (a measured piece tree), `theme` and `ui`.
- The session tests run without Revue.

**The draw layer, a deviation from 3.2 and 3.7 to note.** The view draws through Revue's pipeline: `RenderContext` cells in theme roles, Revue's `Border` for every frame, OSC 8 hyperlinks through the buffer's registry, `PipelineHarness` for snapshots, and `App::run` for the local loop. The own widgets are draw functions in `ui/draw.rs` and `ui/overlays.rs` (field input, labels, lists, the drop-down, the date picker) rather than wrappers over Revue's `Input`, `OptionList`, `DateTimePicker` and `DataGrid`. The survey's gaps decided that:

- `Input` takes no cursor from outside and no length or mask.
- The others hold state the session already owns.

Revue always writes RGB escapes, so the 16 and 256 color depths only choose nearer colors.

**Packaging.** `tui` is in `enterprise` and `maximal` as 4.6 said then (so `cargo check -p moqui` compiles Revue), not in `lite`; `lite` got it later (4.6). The generated app crate forwards it.

**Smaller decisions.**

- The drawer docks from 146 columns, which is the 120-column rule; the text said 150.
- Mouse capture is off: no mouse handling yet.
- The cursor is drawn as a cell. A view cannot move the terminal cursor.
- Calls run as the user through `internal_login_user` per request, the pattern of 2.7.
- The login dialog takes no second factor: such an account is told to use the browser.
- `moqui tui` refuses to start when the runtime's HTTP port answers, and does not suggest `--server`, which is Phase 4.
- The core render tests assert on the documents rather than keep `render/*.json` goldens.

**Left for Phase 3, or open:**

- Saved finds, Settings and Home in the drawer.
- F9's choice kept as the `TUI_DRAWER_OPEN` preference.
- The Markdown preview on F4.
- Row actions on the selected rows.
- A host test of an options transition that fails (the session shows the error, and the host logs it).

### Phase 3 — Shell and dynamics: a full admin task over SSH

1. **SSH.** Moved to Phase 3b, below, and built there.
2. **Dynamics.** Autocomplete, `depends-on` everywhere, `dynamic-dialog`/`dynamic-container` with reload and hide ids, `editable`, multi lists with inline editing, stacked columns, row actions, saved finds and the columns dialog.
3. **Shell.** Palette, quick create, counters, tray (as a modal), help with screen documents, message history.
4. **Structure.** Trees, wizard and stack panels, `container-split`, `status-bar`, `number-card` and `dashboard`.
5. **Components.** The component registry of 2.9 and the framework set (`board`, `calendar`, `chart`, `timeline`). The Gallery's `Board.tui.json` and `Calendar.tui.json` as the reference pair. `tui` texts for the framework's own inline-`qvt` screens on the lint worklist, and the `tui` feature forwarding to component crates in `moq`.

**Exit:** an administrator completes a Tools task in `moqui tui` (for example: find a user, change their groups, run a service job, read its result) without leaving the TUI. The Gallery's Board and Calendar work in the TUI through their components. The lint worklist has no framework screen left on it. The SSH part of this exit is Phase 3b's.

#### Phase 3 findings

Recorded 2026-10-20. Items 2 to 5 are built; item 1 is Phase 3b.

**The client.**
- `session.rs` is now `session/`: `mod.rs` (the state, keys, replies, navigation), `submit` (forms), `lists`, `overlays`, `dynamic` (fragments, server search, value urls), `shell` (palette, menus, quick create, tray, counters, documents, find text, exports, saved finds, columns), `structure` (trees, boxes, components, the wizard, the row editor, click to edit).
- The link has three more calls: `Json` (a transition's JSON, or text as a JSON string), `Fragment` (a dynamic body: the url rendered in `tui`, a transition's redirect followed, not put in the history), and `Raw` (a file). Each new purpose has its lane (3.6).
- Timed work is the session's: `Session::next_deadline` and `on_time` run the message expiry, the counters each minute, and the 300 ms wait of a server-search drop-down and of the palette. The local loop redraws on a tick only when one of those fired, a request is busy, or the clock's minute changed. This is the hook `run_session` will block on.
- Layout reads what only the session knows through a `Shown` trait: a box the user closed, a tree's open rows, a component's lines.
- A dynamic body's ids get its container's id and `/` before them, since it is its own render and its ids can be the screen's. Bodies are kept by container id, so a reload of the screen shows the old body until the new one comes. A body loads only the containers, trees and components inside it, not the screen's again (an early version asked for itself in a loop).
- A save whose form has a `background-reload-id` naming a dynamic container on screen reloads only that container. A successful save closes the dialog its form is in, as the browser's page reload does.
- Extra dialogs (quick create, tray, a document, an export as text) are the session's, indexed with the screen. Their ids have no `#`, which is the separator of a field's parts.
- List rows with fields are edited in a row editor (Enter or `e`) rather than inline: the proposal's fallback. It reuses the form layout, focus and submit; the table shows each cell's current value with its change and error marks. A multi list posts every row once, as qvt's multi `m-form`, not only the changed rows with `_rowSubmit_N`.
- A row action is a `dialog` node with `list`; its form posts the selected ids as `m-checkbox-set` does (`parameter=a,b` in list mode, else `parameter_N` and `_isMulti`) and refuses with no row selected.
- In a multi row, a field's `dependsOn` names the plain field; the client finds the row's own (`orgId_3`).
- Markdown (the node, and F4's preview of an `editor-type="md"` text area) is converted by `pulldown-cmark` to the blocks and links of an `html` node; both list their links as References to follow.
- Components (2.9) draw lines of theme-role spans and are in the model feature, so their tests need no Revue: `board`, `calendar`, `chart`, `timeline`, registered with `inventory`. There is no `diagram` component; the Mermaid evaluation stays Phase 4.
- A shifted character key matches its binding without Shift (`N`, `?`).
- Exports in local mode are saved by the loop to `XDG_DOWNLOAD_DIR` or `~/Downloads`; the text export shows in a dialog.

**The server.** `ScreenTui.js` emits all of section 2.3's Phase 3 shapes (README "The tui screen model"):
- an `ac-transition` text line is a server-search drop-down, as in qvt;
- default and value urls, `editable`, `multipart`;
- list row forms and multi forms, stacked cells, typed columns;
- row actions, saved finds, columns and exports;
- trees, wizard and stack subscreens, the status bar computed on the server, button menus;
- a component's `base`.

Other server changes:
- `screenDoc` answers an `html` node with `renderMode=tui`.
- The shell data's `tui.drawerOpen` is the `TUI_DRAWER_OPEN` preference.
- A list cell never shows a widget's element name.
- A `dynamic-dialog` in a list cell is a node, which F10 offers for the current row.

**Exit evidence.** `crates/moqui-host/tests/tui_screens.rs` adds to the Phase 2 flow, on the in-process link:
- Gallery Board and Calendar through their components, with a card moved through `moveCard`;
- the Widgets dynamic container's body;
- the palette's server search;
- the Tools task: find john.doe, open the account, add a group in its dialog, change the membership's thru date in the group list's row editor, run `clean_ServiceJobRun_daily`, and open its run.

The client has 33 more tests (97 in all): `dynamic.rs`, `shell.rs`, `structure.rs`, the components', and snapshots of the new pieces and overlays. The core has 10 more render tests.

**The worklist.** No framework screen is left on the lint's TUI worklist. What it lists now is in ironstone-boss (202 parts) and MrSimpleScreens (48), each a repository of its own.
- 16 of the framework's 19 screens got `tui` twins:
  - the Gallery's Board and Calendar as components;
  - Landing, Report, Record360 and Chatter with `chart`, `timeline` and number cards;
  - Editor's page as an `html` node and its compare as text;
  - QuickTask as a form, now `qvue,tui`, which F6 loads;
  - Panel, RecordFiles and Widgets;
  - Tools' AutoEdit, SqlRunner, ServiceLoadRunner and StatusFlows (the flow as text);
  - AppList.
- EntityDetail and ServiceDetail had only a highlighter script in their qvt text, so their twin is empty.
- ElFinder is browser-only (`render-modes="html,qvt"`): the whole screen is the elFinder file manager.

**Found on the way.**
- A dynamic dialog's or container's url lost its `<parameter>`s: the macro passed the node without its children. The Widgets container rendered the whole screen.
- `HostLink::level` now follows a redirect, as a browser follows one from an anchor to a transition (Tools' Run Job is a GET to `runJob`).
- The script runtime had no `ec.service.job(name)`. It is added (`parameters`, `parameter`, `localOnly`, `run` giving the run id, over `service::run_job_now`), so Tools' Run Job works in every mode.

**Open.**
- A `container-dialog` in a list cell (Gallery Dialog's Edit per row) draws nothing: its body is statement code, and a cell is a value. **Fixed Oct 21:** `ScreenTui.js` captures the dialog's writes (`$captured`) into the row's cell as a `dialog` node; F10 offers it. A section, include or subscreen inside such a dialog is an `error` node.
- `split.selectParameter` is carried but not used: the side's links already hold the parameter.
- `/apps/setPreference` with no user answers 500, not 401 (the screen names a Java exception class the JS runtime does not have). **Fixed Oct 21:** `apps.xml` answers `ec.web.sendJsonError(401, …)`, as `webroot.xml` does.
- The Calendar component shows and moves through months, but does not create an event as the `.qvue` does.
- The Service Load Runner needs `ec.service.getLoadRunner`, which the port does not have, in every mode; its twin says so and is ready for it.

### Phase 3b — SSH

1. **SSH.** `apps` over SSH with the chosen session route, `SSH_APPS` at every gate of 4.2 (removed Oct 21, 4.2), `tui_max_sessions`, pty size, window change, and the guard against downloads and uploads.

**Exit:** an administrator connects by `ssh user@host`, lands in the apps, and completes the Phase 3 Tools task.

#### Phase 3b findings

Recorded 2026-10-20.

**Route.** [hawk90/revue#1153](https://github.com/hawk90/revue/issues/1153) was still open with no maintainer reply, and 3.10.2 has no run-on-writer entry. It is not needed: route 4 of 3.5 was found and built. The pin stays `revue =3.10.1`. #1153 would still be nice, since `run_local` could then share the loop, but it blocks nothing. Comment upstream only with the user's leave.

**The client** (`crates/moqui-tui`, `ui` feature).
- `run_session(link, options, events, replies, out)` (`ui/session_loop.rs`). It takes `SessionEvent`s: input bytes, a window size, a request's answer, close. It writes frames to any `Write` and never touches the process terminal, its signals or its panic hook.
- It blocks on its channel until the earliest of the session's deadlines (`next_deadline`), the decoder's ESC wait, the clock's next minute, and the spinner while a request runs. An idle session wakes once a minute. Events already queued are applied before one draw.
- It enters and leaves the alternate screen with bracketed paste on and the cursor hidden. A panic is caught: the terminal is put back, and the error goes to the caller's log.
- The loop keeps a sender for its worker threads, so the channel never disconnects while it runs. The caller sends `Close` when its input ends. The first test drop of the sender hung on this.
- Requests run on threads, one per request, through the same `send` the local loop uses. Lanes keep one per purpose (3.6).
- `Screen` (`ui/encode.rs`):
  - writes the whole frame after a clear on the first draw and after a resize, and only the changed cells after that, with a cursor move only on a jump;
  - writes the SGR of each change of look at the terminal's depth: true color, the 256-color cube, the 16 ANSI colors, or none (`theme::sgr_color`);
  - writes OSC 8 by URL, writes a wide glyph once, and writes an unchanged frame as nothing.
- The decoder now gives Ctrl with a letter (Ctrl+K, Ctrl+Q), Ctrl+Space, Alt+Enter, Alt+Backspace, and the `1~`/`4~`/`7~`/`8~` and SS3 Home and End. A lone ESC becomes Escape after `ESC_WAIT` (50 ms) without a next byte: `Decoder::deadline` and `flush`. Before, it waited for the next key.
- Exports are refused in a session ("downloads are off in a remote session; use the browser"). The text export still shows in its dialog.
- `HostLink` holds the factory weakly (`HostLink::weak_as_user`). A call after the server stopped answers that the server is stopping.

**The host.** As 4.2, "As built".

**Exit evidence.**
- `crates/moqui-tui/tests/session_loop.rs` (6 tests).
  - The bytes a `Screen` writes, replayed on a small terminal, equal `PipelineHarness`'s screen text at 100x40, after a key, and at 180x50.
  - An unchanged frame writes nothing, and a key's frame is under a quarter of the first.
  - Depth, a wide glyph and links.
  - `run_session` over a channel: keys in split reads to the detail screen, a resize, close, a lone ESC closing the help after its wait, Ctrl+Q, and a refused export.
- `crates/moqui-host/tests/it/ssh.rs` `apps_runs_the_terminal_client_over_ssh`, on a russh client.
  - (`SSH_APPS` was removed Oct 21, 4.2; the test now covers a user without `SSH_SHELL`.)
  - A user with only `SSH_APPS` lands in the apps from a shell request, in true color from the `COLORTERM` env request.
  - A second channel of that user is refused by `tui_max_sessions_per_user`.
  - A window change redraws whole, and Ctrl+Q exits 0 with the terminal put back.
  - A shell user without `SSH_APPS` is told why at the prompt.
  - john.doe types `apps` at the prompt, goes to the Tools user list by `g`, quits, and is at `moqui>` again.
  - `apps` without a pty says to use `ssh -t`.
  - The Phase 3 Tools task itself runs on the same `Session` in `tui_screens.rs` through the in-process link.
- Unit tests: the session limits, the terminal's theme from `TERM` and the locale, and the SGR colors.

**Not yet checked.** No live terminal has run it: xterm, kitty, GNOME Terminal, tmux and the Linux console are still to do by hand (F-keys, Alt, paste, resize, 256 colors). The full Tools task by keyboard over a real `ssh` is the manual exit check.

**Run over SSH, Oct 21, 2026.**
- An agent ran the full Tools task as john.doe against the release build, by keyboard over a real `ssh`, with OpenSSH on a pty and a small screen model. `runtime/conf/MoquiDevConf.xml` now sets `ssh_port` 2222, on 127.0.0.1.
- The run found that a parent level rendered alone (the client asks for each level, `/apps/system/Security.tui`) showed "Tried to render subscreen … no subscreens.@default-item" for its `subscreens-active`. A level under it is the client's next request, so in an always-standalone mode `render_next_screen` (`sri/sections.rs`) now renders nothing there. Test: `screen::tests::tui::a_parent_level_alone_has_an_empty_active_node`.

### Phase 3c — agents

1. **`agent-apps`.** The client for a program: deterministic answers (settled, text or JSON), elements by id, number or label, server-held sessions so each step is one `ssh`, a line mode, and `moqui tui --agent`.

**Exit:** an agent completes the Phase 3 Tools task with one plain `ssh … agent-apps <id> <command>` per step and no harness.

#### Phase 3c findings

Recorded 2026-10-21.

**The client.** `moqui_tui::Agent` (`crates/moqui-tui/src/ui/agent.rs`) runs one command line on an `AppView` and answers it.
- **Settling.** Keys run their requests in line (`AppView::on_key`). `Session::fire_waits` then fires the server-search and palette waits at once, not after 300 ms, and the requests run again until none is left. The counters keep their time.
- **Answer.**
  - The header names the path with its query, the full breadcrumb (`Session::crumbs`, now also the title line's), the focus as kind, id and label, any overlay, and the message.
  - Then the screen: drawn into a Revue `Buffer` as `ui/encode.rs` does, with reverse-video runs (the focus) between `[[ ]]`. A run of three or more empty rows is one counted line.
  - Then `--- ok` or `--- error <why>`.
  - `--json` gives the same as an object with every row and the elements.
- **Elements.**
  - An element is the focus order of the current scope. Its label is the field title, link text, dialog title or menu text, else the node's `text`, `button`, `title` or `name`.
  - It is found by `#n`, by id, by the same label, or by a label holding the text. `Session::focus_key` moves to it.
- **`set` and `do`.**
  - `set` writes a text field through `Session::set_buffer`, so depends-on runs. It chooses a drop-down as a person does (F4, the text, Enter on the exact match) and checks the value taken.
  - `do Yes` answers an open confirm.
  - `find` is `/` with the text, so list rows are reached and `key Right` follows a row's link.

**The host.** `crates/moqui-host/src/ssh/agent.rs`.
- `agent-apps open` keeps an `Agent<HostLink>` in a registry by an 8-character random id, found only by its own user.
- Each command runs under `spawn_blocking` with the session locked. A session unused for `IDLE` is dropped on the next `agent-apps` call, which gives its `tui_max_sessions` slot back.
- `agent-apps` alone is the line mode. Exit status 0, 1 or 2.
- `moqui tui --agent` is the line mode on stdin.

**Exit evidence.**
- `crates/moqui-tui/tests/agent.rs` (6 tests): the answer's form, elements by number, id and label, `set` on a text field and on a dependent drop-down by label and by value, a settled palette search, JSON, a confirm, the empty-row count, and errors.
- `crates/moqui-host/tests/it/ssh.rs` `agent_apps_answers_one_command_per_exec`: open, the user list's find by label, JSON elements, another user refused, `tui_max_sessions_per_user`, `SSH_APPS` refused (removed Oct 21, 4.2), `close`, and the line mode.
- **Live.** The Tools task, run by an agent on the release build with one `ssh -S <control socket> … agent-apps <id> <command>` per step: 20 commands, none of them a wait, a retry or a Tab count. The command list is in docs/TUI.md "For agents".

**Still awkward for an agent.**
- Unnamed links have path ids (`widgets/container-row/row-col[0]/link[0]`). Labels work, but an id is no help.
- A list row is reached by `find`, not by naming the row.
- Every answer carries the whole screen, even when only the message changed.

#### Quirks found over SSH (Oct 21, 2026)

To fix next, with `agent-apps` to check each. Items later phases already cover are left out.

1. Below the docking width the title line's counters squeeze the breadcrumb, to `system ▸ Ser…` at 120 columns. The counters should take their short form (1.6) or give way first.
2. A row of buttons does not wrap: AppList's app buttons run off the right edge at 120 columns.
3. Find text (`/`) does not search the second part of a stacked cell. `MY_ACCOUNT_USERS` under "My Account Users" is "not on this screen".
4. The confirm dialog shows its choice only as `< Yes >` against `[ No ]`. Elsewhere the chosen button is in reverse video.
5. Run Job on ServiceJobDetail gives no message, and its "Last Run" still shows the old time after F5. Check whether it is the screen's data or a message lost on the redirect.
6. The breadcrumb repeats the leaf as its name and its title: `JobRunDetail ▸ Job Run Detail`.
7. ServiceJobDetail's empty From Date shows the current time. Check against the browser whether it is the field's default value.

#### Quirks: findings (Oct 21, 2026)

Each fix was checked with `agent-apps` against the release build at 120 columns.

1. **The counters.** `title_line` takes the long counters only when the crumbs still fit beside them, then the short form
   (`T3 C1 N2`), then drops the user, then the clock. At 120 columns the Security crumbs are whole with `T3 C4 N4 …`.
2. **Buttons wrap.** A run of links and buttons is split into lines that fit (`layout::wrap_inline`, by
   `Inline::width`, which matches the looks `draw_inline` draws and counts a `[n]` while links are numbered). So are a
   form's button line and a wizard's buttons. A box toolbar stays one line. AppList's apps take five lines at 120.
   Test: `layout.rs` `a_row_of_buttons_wraps`.
3. **Find in a stacked cell.** `find_next` searches every part of a cell. `find MY_ACCOUNT_USERS` finds the row.
4. **The confirm choice.** The choices are `[ No ]  [ Yes ]`, and the chosen one is reversed as a focused button is; the
   agent shows it as `[[[ Yes ]]]`.
5. **Run Job.** As Java: `runJob` adds no message, the run is async, and Last Run is the run lock's time, which a
   run-now never takes. Two things were wrong, and are fixed:
   - A link to a transition that runs actions was a plain GET. Over HTTP the server refuses its parameters on the url
     (`insecure_transition`, as Java). `ScreenTui.js` now marks it `method: post` when `sri.isAnchorLink` says it is not
     an anchor, as qvt's `m-form` and Java's hidden form do; the client posts it as a form. With no message of its
     own, the answer is "Done", not "Saved".
   - A message from a transition reached as a level was lost: the client asks for each level as its own request, with
     no session between them for the flash. `wants_json` (`render.rs`) is true for a `tui` request, so such a transition
     answers `send_json_redirect` with its messages; `Link::level` follows it and puts the messages ahead of the
     screen's. Thread List's Interrupt shows "Thread … not interrupted". Tests: `tui.rs`
     `a_transition_asked_for_in_tui_answers_its_redirect_as_json`, and steps in `tui_screens.rs`.
6. **The breadcrumb.** The title takes the place of the leaf's name: `JobRuns ▸ Job Run Detail`. The agent's `path`
   line keeps the name.
7. **From Date.** As Java and qvt: `auto-fields-entity` gives an edit `fromDate` a `nowTimestamp` default
   (`screen/form.rs:1116`, Java `ScreenForm.groovy:819`). No change.

Also: a `tui` request that needs a login gets the 401, not the browser's redirect to `/Login` (`redirect_to_login`),
so a remote client shows its login dialog.

### Phase 4 — Reach and polish

- **Remote.** Remote client mode with stored login keys, and the `tui-client` build.
- **Theming.** Themes from the server theme, dark/light detection, `NO_COLOR` and ASCII verified.
- **Views and diagrams.** List views (board, agenda) on the Phase 3 components, and the Mermaid evaluation for the `diagram` component.
- **Markdown editor.** The `RichTextEditor` round-trip check and the styled-source editor of 3.8.
- **Push.** Counter updates in place of polling: the in-process link subscribes to the notification topic directly; the remote client uses `/notws`.
- **CI.** Snapshot tests in CI (section 6), the `--snapshot` command.
- **On demand.** The browser terminal; the log pane over SSH for permitted users.

#### Phase 4 findings

Recorded 2026-10-21. Built: the remote client, push, theming, list views, the Markdown source editor, the Gallery
snapshots and the log pane over SSH. Evaluated and not built: a `diagram` component. Not built, by the user's choice:
the browser terminal (4.4); it was built later the same day, once SSH was open to every user (4.2, 4.4 "As built").

**Remote client (4.3).**
- `crates/moqui-host/src/tui.rs` is now one `Link<T: Transport>`: the level, transition, options, json, fragment, raw and
  shell calls and their parsing are shared. `HostLink` is `Link<InProcess>`. `HttpLink` (`tui_remote.rs`) is `Link<Http>`
  on blocking reqwest, building a `RenderOutput` from each response so the same parsing reads it. Redirects are not
  followed by reqwest; `Link::level` follows them, a JSON redirect included (quirk 5).
- **Login.** The REST `login` takes `createLoginKey=true` and answers `loginKey` (`dispatch.rs` `send_logged_in`, through
  `get_login_key`; Parity-Diffs). The key file is `~/.config/moqui/tui/<host>_<port>.toml`, mode 0600, with the user and
  the key. No expiry is kept: a 401 opens the login dialog, which is the same for a key that expired or was revoked.
  `--forget` removes it.
- **`tui-client` is not a feature.** moqui-host with `tui` alone already builds without a database driver or the HTTP
  server (moqui-core is always linked), and `--server` runs outside the axum/DB cfg of `tui_cmd`. check-packages.sh
  builds `-p moqui-host --no-default-features --features tui`.
- Live: `moqui tui --agent --server http://localhost:8080` against the dev server logged in by typing into the login
  dialog (the agent's `set` names no login field), found a user, opened it, ran a job; a second run used the stored key.
- Test: `tests/it/tui_remote.rs`.

**Push.** `ScreenLink::subscribe(PushSink) -> Option<Subscription>`, the subscription a guard that ends it on drop.
`InProcess` listens on the process's notification topic, filtered to the link's user id (known after its first call).
`Http` runs `/notws` on a thread of its own, with the login key, reconnecting with a backoff; `/notws` now takes a
`login_key` header when there is no session cookie (`notws.rs` `key_user`). A push reads the counters and puts the
notification's title on the message line; with a subscription, the counters are read every ten minutes, not each
minute. The local loop drains pushes on its tick, the session loop gets `SessionEvent::Push`, the agent applies them
before its next command. Tests: `tui_remote.rs` (over `/notws`), `tui_screens.rs` `drive_push`.

**Theming.**
- The shell data's `tui.colors` takes the brand colors of `STRT_QUASAR_CONFIG` for the six roles they name, under
  `STRT_TUI_COLORS`.
- A color name may start `dark.` or `light.`.
- The SSH session loop asks OSC 11 (ended with ST) and the decoder reads the answer as `Decoded::Background`. Local
  mode does not ask: Revue reads its input, and an answer that came after a timeout would be taken as keys.
- Verifying ASCII found client marks still written in it (`…`, `–`, `·`): the painter now writes a one-column ASCII
  stand-in for each of its marks in ASCII mode, so nothing moves.

**List views.**
- `ScreenTui.js` writes a form-list's `views`: the kinds in the form's order, the chosen one (a `view` parameter, else
  the `LIST_VIEW_` preference qvt keeps, else the first), and the props of a `board` and a `calendar` with the page's
  rows inline. The board's columns come from `get#ListViewColumns`, as qvt's do; a card's fields are their cell text,
  and its `_open` is the row's first link.
- The session swaps the list node for its view's component node before indexing (`apply_views`, the list kept aside),
  so layout, focus and keys are a component's. `v` cycles and saves the preference.
- Board sums and the calendar's create and resize are not in the terminal.
- Test: `tui_screens.rs` `drive_views` (board, list, calendar, a card moved to its next status).

**Diagrams.** Revue 3.10.1's `Diagram` was run on a status flow (nine transitions) at 100 and 180 columns:
- it lays the nodes on a square grid with straight vertical lines, so most edges are missing and the lines it draws
  join the wrong nodes;
- its parser reads only `A --> B` lines, so a `stateDiagram-v2` (Tools' Status Flows) reads `B: label` as a node.

Not usable, so no `diagram` component; Status Flows' twin stays text.

**Markdown.**
- `tests/markdown_roundtrip.rs` runs `RichTextEditor`'s round trip:
  - the Gallery sample, the wiki pages and plans/README.md come back as they went in;
  - a line that starts with digits and holds `. ` is rewritten as a numbered item (`00:53 … quotes. An answer is:`
    became `1. An answer is:` in docs/TUI.md);
  - inline markup is drawn plain.
- So the editor is styled source (3.8). `markdown::source_looks` gives each character its look from pulldown-cmark's
  offsets, `draw.rs` `source_line` draws the runs, and Alt+B, Alt+I and Alt+K wrap the word at the cursor. F1 lists
  them on a Markdown field.
- The text area does not scroll: a source longer than its rows shows its first lines, as before.

**Snapshots.**
- `tui_screens.rs` `snapshot_gallery` keeps every Gallery screen at 180x50 in `tests/tui_snapshots/`, and List, Detail
  and Widgets in ASCII.
- Every screen in monochrome must write no SGR color.
- Today's date and a time after it are masked, since some records are stamped while the data loads. The Calendar has
  no golden: its events are days of this month.
- No CI service is configured. check-packages.sh runs these, as before.

**Log pane over SSH.**
- `logging::tap_at(capacity, least)` keeps the lines at a level and above.
- `apps` asks whether the user may view `component://tools/screen/System.xml` (the Log Viewer's app, which grants by
  inheritance) and, if so, forwards WARN and ERROR lines as `SessionEvent::Log`. `SessionOptions::log` says the pane is
  offered; otherwise Alt+L says it is not.
- Tests: `logging` `a_tap_takes_the_lines_at_its_level_and_above`, `session_loop.rs`
  `the_log_pane_shows_the_lines_the_caller_sends`, `ssh.rs` (apps.only refused, an admin's pane opens).

**Found on the way.**
- The `agent`, `session_loop` and `markdown_roundtrip` tests and `theme::sgr_color` failed the model-only clippy of
  check-packages.sh. The tests now require `ui`, and `sgr_color` allows dead code without it.
- A changed text area drew its `•` over the first column of its text; the marks now follow the text's room.
- `ssh.rs` failed once at the shell user's `exit` (no exit status within 30 s) and passed on each run after; not
  reproduced.

### Phase 5 — Mouse

Added Oct 21, 2026 (user), after the browser terminal: clicks, the wheel, and drag to select with copy on select. The
keyboard stays complete; the mouse is a second way in, never the only one.

**Today.** No channel has a mouse. `ENTER` (`ui/encode.rs`) turns on no mouse mode, the decoder (`decode.rs`) reads no
mouse report, and `run_local` builds Revue with `mouse_capture(false)` and drops `Event::Mouse` (`ui/run.rs`). Revue's
own mouse handling does not apply, because the client paints its own cells (`ui/paint.rs`), so nothing knows what is
under a cell.

1. **Modes.** `ENTER` adds button-event tracking with SGR coordinates (`?1002h`, `?1006h`), and `LEAVE` turns them off.
   `?1002` (motion while a button is down) is needed for the drag; `?1003` (all motion) is not used, as it would send a
   report for every move. `run_local` turns Revue's mouse capture on and maps `Event::Mouse` to the same `Mouse` event the
   decoder gives, so `Session` has one mouse path for every channel. xterm.js sends these reports as data and its X10
   bytes through `onBinary`, both already forwarded by `Terminal.qvue`.
2. **Decoder.** `decode.rs` reads SGR reports, `ESC [ < b ; x ; y M` (press, or motion with a button) and `m` (release),
   into `Decoded::Mouse { kind: Press | Drag | Release | Wheel(up/down), button, column, row, modifiers }`, with the
   coordinates 1-based to 0-based. A report split across reads waits for the rest, as a CSI does. The legacy X10
   encoding (`ESC [ M` and three bytes) is read too, for a terminal that ignores `?1006`.
3. **Hit map.** Each frame, `Painter` records the area of every element it draws that a click can act on, in a hit map
   kept on `AppView` beside the frame: `Hit { rect, target }`, the last drawn on top. The draw sites name their target
   through one call around their drawing (`paint.target(target, |paint| ...)`), as the link id is named for OSC 8 today.
   Targets:
   - on the canvas: a link, a button, a field (its label and its box), a list row (and its first-column link), a column
     title (the sort), a subscreen tab, a tree node and its toggle, a box title that opens and closes, a status bar stage;
   - on the fixed rows: a drawer item and the drawer's toggle, the title line's app name (the palette), a key-line entry
     (it presses its key);
   - in the overlays, which cover the canvas: a dialog's buttons and fields, a drop-down list's choices, the palette's and
     the quick create's entries, a menu's items, the confirm's choices, the login dialog's fields and its button. A click
     outside a dialog does nothing (it does not close it, so a stray click loses no edits).
   The scrolled canvas records its hits at screen rows after its scroll, and only the rows on screen.
4. **Clicks and the wheel** (`Session::on_mouse`).
   - A press and release on the same target is a click. A click focuses the target and then does what Enter does on it: a
     link follows, a button presses, a tab opens, a row opens its first link (or edits the row when the list has fields),
     a drop-down choice picks, a key-line entry presses its key. On a field it only focuses (and a check box toggles, as
     Space does). The leave-a-changed-form question and every other confirmation stay as they are by key.
   - A double click on a list row edits the row when the list has fields; elsewhere it is two clicks.
   - The wheel scrolls the canvas three rows, or moves through the open drop-down list or palette; over a list at its end
     it does not page (PgDn does), so a wheel cannot send page requests at the speed of a flick.
   - Later, not in this phase: placing the cursor in a text field at the click, and dragging a column wider.
5. **Drag to select, copy on select.** A press that moves off its cell before the release is a selection, not a click.
   - The selection runs in reading order from the press to the pointer, as a terminal's own does, and Alt while dragging
     makes it a rectangle (for a column of a list). It is drawn in the selection look of the theme (reverse video in
     monochrome), over the frame, and goes on the next key, click or redraw.
   - Text comes from the frame's cells, not the model: what is on screen, wide glyphs once, trailing blanks of each row
     dropped, rows joined by newlines. A password field is drawn as `•`, so it is never copied.
   - Double click on text that is not a target selects the word, and triple click the row.
   - On release the text goes to the clipboard by OSC 52 (`ESC ] 52 ; c ; <base64> ST`), and the message line says
     "Copied 42 characters". Local mode writes OSC 52 to its own terminal the same way, so it needs no clipboard crate and
     works over a nested ssh or tmux (`set-clipboard on`). OSC 52 is at most 64 KiB here; a longer selection is cut,
     and the message says so.
   - Terminals differ. kitty, WezTerm, Alacritty, foot, Windows Terminal and xterm (with `allowWindowOps` or
     `disallowedWindowOps` set for it) take OSC 52; iTerm2 when the user allows it; GNOME Terminal and the other VTE
     terminals do not. So the first copy of a session also says how to fall back: Shift+drag (Option+drag on macOS) for
     the terminal's own selection, which every terminal keeps while an app has the mouse.
   - In the browser, `Terminal.qvue` registers an OSC 52 handler (`term.parser.registerOscHandler(52, …)`) that writes
     the text with `navigator.clipboard.writeText`. The release that ended the drag is a user gesture shortly before, and
     the page is a secure context or localhost; where the browser still refuses, the page shows the text in a small
     dialog to copy by hand. `@xterm/addon-clipboard` is not needed for this.
6. **Mouse off.** Alt+P turns the mouse modes off and on, so the terminal's own selection and its right-click menu work
   without Shift. The choice is the user's `TUI_MOUSE` preference, as F9's drawer is (`TUI_DRAWER_OPEN`, D11); it is on
   when not set. F1 and the key line show it. The agent sends no mouse; `agent-apps` gets `click <element>` only if
   tests want it, since it already names elements.

**Exit:** in the browser terminal, over SSH in kitty and GNOME Terminal, and in local mode: a menu item, a list row's
link, a field, a tab, a dialog button and a drop-down choice each work by click; the wheel scrolls a long screen; a drag
across three rows of a list puts their text on the clipboard where the terminal takes OSC 52, and says how to fall back
where it does not; Alt+P gives the terminal its own selection back.

**Tests.**
- Decoder: SGR press, drag, release and wheel with modifiers; X10; a report split across reads; coordinates past 223
  (SGR only).
- Hit map: the Gallery list and detail at 100x40, the target under chosen cells, in the canvas after a scroll, and in a
  dialog and a drop-down over it.
- Session (`FakeLink`): a click on a link sends its level request; on a field focuses it; on a dialog's Cancel closes
  it; outside a dialog does nothing; the wheel scrolls; a drag selects and writes one OSC 52 with the expected text;
  Alt+P writes the mode sequences and the preference.
- `session_loop.rs`: the mode sequences in `ENTER` and `LEAVE`, and a click as bytes through `run_session`.
- The browser page's OSC 52 handler by hand, in Chrome and Firefox.


#### Leftovers (Oct 21, 2026)

Built after Phase 4, from Remaining-Work and the Phase 3 "Open" list.

- **Second factor.** The login dialog takes the code. A right password answers `LinkError::Code` with the account's
  factors, and the dialog moves to Code, keeping the password only until the try with the code. In process,
  `user::login_with_code` checks the password and the code in one request, as `/rest/login` does; over HTTP it is
  `/rest/login` with `code`. Alt+S sends a code to the first factor that takes one (`user::send_login_code`; over HTTP,
  `/rest/login` and then `sendOtp` with that session's cookie and CSRF token). A right password that wants a code is
  not a refused login against `MAX_LOGIN_FAILURES`; a wrong code is.
- **Client address.** `user::with_client_ip` gives a login with no HTTP request the address `ipAllowed` is checked
  against. The SSH password login and the dialog login over SSH pass the peer; a browser terminal session has no
  dialog.
- **The agent and the login dialog.** While it is open, its fields are the elements `login.username`,
  `login.password` and `login.code`, and its button is `login.submit`; `set` fills them and `do Log in` sends the
  dialog. The password never shows in an answer.
- **Text areas** scroll with their cursor. PgUp and PgDn (and later the wheel) move by their rows, with `▲` and `▼` on
  the bar; the Markdown preview scrolls too.
- **A `container-dialog` in a list cell** is the row's `dialog` node. `ScreenTui.js` captures the writes of its
  statements (`$captured`, a `$sink` the writers use), so the body comes with the row's values. A section inside such a
  dialog is an `error` node. F10 offers the dialog for the current row.
- **`apps.xml`** answers 401 with no user, through `ec.web.sendJsonError`, as `webroot.xml` does.
- **Found on the way.** The Gallery transitions wrote `<message message="…"/>`, but the XSD's `message` takes its text
  as content, so their messages were empty in every mode. They now write the text as content.

#### Phase 5 findings

Recorded 2026-10-21. Built as planned above, with these decisions:

- `Target` and `Mouse` are in `keys.rs`, so `Session::click` (`session/mouse.rs`) builds without Revue. The painter
  records areas through `Painter::target` (the cells a draw wrote), `area` and `canvas_area`. `clear`, which every
  overlay calls, records a `Block`, so nothing under an overlay takes a click. `AppView` keeps the hits and the frame's
  characters from its last `render`, and draws the selection in reverse video over the frame.
- **A click on a list row** makes it the current row, and a double click is Enter on it. The plan had a single click
  open the row, but then picking a row would leave the screen. The first column of a row with a link of its own, and a
  link in a cell, follow on a single click, as the browser's record link does.
- **Overlays.** A click on a drop-down choice, menu item, palette row, confirm choice or picker day sets that overlay's
  index and presses Enter, so it takes the keys' path. While such a popup is open, a click elsewhere does nothing.
- **The wheel** sets a free scroll that `draw` keeps until a key or a click.
- **Local mode** keeps Revue's `mouse_capture(false)` and writes `MOUSE_ON` and `MOUSE_OFF` itself, since Revue fixes
  capture when the app is built. Crossterm parses the reports either way. The OSC 52 text goes to stdout between
  Revue's frames.
- The base64 for OSC 52 is a local function in `ui/encode.rs`, so no new crate is needed.

**Tests.**
- `tests/mouse.rs` (9): the hit map under chosen cells; clicks on a link, a field, a list row, a row link, a key line
  entry and a drop-down choice; a dialog's clicks and one outside it; the wheel; a drag in reading order and as a
  rectangle; a double-click word; Alt+P.
- `decode.rs`: SGR and X10, split reads, past column 223.
- `session_loop.rs`: the modes, a drag's OSC 52 and a click as SGR bytes through `run_session`.
- `encode.rs`: base64.
- `tui_screens` keys: the key line names Alt+P.

**Not checked live.** No real terminal or browser has run Phase 5 yet: kitty, GNOME Terminal (the fallback message),
local mode's crossterm reports, and the browser's OSC 52 in Chrome and Firefox. These are in Remaining-Work.

---

## 6. Client tests

| Test | How | Where |
| --- | --- | --- |
| Model round trip | Every server golden from 2.8 deserializes and re-serializes equal | `moqui-core` render tests |
| Layout | `layout` computed for fixture models at 72/100/120/153/180/240 columns, asserted as a text outline of boxes and widths | `crates/moqui-tui/tests/layout.rs` |
| Snapshots | `PipelineHarness` draws fixture models at 72×24, 100×40, 180×50. The screen text is compared with checked-in files, rewritten with `UPDATE_GOLDEN=1` as in the rest of the repository (not Revue's `REVUE_UPDATE_SNAPSHOTS`). One set uses ASCII and no color. | `crates/moqui-tui/tests/snapshots/` |
| Key flows | `FakeLink` records requests. Scripted keys check focus order, Enter submit, F4 prompt, F7/F8 paging, Esc and dirty-leave, palette, numbered links, that a superseded answer is dropped, and that a held PgDn sends at most one request in flight and one pending. | `crates/moqui-tui/tests/keys.rs` |
| Input decoder | Byte sequences of xterm, kitty, tmux and the Linux console to events, paste with newlines, split sequences across reads; mouse reports in SGR and X10 (Phase 5) | unit tests in `decode.rs` |
| Mouse | The hit map under chosen cells (canvas, scrolled canvas, overlays), clicks and the wheel, drag to select and the OSC 52 it writes, Alt+P (Phase 5) | `crates/moqui-tui/tests/mouse.rs` |
| Sanitizing | Control and bidi characters in every string field of a model are not drawn | unit test |
| In-process link | A real factory: login, render, submit with a validation error, options, 401 | `crates/moqui-host/tests/it/tui.rs` |
| SSH | pty request with size, `apps` start, window change to a new size, exit back to the prompt. A user without `SSH_SHELL` logs in by password and lands in `apps` from `ssh user@host` and from `ssh -t host apps`. A client with no key gets the login dialog, and is disconnected after `LOGIN_WAIT` without a login; `ssh_login_dialog=false` turns it off. Channels past `tui_max_sessions` are refused. | extend `crates/moqui-host/tests/it/ssh.rs` |
| Gallery | Every Gallery screen at 180×50 as a snapshot in the example component's tests, with a fixed date, so a style change shows in the diff as the screenshots do for qvt. Board and Calendar through their components. | `runtime/base-component/example/test/` |
| Components | Each framework component drawn from fixture props at two widths, its keys, and the unknown-name placeholder | `crates/moqui-tui/tests/components.rs` |
| Theme keys | `STRT_TUI_KEYS` overrides merge, a bad entry is skipped and logged, a binding of Esc/Enter/Tab is refused, F1 and the key line show the merged keys | `crates/moqui-tui/tests/keys.rs` |

---

## 7. Risks and open questions

These update the proposal's list with what the survey found.

| Item | State after this plan |
| --- | --- |
| API churn | Pin `=3.10.1`. Upgrade only with the snapshot suite green. Revue's `STABILITY.md` marks `FormState` and `Store` as Evolving, and this plan uses neither. |
| One maintainer | Unchanged. Contained by D3 and D5: the model, link, nav, state and layout modules have no Revue import. |
| Toolchain | Revue needs 1.95; the workspace needs 1.94 and builds on 1.99. No conflict. |
| Terminal I/O (was the proposal's "pluggable backend?") | **Answered: no**, and not needed. Revue has a `Backend` trait that `Terminal` does not use, and `App::run` is tied to the process. Phase 3b built route 4 of 3.5: our own frame loop on the public `Buffer`, `RenderContext` and `diff`. |
| Date picker | **Exists**, with the fixed today/now bug and no parser: the `DateField` wrapper. |
| Inline table editing | **Exists** in `DataGrid`, without a validation hook: the `ListTable` wrapper owns validation. |
| Large lists | **Answered:** `DataGrid` and `Table` draw only visible rows. Server paging stays, with the page sized to the screen. |
| Key routing | Revue does not route keys. The interpreter does (D5), which the JSON model needs anyway. |
| Paste | Not enabled by Revue. Enabled by our loops (3.3). |
| Mouse and copy | Not given by Revue for this client (it paints its own cells). Phase 5 records a hit map per frame. Capturing the mouse takes the terminal's own drag selection, so the client selects and copies itself by OSC 52, which GNOME Terminal and other VTE terminals do not take: they keep Shift+drag, and Alt+P turns the mouse off. |
| Idle cost | Revue ticks every 16 ms. The session loop blocks until input or its next deadline, about once a minute when idle (3.5). |
| `.qvue` screens and inline `qvt` | **Decided (D10):** each gets a `tui` implementation, and until then a placeholder with a browser link (2.9). For the BOSS Comms Inbox (a `.qvue`) to work as an email client in the terminal, BOSS writes its `tui` texts and its components (message list, reader, compose on the 3.8 editor) behind its crate's `tui` feature. |
| Custom components in a remote client | A `tui-client` binary has only the components it was built with. Shown as placeholders, and listed by `=` (2.9). |
| Security over SSH | Open to every user since Oct 21 (4.2), as HTTP is: per-request login as the user, every screen checked as that user, the login dialog for a client with no key (limited by `LOGIN_WAIT`, `MAX_LOGIN_FAILURES` and `ssh_max_sessions`), no file transfer, sanitized output (2.7). |
| Rich text | **Decided (D12):** read-only with "Edit in browser". A two-way Markdown editor in the `markdown-view` look comes later (3.8). |
| Preferences | **Decided (D11):** breakpoints, colors and key bindings are per theme. Only the drawer's open state is per user. Per-user bindings and their UI wait. |

**Upstream asks for Revue** (issues first, PRs where small):

- a public run-on-writer entry with an external event source ([#1153](https://github.com/hawk90/revue/issues/1153); no longer blocking, after route 4 of 3.5);
- bracketed paste in `Terminal::init`;
- real `Date::today`/`Time::now`;
- values on `Select` options, and mutable `Combobox` options;
- a lazy-load hook on `Tree`;
- word wrap in `Text`;
- widgets reading the theme signal;
- a source view mode for `RichTextEditor`, if 3.8 goes that way.

---

## 8. Bookkeeping

- `plans/README.md`: list this plan under the active plans, and keep the proposal in `proposals/`. Per the README conventions, the proposal moves to `archive/` when Phase 4 is done, and the parts still open go to `Remaining-Work.md`.
- `plans/Future-Plans.md`: a paragraph for the proposal pointing here.
- `plans/Parity-Mapping.md`: `tui` has no Java counterpart, so it goes in the "not in Java" list with `ScreenTui.js` and `crates/moqui-tui`.
- `docs/TUI.md` (new): the user guide (channels, keys, widths, themes), and for screen authors: writing a `tui` text, the component props of the framework set, and registering a component from a component crate. `docs/Packaging.md`: the features. `webroot/js/README.md`: the model contract section.
- `plans/archive/License-Review.md` and `deny.toml`: the new crates of 3.1, and xterm.js if 4.4 is built.
- `xml-screen-3.xsd`: the `text` `type` documentation names `tui` beside `qvt` and `qvue`, and states D10's rule.
- In the ironstone-boss repository (its own git repo): the style guide has the rule since Oct 20, 2026, in principle 3, §4.8 "The terminal twin", the Terminal row of §7, the §8.1 checklist and a §9 reference. Still to do there: BOSS's lint worklist goes into its plan once the lint exists, and design-doc-guide's **Impl** column may name the `tui` twin of a `qvue` screen.
