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

//! Component directories. `depends-on` is one insertion pass.
//!
//! A component may load only on some instance purposes (plans/design-dev-aide.md 3.1): `purpose` on a `component-dir`
//! restricts every component found in it, and `purpose` on a `component` element or a `component.xml` root restricts
//! that component. On another purpose the component is left out with a WARN naming it. A component that loads on every
//! purpose may not depend on a restricted one, and one whose `depends-on` names a component that is not there is left
//! out with an error; the others load.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use moqui_lib::env;
use moqui_lib::node::MNode;

use super::error::Error;

#[derive(Clone, Debug)]
pub struct ComponentInfo {
    pub name: String,
    pub version: String,
    pub location: PathBuf,
    pub depends_on: Vec<String>,
    /// The instance purposes this component loads on (`dev`); empty for every purpose.
    pub purposes: Vec<String>,
}

impl ComponentInfo {
    /// Whether this component loads only on some instance purposes.
    #[must_use]
    pub fn is_restricted(&self) -> bool {
        !self.purposes.is_empty()
    }
}

/// Component entity directories in load order, excluding the framework model already loaded by the facade.
pub fn component_entity_dirs(factory: &super::ExecutionContextFactory) -> Vec<PathBuf> {
    factory
        .components()
        .iter()
        .filter(|info| !same_directory(&info.location, factory.framework_dir()))
        .map(|info| info.location.join("entity"))
        .filter(|dir| dir.is_dir())
        .collect()
}

/// What discovery needs besides the conf: the instance purpose the boot will set, which components and directories
/// with a `purpose` are checked against.
struct Discovery<'a> {
    framework_dir: &'a Path,
    purpose: String,
}

/// The components a factory booted on these directories loads, in load order, without booting it: the merged conf's
/// component list, on the instance purpose the boot would set (a `dev` component loads only on a dev instance). `moqui
/// test` keys its database snapshot by them.
pub fn discover_components(runtime_dir: &Path, framework_dir: &Path) -> Result<Vec<ComponentInfo>, Error> {
    let conf = super::conf::load_merged_conf(framework_dir, runtime_dir)?;
    discover(runtime_dir, framework_dir, &conf)
}

pub(super) fn discover(runtime_dir: &Path, framework_dir: &Path, conf: &MNode) -> Result<Vec<ComponentInfo>, Error> {
    discover_for(
        runtime_dir,
        framework_dir,
        conf,
        effective_instance_purpose(conf, runtime_dir),
    )
}

fn discover_for(
    runtime_dir: &Path,
    framework_dir: &Path,
    conf: &MNode,
    purpose: String,
) -> Result<Vec<ComponentInfo>, Error> {
    let discovery = Discovery { framework_dir, purpose };
    let mut found = ComponentSet::default();
    found.insert(component_from_dir(framework_dir, None)?)?;
    if let Some(list) = conf.first_named("component-list") {
        for child in list.child_iter() {
            match &*child.name_rc() {
                "component" => {
                    let info = component_from_element(runtime_dir, &child)?;
                    if discovery.allows(&info, "its component element or component.xml") {
                        found.insert(info)?;
                    }
                }
                "component-dir" => add_component_dir(&mut found, runtime_dir, &child, &discovery, &[])?,
                _ => {}
            }
        }
    }
    let sorted = sort_components(drop_unmet_dependencies(found.items))?;
    let restricted: Vec<&str> = sorted
        .iter()
        .filter(|info| info.is_restricted())
        .map(|info| info.name.as_str())
        .collect();
    if !restricted.is_empty() {
        tracing::info!(
            "Loaded components restricted to instance purpose {}: {}",
            discovery.purpose,
            restricted.join(", ")
        );
    }
    Ok(sorted)
}

impl Discovery<'_> {
    /// Whether `info` loads on this instance's purpose; a WARN names it when it does not.
    fn allows(&self, info: &ComponentInfo, source: &str) -> bool {
        if !info.is_restricted() || info.purposes.contains(&self.purpose) {
            return true;
        }
        tracing::warn!(
            "Excluding component {} ({}): it loads only when instance_purpose is {} ({source}); this instance is {}",
            info.name,
            info.location.display(),
            info.purposes.join(" or "),
            if self.purpose.is_empty() {
                "unset"
            } else {
                &self.purpose
            },
        );
        false
    }
}

