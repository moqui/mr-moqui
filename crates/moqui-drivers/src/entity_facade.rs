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

//! Register one entity facade. The driver is the host's. Core does not open Postgres.

use std::path::Path;
use std::sync::Arc;

use moqui_core::context::ExecutionContextFactory;
use moqui_core::driver::DbDriver;
use moqui_core::entity::EntityFacade;
use moqui_core::transaction::PoolConfig;
use moqui_lib::error::Error;

/// Load the framework model onto the `transactional` group using the `postgres` dialect.
pub fn open_entity_facade(
    framework_dir: &Path,
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
) -> Result<EntityFacade, Error> {
    open_entity_facade_dialect(framework_dir, driver, pool, "postgres")
}

/// Load the framework model onto the `transactional` group using `dialect`.
///
/// `dialect` is a `database-list` name such as `postgres` or `mysql`.
///
/// # Errors
///
/// Returns an error when the conf or the entity model cannot be read, or when `dialect` is not defined.
pub fn open_entity_facade_dialect(
    framework_dir: &Path,
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
    dialect: &str,
) -> Result<EntityFacade, Error> {
    EntityFacade::open_with_driver(framework_dir, driver, pool, dialect, &["transactional"])
}

/// [`open_entity_facade_dialect`] with the `entity` directories of the components
/// ([`component_entity_dirs`]): their entities, extends, and entity ECA rules join the framework's.
///
/// # Errors
///
/// Returns an error when the conf or the entity model cannot be read, or when `dialect` is not defined.
pub fn open_entity_facade_components(
    framework_dir: &Path,
    component_entity_dirs: &[std::path::PathBuf],
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
    dialect: &str,
) -> Result<EntityFacade, Error> {
    EntityFacade::open_with_driver_components(
        framework_dir,
        component_entity_dirs,
        driver,
        pool,
        dialect,
        &["transactional"],
    )
}

/// Open the runtime's merged configuration, not just the framework defaults.
/// In particular, physical naming overrides must be identical for DDL and queries.
///
/// The other datasources' groups are opened too ([`open_entity_facade_for_db`] for one on the embedded cluster).
pub fn open_entity_facade_for_factory(
    factory: &ExecutionContextFactory,
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
    dialect: &str,
) -> Result<EntityFacade, Error> {
    open_facade(factory, driver, pool, dialect, None)
}

/// [`open_entity_facade_for_factory`] over the open entity database `db`: a datasource of another group with no server
/// of its own (the default `content` one) gets a database of its own in the embedded cluster.
pub fn open_entity_facade_for_db(
    factory: &ExecutionContextFactory,
    db: &super::entity_db::EntityDb,
    pool: PoolConfig,
) -> Result<EntityFacade, Error> {
    open_facade(factory, Arc::clone(&db.driver), pool, &db.conf.dialect, Some(db))
}

fn open_facade(
    factory: &ExecutionContextFactory,
    driver: Arc<dyn DbDriver>,
    pool: PoolConfig,
    dialect: &str,
    db: Option<&super::entity_db::EntityDb>,
) -> Result<EntityFacade, Error> {
    let conf = moqui_lib::node::MNode::parse_shared("moqui-conf", &factory.conf_text())?;
    let tx = moqui_core::transaction::TransactionManager::new(Arc::clone(&driver), pool)?;
    tx.configure_from_conf(&conf)?;
    let mut groups = vec![moqui_core::entity::EntityGroup::new("transactional", dialect, tx)];
    groups.extend(other_groups(&conf, &driver, db));
    moqui_core::entity::open_for_factory(factory, "transactional", groups)
}

/// The conf's `<datasource>` elements for groups other than `transactional` that are not `disabled`, by group.
fn other_datasources(conf: &moqui_lib::node::MNode) -> Vec<(String, moqui_lib::node::MNode)> {
    use moqui_core::entity::datasource::{datasource_attribute, datasource_nodes};
    datasource_nodes(conf)
        .into_iter()
        .filter_map(|node| {
            let group = datasource_attribute(&node, "group-name");
            if group.is_empty() || group == "transactional" {
                return None;
            }
            if datasource_attribute(&node, "disabled") == "true" {
                tracing::debug!("Datasource {group} is disabled, not opening it");
                return None;
            }
            Some((group, node))
        })
        .collect()
}

/// The pools of the conf's other datasources on the embedded cluster (the `content` group's database there), which
/// its `max_connections` makes room for beside the transactional pool. Zero when the transactional datasource is not
/// embedded, as those share it.
#[must_use]
pub fn embedded_group_pool_max(factory: &ExecutionContextFactory) -> u32 {
    use moqui_core::entity::datasource::{datasource_attribute, datasource_conf, DatasourceKind};
    let Ok(conf) = moqui_lib::node::MNode::parse_shared("moqui-conf", &factory.conf_text()) else {
        return 0;
    };
    let embedded = |group: &str| {
        datasource_conf(&conf, group)
            .ok()
            .filter(|ds| ds.kind == DatasourceKind::Embedded)
    };
    if embedded("transactional").is_none() {
        return 0;
    }
    other_datasources(&conf)
        .into_iter()
        .filter(|(_, node)| datasource_attribute(node, "object-factory").is_empty())
        .filter_map(|(group, _)| embedded(&group))
        .map(|ds| ds.pool.max)
        .sum()
}

