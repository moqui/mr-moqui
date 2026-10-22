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

//! Encrypt on write, decrypt on read, audit changed fields, and overlay localized fields.

use std::cell::Cell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashSet};
use std::sync::Arc;

use moqui_db::model::{EntityDefinition, EntityModel, PqAlias};
use moqui_db::sql::{Condition, FieldRef};
use moqui_lib::error::Error;
use moqui_lib::object::{decimal_compare, to_plain_string, Value};

use super::value::FieldMap;
use super::crypto::{decrypt_with_alts, encrypt};
use super::facade::EntityFacade;
use crate::context::ExecutionContext;

const AUDIT_ENTITY: &str = "moqui.entity.EntityAuditLog";
const LOCALIZED_ENTITY: &str = "moqui.basic.LocalizedEntityField";
const LOCALIZED_MESSAGE: &str = "moqui.basic.LocalizedMessage";
const TEXT_LIMIT: usize = 4000;

thread_local! {
    static RAW_READ: Cell<bool> = const { Cell::new(false) };
}

/// Sets the raw-read flag and restores the value it replaced on drop, so a nested raw read leaves
/// the outer one in place.
pub(super) struct RawReadGuard(bool);

impl RawReadGuard {
    pub(super) fn enter() -> Self {
        Self(RAW_READ.with(|flag| flag.replace(true)))
    }
}

impl Drop for RawReadGuard {
    fn drop(&mut self) {
        RAW_READ.with(|flag| flag.set(self.0));
    }
}

pub(crate) fn raw_read() -> bool {
    RAW_READ.with(Cell::get)
}

/// Prepare field values before SQL. Encrypts, then the returned map is what the binder uses.
pub fn before_write(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    operation: &str,
    values: FieldMap,
) -> Result<FieldMap, Error> {
    before_write_known(facade, entity, operation, values, None)
}

/// [`before_write`] for an update that already read the stored row (`stored`), which the audit log compares with
/// instead of reading it again.
pub(crate) fn before_write_known(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    operation: &str,
    values: FieldMap,
    stored: Option<&FieldMap>,
) -> Result<FieldMap, Error> {
    audit_log(facade, entity, operation, &values, stored)?;
    encrypt_fields(facade, entity, values)
}

/// [`before_write_known`] for values a caller keeps: borrowed back when no field is encrypted, so a write of an entity
/// with no encrypted field does not copy them.
pub(crate) fn before_write_shared<'a>(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    operation: &str,
    values: std::borrow::Cow<'a, FieldMap>,
    stored: Option<&FieldMap>,
) -> Result<std::borrow::Cow<'a, FieldMap>, Error> {
    audit_log(facade, entity, operation, &values, stored)?;
    if !has_encrypted(entity) {
        return Ok(values);
    }
    encrypt_fields(facade, entity, values.into_owned()).map(std::borrow::Cow::Owned)
}

/// Encrypt a row read from the database for a copy of it, with no audit: nothing about it changed.
pub(crate) fn seal_stored(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    values: FieldMap,
) -> Result<FieldMap, Error> {
    encrypt_fields(facade, entity, values)
}

/// Decrypt a row as read. The stored values stay as they are: localization is applied when a field is read, through
/// [`super::EntityValue::get_localized`], as Java's `getKnownField`, so a write of the value writes what was stored.
pub fn after_read(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    values: FieldMap,
) -> Result<FieldMap, Error> {
    decrypt_fields(facade, entity, values)
}

/// Decrypt a row for the find cache.
pub(crate) fn decrypt_stored(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    values: FieldMap,
) -> Result<FieldMap, Error> {
    decrypt_fields(facade, entity, values)
}

fn encrypt_fields(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    mut values: FieldMap,
) -> Result<FieldMap, Error> {
    if !has_encrypted(entity) {
        return Ok(values);
    }
    let Some((primary, _)) = facade.crypt() else {
        return Ok(values);
    };
    for field in &entity.fields {
        if !field.encrypt {
            continue;
        }
        let Some(plain) = take_string(entity, &field.name, &values)? else {
            continue;
        };
        if plain == DECRYPT_FAILED {
            return Err(Error::new(format!(
                "To prevent data loss, not allowing decrypt failed placeholder for field {} of entity {}",
                field.name, entity.full_name
            )));
        }
        let stored = encrypt(&plain, &primary)?;
        values.insert(Arc::from(field.name.as_str()), Value::string(stored));
    }
    Ok(values)
}

