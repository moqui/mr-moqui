# Screen and Web Parity Review

> Archived 2026-10-21: every finding is fixed, recorded or dropped, and the last eight decisions were made the same
> day (see [Remaining-Work, Parity gap review](../Remaining-Work.md#parity-gap-review)). What stays different from Java
> is in [Parity-Diffs](../Parity-Diffs.md#screen-and-web-review-2026-10-20). The phase tables and the "checked and found
> equivalent" lists stay here as the record of what was compared.

Scope: the screen and web parts of the framework — `crates/moqui-core/src/screen/`,
`crates/moqui-core/src/web/`, the screen macro modules under
`runtime/base-component/framework/template/screen/`, and the HTTP host in `crates/moqui-host/src/`
— compared against the Moqui Java / Groovy / FreeMarker / XML sources they were ported from. This
is the screen-and-web half of [Remaining-Work, Parity gap review](../Remaining-Work.md#parity-gap-review);
the entity and service half is [EntityService-Parity-Review](EntityService-Parity-Review.md).

History:

- **2026-10-19, first pass.** One subagent per phase, 100 findings. It cited both sides and was
  mostly right about behavior, often wrong about severity and impact.
- **2026-10-20, second pass and fixes.** The three High findings (P5-01, P7-01, P8-01) and P8-02
  were fixed (`bcaeeb3`). Then the rows a user hits were fixed: P9-08 (a GET could run a write
  transition), P1-01, P1-02, P1-06, P2-07 (`d616705`). After them P1-03, P1-04, P2-02, P2-03,
  P2-06, P3-03, P4-02, P4-04, P5-02, P5-03, P5-05 to P5-09, P7-08, P8-03, P8-07, P9-09 and P9-10
  (`bbbfa24`). Rows found invalid or duplicated were removed: P1-09, P1-12, P2-01, P2-08, P2-14,
  P4-01, P4-08, P5-04, P9-04, P9-05. What the fixes keep different from Java is in
  [Parity-Diffs](../Parity-Diffs.md#screen-and-web-review-2026-10-20).
- **2026-10-21, third pass.** All 63 open rows were checked again against the current code, which
  now includes the configuration overhaul (`62d0a01`), and against the Java source. This pass
  rewrote the table descriptions where they were wrong. P3-04, P4-07, P6-05 and P7-05 describe
  behavior that is not in the code. Four more rows (P2-04, P4-05, P7-02, P7-03) are worse than the
  first two passes said. One new bug (P8-20) came up while checking P8-10. The phase narratives
  from the earlier passes are removed, since several of them were wrong. They are in git history:
  `git show 62d0a01:plans/ScreenWeb-Parity-Review.md` (before this file moved to `plans/archive/`).
- **2026-10-21, first batch fixed.** P8-20, P4-05, P2-04 and P7-02 (a) are fixed, with a test for each
  that fails on the old code. P7-02's check also stopped counting artifact hits. Every link drawn
  pushed tracked frames, so each permission check of a link counted a hit for its transition. P7-02
  (b) is now a Record. P2-15 (new, found while fixing P2-04) was decided the same day: keep the
  port's aggregated exports and record them. See
  [Fixed on 2026-10-21](#fixed-on-2026-10-21).
- **2026-10-21, second batch fixed and recorded.** The 15 Low Fix rows are fixed (`f9ee72a`, `8840941`,
  `54f4a3b`). The 24 Record rows are in [Parity-Diffs](../Parity-Diffs.md#screen-and-web-review-2026-10-20), with the
  follow-up corrections to `Parity-Mapping.md`. The Decide rows were [decided](#decided-on-2026-10-21) the same day.
- **2026-10-21, first decisions.** P3-01 follows the failure-tolerance rule; Print goes to `/fop`; `.md` files stay
  unblocked; a bad `show-total` is logged.
- **2026-10-21, the last eight decided.** P7-11, P8-05 and P8-08 are fixed; P7-09, P3-05 and P6-04 are recorded. P9-02
  and P9-03 are built: the lifecycle actions run, and `servlet`, `filter` and `listener` are scripts whose value is a
  handler object with the Servlet API's methods ([Web-Scripts](../../docs/Web-Scripts.md)). The search proxies are now
  `ProxyServlet.js` on `/search` and `/dashboard`. See [Decided on 2026-10-21](#decided-on-2026-10-21). The Decide list
  is empty.

The review asks **"is the feature there?"**, not "is this line the same?". The port replaces
FreeMarker macros with JavaScript modules, Groovy with Rust and the servlet API with axum, so the
output does not have to match byte for byte. Severity: **High** = wrong result, data loss,
authorization gap, crash or a security-relevant divergence. **Med** = a gap a user of a shipped
screen hits. **Low** = an edge case, cosmetic or message text. Rust paths are relative to `crates/`
unless they start with `runtime/`. Java paths are under `../moqui/`. Functions are named rather than
line numbers, because the line numbers drift.

## Summary

| Disposition | Rows |
|---|---|
| [Fixed on 2026-10-21](#fixed-on-2026-10-21) | 20: P8-20, P4-05, P2-04, P7-02 (a); then P7-03 with P9-13, P8-06, P8-04, P4-09, P3-06, P9-01, P9-11, P8-09, P8-12, P8-17, P9-07, P7-10, P6-03, P8-11; then P3-01 (decided) |
| [Decided on 2026-10-21](#decided-on-2026-10-21) | 8: P8-05, P9-02, P9-03, P7-11, P7-09, P8-08, P3-05, P6-04 |
| [Recorded in Parity-Diffs](#recorded-in-parity-diffs) | 24: P1-05, P1-07, P1-08, P2-05, P2-09, P2-10, P2-11, P2-15, P3-02, P4-03, P6-01, P6-02, P6-06, P7-02 (b), P7-04, P7-07, P8-13, P8-14, P8-18, P8-19, P9-06, P9-12, P9-14, P9-15 |
| [Dropped](#dropped-on-the-third-pass) | 14: P1-10, P1-11, P2-12, P2-13, P3-04, P4-06, P4-07, P4-10, P6-05, P7-05, P7-06, P8-10, P8-15, P8-16 |

No finding is open.

## Fixed on 2026-10-21

First batch (`d457a30`):

- **P8-20.** `EditDbView.xml` `setMasterFields` reads `ec.web.parameters` once and indexes it
  (`params["field_" + i]`), as other port screens do. The facade stays a plain object.
  `runtime/base-component/tools/test/DataView.spec.js`, the first tools spec, renders the screen,
  posts the save and checks the alias, member and key-map rows (`./moq test tools`).
- **P4-05.** Both qvt Saved Finds dialogs show the active find, "(My Default)", and either its
  schedule ("Scheduled for PDF ...") or Java's `_SCHED` form. That form has Mode and Schedule
  drop-downs and a `ScheduleFind` button with `data-confirm`, not Java's inline `onclick`.
  `sri.getFormListFindScreenScheduled` is the port of `ScreenForm.getFormListFindScreenScheduled`.
  The static package carries `formListFindScheduled`. `schedule_find` now refuses a mode other
  than `xlsx`, `csv` or `xsl-fo`, which Java would store (recorded).
- **P2-04.** `form_rows` passes the shown fields into `Aggregation::apply`. These are
  `list_column_fields`, a port of the macros' `formListColumns` with `_uiType`, minus a field whose
  `hide` holds, plus the hidden fields with a `default-field` for an `entity-find` list. A
  `group-by` field outside that set is not in the group key. With none shown, every row is one
  group, as in Java.
- **P7-02 (a).** `UrlInstance::permitted` checks the single service of a transition that is not
  read-only as `AT_SERVICE`, with Java's anonymous allowances, before the transition frame. Its
  frames are `Frames::untracked()`. Before, each permission check of a link counted artifact hits,
  including a hit for a transition that never ran.

Second batch (`f9ee72a` conf and host, `8840941` request parsing, `54f4a3b` macros):

- **P7-03 with P9-13.** `ConfCache` keeps each webapp's `response-header` entries, and
  `ExecutionContextFactory::response_headers` reads them by type, with the shipped entries for a conf that declares
  none. `web::apply_conf_headers` applies them as Java's `WebappInfo.addHeaders`. Uses: `screen-render` as every
  response's baseline (now with `X-XSS-Protection`), `screen-secure` on https, `cors-preflight` and `cors-actual`,
  `screen-resource-text` and `-binary` for a file under a screen (24 hours; a versioned or `.min.` file keeps a year,
  recorded), `screen-server-static`, and `web-resource-inline`. A conf override, such as the CSP the browser gate
  sets, now takes effect.
- **P8-06.** `handle-cors`, `allow-origins`, `require-session-token` and `upload-executable-allow` read the `webroot`
  webapp's attribute first and the property after, the websocket origin check included.
- **P9-01.** A browser error page is the conf's `error-screen` for the error type (`ConfCache` keys
  `error-screen <error>`), with the shipped paths as the fallback.
- **P9-07.** `context::Error` carries `retry_after`, set by `check_tarpit`. `json_error_of`, `send_json_error_of` and
  the error page add `Retry-After`.
- **P9-11.** `upstream_path` strips `/kibana` as it strips `/elastic`, as Jetty does; the module doc is corrected.
- **P7-10.** `entity.json` and `master.json` are `application/schema+json` and `service.swagger` is
  `application/json`, the document alone with no `messages` merged in (inline, recorded).
- **P8-04.** A lone non-breaking space is a null parameter, the name kept. A list that holds one keeps it; Java
  nulls each multipart element too, an edge case left as it is.
- **P8-09.** A text response sent while the message facade has an error is the errors, one per line, with a 500,
  and a text response with no file name is `Content-Disposition: inline`, as Java's.
- **P8-12.** Only a text type (`is_text_content_type`) is `_requestBodyText`, and only
  `application/x-www-form-urlencoded` is parsed as a form; a binary or untyped body is no parameter.
- **P8-17.** `WebRequest.raw_query` is the query as sent. The request URL is the host without a port of 80 or
  443, the path and that query, so a decoded line break no longer reaches `request_details`.
- **P8-11.** Repeated `Cookie` headers join with `"; "` in `header_map` and the socket cookie read.
- **P4-09.** The qvt summary shows a `display` header field by its value, or its `dynamic-transition` label, and a
  `display-entity` by its entity text. It does not use `@text`, as Java's does not, so Find Invoice's constant
  status text still does not show; Find Payment's payment method does.
- **P3-06.** The html `display` span carries `sri.getFieldValueClass`, and `moqui-html.css` aligns `BigDecimal`
  right and the other numbers centre, as Java's `webroot-layout.css`. `display-entity` has no class, as in Java.
- **P6-03.** An untitled xlsx column is headed by `StringUtilities.camelCaseToPretty` at render time. Only the xlsx
  header uses it in Java; the other modes keep the split on capitals that their FTL has.

Decided and done:

- **P3-01.** An element no macro names no longer fails the render. The walker logs it ("No screen macro in html,
  element for X") and calls the module's `@element` macro, as FreeMarker's `@element` does: html and qvt write Java's
  `<p>=== Doing nothing for element X, not yet implemented. ===</p>`, xml writes it as a comment, and the other
  modes write nothing. The element's children are not rendered, as Java's macro does not recurse; the rest of the
  screen is. XML actions keep the strict error. `moqui lint --screens` fails on such an element, using the html
  module's own macro names, so a typo is still caught before it ships (no shipped screen has one).

## Decided on 2026-10-21

Each row's decision and what was done. What stays different from Java is in
[Parity-Diffs](../Parity-Diffs.md#screen-and-web-review-2026-10-20).

- **P8-05.** Add the 401. `serve_at` answers a request whose `Origin` CORS refuses with 401 "Origin not allowed"
  before it runs (`web::refused_origin`), as `MoquiServlet.handleCors` does, the script servlets included. Behind a
  proxy that rewrites `Host`, the request's own site is also a believed proxy's `X-Forwarded-Host` or the conf webapp's
  host, so a browser's own POST is not refused. Test: `a_disallowed_origin_is_refused_before_the_request_runs`.
- **P9-02.** Implement the lifecycle actions as Java runs them, with the same script tools as services
  (`WebappScripts` in `web/servlet.rs`):
  - `before-request` after the sign in;
  - `after-request` however the request ended, and before the session work ends;
  - `after-login` and `before-logout` for a web sign in and sign out;
  - `after-startup` and `before-shutdown` with the server (`moqui-host/src/lib.rs`);
  - `first-hit-in-visit` when a request starts a visit, which Java never calls (recorded).

  A `before-request` failure fails the request; a failure of the others is logged.
- **P9-03.** `H2Console` is removed. `servlet`, `filter` and `listener` are now scripts in configurable locations.
  - **Handler objects.** Each script's value is a handler object that borrows the Servlet API: `doGet`, `service`,
    `doFilter(request, response, chain)`, `contextInitialized`, `sessionCreated`, `requestDestroyed` and the rest.
  - **Request and response.** `ec.web.request` and `ec.web.response` are subsets of `HttpServletRequest` and
    `HttpServletResponse` (`script/runtime/web_host.rs`).
  - **Mapping.** URL patterns map as the Servlet spec has it. The Java classes the port has natively can still be
    named by class.
  - **The search proxies.** `/elastic` is `/search` and `/kibana` is `/dashboard`.
    - Each is a conf servlet on `webroot/script/ProxyServlet.js` behind `webroot/script/AuthFilter.js`.
    - The permissions are `SearchRemote` and `DashboardRemote`.
    - The bodies stream through: `ec.service.rest()...send()` and `response.forward` (`web/upstream.rs`).
    - `moqui-host/src/search_proxy.rs` is gone.
  - **Conf.** The LLM and A2A servlets left the conf.
  - **Kept native.** Other request handling was reviewed for a move to scripts, and these stay native:
    - `/health` and `/metrics`, a gate that must not depend on scripts;
    - the sockets and `/git`, async protocols the synchronous script engine cannot serve;
    - the request log, which runs on every request.

  Guide: [Web-Scripts](../../docs/Web-Scripts.md).
- **P7-11.** A `get` or `any` transition is now preferred over the first one declared. With no method,
  `ScreenDef::transition` takes the `any` transition, then the `get` one, then the first. Recorded.
- **P7-09.** `skip-actions` is left out of the conf XSD. Recorded.
- **P8-08.** The configured value is used.
  - `getWebappRootUrl` reads Java's arguments. It answers empty for a path-only link in a request, and otherwise the
    render's base URL or the conf webapp's URL, never the request's `Host`.
  - An email's link base is the template's `webHostName`, then the new `EmailServer.webBaseUrl`, then the conf
    webapp.
  - `send#ContactListEmail` and the unsubscribe services follow the same order, and store emails keep
    `ProductStore.storeDomain`.

  Recorded.
- **P3-05.** An export keeps the full columns. Recorded.
- **P6-04.** The page number stays. Recorded.

## Recorded in Parity-Diffs

The rows the review found deliberate, or a harmless superset, are in
[Parity-Diffs](../Parity-Diffs.md#screen-and-web-review-2026-10-20) with their reasons, recorded 2026-10-21: P1-05,
P1-07, P1-08, P2-05, P2-09, P2-10, P2-11, P2-15, P3-02 with P4-03, P6-01, P6-02, P6-06, P7-02 (b), P7-04, P7-07,
P8-13, P8-14, P8-18, P8-19, P9-06, P9-12, P9-14 and P9-15, and the second batch's two kept differences (P7-03's
versioned files, P7-10's inline schema). Do not revert them. Decided 2026-10-21: the Print item goes straight to
`/fop/<path>` for a real PDF, with no page-layout dialog (P3-02, P4-03), and `.md` files under a screen stay unblocked,
since screens read them directly (P7-04). A bad `show-total` is now logged as a bad `aggregate` is (P2-05).

## Dropped on the third pass

| ID | Why |
|---|---|
| P1-10 | `formDisplayOnly` is not used by any screen in either tree or in any component |
| P1-11 | No shipped XML uses `include-in-menu`, `disable-when` or a `wizard`/`stack` menu. The port evaluating `disable-when` is a superset: Java parses it and never calls it. An XSD tidy-up at most |
| P2-12 | Both sides merge the saved parameters into the next request, so the list is not "paged again". The values are missing only on a refresh or a bookmark |
| P2-13 | 403 against 500 for `allow-web-request="false"`, and no shipped screen sets it |
| P3-04 | **Wrong.** Java's export modes take their columns from `getAllColInfo()`, which already drops a field whose `hide` expression is true (`isListFieldHiddenAttr`). Both sides drop it |
| P4-06 | `focus-field` on a form-list. All 68 shipped uses are on form-single, which works |
| P4-07 | **Wrong.** Quasar 2.35.1's QCheckbox emits `(value, evt)`, and `clickCheckbox` gets the click with `shiftKey`. Shift-click range selection works |
| P4-10 | `container-panel` part ids. Nothing references a suffixed id |
| P6-05 | **Wrong.** Java's `formSingleWidget` returns before the table for a `hidden` widget, as the port does. The leftover case (`hide="false"` with a hidden widget) is not used |
| P7-05 | **Wrong.** Java does not reload a screen on the next request. `screen.location` never expires in the default conf and is `expire-time-idle="10"` in dev. The port's 2-second check is faster, and both never reload in production |
| P7-06 | A tree request with no `treeNodeId`. Every client sends `#`, and Java's branch is marked "shouldn't happen" |
| P8-10 | `ec.web.parameters` being a copy rather than the live stack. No script reads a session value through it or writes it. The real bug next to it is P8-20 |
| P8-15 | The artifact info's `toString` format. The shipped `error/Forbidden.xml` reads only the name, action and type |
| P8-16 | Java's `encodeHtml` maps seven C0 bytes (Windows-1252 smart punctuation that lost its high bit) to text. A C0 byte cannot break out of markup, so there is no XSS angle |

---

## Phase 1 — Screen definition, form model and saved finds

Compared `screen/definition.rs`, `screen/form.rs` and `screen/saved_finds.rs` (with the parts of
`screen/render.rs`, `screen/sri/` and `screen/generate.rs` they drive) against
`ScreenDefinition.groovy`, `ScreenSection.groovy`, `ScreenWidgets.groovy` and `ScreenForm.groovy`,
and the two `xml-screen-3.xsd` files.

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P1-05 | `TransitionDef::implicit` (`screen/definition.rs`), `runs_actions` (`screen/render.rs`) | `FormSavedFindsTransitionItem`, `FormSelectColumnsTransitionItem` (`ScreenDefinition.groovy`) | The implicit `formSaveFind` and `formSelectColumns` take only a POST with a session token, and `insecure_transition` also refuses query or `~` parameters and plain http when https is on. In Java they are `method="any"`, need no token, and skip every request check. Every shipped macro POSTs | Low | Recorded |
| P1-07 | `schedule_find` → `template_literal` (`screen/saved_finds.rs`) | `ScreenForm.groovy` saved-find schedule, `ScreenServices.xml` `ec.resource.expand` | The scheduled email subject escapes `${` in the find's description. Java expands it, which is server-side template execution of user text | Low | Recorded |
| P1-08 | `schedule_find`, `make_default` (`screen/saved_finds.rs`) | `ScreenForm.groovy` `ScheduleFind`, `MakeDefault` | Both refuse a find id the user has no user or group link to. Java only checks that the find exists. A find shared with one of the user's groups works in both | Low | Recorded |

## Phase 2 — Screen render and the SRI surface

Compared `screen/render.rs`, `screen/sri/`, `screen/aggregate.rs` and `screen/mod.rs` against
`ScreenRenderImpl.groovy`, `AggregationUtil.java`, `ScreenForm.groovy` (form-list render info) and
`WebFacadeImpl.groovy` (screen history and saved parameters).

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P2-04 | `Aggregation::apply` (`screen/aggregate.rs`), `render_user_columns_form` | `AggregationUtil.processAggregateOriginal`, `ScreenForm` `FormListRenderInfo` (`includeFields`) | The group key uses every `group-by` field. Java leaves out a field that is not displayed (the user's columns minus `hide` fields). `InvoiceAgingDetail.xml` rolls up by party in Java when the Invoice column is removed, and stays one row per invoice in the port | Med | Fixed 2026-10-21 |
| P2-05 | `Aggregation::of` (`screen/aggregate.rs`) | `ScreenForm.groovy` `AggregateFunction.valueOf`, `AggregationUtil.java` constructor | A bad `aggregate` is logged and ignored, and a bad `show-total` is dropped silently. Java throws and the screen fails to load. Every shipped value is valid | Low | Recorded |
| P2-09 | `record_history` (`screen/render.rs`) | `WebFacadeImpl.saveScreenHistory` | The port also leaves out ChangePassword, sso and error screens, and it records only a GET that answers 200 and is not `qjs`/`qvue`. Java skips standalone screens, transitions and `history="false"`. The Java Login and SecondFactor screens already say `history="false"` | Low | Recorded |
| P2-10 | `make_render` (`screen/render.rs`) | `WebFacadeStub.groovy` | A render through the `make_render`/ScreenTest seam skips the session-token check. Java's stub reports `isSecure()=true` and no query string, so only the token check and the login redirect apply there. Port JS specs POST transitions with no token | Low | Recorded |
| P2-11 | `respond` (`screen/render.rs`) | `ScreenRenderImpl.saveMessagesToSession`, `getBasicResponseMap` | A JSON transition answer returns its messages in `messageInfos` and does not save them to the session. Java does both, so a later full page shows them again. The comment in `respond` gives the reason | Low | Recorded |
| P2-15 | `form_rows` (`screen/sri/forms.rs`), called for every render mode | `DefaultScreenMacros.{csv,text,xml,xsl-fo}.ftl` `getListObject(false)` | An aggregated form-list is aggregated in every mode. Java aggregates only in html and qvt, so its exports have the raw rows | Low | Recorded |

## Phase 3 — Macro libraries: html, csv, text, xml, plain

Compared `ScreenSupport.js`, `ScreenHtml.js`, `ScreenCsv.js`, `ScreenText.js`, `ScreenXml.js` and
`ScreenPlain.js` against `DefaultScreenMacros.{html,csv,text,xml,plain}.ftl`.

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P3-01 | `script/walker.rs` (the "no screen macro" error), `guarded` (`screen/sri/sections.rs`) | `@element` fallback in each `DefaultScreenMacros.*.ftl` | An element no mode names is an error. Inside a subscreen or section, `guarded` puts the error text in place of that screen's body. In the root screen the request fails. Java writes a placeholder and renders the rest. No shipped screen uses an unknown element. `screen/tests/modules.rs` locks in the current rule | Low | Fixed 2026-10-21 (decided) |
| P3-02 | `ScreenHtml.js` (`show-pdf-button`) | `DefaultScreenMacros.html.ftl` PDF dialog | A Print link straight to `/fop/<path>?pageNoLimit=true` (the PDF; before 2026-10-21 it was `html-print`). Java opens a "Generate PDF" dialog with `layoutMaster` and `saveFilename` that posts to `/fop`. The one shipped use is `ArtifactHitSummary.xml` | Low | Recorded (decided: no dialog) |
| P3-05 | `S.formListColumns(node)` in `ScreenCsv.js`, `ScreenText.js`, `ScreenXml.js`, `ScreenXlsx.js`, `ScreenHtmlPrint.js` | `ScreenForm.makeFormListColumnInfo` | The export modes always use the untyped `columns`. Java picks the `columns type=` that matches `_uiType`. Only html, qvt, qvt-static and tui read `_uiType` | Low | Recorded (decided 2026-10-21) |
| P3-06 | `ScreenHtml.js` `display`, `display-entity` | `DefaultScreenMacros.html.ftl` `display` (`sri.getFieldValueClass`) | No value-type class on the display span. Java's `webroot-layout.css` right-aligns `td .BigDecimal` and `td .currency`, and the port's html list cells do not. The html display also drops Java's `text-center`/`text-right` align classes, because the cell takes the `align` | Low | Fixed 2026-10-21 |

## Phase 4 — The qvt render mode

Compared `ScreenQvt.js` (and `WebrootVue.qvt.js` where the contract turned on it) against
`DefaultScreenMacros.qvt.ftl`.

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P4-03 | `ScreenQvt.js` export menu | `DefaultScreenMacros.qvt.ftl` PDF dialog | Same as P3-02 in qvt, server-static qvt and tui: a Print item straight to `/fop/<path>`, not a dialog that posts to it. `?layoutMaster=` still selects a page on the URL | Low | Recorded (decided: no dialog) |
| P4-05 | Saved Finds dialog in `ScreenQvt.js` and `ScreenQvtStatic.js` | `DefaultScreenMacros.qvt.ftl` active-find block | No Mode/Schedule drop-downs and no Schedule button. Nothing else in the UI posts `ScheduleFind`, so no user can create a scheduled find. My Account and BOSS only edit existing ones | Med | Fixed 2026-10-21 |
| P4-09 | `valueText` in `ScreenQvt.js` | `DefaultScreenMacros.qvt.ftl` `widgetTextValue` | `display` and `display-entity` are in the no-display list. Java took them out on purpose (an FTL comment says so). Java shows the field's value (not `@text`) and a display-entity's entity text, so Find Payment's `eitherPaymentMethodId` was missing; Find Invoice's constant status text has no value in either | Low | Fixed 2026-10-21 |

## Phase 5 — Server-static qvt

Every finding was fixed on 2026-10-20 (P5-01 was a High), except P5-04, which
`QvtServerStatic.md` already records. Nothing in `runtime/` is
`server-static="qvt"`, and the flag stays opt-in. Separate from this review: Gallery Board,
Gallery Calendar and Service Load Runner say `render-modes="vue,qvue"`, and Gallery QuickTask says
`qvue` only. The port has no `vue` mode, so an html or qvt request for these four 404s, where Java
would render them (Parity-Diffs P8b-09 records the check itself). Fix the screen XML the next time
those files are touched.

## Phase 6 — Print and spreadsheet

Compared `ScreenXlsx.js`, `screen/xlsx.rs`, `ScreenHtmlPrint.js` and `screen/pdf.rs` against
moqui-poi's `FormListExcelRender.groovy` and `ScreenWidgetRenderExcel.groovy`, and
`DefaultScreenMacros.xsl-fo.ftl` with `Header.xsl-fo.ftl`.

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P6-01 | `cellCode` → `S.fieldChain` (`ScreenXlsx.js`) | `FormListExcelRender.renderSheet` | The xlsx cell uses the `conditional-field` whose condition holds, as the csv mode does. Java reads only `default-field` and throws a null-pointer exception when there is none (Find Task's `ownerPartyId`). The module header says so | Low | Recorded |
| P6-02 | container and section macros (`ScreenXlsx.js`) | `ScreenWidgetRenderExcel.renderSubNodes` | The port makes a sheet for a form-list inside `container`, `container-panel`, `container-split`, `section-iterate` and `dynamic-form`. Java walks only form-list, section, container-box and container-row. All 45 shipped `show-xlsx-button` lists are on paths Java reaches | Low | Recorded |
| P6-03 | `S.titleText` (`ScreenSupport.js`) | `StringUtilities.camelCaseToPretty` | A field with no title gets a header split on capitals only. Java also splits on digits, `.` and `#`, drops repeated parts, and writes `id` as "ID" (`address1` → "Address 1"). Java does not localize the header. Only the xlsx header uses `camelCaseToPretty` in Java; every other mode splits on capitals as the port does | Low | Fixed 2026-10-21 |
| P6-04 | `documentCode` `@page` rule (`ScreenHtmlPrint.js`) | `Header.xsl-fo.ftl` (regions commented out) | Every printed page gets `@bottom-center "- N -"`, and `@top-center` too when `documentTitle` is set. Java's default FOP document has neither. That covers `/fop`, `print#ScreenDocument` and scheduled finds | Low | Recorded (decided 2026-10-21) |
| P6-06 | `canonical_render_mode` (`screen/cache.rs`), the port's `MoquiDefaultConf.xml` `xsl-fo` mode | `MoquiDefaultConf.xml` `xsl-fo` (`text/xml`) | A direct `.xsl-fo` request answers `text/html`. Java answers FO XML. Every shipped `renderMode=xsl-fo` goes through `/fop` or a PDF | Low | Recorded |

## Phase 7 — Screen URLs, tree, menu, static files, facade and cache

Compared `screen/url.rs`, `screen/urls.rs`, `screen/tree.rs`, `screen/menu.rs`,
`screen/static_files.rs`, `screen/cache.rs` and `screen/schema.rs` against `ScreenUrlInfo.groovy`,
`ScreenTree.groovy`, `ScreenFacadeImpl.groovy`, `ScreenRenderImpl.groovy` (file branch) and
`RestSchemaUtil.groovy`.

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P7-02 | `UrlInstance::permitted` (`screen/urls.rs`) | `ScreenUrlInfo.isPermitted` | (a) There is no `AT_SERVICE` check of a non-read-only transition's single service, so a user without the service permission sees an enabled link that fails when clicked. (b) The check follows the transition run, which is looser than `isPermitted`. It also skips the last screen's own view check when the path ends on a transition. Not a bypass: the request path matches Java's run | Low-Med | Fixed (a) 2026-10-21, Recorded (b) |
| P7-03 | `static_files.rs` (`LONG_CACHE`, `REVALIDATE_CACHE`), `finish_response`, `apply_cors`, `send_resource_response`; `merge_response_headers` (`context/conf.rs`) | `WebappInfo.addHeaders` from `ScreenRenderImpl` and `WebFacadeImpl` | The conf's `<response-header>` entries (`screen-resource-*`, `web-resource-inline`, `screen-render`, `cors-*`) are merged and never applied. The port's conf now ships the full list. File `Cache-Control` is 1 hour, or 1 year `immutable` for a versioned file, where the conf says 24 hours | Low | Fixed 2026-10-21 |
| P7-04 | `BLOCKED_EXTENSIONS` (`screen/static_files.rs`), file branch (`screen/render.rs`) | `ScreenUrlInfo` file lookup, `ScreenRenderImpl` file branch | Non-screen `.xml`, `.properties`, `.sql`, `.groovy`, `.ftl` and `.html.fancy` files are refused with a 404. Java serves the plain ones and renders the templates through `screen-resource-template`. A `.md` under a screen without `include-child-content` is sent raw, where Java renders it. `screen/tests/files.rs` locks in the list | Low | Recorded (decided: `.md` stays unblocked) |
| P7-07 | `?` tail via `web::form_pairs` (`screen/url.rs`) | `ScreenUrlInfo` `split("=")` | A pair whose value holds `=` or is empty is kept. Java keeps only exactly-two-part pairs. No shipped `url=` has either | Low | Recorded |
| P7-09 | `TextOutput` (`screen/cache.rs`) | `ScreenSection.renderSingle`, `ScreenRenderImpl` (required parameters) | `skip-actions` is not read. No conf sets it. The port's `moqui-conf-3.xsd` still declares it | Low | Recorded (removed from the XSD 2026-10-21) |
| P7-10 | `entity_json` → `send_json_response` (`screen/schema.rs`) | `RestSchemaUtil` `sendTextResponse(..., "application/schema+json", "<name>.schema.json")` | The schema is `application/json` with the facade's `messages` merged in, and it has no attachment name. Only the curl examples in `rest.xml` use it | Low | Fixed 2026-10-21 |
| P7-11 | `UrlInstance` transition lookup (`screen/urls.rs`), `ScreenDef::transition` (`screen/definition.rs`) | `UrlInstance.getTargetTransition` → `getTransitionItem(name, method)` | The port looks up with no method and gets the first transition declared under that name. Java passes the current request method (or `""` outside a request) | Low | Fixed 2026-10-21 (decided), recorded |

## Phase 8 — The web facade

Compared `web/mod.rs`, `web/multipart.rs`, `web/error_info.rs` and the `ec.web` script facade
(`script/runtime/facades.rs`) against `WebFacadeImpl.groovy`, `WebUtilities.java` and
`MoquiServlet.groovy`.

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P8-04 | `pairs_to_parameters` (`web/mod.rs`), `end_part` (`web/multipart.rs`) | `WebUtilities.simplifyRequestParameters`, `WebFacadeImpl.addValueToMultipartParameterMap` | A single non-breaking space is Java's "no value" and becomes null. The port keeps the text. The html remote-option list posts it for an empty option, and qvt does not | Low | Fixed 2026-10-21 |
| P8-05 | `finish_response` (`web/mod.rs`) | `MoquiServlet.handleCors` | A request from a disallowed origin runs, without CORS headers. Java answers 401 "Origin not allowed" and does not run it. `SameSite=Lax` and the session-token check already stop a browser write | Low | Fixed 2026-10-21 (decided) |
| P8-06 | `require_session_token`, `handle_cors`, `allow_origins`, `upload_executable_allowed` (`web/mod.rs`) | `WebappInfo` from the `<webapp>` attributes | These four read `get_prop_or_env("webapp_*")`. The stock attributes point at the same properties, so a default deployment matches, but a conf override of the attribute is ignored | Low | Fixed 2026-10-21 |
| P8-08 | `get_webapp_root_url` (`script/runtime/facades.rs`) | `WebFacadeImpl.getWebappRootUrl`, `makeWebappRootUrl` | The arguments are ignored and the answer is `web::base_url()`, which is empty on a real request. Java answers `""` for the plain form and scheme plus host for the full form | Low | Fixed 2026-10-21 (decided), recorded |
| P8-09 | `send_text_response_with` (`web/mod.rs`) | `WebFacadeImpl.sendTextResponseInternal` | With an error in the message facade, Java sends the errors with a 500. The port sends the text with a 200 | Low | Fixed 2026-10-21 |
| P8-11 | `header_map` (`moqui-host/src/http.rs`), `cookie_value` (`web/mod.rs`) | the servlet's `getHeader`, `getCookies` | A repeated header is joined with `", "`. Two `Cookie` headers make the session cookie unreadable, which logs the user out. The listener is HTTP/1.1 only, and proxies merge cookies with `"; "`. `notws.rs` reads only the first value | Low | Fixed 2026-10-21 |
| P8-12 | `parse_body` (`web/mod.rs`) | `WebFacadeImpl` constructor, `ResourceReference.isTextContentType` | Every non-form body becomes `_requestBodyText` (lossy UTF-8), binary included. A body with no content type that contains `=` is parsed as a form. Java reads only a text content type | Low | Fixed 2026-10-21 |
| P8-13 | `parse_body` (`web/mod.rs`) | `WebFacadeImpl` (`contains("application/json")`, `"text/json"`) | `*+json` is parsed into parameters (`merge-patch+json`, `vnd.api+json`). Java's `contains` matches `json-patch+json` but not those two. A harmless superset | Low | Recorded |
| P8-14 | `base_name`, `MultipartReader` (`web/multipart.rs`) | `WebFacadeImpl` `FileItem.getName()`, `getContentType()` | The upload name loses its directories, and a part with no type is `application/octet-stream`. Java keeps the client's name and gives a null type. A code comment says why | Low | Recorded |
| P8-17 | `full_url` (`web/mod.rs`) | `WebFacadeImpl.getRequestUrl` | The port uses the raw `Host` header with its `:port` and re-joins the decoded query pairs without encoding them. Java uses the server name, drops `:80`/`:443` and appends the raw query string. Visible in `ArtifactHit.requestUrl`, `Visit.initialRequest` and `request_details` | Low | Fixed 2026-10-21 |
| P8-18 | `send_resource_response` (`web/mod.rs`) | `WebFacadeImpl.sendResourceResponseInternal` (`web-resource-inline` headers) | Only an inline `component://` or `classpath://` resource gets the public cache header. Anything else is `private, no-store`. A code comment says why | Low | Recorded |
| P8-19 | `strip_credential_parameters` (`web/mod.rs`, also `screen/statics.rs`) | `WebUtilities.stripCredentialParameters` | The credential names are stripped at every depth of a JSON value. Java strips only the top level | Low | Recorded |
| P8-20 | `EditDbView.xml` save actions, `parameters` getter (`script/runtime/facades.rs`) | `WebFacadeImpl.getParameters` (a `ContextStack`, a `Map`) | **New on the third pass.** The screen calls `ec.web.parameters.get(...)`. The port's getter returns a plain JS object, so `get` is undefined and saving a DB view's fields throws. Untested | Med-High | Fixed 2026-10-21 |

## Phase 9 — The HTTP host and the web app endpoints

Compared `moqui-host/src/` (router, `http.rs`, `search_proxy.rs`, `logging.rs`), `web/error_info.rs`,
`screen/mod.rs` and `context/conf.rs` against `MoquiServlet.groovy`, `MoquiContextListener.groovy`,
`MoquiAuthFilter.groovy`, `log4j2.xml` and the webapp element of `MoquiDefaultConf.xml`.

| ID | Port | Java | Divergence | Sev | Disposition |
|---|---|---|---|---|---|
| P9-01 | `error_screen` (`web/error_info.rs`) | `MoquiServlet.sendErrorResponse` → `getErrorScreenNode` | A hardcoded table picks the error screen. The conf's five `<error-screen>` entries have the same paths and are never read. `confcache` keeps only the first entry's attributes | Low | Fixed 2026-10-21 |
| P9-02 | lifecycle elements merged in `context/conf.rs` | `ExecutionContextImpl`, `UserFacadeImpl`, `MoquiContextListener` | `before-request`, `after-request`, `after-login`, `before-logout`, `after-startup`, `before-shutdown` and `first-hit-in-visit` are merged and never run. The shipped examples are in a comment | Low | Fixed 2026-10-21 (decided) |
| P9-03 | fixed routes (`moqui-host/src/http.rs`), conf merge (`context/conf.rs`) | `MoquiContextListener` servlet and filter setup | `<endpoint>` (`endpoint_enabled`: `/notws`, `/termws`) and `<session-config>` are read. `<servlet>`, `<filter>` and `<listener>` are merged and inert, including the proxies' init-params. The port's conf ships them with Java class names, and `MoquiDevConf.xml` adds an `H2Console` servlet | Low | Fixed 2026-10-21 (decided) |
| P9-06 | `serve_at` (`screen/mod.rs`) | `MoquiServlet.service`, `handleCors` | Every `OPTIONS` answers 204 before the path is resolved. Java renders the screen, or answers 200 for an allowed preflight. Browsers accept the 204 | Low | Recorded |
| P9-07 | `check_tarpit` (`user/authz.rs`), `ErrorKind::Tarpit` | `MoquiServlet.service`, `MoquiFopServlet` | The 429 has the seconds in its text but no `Retry-After` header. Reachable through the `WebrootSecurityDemoData` ALL_SCREENS tarpit | Low | Fixed 2026-10-21 |
| P9-11 | `proxy` (`moqui-host/src/search_proxy.rs`) | Jetty `ProxyServlet$Transparent` with `prefix=/kibana` | The port forwards `/kibana/app/...`. Jetty strips the prefix. The module doc's "as the servlets' prefix settings do" is wrong | Low | Fixed 2026-10-21 |
| P9-12 | `logging.rs` | `log4j2.xml` `ErrorFile` | The second file is `moqui-error.log` at WARN and above. Java's is `error.log` at ERROR (`ThresholdFilter`). The port conf's comment documents the name | Low | Recorded |
| P9-13 | `finish_response` (`web/mod.rs`) | `MoquiDefaultConf.xml` `screen-render` response-header | `X-XSS-Protection: 1; mode=block` is listed in the port's conf and not sent | Low | Fixed 2026-10-21 |
| P9-14 | `forward` (`moqui-host/src/search_proxy.rs`, now `SKIP` in `ProxyServlet.js`) | Jetty `ProxyServlet` | `Authorization` and `Cookie` are not forwarded. Jetty forwards both, and with them the session cookie | Low | Recorded |
| P9-15 | `search_proxy.rs` 401 (now `AuthFilter.js`) | `MoquiAuthFilter` `sendError(401, ...)` | The proxy 401 carries `WWW-Authenticate: Basic realm="Moqui"` and "login required". Java sends no challenge and the message-facade text | Low | Recorded |

## What this review does not cover

- The **screen XML dialect**: the port's `xml-screen-3.xsd` is its own, and the two XSDs were
  compared only for the elements the port's code reads.
- **Excluded on purpose** in [Parity-Mapping](../Parity-Mapping.md): the FreeMarker engine and macro
  libraries, the `js` and `vuet` render modes, `/vapps`, Groovy expression evaluation, JSON-RPC
  and the servlet context (`getServletContext`). `getRequest` and `getResponse` are subsets since 2026-10-21 (P9-03).
- The **client library** (`runtime/base-component/webroot/screen/webroot/js/`), the Quasar widgets
  and the CSS, except where a finding turned on the server-client contract.
- The **screen services** (`ScreenServices.xml`, `WikiServices.xml`), which the entity and service
  review covered, and the **Mantle and IronStone application screens**.

## Next steps

None. Every row under [Decide](#decided-on-2026-10-21) was decided and made on 2026-10-21, and this file was archived
the same day, with a pointer in [Remaining-Work](../Remaining-Work.md#parity-gap-review), as
[EntityService-Parity-Review](EntityService-Parity-Review.md) was.

---

## Appendix: checked and found equivalent

What each phase compared and found to match, kept as the record of what was compared, so the
same ground is not reviewed twice. This is first-pass text. The third pass corrected only the
bullets that touch an open or dropped row (screen history, `screen-last`, messages, aggregation,
log files, `ec.web.parameters`). The line numbers are from 2026-10-19 and have drifted.

### Phase 1

- **The screen model** (`screen/definition.rs`): the element vocabulary the port recognizes —
  `parameter`, `transition`, `transition-include`, `subscreens` with `subscreens-item` and
  `conditional-default`, `section`, `section-iterate`, `section-include`, `form-single`,
  `form-list`, `tree`, `macro-template`, `web-settings`, `always-actions`, `pre-actions`,
  `widgets`/`fail-widgets` — matches Java's `ScreenDefinition` constructor
  (`ScreenDefinition.groovy:196-341`) and `ScreenSection`/`ScreenWidgets`
  (`ScreenSection.groovy:41-72`, `ScreenWidgets.groovy:28-40`); the port also registers the
  sections and forms inside a `fail-widgets`, which Java does not (a superset).
- **`extends` (the screen-extend merge)**: the attributes are copied over the base screen, and
  `parameter`, `transition` and `transition-include` are merged by name, `subscreens` by
  `subscreens-item` name, `actions-extend` before/after/replace and `widgets-extend` before/after
  by name or id (`screen/definition.rs:1061-1130`), as Java does
  (`ScreenDefinition.groovy:120-192`). The port finds the extend file by the path after the
  scheme and by the path after `/screen/`, as Java does
  (`ScreenDefinition.groovy:91-118`).
- **`transition-include`**: the transition is copied from the other screen and runs under the
  including screen (`screen/definition.rs:836-852`), as Java does
  (`ScreenDefinition.groovy:232-239`); a transition that cannot be found fails the screen in
  both.
- **`section-include`**: the sections, forms and trees inside the included section come along, a
  nested include is pulled the same way, and a name or location with `${` is resolved when the
  screen renders (`screen/definition.rs:1175-1259`, `screen/sri/sections.rs:132-153`), as Java's
  `pullSectionInclude` does (`ScreenDefinition.groovy:343-391`).
- **The sections of an extended form**: Java copies the sections of the form a form `extends` into
  the extending screen (`ScreenForm.groovy:119-129`); the port resolves them at render time from
  the screen the extended form is in (`screen/sri/sections.rs:106-125`, `:155-170`), so a form that
  extends a form of another screen renders its sections in both.
- **The transition lookup**: a method-specific transition wins over an `any` one, and a name that
  matches a transition name as a whole regular expression is matched for a request with a method
  (`screen/definition.rs:692-719`), as Java's `getTransitionItem` does
  (`ScreenDefinition.groovy:503-517`).
- **The authorization action of a transition**: the `authz-action` attribute, else the action of
  the transition's single service (the service's own, else by its verb), else `view` for a
  read-only transition and `update` otherwise (`screen/definition.rs:372-377` and
  `screen/render.rs:1147-1153`), as Java's `resolveAuthzAction` does
  (`ScreenDefinition.groovy:891-907`).
- **`read-only`, `begin-transaction` and `require-session-token`**: a transition with no actions
  and no service-call is read-only, `require-session-token` defaults to the webapp default
  (`true`), as Java's `TransitionItem` does (`ScreenDefinition.groovy:821-867`).
- **The responses**: `none`, `url`, `screen-last`, `screen-last-noparam`, the `parameter` children,
  the `parameter-map`, `save-current-screen` and `save-parameters`
  (`screen/definition.rs:176-234`, `:378-390`), as Java's `ResponseItem`
  (`ScreenDefinition.groovy:1195-1253`); the conditional responses are all checked and the last one
  that holds wins, and the error response wins over them (`screen/render.rs:1289-1308`), as Java
  does (`ScreenDefinition.groovy:1010-1021`).
- **The parameters**: a screen's and a transition's `parameter` elements are evaluated (`from` as an
  expression, `value` as an expansion, else the context, else the web parameter) and put into the
  context, and the path parameters are bound to the extra path names
  (`screen/render.rs:1310-1332`), as Java's `setAllParameters` does
  (`ScreenDefinition.groovy:911-941`).
- **The conditions**: the `condition` attribute is an expression and the `condition` element's
  children are an action condition (`screen/definition.rs:82-116`, `:526-534`), as Java's
  `ScreenSection`/`TransitionItem` read the attribute and the first child of the element
  (`ScreenSection.groovy:45-53`, `ScreenDefinition.groovy:848-852`).
- **The automatic transitions**: `actions`, `formSelectColumns`, `formSaveFind` and `screenDoc` are
  added when the screen has none of that name, and a screen's own transition of the name wins
  (`screen/definition.rs:854-864`), as Java does (`ScreenDefinition.groovy:241-245`).
- **The subscreens**: the directory of the screen holds its subscreens, the `subscreens-item`
  elements and the `moqui.screen.SubscreensItem` rows add and replace them in that order, and the
  conf `screen-facade` entries come before the rows (`screen/definition.rs:1284-1425`), as Java's
  `populateSubscreens` does (`ScreenDefinition.groovy:393-454`); the menu order is by index (no
  index first) then by the upper-case title (`screen/definition.rs:461-477`), as Java's
  `SubscreensItemComparator` does (`ScreenDefinition.groovy:1326-1343`); a user-group item is only
  for the members of the group (`screen/definition.rs:435-441`), as Java's `isValidInCurrentContext`
  does (`ScreenDefinition.groovy:1317-1323`).
- **A placeholder screen is server static**: a screen whose widgets are only the subscreens
  placeholders is server static in every render mode (`screen/definition.rs:1162-1172`), as Java
  does (`ScreenDefinition.groovy:329-337`).
- **`hasTabMenu`**: a tab (and, in the port, a wizard or a stack) panel or menu makes the screen
  report a tab menu; the wizard and stack are the port's own, and Parity-Diffs records them
  ("screen menu data", Layout panels section).
- **The form model** (`screen/form.rs`): `extends` of a form of the same screen, of another screen
  or of a `DbForm` (`screen/form.rs:599-621`), as Java's `initForm` does
  (`ScreenForm.groovy:99-136`); the merge of the `entity-find`, `row-actions`, `row-selection`,
  `hidden-parameters`, `field-layout`, `form-list-column` and `columns` of an overriding form
  (`screen/form.rs:638-700`), as Java's `mergeFormNodes` does (`ScreenForm.groovy:1084-1135`); the
  merge of a field of the same name and of the `conditional-field` by its condition
  (`screen/form.rs:702-727`), as Java's `mergeFieldNode` does (`ScreenForm.groovy:1137-1158`); the
  hidden fields a `field-layout` does not reference are added to it
  (`screen/form.rs:578-591`), as Java does (`ScreenForm.groovy:221-229`).
- **The auto fields**: `auto-fields-entity` with `include` of `all`, `pk` or `nonpk`, the
  `default-display` of a view alias deciding a form-list field, the `pq-expression` alias with
  `default-display="true"` as a display field, and `auto-fields-service` with the service's in and
  out parameters (`screen/form.rs:979-1030`, `:1189-1216`), as Java's `addEntityFields` and
  `addServiceFields` do (`ScreenForm.groovy:643-698`, `:618-641`); the `auto-columns` grouping of
  the displayed fields (id and date first, then numbers, short text and long text, about six to a
  column) (`screen/form.rs:1325-1399`), as Java's `addAutoEntityColumns` does
  (`ScreenForm.groovy:700-768`).
- **The auto widgets**: `auto-widget-entity` and `auto-widget-service` are expanded into the
  widget of the field, and a `widget-template-include` is replaced by the template's children
  (`screen/form.rs:737-846`), as Java's `expandFieldNode`/`expandFieldSubNode` do
  (`ScreenForm.groovy:988-1082`).
- **The `DbForm` forms**: the form is built from the `DbForm`, its `DbFormField`, the field
  attributes, the plain and the entity options with their conditions and orders, and the layout
  sequence (`screen/form.rs:320-447`), as Java's `getDbFormNode` does
  (`ScreenForm.groovy:358-452`); the forms of the user's groups are found through the
  `DbFormLookup` view (`screen/form.rs:255-293`), as Java does (`ScreenForm.groovy:73-80`,
  `:334-356`); a form with such an extension is assembled for each render
  (`screen/generate.rs:276-284`).
- **The validation of a field**: a field that is one of the in-parameters of the service the form's
  transition calls gets `validate-service`, or `validate-entity` for an implicit entity-auto
  service (`screen/form.rs:1152-1186`), as Java's `setSubFieldValidateAttrs` does
  (`ScreenForm.groovy:246-276`).
- **The saved finds** (`screen/saved_finds.rs`): the request parameters a saved find stands for
  (`makeFormListFindParameters`, `screen/saved_finds.rs:126-164` vs `ScreenForm.groovy:2288-2317`);
  the finds of the user and of the user's groups with their description, parameters and
  `isByUserId` (`screen/saved_finds.rs:240-302` vs `ScreenForm.groovy:298-323`); the user's default
  find of a screen (`screen/saved_finds.rs:304-318` vs `ScreenForm.groovy:324-332`); the delete of
  a find with its fields once no user and no group has it (`screen/saved_finds.rs:506-545` vs
  `ScreenForm.groovy:2359-2387`); the save of a find, of a group's find copied for the user, of
  its `FormConfig` and of its fields (`screen/saved_finds.rs:666-806` vs
  `ScreenForm.groovy:2475-2558`); the fields a find records (`makeFormListFindFields`,
  `screen/saved_finds.rs:343-406` vs `ScreenForm.groovy:1862-1930`); the user's columns of a
  form-list and their reset (`screen/saved_finds.rs:878-1088` vs `ScreenForm.groovy:1721-1860`,
  `:2561-2682`).
- **The screen documents**: the list of a screen's documents in the user's locale with the title of
  the file when there is none (`screen/saved_finds.rs:1090-1128` vs
  `ScreenDefinition.groovy:731-760`), and the document of a `docIndex` rendered as a template
  (`screen/saved_finds.rs:1130-1161` vs `ScreenDefinition.groovy:1164-1193`).
- **Java's field validation helpers** (`getFieldValidationClasses`,
  `getFieldValidationJsRules`, `getFieldValidationRegexpInfo`,
  `ScreenForm.groovy:1509-1644`) are not used anywhere in the Java tree, not even in the macros,
  so their absence in the port is not a gap.


### Phase 2

- **The sri surface, method by method.** Java's `sri` is the `ScreenRenderImpl` itself (`internalRender` puts it in the context
  stack, `ScreenRenderImpl.groovy:314`), so every public method of that class is a method a macro may call. The port builds the
  same object in `script/runtime/screen_host.rs:992-1068` and `script/runtime/urls.rs:274-284`, and the port's own macro modules
  use the same names (`runtime/base-component/framework/template/screen/*.js`). Compared name by name:
  - the render control — `doBoundaryComments`, `getActiveScreenDef`, `getScreenUrlInfo`, `getScreenUrlInstance`,
    `getCurrentScreenUrl`, `getRenderMode`, `pushContext`, `popContext`, `setInContext`, `renderSection`,
    `renderSectionInclude`, `renderSubscreen`, `renderIncludeScreen`, `renderText`, `renderDynamicForm`,
    `renderUserColumnsForm`, `appendToAfterScreenWriter`, `getAfterScreenWriterText`, `appendToScriptWriter`,
    `getScriptWriterText`, `encodeHtml` — all present with the same meaning (`sri/mod.rs`, `sri/sections.rs`,
    `script/runtime/screen_host.rs`).
  - the field values — `getFieldValue`, `getFieldValueString` (both overloads), `getFieldValuePlainString`,
    `getFieldValueClass`, `getNamedValuePlain`, `getFieldEntityValue`, `getFieldOptions`, `getFieldTransitionValue`,
    `getContextValue` (through `node_parameters`) — present (`sri/fields.rs`, `sri/options.rs`); the widget overload of
    `getFieldValueString` reads the widget's `format` and its `text-format` in the `text` and `csv` modes as Java does
    (`ScreenRenderImpl.groovy:1540-1555`), and the three-argument overload takes the field node as Java's does.
  - the forms — `getFormNode`, `getFormInstance`, `getFormHiddenParameters`, `getUserFormListFinds`,
    `getUserDefaultFormListFindId`, `pushSingleFormMapContext`, `getSingleFormMap`, `openFormList`, `startFormListRow`,
    `endFormListRow`, `startFormListSubRow`, `endFormListSubRow`, `safeCloseList`, `formListColumnCharWidths` — present
    (`sri/forms.rs`, `sri/layout.rs`).
  - the URLs — `buildUrl`, `buildUrlInfo`, `buildUrlFromTarget`, `makeUrlByType`, `isAnchorLink`, `activeInCurrentMenu` — present
    (`screen/urls.rs`).
  - the theme and the menu — `getThemeValues`, `getThemeIconClass`, `getMenuData` — present (`sri/theme.rs`,
    `screen/menu.rs`); `getCurrentThemeId` is not on the port's `sri` object, but the Java macros do not call it either (they
    call `getThemeValues`, which the port has with the same one- and two-argument shape).
  - the five that Java's qvt macro calls and the port's `sri` does not have — `getFormFieldValues`,
    `getFormListHeaderValues`, `getFormListRowValues`, `makeFormListSingleMap`, `makeFormListMultiMap` — are done by the port's
    qvt macro instead (`ScreenQvt.js:386-414`, `singleValues` / `headerValues` / `rowValues`), with the same values: the
    pass-through parameters and `moquiFormName` and `lastUpdatedStamp` of a form-single, the hidden parameters and `orderByField`
    and `formListFindId` and `pageSize` of a form-list header, the URL parameters and the row values and `pageIndex` of a row.
    The same holds for `transformFormListRow` and `addFormFieldValue`, which the port's macro does in `addValue`
    (`ScreenQvt.js:286-360`) with the same per-widget values Java's `addFormFieldValue` puts out
    (`ScreenRenderImpl.groovy:1793-2003`). A macro written by hand against Java's `sri` still cannot call those five, though.
  - the ones Java has that no macro calls and the port does not need — `getWriter`, `getOutputStream`, `getRootScreenDef`,
    `getNextScreenDef`, `getActiveScreenPathName`, `getNextScreenPathName`, `getActiveScreenPath`,
    `getScreenPathHasTransition`, `isInCurrentScreenPath`, `getTemplate`, `getScreenWidgetRender`,
    `getSectionIncludeName`, `getBaseLinkUri`, `getDefaultText`, `addNodeParameters`, `buildUrlInfo`, `sendHttpRedirect`,
    `sendJsonRedirect`, `getBasicResponseMap` — the servlet types are gone with the servlet, and the rest are the internals of the
    path and the URL (`screen/url.rs`, `screen/urls.rs`). `getBaseLinkUri` is the one with a known gap: the port keeps
    `baseLinkUrl` on the render (`screen/render.rs:255-258`) and nothing reads it, which is the open item in
    [Remaining-Work](../Remaining-Work.md) ("`sri.baseLinkUrl(url)` is kept on the render but nothing reads it"), so this phase
    confirms it rather than reports it.
- **The render mode.** Java takes the mode from the builder, then the `renderMode` request parameter, then the extension of the
  target screen, and falls back to `html` (`ScreenRenderImpl.groovy:416-431`); the port does the same in that order
  (`screen/render.rs:708-729`), and both map `xsl-fo` to the print mode. The content type and the character encoding come from
  the `web-settings` of the screens in the render path, else from the mode (`ScreenRenderImpl.groovy:1005-1008`, `:423-431`); the
  port does the same (`screen/render.rs:733-748`), with `require-encryption` defaulting to true when there is no `web-settings`
  element, as Java's `checkWebappSettings` treats it (`screen/definition.rs:810-812`).
- **The always-actions and pre-actions.** Java runs the `always-actions` of every screen in the path from the root, then the
  `pre-actions` from the render start (or from the third screen for a mode that is always standalone), in two separate passes with
  the artifact frames of the screens above the render start kept on the stack so the screens below inherit
  (`ScreenRenderImpl.groovy:842-940`); the port does the same in `render_screens` (`screen/render.rs:579-683`), including the
  always-standalone start index (`:628-633`) and the inherited frames (`:664-680`).
- **The required parameters of a screen.** Java checks the required parameters of the screens to render after the actions have
  run, adds the error "Required parameter missing (name)" and leaves the screen out of the render
  (`ScreenRenderImpl.groovy:945-959`); the port does the same (`screen/render.rs:650-661`) and the screen is skipped in
  `render_subscreen` (`sri/sections.rs:388-395`) as Java skips it in `renderSubscreen` (`ScreenRenderImpl.groovy:1189`).
- **The transition responses.** Java picks the `error-response` when there are errors, else the last `conditional-response` whose
  condition holds, else the `default-response` (`ScreenDefinition.groovy:1010-1021`); the port picks the same
  (`screen/render.rs:1291-1308`). A response of type `none` saves the parameters and the error parameters and sends nothing
  (`ScreenRenderImpl.groovy:529-535`); the port does the same (`screen/render.rs:1343-1351`). A `screen-last` response goes back
  to the saved screen, else to the newest history entry, else to the root (`ScreenRenderImpl.groovy:541-570`); the port does the
  same (`screen/render.rs:1365-1401`), with the same-origin check on a saved absolute URL. One difference: the port's
  `screen-last` carries every saved parameter, where Java keeps only the target's declared parameters when it declares any.
- **The transition parameters.** Java binds the extra path names to the `path-parameter` names and then the screen and transition
  `parameter` entries into the context (`ScreenDefinition.groovy:912-937`); the port does the same
  (`screen/render.rs:1310-1332`), and a missing required parameter of the transition is a 400 in the port
  (`screen/render.rs:1052-1056`) as it is a `BaseArtifactException` in Java (`ScreenDefinition.groovy:963-975`).
- **The authorization of the screens and the transition.** Java pushes a screen frame with the authz required for the target
  screen only when the screen requires authentication, and a transition frame with the authz action of the transition (the
  `authz-action`, else the single service's own, else view for a read-only transition)
  (`ScreenDefinition.groovy:894-896`, `:943-947`); the port does the same (`screen/render.rs:907-929`, `:1147-1154`).
  `anonymous-all` and `anonymous-view` are applied the same way (`ScreenRenderImpl.groovy:674-681`; the port at
  `screen/render.rs:918-929`).
- **The artifact hits.** Java counts a screen hit with the content type, and a transition hit named `screen#transition` with the
  response type (`ScreenRenderImpl.groovy:505-511`, `:664-670`); the port counts the same two (`screen/render.rs:1026-1049`,
  `sri/sections.rs:88-93`).
- **The login and the https redirects.** Java redirects to the last `login-path` of the path (or `/Login`) when a screen that
  requires authentication is asked for with no user, and to https when the webapp has https on
  (`ScreenRenderImpl.groovy:1010-1106`); the port does the same (`screen/render.rs:767-834`), with the 401 for a fragment or a
  transition in both.
- **The screen history.** Java stores every screen that is not standalone, has no `history="false"` and was not a fragment
  (`WebFacadeImpl.groovy:262-273`). The port's `record_history` is narrower: it also leaves out ChangePassword, sso and
  error screens, and records only a GET that answers 200 and is not `qjs`/`qvue` (P2-09).
- **The saved find.** Java reads the user's default saved find for the target screen, honours `_clear`, and adds the saved find's
  parameters when the request has none of its own (`ScreenRenderImpl.groovy:340-378`); the port does the same
  (`screen/saved_finds.rs:170-238`), with the `pageSize` preference handled in the same place.
- **The messages.** The messages, the errors and the validation errors ride the session to the next screen
  (`WebFacadeImpl.groovy:1343-1358`); the port saves them the same way (`screen/render.rs:1619-1646`), and the validation error
  of a field is shown by that field and not also as an error line in both. A JSON transition answer is the exception: the
  port returns the messages in the JSON only, where Java also saves them to the session (P2-11).
- **`WebUtilities.encodeHtml`.** The port's `encode_html` (`sri/mod.rs:157-170`) escapes the same five characters as Java's
  (`ScreenRenderImpl.groovy:1169`, `:1197`, `:1239`).
- **The aggregation.** The functions are the same seven (`min`, `max`, `sum`, `avg`, `count`, `first`, `last`; Java's
  `AggregateFunction` at `AggregationUtil.java:37`), `show-total="true"` means `sum` and `show-total="false"` means none in both
  (`ScreenForm.groovy:1394-1396`; the port at `screen/aggregate.rs:86-90`), a form with aggregate fields and no group-by field is
  an error in both (`ScreenForm.groovy:1425-1430`; the port at `screen/aggregate.rs:118-123`), the totals row is one more row at
  the end marked `_moquiRowType: "total"` in both (`AggregationUtil.java:145-150`; the port at `screen/aggregate.rs:244-248`), the
  sub-list of a group is the list of the original rows with the sub-list totals last in both
  (`AggregationUtil.java:134-144`; the port at `screen/aggregate.rs:234-243`), and `count`, `min`, `max`, `sum`, `first` and `last`
  behave the same over the rows of a group. The port's average includes every row; Java throws on the second row of an
  average. The group key is not the same (P2-04). The row actions and the `from` expressions of the
  fields run per row with the entry, the index and the has-next in the context under the same names in both
  (`AggregationUtil.java:175-198`; the port at `screen/sri/forms.rs:216-247`).
- **The form-list column widths.** `formListColumnCharWidths` reads the same `print-width` and `print-width-type` of each field
  of each column as Java's `getFormListColumnCharWidths` (`ScreenForm.groovy:2180-2234`), the fixed widths, the percent widths
  and the zero width (which hides the column and gives its separator back) come out the same, and so does the error when the
  fixed widths exceed the line — the port's own tests at `sri/mod.rs:251-294`.


### Phase 3

- **The macro list of `DefaultScreenMacros.html.ftl`**: every macro it defines has an equivalent in
  `ScreenHtml.js` — `screen`/`widgets`/`fail-widgets` (`:1973-1975`), `subscreens-menu`/
  `subscreens-panel` (`:1977-1978`), `section`/`section-iterate`/`section-include`
  (`ScreenSupport.js:240-241`, `ScreenHtml.js:1979`), `container`/`container-box`/`container-row`/
  `container-panel`/`container-dialog`/`dynamic-container`/`dynamic-dialog`/`button-menu`
  (`:1981-2001`), `tree` (`:2003`), `link` (`:2004`), `image`/`label`/`editable` (`:2005-2007`),
  `form-single`/`form-list` (`:2018-2019`), the field layout elements `field-ref`,
  `fields-not-referenced`, `field-row`, `field-row-big`, `field-group`, `field-accordion`,
  `field-col-row` (the `layoutCode` at `:1581-1637`), and every field widget `check`, `date-find`,
  `date-period`, `date-time`, `display`, `display-entity`, `drop-down`, `file`, `hidden`, `lookup`,
  `password`, `radio`, `range-find`, `reset`, `submit`, `text-area`, `text-find`, `text-line`
  (`:2028-2045`). The helper macros (`nodeId`, `fieldName`, `fieldId`, `fieldTitle`,
  `fieldIdByName`, `widgetTextValue`, `linkFormForm`, `linkFormLink`, `paginationHeader`,
  `paginationHeaderModals`, `formSingleSubField`, `formSingleWidget`, `formListHeaderField`,
  `formListSubField`, `formListWidget`) are the runtime functions of `ScreenHtml.js` (`nodeId`,
  `titleCode`, `fid`, `fnm`, `valueText`, `linkForm`, `linkElement`, `nav`, `findDialog`,
  `singleFieldCode`, `cellCode`, `columnField`).
- **The macro list of the other modes**: `DefaultScreenMacros.csv.ftl` maps to `ScreenCsv.js`
  (`form-list` at `:95`, `link` at `:68`, the option widgets at `:151-153`, `display` at `:156`,
  `text-area`/`text-line`/`text-find` at `:159-161`, `submit` at `:158`); `DefaultScreenMacros.text.ftl`
  maps to `ScreenText.js` (`form-single` at `:194`, `form-list` at `:184`, the option widgets at
  `:304-306`, `image`/`label` at `:266-267`); `DefaultScreenMacros.xml.ftl` maps to `ScreenXml.js`
  (`form-list` at `:69`, `link` at `:127`, the option widgets at `:135-137`, the `widgets` root
  element at `:96-107`); `DefaultScreenMacros.plain.ftl` maps to `ScreenPlain.js` (the section and
  subscreen calls at `:51-55`, the text and render-mode at `:43-50`). The elements Java leaves as
  empty macros (`file`, `hidden`, `ignored`, `password`, `range-find`, `reset`, `date-find`,
  `row-actions`, `parameter`, `subscreens-menu`) are no-ops in the port, as they are in Java.
- **The shared `.any.ftl` macros**: `include-screen`, `render-mode` and `text` are in
  `ScreenSupport.js:212-243` (`anyMacros`), and they choose the text node the same way Java does
  (the last `type` that matches the mode, else the one with no type or `any`), read the location or
  the inline text, and encode when `@encode="true"`. Parity-Mapping lists
  `DefaultScreenMacros.any.ftl` as excluded because the port has no separate module for it; the
  macros themselves are in `ScreenSupport.js`.
- **How the macros get the sri**: Java's data model is the `ContextStack` with `sri` and `ec` put
  into it (`ScreenRenderImpl.groovy:314`), and the macros call `sri.xxx` and `ec.getResource()`.
  The port installs `sri` and `ec` as globals of the script engine
  (`crates/moqui-core/src/script/runtime/screen_host.rs:992`, `sri_object`) and the generated code
  calls `sri.xxx(...)` and `ec.resource.…`, with the same methods for every widget
  (`getFieldOptions`, `getFieldValueString`, `getFieldValuePlainString`, `getFieldEntityValue`,
  `getFieldValidation`, `getFieldTransitionValue`, `getFormHiddenParameters`, `openFormList`,
  `startFormListRow`, `pushSingleFormMapContext`, `setInContext`, `makeUrlByType`, `buildUrl`,
  `getMenuItems`, `getThemeIconClass`).
- **How a widget's script is emitted**: Java writes an inline `<script>` per widget (jQuery validate,
  select2, datetimepicker, typeahead, jeditable, jstree, sortableLists) and moves the menus with
  `$("#…").append(…)`. The port writes no script at all: it writes the markup and a few `data-*`
  attributes and one shared script, `runtime/base-component/webroot/screen/webroot/js/moqui-html.js`,
  binds them (the tree at `:386`, the editable at `:423`, the remote options and the autocomplete
  select at `:262-335`, the dialogs at `:574`, the sortable columns at `:559`). The libraries a widget
  needs are added to the `footer_scripts`/`footer_stylesheets` sets of the context
  (`ScreenHtml.js:189-197`, set by `runtime/base-component/webroot/screen/webroot.xml:19-24`). This is
  the documented change of the port (no jQuery), not a gap in the widgets themselves.
- **How a section's code is generated and called**: Java keeps one FreeMarker template per render
  mode (`ScreenFacadeImpl.groovy:243-266` makes `<#include macro><#visit widgetsNode>`) and
  `ScreenWidgetRenderFtl.groovy:24-35` pushes a context frame, puts `sri` and `widgetsNode` in it
  and processes the template for one section's widgets. The port keeps the same shape in
  JavaScript: `screen/generate.rs:247-326` builds one function per (mode, macro location, section,
  widgets/fail-widgets) and caches it with the definition (`:199-225`), the mode module's `macros`
  table is the FreeMarker macro set (`screen/generate.rs:462-491`), the walker turns the element tree
  into the statement list the same way `<#visit>` does, and the unit is run with the nesting depth
  (`crates/moqui-core/src/script/runtime/screen_host.rs:60-100`) so a section inside a form-list cell
  gives the state of the unit around it back. A section's widgets are the same tree in both
  (`ScreenSection.groovy:44-58` against `screen/sri/sections.rs:155-202`), the condition and the
  actions of a section run before its widgets, and the fail-widgets are used when the condition does
  not hold.
- **The form map and the field chain**: `pushSingleFormMapContext`/`popContext` around a form and
  around each field (`ScreenHtml.js:1657-1668`, `:1682-1691`) are Java's
  `sri.pushSingleFormMapContext` (`DefaultScreenMacros.html.ftl:583`, `:786`/`:817`); the
  conditional-field chain with the default-field as the fallback is Java's `formSingleSubField`
  (`DefaultScreenMacros.html.ftl:742-753`) and `formListSubField` (`:1651-1663`), and the port's
  `fieldChain` (`ScreenSupport.js:125-145`) takes the first conditional-field whose condition holds,
  else the default-field; the `<set>` elements of a field run before its widgets in both
  (`ScreenHtml.js:1562`, `:1686`; Java `:788`).
- **The form-list rows, the hidden fields and the multi form**: the row forms outside the table
  with the hidden fields of each row, the header form, the first/second/last-row forms, the multi
  form with `_isMulti` and the final row of submit widgets are written the same way
  (`ScreenHtml.js:1143-1307` against `DefaultScreenMacros.html.ftl:1359-1602`), and the row actions
  and the field `from` expressions run per row (`screen/sri/forms.rs:205-257`, Java's
  `startFormListRow`).
- **The pagination**: the nav row with the first/previous/next/last links, the page numbers, the
  "go to page" form, the "show all" link, and the CSV/XLS/text/print buttons are all written
  (`ScreenHtml.js:925-983` against `DefaultScreenMacros.html.ftl:1162-1329`); the page text is the
  localized `PaginationTemplate` with the same variables (`curPageIndex`, `curPageMaxIndex`,
  `listName`, `context[listName + 'PageRangeLow']`), which the port expands with the same map.
- **The saved finds and the column dialog**: the find-options dialog with the saved finds, the
  "save new find" form, the "update to current", the delete and the default buttons, the order-by
  select, and the sortable column dialog that posts `columnsTree` to `formSelectColumns`
  (`ScreenHtml.js:987-1065`) are the port's version of Java's `paginationHeaderModals`
  (`DefaultScreenMacros.html.ftl:837-1161`); the port's `formLocation` is the form's name and the
  server makes the full location of it (`crates/moqui-core/src/screen/saved_finds.rs:322-341`),
  which is what Java's `getSavedFindFullLocation` names.
- **The option widgets**: a drop-down, a check and a radio show the text of the option that is the
  current value, with the `no-current-selected-key` as the fallback, in the html, csv, text and xml
  modes alike (`ScreenHtml.js:455-474`, `ScreenCsv.js:54-66`, `ScreenText.js:128-140`,
  `ScreenXml.js:33-44` against Java's `check`/`radio`/`drop-down` macros).
- **The text mode's column widths**: `sri.formListColumnCharWidths`
  (`crates/moqui-core/src/screen/sri/layout.rs:27-156`) is Java's
  `getFormListColumnCharWidths` (`ScreenForm.groovy:2180-2270`), step for step: the fixed widths
  win over the percent ones, the percent widths are scaled to 100 and the remainder is given out
  column by column, and a set of fixed widths that does not fit the line is an error. The port also
  gives the width back after a container-box or a form-single, where FreeMarker kept the last
  assigned width in a global (`ScreenText.js:18-20`).
- **The csv escaping and the rows**: a value with a comma, a quote or a line break is quoted and its
  quotes doubled (`ScreenCsv.js:31-37`, Java's `csvValue` at `DefaultScreenMacros.csv.ftl:16-21`).
  The port writes one row per line; FreeMarker wrote the newline only after some of the field
  macros, so a Java CSV row ran onto the next line. A label writes nothing in the port — that is the
  recorded difference in [Parity-Diffs, Gallery fixes (2026-10-18)](../Parity-Diffs.md#gallery-fixes-2026-10-18)
  ("csv render"), and the port names it in the header comment of `ScreenCsv.js:18-20`.
- **The xml mode**: the screen's name is the root element (`ScreenXml.js:96-107`, Java's `widgets`
  macro at `DefaultScreenMacros.xml.ftl:18-22`), a form-list is an element of `…Entry` elements
  whose attributes are the displayed fields, encoded with `encodeForXmlAttribute`
  (`ScreenXml.js:28-31`, `:59-90`).
- **The plain mode**: only the text of the screen's own `render-mode`/`text` elements is written, and
  the subscreen and section calls are kept so text inside a section still shows
  (`ScreenPlain.js:37-58` against `DefaultScreenMacros.plain.ftl:16-29`).
- **`lookup`**: Java's `lookup` macro is commented out (`DefaultScreenMacros.html.ftl:2164-2179`),
  so Java writes the "not yet implemented" placeholder for it; the port writes a text line
  (`ScreenHtml.js:2037`) and records the difference in the header comment of the file
  (`ScreenHtml.js:34`).
- **The `date-time` of the export modes**: Java's csv and xml macros format the value only when the
  widget has a `format` (`DefaultScreenMacros.csv.ftl:176-181`), while the port always formats it
  with the pattern or the type's default (`ScreenCsv.js:154-155`, `ScreenXml.js:138-139`); the
  difference is named in the header comments of `ScreenCsv.js:18` and `ScreenXml.js:19`.
- **The link's background attributes and the dialog's tooltip**: the port's link form takes
  `background-submit`, `background-reload-id`, `background-hide-id` and `background-message`
  (`ScreenHtml.js:604-610`) and the container-dialog's button takes the `tooltip`
  (`ScreenHtml.js:672-679`); Java's macros have neither — they are the recorded additions in
  [Parity-Diffs, Framework gaps from the IronStone screens (2026-10-19)](../Parity-Diffs.md#framework-gaps-from-the-ironstone-screens-2026-10-19).
- **The wizard and stack subscreens-panel**: Java writes "LATER … not yet supported"
  (`DefaultScreenMacros.html.ftl:125-128`); the port writes the step list and the Back/Next links
  (`ScreenHtml.js:782-804`), which is the recorded difference in
  [Parity-Diffs, Layout panels and the calendar and contact tools (2026-10-19)](../Parity-Diffs.md#layout-panels-and-the-calendar-and-contact-tools-2026-10-19).


### Phase 4

- **The component set.** Every one of the 80 `<#macro ` of `DefaultScreenMacros.qvt.ftl` has an
  equivalent in the port: the layout macros (`screen`, `widgets`, `fail-widgets`, `section`,
  `section-iterate`, `section-include`, `include-screen`), the containers (`container`, `container-box`
  and its `box-header`/`box-toolbar`/`box-body`/`box-body-nopad`, `container-row`/`row-col`,
  `container-panel` and its five parts, `container-dialog`, `dynamic-container`, `dynamic-dialog`), the
  standalone widgets (`link` with its `linkFormForm`/`linkFormLink` pair, `image`, `label`, `editable`,
  `button-menu`, `tree`, `parameter`), the forms (`form-single`, `field-ref`, `fields-not-referenced`,
  `field-row`, `field-row-big`, `field-group`, `field-accordion`, `field-col-row`), the list
  (`form-list`, `paginationHeader`, `formListSelectedRowCard`, `formListHeaderField`, the four
  `formListSub*` macros), the field widgets (`check`, `date-find`, `date-period`, `date-time`, `display`,
  `display-entity`, `drop-down`, `file`, `hidden`, `ignored`, `password`, `radio`, `range-find`, `reset`,
  `submit`, `text-area`, `text-line`, `text-find`, `lookup`) and the helpers (`nodeId`, `fieldName`,
  `fieldId`, `fieldIdByName`, `fieldTitle`, `widgetTextValue`, `getQuasarColor`, `getMomentDateFormat`).
  The port has no widget Java cannot draw; the elements Java's qvt FTL does not define at all
  (`dashboard`, `context`, `number-card`, `chatter`, `record-files`, `html-view`, `markdown-view`,
  `status-bar`, `container-split`) are the port's own additions and are in
  [Parity-Diffs, Framework gaps from the IronStone screens].
- **The shared macros.** `include-screen`, `render-mode`, `text`, `render-dynamic-form`,
  `form-list-user-columns`, `section`, `section-iterate` and `subscreens-*` come from the port's
  `ScreenSupport.js` `anyMacros` and `ScreenQvt.js`'s own table, and match the FTL's
  `DefaultScreenMacros.any.ftl` and the qvt FTL's `subscreens-*` macros (the wizard and stack panel types
  are the recorded redesign in Parity-Diffs).
- **The `m-*` component props.** Each component the macros write declares the props the macro sets:
  `m-form` (`action`, `method`, `read-only`, `focus-field`, `body-parameter-names`, `submit-message`,
  `submit-reload-id`, `submit-hide-id`, `exclude-empty-fields`, `parent-checkbox-set`, `fields-initial`,
  `validation-errors-initial`, `WebrootVue.qvt.js:1644`), `m-form-link`, `m-form-paginate`
  (`paginate`, `:1885`), `m-form-go-page` (`id-val`, `max-index`), `m-form-column-config`
  (`columns-initial`, `find-parameters`, `form-location`), `m-text-line` (`type`, `prefix`, `size`,
  `maxlength`, `mask`, `fill-mask`, `reverse-fill-mask`, `rules`, `default-url`, `default-parameters`,
  `depends-on`, `:2511`), `m-drop-down` (`options`, `options-url`, `value-field`, `label-field`,
  `server-search`, `server-delay`, `server-min-length`, `multiple`, `allow-empty`, `combo`,
  `required-manual-select`, `submit-on-select`, `show-not` slot, `:2374`), `m-date-time` (`type`,
  `format`, `auto-year`, `minute-step`, `rules`), `m-date-period` (`allow-empty`, `from-thru-type`,
  `fields`), `m-display` (`display`, `value-url`, `depends-on`, `label-wrapper`), `m-check`/`m-radio`
  (`options`, `no-wrapper`, `true-value`, `false-value`, `single-label`, `toggle`), `m-file`, `m-text-area`
  (`editor-type`, `rows`, `cols`, `readonly`, `autogrow`, `mention-url`), `m-container-box` (`type`,
  `title`, `initial-open`; the port omits the attribute when the box is not closed and the component
  default is `true`, which is what Java's `:initial-open="true"` means), `m-container-dialog` (`color`,
  `width`, `button-text`, `button-icon`, `button-class`, `title`), `m-editable` (`label-type`,
  `label-value`, `url`, `url-parameters`, `parameter-name`, `widget-type`, `load-url`, `load-parameters`),
  `m-tree-top` (`items`, `open-path`, `parameters`), `m-markdown`, `m-html-view`, `m-status-bar`,
  `m-number-card`, `m-list-views` and `m-subscreens-*`.
- **The field binding.** Java writes `v-model="${fieldsJsName}.${name}"` and `:fields` for a widget in a
  form and a plain `value` outside one; the port's `bind`/`bindOnly`
  (`ScreenQvt.js:242-244`) do the same, and the value the model holds is the one the `fields-initial`
  of the form seeds.
- **The `fields-initial` values.** The port's `addValue` (`ScreenQvt.js:293-360`) names the same values
  as Java's `ScreenRenderImpl.addFormFieldValue`: the plain value, the `_display` value of a display and
  a display-entity, the `_op`/`_not` of a drop-down and a text-find, the `_from`/`_thru` of a date-find
  and a range-find, the `_poffset`/`_period`/`_pdate` of a date-period, the `_isMulti` flag of a multi
  list, `moquiFormName`, `lastUpdatedStamp` and the pass-through parameters. (The data these carry is
  Phase 5's subject.)
- **The list structure.** The row forms, the multi form with its final submit row, the first, second and
  last rows with their own forms and maps, the sub-rows of an aggregated list, the row-selection
  check boxes with their `checkbox-parameter`, `checkbox-list-mode` and `checkbox-values`, the header
  form and the header dialog, the column layout (`columns` with a `type`, `form-list-column`, the
  per-field columns) and the `columns type` choice by `_uiType` all follow
  `DefaultScreenMacros.qvt.ftl:1113-1478` and Java's `ScreenForm` column rules.
- **The nav row.** The find-options dialog with the order-by drop-down and the "Clear Parameters" button,
  the column-config dialog with the "Do Not Display" column, the saved-finds dialog, the page-size menu
  with the sizes 10/20/50/100/200/500, the "go to page" control, the `m-form-paginate` control, the
  "No results found" and "Find Options required to view results" messages and the reset-to-default
  summary all follow `DefaultScreenMacros.qvt.ftl:668-1078`. The nav row is a `div` above the table and
  not a `tr` (the port's own documented choice in the module comment).
- **The export and text dialogs.** The CSV and XLS items (`renderMode=csv|xlsx`, `pageNoLimit=true`,
  `lastStandalone=true`, `saveFilename`) and the fixed-width text dialog (the line characters, page
  lines, line wrap and file name fields) follow `DefaultScreenMacros.qvt.ftl:924-976`; the XLS item is
  written only when `ec.screen.isRenderModeValid("xlsx")` in both.
- **The links and buttons.** The link text (an entity value, a `text-map` expansion, an image, an icon
  from the theme), the `link-type` variants (anchor, anchor-button, hidden-form-link), the URL forms (a
  screen path, a transition path, a full URL), `url-noparam`, `target-window`, `dynamic-load-id`,
  `pass-through-parameters`, `badge`, `tooltip`, the confirmation (a `data-confirm` attribute the shell
  answers instead of Java's inline `onclick`), the disabled link and the `m-form`/`m-form-link` choice
  follow `DefaultScreenMacros.qvt.ftl:220-345`. A link in a field is written to the after-screen writer
  (`sri.appendToAfterScreenWriter`) in both.
- **The dates and the find widgets.** The default date patterns (`HH:mm`, `yyyy-MM-dd`,
  `yyyy-MM-dd HH:mm`), the Java-to-moment pattern change (`a`→`A`, `d`→`D`, `y`→`Y`), the from/thru
  pair of a date-find, the period widget with its offset and period fields, the range-find pair, the
  text-find with its operator, not and ignore-case options, and the drop-down with its dynamic options,
  its `value-field`/`label-field`, its `depends-on` map and its current-value label follow
  `DefaultScreenMacros.qvt.ftl:1656-1701, 1859-1920, 1975-1994, 2114-2151`.
- **The containers.** The `container` with its type and style, the `container-box` with its header,
  toolbar, body and no-pad body, the `container-row` with its column sizes (the `col-md-12`/`col-sm-12`/
  `col-xs-12` fallbacks), the `container-panel` with its left/right widths and its center class, the
  `container-dialog` with its condition, its title, its button and the `_openDialog` flag, the
  `dynamic-container` with its `_dynamic_container_id`, the `dynamic-dialog` and its disabled button, and
  the `tree` with its items URL and parameters follow `DefaultScreenMacros.qvt.ftl:88-215`.
- **The `m-editable` extras.** The port writes the widget when the text is empty and
  `display-if-empty="true"` is set (with the "Click to edit" placeholder), which Java's qvt macro does not
  do at all, and it supports the `select` and `date` widget types. That is a superset of Java and is
  not a finding.
- **The `text-line` with `ac-transition`.** Java writes an error message ("text-line with @ac-transition
  is not supported", `DefaultScreenMacros.qvt.ftl:2064`); the port writes a server-search drop-down over
  the same transition (`ScreenQvt.js:434-445`). The port's is a superset, recorded in the module comment
  of `ScreenQvt.js`; see the open question below.


### Phase 5

- **The data split itself**: `GET {screen}/actions` answers the screen package and
  `GET {screen}/actions/{formName}` the form-list package, and the two are separate requests with
  `Cache-Control: private, no-store` (`statics.rs:450-461, 802-828, 844-1035`). This matches
  `plans/archive/QvtServerStatic.md:19-20, 26-51`; the screen package drops the form-list's list
  and its six page keys (`statics.rs:818-823`, `PAGE_SUFFIXES` at `:50-57`), the credentials are
  stripped at any depth (`web::strip_credential_parameters`, `:825` and `:1032`), a decimal is a
  string, and a find or a file is left out of the package (`unwrap_context`, `:464-476`). The
  form request answers the list, the page keys, the header field values, the show-total row
  (`:923-926`), the first/second/last rows (`:938-960`), the saved finds and the user's columns
  (`:994-1013`) — the same set the plan lists.
- **The template holds no request data and needs no `with`**: the unit runs with no actions
  (`build_template`, `statics.rs:311-349`, `UnitRequest { section: "" }`), the response is a script
  that registers a render function under the screen path (`wrap_script`, `:269-274`), and the
  compile uses `prefixIdentifiers: true` so the page needs no `unsafe-eval`. The port's own test
  (`crates/moqui-core/src/screen/tests/statics.rs:76-112`) checks that the template and the script
  hold none of the request's values.
- **The `always-actions` / `pre-actions` double side effect**: the form request runs the path's
  actions again, so a write, a send or a message in those blocks happens twice. This is recorded in
  the plan (`QvtServerStatic.md:49`) *and* reported by the scan (`report.rs:366-415`, kinds
  `side-effect`), and the port's test asserts it
  (`tests/statics.rs:530-531`). The form request itself does not require the screen request to have
  run first (`run_actions_transition`, `statics.rs:1193-1231`), as the plan says.
- **A `section-iterate` over a form-list's list, and a label outside the list that reads its page
  keys**: both are reported (`report.rs:453-489`, kind `form-list-key`) and the port's test asserts
  both (`tests/statics.rs:533-534`).
- **The eligibility rules**: a dynamic form, a form that extends an expression, a form that takes
  its fields from an expression, a form that says `server-static` without qvt, `allow-extra-path`, a
  `render-modes` without qvt and a qvt macro module that is not the built-in one all keep the
  screen a server merge (`structure_reasons`, `statics.rs:92-179`; the report's reasons are the
  same list, `report.rs:328-337`), as the plan's "Eligibility" paragraph says.
- **`display-entity`, `entity-options`, `list-options` and `dynamic-options` without `server-search`
  are resolved on the server** and kept in the package as `{name}_display` and
  `mqOpt_{form}_{field}_{sub}_{widget}` (`statics.rs:657-799`, `option_key` at `:659-661`), and the
  template reads exactly those keys (`ScreenQvtStatic.js:377-395`, `:973`). The port's test checks
  both the extra map of a form-single and the option list (`tests/statics.rs:442-457`).
- **The `ec` the browser has, and only that**: the client host is `ec.l10n` (localize, format,
  formatCurrency, formatCurrencyNoSymbol), `ec.user` (the session snapshot and `getPreference`),
  `ec.factory.moquiVersion` and `ec.resource.expand`/`expandNoL10n` (`WebrootVue.qvt.js:633-699`),
  and the plan's table lists the same members (`QvtServerStatic.md:81-92`). The static AST walk
  flags every other `ec` member (`static_ast.rs:33-51`), and the port's test shows an
  `ec.entity.find` caught (`tests/statics.rs:523`).
- **The `oxc_parser` walk of the expressions the static macros write**: `record_ast`
  (`report.rs:209-222`) checks each expression the macros hand over (`ScreenQvtStatic.js:131-135`,
  `sri.staticCheck`) with `check_expression` (`static_ast.rs:71-93`) and names `ec`, `sri`,
  `context`, `decimal`, `utilities` and `server-global` uses. The port's test asserts the `ec`,
  `decimal` and `sri` kinds (`tests/statics.rs:523-525`). The walk's own limit — a method chosen at
  run time is not seen — is stated in the module doc (`static_ast.rs:15-18`) and in the plan
  (`QvtServerStatic.md:106`).
- **The full check's wrapped `ec`**: `WidgetCheck` (`report.rs:310-326`) wraps the execution context
  while widget expressions run, so each use of an area the browser does not have is recorded and
  the render goes on (`report.rs:576-604`); the port's test asserts the `wrapped-ec` kind and that
  the render itself did not fail (`tests/statics.rs:526-528`).
- **The Vue check**: `vue_check.rs` runs the vendored `compiler-dom` bundle in a QuickJS runtime of
  its own, with a memory limit, a stack limit and a 20-second deadline per compile
  (`:36-70`, `:121-161`), and the server compiles the static template with the same compiler and
  the same options it uses for the precompiled client components (`:205-236`). A template the Vue
  compiler rejects is a build failure, not a broken page: `template_for` keeps the failure with the
  definition (`statics.rs:283-301`) and `serve_template` logs it and leaves the screen a server
  merge (`:397-404`).
- **`disableLink` on a link**: the static mode writes the link and lets the server refuse the
  request, which the plan records (`QvtServerStatic.md:69`). The form-wide disabled state is that
  same decision: the template does not grey the form, and the server rejects the
  submit.
- **The tree**: a `tree` element's JSON is answered by the same `/actions/{name}` request
  (`statics.rs:846-855`), as the plan says (`QvtServerStatic.md:148`).


### Phase 6

- **The macro set of the XSL-FO library.** Every `<#macro` of `DefaultScreenMacros.xsl-fo.ftl` has a counterpart in `ScreenHtmlPrint.js`, and the macros Java leaves empty have no-ops in the port. Enumerated against the FTL: `screen`, `widgets`, `fail-widgets` (ScreenHtmlPrint.js:336-339); `subscreens-menu` (empty in Java, a no-op in the port), `subscreens-active` and `subscreens-panel` (`sri.renderSubscreen()`, from `ScreenSupport.js:238-239`); `section` and `section-iterate` (`sri.renderSection`, `ScreenSupport.js:240-241`); `section-include` (`sri.renderSectionInclude`, ScreenHtmlPrint.js:386 — the port passes the node's name, attributes and text, which is all the Rust side reads, `screen/sri/sections.rs:132-153`); `include-screen` (`ScreenSupport.js:216-217`); `container`, `container-box` (with `box-header`, `box-body`, `box-body-nopad`), `container-row`, `row-col`, `container-panel` (`panel-header`, `panel-left`, `panel-center`, `panel-right`, `panel-footer`) and `container-split` (ScreenHtmlPrint.js:341-385); `link` and its inner `linkFormLink` (ScreenHtmlPrint.js:145-168), `label` (:188-203), `image` (:205-217), `parameter` (empty in Java, a no-op in the port); `form-single` with `field-ref`, `fields-not-referenced`, `field-row`, `field-row-big`, `field-group`, `field-accordion` (ScreenHtmlPrint.js:224-299), `field`, `conditional-field`, `default-field`, `header-field`; `form-list` with `formListHeaderField`, `formListSubField` and `formListWidget` (ScreenHtmlPrint.js:320-332 and `ScreenSupport.js:125-145`); the field widgets `check`, `drop-down`, `radio` (ScreenHtmlPrint.js:131-142), `date-time` (:422-423), `display` (:424), `display-entity` (:425), `submit` (:426), `text-area`, `text-line`, `text-find` (:427-429); and the empty ones `date-find`, `date-period`, `file`, `hidden`, `ignored`, `password`, `range-find`, `reset`, `row-actions`, `set`.
- **The attributes of the widgets.** The port reads the same attributes as the FreeMarker macros: the `condition` of a link, a label and a status bar (Java's `ec.getResource().condition`, the port's `raw(condition)`); the `text-map` of a link and a label (`ec.resource.expand(text, "", textMap)` in both); `no-current-selected-key` on a check, a drop-down and a radio (Java's `.node["@no-current-selected-key"]!`, the port's `attr(node, "no-current-selected-key")`); `default-value` and `format`/`type` on a date-time (the port's `S.dateTimeFormat` gives the same `yyyy-MM-dd`, `HH:mm` and `yyyy-MM-dd HH:mm` defaults, `ScreenSupport.js:276-281`); `currency-unit-field` and `currency-hide-symbol` on a display (`S.displayValueExpr`, `ScreenSupport.js:248-264`); `align` on a form-list field (the port writes it as the cell's `text-align`, Java as `fo:block text-align`); `print-width` and `print-width-type` on a field (see the widths below); `entity-name`, `key-field-name`, `entity-key-name` and `use-cache` on a display-entity or a link with an entity (`screen/sri/fields.rs:209-258` follows Java's `getFieldEntityValue`: the default text when the field is empty or the entity is not found, the entity's map for a `text` attribute, the entity's default description field otherwise).
- **The page layout.** The ten page masters of `Header.xsl-fo.ftl:1-55` (letter, legal, tabloid, A4 and A3, portrait and landscape) with their page sizes and margins are the port's `$masters` (ScreenHtmlPrint.js:26-34); the line width for the form-list columns is Java's `layoutWidthIn * 72 / 6` (DefaultScreenMacros.xsl-fo.ftl:33-36) as the port's `$lineChars` (ScreenHtmlPrint.js:35); the column width algorithm is Java's `getFormListColumnCharWidths` (ScreenForm.groovy:2180+) in `screen/sri/layout.rs:27-156` — the fixed character widths, the percent widths, the `print-width="0"` column that takes no room and the rounding of the rest are the same. The `@page` size and margin the PDF renderer uses are the document's own (`preferCSSPageSize`, `moqui-host/src/pdf.rs:477`).
- **The form-list of the print document.** The columns come from the assembled form, as in Java: the macro module is generated from the assembled form node (`screen/generate.rs:262-300`), which is Java's `sri.getFormInstance(name)` and `getAllColInfo()`, and a form-list whose columns the user chose is rendered through `sri.renderUserColumnsForm` (`screen/sri/forms.rs:119-161`), as Java's `form-list-user-columns` does. The page range line ("low - high / count", Java's `context[listName + "PageRangeLow"]` etc., the port's `page-range` div, ScreenHtmlPrint.js:43-44), the header row that repeats on each page (`display: table-header-group`), the row that does not split (`break-inside: avoid`), the grey band on every other row (Java's `background-color="#EEEEEE"` on the even index, the port's `tbody tr:nth-child(odd)`, the same rows) and the cell padding of 2pt are the same. A list with no rows writes no table in both.
- **The PDF images.** `PdfRenderer::render` inlines every `<img>` whose source is a resource location as a `data:` URL (`screen/pdf.rs:96-251`), which is what Java's FOP resolver did through the resource facade (`FopToolFactory.groovy:97-123`, the `LocalResourceResolver`); an `http`/`https` source is not fetched and a source that is not a png, jpeg, gif, webp, bmp or svg is left as written — the recorded difference in Parity-Diffs, [Print fixes (2026-10-18)](../Parity-Diffs.md), "PDF images". The barcode is an inline SVG from `ec.resource.code128Svg` where Java had barcode4j's FOP extension; the difference in the quiet zone and the height is recorded in the same table, "barcodes".
- **The print pipeline around the document.** `/fop/<path>` renders the screen at that path as the print mode and sends it as a PDF, with `filename` or `saveFilename` making it a download (`screen/mod.rs:241-305`), as Java's `MoquiFopServlet` did with its FOP transform; the difference that the port makes only PDF is recorded in Parity-Diffs, "Print fixes (2026-10-18)", `/fop`, and P8a-17 for `print#ScreenDocument` (`service/tools.rs:179-221`, which renders `html-print` and hands it to `PdfRenderer`). The email attachment of a screen rendered in `xsl-fo` becomes a PDF (`service/mail/implementation.rs:926-991`, as Java's `sendEmailTemplate.groovy:305-313`), and the scheduled report of `run#ScreenScheduled` does the same, with `layoutMaster` set to `letter-landscape` and the full render path (`runtime/base-component/framework/service/org/moqui/impl/ScreenServices.xml:84-134`, the same lines as `../moqui/framework/service/org/moqui/impl/ScreenServices.xml:86-143`).
- **The xlsx sheet layout.** One sheet per form-list, named for the form (`ScreenXlsx.js:106-107`; Java's `wb.createSheet(formName)`, FormListExcelRender.groovy:60), a header row of the field titles, a row per list entry, a frozen header row (`set_freeze_panes(1, 0)` for Java's `createFreezePane(0, 1)`), the columns fitted to their text (`autofit` for Java's `autoSizeColumn`), and the row height left to the application (Java's `setHeight((short) -1)`) — `screen/xlsx.rs:150-206`.
- **The xlsx styles.** The header is bold, centred and filled with `GREY_40_PERCENT` (the port's `0x0096_9696`, `screen/xlsx.rs:37` and `:224-229`; Java's `IndexedColors.GREY_40_PERCENT`, FormListExcelRender.groovy:330-341); a string is left-aligned and wrapped, a number centred, a number of a field with `align="right"` right-aligned, a currency or a `.00` format right-aligned with the negatives in red, and the dates, times and stamps carry the `yyyy-MM-dd`, `HH:mm:ss` and `yyyy-MM-dd HH:mm` formats — the same number formats, the same currency format `#,##0.00_);[Red](#,##0.00)` and the same alignments as Java's `makeRow*Style` methods (FormListExcelRender.groovy:343-420).
- **The xlsx cell values.** One value keeps its type (a number is a number, a date is an Excel serial number with a date format, a time and a stamp likewise, a boolean and any other object are text), and several values are joined with line breaks and formatted with their widget's format (the port's `XlsxBook::cell`, `screen/xlsx.rs:103-126`; Java's two branches, FormListExcelRender.groovy:168-237). A value that is null is an empty cell, and the widgets Java ignores (`ignored`, `hidden`, `submit`, `image`, `date-find`, `file`, `password`, `range-find`, `reset`, FormListExcelRender.groovy:41-42) are the port's `IGNORED` list (ScreenXlsx.js:35). The port's cell never fails the whole export: a text over Excel's 32 767 characters is cut with an ellipsis (`screen/xlsx.rs:284-293`).
- **The xlsx response.** The mode is a binary mode with the spreadsheet's media type and is always standalone (`screen/cache.rs:137-152`), the file name comes from `saveFilename` as in Java's XLS link (`ScreenHtml.js:971-973`; Java's `DefaultScreenMacros.html.ftl:1277-1280`), and a screen with no form-list fails with "No sheets rendered" in both (`screen/xlsx.rs:150-153`; Java's `BaseArtifactException("No sheets rendered")`, ScreenWidgetRenderExcel.groovy:69-76). The port's test `a_form_list_in_xlsx_is_a_sheet_with_typed_cells` (`crates/moqui-core/src/screen/tests/widgets.rs:179-217`) checks the sheet name, the header row, the typed cells and the empty cell of a submit field.
- **The `xlsx` feature gate.** `screen/xlsx_off.rs` keeps the same call shape as `screen/xlsx.rs` (a sheet, a header, a cell, a row end, a cell count) and counts what the render asks for, and `to_bytes` reports that xlsx is not built into this host; the mode is still registered, so a request for it fails with that message where Java, without the moqui-poi component, has no xlsx mode at all and the render fails for the missing render mode.
- **The PDF engine's own behaviour.** One long-lived Chromium, one page per document, every request the page makes failed so the document reaches no network, no server and no session, the page size from the document's `@page` (`preferCSSPageSize`), the backgrounds printed (`printBackground`), the load and the fonts awaited before the print, `pdf_page_limit` pages at a time and `pdf_timeout` seconds for each (`moqui-host/src/pdf.rs:344-503`). The browser is found at startup, the reason PDF is unavailable is logged and reported to every caller (`moqui-host/src/pdf.rs:73-175`, `screen/pdf.rs:32-34`), and a browser that died is started again.


### Phase 7

- **The URL walk** (`moqui-core/src/screen/url.rs:210-382` against `screen/ScreenUrlInfo.groovy:449-787`): the
  path names, the render-mode extension (`pathNamePreDot`, `rmExtension`, the `always-standalone` switch), the
  no-sub-path wrapper (the wrapper's name joins the path and the walk continues under it), the transition branch
  (the name is taken off the extra path), the file branch, the `default-item` loop, the `conditional-default`
  elements, the `moqui.screen.SubscreensDefault` rows (the last row whose condition holds wins, the newest
  `defaultSeqId` first), `always-use-full-path`, the `begin-transaction` and `transaction-timeout` merge, the
  standalone rules (a positive `lastStandalone` keeps the last screens, a negative one drops from the front) and
  the menu-image scan with its default-subscreen limit all decide the same way.
- **The not-found branch**: a name that is neither a subscreen nor a transition and matches no file is a 404
  (`ErrorKind::NotFound`, `moqui-core/src/screen/url.rs:288-293`, `:348`), as Java's `targetExists = false` and
  `checkExists()` (`screen/ScreenUrlInfo.groovy:605-614`, `:789-792`); `allow-extra-path` keeps the path
  (`:289-291`).
- **The path cleanup** (`moqui-core/src/screen/url.rs:618-645` against `screen/ScreenUrlInfo.groovy:920-948`):
  the empty names, the `.` and `..`, and the `~name=value` inline parameter, with Java's `split("=")` rule that
  keeps a pair only when it has two parts (tested in `moqui-core/src/screen/tests/walk.rs:266-291`).
- **`ScreenPathAlias`** (`moqui-core/src/screen/url.rs:577-614` against `screen/ScreenUrlInfo.groovy:835-859`):
  the alias is read with the authorization off, the rows outside their `fromDate`/`thruDate` window are dropped, the
  newest `fromDate` wins, and a read failure leaves the path uncached so it is resolved again.
- **The `//` name search** (`moqui-core/src/screen/url.rs:649-728` against
  `screen/ScreenDefinition.groovy:523-614`): the direct child, the transition under the screen, the grandchild,
  the deep recursion and the file under the screen, in that order, and a direct child whose rest does not match is
  a miss rather than a deeper search. The port adds a depth cap where Java has none.
- **The URL cache** (`moqui-core/src/screen/url.rs:134-139`, `:170-172` against
  `screen/ScreenUrlInfo.groovy:136-165`): the key is the root screen, the path names and the `lastStandalone`, a
  walk that took a conditional default is not reused (`:322-328`, `:441-452`), and a cached walk is dropped when
  one of the screens it holds is no longer current (`moqui-core/src/screen/cache.rs:506-514`).
- **The screen-definition cache** (`moqui-core/src/screen/cache.rs:419-503` against
  `screen/ScreenFacadeImpl.groovy:152-209`): the definition is keyed by its location, the files it was read from
  (and the screens it extends) decide whether it is current, and `isScreen` reads only the root element of an
  XML file (`moqui-core/src/screen/cache.rs:556-576`).
- **The facade conf** (`moqui-core/src/screen/cache.rs:163-263` against `screen/ScreenFacadeImpl.groovy:74-88`):
  the render modes, their mime types, their macro modules, `always-standalone`, the binary `screen-output` modes,
  the `boundary-comments` flag and the `screen`/`subscreens-item` entries of the conf. `xsl-fo` is the print mode
  and `html-print` its canonical name in both.
- **The subscreens** (`moqui-core/src/screen/definition.rs:1290-1424` against
  `screen/ScreenDefinition.groovy:395-454`): the directory by convention, the `subscreens/subscreens-item`
  elements, the conf's `screen-facade/screen/subscreens-item` and its `default-subscreen`, and the
  `moqui.screen.SubscreensItem` rows with their `makeDefault`, in that order of override.
- **The root screen from the host** (`moqui-core/src/screen/info.rs:53-86` against
  `screen/ScreenFacadeImpl.groovy:350-365`): the first host pattern that matches the whole host wins, `.*` is the
  fallback even when it is listed first, and the port of the host is stripped first.
- **The screen info list** (`moqui-core/src/screen/info.rs:114-319` against
  `screen/ScreenFacadeImpl.groovy:368-484`): the counts of the screen and of all the screens under it, the
  non-placeholder rule (forms, sections, or more than four transitions), the transitions with actions (the port
  merges a transition's `service-call` into its actions, so it counts as Java's
  `hasActionsOrSingleService` does), the `indentedName` the tools screen reads (added in the script wrapper,
  `moqui-core/src/script/runtime/tools_host.rs:351-355`), and the walk that stops at a screen mounted twice.
- **The screen test** (`moqui-core/src/screen/screen_test.rs` against `screen/ScreenTestImpl.groovy`): the
  `rootScreen`, `baseScreenPath`, `renderMode`, `baseLinkUrl` and the request headers; the paths of the screens
  with no required parameter (`no_required_parameter_paths`, against
  `screen/ScreenDefinition.groovy:616-634`); the error strings (`[Template Error`, `FTL stack trace`,
  `Could not find subscreen or transition`); the counts of the renders, the errors and the characters; and the
  whole `ScreenTest` surface, which the script runtime exposes as `ec.screen.makeTest()`
  (`moqui-core/src/script/runtime/ec_screen.rs:41-86`).
- **The screen tree** (`moqui-core/src/screen/tree.rs:144-272` against `screen/ScreenTree.groovy:53-179`): the
  JSON of the jstree nodes (`id`, `text`, `li_attr.treeNodeName`, `a_attr` with the href, the load id and the URL
  text, `state.opened`/`selected`, `children`), the condition and the actions of the sub-node and of the node, the
  `nodeList` list with `nodeList_entry`, `nodeList_index` and `nodeList_has_next` in the context, the expansion of
  the id and the localized text, and the `javascript:` href of a `dynamic-load-id`.
- **The menu data** (`moqui-core/src/screen/menu.rs:150-351` against
  `screen/ScreenRenderImpl.groovy:2358-2520`): the 401 without a user, the 404 for a path that names no screen,
  the 403 for a screen the user may not view, the pre-actions of the screens below the first two, the entries of
  each level with their paths, the parameter strings, the images and their type, the `hasTabMenu` and
  `renderModes`, the last entry with the screen documents, the extra path list and the user's saved finds. The
  difference Java's `getMenuData` records for a broken subscreen — it throws and the whole menu answers 500 — is
  recorded in [Parity-Diffs](../Parity-Diffs.md) (the `menus` row of the gallery fixes, 2026-10-18), and the port
  leaves the entry out and logs it (`moqui-core/src/screen/menu.rs:76-87`).
- **The static file response** (`moqui-core/src/screen/static_files.rs:217-313`): the media type by extension, the
  `ETag` and the 304 for a matching `If-None-Match`, the one byte range of RFC 9110 (and the 416), the `HEAD`
  without a body, the `405` for another method, and the refusal of a path that leaves the screen's directory, of a
  hidden file and of a percent-encoded traversal (`:46-58`, `:87-103`).
- **The dispatch transitions** (`moqui-core/src/screen/dispatch.rs` against the `rest.xml` of both runtimes): the
  `login` with its second factor, the `sendOtp` and `verifyOtp`, the `logout`, the `e1`/`v1`/`m1`/`s1` entity and
  service calls, and the `sm` system message; the `X-HTTP-Method-Override` of a POST and the `X-Page-*` headers
  of a find. The bodies are the port's own Rust, not a port of the Groovy, so only the behavior was compared.
- **The schema documents** (`moqui-core/src/screen/schema.rs:25-40` against
  `util/RestSchemaUtil.groovy:584-663`): the entity name and the master name out of the path, the `.json`
  suffix, the master name `default` when none is given, and the 400 for an entity that does not exist. The shape
  of the document is a recorded difference ([Parity-Diffs](../Parity-Diffs.md), the `P2-12` row).


### Phase 8

The facade surface, method by method. Java's `WebFacade` interface and the public methods of
`WebFacadeImpl` against the port's `web` module and its script facade
(`script/runtime/facades.rs`, the `HostWeb` class):

| Java | Port | Result |
|---|---|---|
| `getRequestUrl` | `web::request_url()` (internal) | the same URL for the artifact hit; the script facade has no `getRequestUrl` (P8-17 for the encoding) |
| `getParameters` | `web::parameters()`, `ec.web.parameters` | present, but a plain-object copy with no `get` (P8-20) |
| `getRequest` | — | the servlet type; out of scope |
| `getRequestAttributes` | — | the port has no request attributes; the only Java uses of them are `moquiRequestStartTime`, `moqui.webapp.httpHost`, `moqui.session.token.created` and `moqui.login.error`, which the port keeps elsewhere (the request start time in the render, the http host in the conf, the new session in the response headers, the login error in the render job) |
| `getRequestParameters` | `web::parameters()`, `ec.web.requestParameters` | the same set of names; the query and the form body win over JSON and multipart |
| `getSecureRequestParameters` | `web::secure_parameters()` | the same rule: the names in the query string are left out, the body, the multipart and the JSON fields stay (`web/mod.rs:375-385`); the SQL runner reads it (`script/runtime/tools_host.rs:1602`) |
| `getHostName(boolean)` | `web::host_name()`, `ec.web.getHostName` | the host of the `Host` header; Java adds the server port when asked and the port does not add the default 80 or 443 (Low, same shape as P8-17) |
| `getPathInfo`, `getPathInfoList` | `screen/url.rs` | the port splits the path itself; the decoded segments are the same list |
| `getRequestBodyText` | the `_requestBodyText` parameter | present; the port also sets it for a binary body (P8-12) |
| `getResourceDistinctValue` | `web::resource_distinct_value()`, `ec.web.getResourceDistinctValue` | one value per process, as Java's `initStartHex` |
| `getResponse` | the web slot's output | the servlet type; out of scope |
| `getSession` | `user::session` | the port's own session (README redesign 7) |
| `getSessionAttributes` | `ec.web.sessionAttributes` | the same map, read and written |
| `getSessionToken` | `ec.web.sessionToken` | the same value; the response carries `moquiSessionToken` and `X-CSRF-Token` when it is new (`screen/mod.rs:468-474`), as Java's constructor does |
| `getServletContext`, `getApplicationAttributes` | — | the servlet context; out of scope |
| `getWebappRootUrl(boolean, Boolean)` | `ec.web.getWebappRootUrl` | Java's arguments; the full form is the conf webapp's URL, never the request's host (P8-08, fixed 2026-10-21) |
| `getErrorParameters` | `ec.web.errorParameters` | the same saved map, taken once per request |
| `getSavedMessages`, `getSavedErrors`, `getSavedValidationErrors` | `ec.web.savedMessages`, `savedErrors`, `savedValidationErrors` | the same lists, taken from the session once (`web/mod.rs:265-277`) |
| `getSavedPublicMessages` | — | the port has no public messages; no Java screen reads them |
| `getFieldValidationErrors(String)` | — | not ported; no Java screen reads it, and Java's own `WebFacadeStub` answers null for it |
| `getScreenHistory` | `ec.web.getScreenHistory` | the same list, newest first; the port keeps the paths and rebuilds the map, so the `name` is the last path segment where Java has the menu name and the first two parameter values, and there is no `screenLocation` |
| `sendJsonResponse` | `ec.web.sendJsonResponse` | the same rules: a string is sent as it is, the messages become `messages`, the errors become `errors` and the status 500, a JSON-RPC answer keeps its own errors (`web/mod.rs:611-653`) |
| `sendJsonError(int, String, Throwable)` | `ec.web.sendJsonError` | the same `{"errorCode","errors"}` body and status; the throwable is not used in either |
| `sendTextResponse(String)` / `(String, String, String)` | `ec.web.sendTextResponse` | the same content type and the same attachment disposition with the ASCII file name; the port adds `;charset=UTF-8` where Java does not, sends no `Content-Disposition: inline` when there is no file name (Java sends one), and it does not have Java's error rule (P8-09) |
| `sendResourceResponse(String)` / `(String, boolean)` | `ec.web.sendResourceResponse(location, inline, fileName)` | the same 404 for a location that is not there, the same content type from the file name, the same inline and attachment dispositions; the port adds a file name argument and its own cache rule (P8-18) |
| `sendError(int, String, Throwable)` | `ec.web.sendError` | the same three bodies from `Accept`. `DataSnapshot.xml` stays on `sendJsonError` |
| `handleJsonRpcServiceCall` | — | excluded on purpose (JSON-RPC) |
| `handleEntityRestCall` | `screen/dispatch.rs` (`e1`, `v1`, `m1`) | the same: the JSON parse error is a 400, no user is a 401, `X-HTTP-Method-Override` on a POST, the JSON list of objects is a batch, the page and run-time headers |
| `handleServiceRestCall` | `screen/dispatch.rs` (`s1`) | the same: the login error is a 401, the other errors a 500, the JSON parse error a 400, the batch list |
| `handleSystemMessage` | `service/system_sync/message.rs` | the same: the type and the remote are checked, the four auth modes (`SmatLogin`, `SmatHmacSha256`, `SmatHmacSha256Timestamp` with its five minute window and replay cache, `SmatNone`), the same HMAC values (`WebUtilities.hmacSha256Base64` and `hmacSha256Hex`, `service/system_sync/message.rs:378-401`) |
| `sendQzSignedResponse` (impl) | `ec.web.sendQzSignedResponse` | the same: no user is a 401, the Base64 `SHA1withRSA` signature as `text/plain`, a key that cannot be read a 500 |
| `saveScreenLastInfo` (impl) | `ec.web.saveScreenLastInfo`, `user::session::save_screen_last` | the same path and parameters in the session |
| `saveScreenHistory` (impl) | `user::session::push_screen_history` | the same list, newest first, without repeats, 40 entries |
| `addDeclaredPathParameter` (impl) | the screen's `~name=value` path parameters | the same: the name is not decoded, the value is (`screen/url.rs:616-645` against `ScreenUrlInfo.cleanupPathNameList:920-948`) |
| `getRequestDetails` (impl) | `web::request_details()` | the same shape: the request line, the scheme, the remote address, every header, every parameter; the port hides the `Authorization` and `Cookie` headers and leaves out the credential parameters |
| `makeNewSession` (impl) | the login rotation in `user/session.rs` | the same: a new session id, the attributes copied, a new token |
| `saveMessagesToSession`, `saveParametersToSession`, `saveRequestParametersToSession`, `saveErrorParametersToSession` (impl) | `user::session` | the same maps, taken once by the next request |
| `makeDiskFileItemFactory` (impl) | `multipart::Spool` | the same idea: the parts over 256 KiB go to a file under `runtime/tmp/upload`; the port adds limits Java has not (2000 parts, 16 KiB of part headers, 1 MiB of a text field), which is the hardening the web-protection design asks for |
| `viewEmailMessage` (impl) | — | the tracking pixel; Java says it is not an HTTP entry point |
| `runFirstHitInVisitActions`, `runBeforeRequestActions`, `runAfterRequestActions`, `runAfterLoginActions`, `runBeforeLogoutActions` (impl) | `screen/render.rs` | the webapp's lifecycle actions; the screens phase |

Other checks of this phase:

- `WebUtilities.isSameOriginRedirect` (`WebUtilities.java:423-453` against `web/mod.rs:463-499`): the
  same rules — empty, a backslash, a control character, `javascript:`, `data:`, `vbscript:` and a
  `//` target are refused, a path is accepted, an absolute URL is accepted only when it names the
  configured host, and a user info (`a@b`) is refused. The port falls back to the render's base URL
  when no configured host is given, where Java uses the webapp's `http-host` and so refuses every
  absolute URL when it is empty; for a real request the base URL is empty, so the two agree. The port's
  script binding takes the configured host where Java takes the request
  (`script/runtime/ec_extra.rs:691-708`), which the comment there says.
- `WebUtilities.ipMatches` (`WebUtilities.java:385-405` against `web/mod.rs:1327-1356`): the same
  comma separated patterns, the `*` and `*.*.*.*` wildcards, the four dot separated parts, the
  `low-high` ranges and the exact IPv6 match.
- `WebUtilities.canonicalizeClientIp` (`WebUtilities.java:330-355`): the port's `parse_hop`
  (`web/address.rs:107-128`) drops the brackets, the port and the zone, and reads a `Forwarded` style
  `for=` pair, as Java does.
- `getClientIp` (`UserFacadeImpl.groovy:967-1019` against `web/mod.rs:1303-1317` and
  `web/address.rs:130-216`): the same header of the conf, the same first hop when no proxy list is
  set, the same fall back to the socket peer. The port adds `trusted-proxies` with CIDR ranges and
  takes the rightmost hop that is not a trusted proxy; that difference is named in
  [Parity-Mapping, Web and the HTTP host](../Parity-Mapping.md#web-and-the-http-host) (`web/address.rs`,
  "Partial") and is a hardening.
- `WebUtilities.isExecutable` (`WebUtilities.java:752-779` against `multipart.rs:199-203`): the same
  headers — Windows PE, Linux ELF, Java class, macOS Mach-O and its byte swap, and the `#!` shebang —
  and a ZIP is not a program in either.
- `WebMediaTypeException` (`WebMediaTypeException.java:20-23`): the port has no exception type of its
  own for this; the executable upload is refused with a 415 and the same message
  (`screen/mod.rs:114-116`), which is the status Java's `MoquiServlet` gives it
  (`MoquiServlet.groovy:135-137`). The body shape differs: this path uses `early_error`, not `sendError`.
- `WebUtilities.encodeHtmlJsSafe` and `fieldValuesEncodeHtmlJsSafe`
  (`WebUtilities.java:179-264` against `script/runtime/ec_extra.rs:610-658`): the same escapes, with
  the backslash and the U+2028/U+2029 escapes added, and a null argument is empty text in both.
- `WebUtilities.encodeHtml` (`WebUtilities.java:266-287` against `screen/sri/mod.rs:157-170`): the
  same HTML escapes, except for the control characters (P8-16).
- `WebUtilities.stripUserAccountSecrets`, `removeUserAccountSecretsFromMap`,
  `removeUserAccountIdentityFromMap`, `isUserAccountEntity`, `isIdentityAdminEntity`,
  `isSecretConfigEntity` and `isRestrictedGenericEntity` (`WebUtilities.java:496-551` against
  `entity/sensitive.rs`): the same field names, the same entity name classification, the same
  "restricted generic entity" set, and the script binding the Tools screens use
  (`script/runtime/tools_host.rs:806`). The port strips the secrets from a nested map as well, which
  Java does not (P8-19 is about the credential parameters, the same shape).
- `WebUtilities.hmacSha256`, `hmacSha256Base64`, `hmacSha256Hex` and `hmacSha256TimestampHeader`
  (`WebUtilities.java:667-695` against `service/system_sync/message.rs:378-401`): the same values, and
  the same header format `t=<seconds>,v1=<hex>` for the timestamped one.
- `WebUtilities.lastPathSegment` and `isSafeSinglePathSegment` (`WebUtilities.java:468-486` against
  `script/runtime/ec_extra.rs:687-717`): the same rules, including the backslash as a separator and
  the control characters.
- The multipart map (`WebFacadeImpl.groovy:161-237` against `web/mod.rs:1109-1145` and
  `multipart.rs`): the same shape — the fields by name, the files by name, `_fileUploadList` with all
  the files, a repeated name becomes a list — and the same part headers (`Content-Disposition` with
  the name and the file name, the quoted values with the escapes, `Content-Type`). A file input that
  was left empty posts nothing in both.
- The session cookie: the port has its own cookie names (`moqui.session`, `moqui.visitor`) and its own
  session row; that is README redesign 7, named in
  [Parity-Mapping, User and session](../Parity-Mapping.md#user-and-session) (`user/session.rs`,
  "Redesign").


### Phase 9

- **The route table.** Every URL pattern the Java side serves was checked against the port's
  router (`moqui-host/src/http.rs:208-221`): `/*` → the screen dispatcher (the fallback,
  `http.rs:220-221` → `moqui-core/src/screen/mod.rs:89-135`), `/fop/*` → the same dispatcher
  through `fop_screen_path` (`screen/mod.rs:241-254,316-320`, Java's `MoquiFopServlet`), `/notws` →
  `notws::upgrade` (`http.rs:211`), `/elastic/*` and `/kibana/*` → the proxy
  (`http.rs:212-217`, Java's `ElasticSearchProxy`/`KibanaProxy`); since 2026-10-21 every path but the fixed routes goes
  to the conf's servlets (`web/servlet.rs`), the proxies being `ProxyServlet.js` on `/search` and `/dashboard` (P9-03). The port adds `/health`,
  `/metrics` and `/git/*`, which have no Java counterpart. The LLM, A2A and `groovysh` routes are
  excluded (see below).
- **The screen request pipeline.** Java's servlet thread does the render inside
  `MoquiServlet.service` (`MoquiServlet.groovy:52-178`); the port does it on the factory's request
  pool (`http.rs:597-647`, `moqui-core/src/service/request_pool.rs`), with the screen's own
  transaction begun and ended by the render (`moqui-core/src/screen/render.rs:1740-1759`, Java's
  `ScreenRenderImpl` `beginTransaction`/`commit`).
- **The error page of a failed screen.** Java's `sendErrorResponse` renders the `error/*` screen
  with `errorCode`, `errorType`, `errorMessage`, `errorThrowable` and `showErrorDetail`
  (`MoquiServlet.groovy:279-298`); the port renders the same screen with the same context
  (`moqui-core/src/screen/mod.rs:385-449`), and only for a client that takes html and a path that
  is not an API path, as Java's `Accept: text/html` check does.
- **The status of a failed screen.** Java maps `AuthenticationRequiredException` to 401,
  `ArtifactAuthorizationException` to 403, `ScreenResourceNotFoundException` to 404,
  `ArtifactTarpitException` to 429 and `WebMediaTypeException` to 415
  (`MoquiServlet.groovy:119-137`); the port maps its error kinds the same way
  (`moqui-core/src/web/mod.rs:1543-1554`), with the 415 for an executable upload
  (`moqui-core/src/screen/mod.rs:114-116`).
- **The `moquiSession` cookie.** Java's session cookie is the container's, with `HttpOnly` and
  `SameSite=Lax` set by `MoquiContextListener` (`MoquiContextListener.groovy:161-168`) and `Secure`
  on a secure request; the port's `moqui.session` cookie carries `HttpOnly; SameSite=Lax; Path=/`
  and `Secure` over https (`moqui-core/src/user/session.rs:351-366`). The `moqui.visitor` cookie
  has its one-year `Max-Age` in both (`UserFacadeImpl.groovy:223-228`).
- **The session token.** Java creates `moqui.session.token` on the first request of a session and
  sends it as the `moquiSessionToken` and `X-CSRF-Token` headers
  (`WebFacadeImpl.groovy:210-218`); the port sends the same two headers when the session is new or
  the token changed (`moqui-core/src/screen/mod.rs:451-476`). The CSRF check of a write transition
  matches Java's rule (a non-GET method, a transition that runs something, the token from the
  parameter or the header, and the header-authenticated requests are exempt):
  `moqui-core/src/screen/render.rs:1778-1811` against `ScreenRenderImpl.groovy:458-475`.
- **`MoquiAuthFilter`.** Java's rules are: an `OPTIONS` request is answered by `handleCors` before
  the user is read (`MoquiAuthFilter.groovy:64-68`), the session, the Basic header, the `api_key`
  and `login_key` headers and the `authUsername`/`authPassword` parameters log a user in
  (`UserFacadeImpl.groovy:96-184`), no user is 401 and a missing permission is 403
  (`MoquiAuthFilter.groovy:80-90`). The port's `check_permission`
  (`moqui-core/src/screen/mod.rs:546-579`) binds the session, runs the same `authenticate`
  (`screen/mod.rs:485-527`) and answers 401 without a user and 403 without the permission
  (`moqui-host/src/search_proxy.rs:123-136`); the `ElasticRemote` and `KibanaRemote` permissions
  are the ones Java's filter declares (`MoquiDefaultConf.xml:248-255`). Since 2026-10-21 this is
  `webroot/script/AuthFilter.js` with `SearchRemote` and `DashboardRemote` (P9-03).
- **The screen auth rules.** Java's `checkWebappSettings` (`ScreenRenderImpl.groovy:998-1040`)
  refuses a screen with `allow-web-request="false"`, redirects a screen that needs a user to the
  last `login-path` of the path, and answers 401 for a `lastStandalone` or a transition; the port
  has the same three rules (`moqui-core/src/screen/render.rs:750-763,798-834,839-853`), takes the
  last `login-path` of the path as Java does, and sends the same 401.
- **The websocket endpoint.** The path is `/notws` in both (`MoquiDefaultConf.xml:301`); the
  handshake checks the `Origin` against the allow list and the `Host`
  (`MoquiContextListener.groovy:254-282`, Java's `WebUtilities.webSocketOriginAllowed`), which the
  port does with the same rules (`moqui-host/src/notws.rs:85-91`,
  `moqui-core/src/web/mod.rs:1394-1415`); the user comes from the session of the handshake
  (Java's `initFromHttpSession`, `MoquiAbstractEndpoint.groovy:81-96`; the port reads the session
  row, `notws.rs:122-154`); the `subscribe:` and `unsubscribe:` commands and the `ALL` topic are
  the same (`NotificationEndpoint.groovy:44-62`, `NotificationWebSocketListener.groovy:69-83`; the
  port's `notws.rs:242-278,328-363`); the wrapped message has the same fields
  (`NotificationMessageImpl.groovy:458-463`; the port's `notify/message.rs:594-658`); the idle
  timeout is the endpoint's hour (`MoquiDefaultConf.xml:301`, `notws.rs:52-53,179`); and
  `markSent` is called for the user that got the message.
- **The log files.** `moqui.log` in `runtime/log`, rolled by size and by day (`moqui-host/src/logging.rs:419-469`, Java's
  `log4j2.xml:18-36`). The second file differs in name and level (P9-12).
  The event log ships to `moqui_logs` and the request log to `moqui_http_log` through the search
  backend, as Java's `ElasticSearchLogger` and `ElasticRequestLogFilter` do
  (`moqui-host/src/logging.rs:114-158`, `moqui-core/src/search/logs.rs:26-28`).
- **The startup.** Java's `MoquiContextListener.contextInitialized` builds the factory, the webapp
  filters, listeners and servlets, the session cookie config, the websocket endpoints and the
  after-startup actions, and `contextDestroyed` runs the before-shutdown actions and destroys the
  factory (`MoquiContextListener.groovy:57-235`). The port's `boot_server_factory`
  (`moqui-host/src/lib.rs:985-1169`) builds the factory, the datasource, the entity facade, the
  search, the tools, the request pool, the cluster and the notification push, and the serve thread
  flushes the artifact hits and the sessions and stops the log shipping at the end
  (`lib.rs:942-961`), which is Java's `ecfi.destroy()`.
