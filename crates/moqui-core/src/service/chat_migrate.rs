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

//! `org.moqui.chat.ChatServices.migrate#ChatActors` (plans/Comms-Framework.md §4.4 "The C1 migration"): a database from
//! before chat actors moves to them. Run once, in a maintenance window with no server on the old code (docs/Run-Deploy.md
//! "Chat actors"); running it again changes nothing.
//!
//! 1. The table check adds the new columns and tables (additive, as at every start).
//! 2. Backfill: one local `ChatActor` for each user the chat rows name (members, senders, mentions, poll votes, the
//!    rooms' last message, call participants), whether or not the account still exists (a deleted account's history
//!    keeps its actor). Then each row gets its user's actor, one SQL statement per table (a local actor's id is its
//!    userId): a table that fails is logged and counted, and the others go on.
//! 3. The tables whose key changed (`ChatRoomMember`, `ChatEventMention`, `ChatPollVote`) are rebuilt with the new key,
//!    rows copied ([`crate::entity::EntityFacade::rekey_table`]; the old rows stay in `<table>_PRE_C1` for a rollback).
//!    A table with a row that has no actor is not rebuilt (strict: no row is dropped); one already on the new key is
//!    left alone.
//! 4. The references are checked: every actor a row names exists and is the actor of the row's user.
//!
//! `get#PreC1Backups` and `drop#PreC1Backups` (the ChatBridgeAdmin permission; System, Chat Bridges) list the
//! `<table>_PRE_C1` copies and drop the ones whose table is on its new key with at least the copy's rows.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use moqui_db::sql::{physical_column_name, physical_name, quote_ident, Condition};
use moqui_lib::error::Error as LibError;
use moqui_lib::object::{is_empty, to_plain_string, Value};

use crate::context::ExecutionContext;
use crate::entity::EntityFacade;

pub(in crate::service) fn register(
    map: &mut BTreeMap<&'static str, crate::service::rust_services::RustServiceFn>,
) -> Result<(), String> {
    rust_service!(map, migrate_chat_actors)?;
    rust_service!(map, get_pre_c1_backups)?;
    rust_service!(map, drop_pre_c1_backups)?;
    Ok(())
}

const MEMBER: &str = "moqui.chat.ChatRoomMember";
const EVENT: &str = "moqui.chat.ChatEvent";
const MENTION: &str = "moqui.chat.ChatEventMention";
const VOTE: &str = "moqui.chat.ChatPollVote";
const ROOM: &str = "moqui.chat.ChatRoom";
const PARTICIPANT: &str = "moqui.call.CallParticipant";
const ACTOR: &str = "moqui.chat.ChatActor";

/// Each table with a user column and the actor column beside it.
const FILLS: [(&str, &str, &str); 6] = [
    (MEMBER, "userId", "chatActorId"),
    (EVENT, "senderUserId", "senderActorId"),
    (MENTION, "userId", "chatActorId"),
    (VOTE, "userId", "chatActorId"),
    (ROOM, "lastMessageUserId", "lastMessageActorId"),
    (PARTICIPANT, "userId", "chatActorId"),
];

/// The tables whose key changed, and the table their old rows are kept in.
const REKEYED: [(&str, &str); 3] = [
    (MEMBER, "CHAT_ROOM_MEMBER_PRE_C1"),
    (MENTION, "CHAT_EVENT_MENTION_PRE_C1"),
    (VOTE, "CHAT_POLL_VOTE_PRE_C1"),
];

fn text(value: &Value) -> Option<String> {
    (!is_empty(value))
        .then(|| to_plain_string(value).trim().to_string())
        .filter(|text| !text.is_empty())
}

/// The quoted table of an entity and a function from its field names to their quoted columns.
fn quoted<'a>(
    definition: &'a moqui_db::model::EntityDefinition,
    dialect: &'a moqui_db::dialect::Dialect,
) -> (String, impl Fn(&str) -> Result<String, LibError> + 'a) {
    let table = quote_ident(dialect, &physical_name(dialect, &definition.table_name));
    let column = move |name: &str| -> Result<String, LibError> {
        let field = definition
            .fields
            .iter()
            .find(|field| field.name == name)
            .ok_or_else(|| LibError::new(format!("{} has no field {name}", definition.full_name)))?;
        Ok(quote_ident(dialect, &physical_column_name(dialect, &field.column_name)))
    };
    (table, column)
}

