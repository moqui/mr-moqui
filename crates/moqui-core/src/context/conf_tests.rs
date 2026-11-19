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
                <resource-facade><rich-text-images max-per-user="1000" max-bytes-per-user="209715200"/></resource-facade>
                <git-server max-concurrent="4" max-queued="16"/></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><screen-facade><pdf timeout="90"/></screen-facade>
                <user-facade><authz sealed-permissions="X"/></user-facade>
                <resource-facade><rich-text-images max-per-user="3"/></resource-facade>
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
    let resource = base.first_named("resource-facade").unwrap();
    assert_eq!(resource.children_named("rich-text-images").len(), 1);
    let images = resource.first_named("rich-text-images").unwrap();
    assert_eq!(images.attribute("max-per-user").as_deref(), Some("3"));
    assert_eq!(images.attribute("max-bytes-per-user").as_deref(), Some("209715200"));
    let git = base.first_named("git-server").unwrap();
    assert_eq!(git.attribute("max-concurrent").as_deref(), Some("2"));
    assert_eq!(git.attribute("max-queued").as_deref(), Some("16"));
    let ssh = base.first_named("ssh-server").unwrap();
    assert_eq!(ssh.attribute("sftp-root").as_deref(), Some("dbresource://sftp"));
}

/// A component's `shell-providers` in its conf (Mantle's `sms-consent` answer) merge into the framework's by hook and
/// name: a second component's provider is added and one of the same name replaces the earlier one; the parse keeps only
/// the hooks a conf may answer.
#[test]
fn a_components_conf_providers_merge_by_hook_and_name() {
    let base = MNode::parse_text("moqui-conf", r#"<moqui-conf><screen-facade/></moqui-conf>"#).unwrap();
    let first = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><screen-facade><shell-providers>
            <sms-consent name="mantle.consent" service="mantle.A.check#One"/>
        </shell-providers></screen-facade></moqui-conf>"#,
    )
    .unwrap();
    let second = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><screen-facade><shell-providers>
            <sms-consent name="other.consent" service="other.B.check#Two" sequence="-1"/>
            <sms-consent name="mantle.consent" service="mantle.A.check#Again"/>
            <inbox-source name="other.inbox" service="other.B.get#Inbox"/>
        </shell-providers></screen-facade></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &first);
    super::merge_conf(&base, &second);
    let parsed = crate::screen::ScreenConf::parse(&base.write_string());
    let found: Vec<(String, String)> = parsed
        .providers
        .iter()
        .map(|provider| (provider.name.clone(), provider.service.clone()))
        .collect();
    assert_eq!(
        found,
        [
            ("other.consent".to_string(), "other.B.check#Two".to_string()),
            ("mantle.consent".to_string(), "mantle.A.check#Again".to_string())
        ]
    );
}

/// A component's `<git-ci>` (Mantle names its run check and run URL services) merges over the framework's, attribute
/// by attribute; and every top-level element of the framework's default conf has a merge rule, so none is dropped.
#[test]
fn a_components_git_ci_attributes_merge_over_the_default() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><git-ci enabled="" run-check-service="" run-url-service="" run-url="" max-jobs="50"/></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><git-ci run-check-service="mantle.smithy.SmithyCiServices.check#PipelineRun"
                run-url-service="mantle.smithy.SmithyCiServices.get#PipelineRunUrl"/></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let ci = base.first_named("git-ci").unwrap();
    assert_eq!(
        ci.attribute("run-check-service").as_deref(),
        Some("mantle.smithy.SmithyCiServices.check#PipelineRun")
    );
    assert_eq!(
        ci.attribute("run-url-service").as_deref(),
        Some("mantle.smithy.SmithyCiServices.get#PipelineRunUrl")
    );
    assert_eq!(ci.attribute("max-jobs").as_deref(), Some("50"));
    assert_eq!(base.children_named("git-ci").len(), 1);

    // the runtime's component-deploy turns the gate on and names its allow entries over the default's
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><component-deploy require-passed-run="false" max-age-seconds="300"/></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><component-deploy require-passed-run="true"><allow component="site"
                repository-url="https://source.example.com/git/site.git" branch="main"/></component-deploy></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let deploy = base.first_named("component-deploy").unwrap();
    assert_eq!(deploy.attribute("require-passed-run").as_deref(), Some("true"));
    assert_eq!(deploy.attribute("max-age-seconds").as_deref(), Some("300"));
    assert_eq!(deploy.children_named("allow").len(), 1);

    // each top-level element of the default conf is merged: a component's copy of it changes the one element
    let defaults = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/components/base/framework/conf/MoquiDefaultConf.xml"),
    )
    .unwrap();
    let defaults = MNode::parse_text("moqui-conf", &defaults).unwrap();
    let merge_rules = include_str!("conf.rs");
    for child in defaults.children() {
        let name = child.name();
        assert!(
            merge_rules.contains(&format!("\"{name}\"")),
            "the conf element {name} has no merge rule in context/conf.rs, so a component's is dropped"
        );
    }
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

