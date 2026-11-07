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

//! Golden tests for the fancy dialect. A fixture `<name>.fancy` has its data in `<name>.json`, its
//! generated statements in `<name>.js` and its output in `<name>.out` (`<name>.html.out` with HTML
//! escaping). Both renderers must give the output. `UPDATE_GOLDEN=1` rewrites the expected files.

use std::path::PathBuf;

use moqui_lib::object::Value;

use super::*;

/// Run a test body under `rquickjs` and under `oxide` (plans/Script-Engine-Oxide.md section 8): templates stay on
/// QuickJS under `small-oxide`, so `oxide` is the setting that runs them on quickjs-oxide. A failure names the engine.
fn on_template_engines(body: fn()) {
    use crate::script::engine::{with_script_engine, ScriptEngine};
    for engine in [ScriptEngine::Rquickjs, ScriptEngine::Oxide] {
        let ran = std::panic::catch_unwind(|| with_script_engine(engine, body));
        if let Err(payload) = ran {
            let message = payload
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_string()))
                .unwrap_or_default();
            panic!("under {}: {message}", engine.name());
        }
    }
}

fn testdata() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src/script/testdata/template")
}

fn expect(file: &str, actual: &str) {
    let path = testdata().join(file);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::write(&path, actual).unwrap();
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|_| panic!("missing {}", path.display()));
    assert_eq!(actual, expected, "{file}");
}

fn data(name: &str) -> Value {
    let text = std::fs::read_to_string(testdata().join(format!("{name}.json"))).unwrap();
    moqui_lib::json::from_json(&serde_json::from_str(&text).unwrap())
}

fn check(name: &str, escape: Escape, out_file: &str) {
    let source = std::fs::read_to_string(testdata().join(format!("{name}.fancy"))).unwrap();
    if escape == Escape::None {
        expect(&format!("{name}.js"), &compile_body(&source, escape).unwrap());
    }
    let values = data(name);
    let trusted = FancyRenderer { escape }.render(&source, Some(&values)).unwrap();
    let locked = LockedRenderer { escape }.render(&source, Some(&values)).unwrap();
    assert_eq!(trusted, locked, "{name}: trusted and locked-down differ");
    expect(out_file, &trusted);
}

#[test]
fn golden_if() {
    on_template_engines(golden_if_on);
}

fn golden_if_on() {
    check("if", Escape::None, "if.out");
}

#[test]
fn golden_list() {
    on_template_engines(golden_list_on);
}

fn golden_list_on() {
    check("list", Escape::None, "list.out");
}

#[test]
fn golden_assign_and_comments() {
    on_template_engines(golden_assign_and_comments_on);
}

fn golden_assign_and_comments_on() {
    check("assign", Escape::None, "assign.out");
}

#[test]
fn golden_interpolation_and_escape_modes() {
    on_template_engines(golden_interpolation_and_escape_modes_on);
}

fn golden_interpolation_and_escape_modes_on() {
    check("interp", Escape::None, "interp.out");
    check("interp", Escape::Html, "interp.html.out");
}

#[test]
fn compile_errors_name_the_line() {
    for (source, needle) in [
        ("a\n<#if x>\nb", "<#if> is not closed"),
        ("</#if>", "without an open"),
        ("<#else>", "without an open"),
        ("x\n<#visit node>", "line 2: unknown tag <#visit>"),
        ("<@macro/>", "macro calls"),
        ("<#list items>x</#list>", "expression as name"),
        ("<#assign x>", "name = expression"),
        ("${a b}", "not JavaScript"),
        ("${unclosed", "no closing"),
        ("<#if def x = 1>y</#if>", "not JavaScript"),
    ] {
        let err = check_template(source).expect_err(source);
        assert!(err.message().contains(needle), "{source}: {}", err.message());
    }
}

#[test]
fn tag_only_lines_leave_no_blank_lines() {
    on_template_engines(tag_only_lines_leave_no_blank_lines_on);
}

fn tag_only_lines_leave_no_blank_lines_on() {
    let out = render_trusted("a\n  <#if true>\nb\n  </#if>\nc\n", Escape::None).unwrap();
    assert_eq!(out, "a\nb\nc\n");
}

