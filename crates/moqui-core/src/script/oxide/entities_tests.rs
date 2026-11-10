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

use std::sync::{Arc, Mutex};

use moqui_lib::object::{HostObject, Value};

use crate::context::Error;
use crate::script::host::entities::{EntityCell, ScriptList};
use crate::script::oxide::{eval_value, Outcome};
use crate::script::text::hash_text;
use crate::script::{clear_context_stack, push_context_frame, set_context_name};

fn value(source: &str) -> Result<Value, Error> {
    match eval_value(source, hash_text(source), false) {
        Ok(value) => Ok(value),
        Err(Outcome::Error(err)) => Err(err),
        Err(Outcome::Fallback(reason)) => Err(Error::new(format!("{source}: fallback {reason}"))),
    }
}

fn row(id: &str) -> Arc<EntityCell> {
    let mut fields = std::collections::BTreeMap::new();
    fields.insert(Arc::from("enumId"), Value::string(id));
    Arc::new(Mutex::new(crate::entity::EntityValue::from_read_fields(
        Arc::from("moqui.basic.Enumeration"),
        fields,
    )))
}

/// An entity list on oxide answers as `HostList` does on QuickJS: `length` and its rows by index (the rows themselves),
/// its methods and the shim's iteration and array methods from the table's prototype, and it is read-only; it converts
/// back to the same rows. It has no cursor: a find's `iterator()` is the cursor, as Java's `EntityList` is a list.
#[test]
fn an_entity_list_indexes_and_iterates_and_has_no_cursor() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let cells = vec![row("E1"), row("E2"), row("E3")];
    let list = Arc::new(ScriptList::of_cells(cells.clone()));
    set_context_name("rows", Value::Object(HostObject(list.clone() as _)))?;
    for (script, expected) in [
        ("rows.length", "3"),
        ("rows[1].enumId", "E2"),
        ("String(rows[3])", "undefined"),
        ("rows.get(2).enumId + rows.size() + rows.isEmpty()", "E33false"),
        ("[...rows].map((row) => row.enumId).join()", "E1,E2,E3"),
        ("rows.map((row) => row.enumId).join('')", "E1E2E3"),
        ("Object.keys(rows).join()", "0,1,2"),
        (
            "'length' in rows && 1 in rows && !(5 in rows) && !('next' in rows) && !('hasNext' in rows)",
            "true",
        ),
        // Groovy's first() and last(), which do not move anything
        (
            "rows.first().enumId + rows.last().enumId + rows.first().enumId",
            "E1E3E1",
        ),
        ("rows.getPartialList(1, 5).length", "2"),
        ("rows.filter((row) => row.enumId > 'E1').length", "2"),
        (
            "(() => { 'use strict'; try { rows[0] = 1; return 'set'; } catch (e) { return e.name; } })()",
            "TypeError",
        ),
        // a new object of the same row each read, as on QuickJS
        ("rows[0] !== rows[0] && rows.get(0) !== rows[0]", "true"),
        ("rows.get.length + '/' + rows.getPartialList.length", "1/2"),
    ] {
        let got = value(&format!("String({script})"))?;
        assert_eq!(moqui_lib::object::to_plain_string(&got), expected, "{script}");
    }
    let Value::Object(back) = value("rows")? else {
        panic!("not an object");
    };
    assert!(back
        .downcast::<ScriptList>()
        .is_some_and(|back| Arc::ptr_eq(&back, &list)));
    Ok(())
}

