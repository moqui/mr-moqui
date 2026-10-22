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

//! One entity record and a list of records.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, RwLock, RwLockReadGuard};
use std::time::{SystemTime, UNIX_EPOCH};

use moqui_db::model::EntityDefinition;
use moqui_db::sql::{delete, insert, insert_or_update, update, Condition};
use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, Value};
use moqui_lib::sync::{lock, read, write};

use super::cache::WriteGate;
use super::exec::{after_statement, execute_deferred_row_tx, execute_deferred_tx, execute_tx, pipelining};
use super::facade::EntityFacade;
use super::field_rw;
use super::load_feed::EcaRule;

use super::{cache, fk, load_feed, sequence};

/// The fields of a record by name. A name is shared, not copied: the rows of one read share each column's name, and a
/// copy of a map shares its names, as Java's entity values share interned field names.
pub type FieldMap = BTreeMap<Arc<str>, Value>;

/// A field name as [`EntityValue::set`] takes it: text, copied once, or a name already shared, which is not copied.
pub trait IntoFieldName {
    fn name_str(&self) -> &str;

    fn into_field_name(self) -> Arc<str>;
}

impl IntoFieldName for &str {
    fn name_str(&self) -> &str {
        self
    }

    fn into_field_name(self) -> Arc<str> {
        Arc::from(self)
    }
}

impl IntoFieldName for String {
    fn name_str(&self) -> &str {
        self
    }

    fn into_field_name(self) -> Arc<str> {
        Arc::from(self)
    }
}

impl IntoFieldName for &String {
    fn name_str(&self) -> &str {
        self
    }

    fn into_field_name(self) -> Arc<str> {
        Arc::from(self.as_str())
    }
}

impl IntoFieldName for Arc<str> {
    fn name_str(&self) -> &str {
        self
    }

    fn into_field_name(self) -> Arc<str> {
        self
    }
}

impl IntoFieldName for &Arc<str> {
    fn name_str(&self) -> &str {
        self
    }

    fn into_field_name(self) -> Arc<str> {
        Arc::clone(self)
    }
}

/// `map` with its names as [`FieldMap`] names.
pub fn field_map(map: BTreeMap<String, Value>) -> FieldMap {
    map.into_iter().map(|(name, value)| (Arc::from(name), value)).collect()
}

/// `fields` with owned names, for a map that leaves the entity layer: service parameters, a script's map.
pub fn string_map(fields: &FieldMap) -> BTreeMap<String, Value> {
    fields
        .iter()
        .map(|(name, value)| (name.to_string(), value.clone()))
        .collect()
}

/// One entity value. Field values are [`Value`].
///
/// The fields are the stored values, decrypted. A value read by a find of an entity with localized fields also reads
/// those fields in the user's locale through [`Self::get_localized`] and [`Self::localized_fields`], as Java's
/// `get` and `getMap`, and a value of a view reads its `pq-expression` aliases there too; [`Self::get`],
/// [`Self::fields`] and every write use the stored values.
///
/// A value read from the database (a find, the entity cache or a refresh), or written to it since, keeps what each
/// field it sets held before, as Java's `dbValueMap`, so its update writes only the fields that changed and leaves a
/// field another transaction changed in between as that one wrote it. A value from [`EntityFacade::make_value`] that
/// was not written yet updates every field it holds. See [`Self::is_field_modified`].
#[derive(Clone, Debug)]
pub struct EntityValue {
    /// Shared. A clone does not copy the name.
    entity_name: Arc<str>,
    /// Shared until [`Self::set`] or [`Self::refresh`]. A cache hit clones the `Arc`.
    fields: Arc<FieldMap>,
    /// The entity's field names, sorted, on a value made to be written: [`Self::set`] of one of them shares the name.
    names: Option<EntityNames>,
    /// The entity's localized fields, computed once and shared. Absent when the entity has none.
    l10n: Option<Arc<field_rw::L10nTargets>>,
    /// Created on the first localized read. A `set` replaces it so a clone keeps the old memo.
    l10n_memo: L10nMemo,
    read: ReadState,
}

/// The field names of an entity, sorted, shared by the values made of it.
#[derive(Clone)]
struct EntityNames(Arc<[Arc<str>]>);

impl std::fmt::Debug for EntityNames {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "EntityNames({})", self.0.len())
    }
}

/// The localized reads of one value. Empty until the first [`EntityValue::get_localized`]. A clone shares them, and
/// reads the slot without a lock: a cache hit clones a value other threads clone too.
#[derive(Clone, Default)]
struct L10nMemo(OnceLock<Arc<Mutex<LocalizedReads>>>);

impl L10nMemo {
    fn get_or_init(&self) -> Arc<Mutex<LocalizedReads>> {
        Arc::clone(self.0.get_or_init(|| Arc::new(Mutex::new(HashMap::new()))))
    }

    /// Drop `field`'s remembered reads on this value only.
    fn replace_without(&mut self, field: &str) {
        let Some(memo) = self.0.get() else {
            return;
        };
        let kept: HashMap<_, _> = lock(memo)
            .iter()
            .filter(|((name, _), _)| *name != field)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect();
        self.0 = OnceLock::from(Arc::new(Mutex::new(kept)));
    }
}

impl std::fmt::Debug for L10nMemo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("L10nMemo")
    }
}

/// What an [`EntityValue`] read from the database held before the caller changed it. Behind a lock so a write, which
/// takes the value by reference, can mark the value written; a clone has a copy of its own. Read-write, so the clones
/// of one cached value on several threads do not wait for each other.
#[derive(Default)]
struct ReadState(RwLock<Read>);

#[derive(Clone, Debug, Default)]
struct Read {
    /// Read from the database, or written to it since.
    from_db: bool,
    /// Each field set since the value was read or written, with what it held then (`None`: it held nothing).
    before: BTreeMap<Arc<str>, Option<Value>>,
}

impl ReadState {
    fn read() -> Self {
        Self(RwLock::new(Read {
            from_db: true,
            before: BTreeMap::new(),
        }))
    }

    fn get(&self) -> RwLockReadGuard<'_, Read> {
        read(&self.0)
    }

    fn get_mut(&mut self) -> &mut Read {
        self.0.get_mut().unwrap_or_else(PoisonError::into_inner)
    }

    /// The value holds what the database does, as Java's `setSyncedWithDb` after a create or an update.
    fn synced(&self) {
        let mut read = write(&self.0);
        read.from_db = true;
        read.before.clear();
    }
}

impl Clone for ReadState {
    fn clone(&self) -> Self {
        Self(RwLock::new(self.get().clone()))
    }
}

impl std::fmt::Debug for ReadState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.get().fmt(f)
    }
}

/// Two values are equal when they hold the same stored fields of the same entity.
impl PartialEq for EntityValue {
    fn eq(&self, other: &Self) -> bool {
        self.entity_name == other.entity_name
            && (Arc::ptr_eq(&self.fields, &other.fields) || *self.fields == *other.fields)
    }
}

/// The localized fields of a value, and what each read in each locale, as Java's `localizedByLocaleByField`, and the
/// view's `pq-expression` aliases. A clone shares what was read until either one sets a field.
#[derive(Clone)]
struct Localizer {
    targets: Arc<field_rw::L10nTargets>,
    memo: Arc<Mutex<LocalizedReads>>,
}

/// What each localized read gave, keyed by field and locale.
type LocalizedReads = HashMap<(String, String), Option<Value>>;

impl std::fmt::Debug for Localizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Localizer")
            .field("targets", &self.targets)
            .finish_non_exhaustive()
    }
}

/// A localized read that needs a lookup, or a `pq-expression` alias to evaluate, taken from the value so a caller that
/// holds the value behind a lock can let go of it before the lookup or the script runs. See
/// [`EntityValue::read_localized`].
pub(crate) struct LocalizedRead {
    l10n: Localizer,
    field: String,
    locale: String,
    /// The stored fields the lookup reads: the field itself and the primary key. Every stored field for an expression.
    fields: FieldMap,
    /// The entity and the expression of a `pq-expression` alias. Boxed, so a read that needs no lookup stays small.
    pq: Option<Box<(String, String)>>,
}

impl LocalizedRead {
    pub(crate) fn run(self, facade: &EntityFacade) -> Option<Value> {
        // An expression is not remembered: as Java's `get`, it runs on every read, since it may read more than the row.
        if let Some((entity_name, expression)) = self.pq.as_deref() {
            return Some(eval_pq_alias(entity_name, &self.field, expression, &self.fields));
        }
        let found = field_rw::localize_field(facade, &self.l10n.targets, &self.fields, &self.field, &self.locale);
        let out = found.clone().or_else(|| self.fields.get(self.field.as_str()).cloned());
        lock(&self.l10n.memo).insert((self.field, self.locale), found);
        out
    }
}

