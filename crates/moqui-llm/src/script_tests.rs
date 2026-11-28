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

use moqui_core::context::ExecutionContextFactory;
use moqui_core::script::{clear_context_stack, eval_condition, run_script};

const CONF: &str = r#"<moqui-conf><llm-facade>
    <endpoint id="script-fake" protocol="fake"/>
    <model id="script-model" endpoint="script-fake" name="fake-1"/>
    <bind feature="classify" model="script-model" reasoning="none"/>
    <bind feature="summarize" model="script-model" reasoning="none"/>
    <bind feature="extract" model="script-model" reasoning="none"/>
</llm-facade></moqui-conf>"#;

#[test]
fn ec_llm_complete_answers_with_the_checked_json_of_a_schema() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context_with_conf(CONF);
    context.factory().set_entity(moqui_core::testing::open());
    let _entered = context.enter();
    crate::provider::fake::clear("script-fake");
    let holds = |expression: &str| assert!(eval_condition(expression).unwrap(), "{expression}");
    holds("ec.llm.isEnabled() === true");
    holds(
        r#"(function () {
            const answer = ec.llm.complete({feature: "classify", prompt: "Invoice #12",
                schema: {type: "object", required: ["label"], properties: {label: {enum: ["invoice", "other"]}}}});
            return answer.json.label === "invoice" && answer.modelId === "script-model" && answer.finish === "stop"
                && answer.callIds.length === 1 && answer.inputTokens > 0;
        })()"#,
    );
    holds(r#"ec.llm.complete({model: "script-model", prompt: "Say hi"}).text === "Fake reply to: Say hi""#);
    // the typed AI tasks: their schemas give the fake's answers
    holds(r#"ec.llm.classify("Invoice #12", ["invoice", "other"]) === "invoice""#);
    holds(r#"typeof ec.llm.summarize("A long text.", {maxWords: 20}) === "string""#);
    holds(
        r#"(function () { const values = ec.llm.extract("Ship to Acme", {customer: "The customer"}); return "customer" in values; })()"#,
    );
    for (script, expected) in [
        (
            r#"ec.llm.complete({model: "script-model"})"#,
            "needs a prompt or messages",
        ),
        (r#"ec.llm.complete({tier: "huge", prompt: "x"})"#, "tier huge"),
        (
            r#"ec.llm.complete({model: "nope", prompt: "x"})"#,
            "model nope is not configured",
        ),
    ] {
        let error = run_script(script).unwrap_err().to_string();
        assert!(error.contains(expected), "{script}: {error}");
    }
}

/// A typed AI task inside a transaction is refused as `complete` is, and its options' `allowInTransaction` lets it
/// run (plans/archive/High-Level-Review.md HL-041).
#[test]
fn ec_llm_tasks_in_a_transaction_are_refused_unless_they_opt_in() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context_with_conf(&CONF.replace("script-fake", "task-tx-fake"));
    let facade = moqui_core::testing::open();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    crate::provider::fake::clear("task-tx-fake");
    let transactions = facade.transactions_for("moqui.basic.Enumeration").unwrap();
    let in_transaction = |expression: &str| {
        transactions.run(moqui_core::transaction::TransactionAttribute::UseOrBegin, None, || {
            Ok(eval_condition(expression)?)
        })
    };
    for script in [
        r#"ec.llm.classify("Invoice #12", ["invoice", "other"]) !== null"#,
        r#"ec.llm.summarize("A long text.", {maxWords: 20}) !== null"#,
        r#"ec.llm.extract("Ship to Acme", {customer: "The customer"}) !== null"#,
    ] {
        let error = in_transaction(script).unwrap_err().to_string();
        assert!(error.contains(crate::provider::IN_TRANSACTION), "{script}: {error}");
    }
    assert!(
        crate::provider::fake::requests("task-tx-fake").is_empty(),
        "nothing was sent"
    );
    for expression in [
        r#"ec.llm.classify("Invoice #12", ["invoice", "other"], {allowInTransaction: true}) === "invoice""#,
        r#"typeof ec.llm.summarize("A long text.", {allowInTransaction: true}) === "string""#,
        r#""customer" in ec.llm.extract("Ship to Acme", {customer: "The customer"}, {allowInTransaction: "Y"})"#,
    ] {
        assert!(in_transaction(expression).unwrap(), "{expression}");
    }
    assert_eq!(crate::provider::fake::requests("task-tx-fake").len(), 3);
}

/// `ec.llm.embed` needs a feature or a model, refuses an open transaction, and reaches the embeddings API only
/// after `allowInTransaction` (the fake protocol has none, so nothing is posted).
#[test]
fn ec_llm_embed_asks_for_a_model_and_stays_out_of_a_transaction() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context_with_conf(CONF);
    let facade = moqui_core::testing::open();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let missing = run_script(r#"ec.llm.embed(["hello"])"#).unwrap_err().to_string();
    assert!(missing.contains("feature or a model"), "{missing}");
    // a blank text is refused by its index rather than dropped, which would shift the vectors after it
    let blank = run_script(r#"ec.llm.embed(["hello", "  ", "world"], {feature: "classify"})"#)
        .unwrap_err()
        .to_string();
    assert!(blank.contains("text 1 is blank"), "{blank}");
    let blank = run_script(r#"ec.llm.embed("", {feature: "classify"})"#)
        .unwrap_err()
        .to_string();
    assert!(blank.contains("text 0 is blank"), "{blank}");
    assert!(eval_condition(r#"ec.llm.embed([], {feature: "classify"}).vectors.length === 0"#).unwrap());
    let one = run_script(r#"ec.llm.embed("hello", {feature: "classify"})"#)
        .unwrap_err()
        .to_string();
    assert!(one.contains("no embeddings API"), "{one}");
    let transactions = facade.transactions_for("moqui.basic.Enumeration").unwrap();
    let in_transaction = |expression: &str| {
        transactions.run(moqui_core::transaction::TransactionAttribute::UseOrBegin, None, || {
            Ok(eval_condition(expression)?)
        })
    };
    let refused = in_transaction(r#"ec.llm.embed(["hi"], {feature: "classify"}) !== null"#)
        .unwrap_err()
        .to_string();
    assert!(refused.contains(crate::provider::IN_TRANSACTION), "{refused}");
    // allowed, the call is made with the transaction suspended, and the transaction is back in place after it
    let (no_api, resumed) = transactions
        .run(moqui_core::transaction::TransactionAttribute::UseOrBegin, None, || {
            let failed =
                eval_condition(r#"ec.llm.embed(["hi"], {feature: "classify", allowInTransaction: true}) !== null"#)
                    .unwrap_err()
                    .to_string();
            Ok((failed, transactions.in_transaction()))
        })
        .unwrap();
    assert!(no_api.contains("no embeddings API"), "{no_api}");
    assert!(resumed);
    let refused = run_script(r#"ec.llm.embed(["hi", {a: 1}], {feature: "classify"})"#)
        .unwrap_err()
        .to_string();
    assert!(refused.contains("text 1 is not a string or a number"), "{refused}");
}

const STUB_HOST: &str = "https://embed-stub.llm.test/";

/// Answers `/v1/embeddings` on [`STUB_HOST`] with the vectors out of `index` order, keeping each request; any other
/// request goes on, as a test elsewhere in the crate may make one meanwhile.
struct EmbeddingsStub {
    seen: std::sync::Mutex<Vec<moqui_core::outbound::Request>>,
}

impl moqui_core::outbound::HttpSender for EmbeddingsStub {
    fn send(
        &self,
        request: moqui_core::outbound::Request,
    ) -> Result<moqui_core::outbound::Response, moqui_lib::error::Error> {
        if !request.url.starts_with(STUB_HOST) {
            return moqui_core::outbound::send_default(request);
        }
        self.seen.lock().unwrap().push(request);
        let body = serde_json::json!({"data": [
            {"index": 1, "embedding": [0.5, 2]},
            {"index": 0, "embedding": [1.5, -0.25]}
        ], "usage": {"prompt_tokens": 3}});
        Ok(moqui_core::outbound::Response {
            status: 200,
            body: body.to_string(),
            bytes: Vec::new(),
            headers: vec![("content-type".to_string(), "application/json".to_string())],
        })
    }
}

/// `ec.llm.embed` gives the vectors back in the order of the texts, as numbers, with the model's id and name and
/// the dimension; with `allowInTransaction` the request is cut to the time the transaction has left.
#[test]
fn ec_llm_embed_returns_the_vectors_in_the_order_of_the_texts() {
    let _turn = moqui_core::user::session::cookie_property_lock();
    clear_context_stack();
    let context = ExecutionContextFactory::test_context_with_conf(&format!(
        r#"<moqui-conf><llm-facade>
            <endpoint id="embed-stub" protocol="chat-completions" url="{STUB_HOST}" local="true"/>
            <model id="embed-stub-model" endpoint="embed-stub" name="stub-embed-1"/>
        </llm-facade></moqui-conf>"#
    ));
    let facade = moqui_core::testing::open();
    context.factory().set_entity(facade.clone());
    let _entered = context.enter();
    let stub = std::sync::Arc::new(EmbeddingsStub {
        seen: std::sync::Mutex::new(Vec::new()),
    });
    moqui_core::outbound::install_http_sender(stub.clone());
    // the stub goes when the test ends, a failed assertion too
    struct ClearSender;
    impl Drop for ClearSender {
        fn drop(&mut self) {
            moqui_core::outbound::clear_http_sender();
        }
    }
    let _clear = ClearSender;
    let embedded = |options: &str| {
        let out = moqui_core::script::eval_value(&format!(
            r#"JSON.stringify(ec.llm.embed(["first", "second"], {options}))"#
        ))
        .unwrap();
        serde_json::from_str::<serde_json::Value>(&moqui_lib::object::to_plain_string(&out)).unwrap()
    };
    let expected = serde_json::json!({
        "vectors": [[1.5, -0.25], [0.5, 2]],
        "dimension": 2,
        "modelId": "embed-stub-model",
        "modelName": "stub-embed-1",
    });
    assert_eq!(embedded(r#"{model: "embed-stub-model"}"#), expected);
    let transactions = facade.transactions_for("moqui.basic.Enumeration").unwrap();
    let in_transaction = transactions
        .run(
            moqui_core::transaction::TransactionAttribute::UseOrBegin,
            Some(std::time::Duration::from_secs(30)),
            || Ok(embedded(r#"{model: "embed-stub-model", allowInTransaction: true}"#)),
        )
        .unwrap();
    assert_eq!(in_transaction, expected);
    // past its deadline nothing is sent
    let llm = crate::facade(context.factory());
    let target = llm
        .resolve(
            &facade,
            &crate::registry::Want {
                model: Some("embed-stub-model"),
                ..crate::registry::Want::default()
            },
        )
        .unwrap();
    let late = llm
        .embed_batches(
            &facade,
            &target,
            &["late".to_string()],
            std::time::Duration::from_secs(5),
            Some(std::time::Instant::now()),
        )
        .unwrap();
    assert!(late.vectors.is_empty());
    assert_eq!(
        late.error.map(|error| error.class),
        Some(crate::provider::ErrorClass::Timeout)
    );
    drop(_clear);
    let seen = stub.seen.lock().unwrap();
    assert_eq!(seen.len(), 2);
    assert_eq!(seen[0].url, format!("{STUB_HOST}v1/embeddings"));
    let body: serde_json::Value = serde_json::from_str(seen[0].body.as_deref().unwrap()).unwrap();
    assert_eq!(
        body,
        serde_json::json!({"model": "stub-embed-1", "input": ["first", "second"]})
    );
    let limit = |request: &moqui_core::outbound::Request| request.timeout.unwrap();
    assert!(
        limit(&seen[0]) > std::time::Duration::from_secs(60),
        "{:?}",
        limit(&seen[0])
    );
    assert!(
        limit(&seen[1]) <= std::time::Duration::from_secs(30),
        "{:?}",
        limit(&seen[1])
    );
}

/// `ec.llm.stream` calls back with the text so far as it comes, and gives what `complete` gives; a callback that returns
/// false stops the call with the text so far, and one that throws stops it with the error.
#[test]
fn ec_llm_stream_calls_back_with_the_text_so_far_and_can_stop() {
    clear_context_stack();
    let context = ExecutionContextFactory::test_context_with_conf(&CONF.replace("script-fake", "stream-fake"));
    context.factory().set_entity(moqui_core::testing::open());
    let _entered = context.enter();
    crate::provider::fake::clear("stream-fake");
    let holds = |expression: &str| assert!(eval_condition(expression).unwrap(), "{expression}");
    holds(
        r#"(function () {
            const seen = [];
            const answer = ec.llm.stream({model: "script-model", prompt: "Say hi"}, (text) => { seen.push(text); });
            return seen.length > 1 && seen[seen.length - 1] === answer.text && answer.text === "Fake reply to: Say hi"
                && seen.every((text, i) => i === 0 || text.startsWith(seen[i - 1])) && answer.finish === "stop"
                && answer.callIds.length === 1;
        })()"#,
    );
    holds(
        r#"(function () {
            const seen = [];
            const answer = ec.llm.stream({model: "script-model", prompt: "Say hi"}, (text) => { seen.push(text); return false; });
            return seen.length === 1 && answer.text === seen[0] && answer.text.length < "Fake reply to: Say hi".length
                && answer.finish === "canceled" && answer.json === null;
        })()"#,
    );
    holds(
        r#"(function () {
            const answer = ec.llm.stream({feature: "classify", prompt: "Invoice #12",
                schema: {type: "object", required: ["label"], properties: {label: {enum: ["invoice", "other"]}}}}, () => true);
            return answer.json.label === "invoice" && answer.finish === "stop";
        })()"#,
    );
    let error = run_script(
        r#"ec.llm.stream({model: "script-model", prompt: "Say hi"}, () => { throw new Error("no more"); })"#,
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("no more"), "{error}");
    let error = run_script(r#"ec.llm.stream("Say hi", () => true)"#)
        .unwrap_err()
        .to_string();
    assert!(error.contains("ec.llm.stream takes an options map"), "{error}");
}
