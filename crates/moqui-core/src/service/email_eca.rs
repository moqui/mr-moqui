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

//! Email ECA. Rules come from the factory's `*.emecas.xml` files. Actions run through the same
//! XML action host as service ECA.

use std::collections::BTreeMap;

use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::eca_rules;
use crate::context::Error;

#[derive(Clone, Debug)]
pub(super) struct EmailRule {
    condition: String,
    actions: String,
}

pub(super) fn email_rule_from(node: &MNode) -> EmailRule {
    EmailRule {
        condition: super::rule_actions::first_condition(node),
        actions: node
            .first_named("actions")
            .map(|child| child.write_string())
            .unwrap_or_default(),
    }
}

/// Run email ECA actions whose condition passes. `fields` are placed on the script context. A rule that fails is
/// logged and the others still run; the first failure is returned, so the poll leaves that message on the server.
pub fn run_email_rules(fields: &BTreeMap<String, Value>) -> Result<(), Error> {
    let Some(rules) = eca_rules::current() else {
        return Ok(());
    };
    let mut first = None;
    for rule in &rules.email {
        let ran = (|| -> Result<(), Error> {
            let _frame = super::rule_actions::publish_names(fields)?;
            if super::rule_actions::condition_allows(&rule.condition)? {
                super::rule_actions::run_stored_actions(&rule.actions)?;
            }
            Ok(())
        })();
        if let Err(err) = ran {
            tracing::error!("email ECA rule failed: {err}");
            first.get_or_insert(err);
        }
    }
    first.map_or(Ok(()), Err)
}

#[cfg(all(test, feature = "mail"))]
mod tests {
    use std::collections::BTreeMap;

    use super::run_email_rules;
    use crate::context::ExecutionContextFactory;
    use crate::service::eca_rules;

    use crate::entity::EntityFacade;

    #[test]
    fn email_eca_runs_its_action_and_sends_the_screen_mail() {
        let _lock = crate::user::session::cookie_property_lock();
        let facade = open();
        let context = ExecutionContextFactory::test_context_in(
            "",
            crate::service::ServicePools::idle(),
            crate::context::framework_dir(),
        );
        eca_rules::install(
            context.factory(),
            eca_rules::from_xml(
                "",
                r#"<emecas>
                <emeca rule-name="SendNotice">
                    <condition><expression>true</expression></condition>
                    <actions>
                        <script>ec.message.addMessage("emailEcaRan");</script>
                        <service-call name="org.moqui.impl.EmailServices.send#EmailTemplate">
                            <field-map field-name="emailTemplateId" value="T1"/>
                            <field-map field-name="toAddresses" value="a@example.com"/>
                        </service-call>
                    </actions>
                </emeca>
            </emecas>"#,
            ),
        );
        context.factory().set_entity(facade.clone());
        let _entered = context.enter();
        crate::service::load_definitions().unwrap();
        // No template yet: the send adds an error and the rule's generated code returns, as in Java.
        run_email_rules(&BTreeMap::new()).unwrap();
        let errors = context.message().errors().join("\n");
        assert!(errors.contains("No EmailTemplate record found"), "{errors}");
        assert!(context.message().messages().iter().any(|text| text == "emailEcaRan"));
        context.message().clear();

        // With the template, its body screen renders in html and text and the mail goes out.
        let body = format!("{}/src/service/testdata/mail/MailBody.xml", env!("CARGO_MANIFEST_DIR"));
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailServer",
            &[("emailServerId", "ECA_SMTP"), ("smtpHost", "smtp.example.com")],
        );
        seed(
            &facade,
            &context,
            "moqui.basic.email.EmailTemplate",
            &[
                ("emailTemplateId", "T1"),
                ("emailServerId", "ECA_SMTP"),
                ("fromAddress", "from@example.com"),
                ("subject", "Noticed"),
                ("bodyScreenLocation", &body),
            ],
        );
        let sink = crate::service::SinkGuard::install();
        run_email_rules(&BTreeMap::new()).unwrap();
        let errors = context.message().errors().join("\n");
        assert!(errors.is_empty(), "{errors}");
        let sent = sink.sent();
        assert_eq!(sent.len(), 1);
        assert_eq!(sent[0].to, ["a@example.com"]);
        assert!(sent[0].body.contains("Hello nobody"), "{}", sent[0].body);
        assert!(sent[0].body_text.contains("Subject: Noticed"), "{}", sent[0].body_text);
    }

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

    fn seed(facade: &EntityFacade, context: &crate::context::ExecutionContext, entity: &str, fields: &[(&str, &str)]) {
        let _authz = context.disable_authz();
        let mut row = facade.make_value(entity).expect("make");
        for (name, value) in fields {
            row.set(*name, moqui_lib::object::Value::string(*value));
        }
        row.create(facade).expect("create");
    }

    fn open() -> EntityFacade {
        crate::testing::open()
    }
}