/// What the `UPDATE` that [`EntityFacade::create_or_update_value`] tries first did.
enum Probe {
    Updated,
    /// It matched no row.
    Missing,
    /// It was not run.
    NotTried,
}

impl EntityValue {
    pub(crate) fn make(facade: &EntityFacade, entity_name: &str) -> Result<Self, Error> {
        let (entity_name, names) = facade.with_entity(entity_name, |entity, dialect, _| {
            if entity.is_view {
                return Ok((Arc::from(entity.full_name.as_str()), None));
            }
            let shape = super::sql_cache::read_shape(facade, entity, dialect);
            Ok((Arc::clone(&shape.full_name), Some(EntityNames(Arc::clone(&shape.names)))))
        })?;
        Ok(Self {
            entity_name,
            fields: Arc::new(BTreeMap::new()),
            names,
            l10n: None,
            l10n_memo: L10nMemo::default(),
            read: ReadState::default(),
        })
    }

    /// A value read from the database or the entity cache. The rows of one read share `entity_name`.
    pub(crate) fn from_fields(entity_name: Arc<str>, fields: FieldMap) -> Self {
        Self {
            entity_name,
            fields: Arc::new(fields),
            names: None,
            l10n: None,
            l10n_memo: L10nMemo::default(),
            read: ReadState::read(),
        }
    }

    /// The field map, moved out when this value is the only owner.
    pub(crate) fn into_fields(self) -> FieldMap {
        Arc::try_unwrap(self.fields).unwrap_or_else(|fields| (*fields).clone())
    }

    /// Whether `other` still shares this value's field map. A `set` or `refresh` on either one ends that.
    #[cfg(test)]
    pub(crate) fn shares_fields(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.fields, &other.fields)
    }

    /// Remember `targets` for a later localized read. The per-value memo waits until that read.
    pub(crate) fn with_l10n(mut self, targets: Option<&Arc<field_rw::L10nTargets>>) -> Self {
        self.l10n = targets.cloned();
        self.l10n_memo = L10nMemo::default();
        self
    }

    pub fn entity_name(&self) -> &str {
        &self.entity_name
    }

    /// The stored value of `field`.
    pub fn get(&self, field: &str) -> Option<&Value> {
        self.fields.get(field)
    }

    /// Java's `get`: `field` in the user's locale when it is a localized field, the value of its expression when it is a
    /// `pq-expression` alias of a view, else the stored value. A lookup is made once per field and locale; an expression
    /// runs on every read.
    pub fn get_localized(&self, facade: &EntityFacade, field: &str) -> Option<Value> {
        match self.read_localized(facade, field) {
            Ok(value) => value,
            Err(read) => read.run(facade),
        }
    }

    /// Whether `field` reads localized or is a `pq-expression` alias, so a caller can skip finding the facade for every
    /// other field.
    pub(crate) fn localizes(&self, field: &str) -> bool {
        self.l10n
            .as_deref()
            .is_some_and(|l10n| l10n.has(field) || l10n.pq_expression(field).is_some())
    }

    /// Whether `name` is a `pq-expression` alias of the value's view.
    pub(crate) fn is_pq_alias(&self, name: &str) -> bool {
        self.l10n
            .as_deref()
            .is_some_and(|l10n| l10n.pq_expression(name).is_some())
    }

    /// Whether any field reads localized, or the view has a `pq-expression` alias.
    pub(crate) fn has_l10n(&self) -> bool {
        self.l10n.is_some()
    }

    /// `field` as [`Self::get_localized`] reads it when that needs no lookup; otherwise the lookup or the expression to
    /// run.
    pub(crate) fn read_localized(&self, facade: &EntityFacade, field: &str) -> Result<Option<Value>, LocalizedRead> {
        let stored = self.fields.get(field).cloned();
        let Some(targets) = self.l10n.as_ref() else {
            return Ok(stored);
        };
        if field_rw::raw_read() || !self.localizes(field) {
            return Ok(stored);
        }
        let l10n = Localizer {
            targets: Arc::clone(targets),
            memo: self.l10n_memo.get_or_init(),
        };
        // As Java, the expression reads the stored fields, not the localized ones.
        if let Some(expression) = targets.pq_expression(field) {
            return Err(LocalizedRead {
                l10n,
                field: field.to_string(),
                locale: String::new(),
                fields: (*self.fields).clone(),
                pq: Some(Box::new((self.entity_name.to_string(), expression.to_string()))),
            });
        }
        let locale = field_rw::current_locale(facade);
        let key = (field.to_string(), locale);
        if let Some(known) = lock(&l10n.memo).get(&key) {
            return Ok(known.clone().or(stored));
        }
        let (field, locale) = key;
        let fields = l10n
            .targets
            .read_columns(&field)
            .filter_map(|name| self.fields.get_key_value(name).map(|(name, value)| (Arc::clone(name), value.clone())))
            .collect();
        Err(LocalizedRead {
            l10n,
            field,
            locale,
            fields,
            pq: None,
        })
    }

    /// Java's `getMap`: every field held, the localized ones in the user's locale, and a view's `pq-expression` aliases.
    pub fn localized_fields(&self, facade: &EntityFacade) -> FieldMap {
        let mut out = (*self.fields).clone();
        let Some(l10n) = self.l10n.as_deref() else {
            return out;
        };
        for column in l10n.columns().chain(l10n.pq_names()) {
            if let Some(value) = self.get_localized(facade, column) {
                out.insert(Arc::from(column), value);
            }
        }
        out
    }

    /// Set a field. On a value read from the database the first set of a field keeps what it held, as Java's `put`
    /// keeps it in `dbValueMap`, for [`Self::is_field_modified`].
    pub fn set(&mut self, field: impl IntoFieldName, value: Value) {
        // A name the value holds, or one of its entity's, is shared rather than copied.
        let shared = self
            .fields
            .get_key_value(field.name_str())
            .map(|(name, _)| Arc::clone(name))
            .or_else(|| {
                let names = &self.names.as_ref()?.0;
                let at = names.binary_search_by(|name| (**name).cmp(field.name_str())).ok()?;
                Some(Arc::clone(&names[at]))
            });
        let field = shared.unwrap_or_else(|| field.into_field_name());
        let read = self.read.get_mut();
        if read.from_db && !read.before.contains_key(&*field) {
            read.before.insert(field.clone(), self.fields.get(&*field).cloned());
        }
        self.l10n_memo.replace_without(&field);
        Arc::make_mut(&mut self.fields).insert(field, value);
    }

    pub fn fields(&self) -> &FieldMap {
        &self.fields
    }

    /// Java's `isFieldModified`: whether `field` changed, so an update writes it. On a value not read from the
    /// database, every field it holds. On one read from the database or written since, a field set to something
    /// other than what it held then (compared as an update compares, a number by value) or set when it held nothing.
    pub fn is_field_modified(&self, field: &str) -> bool {
        let Some(value) = self.fields.get(field) else {
            return false;
        };
        let read = self.read.get();
        if !read.from_db {
            return true;
        }
        match read.before.get(field) {
            None => false,
            Some(None) => true,
            Some(Some(old)) => !same_field_value(old, value),
        }
    }

    /// Java's `isModified`: whether any field changed (see [`Self::is_field_modified`]).
    pub fn is_modified(&self) -> bool {
        let read = self.read.get();
        if !read.from_db {
            return !self.fields.is_empty();
        }
        read.before
            .iter()
            .any(|(name, old)| match (old, self.fields.get(name)) {
                (_, None) => false,
                (None, Some(_)) => true,
                (Some(old), Some(value)) => !same_field_value(old, value),
            })
    }

    /// Java's `getOriginalDbValue`: what `field` held when the value was read from the database or last written, or
    /// what it holds when it was not set since.
    pub fn original_db_value(&self, field: &str) -> Option<Value> {
        match self.read.get().before.get(field) {
            Some(old) => old.clone(),
            None => self.fields.get(field).cloned(),
        }
    }

    /// What each field set since the value was read or written held then; `None` when the value was not read from
    /// the database, so that an update writes every field it holds.
    pub(crate) fn read_before(&self) -> Option<BTreeMap<Arc<str>, Option<Value>>> {
        let read = self.read.get();
        read.from_db.then(|| read.before.clone())
    }

    /// `written`, a copy of this value taken for a write, was written: what it held is what the database holds now. A
    /// field set on this value since the copy was taken (by an ECA rule during the write) stays modified.
    pub(crate) fn note_written(&mut self, written: &EntityValue) {
        let fields = &*self.fields;
        let read = self.read.get_mut();
        read.from_db = true;
        read.before.clear();
        for (name, value) in fields {
            let was = written.fields.get(name);
            if was != Some(value) {
                read.before.insert(name.clone(), was.cloned());
            }
        }
    }

    /// Insert the value. It then holds what the database does: a later update writes only the fields set after.
    pub fn create(&self, facade: &EntityFacade) -> Result<(), Error> {
        facade.create_value(self)
    }

    /// Java's `update`: write the fields that changed (see [`Self::is_field_modified`]), and nothing when none did.
    pub fn update(&self, facade: &EntityFacade) -> Result<(), Error> {
        facade.update_value(self)
    }

    pub fn delete(&self, facade: &EntityFacade) -> Result<(), Error> {
        facade.delete_value(self)
    }

    pub fn refresh(&mut self, facade: &EntityFacade) -> Result<(), Error> {
        let fresh = facade.refresh_fields(&self.entity_name, &self.fields)?;
        self.read = ReadState::read();
        self.fields = Arc::new(fresh);
        self.l10n_memo = L10nMemo::default();
        Ok(())
    }

    pub fn create_or_update(&self, facade: &EntityFacade) -> Result<(), Error> {
        facade.create_or_update_value(self)
    }

    /// Set the first primary key from the in-memory sequence bank.
    pub fn set_sequenced_id_primary(&mut self, facade: &EntityFacade) -> Result<(), Error> {
        let (field, id) = sequence::assign_primary(facade, &self.entity_name, &self.fields)?;
        self.set(field, Value::string(id));
        Ok(())
    }

    /// Set the last primary key to the next padded integer among rows with the other keys.
    pub fn set_sequenced_id_secondary(&mut self, facade: &EntityFacade) -> Result<(), Error> {
        let (field, id) = facade.next_secondary_id(&self.entity_name, &self.fields)?;
        self.set(field, Value::string(id));
        Ok(())
    }
}

