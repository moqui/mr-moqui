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
//! that the database refuses, rolled back with its error rows and file; field matching and the grid's file info. Then,
//! with authorization on as an ADMIN who is not in ADMIN_ADV: an expression refused without GROOVY_SHELL_WEB, a
//! location outside the allowed places refused, a UserAccount password lookup refused, and a run on the worker that
//! would add the user to ADMIN_ADV refused by the security rules.

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
    // both rows would be new
    assert!(
        matches!(check.get("diff"), Some(Value::List(rows)) if rows.len() == 2
            && rows.iter().all(|row| matches!(row, Value::Map(row) if row.get("notFound") == Some(&Value::Bool(true))))),
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
    // the error file is the run user's, outside the import root, and read through get#EtlRun
    assert!(
        text(&bad, "errorLocation").starts_with("dbresource://moqui/etl-run/EX_JOHN_DOE/"),
        "{bad:?}"
    );
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
    let detail = call(
        "get#EtlRun",
        &[
            ("runId", Value::string(text(&bad, "runId"))),
            ("includeErrorFile", Value::Bool(true)),
        ],
    );
    assert!(
        matches!(detail.get("errors"), Some(Value::List(rows)) if !rows.is_empty()),
        "{detail:?}"
    );
    assert!(text(&detail, "errorFile").contains("NoSuchType"), "{detail:?}");
    let mut entries = IndexMap::new();
    entries.insert("Active".to_string(), Value::string("ACTIVE"));
    let stored_map = call(
        "store#EtlValueMap",
        &[
            ("description", Value::string("john's")),
            ("entries", Value::Map(entries)),
        ],
    );
    let value_map_id = text(&stored_map, "valueMapId");
    assert!(!value_map_id.is_empty(), "{stored_map:?}");

    reruns(&context, &facade, &mapping_id, &text(&check, "runId"), &run_id);

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

    scheduled_runs_run_as_the_owner(factory, &facade, &mapping_id);
    authorization_is_on(&context, &facade, &text(&bad, "runId"), &value_map_id);
}

/// The errors `call` left in the message facade, cleared.
fn take_errors() -> String {
    moqui_core::context::ExecutionContext::with_current(|ec| {
        let errors = ec.message().errors().join("; ");
        ec.message().clear_errors();
        errors
    })
    .unwrap_or_default()
}

/// rerun#EtlRun: a run that read nothing (its file was not there yet) has nextStartRow 0, not 1, and keeps the location
/// it expanded; its rerun reads that file from row 0 though the mapping's own location is another file. A check, and a
/// run done with no errors, are not rerun.
fn reruns(
    context: &moqui_core::context::ExecutionContext,
    facade: &moqui_core::entity::EntityFacade,
    mapping_id: &str,
    check_run: &str,
    done_run: &str,
) {
    let missing = call(
        "run#EtlMapping",
        &[
            ("mappingId", Value::string(mapping_id)),
            ("wait", Value::Bool(true)),
            (
                "location",
                Value::string("dbresource://moqui/etl/test/rerun-${date}.csv"),
            ),
        ],
    );
    assert_eq!(text(&missing, "statusId"), "EtlrFailed", "{missing:?}");
    take_errors();
    let detail = call("get#EtlRun", &[("runId", Value::string(text(&missing, "runId")))]);
    let Some(Value::Map(run)) = detail.get("run") else {
        panic!("{detail:?}")
    };
    let location = to_plain_string(&run["sourceLocation"]);
    assert!(
        location.starts_with("dbresource://moqui/etl/test/rerun-") && !location.contains("${"),
        "{run:?}"
    );
    assert_eq!(to_plain_string(&run["startRow"]), "0", "{run:?}");
    assert_eq!(to_plain_string(&run["nextStartRow"]), "0", "{run:?}");
    assert_eq!(detail.get("canRerun"), Some(&Value::Bool(true)), "{detail:?}");

    // the file arrives: the rerun reads it, not the mapping's good.csv
    write(&location, "Id,Type,Name\nEtlHostRerun,EtlHostType,Again\n");
    let rerun = call(
        "rerun#EtlRun",
        &[
            ("runId", Value::string(text(&missing, "runId"))),
            ("wait", Value::Bool(true)),
        ],
    );
    assert_eq!(text(&rerun, "statusId"), "EtlrDone", "{rerun:?} {}", take_errors());
    assert_eq!(text(&rerun, "loadCount"), "1", "{rerun:?}");
    {
        let _authz = context.disable_authz();
        assert!(facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .eq("enumId", Value::string("EtlHostRerun"))
            .one()
            .unwrap()
            .is_some());
        let again = facade
            .find("moqui.etl.EtlRun")
            .unwrap()
            .eq("runId", Value::string(text(&rerun, "runId")))
            .one()
            .unwrap()
            .expect("the rerun's EtlRun");
        assert_eq!(again.get("sourceLocation").map(to_plain_string), Some(location.clone()));
        assert_eq!(again.get("nextStartRow").map(to_plain_string).as_deref(), Some("3"));
    }

    let run_count = || {
        let _authz = context.disable_authz();
        facade.find("moqui.etl.EtlRun").unwrap().list().unwrap().len()
    };
    let runs_before = run_count();
    for (run_id, refusal) in [
        (check_run, "is a check"),
        (done_run, "nothing left"),
        (text(&rerun, "runId").as_str(), "nothing left"),
    ] {
        let detail = call("get#EtlRun", &[("runId", Value::string(run_id))]);
        assert_eq!(detail.get("canRerun"), Some(&Value::Bool(false)), "{detail:?}");
        let refused = moqui_core::service::call::sync_checked(
            &format!("{SERVICES}.rerun#EtlRun"),
            &BTreeMap::from([("runId".to_string(), Value::string(run_id))]),
            &moqui_core::service::call::CallOptions {
                disable_authz: true,
                ..moqui_core::service::call::CallOptions::default()
            },
        );
        let errors = take_errors();
        let message = refused.expect_err("a refused rerun").message().to_string();
        assert!(
            message.contains(refusal) || errors.contains(refusal),
            "{run_id}: {message} {errors}"
        );
    }
    assert_eq!(run_count(), runs_before, "a refused rerun made a run");
}

