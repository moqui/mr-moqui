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

//! `JdbcExtractor`: an ETL extractor over one SQL select, beside the drivers.
//!
//! The query runs on a connection of its own, not one from the entity pool, as Java's
//! `getConfConnection` does. Each row is an entry of the record type, its values keyed by the
//! column names the database reports. The rows are read through the driver's stream, inside a
//! read-only transaction that is rolled back at the end.

use std::collections::BTreeMap;
use std::sync::Arc;

use moqui_core::driver::{DbConnection, DbDriver};
use moqui_lib::error::Error;
use moqui_lib::etl::{Extractor, SimpleEntry, SimpleEtl};

#[derive(Default)]
pub struct JdbcExtractor {
    driver: Option<Arc<dyn DbDriver>>,
    record_type: String,
    select_sql: String,
}

impl JdbcExtractor {
    pub fn new() -> Self {
        Self::default()
    }

    /// The entry type for each row, and the select that reads them.
    #[must_use]
    pub fn set_sql_info(mut self, record_type: impl Into<String>, select_sql: impl Into<String>) -> Self {
        self.record_type = record_type.into();
        self.select_sql = select_sql.into();
        self
    }

    /// Read from a driver the host already has, such as a configured datasource.
    #[must_use]
    pub fn set_driver(mut self, driver: Arc<dyn DbDriver>) -> Self {
        self.driver = Some(driver);
        self
    }

    /// Connect to the database by its `database-list` conf name, through [`super::conf_driver`].
    ///
    /// # Errors
    ///
    /// When the conf name has no driver or the port is not a number.
    pub fn set_db_info(
        mut self,
        db_type: &str,
        host: &str,
        port: &str,
        database: &str,
        user: &str,
        password: &str,
    ) -> Result<Self, Error> {
        self.driver = Some(super::conf_driver(&moqui_core::driver::ConfDb {
            db_type: db_type.to_string(),
            host: host.to_string(),
            port: port.to_string(),
            database: database.to_string(),
            user: user.to_string(),
            password: password.to_string(),
        })?);
        Ok(self)
    }

    pub fn record_type(&self) -> &str {
        &self.record_type
    }

    fn run(&self, conn: &mut dyn DbConnection, etl: &mut SimpleEtl) -> Result<(), Error> {
        conn.begin()?;
        conn.open_stream(&self.select_sql, &[])?;
        let mut outcome = Ok(());
        loop {
            let row = match conn.next_row() {
                Ok(Some(row)) => row,
                Ok(None) => break,
                Err(err) => {
                    outcome = Err(err);
                    break;
                }
            };
            let values: BTreeMap<_, _> = row
                .columns
                .into_iter()
                .map(|(name, value)| (name.to_string(), value))
                .collect();
            if let Err(stop) = etl.process_entry(SimpleEntry::new(self.record_type.clone(), values)) {
                tracing::warn!(cause = %stop.cause, "JDBC extract stopped");
                break;
            }
        }
        let closed = conn.close_stream();
        let ended = conn.rollback();
        outcome.and(closed).and(ended)
    }
}

impl Extractor for JdbcExtractor {
    fn extract(&mut self, etl: &mut SimpleEtl) -> Result<(), Error> {
        let wrap = |err: Error| Error::new(format!("Error in SQL query {}", self.select_sql)).with_source(err);
        let driver = self
            .driver
            .clone()
            .ok_or_else(|| wrap(Error::new("no database set; call set_db_info or set_driver")))?;
        let mut conn = driver.connect().map_err(wrap)?;
        self.run(conn.as_mut(), etl).map_err(wrap)
    }
}

#[cfg(all(test, unix, feature = "embedded-postgres"))]
#[path = "jdbc_extract_tests.rs"]
mod tests;