/// A find result. Rust callers filter and sort it with the slice and iterator methods; the Java `EntityList` helpers
/// scripts call (`filterByDate`, `getPartialList`, `first`) are in the script runtime's list.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityList {
    values: Vec<EntityValue>,
}

impl EntityList {
    pub(crate) fn new(values: Vec<EntityValue>) -> Self {
        Self { values }
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<&EntityValue> {
        self.values.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &EntityValue> {
        self.values.iter()
    }

    pub fn values(&self) -> &[EntityValue] {
        &self.values
    }
    pub fn into_values(self) -> Vec<EntityValue> {
        self.values
    }
}

impl EntityFacade {
    /// Insert `value`, which then holds what the database does (see [`EntityValue::is_field_modified`]).
    pub fn create_value(&self, value: &EntityValue) -> Result<(), Error> {
        let mut stack = HashSet::new();
        self.insert_record(value, &mut stack)?;
        value.read.synced();
        Ok(())
    }

    /// Java's `update`: write the fields of `value` that changed since it was read or written, or every field it
    /// holds when it was not read; then it holds what the database does.
    pub fn update_value(&self, value: &EntityValue) -> Result<(), Error> {
        let before = value.read_before();
        self.write_update(&value.entity_name, &value.fields, None, before.as_ref())?;
        value.read.synced();
        Ok(())
    }

    pub fn delete_value(&self, value: &EntityValue) -> Result<(), Error> {
        self.write_delete(&value.entity_name, &value.fields).map(|_| ())
    }

    /// [`Self::delete_value`] with the number of rows the delete removed: 0 when the row was already gone, as when
    /// another transaction deleted it first. A delete the transaction cache holds counts as 1.
    pub(crate) fn delete_value_counted(&self, value: &EntityValue) -> Result<u64, Error> {
        self.write_delete(&value.entity_name, &value.fields)
    }

    /// Java's `createOrUpdate`: update the row when it is there, as [`Self::update_value`] (a value read from the
    /// database writes only the fields that changed), else create it with every field the value holds.
    pub fn create_or_update_value(&self, value: &EntityValue) -> Result<(), Error> {
        let before = value.read_before();
        // As Java, fill primary-key defaults first.
        let mut copy = value.clone();
        let defaults = self.apply_field_defaults(&copy.entity_name, &copy.fields)?;
        let keys = self.pk_map(&copy.entity_name, &defaults)?;
        Arc::make_mut(&mut copy.fields).extend(keys.clone());
        // Rust then tries one UPDATE by primary key, for a row that is probably there: an `update` hit when it
        // updates the row, the Rust-only `update-probe` when it matches none. Otherwise as Java: refresh a clone
        // before choosing the write.
        // Opt in to Java's read-first create-or-update; keep UPDATE-first by default.
        let probe = if moqui_lib::env::get_prop_or_env("create-or-update") == "true" {
            Probe::NotTried
        } else {
            self.update_if_exists(&copy.entity_name, &copy.fields, before.as_ref())?
        };
        match probe {
            Probe::Updated => {
                value.read.synced();
                return Ok(());
            }
            // The update just saw no row in this transaction, which is what a refresh would read now.
            Probe::Missing => {
                self.create_value(&copy)?;
                value.read.synced();
                return Ok(());
            }
            Probe::NotTried => {}
        }
        let stored = {
            // Java refreshes a throwaway clone without accessing its localized fields.
            let _raw = super::field_rw::RawReadGuard::enter();
            self.refresh_by_pk(&copy.entity_name, &keys)?
        };
        // The row just read is the one the update would read again, in the same transaction.
        match stored {
            Some(row) => self.write_update(
                &copy.entity_name,
                &copy.fields,
                Some(row.into_fields()),
                before.as_ref(),
            )?,
            None => self.create_value(&copy)?,
        }
        value.read.synced();
        Ok(())
    }

    pub(crate) fn insert_record(&self, value: &EntityValue, stack: &mut HashSet<String>) -> Result<(), Error> {
        if !stack.insert(value.entity_name.to_string()) {
            return Err(Error::new(format!("foreign key cycle at {}", value.entity_name)));
        }
        let result = self.insert_prepared(value, stack);
        stack.remove(value.entity_name.as_ref());
        result
    }

    fn insert_prepared(&self, value: &EntityValue, stack: &mut HashSet<String>) -> Result<(), Error> {
        let owned = if self.with_entity(&value.entity_name, |entity, _, _| Ok(has_field_defaults(entity)))? {
            self.apply_field_defaults(&value.entity_name, value.fields())?
        } else {
            value.fields().clone()
        };
        let plain = self.normalize_values(&value.entity_name, owned, false)?;
        self.require_pk(&value.entity_name, &plain)?;
        fk::enforce(self, &value.entity_name, &plain, stack)?;
        self.reject_view(&value.entity_name, "create")?;
        self.finish_write(&value.entity_name, "create", plain, None, None, None)
            .map(|_| ())
    }

    /// Fill the fields an entity definition gives a `default`, as Java's `checkSetFieldDefaults` on create.
    pub(crate) fn apply_field_defaults(
        &self,
        entity_name: &str,
        values: &FieldMap,
    ) -> Result<FieldMap, Error> {
        self.fill_field_defaults(entity_name, values, DefaultFields::All, None)
    }

    /// Java's `checkSetFieldDefaults`: each field of `which` with a `default` whose value is empty (null or `""`, as
    /// `ObjectUtilities.isEmpty`) takes the default's value. The value is the one in `values`, or, when `values` leaves
    /// the field out, the one in `stored`. The default is an expression, evaluated with the `stored` fields and then
    /// the `values` in the context; a null result leaves the field as it was.
    fn fill_field_defaults(
        &self,
        entity_name: &str,
        values: &FieldMap,
        which: DefaultFields,
        stored: Option<&FieldMap>,
    ) -> Result<FieldMap, Error> {
        let defaults = self.with_entity(entity_name, |entity, _, _| {
            Ok(entity
                .fields
                .iter()
                .filter(|field| match which {
                    DefaultFields::All => true,
                    DefaultFields::Pk => field.is_pk,
                    DefaultFields::NonPk => !field.is_pk,
                })
                .filter_map(|field| {
                    let default = field
                        .default_value
                        .as_deref()
                        .map(str::trim)
                        .filter(|d| !d.is_empty())?;
                    Some((field.name.clone(), FieldDefault::parse(entity, default)))
                })
                .collect::<Vec<_>>())
        })?;
        let mut out = values.clone();
        for (name, default) in defaults {
            let current = out.get(name.as_str()).or_else(|| stored.and_then(|row| row.get(name.as_str())));
            if current.is_some_and(|value| !moqui_lib::object::is_empty(value)) {
                continue;
            }
            let value = match default {
                FieldDefault::Literal(value) => Some(value),
                FieldDefault::Field(other) => out
                    .get(other.as_str())
                    .or_else(|| stored.and_then(|row| row.get(other.as_str())))
                    .cloned(),
                // the execution context's effective time, as the Groovy default expression read it
                FieldDefault::NowTimestamp => Some(crate::user::now_effective_value()),
                FieldDefault::UserId => crate::user::current_user_id().ok().flatten().map(Value::string),
                FieldDefault::Expression(expression) => {
                    Some(eval_field_default(&expression, &out, stored).map_err(|err| {
                        Error::new(format!(
                            "Error evaluating default [{expression}] of field {name} on entity {entity_name}: {err}"
                        ))
                    })?)
                }
            };
            if let Some(value) = value.filter(|value| !value.is_null()) {
                out.insert(Arc::from(name), value);
            }
        }
        Ok(out)
    }

    /// Java's `update`. The primary key defaults are filled first. When the entity needs the stored row (see
    /// [`update_reads_row`]) it is read, or taken from `known` when the caller just read it, and the update then writes
    /// only the fields that differ from it, and nothing when none do. See [`Self::update_columns`].
    ///
    /// `before` is what each field a value read from the database set since held then
    /// ([`EntityValue::read_before`]). With it the update writes only the fields that changed from that (and those a
    /// field default or a run-before ECA rule fills), so a field another transaction changed since the read is left as
    /// it wrote it; with no change it writes nothing and reads no row. With none (a value that was not read, or a
    /// changed primary key) it writes every field `values` holds.
    fn write_update(
        &self,
        entity_name: &str,
        values: &FieldMap,
        known: Option<FieldMap>,
        before: Option<&BTreeMap<Arc<str>, Option<Value>>>,
    ) -> Result<(), Error> {
        self.reject_view(entity_name, "update")?;
        let (has_defaults, reads_row) = self.with_entity(entity_name, |entity, _, _| {
            Ok((has_field_defaults(entity), update_reads_row(entity)))
        })?;
        let rules = load_feed::match_rules_with(self, entity_name, "update");
        // A value read and not set since has nothing to write, unless a default or a run-before rule sets a field.
        let untouched = before.is_some_and(BTreeMap::is_empty) && !has_defaults && rules.is_empty();
        let reads_row = !untouched && (reads_row || load_feed::feed_wants_old(self, entity_name));
        let mut values = if has_defaults {
            self.fill_field_defaults(entity_name, values, DefaultFields::Pk, None)?
        } else {
            values.clone()
        };
        let stored = if reads_row {
            match known {
                Some(row) => Some(row),
                None => {
                    let keys = self.pk_map(entity_name, &values)?;
                    let _raw = super::field_rw::RawReadGuard::enter();
                    self.refresh_by_pk(entity_name, &keys)?.map(|row| row.into_fields())
                }
            }
        } else {
            None
        };
        let unfilled = (has_defaults && before.is_some()).then(|| values.clone());
        if has_defaults {
            values = self.fill_field_defaults(entity_name, &values, DefaultFields::NonPk, stored.as_ref())?;
        }
        // Keep `values` when a default may have filled a field; otherwise move the map into the conversion.
        let mut plain = if has_defaults {
            self.normalize_values(entity_name, values.clone(), true)?
        } else {
            let owned = std::mem::take(&mut values);
            self.normalize_values(entity_name, owned, true)?
        };
        let mut modified = match before {
            Some(before) => {
                self.with_entity(entity_name, |entity, _, _| Ok(modified_fields(entity, before, &plain)))?
            }
            None => None,
        };
        // A field a default filled changed too, as Java's `checkSetFieldDefaults` puts it on the value.
        if let (Some(modified), Some(unfilled)) = (modified.as_mut(), unfilled) {
            for (name, value) in &values {
                if unfilled.get(name) != Some(value) {
                    modified.insert(name.clone());
                }
            }
        }
        let stored = match stored {
            // As Java's `setDbValueMap`, the value takes each stored field it does not set. A value read from the
            // database takes the stored value of each field it did not change too, so the ECA rules, the cache and the
            // data feed see the row as this update leaves it.
            Some(row) => {
                for (name, value) in &row {
                    plain.entry(name.clone()).or_insert_with(|| value.clone());
                }
                if let Some(modified) = &modified {
                    take_unmodified(&mut plain, &row, modified);
                }
                Some(row)
            }
            // As Java, a row that is not there leaves the value compared with itself: only what a run-before rule sets
            // is written.
            None if reads_row => Some(plain.clone()),
            None => None,
        };
        self.finish_write(entity_name, "update", plain, Some(rules), stored, modified.as_ref())
            .map(|_| ())
    }

    /// The columns an update writes, after its run-before ECA rules, as Java's `update`; `None` when it writes nothing.
    ///
    /// Only the fields in `modified` are written when it is given (a value read from the database: the fields that
    /// changed); every non-PK field of `plain` otherwise. With a `stored` row only those that differ from it are. A
    /// `lastUpdatedStamp` in `plain` is kept only when it differs from a stored non-null one, or with no stored row
    /// when `modified` has it (the caller changed a non-null stamp it read); otherwise it is set to the transaction
    /// start time. Nothing is written when no other field would be. A create-only field that would change is an
    /// error, as is, with optimistic locking, a `lastUpdatedStamp` that is not the stored one.
    fn update_columns(
        &self,
        entity_name: &str,
        stored: Option<&FieldMap>,
        plain: &mut FieldMap,
        modified: Option<&BTreeSet<Arc<str>>>,
    ) -> Result<Option<Vec<String>>, Error> {
        let (mut columns, kept_stamp, create_only, optimistic_lock, has_stamp) =
            self.with_entity(entity_name, |entity, _, _| {
                let mut columns = Vec::new();
                let mut kept_stamp = false;
                let mut create_only = Vec::new();
                for field in entity.fields.iter().filter(|field| !field.is_pk) {
                    let Some(value) = plain.get(field.name.as_str()) else {
                        continue;
                    };
                    if modified.is_some_and(|modified| !modified.contains(field.name.as_str())) {
                        continue;
                    }
                    let old = stored.map(|row| row.get(field.name.as_str()).unwrap_or(&Value::Null));
                    if field.name == STAMP {
                        match old {
                            Some(old) if !old.is_null() && !same_field_value(old, value) => kept_stamp = true,
                            None if modified.is_some() => kept_stamp = true,
                            _ => continue,
                        }
                    } else if old.is_some_and(|old| same_field_value(old, value)) {
                        continue;
                    }
                    columns.push(field.name.clone());
                    // As Java, the stamp counts here only when the caller changed it; the one the update sets does not.
                    if field.create_only {
                        create_only.push(field.name.clone());
                    }
                }
                Ok((
                    columns,
                    kept_stamp,
                    create_only,
                    entity.optimistic_lock,
                    entity.field(STAMP).is_some(),
                ))
            })?;
        if columns.is_empty() || (columns.len() == 1 && kept_stamp) {
            return Ok(None);
        }
        let full_name = || self.with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone()));
        if !create_only.is_empty() {
            return Err(Error::new(format!(
                "Cannot update create-only (immutable) fields [{}] on entity {}",
                create_only.join(", "),
                full_name()?
            )));
        }
        if optimistic_lock {
            let present = |value: Option<&Value>| value.filter(|value| !value.is_null()).cloned();
            if let (Some(mine), Some(theirs)) = (
                present(plain.get(STAMP)),
                present(stored.and_then(|row| row.get(STAMP))),
            ) {
                if !same_field_value(&mine, &theirs) {
                    return Err(Error::new(format!(
                        "This record was updated by someone else at {} which was after the version you loaded at {}. \
                         Not updating to avoid overwriting data.",
                        moqui_lib::object::to_plain_string(&theirs),
                        moqui_lib::object::to_plain_string(&mine)
                    )));
                }
            }
        }
        if has_stamp && !kept_stamp {
            plain.insert(Arc::from(STAMP), self.write_stamp(entity_name));
            columns.push(STAMP.to_string());
        }
        Ok(Some(columns))
    }

