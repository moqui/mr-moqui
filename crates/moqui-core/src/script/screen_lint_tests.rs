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

use super::*;

fn names(text: &str) -> Vec<&'static str> {
    classify(text).into_iter().map(|(idiom, _)| idiom.name()).collect()
}

#[test]
fn unnamed_containers_are_listed_as_anchors_a_screen_extend_cannot_use() {
    let text = r#"<screen><widgets>
    <container name="Named"><container><label text="x"/></container></container>
    <section name="S"><widgets/></section>
    <container-box id="Box"/>
    <container-row/>
</widgets></screen>"#;
    let file = super::lint_screen_text(std::path::Path::new("Anchors.xml"), text)
        .unwrap()
        .unwrap();
    assert_eq!(
        file.unnamed_anchors,
        [(2, "container".to_string()), (5, "container-row".to_string())]
    );
}

#[test]
fn context_plus_is_its_own_idiom() {
    assert!(names("context + [a: 1]").contains(&"context-plus"));
    assert!(names("context+[a:b]").contains(&"context-plus"));
    assert!(!names("mycontext + [a: 1]").contains(&"context-plus"));
    assert!(!names("Object.assign({}, context, {a: 1})").contains(&"context-plus"));
}

#[test]
fn elvis_and_safe_navigation_are_told_apart() {
    assert_eq!(names("a ?: 'x'"), vec!["elvis"]);
    assert_eq!(names("a?.b"), vec!["safe-navigation"]);
    assert!(names("a ? b : c").is_empty());
}

#[test]
fn comments_and_template_text_do_not_trigger_idioms_but_template_code_does() {
    assert!(names("// Java's list.size() as BigDecimal 1.5 ..\nx").is_empty());
    assert!(names("/* x as String; a..b */ y").is_empty());
    assert!(names("`as many as ${n}`").is_empty());
    assert!(names("`n: ${list.size()}`").contains(&"collection-method"));
    assert!(names("f('it\\'s') // don't\nlist.size()").contains(&"collection-method"));
    assert_eq!(mask_strings("a /* é */ b").len(), "a /* é */ b".len());
}

#[test]
fn valid_javascript_idioms_are_not_flagged_by_default() {
    let site = |text: &str| Site {
        line: 1,
        element: "script".into(),
        attribute: "text".into(),
        kind: SiteKind::Script,
        text: text.into(),
        error: None,
        idioms: classify(text),
    };
    assert!(!site("const s = new Set(); s.add(1); a?.b; try { f() } catch (e) {}").flagged(false));
    assert!(site("const s = new Set(); s.add(1)").flagged(true));
    assert!(site("list.size()").flagged(false));
    assert_eq!(Idiom::DecimalOperator.severity(), Severity::Hazard);
}

#[test]
fn strings_do_not_trigger_idioms() {
    assert!(names("'a ?: b => [x:1] new Foo()'").is_empty());
}

#[test]
fn ranges_closures_and_maps() {
    assert_eq!(names("list[1..size-1]"), vec!["range"]);
    assert!(names("[...a, ...b]").is_empty());
    assert!(names("items.each { it }").contains(&"closure"));
    assert!(names("items.map(x => x.id)").is_empty());
    assert_eq!(names("[a:1, b:2]"), vec!["map-literal"]);
    assert!(names("rows[ok ? 1 : 2]").is_empty());
    assert!(names("rows[0]").is_empty());
}

#[test]
fn gstrings_regex_decimals_and_types() {
    assert!(names("\"Hi ${name}\"").contains(&"gstring"));
    assert!(names("'Hi ${name}'").is_empty());
    assert_eq!(names("a =~ /x/"), vec!["regex-operator"]);
    assert!(names("price * 10g").contains(&"decimal-literal"));
    assert!(names("price * 1.5").contains(&"decimal-point-literal"));
    assert!(names("x.setScale(2)").contains(&"set-scale"));
    assert!(names("new BigDecimal(a)").contains(&"bigdecimal"));
    let new_type = classify("new TreeSet(a)");
    assert_eq!(new_type, vec![(Idiom::NewType, "TreeSet".to_string())]);
    assert_eq!(names("new Date(a)"), vec!["new-js-type"]);
    assert!(names("StringUtilities.camelCaseToPretty(x)").contains(&"static-class"));
    assert_eq!(names("ResourceReference.isTextFilename(x)"), vec!["host-static"]);
    assert!(names("Math.max(1, 2)").is_empty());
    // with the package it is a Java name the script does not have; a service name in a string is not
    assert!(names("org.moqui.resource.ResourceReference.isTextFilename(x)").contains(&"static-class"));
    assert!(names("new java.util.Date()").contains(&"static-class"));
    assert!(
        !names("ec.service.sync().name('org.moqui.impl.UserServices.update#UserAccount')").contains(&"static-class")
    );
}

