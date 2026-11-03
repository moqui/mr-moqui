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

//! Where the node's runtime directory is (plans/Mobile-Desktop.md, configuration 2). `MOQUI_RUNTIME` when set; in a
//! development run this repository's `runtime`; in a bundled app a runtime in the app data directory, whose read-only
//! parts (the components and the conf) are links into the app's resources, so an app upgrade replaces them and keeps
//! the data, `.env` and logs, which the node writes beside them.

use std::path::{Path, PathBuf};

/// The parts of a bundled runtime that come with the app: what `./moq archive --no-data` puts in, less the binary.
pub(crate) const READ_ONLY: [&str; 5] = ["base-component", "component", "ecomponent", "conf", "MoquiConf.xml"];

/// Where the runtime is, before any of it is made.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Location {
    /// `MOQUI_RUNTIME`, or this repository's runtime in a development run: used as it is.
    Existing(PathBuf),
    /// The runtime in the app data directory, linked to the bundled parts in `resources`.
    Bundled { resources: PathBuf, runtime: PathBuf },
}

impl Location {
    /// `env` is `MOQUI_RUNTIME`, `dev` whether this is a development run (`cargo tauri dev`), `resources` the app's
    /// resource directory and `app_data` its data directory.
    pub(crate) fn choose(env: Option<PathBuf>, dev: bool, resources: &Path, app_data: &Path) -> Self {
        if let Some(dir) = env.filter(|dir| !dir.as_os_str().is_empty()) {
            return Self::Existing(dir);
        }
        if dev {
            return Self::Existing(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime"));
        }
        Self::Bundled {
            resources: resources.join("runtime"),
            runtime: app_data.join("runtime"),
        }
    }

    /// Make the runtime when it is bundled, and return its directory.
    ///
    /// # Errors
    ///
    /// Returns an error naming the path when a directory or link cannot be made, or a file is in the way of a link.
    pub(crate) fn prepare(self) -> Result<PathBuf, String> {
        match self {
            Self::Existing(dir) => Ok(dir),
            Self::Bundled { resources, runtime } => {
                link_bundled(&resources, &runtime)?;
                Ok(runtime)
            }
        }
    }
}

/// Link each read-only part in `resources` from `runtime`, replacing a link that points elsewhere (the app moved or
/// was upgraded). A part the bundle does not have (no `ecomponent`) is skipped.
fn link_bundled(resources: &Path, runtime: &Path) -> Result<(), String> {
    std::fs::create_dir_all(runtime).map_err(|err| format!("could not create {}: {err}", runtime.display()))?;
    for name in READ_ONLY {
        let source = resources.join(name);
        if !source.exists() {
            continue;
        }
        let link = runtime.join(name);
        match std::fs::read_link(&link) {
            Ok(target) if target == source => continue,
            Ok(_) => {
                std::fs::remove_file(&link).map_err(|err| format!("could not replace {}: {err}", link.display()))?
            }
            Err(_) if link.symlink_metadata().is_ok() => {
                return Err(format!(
                    "{} is in the way of the app's {name}; move it aside",
                    link.display()
                ));
            }
            Err(_) => {}
        }
        symlink(&source, &link).map_err(|err| format!("could not link {}: {err}", link.display()))?;
    }
    Ok(())
}

#[cfg(unix)]
fn symlink(source: &Path, link: &Path) -> std::io::Result<()> {
    std::os::unix::fs::symlink(source, link)
}

#[cfg(not(unix))]
fn symlink(source: &Path, link: &Path) -> std::io::Result<()> {
    let _ = (source, link);
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "the bundled runtime is linked on Unix only",
    ))
}

#[cfg(test)]
#[path = "runtime_dir_tests.rs"]
mod tests;
