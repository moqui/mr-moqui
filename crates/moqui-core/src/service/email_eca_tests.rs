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

use crate::context::ExecutionContextFactory;

use crate::entity::EntityFacade;

#[test]
fn save_eca_email_message_stores_the_message_once() {
    use moqui_lib::object::Value;

    let facade = open();
    let context = ExecutionContextFactory::test_context_in(
        "",
        crate::service::ServicePools::idle(),
        crate::context::framework_dir(),
    );
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    crate::service::load_definitions().unwrap();
    let map = |pairs: Vec<(&str, Value)>| {
        Value::Map(pairs.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
    };
    let list = |items: &[&str]| Value::List(items.iter().map(|item| Value::string(*item)).collect());
    let mut parameters = BTreeMap::new();
    parameters.insert("emailServerId".to_string(), Value::string("ECA_IMAP"));
    parameters.insert(
        "fields".to_string(),
        map(vec![
            ("toList", list(&["a@example.com", "b@example.com"])),
            ("ccList", list(&[])),
            ("from", Value::string("from@example.com")),
            ("subject", Value::string("Saved")),
        ]),
    );
    parameters.insert(
        "bodyPartList".to_string(),
        Value::List(vec![map(vec![("contentText", Value::string("the body"))])]),
    );
    parameters.insert(
        "headers".to_string(),
        map(vec![("message-id", Value::string("<m1@example.com>"))]),
    );
    let out = crate::service::sync("org.moqui.impl.EmailServices.save#EcaEmailMessage", &parameters).unwrap();
    let errors = context.message().errors().join("\n");
    assert!(errors.is_empty(), "{errors}");
    let id = out
        .get("emailMessageId")
        .map(moqui_lib::object::to_plain_string)
        .unwrap_or_default();
    assert!(!id.is_empty(), "{out:?}");
    let saved = facade
        .find("moqui.basic.email.EmailMessage")
        .unwrap()
        .eq("emailMessageId", Value::string(&id))
        .one()
        .unwrap()
        .unwrap();
    let text = |field: &str| {
        saved
            .get(field)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    assert_eq!(text("toAddresses"), "a@example.com, b@example.com");
    assert_eq!(text("ccAddresses"), "");
    assert_eq!(text("body"), "the body");
    assert_eq!(text("messageId"), "<m1@example.com>");
    assert_eq!(text("statusId"), "ES_RECEIVED");

    // The same Message-ID from the same server is not stored again.
    let out = crate::service::sync("org.moqui.impl.EmailServices.save#EcaEmailMessage", &parameters).unwrap();
    assert!(
        out.get("emailMessageId").is_none_or(moqui_lib::object::is_empty),
        "{out:?}"
    );
}

fn open() -> EntityFacade {
    crate::testing::open()
}
