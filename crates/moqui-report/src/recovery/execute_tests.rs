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

use super::property_allowed;

/// SEC-TLS-12: an adapter reads a `recovery_` property, or one the conf lists; it sends none the conf marks secret
/// unless listed, and never any other property the process holds.
#[test]
fn an_adapter_reads_only_recovery_properties_and_sends_no_conf_secret() {
    let ec = moqui_core::context::ExecutionContextFactory::test_context_with_conf(
        r#"<moqui-conf><default-property name="recovery_token" value="" is-secret="true"/>
            <default-property name="recovery_signing_key" value=""/></moqui-conf>"#,
    );
    let _entered = ec.enter();
    for sent in [true, false] {
        assert!(property_allowed("entity_ds_crypt_pass", sent).is_err());
        assert!(property_allowed("OPS_HOOK_TOKEN", sent).is_err());
    }
    assert!(property_allowed("recovery_auth", true).is_ok());
    // a secret, by the conf or by its name, is not sent; it may key the HMAC, which is not sent
    let refused = property_allowed("recovery_token", true).unwrap_err();
    assert!(refused.contains("telemetry/recovery@properties"), "{refused}");
    assert!(property_allowed("recovery_signing_key", true).is_err());
    assert!(property_allowed("recovery_signing_key", false).is_ok());
    // the conf's list names any other, secret or not
    moqui_lib::env::set_property(
        "moqui_conf__telemetry__recovery__properties",
        "recovery_token, OPS_HOOK_TOKEN",
    );
    let listed = (
        property_allowed("recovery_token", true),
        property_allowed("OPS_HOOK_TOKEN", true),
    );
    moqui_lib::env::clear_property("moqui_conf__telemetry__recovery__properties");
    assert!(listed.0.is_ok() && listed.1.is_ok(), "{listed:?}");
}
