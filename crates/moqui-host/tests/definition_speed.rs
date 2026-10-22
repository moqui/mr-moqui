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

//! Definition load timings and allocations, with no database: the framework's entity model, its service definitions,
//! and the screen definitions of the base components, each built from its XML as at startup or a first request.
//!
//! Out of the default `cargo test` link, and ignored.
//!
//! ```text
//! CARGO_PROFILE_RELEASE_DEBUG=1 cargo test --release -p moqui-host --test definition_speed -- --ignored --nocapture
//! ```
//!
//! `DEFINITION_SPEED_BATCHES` (default 5) sizes it; see `speed/mod.rs` for the columns.

mod speed;

use std::hint::black_box;
use std::path::{Path, PathBuf};

use moqui_core::context::ExecutionContextFactory;
use moqui_core::screen::ScreenFacade;
use moqui_db::model::EntityModel;
use moqui_lib::node::MNode;

use speed::{env_usize, DirGuard, Rows};

#[test]
#[ignore = "definition timings: cargo test --release -p moqui-host --test definition_speed -- --ignored --nocapture"]
fn definition_speed() {
    speed::warn_if_debug("definition_speed");
    let batches = env_usize("DEFINITION_SPEED_BATCHES", 5);
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component");
    let framework = base.join("framework");
    let runtime = speed::temp_dir("definition-speed-rt");
    let _guard = DirGuard(runtime.clone());
    std::fs::create_dir_all(&runtime).expect("runtime dir");
    let factory = ExecutionContextFactory::boot_with_framework(&runtime, &framework).expect("boot");
    let conf = MNode::parse_file(framework.join("conf/MoquiDefaultConf.xml")).expect("conf");
    let mut screens = Vec::new();
    for component in ["framework", "tools", "webroot", "example"] {
        collect_xml(&base.join(component).join("screen"), &mut screens);
    }
    screens.sort();
    let screens: Vec<String> = screens.iter().map(|path| path.display().to_string()).collect();

    let context = factory.new_context();
    let _entered = context.enter();
    let facade = ScreenFacade::of(&factory);
    let built = screens
        .iter()
        .filter(|location| facade.screen_def(&factory, location).is_ok())
        .count();
    println!();
    println!(
        "definitions, {} screen files ({built} build) x {batches} batches. Entity and service rows are one call each.",
        screens.len()
    );
    let mut rows = Rows::default();
    for batch in 0..=batches {
        rows.record = batch > 0;
        rows.run("entity model, framework entity files", 1, || {
            black_box(EntityModel::load_directory(&conf, &framework.join("entity")).expect("entity model"))
        });
        rows.run("service definitions, the configured files", 1, || {
            moqui_core::service::load_definitions().expect("service definitions");
        });
        facade.clear();
        rows.run("screen definitions, per screen", screens.len(), || {
            for location in &screens {
                let _ = black_box(facade.screen_def(&factory, location));
            }
        });
    }
    rows.print();
}

fn collect_xml(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_xml(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "xml") {
            out.push(path);
        }
    }
}