/// The entity groups of the conf's other `<datasource>` elements, each with a pool of its own, as Java opens each
/// datasource. A `disabled` one is left out. An `object-factory` one is opened by the factory registered for it
/// ([`moqui_core::entity::datasource::register_datasource_factory`]). One with no server of its own (an empty or `h2`
/// `database-conf-name`) is a database of its own in the embedded cluster, named by its `databaseName` (else
/// `moqui_<group>`), when `db` is that cluster; otherwise it is left out and its entities share the transactional
/// database. A datasource that cannot be opened is logged and left out: its entities use the `transactional` group.
fn other_groups(
    conf: &moqui_lib::node::MNode,
    main_driver: &Arc<dyn DbDriver>,
    db: Option<&super::entity_db::EntityDb>,
) -> Vec<moqui_core::entity::EntityGroup> {
    use moqui_core::entity::datasource::{datasource_attribute, datasource_conf, datasource_factory, DatasourceKind};
    let mut groups = Vec::new();
    for (group, node) in other_datasources(conf) {
        let object_factory = datasource_attribute(&node, "object-factory");
        if !object_factory.is_empty() {
            match datasource_factory(&object_factory) {
                Some(factory) => match factory.open_group(&group, &node, conf) {
                    Ok(opened) => groups.push(opened),
                    Err(err) => tracing::error!(error = %err, "Could not open datasource {group}, its entities use the transactional group"),
                },
                None => tracing::info!(
                    "Datasource {group} uses {object_factory}, which this port does not serve; its entities use the transactional group"
                ),
            }
            continue;
        }
        let opened = datasource_conf(conf, &group).and_then(|ds| {
            let (driver, target) = if ds.kind == DatasourceKind::Embedded {
                let Some(db) = db.filter(|db| db.is_embedded()) else {
                    tracing::info!(
                        "Datasource {group} has no server of its own and the transactional database is not the embedded one: its entities share the transactional database"
                    );
                    return Ok(None);
                };
                let database = if ds.database.is_empty() {
                    format!("moqui_{group}")
                } else {
                    ds.database.clone()
                };
                let driver = db
                    .embedded_database_driver(&database)?
                    .ok_or_else(|| Error::new("the embedded cluster is not open"))?;
                (driver, format!("database {database} of the embedded cluster"))
            } else {
                let driver = super::entity_db::server_driver(&ds)?.unwrap_or_else(|| Arc::clone(main_driver));
                (driver, format!("{} database {} on {}", ds.dialect, ds.database, ds.host))
            };
            let tx = moqui_core::transaction::TransactionManager::new(driver, ds.pool.clone())?;
            tx.configure_from_conf(conf)?;
            Ok(Some((moqui_core::entity::EntityGroup::new(group.clone(), ds.dialect, tx), target)))
        });
        match opened {
            Ok(Some((opened, target))) => {
                tracing::info!("Opened datasource {group}, {target}");
                groups.push(opened);
            }
            Ok(None) => {}
            Err(err) => {
                tracing::error!(error = %err, "Could not open datasource {group}, its entities use the transactional group")
            }
        }
    }
    groups
}

pub use moqui_core::context::component_entity_dirs;

/// Install the facade so `dbresource://` reads and writes through it, with the conf connection drivers scripts use for
/// another database.
pub fn attach(factory: &ExecutionContextFactory, facade: EntityFacade) {
    super::install_conf_drivers();
    factory.set_entity(facade);
}

/// Each datasource's DDL settings on the facade (the main one from `conf`, the others from the merged conf), and Java's
/// find on a missing table: nothing found, not an error.
pub fn apply_datasource_conf(
    facade: &EntityFacade,
    conf: &moqui_core::entity::datasource::DatasourceConf,
    merged: Option<&moqui_lib::node::MNode>,
) {
    let ddl = |ds: &moqui_core::entity::datasource::DatasourceConf| moqui_core::entity::DdlConf {
        startup_add_missing: ds.startup_add_missing,
        runtime_add_missing: ds.runtime_add_missing,
        runtime_add_fks: ds.runtime_add_fks,
    };
    facade.set_ddl_conf(&conf.group, ddl(conf));
    if let Some(merged) = merged {
        for group in facade.group_names() {
            if group == conf.group {
                continue;
            }
            if let Ok(other) = moqui_core::entity::datasource::datasource_conf(merged, &group) {
                facade.set_ddl_conf(&group, ddl(&other));
            }
        }
    }
    facade.set_find_table_check(true);
}

#[cfg(all(test, unix, feature = "embedded-postgres"))]
#[path = "entity_facade_tests.rs"]
mod tests;