#[test]
fn the_edge_conf_merges_each_child_by_its_key() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><edge queue-retention-days="14" queue-max-attempts="10">
                <delegate service="a.B.call#One"/>
                <queue-kind name="mail" service="a.B.deliver#Mail"/>
                <replicate set="recipients"><entity name="a.Account" fields="address"/></replicate>
            </edge></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><edge queue-retention-days="7">
                <delegate service="a.B.call#Two"/>
                <queue-kind name="mail" service="c.D.deliver#Mail"/>
                <replicate set="recipients"><entity name="a.Identity" fields="name"/></replicate>
            </edge></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let edge = base.first_named("edge").unwrap();
    assert_eq!(edge.attribute("queue-retention-days").as_deref(), Some("7"));
    assert_eq!(edge.attribute("queue-max-attempts").as_deref(), Some("10"));
    let delegates: Vec<String> = edge
        .children_named("delegate")
        .iter()
        .filter_map(|node| node.attribute("service"))
        .collect();
    assert_eq!(delegates, ["a.B.call#One", "a.B.call#Two"]);
    let kinds = edge.children_named("queue-kind");
    assert_eq!(kinds.len(), 1);
    assert_eq!(kinds[0].attribute("service").as_deref(), Some("c.D.deliver#Mail"));
    let sets = edge.children_named("replicate");
    assert_eq!(sets.len(), 1);
    let entities: Vec<String> = sets[0]
        .children_named("entity")
        .iter()
        .filter_map(|node| node.attribute("name"))
        .collect();
    assert_eq!(entities, ["a.Identity"]);
}

/// The dev conf's datasource overrides carry only `pg-stat-statements`: each keeps the default conf's other attributes
/// and its `inline-jdbc` child (content's), and a datasource with no child stays without one.
#[test]
fn the_dev_conf_datasource_overrides_keep_the_default_jdbc_child() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    let base = MNode::parse_file(root.join("components/base/framework/conf/MoquiDefaultConf.xml")).unwrap();
    let over = MNode::parse_file(root.join("conf/MoquiDevConf.xml")).unwrap();
    let default_content = crate::entity::datasource::datasource_nodes(&base)
        .into_iter()
        .find(|node| node.attribute_is("group-name", "content"))
        .and_then(|node| node.first_named("inline-jdbc"))
        .map(|inline| inline.attribute("pool-maxsize"))
        .unwrap();
    let before = crate::entity::datasource::datasource_nodes(&base).len();
    super::merge_conf(&base, &over);
    let datasource = |group: &str| {
        crate::entity::datasource::datasource_nodes(&base)
            .into_iter()
            .find(|node| node.attribute_is("group-name", group))
            .unwrap()
    };
    let content = datasource("content");
    assert_eq!(content.attribute("pg-stat-statements").as_deref(), Some("true"));
    assert_eq!(content.attribute("database-conf-name").as_deref(), Some(""));
    let inline = content
        .first_named("inline-jdbc")
        .expect("content keeps its inline-jdbc");
    assert_eq!(inline.attribute("pool-maxsize"), default_content);
    assert!(
        inline.first_named("xa-properties").is_none(),
        "the connection is the datasource's attributes"
    );
    let transactional = datasource("transactional");
    assert_eq!(transactional.attribute("pg-stat-statements").as_deref(), Some("true"));
    assert_eq!(
        transactional.attribute("database-conf-name").as_deref(),
        Some("embedded")
    );
    assert!(transactional.first_named("inline-jdbc").is_none());
    // merged in place, none added
    assert_eq!(crate::entity::datasource::datasource_nodes(&base).len(), before);
}

