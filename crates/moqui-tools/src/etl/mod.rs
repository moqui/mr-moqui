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

//! ETL mappings (plans/archive/Tools-Database.md §10): saved or inline mappings ([`mapping`]) run through
//! `moqui_lib::etl`: the source's extractor ([`source`]), the [`transform::MappingTransformer`] and the target's loader
//! ([`run`]), as `org.moqui.dev.EtlServices` ([`service`]). `load#SpreadsheetEntity` runs an inline mapping through
//! the same path. What a mapping may reach, and that a run is the user's own with authorization on, is [`access`].

pub mod access;
pub mod mapping;
pub mod run;
pub mod service;
pub mod source;
pub mod transform;

pub use mapping::{LoadMode, Mapping, MappingField, SourceKind, TargetKind};
pub use run::{run, FailedRange, Progress, RunOptions, RunOutcome};
pub use transform::{parse_formatted, ErrorLog, MappingTransformer, RowError};