    /// The first step of [`Self::create_or_update_value`]: one `UPDATE` by primary key, for a row that is probably
    /// there. [`Probe::Updated`] when it updated a row, [`Probe::Missing`] when it matched none, so the row is created.
    /// [`Probe::NotTried`] when nothing was run and the caller must take the find-then-write path: this write is one
    /// where an update without a prior read could differ from the find-then-write path.
    ///
    /// Those are: an ECA rule matches `update` (it must not run for a row that turns out to be new), the update reads
    /// the stored row first (an audit-logged, create-only or defaulted field, optimistic locking, or a data feed
    /// document: see [`update_reads_row`]), the dialect does not report matched rows (only Postgres does; MySQL
    /// counts changed rows), a transaction cache or simulation is in use (a write there is held, not run),
    /// authorization is on (an update checks `update`, a new row checks `create`), or the update has no column to
    /// set. Foreign key checks belong to create only, and the path that follows a miss is the unchanged one, so they
    /// are not skipped. A probe that updates no row leaves no feed touch, cache clear, or ECA run behind.
    ///
    /// `before` is as for [`Self::write_update`]: a value read from the database sets only the fields that changed.
    fn update_if_exists(
        &self,
        entity_name: &str,
        values: &FieldMap,
        before: Option<&BTreeMap<Arc<str>, Option<Value>>>,
    ) -> Result<Probe, Error> {
        if !self.writes_unread(entity_name, |dialect| dialect.name == "postgres")? {
            return Ok(Probe::NotTried);
        }
        let plain = self.normalize_values(entity_name, values.clone(), true)?;
        // The columns as `update_columns` finds them with no stored row: every non-PK field (of those that changed, for
        // a value read from the database) but the stamp it sets.
        let (has_columns, modified) = self.with_entity(entity_name, |entity, _, _| {
            let modified = before.and_then(|before| modified_fields(entity, before, &plain));
            let has_columns = present_columns(entity, &plain, true, true)
                .into_iter()
                .any(|column| column != STAMP && modified.as_ref().is_none_or(|modified| modified.contains(column)));
            Ok((has_columns, modified))
        })?;
        if !has_columns {
            return Ok(Probe::NotTried);
        }
        let rules = load_feed::match_rules_with(self, entity_name, "update");
        if !rules.is_empty() {
            return Ok(Probe::NotTried);
        }
        let updated = self.finish_write(entity_name, "update-probe", plain, Some(rules), None, modified.as_ref())?;
        Ok(if updated > 0 { Probe::Updated } else { Probe::Missing })
    }