/// Call with authorization on: the results and the errors the call added, taken out of the message facade.
fn call_as_user(verb_noun: &str, parameters: &[(&str, Value)]) -> (BTreeMap<String, Value>, String) {
    let parameters: BTreeMap<String, Value> = parameters
        .iter()
        .map(|(name, value)| ((*name).to_string(), value.clone()))
        .collect();
    let mark = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().error_mark()).unwrap();
    let results = moqui_core::service::call::sync_with(
        &format!("{SERVICES}.{verb_noun}"),
        &parameters,
        &moqui_core::service::call::CallOptions::default(),
    )
    .unwrap_or_else(|err| panic!("{verb_noun}: {err}"));
    let errors = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark))
        .unwrap_or_default()
        .join("; ");
    (results, errors)
}

fn csv_mapping(location: &str, target_type: &str, target: &str, fields: Vec<Value>) -> Value {
    let mut mapping = IndexMap::new();
    mapping.insert("sourceTypeEnumId".to_string(), Value::string("EtlsFile"));
    mapping.insert("sourceLocation".to_string(), Value::string(location));
    mapping.insert("targetTypeEnumId".to_string(), Value::string(target_type));
    mapping.insert("targetName".to_string(), Value::string(target));
    mapping.insert("fields".to_string(), Value::List(fields));
    Value::Map(mapping)
}

