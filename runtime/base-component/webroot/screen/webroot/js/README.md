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
| `span.scan-input` | `data-scan-url`, `data-params`, `data-depends`, `data-allowed`, `data-scan-id-field`, `data-no-submit`, `data-confirm-scan` | the `scan-input` field: Enter or a scanner's quick Tab in its `input[data-scan]` posts the text to the screen's `scanResolve`; one match fills the hidden id, type and scan id inputs and submits the form (or shows Confirm), several are buttons in its `.scan-result`, none is named there ([Qvt-Client](../../../../../../docs/ui/Qvt-Client.md#scan-input)) |
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
| `MoquiLib.js` | the `moqui` object: type checks, URLs, `request` (fetch with the session token), `loadScript`, `parseVue`, `NotificationClient`, `LruMap`, `saveBlob` |
| `MoquiL10n.js` | `moqui.l10n`: number, currency and date formatting, a copy of `moqui-lib`'s `format.rs` and `datefmt.rs` |
| `WebrootVue.qvt.js` | the `m-*` components and the `/qapps` application |
| `WebrootPublic.qvt.js` | the public shell of the anonymous token routes (`/s/<token>`) |
| `WebrootChat.js` | `moqui.chat`: the chat store and presence, and the account menu's status components |
| `WebrootExt.js` | `moqui.ext`: the declared actions and panels of a screen (`/extItems`), and `m-ext-actions` and `m-ext-panel`, which a `.qvue` screen puts in its menus and panes ([Qvt-Client](../../../../../../docs/ui/Qvt-Client.md#declared-actions-and-panels)) |
| `WebrootPush.js` | `moqui.push`: registers the service worker `/sw.js`, the account menu's Notifications on this device (`m-push-item`), and the Web Push subscription kept to the signed-in user ([Qvt-Client](../../../../../../docs/ui/Qvt-Client.md#install-and-web-push)) |
| `WebrootCrypto.js` | `moqui.crypto`: this browser's crypto device (matrix-sdk-crypto's OlmMachine, loaded only when needed), its keys and Olm-encrypted to-device messages ([Chat keys](../../../../../../docs/interfaces/Chat-Keys.md)) |
| `WebrootReAuth.js` | `moqui.reauth`: the recent sign-in check of sensitive actions, asking for the password and the second factor (or a fresh SSO sign-in) in a dialog ([Recent sign-in](../../../../../../docs/security/Recent-Auth.md)) |
| `WebrootAssist.js` | `moqui.assist`: the Assist drawer, its store, transport and follow ([Assist](../../../../../../docs/ai/Assist.md)) |
| `MoquiQzComponent.js` | QZ Tray printing, loaded only when the user preference `qz.print.enabled` is `true` |
| `../css/WebrootVue.qvt.css` | the styles of the shell, the div tables and the widgets |

An app around the page (the desktop app, `crates/moqui-desktop/src/bridge.js`) may define `window.moquiDesktop` with
`notify({title, body, link, tag})`, `setCounts(counts)`, `setPresence(status)`, `openExternal(url)` and
`saveFile({name, blob})`, each optional. Where it has one, the client calls it instead of a Web Notification, a new
window or a download link, gives it the shell's counts and the user's presence, and does not start Web Push.

The desktop app as a client of a remote server (configuration 3 of [Mobile-Desktop](../../../../../../plans/Mobile-Desktop.md))
shows these scripts on its own origins, which every server and account share, and its pages cannot open a socket to
the server. So `window.moquiDesktop` may also have:

- `socket(url)`: a socket-like object the host holds for `/notws` and `/callws` (text frames), which
  `moqui.openSocket(url)` returns in place of a WebSocket (the notification client and the call guest use it; the html
  mode's notification socket calls `moquiDesktop.socket` itself; the terminal's `/termws` does not connect in a client).
  The node the app runs in its own process, on its app scheme with no port, has `socket` too, for `/notws`, `/callws`
  and `/termws`, whose binary frames it carries as an `ArrayBuffer` (and takes one, or a typed array, to send);
- `storageScope`: what `moqui.scopedKey(key)` puts browser storage keys of a server's data under (the chat outbox,
  the crypto device and store, drafts); `moqui.storageScope` set to a string is used instead. Empty on the web;
- `outbox`: the host's outbox (`add(kind, payload, files)`, `list`, `retry`, `discard`, `on(fn)`), which keeps and sends
  what a page sends offline. `moqui.hostOutbox()` answers it, or null on the web; `moqui.chat` keeps its outbox there
  (and moves the one `localStorage` held into it once), and Compose queues a mail send there;
- `drafts`: the host's drafts and their files. `moqui.drafts` (`ready`, `get`, `keys`, `files`, `put`, `remove`,
  `saveFile`) is the drafts of Chat and Compose: the host's where there is one (this browser's moved in once), else
  `localStorage` under `scopedKey`.

The client's shell also gives `deviceName`, the name of its chat crypto device, which `WebrootCrypto.js` uses; its app
windows set `window.__moquiCryptoInShell`, so `moqui.crypto` makes no device there (`ensureDevice` refuses with
`elsewhere`, and `moqui.callKeys.prepare` too), and give `openInShell(path)`, which shows a comms screen (an encrypted
room or call) in the shell, which owns the calls (`moqui.call` in an app window rings silently and opens a call there).
`WebrootCall.render.js` is the render functions of `WebrootCall.js`'s components, loaded with it
(`UPDATE_GOLDEN=1 cargo test -p moqui-core --lib call_components_are_precompiled`).

With a host outbox, `moqui.connection.offline` (reactive; `moqui.onConnection(fn)`) says the server is out of reach: set
by `moqui.request` from the host's `X-Moqui-Offline` answers (from the store, or its 502 when the server is not reached)
and from no answer at all, cleared by the server's own answer and by the host's socket opening or bringing a message;
`moqui.unreachable(err)` tells such a failure. The web never goes offline, so the screens'
offline presentation is inert there.

Its bundled shell runs on the runtime build of Vue: `moqui.bundledComponent(name)` answers a screen it bundled
precompiled (`moqui.qvtBundled`), `loadComponent` answers one without fetching it, and on the runtime build a `.qvue` or
`.qjs` that is not bundled is none.

Tests: `moqui test-client webroot --offline` runs the page specs under `test/client/`; what needs a real browser is the
Gate 3 Chromium smoke (`crates/moqui-host/tests/gate3_browser.rs`).

Sections of the reference: Page shell and settings · Loading a screen · The components the macros write · Shell data
and providers · Preferences (and a screen's handheld density) · Appearance · Record panels, rich text, avatars and record
cards · List views: board, calendar and tree · Scan input · Layout panels: split, wizard and stack · Chat store and presence · Notification listeners ·
Collaborative editing · Mail sign-in popup · Server-static screens · The tui screen model · Not here.
