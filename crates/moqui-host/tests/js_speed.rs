/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! Script timings and memory: QuickJS and its bindings by the way the framework enters it. Expressions (`from`,
//! `condition`, `${}` strings) are one engine entry each; scripts and XML actions are one entry that does more inside;
//! services add the service layer to actions and script files; templates are the fancy dialect, trusted and locked
//! down; screens render the fixture screens of `tests/fixtures/js-speed` in html, qvt and text. Then what an engine
//! costs a thread: the time and QuickJS heap of its first script and its first screen.
//!
//! Out of the default `cargo test` link, and ignored so naming the binary does not start a postmaster.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test js_speed -- --ignored --nocapture
//! ```
//!
//! `JS_SPEED_CALLS` (default 2000; services 1000, screens 200) and `JS_SPEED_BATCHES` (default 5) size it;
//! `JS_SPEED_ONLY` (a comma list of `expr`, `script`, `actions`, `service`, `template`, `screen`, `cold`) runs those
//! sections alone, and `JS_SPEED_ROW` (part of a row name) those rows alone, for a profile; the new thread's rows too
//! (`engine start`). `JS_SPEED_PEEK` prints the start of each screen's output, and `JS_SPEED_DUMP` (a directory) writes
//! each screen's output there, to compare two builds' output. See `speed/mod.rs` for the columns. The database is embedded Postgres (or `SPEED_PG_HOST`), for the services and the cached finds; nothing
//! here writes.
//!
//! The context is entered with authorization disabled, as in `service_speed`. Names the expressions read are set once
//! in the context before the rows; each expression text is the same on every call, so it is compiled once per engine,
//! except in the rows named "new text", which compile every call.

#![cfg(all(unix, feature = "embedded-postgres"))]

mod speed;

use std::collections::BTreeMap;
use std::hint::black_box;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::driver::DbDriver;
use moqui_core::entity::EntityFacade;
use moqui_core::script::{self, template, EngineMemory};
use moqui_core::transaction::PoolConfig;
use moqui_host::db::entity_facade::{attach, open_entity_facade};
use moqui_lib::object::Value;

use speed::{env_usize, DirGuard, Rows};

const ENUM: &str = "moqui.basic.Enumeration";
const SERVICES: &str = "jsspeed.JsSpeedServices";
const WARMUP: usize = 32;

/// The body of `echo#Data`.
const ECHO_ACTIONS: &str = r#"<actions>
    <set field="textOut1" from="textIn1"/>
    <set field="textOut2" from="textIn2"/>
    <set field="numberOut" from="numberIn"/>
    <set field="timestampOut" from="timestampIn"/>
</actions>"#;

/// The walk of `loop#Actions` over the 100 items set in the context.
const LOOP_ACTIONS: &str = r#"<actions>
    <set field="total" from="0" type="Integer"/>
    <set field="names" from="[]"/>
    <iterate list="itemList" entry="item">
        <if condition="item.qty &gt; 2">
            <set field="total" from="total + item.qty"/>
            <script>names.push(item.name)</script>
        </if>
    </iterate>
</actions>"#;

struct Sizes {
    calls: usize,
    services: usize,
    screens: usize,
    batches: usize,
}

#[test]
#[ignore = "script timings: cargo test --release -p moqui-host --test js_speed -- --ignored --nocapture"]
fn script_speed() {
    speed::warn_if_debug("js_speed");
    moqui_host::logging::install_with_console(Some(tracing::Level::WARN));
    let calls = env_usize("JS_SPEED_CALLS", 2000);
    let sizes = Sizes {
        calls,
        services: (calls / 2).max(1),
        screens: (calls / 10).max(1),
        batches: env_usize("JS_SPEED_BATCHES", 5),
    };
    let only = std::env::var("JS_SPEED_ONLY").unwrap_or_default();
    let wanted = |section: &str| only.is_empty() || only.split(',').any(|one| one.trim() == section);

    let runtime = speed::temp_dir("js-speed-rt");
    let cluster_dir = speed::temp_dir("js-speed-pg");
    let _guards = (DirGuard(runtime.clone()), DirGuard(cluster_dir.clone()));
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/js-speed");
    let component = runtime.join("component/js-speed");
    copy_dir(&fixture, &component);
    let (cluster, driver) = speed::SpeedDb::start(&cluster_dir, 20);
    let driver: Arc<dyn DbDriver> = Arc::new(driver);
    let framework = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework).expect("boot");
    let facade = open_entity_facade(&framework, Arc::clone(&driver), pool()).expect("open entity facade");
    attach(&factory, facade.clone());
    facade.check_all_tables().expect("check tables");
    seed(&facade);
    let telemetry_measurement = speed::telemetry_measurement(&factory);
    let root = component.join("screen/JsSpeed.xml").to_string_lossy().into_owned();
    let items_template = std::fs::read_to_string(component.join("template/Items.fancy")).expect("template");

    {
        let context = factory.new_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        println!();
        println!(
            "{} calls ({} for services, {} for screens) x {} batches, warmup {WARMUP}. \
             ExecutionContext entered, authorization disabled.",
            sizes.calls,
            sizes.services,
            sizes.screens,
            sizes.batches
        );
        set_names();
        if wanted("expr") {
            section("expressions: one engine entry each", &sizes, sizes.calls, expressions);
        }
        if wanted("script") {
            section("scripts: one entry, the work inside", &sizes, sizes.calls, scripts);
        }
        if wanted("actions") {
            section("XML actions, run directly", &sizes, sizes.calls, actions);
        }
        if wanted("service") {
            section(
                "services: the service layer around actions and scripts",
                &sizes,
                sizes.services,
                services,
            );
        }
        if wanted("template") {
            section(
                "templates: the fancy dialect over 20 items (the locked form in its sandbox runtime)",
                &sizes,
                sizes.calls,
                |rows, calls| templates(rows, calls, &items_template),
            );
        }
        if wanted("screen") {
            section("screens: the js-speed fixture screens", &sizes, sizes.screens, |rows, calls| {
                screens(rows, calls, &factory, &root);
            });
        }
        print_memory("this thread's engine, after the rows", script::engine_memory(true));
        print_engine_stats();
    }
    if wanted("cold") {
        cold(&factory, &root, &sizes);
    }

    drop(telemetry_measurement);
    drop(factory);
    drop(facade);
    drop(driver);
    drop(cluster);
}