/// As an ADMIN who is not in ADMIN_ADV, with authorization on and the Tools screen as the caller (as the app calls the
/// services): what a mapping may not reach is refused, and a run on the worker is the user's own.
fn authorization_is_on(
    context: &moqui_core::context::ExecutionContext,
    facade: &moqui_core::entity::EntityFacade,
    other_run: &str,
    other_value_map: &str,
) {
    {
        let _authz = context.disable_authz();
        let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
        user.set("userId", Value::string("ETL_ADMIN_U"));
        user.set("username", Value::string("etl.admin"));
        user.set("disabled", Value::string("N"));
        user.create(facade).unwrap();
        let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
        member.set("userGroupId", Value::string("ADMIN"));
        member.set("userId", Value::string("ETL_ADMIN_U"));
        member.set("fromDate", Value::Timestamp(jiff::Timestamp::UNIX_EPOCH));
        member.create(facade).unwrap();
    }
    write(
        "dbresource://moqui/etl/test/admin-adv.csv",
        "group,user\nADMIN_ADV,ETL_ADMIN_U\n",
    );
    moqui_core::user::internal_login_user("etl.admin").unwrap();
    assert!(!moqui_core::user::has_permission("GROOVY_SHELL_WEB").unwrap());
    context
        .push_checked(
            "AT_XML_SCREEN",
            "component://tools/screen/Tools.xml",
            "all",
            moqui_core::context::Authz::Required,
            moqui_core::context::Tarpit::Skip,
            None,
        )
        .expect("an ADMIN may use the Tools app");

    // an expression needs GROOVY_SHELL_WEB, to save and to run
    let scripted = csv_mapping(
        "dbresource://moqui/etl/test/good.csv",
        "EtltEntity",
        "moqui.basic.Enumeration",
        vec![
            field(&[("targetField", "enumId"), ("sourceColumn", "Id")]),
            field(&[("targetField", "description"), ("expression", "ec.user.userId")]),
        ],
    );
    let (stored, errors) = call_as_user("store#EtlMapping", &[("mapping", scripted.clone())]);
    assert!(errors.contains("GROOVY_SHELL_WEB"), "{errors} {stored:?}");
    assert!(!stored.contains_key("mappingId"));
    let (ran, errors) = call_as_user("run#EtlMapping", &[("mapping", scripted), ("wait", Value::Bool(true))]);
    assert!(errors.contains("GROOVY_SHELL_WEB"), "{errors} {ran:?}");

    // a location outside the allowed places is not available, to a run and to the grid's file info
    let plain = vec![field(&[("targetField", "enumId"), ("sourceColumn", "Id")])];
    for location in [
        "component://tools/screen/Tools.xml",
        "runtime://conf/MoquiDevConf.xml",
        "dbresource://moqui/database/upload/EX_JOHN_DOE/1/a.csv",
    ] {
        let (ran, errors) = call_as_user(
            "run#EtlMapping",
            &[
                (
                    "mapping",
                    csv_mapping(location, "EtltEntity", "moqui.basic.Enumeration", plain.clone()),
                ),
                ("check", Value::Bool(true)),
                ("wait", Value::Bool(true)),
            ],
        );
        assert_eq!(text(&ran, "failure"), "Not available", "{location}: {errors} {ran:?}");
        let (info, errors) = call_as_user("get#GridFileInfo", &[("location", Value::string(location))]);
        assert_eq!(errors, "Not available", "{location}: {info:?}");
    }
    let (ran, errors) = call_as_user(
        "run#EtlMapping",
        &[
            (
                "mapping",
                csv_mapping(
                    "dbresource://moqui/etl/test/${ec.user.userId}.csv",
                    "EtltEntity",
                    "moqui.basic.Enumeration",
                    plain,
                ),
            ),
            ("check", Value::Bool(true)),
            ("wait", Value::Bool(true)),
        ],
    );
    assert!(text(&ran, "failure").contains("${date}"), "{errors} {ran:?}");

    // a lookup of a UserAccount password is refused
    let (ran, errors) = call_as_user(
        "check#EtlSample",
        &[(
            "mapping",
            csv_mapping(
                "dbresource://moqui/etl/test/good.csv",
                "EtltEntity",
                "moqui.basic.Enumeration",
                vec![
                    field(&[("targetField", "enumId"), ("sourceColumn", "Id")]),
                    field(&[
                        ("targetField", "description"),
                        ("sourceColumn", "Name"),
                        ("lookupEntityName", "moqui.security.UserAccount"),
                        ("lookupMatchField", "username"),
                        ("lookupResultField", "currentPassword"),
                    ]),
                ],
            ),
        )],
    );
    assert!(errors.contains("currentPassword"), "{errors} {ran:?}");
    assert!(!ran.contains_key("rows"));

    // the grid's file source: read only, its columns, rows (filtered and sorted), count and export from the file
    let grid = |verb_noun: &str, parameters: &[(&str, Value)]| {
        let parameters: BTreeMap<String, Value> = parameters
            .iter()
            .map(|(name, value)| ((*name).to_string(), value.clone()))
            .collect();
        let mark = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().error_mark()).unwrap();
        let results = moqui_core::service::call::sync_with(
            &format!("org.moqui.impl.DatabaseServices.{verb_noun}"),
            &parameters,
            &moqui_core::service::call::CallOptions::default(),
        )
        .unwrap();
        let errors = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark))
            .unwrap_or_default()
            .join("; ");
        (results, errors)
    };
    let file_source = |location: &str| {
        let mut source = IndexMap::new();
        source.insert("type".to_string(), Value::string("file"));
        source.insert("location".to_string(), Value::string(location));
        Value::Map(source)
    };
    let good = file_source("dbresource://moqui/etl/test/good.csv");
    let (columns, errors) = grid("get#DataGridColumns", &[("source", good.clone())]);
    assert!(errors.is_empty(), "{errors}");
    assert!(!text(&columns, "readOnlyReason").is_empty(), "{columns:?}");
    assert!(
        matches!(columns.get("columns"), Some(Value::List(list)) if list.len() == 3),
        "{columns:?}"
    );
    let mut filter = IndexMap::new();
    filter.insert("field".to_string(), Value::string("Name"));
    filter.insert("type".to_string(), Value::string("like"));
    filter.insert("value".to_string(), Value::string("tw"));
    let (rows, errors) = grid(
        "get#DataGridRows",
        &[
            ("source", good.clone()),
            ("filter", Value::List(vec![Value::Map(filter.clone())])),
        ],
    );
    assert!(errors.is_empty(), "{errors}");
    let Some(Value::List(data)) = rows.get("data") else {
        panic!("{rows:?}")
    };
    assert_eq!(data.len(), 1, "{rows:?}");
    let (counted, _) = grid("count#DataGridRows", &[("source", good.clone())]);
    assert_eq!(text(&counted, "count"), "2", "{counted:?}");
    let (exported, errors) = grid("export#DataGrid", &[("source", good)]);
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(text(&exported, "rowCount"), "2", "{exported:?}");
    let (refused, errors) = grid(
        "get#DataGridRows",
        &[("source", file_source("runtime://conf/MoquiDevConf.xml"))],
    );
    assert_eq!(errors, "Not available", "{refused:?}");

    // group membership and the ETL's own rows: refused at once as an entity target, as an entity-auto service target,
    // and through the entity-auto service itself
    let membership = |target_type: &str, target: &str| {
        csv_mapping(
            "dbresource://moqui/etl/test/admin-adv.csv",
            target_type,
            target,
            vec![
                field(&[("targetField", "userGroupId"), ("sourceColumn", "group")]),
                field(&[("targetField", "userId"), ("sourceColumn", "user")]),
            ],
        )
    };
    for (target_type, target) in [
        ("EtltEntity", "moqui.security.UserGroupMember"),
        ("EtltService", "create#moqui.security.UserGroupMember"),
        ("EtltEntity", "moqui.etl.EtlMappingField"),
        ("EtltService", "update#moqui.etl.EtlMappingField"),
        ("EtltService", "update#moqui.database.DatabaseConnection"),
        ("EtltService", "delete#moqui.basic.Enumeration"),
    ] {
        let (queued, errors) = call_as_user("run#EtlMapping", &[("mapping", membership(target_type, target))]);
        assert!(!errors.is_empty(), "{target}: {queued:?}");
        assert!(!queued.contains_key("runId"), "{target}: {queued:?}");
    }
    let mark = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().error_mark()).unwrap();
    let planted = moqui_core::service::call::sync_with(
        "update#moqui.etl.EtlMappingField",
        &[
            ("mappingId".to_string(), Value::string("X")),
            ("fieldSeqId".to_string(), Value::string("000001")),
            ("expression".to_string(), Value::string("1")),
        ]
        .into_iter()
        .collect(),
        &moqui_core::service::call::CallOptions::default(),
    );
    let messages = moqui_core::context::ExecutionContext::with_current(|ec| ec.message().take_errors_since(mark))
        .unwrap_or_default()
        .join("; ");
    assert!(
        planted.is_err() || messages.contains("written only by its own services"),
        "{planted:?} {messages}"
    );

    // another user's run, its error file and value map are not this user's
    let (other, errors) = call_as_user("get#EtlRun", &[("runId", Value::string(other_run))]);
    assert!(errors.contains("no ETL run"), "{errors} {other:?}");
    let (rerun, errors) = call_as_user("rerun#EtlRun", &[("runId", Value::string(other_run))]);
    assert!(errors.contains("no ETL run"), "{errors} {rerun:?}");
    let (taken, errors) = call_as_user(
        "run#EtlMapping",
        &[
            (
                "mapping",
                csv_mapping(
                    "dbresource://moqui/etl/test/good.csv",
                    "EtltEntity",
                    "moqui.basic.Enumeration",
                    vec![field(&[("targetField", "enumId"), ("sourceColumn", "Id")])],
                ),
            ),
            ("wait", Value::Bool(true)),
            ("runId", Value::string(other_run)),
        ],
    );
    assert!(errors.contains("not a queued run of yours"), "{errors} {taken:?}");
    let (map, errors) = call_as_user(
        "store#EtlValueMap",
        &[
            ("valueMapId", Value::string(other_value_map)),
            ("description", Value::string("taken")),
        ],
    );
    assert!(errors.contains("owner"), "{errors} {map:?}");

    // a view: a renamed password alias is left out of the grid and an ETL source, and refused in a lookup; a view
    // over a restricted entity is refused
    {
        let _authz = context.disable_authz();
        for (entity, fields) in [
            (
                "moqui.entity.view.DbViewEntity",
                vec![("dbViewEntityName", "EtlUserPw"), ("packageName", "test.etl")],
            ),
            (
                "moqui.entity.view.DbViewEntityMember",
                vec![
                    ("dbViewEntityName", "EtlUserPw"),
                    ("entityAlias", "UA"),
                    ("entityName", "moqui.security.UserAccount"),
                ],
            ),
            (
                "moqui.entity.view.DbViewEntityAlias",
                vec![
                    ("dbViewEntityName", "EtlUserPw"),
                    ("fieldAlias", "userId"),
                    ("entityAlias", "UA"),
                    ("fieldName", "userId"),
                ],
            ),
            (
                "moqui.entity.view.DbViewEntityAlias",
                vec![
                    ("dbViewEntityName", "EtlUserPw"),
                    ("fieldAlias", "pw"),
                    ("entityAlias", "UA"),
                    ("fieldName", "currentPassword"),
                ],
            ),
        ] {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(name, Value::string(value));
            }
            row.create(facade).unwrap();
        }
    }
    let entity_source = |name: &str| {
        let mut source = IndexMap::new();
        source.insert("type".to_string(), Value::string("entity"));
        source.insert("entityName".to_string(), Value::string(name));
        Value::Map(source)
    };
    let (rows, errors) = grid("get#DataGridRows", &[("source", entity_source("EtlUserPw"))]);
    assert!(errors.is_empty(), "{errors}");
    let Some(Value::List(data)) = rows.get("data") else {
        panic!("{rows:?}")
    };
    assert!(!data.is_empty(), "{rows:?}");
    assert!(
        data.iter()
            .all(|row| matches!(row, Value::Map(row) if !row.contains_key("pw") && row.contains_key("userId"))),
        "{rows:?}"
    );
    let mut conditions = IndexMap::new();
    conditions.insert("pw".to_string(), Value::string("x"));
    let mut conditioned = IndexMap::new();
    conditioned.insert("type".to_string(), Value::string("entity"));
    conditioned.insert("entityName".to_string(), Value::string("EtlUserPw"));
    conditioned.insert("conditions".to_string(), Value::Map(conditions));
    let (_, errors) = grid("count#DataGridRows", &[("source", Value::Map(conditioned))]);
    assert!(errors.contains("takes no condition"), "{errors}");
    let (_, errors) = grid(
        "get#DataGridRows",
        &[("source", entity_source("moqui.security.UserGroupMemberUser"))],
    );
    assert!(errors.contains("not available"), "{errors}");
    let mut view_source = IndexMap::new();
    view_source.insert("sourceTypeEnumId".to_string(), Value::string("EtlsEntity"));
    view_source.insert("sourceEntityName".to_string(), Value::string("EtlUserPw"));
    view_source.insert("targetTypeEnumId".to_string(), Value::string("EtltEntity"));
    view_source.insert("targetName".to_string(), Value::string("moqui.basic.Enumeration"));
    view_source.insert(
        "fields".to_string(),
        Value::List(vec![field(&[("targetField", "enumId"), ("sourceColumn", "userId")])]),
    );
    let (sample, errors) = call_as_user("check#EtlSample", &[("mapping", Value::Map(view_source.clone()))]);
    assert!(errors.is_empty(), "{errors}");
    let Some(Value::List(sampled)) = sample.get("rows") else {
        panic!("{sample:?}")
    };
    assert!(!sampled.is_empty(), "{sample:?}");
    assert!(
        sampled.iter().all(|row| matches!(row, Value::Map(row)
            if matches!(row.get("source"), Some(Value::Map(cells)) if !cells.contains_key("pw")))),
        "{sample:?}"
    );
    view_source.insert(
        "sourceEntityName".to_string(),
        Value::string("moqui.security.UserGroupMemberUser"),
    );
    let (_, errors) = call_as_user("check#EtlSample", &[("mapping", Value::Map(view_source))]);
    assert!(errors.contains("not available"), "{errors}");
    let (_, errors) = call_as_user(
        "check#EtlSample",
        &[(
            "mapping",
            csv_mapping(
                "dbresource://moqui/etl/test/good.csv",
                "EtltEntity",
                "moqui.basic.Enumeration",
                vec![
                    field(&[("targetField", "enumId"), ("sourceColumn", "Id")]),
                    field(&[
                        ("targetField", "description"),
                        ("sourceColumn", "Name"),
                        ("lookupEntityName", "EtlUserPw"),
                        ("lookupMatchField", "userId"),
                        ("lookupResultField", "pw"),
                    ]),
                ],
            ),
        )],
    );
    assert!(errors.contains("pw is not available"), "{errors}");

    // a run on the worker is the user's own with authorization on: a user the Tools app allows, but whom an
    // authorization denies the target entity, loads nothing there
    {
        let _authz = context.disable_authz();
        let rows: Vec<(&str, Vec<(&str, &str)>)> = vec![
            (
                "moqui.security.UserAccount",
                vec![("userId", "ETL_TOOLS_U"), ("username", "etl.tools"), ("disabled", "N")],
            ),
            ("moqui.security.UserGroup", vec![("userGroupId", "ETL_TOOLS_ONLY")]),
            (
                "moqui.security.UserGroupMember",
                vec![("userGroupId", "ETL_TOOLS_ONLY"), ("userId", "ETL_TOOLS_U")],
            ),
            (
                "moqui.security.ArtifactAuthz",
                vec![
                    ("artifactAuthzId", "ETL_TOOLS_ONLY_APP"),
                    ("userGroupId", "ETL_TOOLS_ONLY"),
                    ("artifactGroupId", "TOOLS_APP"),
                    ("authzTypeEnumId", "AUTHZT_ALLOW"),
                    ("authzActionEnumId", "AUTHZA_ALL"),
                ],
            ),
            (
                "moqui.security.ArtifactGroup",
                vec![("artifactGroupId", "ETL_DENY_ENUM")],
            ),
            (
                "moqui.security.ArtifactGroupMember",
                vec![
                    ("artifactGroupId", "ETL_DENY_ENUM"),
                    ("artifactName", "moqui.basic.Enumeration"),
                    ("artifactTypeEnumId", "AT_ENTITY"),
                    ("inheritAuthz", "N"),
                ],
            ),
            (
                "moqui.security.ArtifactAuthz",
                vec![
                    ("artifactAuthzId", "ETL_DENY_ENUM_T"),
                    ("userGroupId", "ETL_TOOLS_ONLY"),
                    ("artifactGroupId", "ETL_DENY_ENUM"),
                    ("authzTypeEnumId", "AUTHZT_DENY"),
                    ("authzActionEnumId", "AUTHZA_ALL"),
                ],
            ),
        ];
        for (entity, fields) in rows {
            let mut row = facade.make_value(entity).unwrap();
            for (name, value) in fields {
                row.set(name, Value::string(value));
            }
            if entity.ends_with("UserGroupMember") {
                row.set("fromDate", Value::Timestamp(jiff::Timestamp::UNIX_EPOCH));
            }
            row.create(facade).unwrap();
        }
    }
    write(
        "dbresource://moqui/etl/test/denied.csv",
        "Id,Type,Name\nEtlHost7,EtlHostType,Seven\n",
    );
    moqui_core::user::internal_login_user("etl.tools").unwrap();
    let (queued, errors) = call_as_user(
        "run#EtlMapping",
        &[(
            "mapping",
            csv_mapping(
                "dbresource://moqui/etl/test/denied.csv",
                "EtltEntity",
                "moqui.basic.Enumeration",
                vec![
                    field(&[("targetField", "enumId"), ("sourceColumn", "Id")]),
                    field(&[("targetField", "enumTypeId"), ("sourceColumn", "Type")]),
                ],
            ),
        )],
    );
    assert!(errors.is_empty(), "{errors}");
    assert_eq!(text(&queued, "statusId"), "EtlrPending", "{queued:?}");
    let run_id = text(&queued, "runId");
    let started = Instant::now();
    let run = loop {
        let (detail, _) = call_as_user("get#EtlRun", &[("runId", Value::string(&run_id))]);
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
    let (detail, _) = call_as_user("get#EtlRun", &[("runId", Value::string(&run_id))]);
    assert_eq!(to_plain_string(&run["statusId"]), "EtlrFailed", "{detail:?}");
    assert_eq!(to_plain_string(&run["loadCount"]), "0", "{detail:?}");
    // refused by the authorization, which the worker's call had disabled
    assert!(
        matches!(detail.get("errors"), Some(Value::List(rows)) if rows.iter().any(|row| matches!(row,
            Value::Map(row) if to_plain_string(&row["message"]).contains("not authorized")))),
        "{detail:?}"
    );
    let _authz = context.disable_authz();
    assert!(
        facade
            .find("moqui.basic.Enumeration")
            .unwrap()
            .eq("enumId", Value::string("EtlHost7"))
            .one()
            .unwrap()
            .is_none(),
        "the run wrote what its user may not: {detail:?}"
    );
}

