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

use std::collections::BTreeMap;

use moqui_lib::object::Value;

use super::{run_first_line, token_command};
use crate::context::ExecutionContextFactory;
use crate::service::ServicePools;

#[test]
fn a_token_command_is_named_by_the_conf_only() {
    let conf = r#"<moqui-conf><instance-hosting>
        <token-command name="ecr" command="aws ecr get-login-password --region us-east-1"/>
        <token-command name="empty" command=" "/>
    </instance-hosting></moqui-conf>"#;
    assert_eq!(
        token_command(conf, "ecr").as_deref(),
        Some("aws ecr get-login-password --region us-east-1")
    );
    assert_eq!(token_command(conf, "empty"), None);
    // the text of a row is never a command
    assert_eq!(token_command(conf, "aws ecr get-login-password"), None);
    assert_eq!(token_command("<moqui-conf/>", "ecr"), None);
}

#[cfg(unix)]
#[test]
fn the_token_command_returns_its_first_line_and_a_failure_is_an_error() {
    assert_eq!(run_first_line("printf tok\\nsecond").unwrap(), "tok");
    let err = run_first_line("false").unwrap_err();
    assert!(err.message().contains("token command false failed"), "{err}");
}

/// SEC-SCR-04: a script has no way to start a process; `get#RegistryToken` runs only what the conf names, for a holder
/// of INSTANCE_ADMIN.
#[test]
fn no_script_starts_a_process_and_the_registry_token_needs_the_conf_and_the_permission() {
    let facade = crate::testing::open();
    facade.assume_db_tables();
    let context =
        ExecutionContextFactory::test_context_in("", ServicePools::bounded(1, 4), crate::context::framework_dir());
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let paths = [crate::context::framework_dir().join("service/org/moqui/dev/InstanceServices.xml")];
    assert!(crate::service::lint(&paths).unwrap().is_empty());

    let err = crate::script::run_actions(r#"<actions><script>ec.resource.runTokenCommand("printf tok")</script></actions>"#)
        .unwrap_err();
    assert!(err.message().contains("runTokenCommand"), "{err}");

    {
        let _authz = context.disable_authz();
        let mut image = facade.make_value("moqui.server.instance.InstanceImage").unwrap();
        image.set("instanceImageId", Value::string("TOK1"));
        image.set("authTokenCmd", Value::string("printf planted"));
        image.create(&facade).unwrap();
    }
    let parameters = BTreeMap::from([("instanceImageId".to_string(), Value::string("TOK1"))]);
    // no user, so no INSTANCE_ADMIN
    let result = crate::service::sync("org.moqui.dev.InstanceServices.get#RegistryToken", &parameters);
    let refused = match &result {
        Err(err) => err.message().to_string(),
        Ok(out) => {
            assert!(!out.contains_key("authToken"), "{out:?}");
            context.message().errors().join("\n")
        }
    };
    assert!(refused.contains("INSTANCE_ADMIN") || refused.contains("logged in"), "{refused}");
    context.message().clear_errors();
    // with authorization off (a job), the row's text is still not a command
    let _authz = context.disable_authz();
    let out = crate::service::sync("org.moqui.dev.InstanceServices.get#RegistryToken", &parameters).unwrap_or_default();
    assert!(!out.contains_key("authToken"), "{out:?}");
    assert!(
        context.message().errors().join("\n").contains("is not a token-command of the conf"),
        "{:?}",
        context.message().errors()
    );
}
