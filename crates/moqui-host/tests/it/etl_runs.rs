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

//! ETL services on the embedded cluster (plans/Tools-Database.md §10, §15 step 6): a saved mapping of a CSV in the
//! resource store checked (a missing foreign key, the diff), run on a worker to its EtlRun, and run against a bad file
//! that the database refuses, rolled back with its error rows and file; field matching and the grid's file info.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use crate::common;

const SERVICES: &str = "org.moqui.impl.EtlServices";

fn call(verb_noun: &str, parameters: &[(&str, Value)]) -> BTreeMap<String, Value> {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    moqui_core::service::call::sync_checked(
        &format!("{SERVICES}.{verb_noun}"),
        &parameters,
        // the Tools screens grant the services by inheritance; called alone, each checks the Tools app itself
        &moqui_core::service::call::CallOptions {
            disable_authz: true,
            ..moqui_core::service::call::CallOptions::default()
        },
    )
    .unwrap_or_else(|err| panic!("{verb_noun}: {err}"))
}

fn text(map: &BTreeMap<String, Value>, key: &str) -> String {
    map.get(key).map(to_plain_string).unwrap_or_default()
}

fn field(pairs: &[(&str, &str)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_string(), Value::string(*v)))
            .collect(),
    )
}

fn write(location: &str, text: &str) {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    moqui_core::context::ExecutionContext::with_current(|ec| {
        ec.factory().resource().write_bytes(location, text.as_bytes())
    })
    .unwrap()
    .unwrap();
}

