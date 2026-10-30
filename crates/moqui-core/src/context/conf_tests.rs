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

use moqui_lib::node::MNode;

#[test]
fn a_component_web_protection_merges_by_name_and_appends_rules() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><webapp-list><webapp name="webroot">
                <web-protection client-ipv6-prefix="64">
                    <connections max-per-client="64"/>
                    <rate-limit name="all" requests="50" burst="500"/>
                    <rate-limit name="login" requests="10" seconds="60"/>
                    <ban name="abuse" events="denied" threshold="300" seconds="60" duration="300"/>
                    <client-rule name="loopback" action="exempt" cidr="127.0.0.0/8"/>
                </web-protection>
            </webapp></webapp-list></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><webapp-list><webapp name="webroot">
                <web-protection enabled="true">
                    <connections max-per-client="8"/>
                    <rate-limit name="login" enabled="false"/>
                    <rate-limit name="api" path="/rest/**" requests="5"/>
                    <client-rule action="deny" cidr="198.51.100.0/24"/>
                    <client-rule name="loopback" enabled="false"/>
                </web-protection>
            </webapp></webapp-list></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let protection = base
        .first_named("webapp-list")
        .and_then(|list| list.first_named("webapp"))
        .and_then(|webapp| webapp.first_named("web-protection"))
        .unwrap();
    assert_eq!(protection.attribute("enabled").as_deref(), Some("true"));
    assert_eq!(protection.attribute("client-ipv6-prefix").as_deref(), Some("64"));
    let connections = protection.first_named("connections").unwrap();
    assert_eq!(connections.attribute("max-per-client").as_deref(), Some("8"));
    let limits = protection.children_named("rate-limit");
    let names: Vec<String> = limits.iter().filter_map(|limit| limit.attribute("name")).collect();
    assert_eq!(names, ["all", "login", "api"]);
    assert_eq!(limits[1].attribute("enabled").as_deref(), Some("false"));
    assert_eq!(limits[1].attribute("requests").as_deref(), Some("10"));
    assert_eq!(protection.children_named("ban").len(), 1);
    let rules = protection.children_named("client-rule");
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0].attribute("enabled").as_deref(), Some("false"));
    assert_eq!(rules[0].attribute("cidr").as_deref(), Some("127.0.0.0/8"));
}

#[test]
fn a_component_adds_web_protection_where_there_is_none() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><webapp-list><webapp name="webroot"/></webapp-list></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><webapp-list><webapp name="webroot"><web-protection enabled="false"/></webapp></webapp-list></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let webapp = base
        .first_named("webapp-list")
        .and_then(|list| list.first_named("webapp"))
        .unwrap();
    assert!(webapp.first_named("web-protection").is_some());
}