/// The value of an encrypted field that could not be decrypted, as Java's `FieldInfo.decryptFailedMagicString`.
pub const DECRYPT_FAILED: &str = "_DECRYPT_FAILED_";

fn decrypt_fields(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    mut values: FieldMap,
) -> Result<FieldMap, Error> {
    let names = encrypted_names(entity);
    if names.is_empty() {
        return Ok(values);
    }
    let Some((primary, alts)) = facade.crypt() else {
        return Ok(values);
    };
    for name in names {
        let Some(stored) = take_string(entity, name, &values)? else {
            continue;
        };
        match decrypt_with_alts(&stored, &primary, &alts) {
            Ok(plain) => {
                values.insert(Arc::from(name), Value::string(plain));
            }
            // A value with no colon that is not hex was stored as plaintext.
            Err(_) if !stored.contains(':') && !is_hex_ciphertext(&stored) => {}
            // As Java's `FieldInfo`, the read goes on with a clear placeholder, which a write refuses.
            Err(err) => {
                tracing::error!(error = %err, "Error decrypting field {name} of entity {}", entity.full_name);
                values.insert(Arc::from(name), Value::string(DECRYPT_FAILED));
            }
        }
    }
    Ok(values)
}

/// A find condition on an encrypted field compares with the stored form of the value, as Java binds it: the encryption is
/// deterministic (a constant IV) so that a query by value finds the row. As Java's `FieldInfo` every bound value of the
/// field is encrypted, whatever the operator, and a view's alias of an encrypted field is one too. A condition that names
/// another entity's field is left as it is.
pub(super) fn encrypt_condition(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    condition: Option<Condition>,
) -> Result<Option<Condition>, Error> {
    let Some(condition) = condition else {
        return Ok(None);
    };
    let names = encrypted_names(entity);
    if names.is_empty() {
        return Ok(Some(condition));
    }
    let Some((primary, _)) = facade.crypt() else {
        return Ok(Some(condition));
    };
    Ok(Some(encrypt_in(&names, &primary, condition)?))
}

fn encrypt_in(names: &[&str], config: &super::crypto::CryptConfig, condition: Condition) -> Result<Condition, Error> {
    let encrypted = |field: &FieldRef| field.entity_alias.is_none() && names.contains(&field.name.as_str());
    let seal = |value: Value| -> Result<Value, Error> {
        match value.as_str() {
            Some(plain) => Ok(Value::string(encrypt(plain, config)?)),
            None => Ok(value),
        }
    };
    let seal_all = |values: Vec<Value>| values.into_iter().map(seal).collect::<Result<Vec<_>, _>>();
    let recurse = |list: Vec<Condition>| {
        list.into_iter()
            .map(|inner| encrypt_in(names, config, inner))
            .collect::<Result<Vec<_>, _>>()
    };
    Ok(match condition {
        Condition::Equals(field, value) if encrypted(&field) => Condition::Equals(field, seal(value)?),
        Condition::NotEquals(field, value) if encrypted(&field) => Condition::NotEquals(field, seal(value)?),
        Condition::Less(field, value) if encrypted(&field) => Condition::Less(field, seal(value)?),
        Condition::Greater(field, value) if encrypted(&field) => Condition::Greater(field, seal(value)?),
        Condition::LessEqual(field, value) if encrypted(&field) => Condition::LessEqual(field, seal(value)?),
        Condition::GreaterEqual(field, value) if encrypted(&field) => Condition::GreaterEqual(field, seal(value)?),
        Condition::Like(field, value) if encrypted(&field) => Condition::Like(field, seal(value)?),
        Condition::NotLike(field, value) if encrypted(&field) => Condition::NotLike(field, seal(value)?),
        Condition::Between(field, low, high) if encrypted(&field) => Condition::Between(field, seal(low)?, seal(high)?),
        Condition::NotBetween(field, low, high) if encrypted(&field) => {
            Condition::NotBetween(field, seal(low)?, seal(high)?)
        }
        Condition::In(field, values) if encrypted(&field) => Condition::In(field, seal_all(values)?),
        Condition::NotIn(field, values) if encrypted(&field) => Condition::NotIn(field, seal_all(values)?),
        Condition::And(list) => Condition::And(recurse(list)?),
        Condition::Or(list) => Condition::Or(recurse(list)?),
        Condition::Not(inner) => Condition::Not(Box::new(encrypt_in(names, config, *inner)?)),
        Condition::IgnoreCase(inner) => Condition::IgnoreCase(Box::new(encrypt_in(names, config, *inner)?)),
        other => other,
    })
}

