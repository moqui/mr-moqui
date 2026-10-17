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

//! The integration tests of the host as one test binary.
//!
//! Each file under `tests/` is its own binary, and each one links the whole host. Their tests are modules here instead, so a
//! change to the sources links once. Tests that start a server or set process properties take [`common::serial`]; the
//! seeded gates do so through `common::start*`.
//!
//! `search_logs_postgres.rs` stays apart: it needs the process-wide log shipping as no earlier test left it.
//! `gate3.rs` and `gate3_browser.rs` stay apart, out of the default run: `cargo test -p moqui-host --test gate3 --test gate3_browser`.

#[path = "../common/mod.rs"]
mod common;

mod backup;
mod cluster_notify;
mod elfinder;
mod entity_cache;
mod entity_db;
mod field_rw;
mod gate1;
mod gate2;
mod load_feed;
mod notws;
mod opensearch_start;
mod pdf;
mod query_rest;
mod qvt_report;
mod search;
mod search_opensearch;
mod search_replica;
mod search_semantic;
mod ssh;
mod sso;
mod system_sync;
mod tool_components;
mod tools_screens;
mod tools_services;
mod types;
