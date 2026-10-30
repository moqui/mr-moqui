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

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn expect_parse_err(result: Result<MNode, crate::error::Error>) -> crate::error::Error {
    match result {
        Err(err) => err,
        Ok(_) => panic!("expected a parse error"),
    }
}

#[test]
fn deeply_nested_elements_are_an_error_not_a_stack_overflow() {
    let deep = format!("{}{}", "<a>".repeat(100_000), "</a>".repeat(100_000));
    let err = expect_parse_err(MNode::parse_text("deep.xml", &deep));
    assert!(err.to_string().contains("nested deeper"), "{err}");
    let fine = format!("{}{}", "<a>".repeat(100), "</a>".repeat(100));
    MNode::parse_text("fine.xml", &fine).unwrap();
}

#[test]
fn declaration_comment_cdata_entities_and_raw_names() {
    let xml = r#"<?xml version="1.0" encoding="UTF-8"?>
        <!-- comment with &not-an-entity; -->
        <root xmlns:xsi="http://www.w3.org/2001/XMLSchema-instance" b="1" a="&lt;&gt;&amp;&apos;&quot;&#65;&#x42;">
            <![CDATA[  hello & not-an-entity  ]]>
            <child/>
        </root>
        "#;
    let root = MNode::parse_text("sample.xml", xml).unwrap();
    assert_eq!(root.name(), "root");
    assert_eq!(root.line(), 3);
    let attrs = root.attributes();
    assert_eq!(
        attrs.iter().map(|(key, _)| key.as_str()).collect::<Vec<_>>(),
        vec!["xmlns:xsi", "b", "a"]
    );
    assert_eq!(root.attribute("a").as_deref(), Some("<>&'\"AB"));
    assert_eq!(root.text().as_deref(), Some("hello & not-an-entity"));
    assert_eq!(root.children_named("child").len(), 1);
    assert!(root.first_named("child").unwrap().text().is_none());
}

#[test]
fn raw_text_keeps_whitespace() {
    let xml = "<root>\n  <a>  spaced  </a><b> </b><c><![CDATA[ x ]]>\n</c><d/></root>";
    let raw = MNode::parse_text_raw("raw.xml", xml).unwrap();
    let text = |name: &str| raw.first_named(name).and_then(|child| child.text());
    assert_eq!(text("a").as_deref(), Some("  spaced  "));
    assert_eq!(text("b").as_deref(), Some(" "));
    assert_eq!(text("c").as_deref(), Some(" x \n"));
    assert_eq!(text("d"), None);
    let trimmed = MNode::parse_text("trimmed.xml", xml).unwrap();
    assert_eq!(
        trimmed.first_named("a").and_then(|child| child.text()).as_deref(),
        Some("spaced")
    );
    assert!(trimmed.first_named("b").and_then(|child| child.text()).is_none());
}

