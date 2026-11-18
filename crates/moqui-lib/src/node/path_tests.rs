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

fn conf() -> MNode {
    MNode::parse_text(
        "test",
        r#"<moqui-conf>
            <webapp-list><webapp name="webroot" http-port="8080"><session-config cookie-key=""/></webapp></webapp-list>
            <entity-facade crypt-pass="x">
                <datasource group-name="transactional" host="127.0.0.1"/>
                <datasource group-name="transactional#clone1" host=""/>
                <load-data location="a"/>
            </entity-facade>
            <telemetry><artifact type="AT_SERVICE" spans="true"/></telemetry>
            <cache-list><cache name="a.b" max-elements="1"/><cache name="a_b" max-elements="2"/></cache-list>
            <xa-holder><xa-properties serverName="h"/></xa-holder>
        </moqui-conf>"#,
    )
    .unwrap()
}

fn keying(path: &[&str]) -> Keying {
    match path {
        ["webapp-list", "webapp"] => Keying::By(&["name"]),
        ["entity-facade", "datasource"] => Keying::By(&["group-name"]),
        ["entity-facade", "load-data"] => Keying::Sequence,
        ["telemetry", "artifact"] => Keying::By(&["type"]),
        ["cache-list", "cache"] => Keying::By(&["name"]),
        ["webapp-list", "webapp", "response-header"] => Keying::By(&["type", "name"]),
        _ => Keying::Single,
    }
}

fn env(name: &str, create: bool) -> Result<Target, PathError> {
    let segments = env_segments(name).expect("an environment name of the root");
    resolve_env(&conf(), &segments, &keying, create)
}

#[test]
fn normalize_ignores_case_and_punctuation() {
    assert_eq!(normalize("http-port"), "httpport");
    assert!(same_name("http-port", "HTTP_PORT"));
    assert!(same_name("httpPort", "http_port"));
    assert!(same_name("transactional#clone1", "TRANSACTIONAL_CLONE1"));
    assert!(!same_name("http-port", "https-port"));
}

#[test]
fn selector_parses_and_formats() {
    let path = AttrPath::parse("webapp-list/webapp[webroot]/response-header[request,X-Frame-Options]@value").unwrap();
    assert_eq!(path.steps.len(), 3);
    assert_eq!(path.steps[2].keys, vec!["request", "X-Frame-Options"]);
    assert_eq!(path.attribute, "value");
    assert_eq!(
        path.selector(),
        "webapp-list/webapp[webroot]/response-header[request,X-Frame-Options]@value"
    );
    let screen = AttrPath::parse("screen-facade/screen[component://webroot/screen/webroot.xml]@location").unwrap();
    assert_eq!(screen.steps[1].keys, vec!["component://webroot/screen/webroot.xml"]);
    assert_eq!(
        AttrPath::parse("ssh-server@port").unwrap().steps[0].element,
        "ssh-server"
    );
    assert!(AttrPath::parse("ssh-server").is_err());
    assert!(AttrPath::parse("@port").is_err());
    assert!(AttrPath::parse("webapp[webroot@port").is_err());
}

#[test]
fn env_name_is_lower_case_with_keys_in_capitals() {
    let path = AttrPath::parse("webapp-list/webapp[webroot]@http-port").unwrap();
    assert_eq!(
        path.env_name().unwrap(),
        "moqui_conf__webapp_list__webapp__WEBROOT__http_port"
    );
    let clone = AttrPath::parse("entity-facade/datasource[transactional#clone1]@host").unwrap();
    assert_eq!(
        clone.env_name().unwrap(),
        "moqui_conf__entity_facade__datasource__TRANSACTIONAL_CLONE1__host"
    );
    let cache = AttrPath::parse("cache-list/cache[entity.record.one.]@max-elements").unwrap();
    assert_eq!(
        cache.env_name().unwrap(),
        "moqui_conf__cache_list__cache__ENTITY_RECORD_ONE__max_elements"
    );
    assert_eq!(AttrPath::parse("root-screen[.*]@location").unwrap().env_name(), None);
}

#[test]
fn env_segments_take_only_names_of_the_root() {
    assert_eq!(
        env_segments("moqui_conf__ssh_server__port").unwrap(),
        vec!["ssh_server", "port"]
    );
    assert_eq!(
        env_segments("MOQUI_CONF__SSH_SERVER__PORT").unwrap(),
        vec!["SSH_SERVER", "PORT"]
    );
    assert!(env_segments("MOQUI_CONF").is_none());
    assert!(env_segments("MOQUI_CONFIG__X").is_none());
    assert!(env_segments("entity_ds_db_conf").is_none());
}

