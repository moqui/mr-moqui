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

//! Who reads which index (plans/design-dev-aide.md 7.4), and the seed that splits the read services from the rest.

use std::cell::Cell;

use moqui_lib::node::MNode;

use super::*;

const WHAT: &str = "The artifact index";

/// A dev instance, a dev-aide reader.
const DEV: Access = Access {
    dev: true,
    dev_reader: true,
    reader: false,
};
/// Not a dev instance, a user with `AIDE_READ`.
const READER: Access = Access {
    dev: false,
    dev_reader: false,
    reader: true,
};

fn rows(id: &str) -> Option<ProjectRow> {
    match id {
        "OPEN" => Some(ProjectRow {
            directory: "/srv/open".to_string(),
            java: false,
            allowed: true,
        }),
        "JAVA" => Some(ProjectRow {
            directory: "/srv/java".to_string(),
            java: true,
            allowed: true,
        }),
        "SHUT" => Some(ProjectRow {
            directory: "/srv/shut".to_string(),
            java: false,
            allowed: false,
        }),
        "BLANK" => Some(ProjectRow {
            directory: " ".to_string(),
            java: false,
            allowed: true,
        }),
        _ => None,
    }
}

fn choose(access: Access, project_id: &str, project: &str) -> Result<Source, String> {
    choose_source(access, WHAT, project_id, project, rows)
}

#[test]
fn an_allowed_project_is_read_by_its_id_on_any_instance() {
    for access in [DEV, READER] {
        assert_eq!(
            choose(access, "OPEN", ""),
            Ok(Source::Allowed {
                directory: "/srv/open".to_string(),
                java: false
            })
        );
        assert_eq!(
            choose(access, " JAVA ", ""),
            Ok(Source::Allowed {
                directory: "/srv/java".to_string(),
                java: true
            })
        );
    }
}

/// An unknown id, a row not allowed and a row with no directory get the same error, which does not say which.
#[test]
fn a_project_not_allowed_is_refused_alike_whether_or_not_it_is_there() {
    for access in [DEV, READER] {
        for id in ["SHUT", "BLANK", "NOPE"] {
            assert_eq!(choose(access, id, ""), Err(NOT_ALLOWED.to_string()), "{id}");
        }
    }
}

/// A named project never answers from the server's own rows: an id that is not allowed is an error, not `Own`, also
/// on a dev instance where the own rows are kept.
#[test]
fn a_named_project_never_falls_back_to_the_servers_own_index() {
    let full = Access {
        dev: true,
        dev_reader: true,
        reader: true,
    };
    for access in [DEV, READER, full] {
        assert!(choose(access, "NOPE", "").is_err());
        assert!(!matches!(choose(access, "OPEN", ""), Ok(Source::Own)));
    }
    // with both given, neither is guessed at
    let both = choose(DEV, "OPEN", "/srv/open").unwrap_err();
    assert!(both.contains("not both"), "{both}");
}

/// Off a dev instance no path of a request is read and the server's own index is not answered, whatever the user's
/// permissions; on a dev instance a dev-aide reader keeps `project` and the own rows as before.
#[test]
fn a_directory_and_the_own_index_are_for_dev_aide_readers_on_dev_only() {
    let everything_off_dev = Access {
        dev: false,
        dev_reader: true,
        reader: true,
    };
    for access in [READER, everything_off_dev] {
        let directory = choose(access, "", "/srv/open").unwrap_err();
        assert!(directory.contains("name an allowed projectId"), "{directory}");
        assert!(choose(access, "", "").is_err());
    }
    // an AIDE_READ user on a dev instance has no dev-aide tier: the same
    let reader_on_dev = Access { dev: true, ..READER };
    assert!(choose(reader_on_dev, "", "/srv/open").is_err());
    assert!(choose(reader_on_dev, "", "").is_err());
    assert_eq!(choose(DEV, "", ""), Ok(Source::Own));
    assert_eq!(choose(DEV, "", "/srv/open"), Ok(Source::Directory));
}