/// A ServiceJob row calling run#EtlMapping with `parameters`, with `job_user` as its ServiceJobUser.
fn etl_job(facade: &moqui_core::entity::EntityFacade, job_name: &str, job_user: &str, parameters: &[(&str, &str)]) {
    let mut job = facade.make_value("moqui.service.job.ServiceJob").unwrap();
    job.set("jobName", Value::string(job_name));
    job.set("serviceName", Value::string(format!("{SERVICES}.run#EtlMapping")));
    job.set("paused", Value::string("Y"));
    job.create(facade).unwrap();
    for (name, value) in parameters {
        let mut parameter = facade.make_value("moqui.service.job.ServiceJobParameter").unwrap();
        parameter.set("jobName", Value::string(job_name));
        parameter.set("parameterName", Value::string(*name));
        parameter.set("parameterValue", Value::string(*value));
        parameter.create(facade).unwrap();
    }
    job_user_of(facade, job_name, job_user);
}

/// A ServiceJobUser row: `user_id` is a user of the job `job_name`.
fn job_user_of(facade: &moqui_core::entity::EntityFacade, job_name: &str, user_id: &str) {
    let mut job_user = facade.make_value("moqui.service.job.ServiceJobUser").unwrap();
    job_user.set("jobName", Value::string(job_name));
    job_user.set("userId", Value::string(user_id));
    job_user.set("receiveNotifications", Value::string("N"));
    job_user.create(facade).unwrap();
}

