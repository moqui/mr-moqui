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

//! One entity facade. One [`TransactionManager`] per entity group.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

use moqui_db::dialect::{Dialect, Dialects};
use moqui_db::model::{EntityDefinition, EntityModel};
use moqui_lib::error::Error;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;
use moqui_lib::sync::{lock, read, write};

use super::crypto::CryptConfig;
use super::find::EntityFind;
use super::load_feed::EcaIndex;
use super::query_rest;
use super::sequence::SequenceBanks;
use super::share::SharedModel;
use super::value::EntityValue;
use crate::transaction::TransactionManager;

/// Flags stored for later hooks. `disable_fk_create` and `dummy_fks` are enforced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntityFlags {
    pub disable_entity_eca: bool,
    pub disable_audit_log: bool,
    pub disable_fk_create: bool,
    pub disable_data_feed: bool,
    pub dummy_fks: bool,
}

/// One entity group: a dialect name and the transaction manager the host registered.
pub struct EntityGroup {
    name: String,
    dialect_name: String,
    transactions: TransactionManager,
}

impl EntityGroup {
    pub fn new(name: impl Into<String>, dialect_name: impl Into<String>, transactions: TransactionManager) -> Self {
        Self {
            name: name.into(),
            dialect_name: dialect_name.into(),
            transactions,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn dialect_name(&self) -> &str {
        &self.dialect_name
    }

    pub fn transactions(&self) -> &TransactionManager {
        &self.transactions
    }
}

pub(crate) struct GroupRuntime {
    pub(crate) dialect: Dialect,
    pub(crate) transactions: TransactionManager,
}

pub(crate) struct Inner {
    pub(crate) model: SharedModel,
    pub(crate) framework_dir: std::path::PathBuf,
    /// The `entity` directories of the components, after the framework's: definitions and `*.eecas.xml`.
    pub(crate) component_entity_dirs: Vec<std::path::PathBuf>,
    pub(crate) default_group: String,
    pub(crate) groups: HashMap<String, GroupRuntime>,
    pub(crate) banks: SequenceBanks,
    /// `entity-facade` `sequenced-id-prefix`, in front of every id a sequence bank issues.
    pub(crate) sequenced_id_prefix: Option<String>,
    /// Each `<datasource>` group and its `sequence-primary-use-uuid`.
    pub(crate) datasource_uuid: HashMap<String, bool>,
    /// `<load-entity location>` files of the conf that are not in an `entity` directory; their entities are in the
    /// model and their `*.eecas.xml` rules with the directories'.
    pub(crate) entity_files: Vec<std::path::PathBuf>,
    /// See [`super::cache::screen_config_generation`].
    pub(crate) screen_config_generation: AtomicU64,
    /// See [`super::cache::feed_definition_generation`].
    pub(crate) feed_definition_generation: AtomicU64,
    /// Entity ECA rules, read from the `*.eecas.xml` files on first use. See `load_feed::match_rules_with`.
    pub(crate) eca_rules: RwLock<Option<Arc<EcaIndex>>>,
    pub(crate) flags: RwLock<EntityFlags>,
    pub(crate) db_ready: AtomicBool,
    locale: RwLock<String>,
    crypt: RwLock<Option<(CryptConfig, Vec<CryptConfig>)>>,
    sim_tx: RwLock<Option<TransactionManager>>,
    /// Starts the overlay cluster on the first `enter_sim`. The mutex also serializes that start.
    sim_starter: Mutex<Option<SimStarter>>,
    /// Embedded-database backup. The host installs it; core does not spawn `pg_dump`.
    backup_runner: Mutex<Option<crate::backup::BackupRunner>>,
    /// Read for a lookup ([`EntityFacade::with_slot_read`]), written to change a slot; finds on several threads read
    /// the same slots several times a call, which one mutex made them take in turn.
    slots: RwLock<HashMap<TypeId, Box<dyn Any + Send + Sync>>>,
    /// What is known of the tables, and the groups' DDL settings.
    pub(crate) tables: super::meta::TableState,
    /// Definitions built at runtime, consulted when the file model has no such name. See [`super::derived`].
    pub(crate) derived: super::derived::DerivedDefinitions,
}

/// Starts the sim overlay and answers its pool. See [`EntityFacade::set_sim_starter`].
pub type SimStarter = Box<dyn Fn() -> Result<TransactionManager, Error> + Send + Sync>;

/// Entity facade shared by the factory. Clone shares the model, banks, and pools.
#[derive(Clone)]
pub struct EntityFacade {
    pub(crate) inner: Arc<Inner>,
}

impl EntityFacade {
    /// Open the framework model over one driver. Every name in `groups` shares one transaction
    /// manager; the first is the default group. Reads `conf/MoquiDefaultConf.xml` for the
    /// transaction and field-encryption settings.
    ///
    /// The server opens `transactional` alone; the browser puts `logging` on the same database.
    ///
    /// # Errors
    ///
    /// Returns an error when the conf or the entity model cannot be read, `dialect` is not
    /// defined, or `groups` is empty.
    pub fn open_with_driver(
        framework_dir: &Path,
        driver: Arc<dyn crate::driver::DbDriver>,
        pool: crate::transaction::PoolConfig,
        dialect: &str,
        groups: &[&str],
    ) -> Result<Self, Error> {
        Self::open_with_driver_components(framework_dir, &[], driver, pool, dialect, groups)
    }

    /// [`Self::open_with_driver`] with the `entity` directories of the components: their entities,
    /// `extend-entity` elements, and entity ECA rules join the framework's.
    ///
    /// # Errors
    ///
    /// Returns an error when the conf or the entity model cannot be read, `dialect` is not
    /// defined, or `groups` is empty.
    pub fn open_with_driver_components(
        framework_dir: &Path,
        component_entity_dirs: &[std::path::PathBuf],
        driver: Arc<dyn crate::driver::DbDriver>,
        pool: crate::transaction::PoolConfig,
        dialect: &str,
        groups: &[&str],
    ) -> Result<Self, Error> {
        let Some(default_group) = groups.first() else {
            return Err(Error::new("open_with_driver needs at least one entity group"));
        };
        let transactions = TransactionManager::new(driver, pool)?;
        let conf = MNode::parse_file(framework_dir.join("conf/MoquiDefaultConf.xml"))?;
        transactions.configure_from_conf(&conf)?;
        let groups = groups
            .iter()
            .map(|group| EntityGroup::new(*group, dialect, transactions.clone()))
            .collect();
        let facade = Self::open_components(framework_dir, component_entity_dirs, default_group, groups)?;
        facade.crypt_from_conf(&conf)?;
        Ok(facade)
    }

    /// Load the framework entity model and attach one transaction manager per group.
    pub fn open(framework_dir: &Path, default_group: &str, groups: Vec<EntityGroup>) -> Result<Self, Error> {
        Self::open_components(framework_dir, &[], default_group, groups)
    }

    /// [`Self::open`] with the `entity` directories of the components after the framework's.
    pub fn open_components(
        framework_dir: &Path,
        component_entity_dirs: &[std::path::PathBuf],
        default_group: &str,
        groups: Vec<EntityGroup>,
    ) -> Result<Self, Error> {
        let conf = MNode::parse_file(framework_dir.join("conf/MoquiDefaultConf.xml"))?;
        Self::open_components_with_conf(framework_dir, component_entity_dirs, default_group, groups, &conf)
    }

    /// Load with the runtime's merged configuration, including database name replacements
    /// and the Rust SQL naming compatibility switch.
    pub fn open_components_with_conf(
        framework_dir: &Path,
        component_entity_dirs: &[std::path::PathBuf],
        default_group: &str,
        groups: Vec<EntityGroup>,
        conf: &MNode,
    ) -> Result<Self, Error> {
        if groups.is_empty() {
            return Err(Error::new("entity facade needs a transaction manager"));
        }
        let started = std::time::Instant::now();
        let framework_entities = framework_dir.join("entity");
        let entity_files = load_entity_files(conf, framework_dir, component_entity_dirs);
        // As Java: the conf's `load-entity` files, then the components' `entity` directories. The framework's own
        // `load-entity` files are its directory, read first.
        let mut directories: Vec<&Path> = vec![framework_entities.as_path()];
        directories.extend(entity_files.iter().map(std::path::PathBuf::as_path));
        directories.extend(component_entity_dirs.iter().map(std::path::PathBuf::as_path));
        tracing::info!(
            "Loading entity definitions from {}",
            directories
                .iter()
                .map(|dir| dir.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let model = EntityModel::load_paths(conf, &directories)?;
        let dialects = Dialects::load(conf)?;
        let all = model.entities();
        let views = all.iter().filter(|entity| entity.is_view).count();
        tracing::info!(
            "Loaded {} entity definitions ({} view-entities) in {}ms",
            all.len(),
            views,
            started.elapsed().as_millis()
        );
        let reverses: usize = all
            .iter()
            .map(|entity| entity.relationships.iter().filter(|rel| rel.is_auto_reverse).count())
            .sum();
        tracing::info!("Created {reverses} automatic reverse relationships");
        let mut runtimes = HashMap::new();
        for group in groups {
            if runtimes.contains_key(&group.name) {
                return Err(Error::new(format!("entity group {} is duplicated", group.name)));
            }
            let dialect = dialects
                .get(&group.dialect_name)
                .cloned()
                .ok_or_else(|| Error::new(format!("no database named {}", group.dialect_name)))?;
            runtimes.insert(
                group.name,
                GroupRuntime {
                    dialect,
                    transactions: group.transactions,
                },
            );
        }
        Ok(Self {
            inner: Arc::new(Inner {
                model: SharedModel::new(model),
                framework_dir: framework_dir.to_path_buf(),
                component_entity_dirs: component_entity_dirs.to_vec(),
                default_group: default_group.to_string(),
                groups: runtimes,
                banks: SequenceBanks::default(),
                sequenced_id_prefix: sequenced_id_prefix(conf),
                datasource_uuid: datasource_uuid(conf),
                entity_files,
                screen_config_generation: AtomicU64::new(0),
                feed_definition_generation: AtomicU64::new(0),
                eca_rules: RwLock::new(None),
                flags: RwLock::new(EntityFlags::default()),
                db_ready: AtomicBool::new(false),
                locale: RwLock::new("en".to_string()),
                crypt: RwLock::new(None),
                sim_starter: Mutex::new(None),
                backup_runner: Mutex::new(None),
                sim_tx: RwLock::new(None),
                slots: RwLock::new(HashMap::new()),
                tables: super::meta::TableState::default(),
                derived: super::derived::DerivedDefinitions::default(),
            }),
        })
    }

    /// The framework directory this facade was opened from.
    pub fn framework_dir(&self) -> &Path {
        &self.inner.framework_dir
    }

    /// Every `entity` directory of the model: the framework's, then each component's.
    pub fn entity_dirs(&self) -> Vec<std::path::PathBuf> {
        let mut out = vec![self.inner.framework_dir.join("entity")];
        out.extend(self.inner.component_entity_dirs.iter().cloned());
        out
    }

    /// The conf's `<load-entity>` files outside the `entity` directories, read after the framework's directory.
    pub fn entity_files(&self) -> &[std::path::PathBuf] {
        &self.inner.entity_files
    }

    /// Where the model was read, in order: the framework's `entity` directory, the conf's `load-entity` files, then
    /// each component's `entity` directory.
    pub(crate) fn model_paths(&self) -> Vec<std::path::PathBuf> {
        let mut out = vec![self.inner.framework_dir.join("entity")];
        out.extend(self.inner.entity_files.iter().cloned());
        out.extend(self.inner.component_entity_dirs.iter().cloned());
        out
    }

    /// Whether the datasource of `group` (the default group's when it has none) says `sequence-primary-use-uuid`, as
    /// Java's `EntityInfo` reads it with the entity's own attribute.
    pub(crate) fn datasource_uses_uuid(&self, group: Option<&str>) -> bool {
        let uuid = &self.inner.datasource_uuid;
        let default_group = self.inner.default_group.as_str();
        uuid.get(group.unwrap_or(default_group))
            .or_else(|| uuid.get(default_group))
            .copied()
            .unwrap_or(false)
    }

    /// Hook-module state. `init` runs once per `T`. Do not call `with_slot` or [`Self::with_slot_read`] from `body`.
    pub fn with_slot<T, R>(&self, init: impl FnOnce() -> T, body: impl FnOnce(&mut T) -> R) -> R
    where
        T: Any + Send + Sync + 'static,
    {
        let mut slots = write(&self.inner.slots);
        let entry = slots.entry(TypeId::of::<T>()).or_insert_with(|| Box::new(init()));
        let value = entry.downcast_mut::<T>().expect("slot type matches its TypeId");
        body(value)
    }

    /// [`Self::with_slot`] for a lookup: `body` reads the slot alongside other threads reading it. `init` runs once
    /// per `T`, as there. Do not call either from `body`.
    pub fn with_slot_read<T, R>(&self, init: impl FnOnce() -> T, body: impl FnOnce(&T) -> R) -> R
    where
        T: Any + Send + Sync + 'static,
    {
        {
            let slots = read(&self.inner.slots);
            if let Some(entry) = slots.get(&TypeId::of::<T>()) {
                return body(entry.downcast_ref::<T>().expect("slot type matches its TypeId"));
            }
        }
        let mut slots = write(&self.inner.slots);
        let entry = slots.entry(TypeId::of::<T>()).or_insert_with(|| Box::new(init()));
        body(entry.downcast_ref::<T>().expect("slot type matches its TypeId"))
    }

    /// True while sim mode should keep writes off the main schema on this thread. The cache module
    /// sets this on `enter_sim`; tests set it without a schema. Another thread's sim does not show here.
    pub fn set_sim_active(&self, value: bool) {
        super::cache::set_sim_flag(value);
    }

    pub fn sim_active(&self) -> bool {
        super::cache::sim_flag()
    }

    /// Overlay pool for sim mode. `None` with no starter means sim cannot be entered.
    pub fn set_sim_transactions(&self, transactions: Option<TransactionManager>) {
        let mut slot = write(&self.inner.sim_tx);
        *slot = transactions;
    }

    pub fn sim_transactions(&self) -> Option<TransactionManager> {
        read(&self.inner.sim_tx).clone()
    }

    /// Whether this process can enter a sim: it has the overlay pool, or a starter for it.
    pub fn sim_available(&self) -> bool {
        self.sim_transactions().is_some() || lock(&self.inner.sim_starter).is_some()
    }

    /// The host's start of the overlay cluster, run once by the first `enter_sim` that finds no
    /// overlay pool. Core does not start a cluster itself.
    pub fn set_sim_starter(&self, starter: Option<SimStarter>) {
        *lock(&self.inner.sim_starter) = starter;
    }

    /// The host's embedded-database backup. Absent when this process has no embedded cluster, and the
    /// backup service then reports `skipped`.
    pub fn set_backup_runner(&self, runner: Option<crate::backup::BackupRunner>) {
        *lock(&self.inner.backup_runner) = runner;
    }

    /// Clone of the installed runner. The lock is not held across the backup.
    #[must_use]
    pub fn backup_runner(&self) -> Option<crate::backup::BackupRunner> {
        lock(&self.inner.backup_runner).clone()
    }

    /// The overlay pool, started through the starter when there is none yet. A failed start is
    /// returned and tried again on the next call.
    pub(crate) fn sim_transactions_or_start(&self) -> Result<TransactionManager, Error> {
        if let Some(sim) = self.sim_transactions() {
            return Ok(sim);
        }
        let starter = lock(&self.inner.sim_starter);
        if let Some(sim) = self.sim_transactions() {
            return Ok(sim);
        }
        let Some(start) = starter.as_ref() else {
            return Err(Error::new("sim transactions are not configured"));
        };
        let sim = start()?;
        self.set_sim_transactions(Some(sim.clone()));
        Ok(sim)
    }

    pub fn set_locale(&self, locale: impl Into<String>) {
        let mut slot = write(&self.inner.locale);
        *slot = locale.into();
    }

    pub fn locale(&self) -> String {
        read(&self.inner.locale).clone()
    }

    /// Primary field cipher plus decrypt-only alternates. `None` until the caller sets them.
    pub fn set_crypt(&self, primary: CryptConfig, alts: Vec<CryptConfig>) {
        let mut slot = write(&self.inner.crypt);
        *slot = Some((primary, alts));
    }

    /// Install the cipher from `entity-facade` and its `decrypt-alt` children.
    ///
    /// Attributes expand through [`moqui_lib::env`]. An empty `crypt-pass` leaves encryption off.
    pub fn crypt_from_conf(&self, conf: &MNode) -> Result<(), Error> {
        let facade = if conf.name() == "entity-facade" {
            conf.clone()
        } else {
            conf.first_named("entity-facade")
                .ok_or_else(|| Error::new("conf has no entity-facade"))?
        };
        let pass = moqui_lib::env::expand(&facade.attribute("crypt-pass").unwrap_or_default())?;
        if pass.is_empty() {
            return Ok(());
        }
        let primary = crypt_config_from(&facade, pass)?;
        let mut alts = Vec::new();
        for child in facade.children_named("decrypt-alt") {
            let alt_pass = moqui_lib::env::expand(&child.attribute("crypt-pass").unwrap_or_default())?;
            if alt_pass.is_empty() {
                continue;
            }
            alts.push(crypt_config_from(&child, alt_pass)?);
        }
        self.set_crypt(primary, alts);
        Ok(())
    }

    pub fn crypt(&self) -> Option<(CryptConfig, Vec<CryptConfig>)> {
        read(&self.inner.crypt).clone()
    }

    pub fn flags(&self) -> EntityFlags {
        *read(&self.inner.flags)
    }

    pub fn set_disable_entity_eca(&self, value: bool) {
        self.edit_flags(|flags| flags.disable_entity_eca = value);
    }

    pub fn set_disable_audit_log(&self, value: bool) {
        self.edit_flags(|flags| flags.disable_audit_log = value);
    }

    pub fn set_disable_fk_create(&self, value: bool) {
        self.edit_flags(|flags| flags.disable_fk_create = value);
    }

    pub fn set_disable_data_feed(&self, value: bool) {
        self.edit_flags(|flags| flags.disable_data_feed = value);
    }

    pub fn set_dummy_fks(&self, value: bool) {
        self.edit_flags(|flags| flags.dummy_fks = value);
    }

    /// Transaction manager for the entity's group. The group attribute falls back to the default group.
    pub fn transactions_for(&self, entity_name: &str) -> Result<TransactionManager, Error> {
        self.with_group(entity_name, |_, _, _, tx| Ok(tx.clone()))
    }

    /// Transaction manager of the default group, for work not tied to one entity (Java's transaction facade).
    pub fn default_transactions(&self) -> Result<TransactionManager, Error> {
        self.inner
            .groups
            .get(&self.inner.default_group)
            .map(|runtime| runtime.transactions.clone())
            .ok_or_else(|| Error::new(format!("no entity group named {}", self.inner.default_group)))
    }

    pub fn begin(&self, entity_name: &str, timeout: Option<Duration>) -> Result<bool, Error> {
        self.transactions_for(entity_name)?.begin(timeout)
    }

    pub fn commit(&self, entity_name: &str) -> Result<(), Error> {
        self.transactions_for(entity_name)?.commit()
    }

    pub fn rollback(&self, entity_name: &str) -> Result<(), Error> {
        self.transactions_for(entity_name)?.rollback()
    }

    /// `tempSetSequencedIdPrimary`: pin the next primary ids of `seq_name` (an entity name or any sequence name) to
    /// `next` through `next + bank_size`. Factory wide, as Java. Tests use it so generated ids can be asserted.
    pub fn temp_set_sequenced_id_primary(&self, seq_name: &str, next: i64, bank_size: i64) {
        let seq_name = self.sequence_name(seq_name);
        super::sequence::temp_set_sequenced_id_primary(self, &seq_name, next, bank_size);
    }

    /// `tempResetSequencedIdPrimary`: the next id of `seq_name` comes from the database again.
    pub fn temp_reset_sequenced_id_primary(&self, seq_name: &str) {
        let seq_name = self.sequence_name(seq_name);
        super::sequence::temp_reset_sequenced_id_primary(self, &seq_name);
    }

    /// The bank name of a sequence: the full name of an entity, or the name as given.
    fn sequence_name(&self, seq_name: &str) -> String {
        self.with_entity(seq_name, |entity, _, _| Ok(entity.full_name.clone()))
            .unwrap_or_else(|_| seq_name.to_string())
    }

    pub fn make_value(&self, entity_name: &str) -> Result<EntityValue, Error> {
        EntityValue::make(self, entity_name)
    }

    pub fn find(&self, entity_name: &str) -> Result<EntityFind, Error> {
        let full_name = self.with_entity(entity_name, |entity, _, _| Ok(entity.full_name.clone()))?;
        let conditions = super::derived::document_conditions(self, &full_name)?;
        let find = EntityFind::new(self.clone(), full_name);
        // As Java's `find`, a data document's view finds with the document's conditions, from a clone when one is open.
        Ok(match conditions {
            Some(conditions) => conditions.into_iter().fold(find, EntityFind::condition).use_clone(true),
            None => find,
        })
    }

    /// Drop the definitions built at runtime (DB view entities, `DataDocument.<id>`), so the next lookup reads their
    /// rows again. A write to their rows does this already.
    pub fn clear_derived_definitions(&self) {
        super::derived::clear(self);
    }

    /// `EntityFacade.rest`. Find, store, update, delete, and relationship steps.
    pub fn rest(
        &self,
        operation: &str,
        entity_name: &str,
        parameters: &std::collections::BTreeMap<String, Value>,
    ) -> Result<Value, Error> {
        query_rest::rest(self, operation, entity_name, parameters)
    }

    pub fn dynamic_view(
        &self,
        entity_name: &str,
        parameters: &std::collections::BTreeMap<String, Value>,
    ) -> Result<Vec<std::collections::BTreeMap<String, Value>>, Error> {
        query_rest::dynamic_view(self, entity_name, parameters)
    }

    pub fn write_data(
        &self,
        entity_name: &str,
        records: &[std::collections::BTreeMap<String, Value>],
    ) -> Result<(), Error> {
        query_rest::write_data(self, entity_name, records)
    }

    pub(crate) fn with_model<T>(&self, body: impl FnOnce(&EntityModel) -> T) -> T {
        body(self.inner.model.model())
    }

    /// `body` with the definition of `name`, from the file model or else built at runtime ([`super::derived`]), the
    /// dialect of its group, and the file model.
    pub(crate) fn with_entity<T>(
        &self,
        name: &str,
        body: impl FnOnce(&EntityDefinition, &Dialect, &EntityModel) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let model = self.inner.model.model();
        let entity = self.definition(name)?;
        let runtime = self.group_runtime(entity.group.as_deref())?;
        body(&entity, &runtime.dialect, model)
    }

    /// The file model's definition of `name`, or else one built at runtime. The overlay's lock is not held after this.
    fn definition(&self, name: &str) -> Result<Definition<'_>, Error> {
        match self.inner.model.model().entity(name) {
            Ok(entity) => Ok(Definition::File(entity)),
            Err(err) => super::derived::lookup(self, name).map(Definition::Derived).ok_or(err),
        }
    }

    /// The runtime of an entity group. A group with no datasource of its own uses the default group's, as Java's
    /// `getDatasourceFactory` does.
    fn group_runtime(&self, group: Option<&str>) -> Result<&GroupRuntime, Error> {
        let default_group = self.inner.default_group.as_str();
        let group_name = group.unwrap_or(default_group);
        self.inner
            .groups
            .get(group_name)
            .or_else(|| self.inner.groups.get(default_group))
            .ok_or_else(|| Error::new(format!("no transaction manager for entity group {group_name}")))
    }

    pub(crate) fn with_group<T>(
        &self,
        name: &str,
        body: impl FnOnce(&EntityDefinition, &Dialect, &EntityModel, &TransactionManager) -> Result<T, Error>,
    ) -> Result<T, Error> {
        self.with_read_group(name, false, body)
    }

    /// [`Self::with_group`] for a read. With `use_clone`, a group named `{group}#clone...` serves it when one is open,
    /// as Java's `useClone` reads a clone datasource of the entity's group.
    pub(crate) fn with_read_group<T>(
        &self,
        name: &str,
        use_clone: bool,
        body: impl FnOnce(&EntityDefinition, &Dialect, &EntityModel, &TransactionManager) -> Result<T, Error>,
    ) -> Result<T, Error> {
        let model = self.inner.model.model();
        let entity = self.definition(name)?;
        let runtime = if use_clone {
            let group = entity.group.as_deref().unwrap_or(self.inner.default_group.as_str());
            let prefix = format!("{group}#clone");
            let mut clones: Vec<&String> = self
                .inner
                .groups
                .keys()
                .filter(|name| name.starts_with(&prefix))
                .collect();
            clones.sort();
            match clones.first() {
                Some(clone) => &self.inner.groups[*clone],
                None => self.group_runtime(entity.group.as_deref())?,
            }
        } else {
            self.group_runtime(entity.group.as_deref())?
        };
        body(&entity, &runtime.dialect, model, &runtime.transactions)
    }

    /// The names of the open entity groups, the default first.
    pub fn group_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.inner.groups.keys().cloned().collect();
        names.sort();
        names.sort_by_key(|name| *name != self.inner.default_group);
        names
    }

    fn edit_flags(&self, edit: impl FnOnce(&mut EntityFlags)) {
        let mut flags = write(&self.inner.flags);
        edit(&mut flags);
    }
}

/// A definition of the file model, or one built at runtime and shared from the overlay.
enum Definition<'a> {
    File(&'a EntityDefinition),
    Derived(Arc<EntityDefinition>),
}

impl std::ops::Deref for Definition<'_> {
    type Target = EntityDefinition;

    fn deref(&self) -> &EntityDefinition {
        match self {
            Self::File(entity) => entity,
            Self::Derived(entity) => entity,
        }
    }
}

/// `entity-facade` `sequenced-id-prefix`; none when empty, as Java.
fn sequenced_id_prefix(conf: &MNode) -> Option<String> {
    let facade = conf.first_named("entity-facade")?;
    let raw = facade.attribute("sequenced-id-prefix")?;
    Some(moqui_lib::env::expand(&raw).unwrap_or(raw)).filter(|prefix| !prefix.is_empty())
}

/// `sequence-primary-use-uuid` of each `<datasource>`, expanded as Java's `setSystemExpandAttributes`.
fn datasource_uuid(conf: &MNode) -> HashMap<String, bool> {
    super::datasource::datasource_nodes(conf)
        .iter()
        .filter_map(|node| {
            let group = node.attribute("group-name")?;
            Some((
                group,
                super::datasource::datasource_attribute(node, "sequence-primary-use-uuid") == "true",
            ))
        })
        .collect()
}

/// The files of the conf's `<entity-facade><load-entity location>` that no `entity` directory of the model holds, as
/// Java's `getConfEntityFileLocations`. A `component://` location is the framework or a component whose directory has
/// that name; a location that cannot be found is logged and left out.
fn load_entity_files(
    conf: &MNode,
    framework_dir: &Path,
    component_entity_dirs: &[std::path::PathBuf],
) -> Vec<std::path::PathBuf> {
    let Some(facade) = conf.first_named("entity-facade") else {
        return Vec::new();
    };
    let canonical = |path: &Path| std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let mut dirs: Vec<std::path::PathBuf> = vec![canonical(&framework_dir.join("entity"))];
    dirs.extend(component_entity_dirs.iter().map(|dir| canonical(dir)));
    let mut files: Vec<std::path::PathBuf> = Vec::new();
    for node in facade.children_named("load-entity") {
        let location = node.attribute("location").unwrap_or_default();
        let path = if let Some(rest) = location
            .strip_prefix("component://")
            .or_else(|| location.strip_prefix("component:"))
        {
            let rest = rest.trim_start_matches('/');
            let (name, relative) = rest.split_once('/').unwrap_or((rest, ""));
            let root = if name == "framework" {
                Some(framework_dir.to_path_buf())
            } else {
                component_entity_dirs
                    .iter()
                    .filter_map(|dir| dir.parent())
                    .find(|root| root.file_name().is_some_and(|dir_name| dir_name == name))
                    .map(Path::to_path_buf)
            };
            root.map(|root| root.join(relative))
        } else if let Some(rest) = location.strip_prefix("file:") {
            Some(std::path::PathBuf::from(rest.trim_start_matches("//")))
        } else if location.contains("://") {
            None
        } else {
            Some(std::path::PathBuf::from(&location))
        };
        let Some(path) = path.filter(|path| path.is_file()) else {
            tracing::error!("Could not find the entity file of load-entity location {location}, not loading it");
            continue;
        };
        let path = canonical(&path);
        if path.parent().is_some_and(|parent| dirs.iter().any(|dir| dir == parent)) || files.contains(&path) {
            continue;
        }
        files.push(path);
    }
    files
}

fn crypt_config_from(node: &MNode, pass: String) -> Result<CryptConfig, Error> {
    let salt = moqui_lib::env::expand(&node.attribute("crypt-salt").unwrap_or_default())?;
    let algorithm = moqui_lib::env::expand(&node.attribute("crypt-algo").unwrap_or_default())?;
    let iter_text = moqui_lib::env::expand(&node.attribute("crypt-iter").unwrap_or_default())?;
    let iterations = iter_text
        .parse::<u32>()
        .map_err(|err| Error::new(format!("entity-facade crypt-iter {iter_text} is not a number")).with_source(err))?;
    Ok(CryptConfig {
        pass,
        salt,
        iterations,
        algorithm,
    })
}