/// No permission, or a dev-aide permission off dev only: refused before any project row is looked up.
#[test]
fn a_read_without_a_read_permission_is_refused_before_the_lookup() {
    let looked = Cell::new(false);
    let none = Access {
        dev: true,
        dev_reader: false,
        reader: false,
    };
    let dev_tier_off_dev = Access {
        dev: false,
        dev_reader: true,
        reader: false,
    };
    for access in [none, dev_tier_off_dev] {
        let refused = choose_source(access, WHAT, "OPEN", "", |id| {
            looked.set(true);
            rows(id)
        })
        .unwrap_err();
        assert!(refused.contains(AIDE_READ), "{refused}");
    }
    assert!(!looked.get());
}

fn seed(path: &str) -> String {
    std::fs::read_to_string(format!("{}/../../runtime/{path}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

/// The `artifactName` patterns of an artifact group's `AT_SERVICE` members in seed `text`.
fn service_patterns(text: &str, group: &str) -> Vec<String> {
    let doc = MNode::parse_text("seed", text).unwrap();
    let in_group = |node: &MNode| node.attribute("artifactGroupId").as_deref() == Some(group);
    let mut members = Vec::new();
    for node in doc.children() {
        if node.has_name("artifactGroups") && in_group(&node) {
            members.extend(node.children().into_iter().filter(|child| child.has_name("artifacts")));
        } else if node.has_name("moqui.security.ArtifactGroupMember") && in_group(&node) {
            members.push(node);
        }
    }
    members
        .iter()
        .filter(|member| member.attribute("artifactTypeEnumId").as_deref() == Some("AT_SERVICE"))
        .map(|member| {
            let name = member.attribute("artifactName").unwrap_or_default();
            if member.attribute("nameIsPattern").as_deref() == Some("Y") {
                name
            } else {
                regex::escape(&name)
            }
        })
        .collect()
}

fn matched(patterns: &[String], service: &str) -> bool {
    patterns.iter().any(|pattern| {
        regex::Regex::new(&format!("^(?:{pattern})$"))
            .unwrap()
            .is_match(service)
    })
}

/// The read services are in the framework's `AIDE_READ_API` group, on every instance; what writes, runs or rebuilds is
/// only in the dev-aide's `DEV_AIDE_APP`, which is not on other instances, and no service is in both.
#[test]
fn the_seed_splits_the_read_services_from_the_dev_only_ones() {
    let framework = seed("base-component/framework/data/DevToolsSecurityData.xml");
    let dev_aide = seed("dev-component/dev-aide/data/DevAideSecurityData.xml");
    let read = service_patterns(&framework, "AIDE_READ_API");
    let dev_only = service_patterns(&dev_aide, "DEV_AIDE_APP");
    let services = seed("base-component/framework/service/org/moqui/impl/DevToolsServices.xml");
    let doc = MNode::parse_text("services", &services).unwrap();
    let names: Vec<String> = doc
        .children()
        .iter()
        .filter(|node| node.has_name("service"))
        .map(|node| {
            format!(
                "org.moqui.impl.DevToolsServices.{}#{}",
                node.attribute("verb").unwrap_or_default(),
                node.attribute("noun").unwrap_or_default()
            )
        })
        .collect();
    let reads = [
        "find#Artifacts",
        "get#Artifact",
        "get#ArtifactRefs",
        "get#ArtifactBindings",
        "get#ArtifactAt",
        "get#ArtifactExtensions",
    ];
    for name in &names {
        let short = name.rsplit('.').next().unwrap();
        let is_read = reads.contains(&short);
        assert_eq!(matched(&read, name), is_read, "{name} in AIDE_READ_API");
        assert_eq!(matched(&dev_only, name), !is_read, "{name} in DEV_AIDE_APP");
    }
    for short in reads {
        assert!(names.iter().any(|name| name.ends_with(&format!(".{short}"))), "{short}");
    }
    assert!(framework.contains(r#"userPermissionId="AIDE_READ""#));
    assert!(framework.contains(r#"artifactName="/moqui/aide""#));
}