/// `JS_SPEED_ROW`, when set, is a part of the names of the rows to run; the rest of their sections is skipped. For a
/// profile of one row, as `sample` or `samply` take it.
fn row_wanted(label: &str) -> bool {
    std::env::var("JS_SPEED_ROW").map_or(true, |part| label.contains(&part))
}

/// [`Rows::run`] of a row [`row_wanted`].
trait RunIf {
    fn run_if(&mut self, operation: &str, calls: usize, body: impl FnOnce());
}

impl RunIf for Rows {
    fn run_if(&mut self, operation: &str, calls: usize, body: impl FnOnce()) {
        if row_wanted(operation) {
            self.run(operation, calls, body);
        }
    }
}

fn section(label: &str, sizes: &Sizes, calls: usize, body: impl Fn(&mut Rows, usize)) {
    println!("\n== {label}");
    let mut rows = Rows {
        js: true,
        ..Rows::default()
    };
    body(&mut rows, WARMUP.min(calls));
    rows.record = true;
    for batch in 1..=sizes.batches {
        eprintln!("  {label}: batch {batch}/{}", sizes.batches);
        body(&mut rows, calls);
    }
    rows.print();
}

/// The names the expressions, scripts and templates read.
fn set_names() {
    script::run_script(
        r#"a = 3; b = 4; status = 'ACTIVE'; qty = 5; title = 'Items';
        item = {itemId: 'I1', name: 'Widget', qty: 3};
        itemList = []; for (let i = 0; i < 100; i++) itemList.push({id: 'I' + i, qty: i % 7, name: 'Item ' + i});
        items = itemList.slice(0, 20);"#,
    )
    .expect("set names");
    script::run_actions(
        r#"<actions><set field="price" from="'12.50'" type="BigDecimal"/><set field="dqty" from="'3'" type="BigDecimal"/></actions>"#,
    )
    .expect("set decimals");
    for (name, value) in [
        ("textIn1", Value::string("foo")),
        ("textIn2", Value::string("ping")),
        ("numberIn", Value::string("12.34")),
        ("timestampIn", Value::Timestamp(jiff::Timestamp::now())),
    ] {
        script::set_context_name(name, value).expect("set name");
    }
}