    fn write_delete(&self, entity_name: &str, values: &FieldMap) -> Result<u64, Error> {
        let plain = self.normalize_values(entity_name, values.clone(), false)?;
        self.reject_view(entity_name, "delete")?;
        self.with_entity(entity_name, |entity, _, _| {
            if entity.create_only {
                return Err(Error::new(format!(
                    "Entity {} is create-only (immutable), cannot be deleted.",
                    entity.full_name
                )));
            }
            Ok(())
        })?;
        self.finish_write(entity_name, "delete", plain, None, None, None)
    }

    /// Insert, or update on a primary-key conflict. Values are sealed the same way as a create.
    /// Whether a write of `entity_name` may go without reading the row first and give what the find-then-write path
    /// would: `dialect` takes it, the update would not read the stored row ([`update_reads_row`], or a data feed that
    /// wants the old values), no transaction cache or simulation holds the write, and authorization is off, so an
    /// update and a create need no separate check. See [`Self::update_if_exists`] and [`Self::upsert_fits`].
    fn writes_unread(&self, entity_name: &str, dialect: impl FnOnce(&moqui_db::dialect::Dialect) -> bool) -> Result<bool, Error> {
        let eligible = self.with_entity(entity_name, |entity, entity_dialect, _| {
            Ok(dialect(entity_dialect)
                && !entity.is_view
                && !update_reads_row(entity)
                && (!crate::user::context_entered()
                    || entity.authorize_skip.trim() == "true"
                    || crate::context::ExecutionContext::with_current(|ctx| ctx.authz_disabled()).unwrap_or(false)))
        })?;
        Ok(eligible
            && !self.sim_active()
            && !self.transactions_for(entity_name)?.transaction_cache_active()
            && !load_feed::feed_wants_old(self, entity_name))
    }

    /// Whether a data load may store `fields`, a record with its whole primary key, as one upsert
    /// ([`Self::upsert_record`]): a statement the load sends without waiting, where create-or-update waits for an UPDATE
    /// to say whether the row is there. It may when [`Self::writes_unread`] and no ECA rule matches a create or an
    /// update, so no rule depends on which one it was, and when the record sets no `statusId` outside the primary key
    /// (the entity-auto store checks a status change against the stored row). The row's other fields are left as they are, as a store leaves
    /// them; the hit is a create's.
    pub(crate) fn upsert_fits<K: std::borrow::Borrow<str> + Ord>(
        &self,
        entity_name: &str,
        fields: &BTreeMap<K, Value>,
    ) -> Result<bool, Error> {
        if !self.writes_unread(entity_name, moqui_db::sql::supports_upsert)? {
            return Ok(false);
        }
        let (pk_complete, status) = self.with_entity(entity_name, |entity, _, _| {
            let pk_complete = entity
                .pk_field_names()
                .iter()
                .all(|name| fields.get(*name).is_some_and(|value| !value.is_null()));
            // A `statusId` in the primary key names the row, so it cannot change.
            let status = entity.field("statusId").is_some_and(|field| !field.is_pk);
            Ok((pk_complete, status))
        })?;
        if !pk_complete || (status && fields.contains_key("statusId")) {
            return Ok(false);
        }
        Ok(load_feed::match_rules_with(self, entity_name, "create").is_empty()
            && load_feed::match_rules_with(self, entity_name, "update").is_empty())
    }

    pub(crate) fn upsert_record(&self, entity_name: &str, fields: FieldMap) -> Result<(), Error> {
        let plain = self.normalize_values(entity_name, fields, false)?;
        self.require_pk(entity_name, &plain)?;
        let mut stack = HashSet::new();
        fk::enforce(self, entity_name, &plain, &mut stack)?;
        self.reject_view(entity_name, "create")?;
        self.finish_write(entity_name, "upsert", plain, None, None, None)
            .map(|_| ())
    }

