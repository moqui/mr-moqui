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

//! The transaction cache with an execution context entered: entity ECA rules and the audit log of held writes
//! (plans/archive/TX-Cache.md G2, G6). The rest of its tests are in `moqui-entity` (`entity/cache/tx_tests.rs`).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use moqui_lib::object::Value;

use super::EntityFacade;
use crate::transaction::TransactionAttribute;

const TEST: &str = "moqui.test.TestEntity";

fn framework_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework")
}

fn foo_text(facade: &EntityFacade, id: &str) -> Option<Value> {
    facade
        .find("moqui.test.Foo")
        .unwrap()
        .eq("fooId", Value::string(id))
        .one()
        .unwrap()
        .and_then(|row| row.get("fooText").cloned())
}

/// G2: the run-after entity ECA rules of a write held in a transaction-cache frame run, as for a direct write, and a
/// `get-original-value` rule sees the row as it was before the write.
#[test]
fn a_held_write_runs_its_after_eca_rules() {
    crate::script::clear_context_stack();
    let facade = crate::testing::open_with(
        &framework_dir(),
        crate::testing::Options {
            entity_eca: true,
            ..Default::default()
        },
    );
    super::load_feed::add_eca_rules(
        &facade,
        r#"<eecas>
            <eeca id="TxCacheAfter" entity="moqui.test.TestEntity" on-update="true" get-original-value="true">
                <actions><script>
                    const seen = ec.entity.makeValue("moqui.test.Foo");
                    seen.fooId = "AFTER-" + entityValue.testId;
                    seen.fooText = entityValue.value + "|" + originalValue.value;
                    seen.create();
                </script></actions>
            </eeca>
        </eecas>"#,
    )
    .unwrap();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut row = facade.make_value(TEST).unwrap();
    row.set("testId", Value::string("HELD"));
    row.set("value", Value::string("v0"));
    row.create(&facade).unwrap();
    let tx = facade.transactions_for(TEST).unwrap();
    tx.run(TransactionAttribute::Cache, None, || {
        let mut row = facade.make_value(TEST)?;
        row.set("testId", Value::string("HELD"));
        row.set("value", Value::string("v1"));
        row.update(&facade)?;
        assert!(tx.transaction_cache_active());
        assert_eq!(
            foo_text(&facade, "AFTER-HELD"),
            Some(Value::string("v1|v0")),
            "ran in the frame"
        );
        Ok(())
    })
    .unwrap();
    assert_eq!(foo_text(&facade, "AFTER-HELD"), Some(Value::string("v1|v0")));
}

/// G6: an audited update held in a transaction-cache frame reads its stored row once, for the update and its audit log.
#[test]
fn a_held_audited_update_reads_its_stored_row_once() {
    crate::script::clear_context_stack();
    let tape = Arc::new(Mutex::new(Vec::new()));
    let facade = crate::testing::open_with(
        &framework_dir(),
        crate::testing::Options {
            tape: Some(Arc::clone(&tape)),
            ..Default::default()
        },
    );
    facade.set_disable_audit_log(false);
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
    user.set("userId", Value::string("AUDIT1"));
    user.set("username", Value::string("audit.one"));
    user.create(&facade).unwrap();
    let reads = || {
        tape.lock()
            .unwrap()
            .iter()
            .filter(|sql| sql.starts_with("SELECT") && sql.contains("\"user_account\""))
            .count()
    };
    let tx = facade.transactions_for("moqui.security.UserAccount").unwrap();
    tx.run(TransactionAttribute::Cache, None, || {
        let before = reads();
        let mut user = facade.make_value("moqui.security.UserAccount")?;
        user.set("userId", Value::string("AUDIT1"));
        user.set("username", Value::string("audit.two"));
        user.update(&facade)?;
        assert_eq!(reads() - before, 1, "{:?}", tape.lock().unwrap());
        Ok(())
    })
    .unwrap();
    let audits = facade
        .find("moqui.entity.EntityAuditLog")
        .unwrap()
        .eq("changedEntityName", Value::string("moqui.security.UserAccount"))
        .eq("changedFieldName", Value::string("username"))
        .list()
        .unwrap();
    assert!(
        audits
            .values()
            .iter()
            .any(|row| row.get("oldValueText") == Some(&Value::string("audit.one"))),
        "{audits:?}"
    );
}

/// A transaction begun inside a trace is a span (`AT_TRANSACTION`) under the trace, with its outcome, cache mode and
/// the transaction cache's counts; one outside a trace is none.
#[cfg(feature = "telemetry")]
#[test]
fn a_transaction_is_a_span_with_its_cache_counts_inside_a_trace() {
    use crate::entity::document::Document;
    use crate::telemetry::{Exporter, SpanKind};
    #[derive(Default)]
    struct Memory(Mutex<Vec<Document>>);
    impl Exporter for Memory {
        fn export(&self, index: &str, documents: Vec<Document>) -> Result<usize, moqui_lib::error::Error> {
            let count = documents.len();
            if index == "otel_spans" {
                self.0.lock().unwrap().extend(documents);
            }
            Ok(count)
        }
    }
    let context = crate::context::ExecutionContextFactory::test_context_with_conf(
        "<moqui-conf><telemetry sample-ratio=\"1\"/></moqui-conf>",
    );
    let facade = crate::testing::open();
    context.factory().set_entity(facade.clone());
    let telemetry = crate::telemetry::for_factory(context.factory());
    let memory = Arc::new(Memory::default());
    telemetry.set_exporter(memory.clone());
    let tx = facade.transactions_for(TEST).unwrap();
    let write = |id: &str| {
        tx.run(TransactionAttribute::Cache, None, || {
            let mut row = facade.make_value(TEST)?;
            row.set("testId", Value::string(id));
            row.create(&facade)?;
            Ok(())
        })
        .unwrap();
    };
    let root_id;
    {
        let _entered = context.enter();
        let _authz = context.disable_authz();
        write("OUTSIDE");
        let root = telemetry.start_span("job", SpanKind::Server, None);
        root_id = root.context().unwrap().span_id;
        {
            let _trace = crate::telemetry::attach(root.context());
            write("INSIDE");
        }
        root.finish();
    }
    telemetry.flush();
    let spans = memory.0.lock().unwrap();
    let transactions: Vec<_> = spans
        .iter()
        .filter(|span| span.get("name").and_then(Value::as_str) == Some("transaction"))
        .collect();
    assert_eq!(transactions.len(), 1, "{spans:?}");
    assert_eq!(
        transactions[0].get("parent_span_id").and_then(Value::as_str),
        Some(root_id.to_string().as_str())
    );
    let text = format!("{:?}", transactions[0]);
    for expected in [
        "commit",
        "hold",
        "moqui.tx_cache.held",
        "moqui.tx_cache.batch_round_trips",
    ] {
        assert!(text.contains(expected), "{expected} in {text}");
    }
    drop(spans);
    telemetry.shutdown();
}