#[test]
fn a_saved_mapping_is_checked_run_as_a_job_and_rolled_back_on_a_bad_file() {
    if !common::own_process() {
        return;
    }
    moqui_host::logging::install();
    let gate = common::start("etl-runs");
    let factory = gate.server.factory();
    let facade = factory.entity().expect("entity facade");
    let context = factory.new_context();
    let _entered = context.enter();
    moqui_core::user::internal_login_user("john.doe").unwrap();
    {
        let _authz = context.disable_authz();
        let mut row = facade.make_value("moqui.basic.EnumerationType").unwrap();
        row.set("enumTypeId", Value::string("EtlHostType"));
        row.set("description", Value::string("ETL host test"));
        row.create(&facade).unwrap();
    }
    write(
        "dbresource://moqui/etl/test/good.csv",
        "Id,Type,Name\nEtlHost1,EtlHostType,One\nEtlHost2,EtlHostType,Two\n",
    );
    write(
        "dbresource://moqui/etl/test/bad.csv",
        "Id,Type,Name\nEtlHost3,EtlHostType,Three\nEtlHost4,NoSuchType,Four\n",
    );
    let mut mapping = IndexMap::new();
    mapping.insert("description".to_string(), Value::string("enumerations"));
    mapping.insert("sourceTypeEnumId".to_string(), Value::string("EtlsFile"));
    mapping.insert(
        "sourceLocation".to_string(),
        Value::string("dbresource://moqui/etl/test/good.csv"),
    );
    mapping.insert("targetTypeEnumId".to_string(), Value::string("EtltEntity"));
    mapping.insert("targetName".to_string(), Value::string("moqui.basic.Enumeration"));
    mapping.insert(
        "fields".to_string(),
        Value::List(vec![
            field(&[("targetField", "enumId"), ("sourceColumn", "Id")]),
            field(&[("targetField", "enumTypeId"), ("sourceColumn", "Type")]),
            field(&[("targetField", "description"), ("sourceColumn", "Name")]),
        ]),
    );
    let stored = call("store#EtlMapping", &[("mapping", Value::Map(mapping))]);
    let mapping_id = text(&stored, "mappingId");
    assert!(!mapping_id.is_empty());

    // a check of the bad file: its missing foreign key, and the diff, with nothing written
    let check = call(
        "run#EtlMapping",
        &[
            ("mappingId", Value::string(&mapping_id)),
            ("check", Value::Bool(true)),
            ("wait", Value::Bool(true)),
            ("location", Value::string("dbresource://moqui/etl/test/bad.csv")),
        ],
    );
    assert_eq!(text(&check, "errorCount"), "1", "{check:?}");
    let Some(Value::List(errors)) = check.get("errors") else {
        panic!("{check:?}")
    };
    let Value::Map(error) = &errors[0] else {
        panic!("{errors:?}")
    };
    assert_eq!(to_plain_string(&error["rowNumber"]), "3");
    assert!(to_plain_string(&error["message"]).contains("foreign key"), "{error:?}");
    assert!(
        matches!(check.get("diff"), Some(Value::List(rows)) if rows.len() == 2),
        "{check:?}"
    );

    // a run on the worker pool of the good file
    let queued = call("run#EtlMapping", &[("mappingId", Value::string(&mapping_id))]);
    assert_eq!(text(&queued, "statusId"), "EtlrPending");
    let run_id = text(&queued, "runId");
    let started = Instant::now();
    let run = loop {
        let detail = call("get#EtlRun", &[("runId", Value::string(&run_id))]);
        let Some(Value::Map(run)) = detail.get("run") else {
            panic!("{detail:?}")
        };
        let status = to_plain_string(&run["statusId"]);
        if status == "EtlrDone" || status == "EtlrFailed" {
            break run.clone();
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the run did not end: {run:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(to_plain_string(&run["statusId"]), "EtlrDone", "{run:?}");
    assert_eq!(to_plain_string(&run["loadCount"]), "2");
    assert_eq!(to_plain_string(&run["lastCommittedRow"]), "3");
    for id in ["EtlHost1", "EtlHost2"] {
        let _authz = context.disable_authz();
        assert!(facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .eq("enumId", Value::string(id))
            .one()
            .unwrap()
            .is_some());
    }

    // the bad file: the database refuses the row with no type, and the run is rolled back whole
    let bad = call(
        "run#EtlMapping",
        &[
            ("mappingId", Value::string(&mapping_id)),
            ("wait", Value::Bool(true)),
            ("location", Value::string("dbresource://moqui/etl/test/bad.csv")),
        ],
    );
    assert_eq!(text(&bad, "statusId"), "EtlrFailed", "{bad:?}");
    assert_eq!(text(&bad, "lastCommittedRow"), "0", "{bad:?}");
    assert!(!text(&bad, "errorLocation").is_empty(), "{bad:?}");
    {
        let _authz = context.disable_authz();
        assert!(facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .eq("enumId", Value::string("EtlHost3"))
            .one()
            .unwrap()
            .is_none());
    }
    let detail = call("get#EtlRun", &[("runId", Value::string(text(&bad, "runId")))]);
    assert!(
        matches!(detail.get("errors"), Some(Value::List(rows)) if !rows.is_empty()),
        "{detail:?}"
    );

    let matched = call(
        "match#EtlFields",
        &[
            (
                "columns",
                Value::List(vec![Value::string("Enum Id"), Value::string("description")]),
            ),
            ("targetName", Value::string("moqui.basic.Enumeration")),
        ],
    );
    let Some(Value::List(matches)) = matched.get("matches") else {
        panic!("{matched:?}")
    };
    let targets: Vec<String> = matches
        .iter()
        .map(|row| match row {
            Value::Map(row) => to_plain_string(&row["targetField"]),
            _ => String::new(),
        })
        .collect();
    assert_eq!(targets, ["enumId", "description"]);

    let info = call(
        "get#GridFileInfo",
        &[("location", Value::string("dbresource://moqui/etl/test/good.csv"))],
    );
    assert_eq!(text(&info, "rowCount"), "2", "{info:?}");
    let cleaned = call("clean#GridUploads", &[]);
    assert_eq!(text(&cleaned, "deleted"), "0", "{cleaned:?}");
}