    /// Run the write, or leave it with the cache when the gate handles it. Values are plaintext. `rules` are the
    /// ECA rules a caller already matched for this write; `None` matches them here. `stored` is the row an update read
    /// first; an update returns 0 without writing when [`Self::update_columns`] finds nothing to write. `modified` is
    /// the fields an update of a value read from the database changed (see [`Self::write_update`]); a field a run-before
    /// rule sets is added to it. The ECA rules, the cache and the data feed get every field of `plain`; an update writes
    /// (and audits, and holds in a transaction cache) only its columns.
    fn finish_write(
        &self,
        entity_name: &str,
        operation: &str,
        plain: FieldMap,
        rules: Option<Vec<EcaRule>>,
        stored: Option<FieldMap>,
        modified: Option<&BTreeSet<Arc<str>>>,
    ) -> Result<u64, Error> {
        // `update-probe` is an update that leaves no trace when it matches no row; see `update_if_exists`. It is
        // authorized as an update. Its hit is an `update` when it updated a row, and the Rust-only `update-probe`
        // when it matched none.
        let probe = operation == "update-probe";
        let operation = if probe { "update" } else { operation };
        // Declared first, so it drops last: the authorization cache is invalidated after the write.
        let _note = WriteNote(entity_name);
        let sub_type = if probe {
            RustOnlyEntityHits::UPDATE_PROBE
        } else if operation == "upsert" {
            "create"
        } else {
            operation
        };
        let _artifact = EntityArtifact::push(self, entity_name, operation, sub_type)?;
        let tx = self.transactions_for(entity_name)?;
        if tx.lock_track_enabled() {
            if let Ok(keys) = self.with_entity(entity_name, |entity, _, _| pk_values(entity, &plain)) {
                let pk = keys
                    .iter()
                    .map(|(name, value)| format!("{name}={}", moqui_lib::object::to_plain_string(value)))
                    .collect::<Vec<_>>()
                    .join(",");
                tx.note_entity_lock(entity_name, &pk);
            }
        }
        // An upsert matches ECA rules as a create, whether or not the row existed. Phase 6 `store`
        // needs a read first to pick create or update.
        let seal_operation = if operation == "upsert" { "create" } else { operation };
        // Update already matched in `write_update`.
        let rules = match rules {
            Some(rules) => rules,
            None => load_feed::match_rules_with(self, entity_name, seal_operation),
        };
        let eca = crate::user::context_entered();
        // The rules of each phase, so a write with none copies no values for them.
        let eca_before = eca && rules.iter().any(|rule| rule.run_before);
        let eca_after = eca && rules.iter().any(|rule| !rule.run_before);
        let mut plain = plain;
        let mut modified = modified.cloned();
        if eca_before {
            let before = plain.clone();
            crate::service::run_matched_eca(self, entity_name, seal_operation, true, &rules, &mut plain, None)?;
            // A run-before rule may have set fields (`entityValue` or `set-results`): convert them as a write would.
            if plain != before {
                plain = self.normalize_values(entity_name, std::mem::take(&mut plain), operation == "update")?;
                // What it set is written, as Java's rule sets it on the value; the stamp stays the update's to set.
                if let Some(modified) = modified.as_mut() {
                    for (name, value) in &plain {
                        if &**name != STAMP && before.get(name).is_none_or(|was| !same_field_value(was, value)) {
                            modified.insert(name.clone());
                        }
                    }
                }
            }
        }
        let write_columns = if operation == "update" {
            match self.update_columns(entity_name, stored.as_ref(), &mut plain, modified.as_ref())? {
                Some(columns) => Some(columns),
                None => return Ok(0),
            }
        } else {
            None
        };
        // The cache, the transaction frame and the data feed share these, and a write with no column filter or
        // encrypted field binds from them too.
        let cache_values = Arc::new(plain);
        // As Java binds only the fields an update changes, only those and the key are sealed and written: a stored field
        // that could not be decrypted is not written back, and one another transaction changed since the value was read
        // is left as it wrote it.
        let plain: Cow<'_, FieldMap> = match &write_columns {
            Some(columns) => Cow::Owned(self.with_entity(entity_name, |entity, _, _| {
                Ok(cache_values
                    .iter()
                    .filter(|(name, _)| columns.iter().any(|column| column == &***name) || entity.field(name).is_some_and(|field| field.is_pk))
                    .map(|(name, value)| (name.clone(), value.clone()))
                    .collect::<BTreeMap<_, _>>())
            })?),
            None => Cow::Borrowed(&*cache_values),
        };
        let mut affected = 1;
        if cache::gate_write(self, entity_name, operation, &plain)? == WriteGate::Handled {
            load_feed::note_touch(self, entity_name, seal_operation, Some(&cache_values), None);
        } else {
            let mut after_values = eca_after.then(|| (*cache_values).clone());
            // An after rule with `get-original-value` on an update or delete gets the row as it was before the write.
            let pre_row = if eca
                && (operation == "update" || operation == "delete")
                && rules.iter().any(|rule| !rule.run_before && rule.get_original_value)
            {
                let keys = self.pk_map(entity_name, &plain)?;
                self.find_by_pk(entity_name, &keys)?
            } else {
                None
            };
            // As for the stored row an update reads (see `write_update`): the row as this update leaves it.
            if let (Some(values), Some(row), Some(modified)) = (after_values.as_mut(), &pre_row, &modified) {
                if operation == "update" {
                    take_unmodified(values, row.fields(), modified);
                }
            }
            // The data feed feeds an update only when a document field changes, so it needs the row as it was.
            let feed_old = if operation == "update" && !probe && load_feed::feed_wants_old(self, entity_name) {
                match (&stored, &pre_row) {
                    (Some(row), _) => Some(row.clone()),
                    (None, Some(row)) => Some(row.fields().clone()),
                    (None, None) => {
                        let keys = self.pk_map(entity_name, &plain)?;
                        let _raw = super::field_rw::RawReadGuard::enter();
                        self.find_by_pk(entity_name, &keys)?.map(|row| row.fields().clone())
                    }
                }
            } else {
                None
            };
            // A run-after rule on a delete sees the row as it was: read it now, before the row is gone.
            if let Some(values) = after_values.as_mut().filter(|_| operation == "delete") {
                if rules.iter().any(|rule| !rule.run_before && rule.get_entire_entity) {
                    if let Some(row) = self
                        .refresh_fields(entity_name, values)
                        .ok()
                        .filter(|row| !row.is_empty())
                    {
                        for (name, value) in row {
                            values.entry(name).or_insert(value);
                        }
                    }
                }
            }
            // Java's `checkTableRuntime` before each write, when the group adds missing tables at runtime.
            self.check_table_runtime(entity_name);
            let prepared = self.seal_values(entity_name, seal_operation, plain, stored.as_ref())?;
            let (sql, params, row_key) = self.with_group(entity_name, |entity, dialect, _, _| {
                // A null an update or upsert names is written, so the field is cleared.
                let columns = present_columns(
                    entity,
                    &prepared,
                    operation == "update" || operation == "upsert",
                    operation == "update",
                );
                let mut key = String::new();
                key.push_str(&entity.full_name);
                key.push('|');
                key.push_str(&dialect.name);
                key.push('|');
                key.push_str(operation);
                key.push('|');
                for column in &columns {
                    key.push_str(column);
                    key.push(',');
                }
                // Where-clause values follow primary-key name order, which is the order `pk_condition` writes them.
                let params = if operation == "delete" {
                    pk_params(entity, &prepared)?
                } else if operation == "update" {
                    let mut params = bind_columns(&columns, &prepared);
                    params.extend(pk_params(entity, &prepared)?);
                    params
                } else {
                    bind_columns(&columns, &prepared)
                };
                let sql = super::sql_cache::cached(self, &key, || {
                    if operation == "delete" {
                        let condition = pk_condition(entity, &prepared)?;
                        Ok(delete(entity, dialect, Some(&condition))?.sql)
                    } else if operation == "update" {
                        let condition = pk_condition(entity, &prepared)?;
                        Ok(update(entity, dialect, &columns, Some(&condition))?.sql)
                    } else if operation == "upsert" {
                        insert_or_update(entity, dialect, &columns)
                    } else {
                        insert(entity, dialect, &columns)
                    }
                })?;
                let row_key = if matches!(operation, "create" | "upsert") {
                    pk_key(entity, &prepared)
                } else {
                    0
                };
                Ok((sql, params, row_key))
            })?;
            // A data load sends its writes without waiting; an update that then finds no row fails the file later. Its
            // creates and upserts may go out several rows to a statement.
            affected = if !probe && operation != "delete" && pipelining(&tx) {
                if operation == "update" {
                    let zero_rows = Some(self.missing_update(entity_name, &cache_values));
                    execute_deferred_tx(&tx, &sql, params, zero_rows)?;
                } else {
                    execute_deferred_row_tx(&tx, &sql, params, row_key)?;
                }
                1
            } else {
                execute_tx(&tx, &sql, &params)?
            };
            if probe {
                if affected == 0 {
                    return Ok(0);
                }
                crate::context::ExecutionContext::with_current(|ctx| ctx.set_hit_detail(Some("update"), None));
            } else if operation == "update" && affected == 0 {
                return Err(Error::new(self.missing_update(entity_name, &cache_values)));
            }
            load_feed::note_touch(self, entity_name, seal_operation, Some(&cache_values), feed_old);
            if let Some(mut values) = after_values {
                crate::service::run_matched_eca(
                    self,
                    entity_name,
                    seal_operation,
                    false,
                    &rules,
                    &mut values,
                    pre_row.as_ref(),
                )?;
            }
        }
        // As Java's `EntityCache`, a cache clear or cluster publish that fails is logged; the write already ran.
        if let Err(err) = cache::on_write_shared(self, entity_name, operation, &cache_values) {
            tracing::error!(error = %err, "Could not clear or publish the cache for {operation} of {entity_name}");
        }
        after_statement(self, &tx, true);
        Ok(affected)
    }

    /// As Java's `updateExtended`: an update of a row that is not there is an error, not a silent no-op.
    fn missing_update(&self, entity_name: &str, values: &FieldMap) -> String {
        let keys = self
            .with_entity(entity_name, |entity, _, _| pk_values(entity, values))
            .unwrap_or_default()
            .iter()
            .map(|(name, value)| format!("{name}={}", moqui_lib::object::to_plain_string(value)))
            .collect::<Vec<_>>()
            .join(", ");
        format!("Tried to update a value that does not exist [{entity_name}: {keys}]")
    }

    pub(crate) fn refresh_fields(
        &self,
        entity_name: &str,
        values: &FieldMap,
    ) -> Result<FieldMap, Error> {
        let keys = self.pk_map(entity_name, values)?;
        self.refresh_by_pk(entity_name, &keys)?
            .map(|value| value.into_fields())
            .ok_or_else(|| Error::new(format!("refresh found no {entity_name}")))
    }

    fn refresh_by_pk(&self, entity_name: &str, keys: &FieldMap) -> Result<Option<EntityValue>, Error> {
        // Refresh bypasses the shared entity cache but still sees transaction-held rows.
        let mut find = self.find(entity_name)?.use_cache(false);
        for (field, value) in keys {
            find = find.eq(field, value.clone());
        }
        find.one_internal("refresh")
    }

    pub(crate) fn find_by_pk(
        &self,
        entity_name: &str,
        keys: &FieldMap,
    ) -> Result<Option<EntityValue>, Error> {
        let mut find = self.find(entity_name)?;
        for (field, value) in keys {
            find = find.eq(field, value.clone());
        }
        find.one()
    }

    pub(crate) fn pk_map(
        &self,
        entity_name: &str,
        values: &FieldMap,
    ) -> Result<FieldMap, Error> {
        self.with_entity(entity_name, |entity, _, _| pk_values(entity, values))
    }

    fn require_pk(&self, entity_name: &str, values: &FieldMap) -> Result<(), Error> {
        self.pk_map(entity_name, values).map(|_| ())
    }

    pub(crate) fn next_secondary_id(
        &self,
        entity_name: &str,
        values: &FieldMap,
    ) -> Result<(String, String), Error> {
        let (seq_field, pad, keys) = self.with_entity(entity_name, |entity, _, _| {
            let pks = entity.pk_field_names();
            if pks.len() < 2 {
                return Err(Error::new(format!(
                    "cannot call set_sequenced_id_secondary on {}, it needs at least 2 primary key fields",
                    entity.full_name
                )));
            }
            let seq_field = pks[pks.len() - 1].to_string();
            let mut keys = Vec::new();
            for pk in &pks[..pks.len() - 1] {
                let value = values
                    .get(*pk)
                    .filter(|value| !value.is_null())
                    .cloned()
                    .ok_or_else(|| Error::new(format!("primary key {pk} is required on {}", entity.full_name)))?;
                keys.push(((*pk).to_string(), value));
            }
            Ok((seq_field, entity.sequence_secondary_padded_length, keys))
        })?;
        let mut find = self.find(entity_name)?;
        for (field, value) in keys {
            find = find.eq(&field, value);
        }
        let rows = find.list()?;
        let mut highest: i64 = 0;
        let mut found = false;
        for row in rows.iter() {
            let Some(value) = row.get(&seq_field) else {
                continue;
            };
            if let Value::I64(number) = basic_convert(value, "Long").value {
                found = true;
                highest = highest.max(number);
            }
        }
        let next = if found { highest.saturating_add(1) } else { 1 };
        let width = usize::try_from(pad).unwrap_or(2);
        Ok((seq_field, format!("{next:0width$}")))
    }

    /// Each field converted to its type. A create sets a missing `lastUpdatedStamp`; an update (`update`) leaves the
    /// stamp to [`Self::update_columns`].
    pub(crate) fn normalize_values(
        &self,
        entity_name: &str,
        mut values: FieldMap,
        update: bool,
    ) -> Result<FieldMap, Error> {
        self.with_entity(entity_name, |entity, _, _| {
            // Converted in place: the map is usually all fields of the entity, so it is not built again.
            let mut known = 0;
            for field in &entity.fields {
                let Some(value) = values.get_mut(field.name.as_str()) else {
                    continue;
                };
                known += 1;
                let java = field.java_type.as_str();
                // Text is a script's or a caller's, so a timestamp without an offset is in the session time zone.
                if !moqui_lib::object::is_java_type(value, java) {
                    *value = crate::l10n::basic_convert(value, java).value;
                }
            }
            if known != values.len() {
                values.retain(|name, _| entity.field(name).is_some());
            }
            if !update && entity.field(STAMP).is_some() && values.get(STAMP).is_none_or(Value::is_null) {
                values.insert(Arc::from(STAMP), self.write_stamp(entity_name));
            }
            Ok(values)
        })
    }

    /// `stored` is the row an update read first, which its audit log compares with.
    fn seal_values<'a>(
        &self,
        entity_name: &str,
        operation: &str,
        values: Cow<'a, FieldMap>,
        stored: Option<&FieldMap>,
    ) -> Result<Cow<'a, FieldMap>, Error> {
        let facade = self.clone();
        self.with_entity(entity_name, |entity, _, _| {
            field_rw::before_write_shared(&facade, entity, operation, values, stored)
        })
    }

    /// Java's `lastUpdatedStamp`: when the transaction in place began, so every row one transaction writes has the
    /// same stamp, else the wall clock.
    fn write_stamp(&self, entity_name: &str) -> Value {
        let began = self
            .transactions_for(entity_name)
            .ok()
            .and_then(|tx| tx.start_time_millis())
            .filter(|millis| *millis > 0);
        let millis = began.unwrap_or_else(|| {
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|duration| i64::try_from(duration.as_millis()).ok())
                .unwrap_or(0)
        });
        basic_convert(&Value::I64(millis), "java.sql.Timestamp").value
    }

    fn reject_view(&self, entity_name: &str, verb: &str) -> Result<(), Error> {
        self.with_entity(entity_name, |entity, _, _| {
            if entity.is_view {
                Err(Error::new(format!("cannot {verb} view-entity {}", entity.full_name)))
            } else {
                Ok(())
            }
        })
    }
}