#[test]
fn trusted_reads_the_live_context_and_locked_down_cannot_reach_ec() {
    on_template_engines(trusted_reads_the_live_context_and_locked_down_cannot_reach_ec_on);
}

fn trusted_reads_the_live_context_and_locked_down_cannot_reach_ec_on() {
    let probe = "${typeof ec} ${typeof sri} ${typeof context} ${typeof globalThis.ec}";
    // `sri` and the stack as `context` are not bound on quickjs-oxide yet (`check::QUICKJS_ONLY_GLOBALS`)
    let on_oxide = crate::script::engine::script_engine() == crate::script::engine::ScriptEngine::Oxide;
    let trusted = render_trusted(if on_oxide { "${typeof ec} ${typeof ec}" } else { probe }, Escape::None).unwrap();
    assert!(trusted.starts_with("object object"), "{trusted}");
    let locked = render_locked_with(probe, Escape::None, &Value::Map(Default::default())).unwrap();
    // A name that is not in the snapshot is null, which is an object.
    assert_eq!(locked, "object object object undefined");
    let err = render_locked_with(
        "${ec.entity.find('Enumeration')}",
        Escape::None,
        &Value::Map(Default::default()),
    )
    .expect_err("ec is null");
    assert!(
        err.message().contains("null") || err.message().contains("not an object"),
        "{}",
        err.message()
    );
    let escape_attempt = "${[].constructor.constructor('return typeof ec')()}";
    assert_eq!(
        render_locked_with(escape_attempt, Escape::None, &Value::Map(Default::default())).unwrap(),
        "undefined"
    );
}

/// The snapshot form has no `with`: each name the template uses is a variable of its function, from the snapshot, so
/// an arrow sees it, an assignment changes it for the rest of the render, and code made at run time does not see it.
#[test]
fn locked_down_names_are_variables_of_the_template_function() {
    on_template_engines(locked_down_names_are_variables_of_the_template_function_on);
}

fn locked_down_names_are_variables_of_the_template_function_on() {
    let compiled = compiled("${price}", Escape::None, Scope::Snapshot).unwrap();
    assert!(!compiled.contains("with ("), "{compiled}");
    let data = moqui_lib::json::from_json(&serde_json::json!({"price": 2, "toString": "data"}));
    let out = render_locked_with(
        "${(() => price * 2)()}|${price = 5}${price}|<#assign total = price + 1>${total}|\
         ${Function('return typeof price')()}|${typeof toString}|${missing}${typeof missing}",
        Escape::None,
        &data,
    )
    .unwrap();
    assert_eq!(out, "4|55|6|undefined|function|object");
}

/// The template function is kept for every render on the thread: one render cannot leave a value on it, its
/// `arguments` or its `this` for the next (one user's render read by another's).
#[test]
fn a_locked_down_render_leaves_nothing_on_the_template_function() {
    on_template_engines(a_locked_down_render_leaves_nothing_on_the_template_function_on);
}

fn a_locked_down_render_leaves_nothing_on_the_template_function_on() {
    let alice = moqui_lib::json::from_json(&serde_json::json!({"secret": "alice"}));
    let bob = Value::Map(Default::default());
    let source = "<#if secret != null>${(() => { try { arguments.callee.s = secret; return 'kept' } \
                  catch (e) { return 'refused' } })()}${(() => { try { $t.s = secret; return 'kept' } \
                  catch (e) { return 'refused' } })()}<#else>${(() => { try { return String(arguments.callee.s) } \
                  catch (e) { return 'refused' } })()}${String($t.s)}${typeof this}\
                  </#if>";
    assert_eq!(render_locked_with(source, Escape::None, &alice).unwrap(), "refusedrefused");
    assert_eq!(
        render_locked_with(source, Escape::None, &bob).unwrap(),
        "refusedundefinedundefined"
    );
}

#[test]
fn locked_down_sees_plain_values_and_decimals_as_strings() {
    on_template_engines(locked_down_sees_plain_values_and_decimals_as_strings_on);
}

