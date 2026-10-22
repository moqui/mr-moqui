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

//! The browser host: one factory on the inline executor, one SQLite entity facade.
//!
//! This is the counterpart of `moqui-host` boot. Every call is a top-level request: it enters a
//! fresh execution context and, when that ends, drains queued async work with
//! [`ExecutionContextFactory::run_pending`].
//!
//! There is no login flow yet. Service calls and entity finds run with authorization disabled, the
//! way the data loader does. Local results are never authority over server data (`plans/archive/Wasm.md`).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use moqui_core::context::{BootOptions, ExecutionContextFactory, Executor};
use moqui_core::driver::DbDriver;
use moqui_core::entity::load_feed::{load_entity_data, LoadOptions};
use moqui_core::entity::EntityFacade;
use moqui_core::screen::ScreenTest;
use moqui_core::transaction::PoolConfig;
use moqui_lib::error::Error;
use moqui_lib::object::Value;

use crate::sqlite::SqliteDriver;

/// The `database-list` profile this host uses.
pub const DIALECT: &str = "sqlite";

/// Logical connections share one SQLite handle, so they are cheap. A zero acquire timeout makes an
/// exhausted pool fail at once: a wait would block the only thread.
fn pool() -> PoolConfig {
    PoolConfig {
        min: 0,
        max: 16,
        reserve: 2,
        acquire_timeout: Duration::ZERO,
        ..Default::default()
    }
}

/// A booted framework with a local database.
pub struct Browser {
    factory: Arc<ExecutionContextFactory>,
    facade: EntityFacade,
    sqlite: Arc<SqliteDriver>,
}

impl Browser {
    /// Boot from `runtime_dir` and `framework_dir`, with the database at `db_path` (`:memory:` for
    /// none on disk).
    ///
    /// # Errors
    ///
    /// Returns an error when the conf, the entity model, or the database cannot be opened.
    pub fn open(runtime_dir: &Path, framework_dir: &Path, db_path: &str) -> Result<Self, Error> {
        let factory = ExecutionContextFactory::boot_with_options(
            runtime_dir,
            framework_dir,
            BootOptions {
                executor: Executor::Inline,
                ..BootOptions::default()
            },
        )
        .map_err(|err| Error::new(format!("boot: {err}")))?;
        let sqlite = Arc::new(SqliteDriver::open(db_path)?);
        let driver: Arc<dyn DbDriver> = sqlite.clone();
        let facade = open_entity_facade(factory.framework_dir(), driver)?;
        factory.set_entity(facade.clone());
        factory
            .init_tools()
            .map_err(|err| Error::new(format!("tools: {err}")))?;
        let browser = Self {
            factory,
            facade,
            sqlite,
        };
        // The configured service files, then each runtime component's `service/` tree. A file that
        // still holds Groovy is skipped and named here.
        let skipped = browser.request(true, || {
            let mut paths = moqui_core::service::configured_service_files()
                .map_err(|err| Error::new(format!("services: {err}")))?;
            for component in browser.factory.components() {
                if component.location != browser.factory.framework_dir() {
                    xml_files(&component.location.join("service"), &mut paths);
                }
            }
            moqui_core::service::lint(&paths).map_err(|err| Error::new(format!("services: {err}")))
        })?;
        if !skipped.is_empty() {
            tracing::warn!(count = skipped.len(), "service definitions not registered");
        }
        Ok(browser)
    }

    pub fn factory(&self) -> &Arc<ExecutionContextFactory> {
        &self.factory
    }

    pub fn entity(&self) -> &EntityFacade {
        &self.facade
    }

    /// Run `body` as one top-level request, then drain queued async work.
    ///
    /// # Errors
    ///
    /// Returns the error `body` returned.
    pub fn request<T>(&self, disable_authz: bool, body: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
        let context = self.factory.new_context();
        let entered = context.enter();
        let authz = disable_authz.then(|| context.disable_authz());
        let result = body();
        drop(authz);
        drop(entered);
        drop(context);
        self.factory.run_pending();
        result
    }

    /// Create missing tables for these entities, or for every entity when `names` is empty.
    ///
    /// # Errors
    ///
    /// Returns an error when a table cannot be created.
    pub fn check_tables(&self, names: &[&str]) -> Result<(), Error> {
        // One commit for every CREATE (see SqliteDriver::bulk).
        self.request(true, || {
            self.sqlite.bulk(|| {
                if names.is_empty() {
                    self.facade.check_all_tables()
                } else {
                    self.facade.check_tables(names)
                }
            })
        })
    }