/// The `instance_purpose` the boot will set, read before the default properties are applied (discovery runs first):
/// a process property or environment variable, else `runtime/.env`, else the merged conf's `default-property`. A
/// component's own conf merges after discovery, so it cannot change which components load.
pub(super) fn effective_instance_purpose(conf: &MNode, runtime_dir: &Path) -> String {
    purpose_from(
        env::get_prop_or_env("instance_purpose"),
        std::fs::read_to_string(runtime_dir.join(".env")).ok().as_deref(),
        conf,
    )
}

/// [`effective_instance_purpose`] from what was set (a property or the environment), the `.env` text and the conf.
fn purpose_from(set: String, dotenv: Option<&str>, conf: &MNode) -> String {
    if !set.is_empty() {
        return set;
    }
    if let Some(text) = dotenv {
        let from_env = env::parse_dotenv(text)
            .into_iter()
            .filter(|(key, value)| (key == "instance_purpose" || key == "instance.purpose") && !value.is_empty())
            .map(|(_, value)| value)
            .next();
        if let Some(value) = from_env {
            return value;
        }
    }
    conf.named_iter("default-property")
        .find(|node| node.attribute("name").as_deref() == Some("instance_purpose"))
        .and_then(|node| node.attribute("value"))
        .map(|raw| env::expand(&raw).unwrap_or(raw))
        .unwrap_or_default()
}

