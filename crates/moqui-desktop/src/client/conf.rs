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

//! The client's settings, `<client-conf>` of the Moqui XML conf (plans/Mobile-Desktop.md §6, MD4), so each build
//! embeds its own in its runtime conf. The client mode starts no node, so the conf is read here as the framework would
//! merge it, without booting one: `MoquiDefaultConf.xml`, each component's `MoquiConf.xml` in load order, then the
//! runtime conf (`moqui_core::context::merge_component_confs`). It is read once, when first asked for, off the main
//! thread; a runtime with no framework, or a conf that does not parse, gives the defaults (logged).

use std::path::{Path, PathBuf};

use moqui_client::conf::ClientConf;
use tauri::{AppHandle, Manager};

use crate::runtime_dir::Location;

/// The runtime whose conf the client reads: the one a node's mode would run on (`MOQUI_RUNTIME`, this repository's in
/// a development run, one found beside the app), else the bundled one in the app's resources, read where it is (the
/// client makes no runtime in the app data directory).
pub(crate) fn runtime_dir(app: &AppHandle) -> Option<PathBuf> {
    let paths = app.path();
    let location = Location::choose(
        std::env::var_os("MOQUI_RUNTIME").map(Into::into),
        tauri::is_dev(),
        crate::runtime_dir::find_existing(),
        &paths.resource_dir().ok()?,
        &paths.app_data_dir().ok()?,
    );
    Some(match location {
        Location::Existing(dir) => dir,
        Location::Bundled { resources, .. } => resources,
    })
}

/// The `<client-conf>` of the merged conf of `runtime`; the defaults when there is none, or the conf cannot be read.
pub(crate) fn read(runtime: &Path) -> ClientConf {
    let framework = moqui_core::context::layout::framework_dir(runtime);
    if !framework.join("conf/MoquiDefaultConf.xml").is_file() {
        tracing::warn!(
            "No framework conf in {}: the client's settings are the defaults",
            runtime.display()
        );
        return ClientConf::default();
    }
    let started = std::time::Instant::now();
    let merged = moqui_core::context::discover_components(runtime, &framework).and_then(|components| {
        let dirs: Vec<PathBuf> = components.into_iter().map(|info| info.location).collect();
        match moqui_core::context::merge_component_confs(&framework, runtime, &dirs, false)? {
            Some(conf) => Ok(conf),
            None => moqui_core::context::load_merged_conf(&framework, runtime),
        }
    });
    let conf = match merged {
        Ok(conf) => conf
            .first_named("client-conf")
            .map(|node| ClientConf::from_node(&node))
            .unwrap_or_default(),
        Err(err) => {
            tracing::error!(
                "The conf of {} was not read, so the client's settings are the defaults: {err}",
                runtime.display()
            );
            ClientConf::default()
        }
    };
    tracing::info!(
        "Client conf from {} in {}ms: device mode {}",
        runtime.display(),
        started.elapsed().as_millis(),
        conf.device_mode.as_str()
    );
    conf
}

#[cfg(test)]
#[path = "conf_tests.rs"]
mod tests;