#[test]
fn in_operator_collection_methods_and_casts() {
    assert!(names("x in ['A', 'B']").contains(&"in-operator"));
    assert!(names("'k' in obj").is_empty());
    assert!(names("list.size() > 0").contains(&"collection-method"));
    assert_eq!(names("total.add(x)"), vec!["add-call"]);
    assert_eq!(names("try { a() } catch (e) { b() }"), vec!["catch-bare"]);
    assert_eq!(names("try { a() } catch (Exception e) { b() }"), vec!["catch"]);
    assert!(names("x as String").contains(&"as-cast"));
    assert!(names("def x = 1").contains(&"def"));
}

#[test]
fn embedded_expressions_respect_braces_and_strings() {
    assert_eq!(embedded_expressions("a ${b} c ${d.e('}')}"), vec!["b", "d.e('}')"]);
    assert!(embedded_expressions("\\${literal}").is_empty());
    assert_eq!(dollar_name("cost $price"), Some(5));
    assert_eq!(dollar_name("cost $5 ${x}"), None);
}

#[test]
fn truthiness_candidates_are_bare_collection_names() {
    assert!(truthiness_candidate("condition", "itemList").is_some());
    assert!(truthiness_candidate("condition", "!itemList && x == 1").is_some());
    assert!(truthiness_candidate("condition", "itemList.length > 0").is_none());
    assert!(truthiness_candidate("from", "itemList").is_none());
}

fn workspace(path: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..").join(path)
}

fn write_screen(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-screen-lint-{}-{name}", std::process::id()));
    std::fs::create_dir_all(dir.join("screen")).unwrap();
    let path = dir.join("screen").join("Test.xml");
    std::fs::write(&path, format!("<screen>{body}</screen>")).unwrap();
    dir
}

#[test]
fn screen_trees_have_no_parse_failures() {
    let roots = [
        workspace("runtime/base-component"),
        workspace("runtime/dev-component/example"),
    ];
    let report = lint_screens(&roots).unwrap();
    assert!(report.files.len() > 100, "{} files", report.files.len());
    assert!(report.site_count() > 1000, "{} sites", report.site_count());
    assert_eq!(report.failures(), Vec::<String>::new());
    assert_eq!(report.excluded.len(), KNOWN_EXCLUSIONS.len());
    // The widget templates are scanned, and every template body in the trees compiles in the dialect.
    assert!(
        report.files.iter().any(
            |file| file.path.ends_with("webroot/template/screen/BasicWidgetTemplates.xml") && file.sites.len() > 30
        ),
        "BasicWidgetTemplates.xml was not scanned"
    );
    let bodies: usize = report.files.iter().map(|file| file.dialect_bodies).sum();
    assert!(bodies >= 30, "{bodies} template bodies");
}

/// The page shell, the login pages and the menu templates are files in the dialect. Each one compiles.
#[test]
fn every_fancy_file_compiles() {
    fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                collect(&path, out);
            } else if path
                .file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".fancy"))
            {
                out.push(path);
            }
        }
    }
    let mut files = Vec::new();
    collect(&workspace("runtime"), &mut files);
    assert!(files.len() >= 7, "{files:?}");
    for file in files {
        let source = std::fs::read_to_string(&file).unwrap();
        crate::script::template::check_template(&source)
            .unwrap_or_else(|err| panic!("{}: {}", file.display(), err.message()));
    }
}

