# Node dependency: moving the node tests onto moqui's own test tools

> Archived 2026-10-27: the Node test migration is complete. Current usage is in [Testing](../../docs/Build-Test.md#component-specs) and [Client-Testing](../../docs/dev/Client-Testing.md). Node remains only for the library refresh and WASM smoke tools listed below.

## Why

The aim of the test-client work is a test toolchain of cargo (to build Rust) plus the moqui binary, with all JavaScript
automation on engines we already ship: QuickJS inside moqui, and the headless Chromium that `moqui test-client` drives.
That move is done. The survey below is what the Node files were: 33 `*.test.js` files, 32 under `runtime/base-component`
and `DevAide.test.js` in dev-aide.
They used `node:test`, `node:assert/strict`, `fs`, `path`, `vm`, `os`, `child_process` (Chromium `--dump-dom`) and
`crypto`. Cargo ran 19 of them when `node` was on the PATH, and `./moq test-all` ran the rest.

What is in them (counts from the 2026-10-25 survey):

| Group | Files | Tests | Code under test | Runs in production on |
|---|---|---|---|---|
| Template generators | `framework/template/screen/{ScreenHtml,ScreenQvt,DatePeriod,FieldTitle}.test.js` | ~34 | the screen-to-html/qvt generators | QuickJS (the screen renderer) |
| Client units | `webroot/screen/webroot/js/{MoquiLib,MoquiL10n,moqui-html,WebrootVue.qvt.*,WebrootPublic.qvt,WebrootAssist*,WebrootChat,WebrootCode}.test.js`, `dev-aide/.../DevAide.test.js` | ~260 | browser client code, with `window`/`document` stubbed through `vm` contexts | the browser |
| Browser layout and behaviour | `css/{moqui-html,WebrootVue.qvt}.test.js`, `js/WebrootVue.qvt.display.test.js`, `js/*-browser.test.js` | ~20 | CSS and widgets in a real page, via temp files and `chromium --dump-dom` | the browser |

Decision (2026-10-25): each group runs on the engine its code ships to. Template tests run in QuickJS with
`moqui test`. Client-unit and browser tests run in Chromium with `moqui test-client`. Chromium is already a test
dependency (gate3_browser, client_browser), and `./moq chromium-download` provides it.

## Reviewed (2026-10-26)

Implemented. The work follows this plan, with these corrections:

- Assertions are `crates/moqui-core/src/testkit/assert.js`, loaded by the core prelude both runners already share, and injected into the page harness. `test_client/prelude.js` does not get a second copy.
- The four template files need `runInContext(source, sandbox)`: a second QuickJS context whose global can hold functions such as `sri.write`. `loadTemplate` does not provide that. `ec.resource.getLocationText` still replaces `fs.readFileSync` for the source.
- Offline pages use the origin `https://moqui.test/` and serve `/js`, `/css` and `/libs` from the webroot plus `/files/<repo-relative>` from the repo (read-only, `..` rejected). `files.read` stays confined to the spec's component. Fetch runs only for that origin, and only on an offline page.
- `suite.js`'s `server` stays the `--serve` object `{ components, specs, conf }`. Offline is `spec({ server: false })` on the new webroot and dev-aide specs. Those specs are not added to the framework security suite. `--offline` together with `--serve` is an error.
- `hmac.sha256Base64` and `hmac.sha256Timestamp` stay. An offline spec can still call `hmac`, `files`, `sleep` and `log`. `keyboard.press` and `fill` stay. `keyboard.type` sends key events; it does not call `Input.insertText`.
- `moqui test --db none` boots the factory and skips Postgres, including on a build without `embedded-postgres`. A `database: false` spec masks `ec.entity`, `call` and `screenTest` even when the run did open a database. The mask ends with that spec.
- The Node runners in `files.rs` and `dev-aide/tests/client.rs` are gone. `moq test-all` runs `moqui test` and the offline client specs. Those are not cargo tests that spawn the binary.

## Gaps in the tools today

Closed. `moqui test --db none` and `spec({ database: false })` run the template specs without Postgres, including on a
build without `embedded-postgres`. `moqui test-client --offline` runs `spec({ server: false })` at `https://moqui.test`
with no listening port. Assertions, `browser.runInPage`, `setContent`, `addScript`, `addStyle` and `keyboard.type` are
in [Client-Testing](../../docs/dev/Client-Testing.md). Node is not a test dependency. `libs/refresh.mjs` and `scripts/wasm-smoke.mjs` still use Node.

## Plan

The sections below are the design. What shipped is the Reviewed list above (`runInContext` rather than `loadTemplate`,
`spec({ server: false })` rather than `suite.js` `server: false` for the offline specs).

### Phase 1: shared assertions (both runners)
- One `assert` object with `equal` (strict), `deepEqual` (strict, structural), `notEqual`, `ok`, `match`,
  `doesNotMatch`, `throws(fn, re|Error)`, `rejects(promise, re)` and `fail(msg)`, keeping the bare `assert(cond, msg)`
  call.
- Its messages show both values, in the style of the power-assert output.
- It lives in one JS file that both preludes load: `crates/moqui-core/src/testkit/prelude.js` and
  `crates/moqui-host/src/test_client/prelude.js`.
- The in-page harness (phase 3) uses the same file, so the three places assert alike.

### Phase 2: `moqui test` without a database
- A spec option, `spec({ database: false })`, plus a run option `--db none`. When every spec selected says
  `database: false`, the run skips `fixture::open`.
- Such a spec still has the script runtime, but no `ec.entity`, no `call` and no `screenTest`. Using any of them
  throws an error naming the option.
- Build the no-database path without the `embedded-postgres` gate, so every package can run these specs.
- Add `loadTemplate(name)` (or the renderer's own loader, exposed): it evaluates `framework/template/screen/<name>.js`
  the way the screen renderer does, and returns its exports.
- Convert the 4 template test files to `framework/test/Screen*.spec.js` (or `framework/template/screen/test/`):
  - each `test(name, fn)` becomes a `feature(name, { expect/then })`;
  - `fs.readFileSync` becomes `ec.resource.getLocationText`;
  - Node `assert.*` becomes the phase 1 `assert.*`.
  - `ScreenHtml.test.js` and `ScreenQvt.test.js` are the bulk of the work (28 tests).

### Phase 3: `moqui test-client` pages without a server
- **Offline suites.**
  - `suite.js` `server: false`, or `spec({ server: false })`, skips `check_server`.
  - `moqui test-client <component> --offline` runs only the offline specs. A run with a server runs both kinds.
- **Local origin.**
  - Pages open at a fixed origin, e.g. `https://moqui.local/`. CDP `Fetch.enable` intercepts its requests and answers
    them from the runtime's files: `component://`-style paths, and `/libs`, `/js` and `/css` under
    `base-component/webroot/screen/webroot`.
  - Anything else gets a 404, which is reported as a page problem. Nothing listens on a port.
  - This replaces the temp files and `file://` loads the browser tests use today, so relative library URLs resolve as
    they do on a server.
- **New page calls:**
  - `page.setContent(html)`;
  - `page.addScript(path | {content})` and `page.addStyle(path | {content})`;
  - `page.keyboard.type(text)`, on the existing `type_text`;
  - `page.goto` takes a path on the local origin.
- **In-page runner,** for the client unit tests:
  - `browser.runInPage({ scripts: [...], styles: [...], test: 'js/test/MoquiLib.page.js' })` opens a fresh page on the
    local origin and loads the scripts in order.
  - It injects a small harness exposing the `node:test`-shaped API the files use today (`test(name, fn)`, async
    `fn`), with the phase 1 `assert`.
  - It runs the file and reports each `test` as its own case in the console and JUnit output. Page problems still fail
    a case.
  - A fresh context (today's `vm.runInNewContext` with stub globals) becomes `harness.frame(scripts, globals)`: a new
    iframe that loads the scripts and gets the stub globals set before them. Where the stubs only faked the browser,
    use the real DOM instead.
  - A spec of a few lines (`ClientUnits.spec.js`) lists the page test files with the scripts each needs.
- **Converting the client units** (18 files plus DevAide.test.js), mostly mechanically:
  - `require("./X.js")` becomes an entry in `scripts`;
  - `fs.readFileSync` of fixtures becomes `fetch` from the local origin;
  - `vm` contexts become `harness.frame`;
  - `setTimeout` and promises work, being the browser's own.
  - `WebrootVue.qvt.test.js` compiles templates with `libs/vue/compiler-dom`, which loads as a script.
- **Converting the browser tests** (10 files): each becomes a test-client `feature`.
  - `page.setContent` (or a page on the local origin) plus `addStyle`/`addScript` in place of the temp HTML file;
  - `page.evaluate` and `locator.boundingBox` in place of parsing `--dump-dom` output;
  - `waitFor`/`settle` in place of `--virtual-time-budget`;
  - real `keyboard.type`/`press` in place of synthetic events where the test is about keys (keys-browser,
    qvt-keys-browser).
  - The flaky focus-outline check in `css/moqui-html.test.js` should become a real `focus()` and a computed-style read.

### Phase 4: wiring and removing Node
- **Cargo:**
  - `files.rs` (`the_html_client_passes_its_node_tests`, `the_qvt_client_passes_its_node_tests`) and
    `dev-aide/tests/client.rs` run `moqui test framework` (no database) and `moqui test-client framework --offline`
    through the built binary, as gate3_browser does. Or retire them, if `moq test-all` is the place to run them.
  - The Chromium-free template specs stay in the default `cargo test`. The Chromium ones go with the opt-in browser
    tests.
- **`moq test-all`:**
  - drop the `node --test` suite;
  - add `moqui test-client --offline` for every component;
  - `moqui test` picks up the database-free specs as it runs the others.
- **Delete `js/chromium-for-tests.js`.** `crates/moqui-host/src/chromium.rs` `find_test_browser` is the one lookup.
- **Docs:**
  - `js/README.md` (the Tests section), `docs/Build-Test.md` (`database: false`) and `docs/dev/Client-Testing.md` (offline
    suites, `runInPage`, the new page calls);
  - AGENTS.md or the building skill: Node is no longer a test dependency.

### Out of scope (Node stays, as tools rather than tests)
- `libs/refresh.mjs`: it rebuilds vendored libraries from npm, so it is npm-bound.
- `scripts/wasm-smoke.mjs`: it loads the browser module in Node with JSPI and MEMFS. The Chrome and WebSocket
  smokes are `cargo test -p moqui-host --test wasm_browser`.

## Order and size
1. **Phase 1 assertions and phase 3's local origin plus `setContent`/`addScript`:** these unblock everything else.
2. **Convert the 10 browser tests:** this proves the offline mode on real pages, and removes temp files and
   `--dump-dom`.
3. **`runInPage`, then convert the client units:** the largest part, about 260 tests, done file by file.
   `MoquiLib.test.js` comes first, since most others load it.
4. **Phase 2, the database-free `moqui test`, then the 4 template files.**
5. **Phase 4 wiring, then delete the node runners from cargo and `moq test-all`.**

Each converted file keeps its test count, or the conversion notes why a test was dropped or merged. Delete a Node file
only once its replacement has passed, both in `./moq test-all` and with Chromium absent (the browser cases skip and the
QuickJS ones still pass).

## Verification
- Per converted file:
  - the new spec reports the same number of cases as `node --test` did;
  - deliberately breaking the code under test, e.g. reverting a fix such as the #119 reload bug, makes it fail.
- `moqui test-client framework --offline` passes with no server running and no port bound. Check with
  `ss -ltn` before and after.
- `./moq test-all` passes on a machine with no Node on the PATH (e.g. `PATH` without node), with only cargo, the moqui
  binary and Chromium.
- With no Chromium, the offline browser specs report as skipped (failing under `--fail-on-skip`), and the template
  specs still pass.