/// A saved mapping of good.csv into Enumeration owned by `owner`, written as the framework.
fn owned_mapping(facade: &moqui_core::entity::EntityFacade, mapping_id: &str, owner: &str) {
    let mut mapping = facade.make_value("moqui.etl.EtlMapping").unwrap();
    mapping.set("mappingId", Value::string(mapping_id));
    mapping.set("ownerUserId", Value::string(owner));
    mapping.set("sourceTypeEnumId", Value::string("EtlsFile"));
    mapping.set("sourceLocation", Value::string("dbresource://moqui/etl/test/good.csv"));
    mapping.set("targetTypeEnumId", Value::string("EtltEntity"));
    mapping.set("targetName", Value::string("moqui.basic.Enumeration"));
    mapping.create(facade).unwrap();
    for (seq, target, column) in [
        ("000001", "enumId", "Id"),
        ("000002", "enumTypeId", "Type"),
        ("000003", "description", "Name"),
    ] {
        let mut field = facade.make_value("moqui.etl.EtlMappingField").unwrap();
        field.set("mappingId", Value::string(mapping_id));
        field.set("fieldSeqId", Value::string(seq));
        field.set("targetField", Value::string(target));
        field.set("sourceColumn", Value::string(column));
        field.create(facade).unwrap();
    }
}