fn expressions(rows: &mut Rows, calls: usize) {
    let value = |rows: &mut Rows, label: &str, expression: &str| {
        if !row_wanted(label) {
            return;
        }
        rows.run(label, calls, || {
            for _ in 0..calls {
                black_box(script::eval_value(expression).unwrap_or_else(|err| panic!("{expression}: {err}")));
            }
        });
    };
    value(rows, "value `a` (a bare name)", "a");
    value(rows, "value `a + b * 2`", "a + b * 2");
    value(rows, "value `item.name` (a map's field)", "item.name");
    value(rows, "value `{x: a, y: item.name}` (a new map)", "{x: a, y: item.name}");
    value(rows, "value `price.multiply(dqty)` (decimals)", "price.multiply(dqty)");
    value(rows, "value `itemList.length` (100 maps)", "itemList.length");
    rows.run_if("condition `status == 'ACTIVE' && qty > 2`", calls, || {
        for _ in 0..calls {
            black_box(script::eval_condition("status == 'ACTIVE' && qty > 2").expect("condition"));
        }
    });
    rows.run_if("expand `Item ${item.itemId}: ${item.name}`", calls, || {
        for _ in 0..calls {
            black_box(script::expand("Item ${item.itemId}: ${item.name}").expect("expand"));
        }
    });
    rows.run_if("expand plain text, no ${}", calls, || {
        for _ in 0..calls {
            black_box(script::expand("Plain text with nothing to expand").expect("expand"));
        }
    });
    // Each text once, so each call compiles. The cache keeps them, up to its bound.
    let start = rows.rows.len() * 1_000_000 + calls * 7;
    rows.run_if("value `a + N`, new text each call", calls, || {
        for i in 0..calls {
            black_box(script::eval_value(&format!("a + {}", start + i)).expect("new text"));
        }
    });
}