/// The encrypted fields: an entity's `encrypt` fields, or a view's aliases of one (Java copies `encrypt` to the alias).
fn encrypted_names(entity: &EntityDefinition) -> Vec<&str> {
    if entity.is_view {
        entity
            .aliases
            .iter()
            .filter(|alias| alias.encrypt && alias.function.is_none())
            .map(|alias| alias.name.as_str())
            .collect()
    } else {
        entity
            .fields
            .iter()
            .filter(|field| field.encrypt)
            .map(|field| field.name.as_str())
            .collect()
    }
}

fn has_encrypted(entity: &EntityDefinition) -> bool {
    entity.fields.iter().any(|field| field.encrypt)
}

fn take_string(
    entity: &EntityDefinition,
    field: &str,
    values: &FieldMap,
) -> Result<Option<String>, Error> {
    let Some(value) = values.get(field) else {
        return Ok(None);
    };
    if value.is_null() {
        return Ok(None);
    }
    match value.as_str() {
        Some(text) => Ok(Some(text.to_string())),
        None => Err(Error::new(format!(
            "The encrypt attribute was set to true on non-String field {field} of entity {}",
            entity.full_name
        ))),
    }
}

fn is_hex_ciphertext(value: &str) -> bool {
    // Same acceptance as crypto::parse_hex, including an empty string.
    let bytes = value.as_bytes();
    bytes.len().is_multiple_of(2) && bytes.iter().all(u8::is_ascii_hexdigit)
}

fn audit_log(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    operation: &str,
    values: &FieldMap,
    stored: Option<&FieldMap>,
) -> Result<(), Error> {
    if operation == "delete" || facade.flags().disable_audit_log || facade.sim_active() {
        return Ok(());
    }
    if entity.full_name == AUDIT_ENTITY {
        return Ok(());
    }
    if !entity.fields.iter().any(|field| {
        field
            .enable_audit_log
            .as_deref()
            .is_some_and(|flag| field_selected(operation, flag))
    }) {
        return Ok(());
    }
    let pk_names = entity.pk_field_names();
    if pk_names.is_empty() || pk_names.iter().any(|name| values.get(*name).is_none_or(Value::is_null)) {
        return Ok(());
    }
    let old = match stored {
        Some(row) if operation == "update" => row.clone(),
        _ if operation == "update" => stored_row(facade, entity, values)?,
        _ => BTreeMap::new(),
    };
    for field in &entity.fields {
        let Some(flag) = field.enable_audit_log.as_deref() else {
            continue;
        };
        if !field_selected(operation, flag) || !values.contains_key(field.name.as_str()) {
            continue;
        }
        let new_value = values.get(field.name.as_str());
        let old_value = old.get(field.name.as_str());
        if operation == "create" && new_value.is_none_or(Value::is_null) {
            continue;
        }
        // "update" does not log the first non-null value. That is the initial set.
        if operation == "update" && flag == "update" && old_value.is_none_or(Value::is_null) {
            continue;
        }
        if operation == "update" && same_stored(old_value, new_value) {
            continue;
        }
        let new_text = audit_text(facade, field.encrypt, new_value)?;
        let old_text = audit_text(facade, field.encrypt, old_value)?;
        insert_audit(facade, entity, &field.name, values, new_text, old_text)?;
    }
    Ok(())
}

fn field_selected(operation: &str, flag: &str) -> bool {
    match flag {
        "true" => operation == "create" || operation == "update",
        "create" => operation == "create",
        "update" => operation == "update",
        _ => false,
    }
}

fn stored_row(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    values: &FieldMap,
) -> Result<FieldMap, Error> {
    let _guard = RawReadGuard::enter();
    let mut find = facade.find(&entity.full_name)?;
    for name in entity.pk_field_names() {
        let Some(value) = values.get(name).cloned() else {
            return Ok(BTreeMap::new());
        };
        find = find.eq(name, value);
    }
    Ok(find.one()?.map(|row| row.fields().clone()).unwrap_or_default())
}

fn same_stored(old: Option<&Value>, new: Option<&Value>) -> bool {
    match (present(old), present(new)) {
        (None, None) => true,
        (Some(Value::Decimal(left)), Some(Value::Decimal(right))) => decimal_compare(*left, *right) == Ordering::Equal,
        (Some(left), Some(right)) => to_plain_string(left) == to_plain_string(right),
        _ => false,
    }
}

