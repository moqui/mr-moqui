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

#[test]
fn a_fence_nests_lifts_and_keeps_what_it_refused() {
    assert!(!active());
    assert!(check_write("moqui.basic.Enumeration", "create").is_ok());
    let outer = enter("tool data");
    assert!(active());
    assert_eq!(label().as_deref(), Some("tool data"));
    let refused = check_write("moqui.basic.Enumeration", "create").unwrap_err();
    assert!(is_fence_error(&refused), "{refused}");
    assert!(
        refused.message().contains("create of moqui.basic.Enumeration"),
        "{refused}"
    );
    assert!(refused.message().contains("tool data"), "{refused}");
    assert!(check_write("moqui.agent.AgentRunEvent", "create").is_ok());
    assert!(check_write("moqui.llm.LlmCall", "create").is_ok());
    // the agent's configuration and the user's decisions are not run bookkeeping
    for entity in [
        "moqui.agent.AgentApproval",
        "moqui.agent.AgentToolPolicy",
        "moqui.agent.AgentMemory",
        "moqui.llm.LlmEndpoint",
    ] {
        assert!(check_write(entity, "update").is_err(), "{entity}");
    }
    assert!(check_write("moqui.entity.SequenceValueItem", "update").is_ok());
    assert!(check_write("moqui.agentx.Other", "create").is_err());
    {
        let lifted = lift();
        assert!(!active());
        assert!(check_effect(Effect::Mail, "send#EmailTemplate").is_ok());
        {
            let inner = enter("render /apps/x");
            assert!(check_effect(Effect::AsyncCall, "a#B").is_err());
            assert_eq!(inner.violations().len(), 1);
        }
        assert!(!active());
        drop(lifted);
    }
    assert!(active());
    assert_eq!(
        outer.violations(),
        vec![
            Violation::Write {
                entity: "moqui.basic.Enumeration".to_string(),
                operation: "create".to_string()
            },
            Violation::Write {
                entity: "moqui.agent.AgentApproval".to_string(),
                operation: "update".to_string()
            },
            Violation::Write {
                entity: "moqui.agent.AgentToolPolicy".to_string(),
                operation: "update".to_string()
            },
            Violation::Write {
                entity: "moqui.agent.AgentMemory".to_string(),
                operation: "update".to_string()
            },
            Violation::Write {
                entity: "moqui.llm.LlmEndpoint".to_string(),
                operation: "update".to_string()
            },
            Violation::Write {
                entity: "moqui.agentx.Other".to_string(),
                operation: "create".to_string()
            },
        ]
    );
    drop(outer);
    assert!(!active());
}

#[test]
fn a_sim_preview_writes_entities_and_refuses_every_effect_through_a_lift() {
    let guard = enter_sim("the preview of approval A1");
    assert!(active());
    assert!(
        check_write("moqui.basic.Enumeration", "create").is_ok(),
        "into the overlay"
    );
    assert!(
        check_write("moqui.agent.AgentApproval", "update").is_err(),
        "the agent's records stay real"
    );
    assert!(check_effect(Effect::AsyncCall, "a#B").is_ok(), "sim skips it");
    {
        // an approved transition inside the preview's render: the read fence lifts, the sim fence stays
        let _render = enter("render /apps/x");
        assert!(check_write("moqui.basic.Enumeration", "update").is_err());
        let _lifted = lift();
        assert!(check_write("moqui.basic.Enumeration", "update").is_ok());
        for effect in [
            Effect::Resource,
            Effect::Notify,
            Effect::Mail,
            Effect::RemoteCall,
            Effect::Print,
        ] {
            let refused = check_effect(effect, "x").unwrap_err();
            assert!(refused.message().contains("writes only to the sim"), "{refused}");
        }
        assert!(check_sql("delete from enumeration").is_err());
    }
    assert_eq!(guard.violations().len(), 7, "{:?}", guard.violations());
    drop(guard);
    assert!(!active());
    // the read fence refuses the new effects as well
    let read = enter("tool read_thing");
    assert!(check_effect(Effect::Resource, "runtime://x").is_err());
    assert!(check_effect(Effect::Notify, "Topic").is_err());
    assert_eq!(read.violations().len(), 2);
}

#[test]
fn only_a_plain_query_is_run_under_the_fence() {
    assert!(check_sql("DELETE FROM x").is_ok(), "not fenced");
    let guard = enter("tool data");
    for query in [
        "SELECT * FROM moqui_basic_enumeration",
        "with t as (select 1) select * from t",
        "EXPLAIN SELECT 1",
        "show search_path",
    ] {
        assert!(check_sql(query).is_ok(), "{query}");
    }
    for write in [
        "UPDATE enumeration SET description = 'x'",
        "with d as (delete from enumeration returning *) select * from d",
        "select * into copy_of from enumeration",
        "select nextval('seq')",
        "select 1; drop table enumeration",
        "INSERT INTO t VALUES (1) RETURNING id",
        "vacuum",
    ] {
        let refused = check_sql(write).unwrap_err();
        assert!(is_fence_error(&refused), "{write}");
    }
    assert_eq!(guard.violations().len(), 7);
}

#[test]
fn a_remote_permit_lets_only_its_origin_through_and_never_in_a_sim() {
    assert!(check_remote("https://a.example:443", "x").is_ok(), "not fenced");
    let read = enter("tool run_tool");
    assert!(check_remote("https://a.example:443", "MCP a").is_err(), "no permit");
    {
        let _permit = enter_remote("MCP a", "https://A.example:443");
        assert!(active(), "a permit is no lift");
        assert!(check_remote("https://a.example:443", "MCP a").is_ok());
        assert!(
            check_remote("https://b.example:443", "MCP b").is_err(),
            "another origin"
        );
        assert!(
            check_write("moqui.basic.Enumeration", "create").is_err(),
            "still no writes"
        );
        assert!(
            check_effect(Effect::RemoteCall, "HTTP GET x").is_err(),
            "nor other remote calls"
        );
        assert_eq!(label().as_deref(), Some("tool run_tool"));
    }
    assert!(
        check_remote("https://a.example:443", "MCP a").is_err(),
        "the permit is gone"
    );
    assert_eq!(
        read.violations().len(),
        5,
        "kept on the read fence: {:?}",
        read.violations()
    );
    drop(read);
    let sim = enter_sim("the preview of approval A1");
    {
        let _permit = enter_remote("MCP a", "https://a.example:443");
        assert!(
            check_remote("https://a.example:443", "MCP a").is_err(),
            "a sim refuses it"
        );
    }
    {
        let _render = enter("render");
        let _permit = enter_remote("MCP a", "https://a.example:443");
        assert!(
            check_remote("https://a.example:443", "MCP a").is_err(),
            "a render in a sim too"
        );
    }
    assert_eq!(sim.violations().len(), 1);
}

#[test]
fn a_cleared_thread_is_not_fenced_and_a_late_guard_drop_is_harmless() {
    let guard = enter("leaked");
    clear();
    assert!(!active());
    drop(guard);
    let again = enter("next");
    assert!(active());
    drop(again);
    assert!(!active());
}