#[test]
fn writer_cdata_round_trips() {
    let root = MNode::parse_text("t.xml", r#"<root b="1" a="2">a &lt; b &amp; c<child>x</child></root>"#).unwrap();
    let again = MNode::parse_text("t.xml", &root.write_string()).unwrap();
    assert!(root.same_structure(&again));
    assert_eq!(again.text().as_deref(), Some("a < b & c"));
    assert_eq!(again.attributes()[0].0, "b");
}

#[test]
fn writer_splits_cdata_end_in_text() {
    let root = MNode::parse_text("t.xml", "<root>a ]]&gt; b]]&gt;<child/></root>").unwrap();
    assert_eq!(root.text().as_deref(), Some("a ]]> b]]>"));
    let written = root.write_string();
    let again = MNode::parse_text("t.xml", &written).unwrap();
    assert_eq!(again.text().as_deref(), Some("a ]]> b]]>"), "{written}");
    assert_eq!(again.children().len(), 1);
}

#[test]
fn doctype_and_unknown_entity_report_line_and_column() {
    let err = expect_parse_err(MNode::parse_text("bad.xml", "<root>\n  <!DOCTYPE foo>\n</root>"));
    assert!(err.message().contains("DOCTYPE"));
    let loc = err.location().unwrap();
    assert_eq!(loc.file.as_deref(), Some("bad.xml"));
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 3);

    let err = expect_parse_err(MNode::parse_text("bad.xml", "<root>&foo;</root>"));
    assert!(err.message().contains("unknown entity"));
    assert_eq!(err.location().unwrap().line, 1);

    // Java's empty-entity resolver parses this and yields no secret. This port rejects the DOCTYPE.
    let path = std::env::temp_dir().join(format!(
        "moqui-xxe-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::write(&path, "XXE_SECRET_MARKER").unwrap();
    let xml = format!(
        "<?xml version=\"1.0\"?>\n<!DOCTYPE foo [<!ENTITY xxe SYSTEM \"file://{}\">]>\n<root>&xxe;</root>\n",
        path.display()
    );
    let err = expect_parse_err(MNode::parse_text("xxe.xml", &xml));
    let _ = std::fs::remove_file(&path);
    assert!(err.message().contains("DOCTYPE"), "{err}");
    let loc = err.location().unwrap();
    assert_eq!(loc.line, 2);
    assert_eq!(loc.column, 1);
    assert!(!err.to_string().contains("XXE_SECRET_MARKER"), "{err}");
}

#[test]
fn mismatched_end_tag_names_the_element() {
    let err = expect_parse_err(MNode::parse_text("bad.xml", "<root>\n  <child>\n</root>"));
    assert!(err.message().contains("</root>"));
    assert!(err.message().contains("<child>"));
    assert_eq!(err.location().unwrap().line, 3);
}

#[test]
fn root_only_stops_after_the_start_tag() {
    let root = MNode::parse_root_only("t.xml", r#"<root a="1"><child/></root>"#).unwrap();
    assert_eq!(root.attribute("a").as_deref(), Some("1"));
    assert!(root.children().is_empty());
}

#[test]
fn merge_children_by_key_and_single_child() {
    let base = MNode::parse_text(
        "b.xml",
        r#"<root><cache-list warm-on-start="true"><cache name="entity.definition" value-type="Entity"/></cache-list><user-facade><password min-length="8"/></user-facade></root>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "o.xml",
        r#"<root><cache-list warm-on-start="false"><cache name="entity.definition" expire-time-idle="30"/><cache name="new.one"/></cache-list><user-facade><password min-length="12"><extra/></password></user-facade></root>"#,
    )
    .unwrap();
    base.merge_child_with_child_key(&over, "cache-list", "cache", &["name"], None);
    base.merge_child_with_child_key(&over, "user-facade", "password", &[], None);
    let cache_list = base.first_named("cache-list").unwrap();
    assert_eq!(cache_list.attribute("warm-on-start").as_deref(), Some("false"));
    let entity = cache_list
        .first_named_attrs("cache", &[("name", "entity.definition")])
        .unwrap();
    assert_eq!(entity.attribute("expire-time-idle").as_deref(), Some("30"));
    assert_eq!(entity.attribute("value-type").as_deref(), Some("Entity"));
    assert!(cache_list.first_named_attrs("cache", &[("name", "new.one")]).is_some());
    let password = base
        .first_named("user-facade")
        .unwrap()
        .first_named("password")
        .unwrap();
    assert_eq!(password.attribute("min-length").as_deref(), Some("12"));
    assert!(password.first_named("extra").is_some());
}

#[test]
fn component_version_attribute_is_not_expanded() {
    let path = workspace_root().join("runtime/base-component/framework/component.xml");
    let root = MNode::parse_file(&path).unwrap();
    assert_eq!(root.attribute("name").as_deref(), Some("framework"));
    assert_eq!(root.attribute("version").as_deref(), Some("${moqui_version}"));
}

#[test]
fn corpus_parses_and_catalog_doctype_is_rejected() {
    let root = workspace_root();
    let mut files = Vec::new();
    collect_xml(&root.join("runtime"), &mut files);
    assert!(files.len() > 100, "copied {} files", files.len());
    let mut parsed = 0;
    for path in files {
        let text = std::fs::read_to_string(&path).unwrap();
        if path.ends_with("framework-catalog.xml") {
            let err = expect_parse_err(MNode::parse_text(&path.display().to_string(), &text));
            assert!(err.message().contains("DOCTYPE"), "{path:?} {err}");
            continue;
        }
        MNode::parse_text(&path.display().to_string(), &text)
            .unwrap_or_else(|err| panic!("parse {} failed: {err}", path.display()));
        parsed += 1;
    }
    assert!(parsed > 100);
}

#[test]
fn dev_conf_overlay_changes_the_sections_it_names() {
    let root = workspace_root();
    let base = MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).unwrap();
    let over = MNode::parse_file(root.join("runtime/conf/MoquiDevConf.xml")).unwrap();
    merge_dev_conf(&base, &over);

    let purpose = base
        .children_named("default-property")
        .into_iter()
        .find(|node| node.attribute_is("name", "instance_purpose"))
        .unwrap();
    assert_eq!(purpose.attribute("value").as_deref(), Some("dev"));
    let locale = base
        .children_named("default-property")
        .into_iter()
        .find(|node| node.attribute_is("name", "default_locale"))
        .unwrap();
    assert_eq!(locale.attribute("value").as_deref(), Some("en_US"));

    let caches = base.first_named("cache-list").unwrap();
    assert_eq!(caches.attribute("warm-on-start").as_deref(), Some("false"));
    let entity = caches
        .first_named_attrs("cache", &[("name", "entity.definition")])
        .unwrap();
    assert_eq!(entity.attribute("expire-time-idle").as_deref(), Some("30"));
    assert!(entity.has_attribute("value-type"));

    let screen = base
        .first_named("artifact-execution-facade")
        .unwrap()
        .first_named_attrs("artifact-execution", &[("type", "AT_XML_SCREEN")])
        .unwrap();
    assert_eq!(screen.attribute("tarpit-enabled").as_deref(), Some("false"));
}

#[test]
fn mantle_udm_parses_when_the_sibling_tree_is_present() {
    let path = workspace_root().join("../moqui/runtime/component/mantle-udm");
    if !path.is_dir() {
        eprintln!("skipping: the Java tree {} is not present", path.display());
        return;
    }
    let mut files = Vec::new();
    collect_xml(&path, &mut files);
    assert!(!files.is_empty());
    for file in files {
        MNode::parse_file(&file).unwrap_or_else(|err| panic!("parse {} failed: {err}", file.display()));
    }
}

fn merge_dev_conf(base: &MNode, over: &MNode) {
    base.merge_children_by_keys(over, "default-property", &["name"], None);
    base.merge_child_with_child_key(over, "cache-list", "cache", &["name"], None);
    if let (Some(stats), Some(override_stats)) = (base.first_named("server-stats"), over.first_named("server-stats")) {
        stats.copy_attributes_from(&override_stats);
        for child in override_stats.children_named("artifact-stats") {
            let found = stats
                .children_named("artifact-stats")
                .into_iter()
                .find(|node| same_attr(node, &child, "type") && same_attr(node, &child, "sub-type"));
            if let Some(base_child) = found {
                base_child.copy_attributes_from(&child);
            } else {
                stats.append_node(child.deep_copy());
            }
        }
    }
    base.merge_child_with_child_key(
        over,
        "webapp-list",
        "webapp",
        &["name"],
        Some(&mut |base_web, over_web| {
            base_web.merge_children_by_keys(over_web, "servlet", &["name"], None);
        }),
    );
    base.merge_child_with_child_key(over, "artifact-execution-facade", "artifact-execution", &["type"], None);
}

fn same_attr(left: &MNode, right: &MNode, name: &str) -> bool {
    non_empty(left.attribute(name)) == non_empty(right.attribute(name))
}

/// The XML files under `dir`, but not the component template's, whose placeholders (`<@package@.@Class@Item>`) are not
/// XML names.
fn collect_xml(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if path.ends_with("dev-component/template") {
                continue;
            }
            collect_xml(&path, out);
        } else if path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext == "xml" || ext == "xsd")
        {
            out.push(path);
        }
    }
}