#[test]
fn each_component_adds_authz_elements_and_keeps_the_defaults() {
    let base = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><user-facade><authz privileged-groups="ADMIN,ADMIN_ADV" sealed-permissions="ADMIN_PASSWORD"/>
            </user-facade></moqui-conf>"#,
    )
    .unwrap();
    let first = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><user-facade><authz><privileged-group name="APP_ADMIN"/><sealed-permission name="APP_GLOBAL"/>
            </authz></user-facade></moqui-conf>"#,
    )
    .unwrap();
    let second = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><user-facade><authz><privileged-group name="OTHER_ADMIN"/><privileged-group name="APP_ADMIN"/>
            </authz></user-facade></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &first);
    super::merge_conf(&base, &second);
    let authz = base
        .first_named("user-facade")
        .and_then(|facade| facade.first_named("authz"))
        .unwrap();
    let names = |element: &str| -> Vec<String> {
        authz
            .children_named(element)
            .iter()
            .filter_map(|child| child.attribute("name"))
            .collect()
    };
    // both components' elements apply, once each, and the defaults stay
    assert_eq!(names("privileged-group"), ["APP_ADMIN", "OTHER_ADMIN"]);
    assert_eq!(names("sealed-permission"), ["APP_GLOBAL"]);
    assert_eq!(authz.attribute("privileged-groups").as_deref(), Some("ADMIN,ADMIN_ADV"));
    assert_eq!(authz.attribute("sealed-permissions").as_deref(), Some("ADMIN_PASSWORD"));
    // a component that still sets the attribute replaces it, as before, and the elements stay
    let replacing = MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><user-facade><authz privileged-groups="ADMIN"/></user-facade></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &replacing);
    assert_eq!(authz.attribute("privileged-groups").as_deref(), Some("ADMIN"));
    assert_eq!(names("privileged-group"), ["APP_ADMIN", "OTHER_ADMIN"]);
}

/// Every keyed merge takes its key attributes from the conf schema (`conf_schema::merge_keys`), which asserts in a
/// debug build that its path is a keyed unit: merging one of each keyed element runs them all.
#[test]
fn every_keyed_merge_has_its_key_in_the_conf_schema() {
    let text = r#"<moqui-conf>
        <tools><tool-factory class="t"/></tools>
        <cache-list><cache name="c"/></cache-list>
        <telemetry><artifact type="AT_SERVICE"/><operations><operation service="s"/></operations></telemetry>
        <webapp-list><webapp name="webroot">
            <root-screen host=".*" location="l"/><error-screen error="e"/><token-route name="r"/>
            <token-path pattern="p"/><public-site host="h"/><listener class="l"/><endpoint path="/x"/>
            <filter name="f"><init-param name="i" value="v"/></filter>
            <servlet name="s"><init-param name="i" value="v"/></servlet>
            <web-protection><rate-limit name="r"/><ban name="b"/><client-rule name="c"/><path-rule name="p"/>
                <user-agent-rule name="u"/></web-protection>
        </webapp></webapp-list>
        <artifact-execution-facade><artifact-execution type="AT_SERVICE"/></artifact-execution-facade>
        <user-facade><authz><sealed-permission name="s"/><privileged-group name="g"/></authz></user-facade>
        <resource-facade><resource-reference scheme="s"/><template-renderer extension="e"/><script-runner extension="e"/></resource-facade>
        <screen-facade><screen-text-output type="t"/><screen-output type="t"/>
            <screen location="l"><subscreens-item name="i"/></screen></screen-facade>
        <service-facade><service-location name="l"/><service-type name="t"/><service-file location="f"/>
            <startup-service name="s"/><thread-pool><run-from-pool name="p"/></thread-pool></service-facade>
        <llm-facade><endpoint id="e"><header name="h"/></endpoint><model id="m"><reasoning level="low"/></model>
            <bind feature="f"/><data-entity package="p" entity="e"/><user-facts service="s"/>
            <constitution-provider service="s"/><ai-task name="t"/></llm-facade>
        <entity-facade><datasource group-name="g"/></entity-facade>
        <database-list><dictionary-type type="t"/><database name="d"><database-type type="t"/></database></database-list>
        <repository-list><repository name="r"><init-param name="i"/></repository></repository-list>
        <edge><delegate service="s"/><queue-kind name="q"/><replicate set="s"/></edge>
        <instance-hosting><token-command name="t"/></instance-hosting>
    </moqui-conf>"#;
    let base = MNode::parse_text("moqui-conf", text).unwrap();
    let over = MNode::parse_text("moqui-conf", text).unwrap();
    super::merge_conf(&base, &over);
    assert_eq!(
        base.first_named("webapp-list").unwrap().children_named("webapp").len(),
        1,
        "the same key merges into one element"
    );
}