/// `UPDATE <table> SET <actor> = <user> WHERE <user> IS NOT NULL AND <actor> IS NULL` and the user's actor exists (a
/// user whose actor could not be made keeps no actor, so its table is not rebuilt), on a reserve connection.
fn fill(facade: &EntityFacade, entity: &str, user_field: &str, actor_field: &str) -> Result<i64, LibError> {
    let (actor_table, actor_id, actor_user) = facade.with_group(ACTOR, |definition, dialect, _, _| {
        let (table, column) = quoted(definition, dialect);
        Ok((table, column("chatActorId")?, column("userId")?))
    })?;
    let (sql, tx) = facade.with_group(entity, |definition, dialect, _, tx| {
        let (table, column) = quoted(definition, dialect);
        let (user, actor) = (column(user_field)?, column(actor_field)?);
        Ok((
            format!(
                "UPDATE {table} SET {actor} = {user} WHERE {user} IS NOT NULL AND {actor} IS NULL AND EXISTS \
                 (SELECT 1 FROM {actor_table} A WHERE A.{actor_id} = {table}.{user} AND A.{actor_user} = {table}.{user})"
            ),
            tx.clone(),
        ))
    })?;
    let count = crate::entity::exec::with_reserve_tx(&tx, |conn| conn.execute(&sql, &[]))?;
    Ok(i64::try_from(count).unwrap_or(i64::MAX))
}