/// `ec.entity` on oxide against the in-memory database: a find's chain, its list and one, a builder that chains in
/// place and runs again as Java's, a cached one, a relationship read and a value's methods.
#[test]
fn ec_entity_finds_and_values_run_on_oxide() -> Result<(), Error> {
    clear_context_stack();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let _entered = context.enter();
    crate::script::run_script(
        r#"const type = ec.entity.makeValue("moqui.basic.EnumerationType");
            type.enumTypeId = "T1"; type.description = "Type 1"; type.create();
            for (const id of ["E2", "E1"]) {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = id; value.enumTypeId = "T1"; value.description = "d" + id; value.create();
            }"#,
    )?;
    push_context_frame();
    for (script, expected) in [
        (
            "ec.entity.find('moqui.basic.Enumeration').orderBy('enumId').list()[0].enumId",
            "E1",
        ),
        (
            "ec.entity.find('moqui.basic.Enumeration').condition('enumId', 'E2').one().description",
            "dE2",
        ),
        (
            "ec.entity.find('moqui.basic.Enumeration').condition({ enumTypeId: 'T1', other: 1 }).count()",
            "2",
        ),
        (
            "ec.entity.find('moqui.basic.Enumeration').limit('1').list().length",
            "1",
        ),
        (
            "ec.entity.find('moqui.basic.Enumeration').condition('enumId', 'E1').one().EnumerationType.description",
            "Type 1",
        ),
        (
            "ec.entity.find('moqui.basic.Enumeration').condition('enumId', 'E1').one().getEntityNamePretty()",
            "Enumeration",
        ),
        (
            "ec.entity.find.length + '/' + ec.entity.sequencedIdPrimary.length",
            "1/1",
        ),
    ] {
        let got = value(&format!("String({script})"))?;
        assert_eq!(moqui_lib::object::to_plain_string(&got), expected, "{script}");
    }
    // As Java's builder: a chained call changes the find and returns it, and a find runs again.
    let chained = "(() => { const find = ec.entity.find('moqui.basic.Enumeration'); \
        const same = find.orderBy('-enumId') === find; find.condition('enumTypeId', 'T1'); \
        return [same, find.list()[0].enumId, find.count(), find.list().length, find.getQueryTextList().length > 0, \
            find.limit(1).list().length].join(); })()";
    assert_eq!(value(chained)?, Value::string("true,E2,2,2,true,1"));
    let again =
        "(() => { const find = ec.entity.find('moqui.basic.Enumeration').condition('enumId', 'E1').useCache(true); \
        return [find.one().description, find.one().description, find.condition('enumId', 'E2').one().description, \
            find.useCache(false).one().description].join(); })()";
    assert_eq!(value(again)?, Value::string("dE1,dE1,dE2,dE2"));
    Ok(())
}

/// An uploaded file and its stream have the table's members on oxide, as on QuickJS, and pass through a script as
/// themselves.
#[test]
fn an_upload_and_its_stream_have_their_members_on_both_engines() -> Result<(), Error> {
    clear_context_stack();
    push_context_frame();
    let upload = Arc::new(crate::web::multipart::UploadedFile::in_memory(
        "contentFile",
        "notes.txt",
        "text/plain",
        b"hello".to_vec(),
    ));
    set_context_name("file", Value::Object(HostObject(upload.clone() as _)))?;
    let script = "[file.name, file.fileName, file.fieldName, file.contentType, file.size, file.getSize(), \
        file.isInMemory(), file.getString(), String(file), file.getInputStream().getText('UTF-8'), \
        file.getInputStream().read(1, 2).length, file.getInputStream().size, file.getInputStream().available(), \
        file.getString.length, file.getInputStream().read.length].join()";
    let expected = "notes.txt,notes.txt,contentFile,text/plain,5,5,true,hello,notes.txt,hello,2,5,5,0,2";
    let on_oxide = value(script)?;
    assert_eq!(moqui_lib::object::to_plain_string(&on_oxide), expected, "oxide");
    let on_quickjs = crate::script::runtime::eval_value(script)?;
    assert_eq!(moqui_lib::object::to_plain_string(&on_quickjs), expected, "QuickJS");
    let Value::Object(back) = value("file")? else {
        panic!("not an object");
    };
    assert!(back
        .downcast::<crate::web::multipart::UploadedFile>()
        .is_some_and(|back| Arc::ptr_eq(&back, &upload)));
    let Value::Object(stream) = value("file.getInputStream()")? else {
        panic!("not an object");
    };
    assert!(stream.downcast::<crate::script::host::StreamBytes>().is_some());
    Ok(())
}