/// Every element the XSD takes under `moqui-conf` is one the conf schema knows, so the generic merge of a component's
/// own settings element never merges a framework element a second time; an add-on's element (mr-pass's `pass`) merges
/// by that rule: its attributes replace, a child with a name merges into the base's, any other child is added.
#[test]
fn an_add_on_element_merges_as_settings_and_the_frameworks_are_known() {
    let xsd = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../runtime/components/base/framework/xsd/moqui-conf-3.xsd"),
    )
    .unwrap();
    let start = xsd.find(r#"<xs:element name="moqui-conf">"#).unwrap();
    let end = start + xsd[start..].find("</xs:sequence>").unwrap();
    let names: Vec<&str> = xsd[start..end]
        .split(r#"ref=""#)
        .skip(1)
        .filter_map(|rest| rest.split('"').next())
        .collect();
    assert!(names.len() > 40, "{names:?}");
    for name in names {
        assert!(
            super::super::conf_schema::is_known_element(name),
            "the conf element {name} is not in the conf schema, so a component's copy would also merge as settings"
        );
    }
    let base = MNode::parse_text(
        "base",
        r#"<moqui-conf><pass backend-url="https://a"><lane name="a" size="1"/></pass></moqui-conf>"#,
    )
    .unwrap();
    let over = MNode::parse_text(
        "over",
        r#"<moqui-conf><pass backend-url="https://b"><lane name="a" size="2"/><lane name="b"/><note/></pass>
            <mantle content-root="/c"/></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    let pass = base.first_named("pass").unwrap();
    assert_eq!(pass.attribute("backend-url").as_deref(), Some("https://b"));
    assert_eq!(pass.children_named("lane").len(), 2);
    let a = pass.first_named_attrs("lane", &[("name", "a")]).unwrap();
    assert_eq!(a.attribute("size").as_deref(), Some("2"));
    assert_eq!(pass.children_named("note").len(), 1);
    assert_eq!(
        base.first_named("mantle").unwrap().attribute("content-root").as_deref(),
        Some("/c")
    );
}

/// A `default-property` of a component or runtime conf is kept in the merge, each one whole, so the boot's refusal of
/// retired properties names it (plans/archive/Conf-Paths.md, phase 7).
#[test]
fn a_default_property_in_a_later_conf_reaches_the_refusal() {
    let base = MNode::parse_text("base", "<moqui-conf><instance purpose=\"dev\"/></moqui-conf>").unwrap();
    let over = MNode::parse_text(
        "over",
        r#"<moqui-conf><default-property name="webapp_http_port" value="9999"/>
            <default-property name="entity_ds_host" value="db"/></moqui-conf>"#,
    )
    .unwrap();
    super::merge_conf(&base, &over);
    assert_eq!(base.children_named("default-property").len(), 2);
    let err = super::super::conf_env::refuse_retired(&base).unwrap_err().to_string();
    assert!(err.contains("webapp_http_port") && err.contains("entity_ds_host"), "{err}");
}
