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

//! Startup timings: a server's first start on an empty database (initdb, the table check, the empty-db load and the
//! second factory), then restarts on the loaded database as a developer's `moqui` run is, the stop of each, and the
//! factory boot alone (conf merge, components, the parts with no database).
//!
//! The runtime is a temporary directory with the repository's `runtime/conf/MoquiDevConf.xml` and links to its
//! `components/base` and, unless `STARTUP_SPEED_COMPONENTS=base`, its `component` directory (mr-mantle, mr-marble,
//! mr-boss), on an embedded cluster of its own. The SSH, cluster and HTTP ports are ephemeral.
//!
//! Out of the default `cargo test` link, and ignored so naming the binary does not start a postmaster.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test startup_speed -- --ignored --nocapture
//! ```
//!
//! `STARTUP_SPEED_BATCHES` (default 5) is the restarts of each kind, `STARTUP_SPEED_LOAD` (default
//! `seed,seed-initial,install`) the empty-db load types (`all` adds demo data, as the dev conf has it), and
//! `STARTUP_SPEED_LOG` (`info`) shows the startup's phase lines. The allocation columns count the test thread only;
//! startup work on other threads (the postmaster, pools, loads run elsewhere) is in the time alone. See `speed/mod.rs`
//! for the columns.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

mod speed;

use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContextFactory;
// The example component's MoquiConf.xml names its tool factory, so the binary links it as `moqui` does.
use example as _;

use speed::{env_usize, DirGuard, Rows};

#[test]
#[ignore = "startup timings: cargo test --release -p moqui-host --test startup_speed -- --ignored --nocapture"]
fn startup_speed() {
    speed::warn_if_debug("startup_speed");
    let level = match std::env::var("STARTUP_SPEED_LOG").as_deref() {
        Ok("info") => tracing::Level::INFO,
        _ => tracing::Level::WARN,
    };
    moqui_host::logging::install_with_console(Some(level));
    let batches = env_usize("STARTUP_SPEED_BATCHES", 5);
    let components = std::env::var("STARTUP_SPEED_COMPONENTS").unwrap_or_else(|_| "all".to_string());
    let load = std::env::var("STARTUP_SPEED_LOAD").unwrap_or_else(|_| "seed,seed-initial,install".to_string());

    let repo = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let repo_runtime = std::fs::canonicalize(repo.join("runtime")).expect("runtime");
    let root = speed::temp_dir("startup-speed");
    let _guard = DirGuard(root.clone());
    let runtime = root.join("runtime");
    let cluster = root.join("db");
    std::fs::create_dir_all(runtime.join("conf")).expect("runtime conf");
    std::fs::copy(
        repo_runtime.join("conf/MoquiDevConf.xml"),
        runtime.join("conf/MoquiDevConf.xml"),
    )
    .expect("dev conf");
    std::fs::create_dir_all(runtime.join("components")).expect("runtime components");
    link(&repo_runtime.join("components/base"), &runtime.join("components/base"));
    std::fs::copy(
        repo_runtime.join("components/components.xml"),
        runtime.join("components/components.xml"),
    )
    .expect("components.xml");
    if components != "base" {
        link(
            &repo_runtime.join("components/addon"),
            &runtime.join("components/addon"),
        );
    }
    let framework = runtime.join("components/base/framework");
    for (name, value) in [
        ("entity_empty_db_load", load.as_str()),
        ("ssh_port", "0"),
        ("cluster_https_port", "0"),
        ("cluster_ssh_port", "0"),
    ] {
        moqui_lib::env::set_property(name, value);
    }
    let bind = ([127, 0, 0, 1], 0).into();

    println!();
    println!(
        "startup, components {components}, empty-db load {load}, x {batches} batches. Each row is one start or stop."
    );
    let mut rows = Rows {
        record: true,
        ..Rows::default()
    };
    let server = rows.run("first start, empty database", 1, || {
        moqui_host::start_server(&runtime, &framework, &cluster, bind).expect("first start")
    });
    rows.run("stop", 1, || drop(server));
    for _ in 0..batches {
        let server = rows.run("restart, server", 1, || {
            moqui_host::start_server(&runtime, &framework, &cluster, bind).expect("restart")
        });
        rows.run("stop", 1, || drop(server));
    }
    for _ in 0..batches {
        let local = rows.run("restart, no listeners (moqui tui)", 1, || {
            moqui_host::start_local(&runtime, &framework, &cluster).expect("local start")
        });
        rows.run("stop, no listeners", 1, || drop(local));
    }
    for _ in 0..batches {
        let factory = rows.run("factory boot alone, no database", 1, || {
            ExecutionContextFactory::boot_with_framework(&runtime, &framework).expect("factory")
        });
        rows.run("factory shutdown", 1, || {
            factory.shutdown();
            drop(factory);
        });
    }
    rows.print();
}

fn link(from: &Path, to: &Path) {
    std::os::unix::fs::symlink(from, to).unwrap_or_else(|err| panic!("link {}: {err}", to.display()));
}