fn present(value: Option<&Value>) -> Option<&Value> {
    value.filter(|item| !item.is_null())
}

fn audit_text(facade: &EntityFacade, encrypt_field: bool, value: Option<&Value>) -> Result<Option<String>, Error> {
    let Some(value) = present(value) else {
        return Ok(None);
    };
    // A decimal read back from the database carries the column scale (10.000000); the audit text is the plain value (10).
    let plain = audit_plain(value);
    let text = if encrypt_field {
        if let Some((primary, _)) = facade.crypt() {
            encrypt(&plain, &primary)?
        } else {
            plain
        }
    } else {
        plain
    };
    Ok(Some(clip_text(&text)))
}

fn audit_plain(value: &Value) -> String {
    match value {
        Value::Decimal(number) => number.normalize().to_string(),
        other => to_plain_string(other),
    }
}

fn clip_text(text: &str) -> String {
    if text.chars().nth(TEXT_LIMIT).is_none() {
        return text.to_string();
    }
    text.chars().take(TEXT_LIMIT).collect()
}

fn audit_stack() -> String {
    let names = ExecutionContext::with_current(ExecutionContext::artifact_stack_names).unwrap_or_default();
    // Java length/substring count UTF-16 units. A split surrogate is represented lossily in a Rust string.
    String::from_utf16_lossy(&names.encode_utf16().take(TEXT_LIMIT).collect::<Vec<_>>())
}

fn insert_audit(
    facade: &EntityFacade,
    entity: &EntityDefinition,
    field_name: &str,
    values: &FieldMap,
    new_text: Option<String>,
    old_text: Option<String>,
) -> Result<(), Error> {
    let mut row = BTreeMap::new();
    row.insert(
        "changedEntityName".to_string(),
        Value::string(entity.full_name.as_str()),
    );
    row.insert("changedFieldName".to_string(), Value::string(field_name));
    row.insert("changedDate".to_string(), changed_now());
    row.insert("artifactStack".to_string(), Value::string(audit_stack()));
    // Java stamps the acting user and visit on each audit row.
    if let Some(user_id) = crate::user::current_user_id()
        .ok()
        .flatten()
        .filter(|id| !id.is_empty())
    {
        row.insert("changedByUserId".to_string(), Value::string(user_id));
    }
    if let Some(visit_id) = crate::user::visit_id().ok().flatten().filter(|id| !id.is_empty()) {
        row.insert("changedInVisitId".to_string(), Value::string(visit_id));
    }
    if let Some(text) = new_text {
        row.insert("newValueText".to_string(), Value::string(text));
    }
    if let Some(text) = old_text {
        row.insert("oldValueText".to_string(), Value::string(text));
    }
    // As Java: a `{field}_changeReason` in the context.
    if let Some(reason) = crate::script::context_value(&format!("{field_name}_changeReason"))
        .map(|value| moqui_lib::object::to_plain_string(&value))
        .filter(|reason| !reason.is_empty())
    {
        row.insert("changeReason".to_string(), Value::string(reason));
    }
    put_pk(&mut row, entity, values);
    // As Java, through `create#moqui.entity.EntityAuditLog` with authorization off, so its service ECA rules run. With
    // no context entered, or one whose services write through another facade, the row is created directly.
    let same_facade = ExecutionContext::with_current(|ctx| ctx.factory().entity())
        .flatten()
        .is_some_and(|current| std::sync::Arc::ptr_eq(&current.inner, &facade.inner));
    if same_facade {
        let options = crate::service::call::CallOptions {
            disable_authz: true,
            ..Default::default()
        };
        crate::service::call::sync_with(&format!("create#{AUDIT_ENTITY}"), &row, &options)?;
        return Ok(());
    }
    let mut value = facade.make_value(AUDIT_ENTITY)?;
    value.set_sequenced_id_primary(facade)?;
    for (name, field_value) in row {
        value.set(name, field_value);
    }
    value.create(facade)
}