fn locked_down_sees_plain_values_and_decimals_as_strings_on() {
    use std::str::FromStr;
    let mut entries = indexmap::IndexMap::new();
    entries.insert(
        "amount".to_string(),
        Value::Decimal(moqui_lib::format::Decimal::from_str("10.50").unwrap()),
    );
    entries.insert("count".to_string(), Value::I64(3));
    let data = Value::Map(entries);
    let out = render_locked_with(
        "${typeof amount} ${amount} ${amount < '9'} ${typeof count} ${count + 1}",
        Escape::None,
        &data,
    )
    .unwrap();
    assert_eq!(out, "string 10.50 true number 4");
}

#[test]
fn locked_down_format_and_localize_are_plain_functions() {
    on_template_engines(locked_down_format_and_localize_are_plain_functions_on);
}

fn locked_down_format_and_localize_are_plain_functions_on() {
    let data = moqui_lib::json::from_json(&serde_json::json!({"n": 1234.5, "when": "2024-03-05 10:20:30.0"}));
    let out = render_locked_with(
        "${format(n, '#,##0.00')} ${format(when, 'yyyy-MM-dd')} ${localize('Hello')}",
        Escape::None,
        &data,
    )
    .unwrap();
    assert_eq!(out, "1,234.50 2024-03-05 Hello");
}

/// A theme render's `t(key)` texts last only as long as that render, also when it fails: a later locked render on the
/// thread reads the key itself.
#[test]
fn locked_down_texts_end_with_their_render() {
    on_template_engines(locked_down_texts_end_with_their_render_on);
}

fn locked_down_texts_end_with_their_render_on() {
    let texts = Arc::new(Texts::from([("a".to_string(), "<A>".to_string())]));
    let deadline = Instant::now() + LOCKED_TIME_LIMIT;
    let out = render_locked_bounded(
        "${t('a')}|${t('b')}|${t(1)}",
        "{}",
        &texts,
        deadline,
        LOCKED_MEMORY_LIMIT,
    )
    .unwrap();
    assert_eq!(out, "&lt;A&gt;|b|");
    assert!(render_locked_bounded("${t('a')}${nothing()}", "{}", &texts, deadline, LOCKED_MEMORY_LIMIT).is_err());
    let empty = Value::Map(Default::default());
    assert_eq!(render_locked_with("${t('a')}", Escape::None, &empty).unwrap(), "a");
}

#[test]
fn a_locked_down_template_cannot_change_what_the_next_one_runs() {
    on_template_engines(a_locked_down_template_cannot_change_what_the_next_one_runs_on);
}

fn a_locked_down_template_cannot_change_what_the_next_one_runs_on() {
    let empty = Value::Map(Default::default());
    // Each of these would change the shared context for later renders if it held. The template is strict, so each
    // assignment to a frozen object throws.
    for attempt in [
        "${String.prototype.replace = function () { return 'x' }}",
        "${JSON.parse = null}",
        "${globalThis.leak = 'kept'}",
        "${Array.prototype.join = null}",
        "${Object.getPrototypeOf([][Symbol.iterator]()).next = null}",
    ] {
        let err = render_locked_with(attempt, Escape::None, &empty).expect_err(attempt);
        assert!(err.message().contains("Error"), "{attempt}: {}", err.message());
    }
    let mut entries = indexmap::IndexMap::new();
    entries.insert("v".to_string(), Value::string("<b>"));
    let out = render_locked_with(
        "${v}|${globalThis.leak == null}|<#list ['a', 'b'] as x>${x}</#list>",
        Escape::Html,
        &Value::Map(entries),
    )
    .unwrap();
    assert_eq!(out, "&lt;b&gt;|true|ab");
}

#[test]
fn locked_down_format_reads_a_decimal_text_by_a_number_pattern_and_a_date_by_a_date_pattern() {
    on_template_engines(locked_down_format_reads_a_decimal_text_by_a_number_pattern_and_a_date_by_a_date_pattern_on);
}