#[test]
fn env_name_finds_a_keyed_element_in_any_case() {
    let target = env("moqui_conf__webapp_list__webapp__WEBROOT__http_port", false).unwrap();
    assert_eq!(target.attribute, "http-port");
    assert!(target.attribute_existed);
    assert_eq!(target.path.selector(), "webapp-list/webapp[webroot]@http-port");
    assert_eq!(
        env("MOQUI_CONF__WEBAPP_LIST__WEBAPP__WEBROOT__HTTP_PORT", false)
            .unwrap()
            .path,
        target.path
    );
    let clone = env(
        "moqui_conf__entity_facade__datasource__TRANSACTIONAL_CLONE1__host",
        false,
    )
    .unwrap();
    assert_eq!(clone.path.steps[1].keys, vec!["transactional#clone1"]);
    let nested = env(
        "moqui_conf__webapp_list__webapp__WEBROOT__session_config__cookie_key",
        false,
    )
    .unwrap();
    assert_eq!(
        nested.path.selector(),
        "webapp-list/webapp[webroot]/session-config@cookie-key"
    );
    let camel = env("moqui_conf__xa_holder__xa_properties__server_name", false).unwrap();
    assert_eq!(camel.attribute, "serverName");
}

#[test]
fn env_name_makes_what_is_missing() {
    let root = conf();
    let segments = env_segments("moqui_conf__instance__purpose").unwrap();
    let target = resolve_env(&root, &segments, &keying, true).unwrap();
    assert!(target.created);
    assert!(!target.attribute_existed);
    assert_eq!(target.path.selector(), "instance@purpose");
    assert!(root.first_named("instance").is_some());

    let segments = env_segments("moqui_conf__entity_facade__datasource__ANALYTICS__host").unwrap();
    let target = resolve_env(&root, &segments, &keying, true).unwrap();
    assert_eq!(target.node.attribute("group-name").as_deref(), Some("ANALYTICS"));
    assert_eq!(target.attribute, "host");

    let segments = env_segments("moqui_conf__ssh_server__public_port").unwrap();
    assert!(matches!(
        resolve_env(&root, &segments, &keying, false),
        Err(PathError::Missing(_))
    ));
}

#[test]
fn env_name_refusals() {
    assert!(matches!(
        env("moqui_conf__entity_facade__load_data__location", true),
        Err(PathError::Sequence(_))
    ));
    assert!(matches!(
        env("moqui_conf__cache_list__cache__A_B__max_elements", true),
        Err(PathError::Ambiguous(_))
    ));
    assert!(matches!(
        env("moqui_conf__webapp_list__webapp__WEBROOT", true),
        Err(PathError::NoAttribute)
    ));
    assert!(matches!(
        env("moqui_conf__webapp_list__webapp__http_port", true),
        Err(PathError::NoAttribute)
    ));
    assert!(matches!(env("moqui_conf____port", true), Err(PathError::EmptySegment)));
    assert!(matches!(env("moqui_conf__port", true), Err(PathError::NoAttribute)));
}

#[test]
fn selector_resolves_and_path_of_comes_back() {
    let root = conf();
    let path = AttrPath::parse("telemetry/artifact[at_service]@spans").unwrap();
    let target = resolve(&root, &path, &keying, false).unwrap();
    assert_eq!(target.node.attribute("spans").as_deref(), Some("true"));
    assert_eq!(target.path.selector(), "telemetry/artifact[AT_SERVICE]@spans");
    let back = AttrPath::of(&target.node, "spans", &keying).unwrap();
    assert_eq!(back.selector(), "telemetry/artifact[AT_SERVICE]@spans");
    assert_eq!(
        back.env_name().unwrap(),
        "moqui_conf__telemetry__artifact__AT_SERVICE__spans"
    );
    let wrong = AttrPath::parse("telemetry/artifact@spans").unwrap();
    assert!(matches!(
        resolve(&root, &wrong, &keying, false),
        Err(PathError::Syntax(_))
    ));
    let load = root
        .first_named("entity-facade")
        .unwrap()
        .first_named("load-data")
        .unwrap();
    assert_eq!(AttrPath::of(&load, "location", &keying), None);
    assert_eq!(AttrPath::of(&root, "x", &keying), None);
}