fn put_pk(row: &mut BTreeMap<String, Value>, entity: &EntityDefinition, values: &FieldMap) {
    let names = entity.pk_field_names();
    if let Some(name) = names.first() {
        if let Some(text) = plain_text(values.get(*name)) {
            row.insert("pkPrimaryValue".to_string(), Value::string(text));
        }
    }
    if let Some(name) = names.get(1) {
        if let Some(text) = plain_text(values.get(*name)) {
            row.insert("pkSecondaryValue".to_string(), Value::string(text));
        }
    }
    if names.len() <= 2 {
        return;
    }
    let mut combined = String::new();
    for (index, name) in names.iter().skip(2).enumerate() {
        if index > 0 {
            combined.push(',');
        }
        let text = plain_text(values.get(*name)).unwrap_or_else(|| "null".to_string());
        combined.push_str(name);
        combined.push_str(":'");
        combined.push_str(&text);
        combined.push('\'');
    }
    if !combined.is_empty() {
        row.insert("pkRestCombinedValue".to_string(), Value::string(combined));
    }
}

fn plain_text(value: Option<&Value>) -> Option<String> {
    present(value).map(to_plain_string)
}

/// Java's audit log stamps `ec.user.nowTimestamp`, so it follows the effective time.
fn changed_now() -> Value {
    crate::user::now_effective_value()
}

/// The localized fields of a find's entity, worked out once per find and shared by each value it returns, and a view's
/// `pq-expression` aliases, which a value also reads in place of a stored field.
#[derive(Debug)]
pub(crate) struct L10nTargets {
    targets: Vec<LocalTarget>,
    pq_aliases: Vec<PqAlias>,
}

impl L10nTargets {
    pub(crate) fn has(&self, column: &str) -> bool {
        self.targets.iter().any(|target| target.column == column)
    }

    /// The expression of the `pq-expression` alias `name`.
    pub(crate) fn pq_expression(&self, name: &str) -> Option<&str> {
        self.pq_aliases
            .iter()
            .find(|alias| alias.name == name)
            .map(|alias| alias.expression.as_str())
    }

    pub(crate) fn pq_names(&self) -> impl Iterator<Item = &str> {
        self.pq_aliases.iter().map(|alias| alias.name.as_str())
    }

    pub(crate) fn columns(&self) -> impl Iterator<Item = &str> {
        self.targets.iter().map(|target| target.column.as_str())
    }

    /// The columns the lookup of `column` reads: itself and its record's primary key.
    pub(crate) fn read_columns<'a>(&'a self, column: &'a str) -> impl Iterator<Item = &'a str> {
        let pk = self
            .targets
            .iter()
            .find(|target| target.column == column)
            .and_then(|target| target.pk_column.as_deref());
        std::iter::once(column).chain(pk)
    }
}

#[derive(Debug)]
struct LocalTarget {
    entity_name: String,
    field_name: String,
    column: String,
    /// The column of the record's single primary key field; `None` when it has more than one.
    pk_column: Option<String>,
}

/// The localized fields and `pq-expression` aliases of `entity`, or `None` when it has none or this is a raw read (an
/// internal read that must see what is stored). A lookup that fails is logged and the values stay as stored.
pub(crate) fn l10n_targets(facade: &EntityFacade, entity: &EntityDefinition) -> Option<std::sync::Arc<L10nTargets>> {
    if raw_read() {
        return None;
    }
    let targets = if entity.is_view {
        match view_targets_for(facade, entity) {
            Ok(targets) => targets,
            Err(err) => {
                tracing::error!(error = %err, "Could not find the localized fields of {}, not localizing", entity.full_name);
                Vec::new()
            }
        }
    } else {
        plain_targets(entity)
    };
    let pq_aliases = entity.pq_aliases.clone();
    (!targets.is_empty() || !pq_aliases.is_empty()).then(|| std::sync::Arc::new(L10nTargets { targets, pq_aliases }))
}

/// The user's locale, as Java's `userFacade.getLocale()`; the facade's own with no execution context.
pub(crate) fn current_locale(facade: &EntityFacade) -> String {
    if crate::user::context_entered() {
        crate::l10n::locale()
    } else {
        facade.locale()
    }
}

/// The localized value of `column` in `locale`, or `None` to use the stored value: the column is not localized, there
/// is no localization, or the lookup failed (logged; the read itself still succeeds).
pub(crate) fn localize_field(
    facade: &EntityFacade,
    targets: &L10nTargets,
    values: &FieldMap,
    column: &str,
    locale: &str,
) -> Option<Value> {
    let target = targets.targets.iter().find(|target| target.column == column)?;
    let locales = locale_chain(locale);
    if locales.is_empty() {
        return None;
    }
    let pk = target.pk_column.as_ref().and_then(|column| pk_text(values.get(column.as_str())));
    let stored = plain_text(values.get(target.column.as_str())).filter(|text| !text.is_empty());
    if pk.is_none() && stored.is_none() {
        return None;
    }
    let pk = pk.unwrap_or_default();
    match find_localized(facade, target, &pk, stored.as_deref(), &locales) {
        Ok(found) => found,
        Err(err) => {
            tracing::error!(
                error = %err,
                "Could not localize field {} of {} {pk}, using the stored value",
                target.field_name,
                target.entity_name
            );
            None
        }
    }
}

