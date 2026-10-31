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
//! change to the sources links once. A test that starts a server or sets process properties runs in a process of its
//! own ([`common::own_process`]), so those run in parallel; the seeded gates (`common::start*`) and [`common::serial`]
//! fail outside one.
//!
//! `gate3.rs`, `gate3_browser.rs`, `client_browser.rs` and `tui_screens.rs` stay apart, out of the default run
//! (`cargo test -p moqui-host --test gate3`), and so do the timings (`*_speed.rs`, `search_bench.rs`,
//! `monitor_performance.rs`), which run optimized and by name.

#[path = "../common/mod.rs"]
mod common;

// the example component loads with the repository runtime; link its tool factory as the moqui binary does
use example as _;
// the dev-aide component's services, which tests/it/devaide.rs links into a dev instance
#[cfg(feature = "devtools")]
use ::dev_aide as _;

mod a2a;
mod agent_eval;
mod agent_members;
mod agent_remote;
mod agent_screens;
mod assist;
mod backup;
mod cluster_notify;
mod collab;
mod component_load;
mod create_or_update_race;
mod devaide;
mod elfinder;
mod entity_cache;
mod entity_db;
mod factory_restart;
mod field_rw;
mod gate1;
mod gate2;
mod gateway;
mod git_hosting;
mod git_resources;
mod git_storage;
mod llm;
mod load_feed;
mod lock_then_insert;
#[cfg(all(feature = "devtools", feature = "tui"))]
mod lsp;
mod mantle_search;
mod mcp;
mod monitor_edge;
mod monitor_grpc;
mod monitor_operations;
mod monitor_readers;
mod monitor_recovery;
mod monitor_regressions;
mod monitor_rum;
mod monitor_tui;
mod notws;
mod opensearch_start;
mod pdf;
mod pg_external;
mod query_rest;
mod qvt_report;
mod report_alerts;
mod report_documents;
mod report_monitoring;
mod report_schedule_lock;
mod search;
mod search_logs_postgres;
mod search_opensearch;
mod search_replica;
mod search_semantic;
mod smtp;
mod ssh;
mod sso;
mod system_sync;
mod telemetry_partitions;
mod telemetry_rollup;
mod termws;
mod test_client;
mod tool_components;
mod tools_screens;
mod tools_services;
mod tui_remote;
mod types;
mod user_status;
mod wasm_browser;
mod web_protection;
mod web_scripts;
mod web_streaming;