fn scripts(rows: &mut Rows, calls: usize) {
    let run = |rows: &mut Rows, label: &str, source: &str| {
        if !row_wanted(label) {
            return;
        }
        rows.run(label, calls, || {
            for _ in 0..calls {
                script::run_script(source).unwrap_or_else(|err| panic!("{label}: {err}"));
            }
        });
    };
    run(rows, "empty script", ";");
    run(
        rows,
        "1000 adds to a local",
        "(function () { let s = 0; for (let i = 0; i < 1000; i++) s += i; return s })()",
    );
    run(
        rows,
        "1000 adds to a bare name (context)",
        "total = 0; for (let i = 0; i < 1000; i++) total += i",
    );
    run(
        rows,
        "100 objects made and walked, locals",
        "(function () { const l = []; for (let i = 0; i < 100; i++) l.push({id: 'I' + i, qty: i % 7}); \
         let s = 0; for (const o of l) if (o.qty > 2) s += o.qty; return s })()",
    );
    run(
        rows,
        "walk itemList, 100 context maps",
        "(function () { let s = 0; for (const o of itemList) if (o.qty > 2) s += o.qty; return s })()",
    );
    run(
        rows,
        "100 reads of item.name (context map)",
        "(function () { let n = 0; for (let i = 0; i < 100; i++) n += item.name.length; return n })()",
    );
    run(
        rows,
        "100 decimal multiply + add",
        "(function () { let s = price.subtract(price); for (let i = 0; i < 100; i++) s = s.add(price.multiply(dqty)); \
         return s })()",
    );
    run(
        rows,
        "100 ec.user.nowTimestamp (host getter)",
        "(function () { let t; for (let i = 0; i < 100; i++) t = ec.user.nowTimestamp; return t })()",
    );
    run(
        rows,
        "10 cached entity finds",
        "(function () { let c = 0; for (let i = 0; i < 10; i++) { \
         const v = ec.entity.find('moqui.basic.Enumeration').condition('enumId', '_NA_').useCache(true).one(); \
         if (v && v.description) c++ } return c })()",
    );
    run(
        rows,
        "walk an entity list, 100 cached rows",
        "(function () { const l = ec.entity.find('moqui.basic.Enumeration').condition('enumTypeId', 'JS_SPEED') \
         .useCache(true).list(); let n = 0; for (let i = 0; i < l.length; i++) n += l[i].description.length; \
         return n })()",
    );
    run(
        rows,
        "100 cache gets (a host method, a text argument)",
        "(function () { const c = ec.cache.getCache('js.speed'); let n = 0; for (let i = 0; i < 100; i++) \
         if (c.get('k') == null) n++; return n })()",
    );
    run(
        rows,
        "10 noop service calls",
        "(function () { for (let i = 0; i < 10; i++) \
         ec.service.sync().name('jsspeed.JsSpeedServices.noop#Inline').call(); return 0 })()",
    );
    run(
        rows,
        "10 noop service calls, itemList tracked",
        "(function () { const n = itemList.length; for (let i = 0; i < 10; i++) \
         ec.service.sync().name('jsspeed.JsSpeedServices.noop#Inline').call(); return n })()",
    );
    run(
        rows,
        "1000 string concatenations",
        "(function () { let s = ''; for (let i = 0; i < 1000; i++) s += 'x' + i; return s.length })()",
    );
    run(
        rows,
        "JSON stringify + parse of 100 objects",
        "(function () { const l = []; for (let i = 0; i < 100; i++) l.push({id: 'I' + i, qty: i, name: 'Item ' + i}); \
         return JSON.parse(JSON.stringify(l)).length })()",
    );
    // The rows of Profiling2's Tier 0, each for one path of the engine the templates and screens use.
    run(rows, "$esc over 100 strings", &esc_source());
    // The text the locked template's sandbox parses on each render: the snapshot of its row's data.
    let snapshot =
        template::unwrap_json(&script::eval_value("({title: title, items: items})").expect("data")).to_string();
    let literal = serde_json::to_string(&snapshot).expect("snapshot literal");
    run(
        rows,
        "JSON.parse of the locked snapshot",
        &format!("(function () {{ const d = JSON.parse({literal}); return d.items.length }})()"),
    );
    run(
        rows,
        "1000 numbers to text",
        "(function () { let n = 0; for (let i = 0; i < 1000; i++) { \
         const x = i % 4 === 0 ? i : i % 4 === 1 ? i / 7 : i % 4 === 2 ? i * 0.1 : i * 1000; \
         n += (i & 1 ? String(x) : '' + x).length } return n })()",
    );
    run(
        rows,
        "split('') + charAt, 1000 chars",
        "(function () { const s = 'abcdefghij'.repeat(100); const a = s.split(''); let n = 0; \
         for (let i = 0; i < s.length; i++) if (s.charAt(i) === a[i]) n++; return n })()",
    );
    run(
        rows,
        "regexp scan of a 200-line log",
        "(function () { const t = ('2026-11-08 10:00:01 INFO ServiceRun: 12\\n' + \
         '2026-11-08 10:00:02 ERROR EntityFind: 404\\n' + '2026-11-08 10:00:03 DEBUG Cache hit for key: 7\\n' + \
         '2026-11-08 10:00:04 WARN SlowQuery: 1500\\n' + '2026-11-08 10:00:05 ERROR ServiceCall: 500\\n').repeat(40); \
         let n = 0; for (const m of t.matchAll(/ERROR (\\w+): (\\d+)/g)) n += m[2].length + m[1].length; \
         return n + t.match(/\\d*:/g).length })()",
    );
}

/// `$esc` of the fancy templates' prelude, copied from `PRELUDE` in `moqui-core/src/script/template.rs`.
const ESC: &str = r#"const $esc = (v) => $String(v).replace(/[&<>"']/g, (c) => c === "&" ? "&amp;" : c === "<" ? "&lt;" : c === ">" ? "&gt;" : c === '"' ? "&quot;" : "&#39;");"#;