fn plain_targets(entity: &EntityDefinition) -> Vec<LocalTarget> {
    let pk_names = entity.pk_field_names();
    let pk_column = (pk_names.len() == 1).then(|| pk_names[0].to_string());
    entity
        .fields
        .iter()
        .filter(|field| field.enable_localization)
        .map(|field| LocalTarget {
            entity_name: entity.full_name.clone(),
            field_name: field.name.clone(),
            column: field.name.clone(),
            pk_column: pk_column.clone(),
        })
        .collect()
}

/// A dynamic view is not in the model. Use a member entity to reach the model, and keep `view` as given.
fn view_targets_for(facade: &EntityFacade, view: &EntityDefinition) -> Result<Vec<LocalTarget>, Error> {
    let anchor = view
        .member_entities
        .first()
        .map(|member| member.entity_name.as_str())
        .unwrap_or(view.full_name.as_str());
    match facade.with_entity(anchor, |_, _, model| view_targets(view, model)) {
        Ok(targets) => Ok(targets),
        Err(_) if anchor == view.full_name => Ok(Vec::new()),
        Err(err) => Err(err),
    }
}

fn view_targets(view: &EntityDefinition, model: &EntityModel) -> Result<Vec<LocalTarget>, Error> {
    let mut targets = Vec::new();
    let mut seen = HashSet::new();
    for alias in &view.aliases {
        if alias.function.is_some() {
            continue;
        }
        let Some(entity_alias) = alias.entity_alias.as_deref() else {
            continue;
        };
        let Some(member_name) = member_entity_name(view, entity_alias) else {
            continue;
        };
        let member = model.entity(&member_name)?;
        let field_name = alias.field.clone().unwrap_or_else(|| alias.name.clone());
        if !member.field(&field_name).is_some_and(|field| field.enable_localization) {
            continue;
        }
        let pk_column = member_pk_column(view, member, entity_alias);
        push_target(
            &mut targets,
            &mut seen,
            LocalTarget {
                entity_name: member.full_name.clone(),
                field_name,
                column: alias.name.clone(),
                pk_column,
            },
        );
    }
    Ok(targets)
}

fn push_target(targets: &mut Vec<LocalTarget>, seen: &mut HashSet<String>, target: LocalTarget) {
    if seen.insert(target.column.clone()) {
        targets.push(target);
    }
}

fn member_entity_name(view: &EntityDefinition, alias: &str) -> Option<String> {
    view.member(alias).map(|member| member.entity_name.clone())
}

fn member_pk_column(view: &EntityDefinition, member: &EntityDefinition, entity_alias: &str) -> Option<String> {
    let pk_names = member.pk_field_names();
    if pk_names.len() != 1 {
        return None;
    }
    let pk = pk_names[0].to_string();
    for alias in &view.aliases {
        if alias.function.is_some() || alias.entity_alias.as_deref() != Some(entity_alias) {
            continue;
        }
        let source = alias.field.as_deref().unwrap_or(alias.name.as_str());
        if source == pk {
            return Some(alias.name.clone());
        }
    }
    None
}

fn pk_text(value: Option<&Value>) -> Option<String> {
    let text = plain_text(value)?;
    if text.is_empty() {
        None
    } else {
        Some(text)
    }
}

/// Full locale, then drop the region, then the language, then `default`, as Java's localized field get.
fn locale_chain(locale: &str) -> Vec<String> {
    let mut chain = Vec::new();
    push_locale(&mut chain, locale);
    let normalized = locale.replace('-', "_");
    push_locale(&mut chain, &normalized);
    let mut current = normalized;
    while let Some((head, _)) = current.rsplit_once('_') {
        if head.is_empty() {
            break;
        }
        push_locale(&mut chain, head);
        current = head.to_string();
    }
    if !chain.is_empty() {
        push_locale(&mut chain, "default");
    }
    chain
}