/// The comma-separated purposes of a `purpose` attribute; none when it is missing or empty.
fn purposes_of(node: &MNode) -> Vec<String> {
    node.attribute("purpose")
        .map(|raw| {
            raw.split(',')
                .map(str::trim)
                .filter(|purpose| !purpose.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Leave out each component whose `depends-on` names one that is not there, and each that loads on every purpose but
/// depends on a restricted one, with an error naming both; again until none is left out, since leaving one out can
/// break another. The rest load (AGENTS.md: failure tolerance).
fn drop_unmet_dependencies(mut items: Vec<ComponentInfo>) -> Vec<ComponentInfo> {
    loop {
        let restricted: HashMap<&str, bool> = items
            .iter()
            .map(|info| (info.name.as_str(), info.is_restricted()))
            .collect();
        let mut dropped = Vec::new();
        for info in &items {
            for dep in &info.depends_on {
                match restricted.get(dep.as_str()) {
                    None => {
                        tracing::error!(
                            "Leaving out component {}: it depends on component {dep}, which is not initialized",
                            info.name
                        );
                    }
                    Some(true) if !info.is_restricted() => {
                        tracing::error!(
                            "Leaving out component {}: it loads on every instance purpose but depends on component \
                             {dep}, which loads only on some",
                            info.name
                        );
                    }
                    Some(_) => continue,
                }
                dropped.push(info.name.clone());
                break;
            }
        }
        if dropped.is_empty() {
            return items;
        }
        items.retain(|info| !dropped.contains(&info.name));
    }
}

fn add_component_dir(
    found: &mut ComponentSet,
    base: &Path,
    node: &MNode,
    discovery: &Discovery<'_>,
    outer_purposes: &[String],
) -> Result<(), Error> {
    let framework_dir = discovery.framework_dir;
    let mut dir_purposes = purposes_of(node);
    if dir_purposes.is_empty() {
        dir_purposes = outer_purposes.to_vec();
    }
    let location = node
        .attribute("location")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("component-dir location is empty"))?;
    let dir = resolve_location(base, &location);
    if !dir.exists() {
        tracing::info!("Skipping component directory {}, not found", dir.display());
        return Ok(());
    }
    if !dir.is_dir() {
        return Err(Error::new(format!(
            "component directory is not a directory: {}",
            dir.display()
        )));
    }
    let manifest = dir.join("components.xml");
    if manifest.is_file() {
        let list = MNode::parse_file(&manifest)?;
        for child in list.child_iter() {
            match &*child.name_rc() {
                "component" => {
                    let mut info = component_from_element(&dir, &child)?;
                    if !info.is_restricted() {
                        info.purposes.clone_from(&dir_purposes);
                    }
                    if discovery.allows(&info, "its component directory, component element or component.xml") {
                        found.insert(info)?;
                    }
                }
                "component-dir" => add_component_dir(found, &dir, &child, discovery, &dir_purposes)?,
                _ => {}
            }
        }
        return Ok(());
    }
    let mut entries = Vec::new();
    for entry in std::fs::read_dir(&dir)
        .map_err(|err| Error::new(format!("could not read component directory {}", dir.display())).with_source(err))?
    {
        let entry = entry.map_err(|err| {
            Error::new(format!("could not read component directory {}", dir.display())).with_source(err)
        })?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with('.') || !path.is_dir() || !path.join("component.xml").is_file() {
            continue;
        }
        entries.push(path);
    }
    entries.sort_by(|left, right| left.file_name().cmp(&right.file_name()));
    for path in entries {
        // A component in the directory can be off unless a conf names it with a `component` element.
        if disabled_in_manifest(&path)? {
            tracing::info!(
                "Skipping component {}, its component.xml has disabled=true",
                path.display()
            );
            continue;
        }
        let mut info = component_from_dir(&path, None)?;
        let manifest_purposes = std::mem::take(&mut info.purposes);
        // Both marks hold: a component in a restricted directory, and one whose component.xml names purposes, loads
        // only on a purpose each allows.
        info.purposes.clone_from(&dir_purposes);
        if !discovery.allows(&info, &format!("its component directory {}", dir.display())) {
            continue;
        }
        if !manifest_purposes.is_empty() {
            info.purposes = manifest_purposes;
            if !discovery.allows(&info, "its component.xml") {
                continue;
            }
        }
        // The framework component is registered from the boot directory before this scan. base-component
        // contains that directory, and a symlink in a test runtime is the same place with another path.
        // Keep the boot directory so MOQUI_FRAMEWORK is not replaced by the copy inside the runtime.
        if found
            .get(&info.name)
            .is_some_and(|existing| same_directory(&existing.location, framework_dir))
        {
            if !same_directory(&info.location, framework_dir) {
                tracing::info!(
                    component = %info.name,
                    location = %info.location.display(),
                    "Keeping the framework directory; the component directory has another copy"
                );
            }
            continue;
        }
        found.insert(info)?;
    }
    Ok(())
}

/// True when `component.xml` in `path` has a `disabled` attribute that expands to `true`.
fn disabled_in_manifest(path: &Path) -> Result<bool, Error> {
    let manifest = MNode::parse_file(path.join("component.xml"))?;
    match manifest.attribute("disabled") {
        Some(raw) => Ok(env::expand(&raw)?.trim() == "true"),
        None => Ok(false),
    }
}

fn same_directory(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn component_from_element(base: &Path, node: &MNode) -> Result<ComponentInfo, Error> {
    let location = node
        .attribute("location")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new("component location is empty"))?;
    if location.ends_with(".zip") {
        return Err(Error::new(format!("component archives are not expanded: {location}")));
    }
    let path = resolve_location(base, &location);
    component_from_dir(&path, Some(node))
}

fn component_from_dir(path: &Path, fallback: Option<&MNode>) -> Result<ComponentInfo, Error> {
    if !path.exists() {
        return Err(Error::new(format!(
            "Could not find component directory at: {}",
            path.display()
        )));
    }
    if !path.is_dir() {
        return Err(Error::new(format!(
            "Component location is not a directory: {}",
            path.display()
        )));
    }
    let manifest = path.join("component.xml");
    let parsed = if manifest.is_file() {
        Some(MNode::parse_file(&manifest)?)
    } else {
        None
    };
    let node = parsed.as_ref().or(fallback);
    let name = node
        .and_then(|node| node.attribute("name"))
        .filter(|value| !value.is_empty())
        .or_else(|| path.file_name().map(|name| name.to_string_lossy().into_owned()))
        .filter(|value| !value.is_empty())
        .ok_or_else(|| Error::new(format!("component at {} has no name", path.display())))?;
    let version_attr = node
        .and_then(|node| node.attribute("version"))
        .filter(|value| !value.is_empty());
    let version = match version_attr {
        Some(value) => env::expand(&value)?,
        None => "unknown".to_string(),
    };
    let mut depends_on = Vec::new();
    // The element that names the component restricts it, else its component.xml.
    let mut purposes = fallback.map(purposes_of).unwrap_or_default();
    if purposes.is_empty() {
        purposes = parsed.as_ref().map(purposes_of).unwrap_or_default();
    }
    if let Some(node) = node {
        for child in node.named_iter("depends-on") {
            if let Some(dep) = child.attribute("name").filter(|value| !value.is_empty()) {
                depends_on.push(dep);
            }
        }
    }
    Ok(ComponentInfo {
        name,
        version,
        location: path.to_path_buf(),
        depends_on,
        purposes,
    })
}

fn resolve_location(base: &Path, location: &str) -> PathBuf {
    let location = location.trim_end_matches('/');
    let path = Path::new(location);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        base.join(path)
    }
}

#[derive(Default)]
struct ComponentSet {
    items: Vec<ComponentInfo>,
    index: HashMap<String, usize>,
}

impl ComponentSet {
    fn get(&self, name: &str) -> Option<&ComponentInfo> {
        self.index.get(name).map(|&slot| &self.items[slot])
    }

    fn insert(&mut self, info: ComponentInfo) -> Result<(), Error> {
        if let Some(&slot) = self.index.get(&info.name) {
            tracing::warn!(
                component = %info.name,
                location = %info.location.display(),
                "Overriding component because another component of the same name was initialized"
            );
            self.items[slot] = info;
            return Ok(());
        }
        tracing::info!("Added component {:<18} at {}", info.name, info.location.display());
        self.index.insert(info.name.clone(), self.items.len());
        self.items.push(info);
        Ok(())
    }
}

fn sort_components(items: Vec<ComponentInfo>) -> Result<Vec<ComponentInfo>, Error> {
    let order: Vec<String> = items.iter().map(|item| item.name.clone()).collect();
    let mut by_name: HashMap<String, ComponentInfo> = HashMap::new();
    for item in items {
        by_name.insert(item.name.clone(), item);
    }
    let mut sorted_names = Vec::new();
    let mut stack = Vec::new();
    for name in &order {
        let info = by_name
            .get(name)
            .ok_or_else(|| Error::new(format!("component {name} is missing")))?;
        add_with_dependencies(info, &by_name, &mut sorted_names, &mut stack)?;
    }
    check_dependency_order(&sorted_names, &by_name)?;
    let mut sorted = Vec::with_capacity(sorted_names.len());
    for name in sorted_names {
        let info = by_name
            .remove(&name)
            .ok_or_else(|| Error::new(format!("component {name} is missing")))?;
        sorted.push(info);
    }
    tracing::info!("Components after depends-on sort: {}", names_of(&sorted));
    Ok(sorted)
}

fn names_of(items: &[ComponentInfo]) -> String {
    let mut out = String::new();
    for (index, item) in items.iter().enumerate() {
        if index > 0 {
            out.push_str(", ");
        }
        out.push_str(&item.name);
    }
    out
}

fn add_with_dependencies(
    info: &ComponentInfo,
    by_name: &HashMap<String, ComponentInfo>,
    sorted: &mut Vec<String>,
    stack: &mut Vec<String>,
) -> Result<(), Error> {
    if sorted.iter().any(|name| name == &info.name) {
        return Ok(());
    }
    // A cycle is an error. The Java walk recurses until the stack overflows.
    if stack.iter().any(|name| name == &info.name) {
        return Err(Error::new(format!(
            "component dependency cycle involving {}",
            info.name
        )));
    }
    stack.push(info.name.clone());
    for dep_name in &info.depends_on {
        let dep = by_name.get(dep_name).ok_or_else(|| {
            Error::new(format!(
                "Component {} depends on component {dep_name} which is not initialized",
                info.name
            ))
        })?;
        add_with_dependencies(dep, by_name, sorted, stack)?;
    }
    stack.pop();
    if !sorted.iter().any(|name| name == &info.name) {
        sorted.push(info.name.clone());
    }
    Ok(())
}

fn check_dependency_order(sorted: &[String], by_name: &HashMap<String, ComponentInfo>) -> Result<(), Error> {
    let mut messages = Vec::new();
    for (index, name) in sorted.iter().enumerate() {
        let Some(info) = by_name.get(name) else {
            continue;
        };
        for dep in &info.depends_on {
            if let Some(dep_index) = sorted.iter().position(|item| item == dep) {
                if dep_index > index {
                    messages.push(format!(
                        "Broken dependency order after initial pass: [{dep}] is after [{name}]"
                    ));
                }
            }
        }
    }
    if messages.is_empty() {
        Ok(())
    } else {
        Err(Error::new(messages.join(" ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Scratch(PathBuf);

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn scratch(label: &str) -> Scratch {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("moqui-component-{label}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        Scratch(path)
    }

    fn component(dir: &Path, name: &str, attributes: &str, depends_on: &[&str]) {
        std::fs::create_dir_all(dir.join(name)).unwrap();
        let deps: String = depends_on
            .iter()
            .map(|dep| format!("<depends-on name=\"{dep}\"/>"))
            .collect();
        std::fs::write(
            dir.join(name).join("component.xml"),
            format!("<component name=\"{name}\" {attributes}>{deps}</component>"),
        )
        .unwrap();
    }

    /// A runtime with a framework, `component` (every purpose) and `dev-component` (dev only) directories.
    fn runtime(label: &str) -> (Scratch, MNode) {
        let root = scratch(label);
        component(&root.0, "framework", "", &[]);
        std::fs::create_dir_all(root.0.join("component")).unwrap();
        std::fs::create_dir_all(root.0.join("dev-component")).unwrap();
        let conf = MNode::parse_text(
            "conf",
            r#"<moqui-conf><component-list><component-dir location="component"/>
                <component-dir location="dev-component" purpose="dev"/></component-list></moqui-conf>"#,
        )
        .unwrap();
        (root, conf)
    }

    fn names(root: &Path, conf: &MNode, purpose: &str) -> Vec<String> {
        discover_for(root, &root.join("framework"), conf, purpose.to_string())
            .unwrap()
            .into_iter()
            .map(|info| info.name)
            .collect()
    }

    #[test]
    fn a_dev_directory_loads_only_on_dev() {
        let (root, conf) = runtime("dir");
        component(&root.0.join("component"), "app", "", &[]);
        component(&root.0.join("dev-component"), "tools-dev", "", &[]);
        assert_eq!(names(&root.0, &conf, "dev"), ["framework", "app", "tools-dev"]);
        assert_eq!(names(&root.0, &conf, "production"), ["framework", "app"]);
        let restricted = discover_for(&root.0, &root.0.join("framework"), &conf, "dev".into()).unwrap();
        assert!(restricted
            .iter()
            .find(|info| info.name == "tools-dev")
            .unwrap()
            .is_restricted());
    }

    #[test]
    fn a_component_xml_purpose_holds_in_any_directory() {
        let (root, conf) = runtime("manifest");
        component(&root.0.join("component"), "copied-dev", r#"purpose="dev""#, &[]);
        component(&root.0.join("component"), "either", r#"purpose="dev, test""#, &[]);
        assert_eq!(names(&root.0, &conf, "production"), ["framework"]);
        assert_eq!(names(&root.0, &conf, "test"), ["framework", "either"]);
        assert_eq!(names(&root.0, &conf, "dev"), ["framework", "copied-dev", "either"]);
    }

    #[test]
    fn a_component_element_naming_a_dev_component_is_left_out_elsewhere() {
        let (root, _) = runtime("element");
        component(&root.0, "named-dev", r#"purpose="dev""#, &[]);
        let conf = MNode::parse_text(
            "conf",
            r#"<moqui-conf><component-list><component location="named-dev"/></component-list></moqui-conf>"#,
        )
        .unwrap();
        assert_eq!(names(&root.0, &conf, "production"), ["framework"]);
        assert_eq!(names(&root.0, &conf, "dev"), ["framework", "named-dev"]);
    }

    #[test]
    fn unmet_dependencies_leave_out_the_dependent_and_the_rest_load() {
        let (root, conf) = runtime("depends");
        component(&root.0.join("dev-component"), "dev-lib", "", &[]);
        component(&root.0.join("component"), "uses-dev", "", &["dev-lib"]);
        component(&root.0.join("component"), "uses-missing", "", &["nowhere"]);
        component(&root.0.join("component"), "uses-dropped", "", &["uses-missing"]);
        component(&root.0.join("component"), "plain", "", &[]);
        component(&root.0.join("dev-component"), "dev-uses-app", "", &["plain"]);
        // On dev the dev library loads, but a component for every purpose may not depend on it.
        assert_eq!(
            names(&root.0, &conf, "dev"),
            ["framework", "plain", "dev-lib", "dev-uses-app"]
        );
        assert_eq!(names(&root.0, &conf, "production"), ["framework", "plain"]);
    }

    #[test]
    fn the_purpose_is_set_then_dotenv_then_conf() {
        let conf = MNode::parse_text(
            "conf",
            r#"<moqui-conf><default-property name="instance_purpose" value="production"/></moqui-conf>"#,
        )
        .unwrap();
        assert_eq!(purpose_from("test".into(), Some("instance_purpose=dev"), &conf), "test");
        assert_eq!(
            purpose_from(String::new(), Some("# x\ninstance_purpose='dev'\n"), &conf),
            "dev"
        );
        assert_eq!(purpose_from(String::new(), Some("other=1"), &conf), "production");
        assert_eq!(purpose_from(String::new(), None, &conf), "production");
        let empty = MNode::parse_text("conf", "<moqui-conf/>").unwrap();
        assert_eq!(purpose_from(String::new(), None, &empty), "");
    }
}