fn locked_down_format_reads_a_decimal_text_by_a_number_pattern_and_a_date_by_a_date_pattern_on() {
    use std::str::FromStr;
    let mut entries = indexmap::IndexMap::new();
    entries.insert(
        "qty".to_string(),
        Value::Decimal(moqui_lib::format::Decimal::from_str("100").unwrap()),
    );
    entries.insert(
        "day".to_string(),
        Value::Date(moqui_lib::object::parse_date("2024-03-05").unwrap()),
    );
    let out = render_locked_with(
        "${format(qty, '#,##0.00')} ${format(day, 'yyyy-MM-dd')}",
        Escape::None,
        &Value::Map(entries),
    )
    .unwrap();
    assert_eq!(out, "100.00 2024-03-05");
}

#[test]
fn locked_down_stops_a_runaway_loop() {
    on_template_engines(locked_down_stops_a_runaway_loop_on);
}

fn locked_down_stops_a_runaway_loop_on() {
    let started = std::time::Instant::now();
    let err = render_locked_with(
        "${(() => { while (true) {} })()}",
        Escape::None,
        &Value::Map(Default::default()),
    )
    .expect_err("interrupted");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(30),
        "{}",
        err.message()
    );
    assert!(err.message().contains("interrupted"), "{}", err.message());
}

/// A theme render's lower memory limit holds for that render only; a limit of 0 renders nothing.
#[test]
fn locked_down_memory_is_lowered_for_one_render() {
    on_template_engines(locked_down_memory_is_lowered_for_one_render_on);
}

fn locked_down_memory_is_lowered_for_one_render_on() {
    let texts = Arc::new(Texts::new());
    let deadline = Instant::now() + LOCKED_TIME_LIMIT;
    let hungry = "${'x'.repeat(4000000).length}";
    assert!(render_locked_bounded(hungry, "{}", &texts, deadline, 2_000_000).is_err());
    assert!(render_locked_bounded("${1}", "{}", &texts, deadline, 0).is_err());
    let out = render_locked_bounded(hungry, "{}", &texts, deadline, LOCKED_MEMORY_LIMIT).unwrap();
    assert_eq!(out, "4000000");
}

#[test]
fn compiled_functions_are_cached_by_source() {
    let source = "cache probe ${1 + 1}";
    let first = compiled(source, Escape::None, Scope::Live).unwrap();
    let second = compiled(source, Escape::None, Scope::Live).unwrap();
    assert!(Arc::ptr_eq(&first, &second));
    let html = compiled(source, Escape::Html, Scope::Live).unwrap();
    assert!(!Arc::ptr_eq(&first, &html));
}

#[test]
fn registry_picks_the_longest_extension() {
    on_template_engines(registry_picks_the_longest_extension_on);
}

fn registry_picks_the_longest_extension_on() {
    let registry = TemplateRegistry::parse(
        r#"<moqui-conf><resource-facade>
        <template-renderer extension="fancy" class="fancy"/>
        <template-renderer extension="html.fancy" class="fancy-html"/>
        <template-renderer extension="ftl" class="org.moqui.impl.context.renderer.FtlTemplateRenderer"/>
        </resource-facade></moqui-conf>"#,
    );
    assert!(registry.renderer_for("component://x/a.fancy").is_some());
    assert!(registry.renderer_for("component://x/a.html.fancy").is_some());
    assert!(registry.renderer_for("component://x/a.ftl").is_none());
    let html = registry
        .renderer_for("component://x/a.html.fancy")
        .unwrap()
        .render(
            "${v}",
            Some(&moqui_lib::json::from_json(&serde_json::json!({"v": "<"}))),
        )
        .unwrap();
    assert_eq!(html, "&lt;");
}

#[test]
fn markdown_renders_html_and_passes_raw_html_through() {
    let registry = TemplateRegistry::parse("<moqui-conf/>");
    let renderer = registry.renderer_for("component://x/page.md").expect("md renderer");
    let html = renderer
        .render(
            "# Title\n\nSome *text* and <span class=\"k\">raw</span> & ${notAnInterp}\n",
            None,
        )
        .unwrap();
    assert!(html.contains("<h1>Title</h1>"), "{html}");
    assert!(html.contains("<em>text</em>"), "{html}");
    assert!(html.contains("<span class=\"k\">raw</span>"), "{html}");
    assert!(html.contains("&amp; ${notAnInterp}"), "{html}");
    assert!(registry.renderer_for("component://x/page.markdown").is_some());
    // a markdown file is not a template, so there is nothing to check
    assert!(renderer.check("<#if broken").is_ok());
}