    /// Load one entity data file. `data_types` empty loads every type.
    ///
    /// # Errors
    ///
    /// Returns an error when the file cannot be read or a record cannot be written.
    pub fn load_data(&self, location: &str, data_types: &[String]) -> Result<u64, Error> {
        let options = LoadOptions {
            data_types: data_types.to_vec(),
            components: Vec::new(),
            only_create: false,
            dummy_fks: false,
            disable_fk_create: true,
            timeout: None,
            restrict_sensitive_entities: false,
            ..Default::default()
        };
        self.request(true, || Ok(load_entity_data(&self.facade, location, &options)?.records))
    }

    /// Call a service and return its out parameters.
    ///
    /// # Errors
    ///
    /// Returns the error the service raised, or its error messages.
    pub fn call_service(
        &self,
        name: &str,
        parameters: &BTreeMap<String, Value>,
    ) -> Result<BTreeMap<String, Value>, Error> {
        self.request(true, || {
            let out = moqui_core::service::sync(name, parameters).map_err(|err| Error::new(err.to_string()))?;
            // A failed inner call leaves its error in the message facade and rolls the outer
            // transaction back; the caller sees that as a failure, not empty out parameters.
            let errors = moqui_core::context::ExecutionContext::with_current(|ctx| ctx.message().errors().join("\n"))
                .unwrap_or_default();
            if errors.is_empty() {
                Ok(out)
            } else {
                Err(Error::new(errors))
            }
        })
    }

    /// Render `path` under the root screen at `root` in `mode` (`text`, `html`, `json`, ...).
    ///
    /// # Errors
    ///
    /// Returns the error the render raised.
    pub fn render(
        &self,
        root: &str,
        path: &str,
        mode: &str,
        parameters: BTreeMap<String, Value>,
    ) -> Result<(u16, String), Error> {
        self.request(false, || {
            let rendered = ScreenTest::new(&self.factory)
                .root_screen(root)
                .render_mode(mode)
                .render(path, parameters)
                .map_err(|err| Error::new(err.to_string()))?;
            Ok((rendered.status(), rendered.text()))
        })
    }

    /// Find records of `entity` where each `equals` field matches, in `order_by` order.
    ///
    /// # Errors
    ///
    /// Returns an error when the entity is unknown or the query fails.
    pub fn find(
        &self,
        entity: &str,
        equals: &BTreeMap<String, Value>,
        order_by: &[String],
        limit: Option<i64>,
    ) -> Result<Vec<BTreeMap<String, Value>>, Error> {
        self.request(true, || {
            let mut find = self.facade.find(entity)?;
            for (field, value) in equals {
                find = find.eq(field, value.clone());
            }
            for field in order_by {
                find = find.order_by_text(field);
            }
            if let Some(limit) = limit {
                find = find.limit(limit);
            }
            Ok(find.list()?.iter().map(|value| moqui_core::entity::string_map(value.fields())).collect())
        })
    }

    /// Create or update one record from field values.
    ///
    /// # Errors
    ///
    /// Returns an error when the entity is unknown or the write fails.
    pub fn store(&self, entity: &str, fields: &BTreeMap<String, Value>) -> Result<(), Error> {
        self.request(true, || {
            let mut value = self.facade.make_value(entity)?;
            for (field, field_value) in fields {
                value.set(field.clone(), field_value.clone());
            }
            value.create_or_update(&self.facade)
        })
    }
}

/// Every `.xml` file under `dir`, sorted, appended to `out`. A missing dir adds nothing.
fn xml_files(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut paths: Vec<_> = entries.filter_map(Result::ok).map(|entry| entry.path()).collect();
    paths.sort();
    for path in paths {
        if path.is_dir() {
            xml_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "xml") {
            out.push(path);
        }
    }
}

/// Load the framework model onto the `transactional` and `logging` groups with the `sqlite` dialect.
/// One database holds every group. The server puts `logging` on a separate datasource.
///
/// # Errors
///
/// Returns an error when the conf or the entity model cannot be read.
pub fn open_entity_facade(framework_dir: &Path, driver: Arc<dyn DbDriver>) -> Result<EntityFacade, Error> {
    EntityFacade::open_with_driver(framework_dir, driver, pool(), DIALECT, &["transactional", "logging"])
}

#[cfg(test)]
mod tests;