#[test]
fn the_llm_facade_merges_endpoints_models_and_binds_by_key() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><llm-facade enabled="true">
                <endpoint id="local" url="http://127.0.0.1:1234" local="true"><header name="A" value="1"/></endpoint>
                <model id="m" endpoint="local" name="qwen"><reasoning level="low">{"a":1}</reasoning><api-config>{"b":1}</api-config></model>
                <bind feature="assist" model="m" reasoning="low"/>
                <limits max-steps="40" user-daily-tokens="100"/>
                <a2a enabled="false"><push timeout="15"/><security bearer="true"/></a2a>
            </llm-facade></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><llm-facade enabled="false">
                <endpoint id="local" url="http://gpu:1234"><header name="B" value="2"/></endpoint>
                <endpoint id="hosted" url="https://api.example.com"/>
                <model id="m" max-output="10"><reasoning level="high">{"c":1}</reasoning><api-config>{"d":1}</api-config></model>
                <bind feature="assist" model="other"/>
                <limits max-steps="10"/>
                <a2a enabled="true"><security bearer="false"/></a2a>
            </llm-facade></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let facade = base.first_named("llm-facade").unwrap();
    assert_eq!(facade.attribute("enabled").as_deref(), Some("false"));
    let endpoints = facade.children_named("endpoint");
    assert_eq!(endpoints.len(), 2);
    assert_eq!(endpoints[0].attribute("url").as_deref(), Some("http://gpu:1234"));
    assert_eq!(endpoints[0].attribute("local").as_deref(), Some("true"));
    assert_eq!(endpoints[0].children_named("header").len(), 2);
    let model = facade.first_named("model").unwrap();
    assert_eq!(model.attribute("name").as_deref(), Some("qwen"));
    assert_eq!(model.attribute("max-output").as_deref(), Some("10"));
    assert_eq!(model.children_named("reasoning").len(), 2);
    let api = model.children_named("api-config");
    assert_eq!(api.len(), 1);
    assert_eq!(api[0].text().as_deref(), Some(r#"{"d":1}"#));
    let bind = facade.first_named("bind").unwrap();
    assert_eq!(bind.attribute("model").as_deref(), Some("other"));
    assert_eq!(bind.attribute("reasoning").as_deref(), Some("low"));
    let limits = facade.first_named("limits").unwrap();
    assert_eq!(limits.attribute("max-steps").as_deref(), Some("10"));
    assert_eq!(limits.attribute("user-daily-tokens").as_deref(), Some("100"));
    let a2a = facade.first_named("a2a").unwrap();
    assert_eq!(a2a.attribute("enabled").as_deref(), Some("true"));
    assert_eq!(
        a2a.first_named("push").unwrap().attribute("timeout").as_deref(),
        Some("15")
    );
    assert_eq!(
        a2a.first_named("security").unwrap().attribute("bearer").as_deref(),
        Some("false")
    );
}

#[test]
fn a_runtime_conf_overrides_one_attribute_of_a_settings_element() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><screen-facade><pdf page-limit="4" timeout="60"/></screen-facade>
                <user-facade><authz privileged-groups="ADMIN"/></user-facade>
                <git-server max-concurrent="4" max-queued="16"/></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><screen-facade><pdf timeout="90"/></screen-facade>
                <user-facade><authz sealed-permissions="X"/></user-facade>
                <git-server max-concurrent="2"/><ssh-server sftp-root="dbresource://sftp"/></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let pdf = base
        .first_named("screen-facade")
        .and_then(|facade| facade.first_named("pdf"))
        .unwrap();
    assert_eq!(pdf.attribute("page-limit").as_deref(), Some("4"));
    assert_eq!(pdf.attribute("timeout").as_deref(), Some("90"));
    let authz = base
        .first_named("user-facade")
        .and_then(|facade| facade.first_named("authz"))
        .unwrap();
    assert_eq!(authz.attribute("privileged-groups").as_deref(), Some("ADMIN"));
    assert_eq!(authz.attribute("sealed-permissions").as_deref(), Some("X"));
    let git = base.first_named("git-server").unwrap();
    assert_eq!(git.attribute("max-concurrent").as_deref(), Some("2"));
    assert_eq!(git.attribute("max-queued").as_deref(), Some("16"));
    let ssh = base.first_named("ssh-server").unwrap();
    assert_eq!(ssh.attribute("sftp-root").as_deref(), Some("dbresource://sftp"));
}

#[test]
fn telemetry_and_report_settings_merge_without_losing_security_limits() {
    let base = moqui_lib::node::MNode::parse_text("base",r#"<moqui-conf><telemetry enabled="true" sample-ratio="1" log-retention-days="7"><artifact type="AT_SERVICE" spans="true" metrics="true"/><receiver body-limit="8192" requests-per-minute="10"/></telemetry><report week-start="1" fiscal-start-month="1"/></moqui-conf>"#).unwrap();
    let over = moqui_lib::node::MNode::parse_text("site",r#"<moqui-conf><telemetry sample-ratio="0"><artifact type="AT_SERVICE" spans="false"/><receiver body-limit="4096"/><browser enabled="true"/></telemetry><report week-start="0"/></moqui-conf>"#).unwrap();
    super::merge_conf(&base, &over);
    let telemetry = base.first_named("telemetry").unwrap();
    assert_eq!(
        telemetry
            .first_named("browser")
            .unwrap()
            .attribute("enabled")
            .as_deref(),
        Some("true")
    );
    assert_eq!(telemetry.attribute("sample-ratio").as_deref(), Some("0"));
    assert_eq!(telemetry.attribute("log-retention-days").as_deref(), Some("7"));
    let receiver = telemetry.first_named("receiver").unwrap();
    assert_eq!(receiver.attribute("body-limit").as_deref(), Some("4096"));
    assert_eq!(receiver.attribute("requests-per-minute").as_deref(), Some("10"));
    assert_eq!(
        base.first_named("report").unwrap().attribute("week-start").as_deref(),
        Some("0")
    );
}
