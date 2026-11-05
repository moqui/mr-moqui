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

//! The Tools app's server side (plans/Tools-Database.md §13): `org.moqui.impl.DatabaseServices` ([`service`]) on the
//! entities of `framework/entity/DatabaseEntities.xml`: what each database allows ([`capabilities`]), the catalog
//! queries per dialect ([`catalog`]), SQL runs, explain and the database's statement stats ([`sql`], with their
//! services in [`sql_service`]) and the audit ([`audit`]). The jobs, data grid and ETL of the plan come here as well.
//!
//! A service that works on a database takes `db`: `group:<groupName>` for an entity group's database, or
//! `ext:<connectionId>` for a `moqui.database.DatabaseConnection` ([`DbRef::parse`]). An external database is refused
//! with one vague error ([`NOT_AVAILABLE`]) on a production instance or without `DATABASE_EXTERNAL` (§4.3); until
//! external connections are built (§15 step 7) it is always refused so.
//!
//! This crate is on moqui-core (plans/archive/Crate-Split.md), which reaches it through the
//! [`moqui_core::extension`] entry below. moqui-host links it in every build; the browser build leaves it out, and its
//! services report that `tools` is not built in.

pub mod audit;
pub mod capabilities;
pub mod catalog;
pub mod grid;
pub mod grid_service;
pub mod service;
pub mod sql;
pub mod sql_service;

moqui_core::extension::submit! {
    moqui_core::extension::Extension {
        features: &["tools"],
        register_services: service::register,
        ..moqui_core::extension::Extension::EMPTY
    }
}

/// The one error a refused external database gets, naming neither cause (production, or the permission).
pub const NOT_AVAILABLE: &str = "Not available";

/// The database a service works on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DbRef {
    /// An entity group's database: `group:<groupName>`.
    Group(String),
    /// A database the entity groups do not use, by its `DatabaseConnection`: `ext:<connectionId>`.
    External(String),
}

impl DbRef {
    /// Read `db`; empty is the facade's default group.
    ///
    /// # Errors
    ///
    /// Returns an error naming the two forms when `db` is neither.
    pub fn parse(db: &str, default_group: &str) -> Result<Self, String> {
        let db = db.trim();
        if db.is_empty() {
            return Ok(Self::Group(default_group.to_string()));
        }
        match db.split_once(':') {
            Some(("group", name)) if !name.trim().is_empty() => Ok(Self::Group(name.trim().to_string())),
            Some(("ext", id)) if !id.trim().is_empty() => Ok(Self::External(id.trim().to_string())),
            _ => Err(format!("db {db:?} is neither group:<groupName> nor ext:<connectionId>")),
        }
    }

    /// As the services take it.
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Group(name) => format!("group:{name}"),
            Self::External(id) => format!("ext:{id}"),
        }
    }
}

#[cfg(test)]
mod tests;