/// `ec.entity.conditionFactory` and its conditions answer the same on both engines, against the in-memory database.
#[test]
fn conditions_answer_the_same_on_both_engines() -> Result<(), Error> {
    clear_context_stack();
    let context = crate::context::ExecutionContextFactory::test_context();
    context.factory().set_entity(crate::testing::open());
    let _entered = context.enter();
    crate::script::run_script(
        r#"for (const id of ["E2", "E1", "X3"]) {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = id; value.description = "First " + id; value.create();
            }"#,
    )?;
    push_context_frame();
    let script = "(() => { const cf = ec.entity.conditionFactory; const count = (condition) => \
        ec.entity.find('moqui.basic.Enumeration').condition(condition).count(); \
        return [cf.makeCondition.length, cf.makeConditionWhere.length, cf.makeConditionDate.length, \
            cf.makeActionConditionDirect.length, count(cf.makeCondition('enumId', 'like', 'E%')), \
            count(cf.makeCondition('description', 'like', 'FIRST%').ignoreCase()), \
            count(cf.makeCondition([cf.makeCondition('enumId', 'equals', 'E1'), \
                cf.makeCondition('enumId', 'equals', 'X3')], 'or')), \
            count(cf.makeCondition({ enumId: 'E2' })), cf.makeConditionWhere('') === null, \
            typeof cf.makeConditionWhere('ENUM_ID = 1'), \
            cf.makeActionConditionDirect('enumId', 'equals', null, 'E1', null, false, false, false, true) === null, \
            count(cf.makeActionConditionDirect('enumId', 'equals', null, 'E1', null, false, false, false, false)), \
            (() => { try { cf.makeCondition(1); } catch (e) { return e.message; } })()].join(); })()";
    let expected = "1,0,3,0,2,3,2,1,true,object,true,1,makeCondition needs a field, a list, or a map";
    let on_oxide = value(script)?;
    assert_eq!(moqui_lib::object::to_plain_string(&on_oxide), expected, "oxide");
    let on_quickjs = crate::script::runtime::eval_value(script)?;
    assert_eq!(moqui_lib::object::to_plain_string(&on_quickjs), expected, "QuickJS");
    Ok(())
}

/// `ec.getTool`, `ec.factory.getTool` and `ec.factory.getToolFactory` answer the same on both engines: their lengths,
/// null for a factory that is not there, and the error for a tool that is not.
#[test]
fn the_tool_functions_answer_the_same_on_both_engines() -> Result<(), Error> {
    clear_context_stack();
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    push_context_frame();
    let script = "[ec.getTool.length, ec.factory.getTool.length, ec.factory.getToolFactory.length, \
        ec.factory.getToolFactory('NoSuchTool') === null, \
        (() => { try { ec.getTool('NoSuchTool', 1, 'a'); } catch (e) { return e.message; } })()].join()";
    let expected = "1,1,1,true,no tool factory NoSuchTool";
    let on_oxide = value(script)?;
    assert_eq!(moqui_lib::object::to_plain_string(&on_oxide), expected, "oxide");
    let on_quickjs = crate::script::runtime::eval_value(script)?;
    assert_eq!(moqui_lib::object::to_plain_string(&on_quickjs), expected, "QuickJS");
    Ok(())
}

/// What the static check allows (`host::path_allowed`) is bound on both engines: every path of the table and every
/// member of its class is there, with the same type, on oxide and on QuickJS.
#[test]
fn every_allowed_path_is_bound_on_both_engines() -> Result<(), Error> {
    use crate::script::host::{all_paths, path_allowed, Bound};
    clear_context_stack();
    let context = crate::context::ExecutionContextFactory::test_context();
    let _entered = context.enter();
    push_context_frame();
    let mut names = Vec::new();
    for entry in all_paths() {
        match entry.bound {
            Bound::Shim(_) => continue,
            Bound::Function(_) => names.push(entry.path.to_string()),
            Bound::Object(class) => {
                names.push(entry.path.to_string());
                for method in class.methods {
                    names.push(format!("{}.{}", entry.path, method.name));
                }
                for member in class.shim_members {
                    names.push(format!("{}.{member}", entry.path));
                }
            }
        }
    }
    for name in &names {
        assert!(path_allowed(name), "{name} is bound but not allowed");
    }
    let script = format!(
        "[{}].join()",
        names
            .iter()
            .map(|name| format!("typeof {name}"))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let on_oxide = moqui_lib::object::to_plain_string(&value(&script)?);
    let on_quickjs = moqui_lib::object::to_plain_string(&crate::script::runtime::eval_value(&script)?);
    assert_eq!(on_oxide, on_quickjs);
    for (name, kind) in names.iter().zip(on_oxide.split(',')) {
        assert!(kind == "function" || kind == "object", "{name} is {kind}");
    }
    Ok(())
}