/// `migrate#ChatActors`. Out: actorCount (the users with an actor), filledCount (rows given their actor), failedCount
/// (users or tables that failed, each logged), rebuiltList (the tables rebuilt now), problemList (what failed or does not
/// hold, by name).
fn migrate_chat_actors(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut problems: Vec<String> = Vec::new();
    let mut failed = 0_i64;
    let entities: Vec<&str> = [ACTOR]
        .into_iter()
        .chain(FILLS.iter().map(|(entity, _, _)| *entity))
        .collect();
    if let Err(error) = facade.check_tables_ddl(&entities) {
        tracing::error!("chat actors: the table check failed: {error}");
        problems.push(format!("table check: {error}"));
        failed += 1;
    }

    // 1. the users the chat rows name
    let mut users: BTreeSet<String> = BTreeSet::new();
    for (entity, user_field, _) in FILLS {
        match facade
            .find(entity)
            .and_then(|find| find.select_field(user_field).distinct(true).disable_authz().list())
        {
            Ok(rows) => users.extend(
                rows.values()
                    .iter()
                    .filter_map(|row| row.get(user_field).and_then(text)),
            ),
            Err(error) => {
                tracing::error!("chat actors: could not read the users of {entity}: {error}");
                problems.push(format!("{entity}: {error}"));
                failed += 1;
            }
        }
    }
    let users: Vec<String> = users.into_iter().collect();
    let mut actors: HashMap<String, String> = HashMap::new();
    for chunk in users.chunks(200) {
        match super::chat_bridge::local_actors(facade, chunk) {
            Ok(map) => actors.extend(map),
            Err(error) => {
                tracing::error!(
                    "chat actors: could not make the actors of {} users: {error}",
                    chunk.len()
                );
                problems.push(format!("actors: {error}"));
                failed += 1;
            }
        }
    }

    // 2. each row gets its user's actor (a local actor's id is its userId): one statement per table, in SQL, as the rows
    // of a table on its old key cannot be updated by their new key
    let mut filled = 0_i64;
    for (entity, user_field, actor_field) in FILLS {
        match fill(facade, entity, user_field, actor_field) {
            Ok(count) => filled += count,
            Err(error) => {
                tracing::error!("chat actors: the rows of {entity} did not get their actors: {error}");
                problems.push(format!("{entity}: {error}"));
                failed += 1;
            }
        }
    }

    // 3. the tables whose key changed, rebuilt with the new key
    let mut rebuilt: Vec<Value> = Vec::new();
    for (entity, backup) in REKEYED {
        let key = match facade.table_primary_key(entity) {
            Ok(key) => key,
            Err(error) => {
                tracing::error!("chat actors: could not read the key of {entity}: {error}");
                problems.push(format!("{entity} key: {error}"));
                failed += 1;
                continue;
            }
        };
        if key.is_empty() {
            continue;
        }
        if key.iter().any(|column| column.replace('_', "") == "chatactorid") {
            // on the new key; a rebuild cut off after its table was made and before its rows came back (on MySQL, whose
            // table changes are not transactional) leaves it empty beside a backup with rows: said, not repaired, as a
            // table emptied since would look the same
            let rows = facade.find(entity).and_then(|find| find.disable_authz().count());
            if let (Ok(0), Ok(Some(kept))) = (rows, facade.backup_table_rows(entity, backup)) {
                if kept > 0 {
                    tracing::error!(
                        "chat actors: {entity} is empty and {backup} has {kept} rows: if its rebuild was cut off, copy them back"
                    );
                    problems.push(format!("{entity}: empty, {backup} has {kept} rows"));
                    failed += 1;
                }
            }
            continue;
        }
        let missing = facade.find(entity).and_then(|find| {
            find.condition(Condition::is_null("chatActorId"))
                .disable_authz()
                .count()
        });
        match missing {
            Ok(0) => {}
            Ok(count) => {
                tracing::error!("chat actors: {entity} is not rebuilt: {count} rows have no actor");
                problems.push(format!("{entity}: {count} rows have no actor"));
                failed += 1;
                continue;
            }
            Err(error) => {
                tracing::error!("chat actors: could not count the rows of {entity} without an actor: {error}");
                problems.push(format!("{entity}: {error}"));
                failed += 1;
                continue;
            }
        }
        match facade.rekey_table(entity, backup) {
            Ok((before, after)) => {
                tracing::info!("chat actors: {entity} rebuilt with its new key, {after} of {before} rows; the old rows are in {backup}");
                rebuilt.push(Value::string(entity));
            }
            Err(error) => {
                tracing::error!("chat actors: {entity} was not rebuilt: {error}");
                problems.push(format!("{entity} rebuild: {error}"));
                failed += 1;
            }
        }
    }

    // 4. every actor a row names exists and is its user's
    let known: HashMap<String, Option<String>> = match facade.find(ACTOR).and_then(|find| find.disable_authz().list()) {
        Ok(rows) => rows
            .values()
            .iter()
            .filter_map(|row| Some((row.get("chatActorId").and_then(text)?, row.get("userId").and_then(text))))
            .collect(),
        Err(error) => {
            problems.push(format!("actors: {error}"));
            HashMap::new()
        }
    };
    for (entity, user_field, actor_field) in FILLS {
        let rows = facade.find(entity).and_then(|find| {
            find.select_field(user_field)
                .select_field(actor_field)
                .distinct(true)
                .disable_authz()
                .list()
        });
        let Ok(rows) = rows else {
            continue;
        };
        for row in rows.values() {
            let user = row.get(user_field).and_then(text);
            let actor = row.get(actor_field).and_then(text);
            let problem = match (&user, &actor) {
                (Some(user), None) => Some(format!("{entity}: user {user} has no actor")),
                (_, Some(actor)) => match known.get(actor) {
                    None => Some(format!("{entity}: actor {actor} does not exist")),
                    Some(owner) if user.is_some() && owner != &user => Some(format!(
                        "{entity}: actor {actor} is not user {}'s",
                        user.clone().unwrap_or_default()
                    )),
                    Some(_) => None,
                },
                (None, None) => None,
            };
            if let Some(problem) = problem {
                tracing::error!("chat actors: {problem}");
                problems.push(problem);
                failed += 1;
            }
        }
    }

    Ok(BTreeMap::from([
        (
            "actorCount".to_string(),
            Value::I64(i64::try_from(actors.len()).unwrap_or(i64::MAX)),
        ),
        ("filledCount".to_string(), Value::I64(filled)),
        ("failedCount".to_string(), Value::I64(failed)),
        ("rebuiltList".to_string(), Value::List(rebuilt)),
        (
            "problemList".to_string(),
            Value::List(problems.into_iter().map(Value::string).collect()),
        ),
    ]))
}

/// The permission the backups need (the chat bridges' administrators: ADMIN has it).
const BACKUP_ADMIN: &str = "ChatBridgeAdmin";