#[test]
fn markdown_fancy_converts_markdown_then_runs_the_template() {
    on_template_engines(markdown_fancy_converts_markdown_then_runs_the_template_on);
}

fn markdown_fancy_converts_markdown_then_runs_the_template_on() {
    let registry = TemplateRegistry::parse(
        r#"<moqui-conf><resource-facade>
        <template-renderer extension="fancy" class="fancy"/>
        <template-renderer extension="md" class="markdown"/>
        <template-renderer extension="md.fancy" class="markdown-fancy"/>
        </resource-facade></moqui-conf>"#,
    );
    assert!(registry.renderer_for("component://x/page.md").is_some());
    let renderer = registry
        .renderer_for("component://x/page.md.fancy")
        .expect("md.fancy renderer");
    let source = "# Hello ${name}\n\n<#if (items.length > 0) && show>\n\n<#list items as item>\n\n- **${item}** \\${kept}\n\n</#list>\n\n</#if>\n\n[link](${url})\n";
    // markdown would escape the `>` and `&&` of the tag and drop the backslash of the escape, and a
    // tag alone in a paragraph loses the paragraph
    let prepared = markdown_fancy_source(source).unwrap();
    assert!(prepared.contains("<#if (items.length > 0) && show>\n"), "{prepared}");
    assert!(prepared.contains("\\${kept}"), "{prepared}");
    assert!(
        !prepared.contains("<p><#") && !prepared.contains("<p></#"),
        "{prepared}"
    );
    let data = moqui_lib::json::from_json(&serde_json::json!({
        "name": "<World>", "items": ["a", "b"], "show": true, "url": "/x?a=1&b=2"
    }));
    let html = renderer.render(source, Some(&data)).unwrap();
    assert!(html.contains("<h1>Hello &lt;World&gt;</h1>"), "{html}");
    assert!(html.contains("<li><strong>a</strong> ${kept}</li>"), "{html}");
    assert!(html.contains("<li><strong>b</strong> ${kept}</li>"), "{html}");
    assert!(html.contains("<a href=\"/x?a=1&amp;b=2\">link</a>"), "{html}");
    renderer.check(source).unwrap();
    assert!(renderer.check("<#if x>\n\nno end\n").is_err());
    assert!(markdown_fancy_source("${unclosed").is_err());
}

#[test]
fn markdown_fancy_tag_lines_in_a_table_do_not_become_rows() {
    on_template_engines(markdown_fancy_tag_lines_in_a_table_do_not_become_rows_on);
}

fn markdown_fancy_tag_lines_in_a_table_do_not_become_rows_on() {
    // markdown makes a row of each line inside a table, so the list would repeat an empty row with
    // each item and the tag lines would leave empty rows of their own
    let source = "| Item | Qty |\n|---|--:|\n<#list items as item>\n| ${item.id} | ${item.qty} |\n</#list>\n\
        <#if hasTotal><#-- total row -->\n| **Total** | ${total} |\n</#if>\n| ${note} | |\n";
    let prepared = markdown_fancy_source(source).unwrap();
    assert!(prepared.contains("<tbody>\n<#list items as item>\n<tr>"), "{prepared}");
    assert!(
        prepared.contains("\n<#if hasTotal><#-- total row -->\n<tr>"),
        "{prepared}"
    );
    let data = moqui_lib::json::from_json(&serde_json::json!({
        "items": [{"id": "a", "qty": 1}, {"id": "b", "qty": 2}], "hasTotal": true, "total": 3, "note": "n"
    }));
    let html = MarkdownFancyRenderer.render(source, Some(&data)).unwrap();
    assert_eq!(html.matches("<tr>").count(), 5, "{html}");
    assert!(
        html.contains("<tr><td>a</td><td style=\"text-align: right\">1</td></tr>"),
        "{html}"
    );
    assert!(
        html.contains("<tr><td>b</td><td style=\"text-align: right\">2</td></tr>"),
        "{html}"
    );
    assert!(
        html.contains("<tr><td><strong>Total</strong></td><td style=\"text-align: right\">3</td></tr>"),
        "{html}"
    );
    // a row of only `${}` is a row
    assert!(
        html.contains("<tr><td>n</td><td style=\"text-align: right\"></td></tr>"),
        "{html}"
    );
}