/// [`ESC`] over 100 literal strings, of which 10 have a character to escape.
fn esc_source() -> String {
    let mut list = String::new();
    for index in 0..100 {
        if index % 10 == 0 {
            list.push_str(&format!("'Item <{index}> & \"co\"', "));
        } else {
            list.push_str(&format!("'Widget {index} blue, large', "));
        }
    }
    format!(
        "(function () {{ const $String = String; {ESC} \
         const l = [{list}]; let n = 0; for (let i = 0; i < l.length; i++) n += $esc(l[i]).length; return n }})()"
    )
}

fn actions(rows: &mut Rows, calls: usize) {
    let run = |rows: &mut Rows, label: &str, xml: &str| {
        if !row_wanted(label) {
            return;
        }
        rows.run(label, calls, || {
            for _ in 0..calls {
                script::run_actions(xml).unwrap_or_else(|err| panic!("{label}: {err}"));
            }
        });
    };
    run(rows, "empty actions", "<actions/>");
    run(
        rows,
        "one set",
        r#"<actions><set field="textOut1" from="textIn1"/></actions>"#,
    );
    run(rows, "four sets (the echo#Data body)", ECHO_ACTIONS);
    run(rows, "iterate 100 context maps with if and set", LOOP_ACTIONS);
    run(
        rows,
        "entity-find-one, cached",
        r#"<actions><entity-find-one entity-name="moqui.basic.Enumeration" value-field="found" cache="true">
            <field-map field-name="enumId" value="_NA_"/></entity-find-one></actions>"#,
    );
}

fn services(rows: &mut Rows, calls: usize) {
    let echo = || {
        [
            ("textIn1", Value::string("foo")),
            ("numberIn", Value::string("12.34")),
            ("timestampIn", Value::Timestamp(jiff::Timestamp::now())),
        ]
    };
    let run = |rows: &mut Rows, label: &str, name: &str, parameters: &[(&str, Value)]| {
        if !row_wanted(label) {
            return;
        }
        let parameters: BTreeMap<String, Value> = parameters
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect();
        rows.run(label, calls, || {
            for _ in 0..calls {
                black_box(moqui_core::service::sync(name, &parameters).unwrap_or_else(|err| panic!("{name}: {err}")));
            }
        });
    };
    let name = |verb_noun: &str| format!("{SERVICES}.{verb_noun}");
    run(
        rows,
        "noop#Inline (empty actions)",
        &name("noop#Inline"),
        &[("textIn1", Value::string("foo"))],
    );
    run(
        rows,
        "set#One (one set)",
        &name("set#One"),
        &[("textIn1", Value::string("foo"))],
    );
    run(rows, "echo#Inline (four sets)", &name("echo#Inline"), &echo());
    run(
        rows,
        "echo#Script (the same, a .js file)",
        &name("echo#Script"),
        &echo(),
    );
    run(
        rows,
        "BasicServices.echo#Data (typed parameters)",
        "org.moqui.impl.BasicServices.echo#Data",
        &echo(),
    );
    run(
        rows,
        "loop#Actions (100 items, XML actions)",
        &name("loop#Actions"),
        &[],
    );
    run(rows, "loop#Script (100 items, one script)", &name("loop#Script"), &[]);
    run(
        rows,
        "calc#Decimal (20 multiply + add)",
        &name("calc#Decimal"),
        &[("price", Value::string("12.50")), ("qty", Value::string("3"))],
    );
    run(
        rows,
        "call#Nested (service-call to set#One)",
        &name("call#Nested"),
        &[("textIn1", Value::string("foo"))],
    );
    run(rows, "find#Cached (10 cached finds)", &name("find#Cached"), &[]);
}

fn templates(rows: &mut Rows, calls: usize, source: &str) {
    rows.run_if("fancy, trusted (live context)", calls, || {
        for _ in 0..calls {
            black_box(template::render_trusted(source, template::Escape::Html).expect("trusted"));
        }
    });
    let data = script::eval_value("({title: title, items: items})").expect("data");
    rows.run_if("fancy, locked down (a snapshot)", calls, || {
        for _ in 0..calls {
            black_box(template::render_locked_with(source, template::Escape::Html, &data).expect("locked"));
        }
    });
}