/// The errors of the job run `job_run` once it has them.
fn job_run_errors(facade: &moqui_core::entity::EntityFacade, job_run: &str) -> String {
    let started = Instant::now();
    loop {
        let found = facade
            .find("moqui.service.job.ServiceJobRun")
            .unwrap()
            .eq("jobRunId", Value::string(job_run))
            .one()
            .unwrap();
        if let Some(errors) = found
            .and_then(|row| row.get("errors").map(to_plain_string))
            .filter(|errors| !errors.is_empty())
        {
            return errors;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "job run {job_run} did not end with an error"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// No EtlRun names the job run `job_run`.
fn no_etl_run(facade: &moqui_core::entity::EntityFacade, job_run: &str) -> bool {
    facade
        .find("moqui.etl.EtlRun")
        .unwrap()
        .eq("jobRunId", Value::string(job_run))
        .one()
        .unwrap()
        .is_none()
}

/// Run the job `job_name` with no user signed in, as a cron run is; its run id.
fn run_without_user(factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>, job_name: &str) -> String {
    let context = factory.new_context();
    let _entered = context.enter();
    assert!(moqui_core::user::current_user_id().unwrap().is_none());
    // as the scheduler: no user, and authorization off for the job's own rows
    let _authz = context.disable_authz();
    moqui_core::service::run_job_now(job_name, &BTreeMap::new()).unwrap()
}

/// A scheduled run with no user signed in runs as the mapping's owner, with the owner's checks: a saved mapping loads
/// from its own source (the job's `location` is ignored) and its EtlRun names the owner and the job's run; one with an
/// expression is refused for an owner without GROOVY_SHELL_WEB; a job the owner is not a user of is refused; and two
/// runs in one job's context each run as their own mapping's owner, with no user signed in after either.
fn scheduled_runs_run_as_the_owner(
    factory: &std::sync::Arc<moqui_core::context::ExecutionContextFactory>,
    facade: &moqui_core::entity::EntityFacade,
    mapping_id: &str,
) {
    let _authz =
        moqui_core::context::ExecutionContext::with_current(moqui_core::context::ExecutionContext::disable_authz);
    etl_job(
        facade,
        "EtlHostScheduled",
        "EX_JOHN_DOE",
        &[
            ("mappingId", mapping_id),
            // the job's writer may not pick the owner's source
            ("location", "dbresource://moqui/etl/test/bad.csv"),
        ],
    );
    // a mapping of the same owner with an expression, written as the framework (its own service would refuse it)
    let mut scripted = facade.make_value("moqui.etl.EtlMapping").unwrap();
    scripted.set("mappingId", Value::string("EtlHostScripted"));
    scripted.set("ownerUserId", Value::string("EX_JOHN_DOE"));
    scripted.set("sourceTypeEnumId", Value::string("EtlsFile"));
    scripted.set("sourceLocation", Value::string("dbresource://moqui/etl/test/good.csv"));
    scripted.set("targetTypeEnumId", Value::string("EtltEntity"));
    scripted.set("targetName", Value::string("moqui.basic.Enumeration"));
    scripted.create(facade).unwrap();
    for (seq, target, column, expression) in [
        ("000001", "enumId", "Id", None),
        ("000002", "description", "Name", Some("'planted ' + value")),
    ] {
        let mut field = facade.make_value("moqui.etl.EtlMappingField").unwrap();
        field.set("mappingId", Value::string("EtlHostScripted"));
        field.set("fieldSeqId", Value::string(seq));
        field.set("targetField", Value::string(target));
        field.set("sourceColumn", Value::string(column));
        if let Some(expression) = expression {
            field.set("expression", Value::string(expression));
        }
        field.create(facade).unwrap();
    }
    etl_job(
        facade,
        "EtlHostScriptedJob",
        "EX_JOHN_DOE",
        &[("mappingId", "EtlHostScripted")],
    );
    // the job's user is someone else: whoever wrote it may not run john.doe's mapping as john.doe
    etl_job(
        facade,
        "EtlHostOtherUserJob",
        "ETL_SECOND_U",
        &[("mappingId", mapping_id)],
    );

    let job_run = run_without_user(factory, "EtlHostScheduled");
    let started = Instant::now();
    let run = loop {
        let found = facade
            .find("moqui.etl.EtlRun")
            .unwrap()
            .eq("jobRunId", Value::string(&job_run))
            .one()
            .unwrap();
        if let Some(run) = found.filter(|run| {
            matches!(
                run.get("statusId").map(to_plain_string).as_deref(),
                Some("EtlrDone" | "EtlrFailed")
            )
        }) {
            break run;
        }
        assert!(
            started.elapsed() < Duration::from_secs(30),
            "the scheduled run did not end"
        );
        std::thread::sleep(Duration::from_millis(50));
    };
    assert_eq!(
        run.get("statusId").map(to_plain_string).as_deref(),
        Some("EtlrDone"),
        "{run:?}"
    );
    assert_eq!(
        run.get("userId").map(to_plain_string).as_deref(),
        Some("EX_JOHN_DOE"),
        "{run:?}"
    );
    assert_eq!(
        run.get("loadCount").map(to_plain_string).as_deref(),
        Some("2"),
        "{run:?}"
    );
    assert_eq!(
        run.get("errorCount").map(to_plain_string).as_deref(),
        Some("0"),
        "{run:?}"
    );

    let job_run = run_without_user(factory, "EtlHostScriptedJob");
    let errors = job_run_errors(facade, &job_run);
    assert!(errors.contains("GROOVY_SHELL_WEB"), "{errors}");
    assert!(no_etl_run(facade, &job_run));

    let job_run = run_without_user(factory, "EtlHostOtherUserJob");
    let errors = job_run_errors(facade, &job_run);
    assert!(
        errors.contains("is not a user of ServiceJob EtlHostOtherUserJob"),
        "{errors}"
    );
    assert!(no_etl_run(facade, &job_run));

    // a second owner, an ADMIN as john.doe is, and a job both owners are users of
    let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
    user.set("userId", Value::string("ETL_SECOND_U"));
    user.set("username", Value::string("etl.second"));
    user.set("disabled", Value::string("N"));
    user.create(facade).unwrap();
    let mut member = facade.make_value("moqui.security.UserGroupMember").unwrap();
    member.set("userGroupId", Value::string("ADMIN"));
    member.set("userId", Value::string("ETL_SECOND_U"));
    member.set("fromDate", Value::Timestamp(jiff::Timestamp::UNIX_EPOCH));
    member.create(facade).unwrap();
    owned_mapping(facade, "EtlHostSecond", "ETL_SECOND_U");
    job_user_of(facade, "EtlHostScheduled", "ETL_SECOND_U");
    let context = factory.new_context();
    let _entered = context.enter();
    let _job_authz = context.disable_authz();
    moqui_core::service::in_job_run("EtlHostPairRun", "EtlHostScheduled", || {
        for (mapping, owner) in [(mapping_id, "EX_JOHN_DOE"), ("EtlHostSecond", "ETL_SECOND_U")] {
            let out = call("run#EtlMapping", &[("mappingId", Value::string(mapping))]);
            assert_eq!(text(&out, "statusId"), "EtlrDone", "{mapping}: {out:?}");
            assert!(
                moqui_core::user::current_user_id().unwrap().is_none(),
                "the owner of {mapping} is still signed in"
            );
            let run = facade
                .find("moqui.etl.EtlRun")
                .unwrap()
                .eq("runId", Value::string(text(&out, "runId")))
                .one()
                .unwrap()
                .expect("the run's EtlRun");
            assert_eq!(
                run.get("userId").map(to_plain_string).as_deref(),
                Some(owner),
                "{run:?}"
            );
            assert_eq!(
                run.get("jobRunId").map(to_plain_string).as_deref(),
                Some("EtlHostPairRun"),
                "{run:?}"
            );
        }
    });
}