/// A backup of [`REKEYED`] as it is now: (entity, backup table, the backup's rows, the table's rows, why it must stay).
type BackupState = (&'static str, &'static str, u64, u64, Option<&'static str>);

/// The backups the migration kept that are still there, each with whether it may go: only once its table is on the new
/// key and holds at least the backup's rows (a rebuild cut off leaves fewer, and then the backup is the only copy).
fn pre_c1_backups(facade: &EntityFacade) -> Result<Vec<BackupState>, LibError> {
    let mut found = Vec::new();
    for (entity, backup) in REKEYED {
        let Some(kept) = facade.backup_table_rows(entity, backup)? else {
            continue;
        };
        let rekeyed = facade
            .table_primary_key(entity)?
            .iter()
            .any(|column| column.replace('_', "").eq_ignore_ascii_case("chatactorid"));
        let rows = facade.find(entity)?.disable_authz().count()?;
        let rows = u64::try_from(rows).unwrap_or(0);
        let reason = if !rekeyed {
            Some("notRebuilt")
        } else if rows < kept {
            Some("fewerRows")
        } else {
            None
        };
        found.push((entity, backup, kept, rows, reason));
    }
    Ok(found)
}

fn backup_admin() -> Result<(), LibError> {
    if crate::user::has_permission(BACKUP_ADMIN).map_err(|error| LibError::new(error.message().to_string()))? {
        Ok(())
    } else {
        Err(LibError::new(
            "The pre-C1 copies are for administrators (ChatBridgeAdmin)",
        ))
    }
}

fn backup_value(state: &BackupState) -> Value {
    let (entity, backup, kept, rows, reason) = *state;
    let mut map = indexmap::IndexMap::new();
    map.insert("entityName".to_string(), Value::string(entity));
    map.insert("backupTable".to_string(), Value::string(backup));
    map.insert(
        "backupRows".to_string(),
        Value::I64(i64::try_from(kept).unwrap_or(i64::MAX)),
    );
    map.insert(
        "tableRows".to_string(),
        Value::I64(i64::try_from(rows).unwrap_or(i64::MAX)),
    );
    map.insert("droppable".to_string(), Value::Bool(reason.is_none()));
    map.insert("reasonCode".to_string(), reason.map_or(Value::Null, Value::string));
    Value::Map(map)
}

/// `get#PreC1Backups` (ChatBridgeAdmin): the copies the C1 migration kept that are still there. Out: backupList
/// [{entityName, backupTable, backupRows, tableRows, droppable, reasonCode (notRebuilt, fewerRows)}].
fn get_pre_c1_backups(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    backup_admin()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let list = pre_c1_backups(facade)?.iter().map(backup_value).collect();
    Ok(BTreeMap::from([("backupList".to_string(), Value::List(list))]))
}

/// `drop#PreC1Backups` (ChatBridgeAdmin): drop each copy whose table is on its new key with at least the copy's rows;
/// the others stay, each named with why. One that fails is logged and the rest go on. Out: droppedList (the backup
/// tables dropped), keptList (as get#PreC1Backups lists them).
fn drop_pre_c1_backups(
    _parameters: &BTreeMap<String, Value>,
    facade: &EntityFacade,
) -> Result<BTreeMap<String, Value>, LibError> {
    backup_admin()?;
    let _authz = ExecutionContext::with_current(ExecutionContext::disable_authz);
    let mut dropped = Vec::new();
    let mut kept = Vec::new();
    for state in pre_c1_backups(facade)? {
        let (entity, backup, rows_kept, rows, reason) = state;
        if let Some(reason) = reason {
            tracing::warn!("chat actors: {backup} is kept: {entity} has {rows} rows, the copy {rows_kept} ({reason})");
            kept.push(backup_value(&state));
            continue;
        }
        match facade.drop_backup_table(entity, backup) {
            Ok(_) => {
                tracing::info!("chat actors: {backup} dropped ({entity} has {rows} rows, the copy had {rows_kept})");
                dropped.push(Value::string(backup));
            }
            Err(error) => {
                tracing::error!("chat actors: {backup} was not dropped: {error}");
                let mut value = backup_value(&state);
                if let Value::Map(map) = &mut value {
                    map.insert("reasonCode".to_string(), Value::string("dropFailed"));
                    map.insert("droppable".to_string(), Value::Bool(false));
                }
                kept.push(value);
            }
        }
    }
    Ok(BTreeMap::from([
        ("droppedList".to_string(), Value::List(dropped)),
        ("keptList".to_string(), Value::List(kept)),
    ]))
}
