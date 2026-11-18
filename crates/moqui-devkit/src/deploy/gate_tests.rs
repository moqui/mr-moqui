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

use moqui_core::context::conf_schema::{Class, CATALOG};
use moqui_core::context::ExecutionContextFactory;

use super::{Allow, GateConf};

const SECRET: &str = "issued-by-this-target-0123456789";

fn conf(require: bool) -> String {
    format!(
        r#"<moqui-conf><component-deploy require-passed-run="{require}" attestation-secret="{SECRET}"
            max-age-seconds="120" public-urls="https://a.example.com, https://b.example.com/">
            <allow component="site" repository-url="https://source.example.com/git/site.git" branch="main"/>
            <allow component="half" repository-url="https://source.example.com/git/half.git"/>
        </component-deploy></moqui-conf>"#
    )
}

/// The `component-deploy` conf: its public URLs, its allow entries (one that leaves out its branch is left out), and
/// an allow entry's URL compared without the case of its host or a trailing slash; the secret never in its Debug.
#[test]
fn the_gate_conf_names_its_public_urls_and_allow_entries() {
    let context = ExecutionContextFactory::test_context_with_conf(&conf(true));
    let gate = GateConf::read(context.factory());
    assert!(gate.require_passed_run);
    assert_eq!(gate.max_age_millis, 120_000);
    assert_eq!(gate.public_urls, ["https://a.example.com", "https://b.example.com/"]);
    assert_eq!(
        gate.allows,
        [Allow {
            component: "site".to_string(),
            repository_url: "https://source.example.com/git/site.git".to_string(),
            branch: "main".to_string(),
        }]
    );
    assert!(gate
        .allowed("site", "https://SOURCE.example.com/git/site.git/", "main")
        .is_some());
    assert!(gate
        .allowed("site", "https://source.example.com/git/site.git", "dev")
        .is_none());
    assert!(gate
        .allowed("half", "https://source.example.com/git/half.git", "main")
        .is_none());
    assert!(!format!("{gate:?}").contains(SECRET));

    // no public URL named and no webapp-list/webapp[webroot]@http-host: the server cannot tell its own URL, so it takes none
    let context = ExecutionContextFactory::test_context_with_conf("<moqui-conf><component-deploy/></moqui-conf>");
    let gate = GateConf::read(context.factory());
    assert!(gate.public_urls.is_empty() && gate.allows.is_empty() && !gate.require_passed_run);
}

/// The gate's conf is read once, at start: a conf reload of a running server that turns it off changes nothing until
/// the next start, and the conf reload says it waits for a restart.
#[test]
fn the_gate_conf_is_read_once_and_a_conf_reload_does_not_turn_the_gate_off() {
    let context = ExecutionContextFactory::test_context_with_conf(&conf(true));
    let factory = context.factory();
    let _entered = context.enter();
    assert!(crate::deploy::require_passed_run(factory));
    let off = moqui_lib::node::MNode::parse_text(
        "moqui-conf",
        r#"<moqui-conf><component-deploy require-passed-run="false" attestation-secret=""/></moqui-conf>"#,
    )
    .unwrap();
    // whatever the reload makes of the rest of this test's conf, the gate is as it was read
    let _ = moqui_core::context::conf_reload::apply_conf(factory, &off);
    assert!(crate::deploy::require_passed_run(factory));
    assert_eq!(super::conf(factory).secret, SECRET);
    let unit = CATALOG
        .iter()
        .find(|unit| unit.name == "component-deploy")
        .expect("the conf reload knows component-deploy");
    assert!(matches!(unit.class, Class::Restart(_)), "{:?}", unit.class);
    assert!(matches!(unit.attr_class("require-passed-run"), Class::Restart(_)));
}