fn pk_values(entity: &EntityDefinition, values: &FieldMap) -> Result<FieldMap, Error> {
    let mut keys = BTreeMap::new();
    for pk in entity.pk_field_names() {
        let (name, value) = values
            .get_key_value(pk)
            .filter(|(_, value)| !value.is_null())
            .ok_or_else(|| Error::new(format!("primary key {pk} is required on {}", entity.full_name)))?;
        keys.insert(Arc::clone(name), value.clone());
    }
    if keys.is_empty() {
        return Err(Error::new(format!("{} has no primary key", entity.full_name)));
    }
    Ok(keys)
}

/// Primary-key values in the order [`pk_condition`] binds them.
/// A hash of the primary key of `values`, which names the row to [`crate::driver::DbConnection::execute_deferred_row`].
/// Two keys that hash the same only send their rows in two statements.
fn pk_key(entity: &EntityDefinition, values: &FieldMap) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    // A write is never to a view, so the key is the entity's own primary key fields.
    for field in entity.fields.iter().filter(|field| field.is_pk) {
        match values.get(field.name.as_str()) {
            Some(Value::String(text)) => text.hash(&mut hasher),
            Some(Value::I64(number)) => number.hash(&mut hasher),
            Some(Value::I32(number)) => i64::from(*number).hash(&mut hasher),
            Some(Value::Null) | None => 0u8.hash(&mut hasher),
            Some(other) => moqui_lib::object::to_plain_string(other).hash(&mut hasher),
        }
    }
    hasher.finish()
}

fn pk_params(entity: &EntityDefinition, values: &FieldMap) -> Result<Vec<Value>, Error> {
    Ok(pk_values(entity, values)?.into_values().collect())
}

fn pk_condition(entity: &EntityDefinition, values: &FieldMap) -> Result<Condition, Error> {
    let keys = pk_values(entity, values)?;
    let mut parts: Vec<Condition> = keys
        .into_iter()
        .map(|(field, value)| Condition::equals(&*field, value))
        .collect();
    if parts.len() == 1 {
        return Ok(parts.remove(0));
    }
    Ok(Condition::and(parts))
}

fn present_columns<'a>(
    entity: &'a EntityDefinition,
    values: &FieldMap,
    include_null: bool,
    skip_pk: bool,
) -> Vec<&'a str> {
    entity
        .fields
        .iter()
        .filter(|field| {
            if skip_pk && field.is_pk {
                return false;
            }
            matches!(values.get(field.name.as_str()), Some(value) if include_null || !value.is_null())
        })
        .map(|field| field.name.as_str())
        .collect()
}

fn bind_columns(columns: &[&str], values: &FieldMap) -> Vec<Value> {
    columns
        .iter()
        .map(|column| values.get(*column).cloned().unwrap_or(Value::Null))
        .collect()
}

/// Tells the authorization cache an entity changed, once the write is over.
struct WriteNote<'a>(&'a str);

impl Drop for WriteNote<'_> {
    fn drop(&mut self) {
        crate::user::note_entity_write(self.0);
    }
}

thread_local! {
    static RUST_ONLY_SUB_TYPE: std::cell::Cell<Option<&'static str>> = const { std::cell::Cell::new(None) };
}

/// While held, entity operations on this thread are counted as artifact hits under `sub_type` instead of Java's find
/// method or write operation. For reads Java does not make: FK existence checks (Java leaves those to the database)
/// and the load report counts. They are still pushed, authorized and counted, but in bins of their own, so a
/// comparison with Java can set them apart. The previous sub type is restored on drop.
#[must_use = "the sub type is restored when the guard drops"]
pub struct RustOnlyEntityHits(Option<&'static str>);

impl RustOnlyEntityHits {
    /// FK existence reads before a write.
    pub const FK_CHECK: &'static str = "fk-check";
    /// The record counts the startup load reports.
    pub const LOAD_REPORT: &'static str = "load-report";
    /// The UPDATE create-or-update tries before it reads the row, when it matched no row (Java reads first).
    pub const UPDATE_PROBE: &'static str = "update-probe";

    pub fn enter(sub_type: &'static str) -> Self {
        Self(RUST_ONLY_SUB_TYPE.with(|current| current.replace(Some(sub_type))))
    }
}

impl Drop for RustOnlyEntityHits {
    fn drop(&mut self) {
        RUST_ONLY_SUB_TYPE.with(|current| current.set(self.0));
    }
}

fn rust_only_sub_type() -> Option<&'static str> {
    RUST_ONLY_SUB_TYPE.with(std::cell::Cell::get)
}