fn push_locale(chain: &mut Vec<String>, locale: &str) {
    if locale.is_empty() || chain.iter().any(|item| item == locale) {
        return;
    }
    chain.push(locale.to_string());
}

/// Java's localized field get: the `LocalizedEntityField` for the record (single primary key only) in each locale of
/// the chain, else the `LocalizedMessage` for the stored text in each locale. Both read through the entity cache.
fn find_localized(
    facade: &EntityFacade,
    target: &LocalTarget,
    pk: &str,
    stored: Option<&str>,
    locales: &[String],
) -> Result<Option<Value>, Error> {
    if !pk.is_empty() {
        for locale in locales {
            let row = facade
                .find(LOCALIZED_ENTITY)?
                .eq("entityName", Value::string(target.entity_name.as_str()))
                .eq("fieldName", Value::string(target.field_name.as_str()))
                .eq("pkValue", Value::string(pk))
                .eq("locale", Value::string(locale.as_str()))
                .use_cache(true)
                .one()?;
            if let Some(row) = row {
                return Ok(Some(row.get("localized").cloned().unwrap_or(Value::Null)));
            }
        }
    }
    let Some(original) = stored else {
        return Ok(None);
    };
    for locale in locales {
        let row = facade
            .find(LOCALIZED_MESSAGE)?
            .eq("original", Value::string(original))
            .eq("locale", Value::string(locale.as_str()))
            .use_cache(true)
            .one()?;
        if let Some(row) = row {
            return Ok(Some(row.get("localized").cloned().unwrap_or(Value::Null)));
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::{audit_plain, audit_stack, encrypt_in, raw_read, RawReadGuard};
    use moqui_db::sql::Condition;
    use moqui_lib::object::Value;

    use crate::context::ExecutionContextFactory;
    use crate::entity::{EntityFacade, EntityList};

    fn audit_facade() -> EntityFacade {
        let facade = crate::testing::open();
        facade.set_disable_audit_log(false);
        facade
    }

    fn write_username(facade: &EntityFacade, username: &str, create: bool) {
        let mut user = facade.make_value("moqui.security.UserAccount").unwrap();
        user.set("userId", Value::string("AUDIT_STACK"));
        user.set("username", Value::string(username));
        if create {
            user.create(facade).unwrap();
        } else {
            user.update(facade).unwrap();
        }
    }

    fn username_audits(facade: &EntityFacade) -> EntityList {
        facade
            .find("moqui.entity.EntityAuditLog")
            .unwrap()
            .eq("changedEntityName", Value::string("moqui.security.UserAccount"))
            .eq("changedFieldName", Value::string("username"))
            .list()
            .unwrap()
    }

    #[test]
    fn audit_stack_follows_create_update_and_scoped_contexts() {
        let facade = audit_facade();
        let context = ExecutionContextFactory::test_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        assert!(facade.begin("moqui.security.UserAccount", None).unwrap());
        context.push("AT_OTHER", "data load", "all");
        context.push("AT_SERVICE", "create#Account", "create");
        write_username(&facade, "alice", true);
        context.pop().unwrap();
        assert_eq!(context.artifact_stack_names(), "data load");

        // A nested execution context must not pick up the outer context's stack.
        let inner = ExecutionContextFactory::test_context();
        {
            let _entered = inner.enter();
            let _authz = inner.disable_authz();
            inner.push("AT_SERVICE", "update#Account", "update");
            write_username(&facade, "alice2", false);
            assert_eq!(inner.artifact_stack_names(), "update#Account");
        }
        write_username(&facade, "alice3", false);
        assert_eq!(context.artifact_stack_names(), "data load");
        facade.commit("moqui.security.UserAccount").unwrap();

        // Stack capture must not move audit writes out of the entity's transaction.
        assert!(facade.begin("moqui.security.UserAccount", None).unwrap());
        write_username(&facade, "rolled-back", false);
        facade.rollback("moqui.security.UserAccount").unwrap();
        let audits = username_audits(&facade);
        assert_eq!(audits.len(), 3);
        for (username, expected) in [
            ("alice", "moqui.security.UserAccount, create#Account, data load"),
            ("alice2", "moqui.security.UserAccount, update#Account"),
            ("alice3", "moqui.security.UserAccount, data load"),
        ] {
            let row = audits
                .iter()
                .find(|row| row.get("newValueText").and_then(Value::as_str) == Some(username))
                .unwrap();
            assert_eq!(row.get("artifactStack").and_then(Value::as_str), Some(expected));
        }
    }

    #[test]
    fn audit_stack_without_an_entered_context_is_empty_not_null() {
        let facade = audit_facade();
        write_username(&facade, "alice", true);
        write_username(&facade, "alice2", false);
        let audits = username_audits(&facade);
        assert_eq!(audits.len(), 2);
        for row in audits.iter() {
            assert_eq!(row.get("artifactStack").and_then(Value::as_str), Some(""));
        }
    }

    #[test]
    fn audit_stack_of_an_entered_empty_context_is_empty() {
        let context = ExecutionContextFactory::test_context();
        let _entered = context.enter();
        assert_eq!(audit_stack(), "");
    }

    #[test]
    fn audit_stack_truncates_to_java_utf16_length() {
        let facade = audit_facade();
        let context = ExecutionContextFactory::test_context();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        let prefix = "moqui.security.UserAccount, ";
        // Non-BMP characters count as two Java string characters, not one Rust char or four bytes.
        let name = format!("{}😀tail", "é".repeat(3997 - prefix.len()));
        context.push("AT_OTHER", &name, "all");
        write_username(&facade, "alice", true);
        let audits = username_audits(&facade);
        let stack = audits
            .get(0)
            .unwrap()
            .get("artifactStack")
            .and_then(Value::as_str)
            .unwrap();
        assert_eq!(stack, format!("{prefix}{}😀t", "é".repeat(3997 - prefix.len())));
        assert_eq!(stack.encode_utf16().count(), 4000);
        context.pop().unwrap();

        // Java substring can split a surrogate pair. Rust strings represent that orphan lossily.
        context.push("AT_OTHER", &format!("{}😀", "a".repeat(3999 - prefix.len())), "all");
        write_username(&facade, "alice2", false);
        let audits = username_audits(&facade);
        let row = audits
            .iter()
            .find(|row| row.get("newValueText").and_then(Value::as_str) == Some("alice2"))
            .unwrap();
        let stack = row.get("artifactStack").and_then(Value::as_str).unwrap();
        assert_eq!(stack, format!("{prefix}{}\u{fffd}", "a".repeat(3999 - prefix.len())));
        assert_eq!(stack.encode_utf16().count(), 4000);
    }

    /// As Java's `FieldInfo`, every bound value of an encrypted field is sealed, whatever the operator.
    #[test]
    fn every_bound_value_of_an_encrypted_field_is_sealed() {
        let config = crate::entity::crypto::CryptConfig {
            pass: "MoquiDefaultPassword:CHANGEME".to_string(),
            salt: "20201202".to_string(),
            iterations: 10,
            algorithm: "PBEWithHmacSHA256AndAES_128".to_string(),
        };
        let sealed = Value::string(crate::entity::crypto::encrypt("v", &config).unwrap());
        let plain = || Value::string("v");
        let condition = Condition::and(vec![
            Condition::Like("secret".into(), plain()),
            Condition::Between("secret".into(), plain(), plain()),
            Condition::IgnoreCase(Box::new(Condition::Greater("secret".into(), plain()))),
            Condition::Equals("other".into(), plain()),
        ]);
        let Condition::And(list) = encrypt_in(&["secret"], &config, condition).unwrap() else {
            panic!("an and");
        };
        assert_eq!(list[0], Condition::Like("secret".into(), sealed.clone()));
        assert_eq!(
            list[1],
            Condition::Between("secret".into(), sealed.clone(), sealed.clone())
        );
        assert_eq!(
            list[2],
            Condition::IgnoreCase(Box::new(Condition::Greater("secret".into(), sealed)))
        );
        assert_eq!(list[3], Condition::Equals("other".into(), plain()));
    }

    #[test]
    fn audit_text_drops_the_column_scale_of_a_decimal() {
        let value = Value::Decimal("10.000000".parse().unwrap());
        assert_eq!(audit_plain(&value), "10");
        assert_eq!(audit_plain(&Value::Decimal("1.50".parse().unwrap())), "1.5");
        assert_eq!(audit_plain(&Value::string("x")), "x");
    }

    #[test]
    fn a_nested_raw_read_restores_the_outer_flag() {
        assert!(!raw_read());
        {
            let _outer = RawReadGuard::enter();
            {
                let _inner = RawReadGuard::enter();
                assert!(raw_read());
            }
            assert!(raw_read());
        }
        assert!(!raw_read());
    }
}