fn screens(rows: &mut Rows, calls: usize, factory: &Arc<ExecutionContextFactory>, root: &str) {
    for (label, path, mode, row_count) in [
        ("Plain, html (one label)", "/Plain", "html", None),
        ("Labels, html (40 expanded labels)", "/Labels", "html", None),
        ("Single, html (form-single, 11 fields)", "/Single", "html", None),
        ("Single.qvt (form-single, 11 fields)", "/Single.qvt", "qvt", None),
        ("List, html, 20 rows", "/List", "html", Some("20")),
        ("List, html, 100 rows", "/List", "html", Some("100")),
        ("Plain.qvt", "/Plain.qvt", "qvt", None),
        ("List.qvt, 100 rows", "/List.qvt", "qvt", Some("100")),
        ("List, text, 100 rows", "/List", "text", Some("100")),
        ("List, tui, 100 rows", "/List", "tui", Some("100")),
        ("Links, html, 100 rows", "/Links", "html", Some("100")),
        ("Links.qvt, 100 rows", "/Links.qvt", "qvt", Some("100")),
        ("Links, tui, 100 rows", "/Links", "tui", Some("100")),
    ] {
        if !row_wanted(label) {
            continue;
        }
        let parameters: BTreeMap<String, Value> = row_count
            .map(|count| [("rowCount".to_string(), Value::string(count))].into_iter().collect())
            .unwrap_or_default();
        rows.run(label, calls, || {
            for _ in 0..calls {
                black_box(render(factory, root, path, mode, &parameters));
            }
        });
    }
}

fn render(
    factory: &Arc<ExecutionContextFactory>,
    root: &str,
    path: &str,
    mode: &str,
    parameters: &BTreeMap<String, Value>,
) -> usize {
    let output = moqui_core::screen::make_render(factory)
        .root_screen(root)
        .screen_path(path)
        .render_mode(mode)
        .parameters(parameters.clone())
        .render()
        .unwrap_or_else(|err| panic!("{path} {mode}: {err}"));
    assert_eq!(output.status, 200, "{path} {mode}: {}", output.body_text());
    if let Some(dir) = std::env::var_os("JS_SPEED_DUMP") {
        let rows = parameters
            .get("rowCount")
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default();
        let file = format!("{}.{mode}{}.out", path.trim_start_matches('/').replace('/', "_"), rows);
        std::fs::write(PathBuf::from(dir).join(file), &output.body).expect("dump");
    }
    if std::env::var_os("JS_SPEED_PEEK").is_some() {
        let text = output.body_text();
        eprintln!(
            "--- {path} {mode} {:?} {} bytes\n{}",
            parameters,
            text.len(),
            &text[..text.len().min(1500)]
        );
    }
    output.body.len()
}

/// What a new thread pays for its engine: the first script, the first screen render, and the heap each leaves. Each
/// row is one call on each of the threads; `JS_SPEED_ROW` selects them as it does the others (the renders still run
/// after a first script, which starts the engine).
fn cold(factory: &Arc<ExecutionContextFactory>, root: &str, sizes: &Sizes) {
    const START: &str = "engine start (a thread's first script)";
    const RENDER_FIRST: &str = "then first render, List html 20 rows";
    const RENDER_SECOND: &str = "then a second screen, Plain html";
    let renders = row_wanted(RENDER_FIRST) || row_wanted(RENDER_SECOND);
    if !row_wanted(START) && !renders {
        return;
    }
    let threads = (sizes.screens / 10).clamp(5, 50);
    println!("\n== a new thread's first script and first screen, {threads} threads one after another");
    let mut rows = Rows {
        js: true,
        record: true,
        ..Rows::default()
    };
    let mut after_script = EngineMemory::default();
    let mut after_render = EngineMemory::default();
    let mut after_second = EngineMemory::default();
    for _ in 0..threads {
        let (measured, memory) = std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    let context = factory.new_context();
                    let _entered = context.enter();
                    let _authz = context.disable_authz();
                    let no_parameters = BTreeMap::new();
                    let twenty: BTreeMap<String, Value> =
                        [("rowCount".to_string(), Value::string("20"))].into_iter().collect();
                    let first = speed::measure(|| script::run_script(";").expect("first script")).0;
                    let script_memory = script::engine_memory(true);
                    if !renders {
                        return (
                            [Some(first), None, None],
                            [script_memory, EngineMemory::default(), EngineMemory::default()],
                        );
                    }
                    let render_first = speed::measure(|| render(factory, root, "/List", "html", &twenty)).0;
                    let render_memory = script::engine_memory(true);
                    let render_second = speed::measure(|| render(factory, root, "/Plain", "html", &no_parameters)).0;
                    let second_memory = script::engine_memory(true);
                    (
                        [Some(first), Some(render_first), Some(render_second)],
                        [script_memory, render_memory, second_memory],
                    )
                })
                .join()
                .expect("cold thread")
        });
        for (label, measured) in [START, RENDER_FIRST, RENDER_SECOND].into_iter().zip(measured) {
            if let Some(measured) = measured.filter(|_| row_wanted(label)) {
                rows.note(label, 1, measured);
            }
        }
        [after_script, after_render, after_second] = memory;
    }
    rows.print();
    print_memory("a new thread's engine after its first script", after_script);
    if renders {
        print_memory("the same after List html", after_render);
        print_memory("the same after Plain html", after_second);
    }
}

