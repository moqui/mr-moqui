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

//! File paths for `classpath://`, `component://`, and `file:` locations.
//!
//! The roots come from the entered factory, or from the last factory that booted when no context
//! is entered, as `moqui lint` runs. With neither, the process working directory is the root, the
//! way a Moqui install runs from its own directory.
//!
//! `classpath://` is a compatibility alias for `service/`, `entity/`, `data/`, `screen/`, and
//! `template/` under the framework component. New locations use `component://framework/`.

use std::path::{Path, PathBuf};
use std::sync::RwLock;

use super::{Error, ExecutionContext};

#[derive(Clone)]
struct Roots {
    framework: PathBuf,
    components: Vec<(String, PathBuf)>,
}

static LAST_BOOTED: RwLock<Option<Roots>> = RwLock::new(None);

pub(super) fn remember(framework: &Path, components: &[super::ComponentInfo]) {
    let roots = Roots {
        framework: framework.to_path_buf(),
        components: components
            .iter()
            .map(|component| (component.name.clone(), component.location.clone()))
            .collect(),
    };
    if let Ok(mut slot) = LAST_BOOTED.write() {
        *slot = Some(roots);
    }
}

fn roots() -> Roots {
    let entered = ExecutionContext::with_current(|ec| Roots {
        framework: ec.factory().framework_dir().to_path_buf(),
        components: ec
            .factory()
            .components()
            .iter()
            .map(|component| (component.name.clone(), component.location.clone()))
            .collect(),
    });
    if let Some(roots) = entered {
        return roots;
    }
    if let Some(roots) = LAST_BOOTED.read().ok().and_then(|slot| slot.clone()) {
        return roots;
    }
    Roots {
        framework: fallback_root().join("runtime/base-component/framework"),
        components: Vec::new(),
    }
}

#[cfg(test)]
fn fallback_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[cfg(not(test))]
fn fallback_root() -> PathBuf {
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// The framework directory.
pub fn framework_dir() -> PathBuf {
    roots().framework
}

/// A local path for a `classpath://`, `component://`, `file:`, or plain location.
///
/// `classpath://` is the compatibility alias described on this module.
///
/// # Errors
///
/// Returns an error when a `component://` location has no name or names an unknown component, and
/// for a `content://` location: JCR is not part of this port, and the location is not remapped.
pub fn locate(location: &str) -> Result<PathBuf, Error> {
    if location.starts_with("content:") {
        return Err(Error::new(format!("content resource is not available: {location}")));
    }
    let roots = roots();
    if let Some(rest) = strip_prefix(location, &["classpath://", "classpath:"]) {
        return Ok(roots.framework.join(rest.trim_start_matches('/')));
    }
    if let Some(rest) = strip_prefix(location, &["component://", "component:"]) {
        let rest = rest.trim_start_matches('/');
        let (name, relative) = match rest.split_once('/') {
            Some((name, relative)) => (name, relative.trim_start_matches('/')),
            None => (rest, ""),
        };
        if name.is_empty() {
            return Err(Error::new(format!("component location has no name: {location}")));
        }
        let root = component_root(&roots, name).ok_or_else(|| Error::new(format!("component {name} was not found")))?;
        return Ok(if relative.is_empty() { root } else { root.join(relative) });
    }
    if location.starts_with("file:") {
        return file_path(location);
    }
    Ok(PathBuf::from(location))
}

/// Resolve a local file location without turning a relative directory into an authority.
pub(super) fn file_path(location: &str) -> Result<PathBuf, Error> {
    let Some(rest) = location.strip_prefix("file:") else {
        return Ok(PathBuf::from(location));
    };
    let path = match rest.strip_prefix("//") {
        Some(rest) => {
            let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
            if !host.is_empty() && !host.eq_ignore_ascii_case("localhost") {
                return Err(Error::new(format!(
                    "file resource has a non-local authority: {location}"
                )));
            }
            if path.is_empty() {
                "/".to_string()
            } else {
                format!("/{path}")
            }
        }
        None => rest.to_string(),
    };
    Ok(PathBuf::from(path))
}

fn component_root(roots: &Roots, name: &str) -> Option<PathBuf> {
    if let Some((_, location)) = roots.components.iter().find(|(known, _)| known == name) {
        return Some(location.clone());
    }
    // Not discovered. The framework component lives in runtime/base-component, beside tools and
    // webroot, and app components live in runtime/component. An external framework directory whose
    // parent is still the install root keeps the older runtime/component lookup.
    let parent = roots.framework.parent()?;
    let mut candidates = vec![parent.join(name), parent.join("runtime/component").join(name)];
    if let Some(runtime) = parent.parent() {
        candidates.insert(1, runtime.join("component").join(name));
    }
    candidates.into_iter().find(|path| path.is_dir())
}

fn strip_prefix<'a>(location: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    prefixes.iter().find_map(|prefix| location.strip_prefix(prefix))
}

#[cfg(test)]
mod tests {
    #[test]
    fn file_locations_preserve_relative_paths_and_only_accept_local_authorities() {
        for (location, expected) in [
            ("file:runtime/conf", "runtime/conf"),
            ("file:runtime", "runtime"),
            ("file:/tmp/a", "/tmp/a"),
            ("file:///tmp/a", "/tmp/a"),
            ("file://localhost/tmp/a", "/tmp/a"),
            ("file://LOCALHOST/tmp/a", "/tmp/a"),
        ] {
            assert_eq!(super::locate(location).unwrap(), std::path::Path::new(expected));
        }
        assert!(super::locate("file://remote/tmp/a").is_err());
    }
    #[test]
    fn email_template_seeds_keep_java_classpath_uris() {
        let framework = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
        let seed = moqui_lib::node::MNode::parse_file(framework.join("data/MoquiSetupData.xml")).unwrap();
        let templates = seed.children_named("moqui.basic.email.EmailTemplate");
        let expected = [
            ("PASSWORD_RESET", "PasswordReset.xml"),
            ("SINGLE_USE_CODE", "SingleUseCode.xml"),
            ("ADDED_EMAIL_AUTHC_FACTOR", "AddedEmailAuthcFactor.xml"),
            ("EMAIL_AUTHC_FACTOR_SENT", "EmailAuthcFActorSent.xml"),
            ("NOTIFICATION", "NotificationEmail.xml"),
            ("NOTIFICATION_DIGEST", "NotificationDigestEmail.xml"),
            ("SCREEN_RENDER", "ScreenRenderEmail.xml"),
        ];
        assert_eq!(templates.len(), expected.len());
        for (id, file) in expected {
            let template = templates
                .iter()
                .find(|node| node.attribute_is("emailTemplateId", id))
                .unwrap();
            let location = template.attribute("bodyScreenLocation").unwrap();
            assert_eq!(location, format!("classpath://screen/{file}"));
            assert_eq!(
                super::locate(&location).unwrap(),
                super::locate(&format!("component://framework/screen/{file}")).unwrap()
            );
        }
    }

    #[test]
    fn a_content_location_is_an_error_that_names_it() {
        let err = super::locate("content://main/WikiSpace/Page.md").unwrap_err();
        assert_eq!(
            err.message(),
            "content resource is not available: content://main/WikiSpace/Page.md"
        );
        assert!(super::locate("component://framework/screen/ScreenRenderEmail.xml").is_ok());
    }
}
