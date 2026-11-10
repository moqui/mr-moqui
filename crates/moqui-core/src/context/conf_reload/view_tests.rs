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

use std::collections::HashMap;
use std::path::PathBuf;

use super::*;
use crate::context::conf::conf_files_of;
use crate::context::tests::TempRuntime;

const DEFAULT: &str = r#"<?xml version="1.0" encoding="UTF-8" ?>
<moqui-conf>
    <!-- The purpose of the instance:
        production, dev or test. -->
    <default-property name="instance_purpose" value="production"/>
    <default-property name="view_test_password" value="sekret"/>
    <webapp-list>
        <webapp name="webroot" http-port="8080">
            <session-config timeout="60"/><!-- minutes -->
        </webapp>
    </webapp-list>
    <smtp-server enabled="false" password="hunter2"/>
</moqui-conf>
"#;

const COMPONENT: &str = r#"<moqui-conf>
    <webapp-list>
        <webapp name="webroot">
            <session-config timeout="90"/>
        </webapp>
    </webapp-list>
</moqui-conf>
"#;

const RUNTIME: &str = r#"<moqui-conf>
    <default-property name="instance_purpose" value="dev"/>
    <webapp-list>
        <webapp name="webroot" http-port="9090"/>
    </webapp-list>
</moqui-conf>
"#;

struct Files {
    /// Kept until the test ends: dropping it removes the files.
    _runtime: TempRuntime,
    files: Vec<ConfFile>,
}

fn files() -> Files {
    let runtime = TempRuntime::new("conf-view");
    let framework = runtime.path().join("framework");
    let component = runtime.path().join("component/app");
    std::fs::create_dir_all(framework.join("conf")).unwrap();
    std::fs::create_dir_all(&component).unwrap();
    std::fs::write(framework.join("conf/MoquiDefaultConf.xml"), DEFAULT).unwrap();
    std::fs::write(component.join("MoquiConf.xml"), COMPONENT).unwrap();
    std::fs::write(runtime.path().join("conf/MoquiDevConf.xml"), RUNTIME).unwrap();
    let components: Vec<(String, PathBuf)> = vec![("app".to_string(), component)];
    let files = conf_files_of(&framework, runtime.path(), &components, false).unwrap();
    Files {
        _runtime: runtime,
        files,
    }
}

fn child<'a>(node: &'a ViewNode, name: &str, key: Option<&str>) -> &'a ViewNode {
    node.children
        .iter()
        .find(|child| child.name == name && child.key.as_deref() == key)
        .unwrap_or_else(|| panic!("no {name} {key:?} under {}", node.name))
}

fn attr<'a>(node: &'a ViewNode, name: &str) -> &'a ViewAttr {
    node.attrs
        .iter()
        .find(|attr| attr.name == name)
        .unwrap_or_else(|| panic!("no attribute {name} on {}", node.name))
}