/// The process's scripts by kind and its runtimes.
fn print_engine_stats() {
    let stats = script::engine_stats();
    let ran: Vec<String> = stats
        .scripts_by_kind
        .iter()
        .filter(|(_, runs)| *runs > 0)
        .map(|(kind, runs)| format!("{kind} {runs}"))
        .collect();
    println!(
        "\n  scripts by kind: {}; runtimes started {}, texts compiled {}, heap {:.0} KiB (most {:.0} KiB)",
        ran.join(", "),
        stats.oxide_engines_started,
        stats.oxide_compiled,
        stats.oxide_heap_bytes as f64 / 1024.0,
        stats.oxide_heap_bytes_max as f64 / 1024.0,
    );
}

fn print_memory(label: &str, memory: EngineMemory) {
    let kib = |bytes: usize| bytes as f64 / 1024.0;
    println!(
        "\n  {label}: {} runtime, heap reserves {:.0} KiB ({:.0} KiB live at the last collection); {} code units \
         ({:.0} KiB of instructions, positions and source); cached: {} compiled expressions ({:.0} KiB source)",
        memory.oxide_engines,
        kib(memory.oxide_reserved_bytes),
        kib(memory.oxide_live_bytes),
        memory.oxide_code_units,
        kib(memory.oxide_code_bytes),
        memory.oxide_compiled_scripts,
        kib(memory.oxide_compiled_script_bytes),
    );
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("create fixture copy");
    for entry in std::fs::read_dir(from).expect("read fixture") {
        let entry = entry.expect("fixture entry");
        let target = to.join(entry.file_name());
        if entry.file_type().expect("file type").is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copy fixture file");
        }
    }
}

fn seed(facade: &EntityFacade) {
    facade.begin(ENUM, None).expect("begin seed");
    let mut kind = facade.make_value("moqui.basic.EnumerationType").expect("type");
    kind.set("enumTypeId", Value::string("_NA_"));
    kind.set("description", Value::string("Not Applicable"));
    kind.create(facade).expect("create type");
    let mut row = facade.make_value(ENUM).expect("enum");
    row.set("enumId", Value::string("_NA_"));
    row.set("enumTypeId", Value::string("_NA_"));
    row.set("description", Value::string("Not Applicable"));
    row.create(facade).expect("create enum");
    // the rows `walk an entity list` reads
    let mut kind = facade.make_value("moqui.basic.EnumerationType").expect("type");
    kind.set("enumTypeId", Value::string("JS_SPEED"));
    kind.set("description", Value::string("js_speed rows"));
    kind.create(facade).expect("create type");
    for index in 0..100 {
        let mut row = facade.make_value(ENUM).expect("enum");
        row.set("enumId", Value::string(format!("JS_SPEED_{index:03}")));
        row.set("enumTypeId", Value::string("JS_SPEED"));
        row.set("description", Value::string(format!("Speed row {index}")));
        row.create(facade).expect("create enum");
    }
    facade.commit(ENUM).expect("commit seed");
}

fn pool() -> PoolConfig {
    PoolConfig {
        min: 1,
        max: 2,
        reserve: 1,
        acquire_timeout: Duration::from_secs(30),
        ..Default::default()
    }
}