/// A render logs an element no macro names and writes a placeholder; the lint fails on it, as it is most often a
/// typo. A subtree a macro reads for itself and a widget-template-include are not looked into.
#[test]
fn an_element_no_screen_macro_names_is_a_failure() {
    let dir = write_screen(
        "unknown-element",
        r#"<widgets><container><lable text="typo"/></container>
            <form-single name="F"><field name="a"><default-field><display/></default-field></field>
            <field-layout><field-ref name="a"/></field-layout></form-single>
            <section name="S"><widgets><no-such-widget/></widgets></section></widgets>"#,
    );
    let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
    let failures = report.failures();
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures[0].contains("lable: no screen macro names this element"),
        "{failures:?}"
    );
    assert!(failures[1].contains("no-such-widget"), "{failures:?}");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn freemarker_left_in_a_text_body_is_a_failure() {
    let dir = write_screen(
        "freemarker",
        r#"<widgets><render-mode>
            <text type="html"><![CDATA[<#if x?has_content>${x!""}</#if>]]></text>
            <text type="html"><![CDATA[<#assign y = 1><@macro/>]]></text>
            <text type="vuet"><![CDATA[<#visit node>]]></text>
            <text type="html" template="false"><![CDATA[<#visit node>]]></text>
            <text type="html"><![CDATA[<#if x>${x}<#else>none</#if>]]></text>
            </render-mode></widgets>"#,
    );
    let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
    let failures = report.failures();
    assert_eq!(failures.len(), 2, "{failures:?}");
    assert!(
        failures[0].contains("text body") && failures[0].contains("not JavaScript"),
        "{failures:?}"
    );
    assert!(failures[1].contains("macro calls"), "{failures:?}");
    // The dropped vuet mode and the template="false" text are not compiled; the last one is.
    assert_eq!(report.files[0].dialect_bodies, 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn groovy_in_a_screen_is_a_failure() {
    let dir = write_screen(
        "groovy",
        r#"<widgets><label text="${a ?: 'x'}"/><link url="." parameter-map="[a:b]"/>
            <if condition="items"><label text="ok"/></if></widgets>
            <actions><script>def x = 1</script></actions>"#,
    );
    let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
    let failures = report.failures();
    assert!(failures.iter().any(|line| line.contains("label@text")), "{failures:?}");
    assert!(
        failures.iter().any(|line| line.contains("link@parameter-map")),
        "{failures:?}"
    );
    assert!(failures.iter().any(|line| line.contains("script@text")), "{failures:?}");
    let totals = report.idiom_totals();
    assert!(totals.contains_key(&Idiom::Elvis));
    assert!(totals.contains_key(&Idiom::MapLiteral));
    assert!(totals.contains_key(&Idiom::Def));
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn browser_only_parts_and_bare_expansions_in_tui_texts_are_listed() {
    let dir = write_screen(
        "tui",
        r#"<widgets>
            <render-mode><text type="qvt"><![CDATA[<m-chart/>]]></text></render-mode>
            <render-mode><text type="qvt">x</text><text type="tui"><![CDATA[{"t":"text","text":${JSON.stringify(a)}}]]></text></render-mode>
            <render-mode><text type="tui"><![CDATA[{"t":"text","text":"${a}"}]]></text></render-mode>
            </widgets>"#,
    );
    let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
    let file = &report.files[0];
    assert_eq!(file.tui_missing.len(), 1, "{:?}", file.tui_missing);
    assert!(file.tui_missing[0].1.contains("qvt"), "{:?}", file.tui_missing);
    assert_eq!(file.tui_unsafe.len(), 1, "{:?}", file.tui_unsafe);
    assert!(report
        .render()
        .contains("TUI worklist: 1 browser-only part(s) in 1 screen(s)"));
    // information, not a failure
    assert_eq!(report.failures(), Vec::<String>::new());
    std::fs::remove_dir_all(dir).unwrap();
    let dir = write_screen("qvue", "<widgets/>");
    std::fs::write(
        dir.join("screen").join("Test.xml"),
        r#"<screen render-modes="vue,qvue"><widgets/></screen>"#,
    )
    .unwrap();
    let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
    assert_eq!(report.files[0].tui_missing.len(), 1);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn javascript_in_a_screen_passes_and_template_bodies_compile() {
    let dir = write_screen(
        "javascript",
        r#"<widgets><label text="${a || 'x'}" condition="a &amp;&amp; b.length"/>
            <link url="." parameter-map="{a: b}"/>
            <render-mode><text type="html"><![CDATA[<#if x>${y}</#if>]]></text></render-mode></widgets>
            <actions><set field="n" from="parseInt(m, 10)"/><script>const x = [1, 2].map(v => v * 2)</script></actions>"#,
    );
    let report = lint_screens(std::slice::from_ref(&dir)).unwrap();
    assert_eq!(report.failures(), Vec::<String>::new());
    assert_eq!(report.files[0].dialect_bodies, 1);
    assert!(report.files[0].ftl.is_empty());
    std::fs::remove_dir_all(dir).unwrap();
}