#[test]
fn the_view_has_each_values_file_class_and_default_comments() {
    let held = files();
    let files = &held.files;
    let roles: Vec<&str> = file_infos(files).iter().map(|info| info.role).collect();
    assert_eq!(roles, ["default", "component", "runtime"]);
    assert!(!file_infos(files)[0].editable);
    assert!(file_infos(files)[2].editable);

    let merged = with_merge_sources(|| merge_files(files, None)).unwrap();
    let view = build_view(files, &merged, None, &HashMap::new());

    let purpose = child(&view.root, "default-property", Some("instance_purpose"));
    assert_eq!(purpose.path, "default-property[instance_purpose]");
    assert_eq!(
        purpose.comments,
        ["The purpose of the instance:\nproduction, dev or test."]
    );
    assert_eq!(purpose.sources, [At { file: 0, line: 5 }, At { file: 2, line: 2 }]);
    assert_eq!(attr(purpose, "value").value, "dev");
    assert_eq!(attr(purpose, "value").source, Some(At { file: 2, line: 2 }));
    assert_eq!(attr(purpose, "name").source, Some(At { file: 2, line: 2 }));

    let webapp = child(child(&view.root, "webapp-list", None), "webapp", Some("webroot"));
    assert_eq!(webapp.path, "webapp-list/webapp[webroot]");
    assert_eq!(webapp.class, "live");
    assert_eq!(
        webapp.sources,
        [
            At { file: 0, line: 8 },
            At { file: 1, line: 3 },
            At { file: 2, line: 4 }
        ]
    );
    let port = attr(webapp, "http-port");
    assert_eq!(port.value, "9090");
    assert_eq!(port.class, Some("restart"));
    assert_eq!(port.source, Some(At { file: 2, line: 4 }));
    // the component's session-config merged into the default's, which keeps its comment on the same line
    let session = child(webapp, "session-config", None);
    assert_eq!(session.class, "live");
    assert_eq!(session.sources, [At { file: 0, line: 9 }, At { file: 1, line: 4 }]);
    assert_eq!(session.comments, ["minutes"]);
    assert_eq!(attr(session, "timeout").value, "90");
    assert_eq!(attr(session, "timeout").class, None);

    // secrets are masked, by the property's name and by the attribute's
    let property = child(&view.root, "default-property", Some("view_test_password"));
    assert_eq!(attr(property, "value").value, MASKED);
    let smtp = child(&view.root, "smtp-server", None);
    assert_eq!(attr(smtp, "password").value, MASKED);
    assert_eq!(attr(smtp, "enabled").value, "false");
    assert_eq!(smtp.class, "restart");
    assert!(smtp.reason.is_some());
}

#[test]
fn the_lines_of_one_file_have_classes_and_follow_an_edited_text() {
    let held = files();
    let files = &held.files;
    let merged = with_merge_sources(|| merge_files(files, None)).unwrap();
    let lines = file_lines(files, 2, &merged);
    let found: Vec<(u32, &str, &str)> = lines
        .iter()
        .map(|line| (line.line, line.path.as_str(), line.class))
        .collect();
    assert_eq!(
        found,
        [
            (2, "default-property[instance_purpose]", "apply"),
            (3, "webapp-list", "restart"),
            (4, "webapp-list/webapp[webroot]", "live"),
        ]
    );
    let webapp = &lines[2];
    assert!(webapp
        .attrs
        .iter()
        .any(|(name, class, _)| name == "http-port" && *class == "restart"));
    // the default conf's comment of the element a line merges into
    assert_eq!(
        lines[0].comments,
        ["The purpose of the instance:\nproduction, dev or test."]
    );

    // an edited text in place of the runtime conf: two lines down, a new session timeout
    let edited = RUNTIME.replace("<webapp-list>", "\n\n<webapp-list>").replace(
        r#"<webapp name="webroot" http-port="9090"/>"#,
        r#"<webapp name="webroot" http-port="9090"><session-config timeout="30"/></webapp>"#,
    );
    let runtime_path = files[2].path.clone();
    let merged = with_merge_sources(|| merge_files(files, Some((&runtime_path, &edited)))).unwrap();
    let lines = file_lines(files, 2, &merged);
    assert_eq!(lines.last().map(|line| line.line), Some(6));
    let view = build_view(files, &merged, None, &HashMap::new());
    let webapp = child(child(&view.root, "webapp-list", None), "webapp", Some("webroot"));
    assert_eq!(attr(child(webapp, "session-config", None), "timeout").value, "30");
}

#[test]
fn a_text_that_is_not_a_conf_file_is_a_problem() {
    let broken = text_problems("x.xml", "<moqui-conf>\n  <webapp-list>\n</moqui-conf>");
    assert_eq!(broken.len(), 1);
    assert_eq!(broken[0].severity, "error");
    assert!(broken[0].line > 1, "{broken:?}");
    let other = text_problems("x.xml", "<screen/>");
    assert!(other[0].message.contains("moqui-conf"));
    assert!(text_problems("x.xml", RUNTIME).is_empty());
}