/// Whether `sub_type` is one [`RustOnlyEntityHits`] counts under.
pub(crate) fn is_rust_only_sub_type(sub_type: Option<&str>) -> bool {
    matches!(
        sub_type,
        Some(RustOnlyEntityHits::FK_CHECK | RustOnlyEntityHits::LOAD_REPORT | RustOnlyEntityHits::UPDATE_PROBE)
    )
}

/// An `AT_ENTITY` frame on the artifact stack for one entity operation.
///
/// The frame is decided against the one below it, so a service or screen that was allowed to do
/// this inherits the entity access. An entity whose `authorize-skip` covers the operation is
/// pushed without needing an allow.
pub(crate) struct EntityArtifact {
    pushed: bool,
}

impl EntityArtifact {
    /// `sub_type` is the hit sub-type, Java's find method or write operation: `list`, `count`, `create`.
    pub(crate) fn push(
        facade: &EntityFacade,
        entity_name: &str,
        operation: &str,
        sub_type: &str,
    ) -> Result<Self, Error> {
        if !crate::user::context_entered() {
            return Ok(Self { pushed: false });
        }
        let action = if operation == "upsert" { "create" } else { operation };
        let skip = facade
            .with_entity(entity_name, |entity, _, _| Ok(entity.authorize_skip.clone()))
            .unwrap_or_default();
        let requires_authz = !skips_authz(&skip, action);
        let outcome = crate::context::ExecutionContext::with_current(|ctx| {
            ctx.push_checked("AT_ENTITY", entity_name, action, requires_authz, false, None)?;
            ctx.set_hit_detail(Some(rust_only_sub_type().unwrap_or(sub_type)), None);
            Ok::<(), crate::context::Error>(())
        });
        match outcome {
            Some(Ok(())) => Ok(Self { pushed: true }),
            Some(Err(err)) => Err(Error::new(err.message().to_string()).with_source(err)),
            None => Ok(Self { pushed: false }),
        }
    }
}

/// `authorize-skip` is `true`, or a list that can name `create` and `view`. Update and delete
/// are skipped only by `true`.
fn skips_authz(skip: &str, action: &str) -> bool {
    match skip {
        "" | "false" => false,
        "true" => true,
        list => matches!(action, "create" | "view") && list.split(',').any(|part| part.trim() == action),
    }
}

impl Drop for EntityArtifact {
    fn drop(&mut self) {
        if !self.pushed {
            return;
        }
        crate::context::ExecutionContext::with_current(|ctx| {
            let _ = ctx.pop();
        });
    }
}

const STAMP: &str = "lastUpdatedStamp";

/// Which field defaults to fill, as the `pks` argument of Java's `checkSetFieldDefaults`.
#[derive(Clone, Copy)]
enum DefaultFields {
    All,
    Pk,
    NonPk,
}

/// A field's `default`, read once for the forms that need no script: a quoted string with nothing to expand, a number,
/// another field of the value, or `ec.user.nowTimestamp` or `ec.user.userId`, which also work with no context entered.
enum FieldDefault {
    Literal(Value),
    Field(String),
    NowTimestamp,
    UserId,
    Expression(String),
}

impl FieldDefault {
    fn parse(entity: &EntityDefinition, default: &str) -> Self {
        let bytes = default.as_bytes();
        let quote = bytes[0];
        if bytes.len() >= 2 && (quote == b'\'' || quote == b'"') && bytes[bytes.len() - 1] == quote {
            let inner = &default[1..default.len() - 1];
            if !inner.contains(['\'', '"', '\\', '$']) {
                return Self::Literal(Value::string(inner));
            }
        }
        if (quote.is_ascii_digit() || quote == b'-') && default.parse::<f64>().is_ok() {
            return Self::Literal(Value::string(default));
        }
        match default {
            "ec.user.nowTimestamp" => Self::NowTimestamp,
            "ec.user.userId" => Self::UserId,
            name if entity.field(name).is_some() => Self::Field(name.to_string()),
            _ => Self::Expression(default.to_string()),
        }
    }
}

/// A default expression, run with the stored fields and then the value's fields in the context, as Java pushes the
/// `dbValueMap` and then the value map.
fn eval_field_default(
    expression: &str,
    values: &FieldMap,
    stored: Option<&FieldMap>,
) -> Result<Value, Error> {
    let _frame = crate::script::ContextFrame::push();
    for (name, value) in stored.into_iter().flatten().chain(values) {
        crate::script::set_context_name(name, value.clone())?;
    }
    Ok(crate::script::eval_value(expression)?)
}

/// A `pq-expression` alias's value, as Java's `get` evaluates it with the row's stored fields in the context. A failing
/// expression is logged and reads as null, where Java throws: as for other independent pieces (AGENTS.md failure
/// tolerance), one bad expression does not fail the read of a row, its map, or a form-list of such rows.
fn eval_pq_alias(entity_name: &str, alias: &str, expression: &str, fields: &FieldMap) -> Value {
    let evaluated = (|| {
        let _frame = crate::script::ContextFrame::push();
        for (name, value) in fields {
            crate::script::set_context_name(name, value.clone())?;
        }
        Ok::<_, Error>(crate::script::eval_value(expression)?)
    })();
    evaluated.unwrap_or_else(|err| {
        tracing::error!(
            error = %err,
            "Error evaluating pq-expression [{expression}] of alias {alias} on entity {entity_name}, reading null"
        );
        Value::Null
    })
}

fn has_field_defaults(entity: &EntityDefinition) -> bool {
    entity
        .fields
        .iter()
        .any(|field| field.default_value.as_deref().is_some_and(|d| !d.trim().is_empty()))
}

/// Whether an update reads the stored row first, as Java's `update` refreshes for `needsAuditLog`, `createOnlyAny`,
/// `optimisticLock` and `hasFieldDefaults`. Java's fifth reason, a data feed document on the entity, is
/// [`load_feed::feed_wants_old`].
fn update_reads_row(entity: &EntityDefinition) -> bool {
    entity.create_only
        || entity.optimistic_lock
        || has_field_defaults(entity)
        || entity.fields.iter().any(|field| {
            field.create_only
                || field
                    .enable_audit_log
                    .as_deref()
                    .is_some_and(|flag| matches!(flag, "true" | "update"))
        })
}

/// The fields of `plain`, an update's values converted to their types, that changed from what a value read from the
/// database held (`before`, see [`EntityValue::read_before`]): each one set to something else, or set when it held
/// nothing. The stamp only when it read a non-null one, as Java counts `lastUpdatedStamp` against `dbValueMap`. `None`
/// when a primary key field changed: the value names another row, so each field it holds is written.
fn modified_fields(
    entity: &EntityDefinition,
    before: &BTreeMap<Arc<str>, Option<Value>>,
    plain: &FieldMap,
) -> Option<BTreeSet<Arc<str>>> {
    let mut names = BTreeSet::new();
    for (name, old) in before {
        let (Some(field), Some(value)) = (entity.field(name), plain.get(name)) else {
            continue;
        };
        if old.as_ref().is_some_and(|old| same_field_value(old, value)) {
            continue;
        }
        if field.is_pk {
            return None;
        }
        if &**name == STAMP && old.as_ref().is_none_or(Value::is_null) {
            continue;
        }
        names.insert(name.clone());
    }
    Some(names)
}

/// Each field of `row`, a stored row just read, that `modified` does not name into `values`, the fields of an update of a
/// value read from the database: such a value holds a field it did not change as it read it, and another transaction
/// may have written it since. The stamp stays the value's, for the optimistic lock check.
fn take_unmodified(values: &mut FieldMap, row: &FieldMap, modified: &BTreeSet<Arc<str>>) {
    for (name, value) in row {
        if &**name != STAMP && !modified.contains(name) {
            values.insert(name.clone(), value.clone());
        }
    }
}

/// Whether an update leaves a field as stored: both null, or the same value (a number by value, so `10.0` and
/// `10.000000` are the same).
fn same_field_value(old: &Value, new: &Value) -> bool {
    match (old.is_null(), new.is_null()) {
        (true, true) => true,
        (false, false) => {
            old == new || moqui_lib::object::to_plain_string(old) == moqui_lib::object::to_plain_string(new)
        }
        _ => false,
    }
}
