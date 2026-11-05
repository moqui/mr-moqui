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

    authorization_is_on(&context, &facade);
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
fn authorization_is_on(context: &moqui_core::context::ExecutionContext, facade: &moqui_core::entity::EntityFacade) {
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

    // group membership: as an entity target it is refused at once; as a service target the run on the worker is the
    // user's own, and the security rules refuse ADMIN_ADV to a user not in it
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
    let (queued, errors) = call_as_user(
        "run#EtlMapping",
        &[("mapping", membership("EtltEntity", "moqui.security.UserGroupMember"))],
    );
    assert!(errors.contains("not available"), "{errors} {queued:?}");
    assert!(!queued.contains_key("runId"));
    let (queued, errors) = call_as_user(
        "run#EtlMapping",
        &[(
            "mapping",
            membership("EtltService", "create#moqui.security.UserGroupMember"),
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
    assert_eq!(to_plain_string(&run["statusId"]), "EtlrFailed", "{run:?}");
    assert_eq!(to_plain_string(&run["loadCount"]), "0", "{run:?}");
    // refused by the security rule, which runs only with authorization on
    let (detail, _) = call_as_user("get#EtlRun", &[("runId", Value::string(&run_id))]);
    assert!(
        matches!(detail.get("errors"), Some(Value::List(rows)) if rows.iter().any(|row| matches!(row,
            Value::Map(row) if to_plain_string(&row["message"]).contains("change membership of group ADMIN_ADV")))),
        "{detail:?}"
    );
    let _authz = context.disable_authz();
    let added = facade
        .find("moqui.security.UserGroupMember")
        .unwrap()
        .eq("userGroupId", Value::string("ADMIN_ADV"))
        .eq("userId", Value::string("ETL_ADMIN_U"))
        .list()
        .unwrap();
    assert!(added.is_empty(), "the run added the user to ADMIN_ADV: {run:?}");
}