#[test]
fn css_string_quotes_text_for_a_style_element() {
    on_template_engines(css_string_quotes_text_for_a_style_element_on);
}

fn css_string_quotes_text_for_a_style_element_on() {
    crate::script::clear_context_stack();
    crate::script::set_context_name("t", Value::string("a\"b\\c</style>\r\nd & e")).unwrap();
    let out = render_trusted(
        "<style>p { content: ${cssString(t)} counter(page); }</style>",
        Escape::Html,
    )
    .unwrap();
    assert_eq!(
        out,
        "<style>p { content: \"a\\22 b\\5C c\\3C /style\\3E \\D \\A d & e\" counter(page); }</style>"
    );
    assert_eq!(render_trusted("${cssString(null)}", Escape::Html).unwrap(), "\"\"");
    crate::script::clear_context_stack();
}

#[test]
fn assign_reads_the_context_value_before_it_and_its_own_value_after() {
    on_template_engines(assign_reads_the_context_value_before_it_and_its_own_value_after_on);
}

fn assign_reads_the_context_value_before_it_and_its_own_value_after_on() {
    crate::script::clear_context_stack();
    crate::script::set_context_name("n", Value::I32(5)).unwrap();
    let out = render_trusted("${n}<#assign n = n + 1>${n}", Escape::None).unwrap();
    assert_eq!(out, "56");
    crate::script::clear_context_stack();
}

#[test]
fn the_extension_set_lists_each_registered_extension_once_in_conf_order() {
    let registry = TemplateRegistry::parse("<moqui-conf/>");
    let extensions = registry.extensions();
    assert_eq!(extensions.first().map(String::as_str), Some("fancy"));
    assert!(extensions.iter().any(|extension| extension == "md"), "{extensions:?}");
    assert!(
        extensions.iter().any(|extension| extension == "md.fancy"),
        "{extensions:?}"
    );
    let mut unique = extensions.clone();
    unique.dedup();
    assert_eq!(unique, extensions);
}

#[test]
fn list_tags_split_at_the_last_as_word() {
    assert_eq!(split_last_word("a.b as item", "as"), Some(("a.b", "item")));
    assert_eq!(split_last_word("x as y as z", "as"), Some(("x as y", "z")));
    assert_eq!(split_last_word("items", "as"), None);
}

#[test]
fn tag_rows_take_the_first_cell_of_placeholders_alone() {
    let held = vec![
        Held {
            placeholder: "p0x".to_string(),
            original: "<#if a>".to_string(),
            is_tag: true,
        },
        Held {
            placeholder: "p1x".to_string(),
            original: "${b}".to_string(),
            is_tag: false,
        },
    ];
    assert_eq!(tag_row_text("<tr><td>p0x</td><td></td></tr>", "p", &held), Some("p0x"));
    assert_eq!(
        tag_row_text("<tr><td align=\"left\">p0x p0x</td></tr>", "p", &held),
        Some("p0x p0x")
    );
    // A cell that is not empty after the first, an expression placeholder, other text and a bad row are not tag rows.
    assert_eq!(tag_row_text("<tr><td>p0x</td><td>more</td></tr>", "p", &held), None);
    assert_eq!(tag_row_text("<tr><td>p1x</td></tr>", "p", &held), None);
    assert_eq!(tag_row_text("<tr><td>p0x text</td></tr>", "p", &held), None);
    assert_eq!(tag_row_text("<tr><td>p9x</td></tr>", "p", &held), None);
    assert_eq!(tag_row_text("<tr><td>p0x</tr>", "p", &held), None);
    assert_eq!(tag_row_text("<tr><td></td></tr>", "p", &held), None);
}
