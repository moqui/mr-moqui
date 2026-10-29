# moqui-html.js

The client of the `html` render mode, with `../css/moqui-html.css`. It has no dependencies and no build step: the
file is plain ES2020 and is served as it is. About 8 KB gzipped. `ScreenHtml.js` (`framework/template/screen/`)
writes the markup; this file binds the data attributes below. A page without JavaScript still works: forms post,
links follow, and a select shows its current option.

Tests: `moqui test-client webroot --offline` (`test/client/*.spec.js`: `moqui-html.spec.js` for the pure functions, the
others for dialogs, keys, downloads and the saved-find popup). A full screen is also in the Gate 3 Chromium smoke
(`crates/moqui-host/tests/gate3_browser.rs`).

## Attributes

| Element | Attribute | What the client does |
|---|---|---|
| `button`, `a` | `data-dialog="id"` | `showModal()` of the `dialog` with that id |
| `dialog` | `data-open` | opens when the page loads |
| `dialog` | `data-load="url"` | loads its `.body` from the URL when opened, empties it when closed |
| any | `data-close` | closes the dialog around it |
| `div` | `data-load="url"` | loads the URL into the element when the page loads. `moqui.load(el)` loads it again |
| `a` | `data-load-into="id"` | loads the link into the element instead of following it |
| `nav.tabs` | `data-dynamic="id"` | links with `data-load-url` load into the element and mark their tab active |
| `nav` | `data-tab-hash` | Login tabs: the `.tab-panel` sections follow the URL hash |
| any | `data-move-to="id"` | moves the element into `#id` (a `ul` gets a `li` around it): popup menus and crumbs go to the navbar |
| `button`, `a` | `data-confirm="text"` | `confirm()` before the click does anything |
| `select` | `data-saved-finds` | navigates to the selected option's `data-url` |
| `button` | `data-clear="formId"` | clears the form's fields |
| `button` | `data-theme-toggle`, `data-pref-url` | switches dark and light, and posts `OUTER_STYLE` |
| `button` | `data-nav-toggle` | opens the navbar on a narrow screen |
| `a` | `data-history-back` | `history.back()` |
| `.alert` | `button[data-dismiss]` | removes the message |
| `form` | `data-bg`, `data-reload`, `data-msg`, `data-hide` | background submit; then reload the container with that id, toast a message, close the dialog with that id |
| `form` | `data-focus="name"` | focuses the first field whose name starts with it |
| `form` | `data-gopage="n"` | the page number box is 1-based and posts a 0-based `pageIndex` |
| `input[type=datetime-local]` | | the `T` becomes a space in the posted value (`yyyy-MM-dd HH:mm`) |
| `input[type=date\|datetime-local\|time]`, `input[data-format]` | `data-step` | date keys: `t` today or now, `y` `m` `d` `w` a year, month, day, week later, `h` `i` `s` an hour, `data-step` (or `step`) minutes, a second later, Shift earlier; a text field with a numeric `data-format` also ArrowUp and ArrowDown for a day. A native input keeps its own arrows |
| `select` | `class="submit-on-close"` | submits its form when its Tom Select menu closes, or a plain select changes |
| `select` | `data-ts="select\|remote\|ac"` | Tom Select, loaded on first use; it opens on a click or typing, not on focus, and dynamic options keep the transition's order. `data-combo` allows new values, `data-orderby="inputId"` writes the order-by text there. A quick ENTER or TAB waits for the options of what was typed; TAB takes the highlighted option and moves on |
| `select`, `input`, `span` | `data-url`, `data-params`, `data-depends`, `data-optional` | transition URL, fixed parameters (JSON), `[[fieldId, parameter], ...]` read when the options load or the field changes |
| `select` | `data-search`, `data-min`, `data-delay`, `data-label`, `data-value`, `data-empty`, `data-use-actual` | search as you type, minimum length, delay, label and value field names, empty first option, the typed text is the value |
| `input` | `data-default` | text-line `default-transition`: fills the value from the transition |
| `span[data-dynamic="fieldId"]` | `data-hidden` | display with `dynamic-transition`: sets `fieldId_display` and the hidden input |
| `.tree` | `data-url`, `data-params` | fetches nodes (POST, `treeNodeId`, `treeNodeName`), draws a nested list, opens children on click |
| `.editable` | `data-url`, `data-params`, `data-name`, `data-widget`, `data-load-url`, `data-load-params`, `data-options`, `data-value`, `data-empty` | click to edit, save by POST; `data-widget` `select` (the options, or those `data-load-url` answers), `date` and `date-time` (a date input) |
| `.status-bar` | `form.status-form` | the `status-bar` element: the stage chips (`data-state`, `data-color`) and a form per transition; nothing to bind beyond `data-confirm` |
| `ul` | `data-sortable-tree="inputId"` | form-list column picker with SortableJS; the hierarchy goes to the input as JSON |
| any | `data-btn`, `data-size`, `data-type`, `data-align`, `data-wide`, `data-submit` | style only |
| `meta[name=moquiSessionToken]` | | the CSRF token, sent as the `X-CSRF-Token` header and `moquiSessionToken` parameter on every request |
| `meta[name=moqui-notify]` | | the path of the notification WebSocket. Nothing connects when it is absent |

`window.moqui` has `bind(element)` (call it after you insert html), `notify(text, type)`, `load(element, url)`,
`request(url, fetchOptions)` and `post(url, params)`.

# The qvt client

The client of the `qvt` render mode and of the `/qapps` shell: Vue 3.5 (the build with the template compiler) and
Quasar 2.35 from `../libs`, with no build step and no other library. The `html` mode pages load `moqui-html.js`
instead; the two never share a page (both define `window.moqui`). The full reference is
[docs/ui/Qvt-Client.md](../../../../../../docs/ui/Qvt-Client.md).

| File | What |
|---|---|
| `MoquiLib.js` | the `moqui` object: type checks, URLs, `request` (fetch with the session token), `loadScript`, `parseVue`, `NotificationClient`, `LruMap` |
| `MoquiL10n.js` | `moqui.l10n`: number, currency and date formatting, a copy of `moqui-lib`'s `format.rs` and `datefmt.rs` |
| `WebrootVue.qvt.js` | the `m-*` components and the `/qapps` application |
| `WebrootPublic.qvt.js` | the public shell of the anonymous token routes (`/s/<token>`) |
| `WebrootChat.js` | `moqui.chat`: the chat store and presence, and the account menu's status components |
| `WebrootAssist.js` | `moqui.assist`: the Assist drawer, its store, transport and follow ([Assist](../../../../../../docs/ai/Assist.md)) |
| `MoquiQzComponent.js` | QZ Tray printing, loaded only when the user preference `qz.print.enabled` is `true` |
| `../css/WebrootVue.qvt.css` | the styles of the shell, the div tables and the widgets |

Tests: `moqui test-client webroot --offline` runs the page specs under `test/client/`; what needs a real browser is the
Gate 3 Chromium smoke (`crates/moqui-host/tests/gate3_browser.rs`).

Sections of the reference: Page shell and settings · Loading a screen · The components the macros write · Shell data
and providers · Preferences · Appearance · Record panels, rich text, avatars and record cards · List views: board,
calendar and tree · Layout panels: split, wizard and stack · Chat store and presence · Notification listeners ·
Collaborative editing · Mail sign-in popup · Server-static screens · The tui screen model · Not here.
