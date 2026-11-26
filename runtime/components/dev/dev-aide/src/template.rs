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

//! New components from a template (plans/design-dev-aide.md 12): the component template in
//! `runtime/components/dev/template` (the App kind, and the other kinds in its `kinds/`) and the dev-aide's Site
//! template. The rules for the component template, its placeholders, kinds and parts, are in
//! `runtime/components/dev/README.md`; `./moq component-create` follows the same ones.

use std::path::{Path, PathBuf};

use moqui_lib::error::Error as LibError;

/// The forms of a component name that the component template's placeholders stand for: for `my-shop`, `@component@`
/// my-shop, `@Title@` My Shop, `@Class@` MyShop, `@package@` myshop and `@ID@` MY_SHOP.
pub(crate) struct Names {
    component: String,
    title: String,
    class: String,
    package: String,
    id: String,
}

impl Names {
    pub(crate) fn of(name: &str) -> Self {
        let words: Vec<String> = name
            .split(['-', '_'])
            .filter(|part| !part.is_empty())
            .map(|part| {
                let mut chars = part.chars();
                chars.next().map_or_else(String::new, |first| {
                    first.to_ascii_uppercase().to_string() + chars.as_str()
                })
            })
            .collect();
        Names {
            component: name.to_string(),
            title: words.join(" "),
            class: words.concat(),
            package: name
                .chars()
                .filter(|ch| *ch != '-' && *ch != '_')
                .collect::<String>()
                .to_ascii_lowercase(),
            id: name.to_ascii_uppercase().replace('-', "_"),
        }
    }

    /// `text` with each placeholder filled in.
    pub(crate) fn fill(&self, text: &str) -> String {
        text.replace("@component@", &self.component)
            .replace("@Title@", &self.title)
            .replace("@Class@", &self.class)
            .replace("@package@", &self.package)
            .replace("@ID@", &self.id)
    }
}

/// Whether `name` can be a new component's name: a letter, then letters, digits, `-` and `_`, so each of its forms is
/// an XML name, a package and a path in a URL.
pub(crate) fn valid_name(name: &str) -> bool {
    name.chars().next().is_some_and(|ch| ch.is_ascii_alphabetic())
        && name
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '-' || ch == '_')
}

/// The component directories under `components` (`runtime/components`): each set's directories with a `component.xml`,
/// the component template left out.
fn component_dirs(components: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let Ok(sets) = std::fs::read_dir(components) else {
        return dirs;
    };
    for set in sets.flatten() {
        let Ok(entries) = std::fs::read_dir(set.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let dir = entry.path();
            if entry.file_name() != "template" && dir.join("component.xml").is_file() {
                dirs.push(dir);
            }
        }
    }
    dirs.sort();
    dirs
}

/// The first segments of the service and entity packages of the components under `components`, lower case: each
/// `service/<segment>/` directory and each `package="<segment>...` of a top-level `entity/*.xml`. A new component
/// whose `@package@` is one of them would put its services or entities in another's package (`org`, `moqui`,
/// `mantle`), and its artifact group would grant theirs.
pub(crate) fn taken_packages(components: &Path) -> std::collections::BTreeSet<String> {
    static PACKAGE: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let package = PACKAGE.get_or_init(|| regex::Regex::new(r#"package="([^".]*)"#).ok());
    let mut taken = std::collections::BTreeSet::new();
    for dir in component_dirs(components) {
        for entry in std::fs::read_dir(dir.join("service")).into_iter().flatten().flatten() {
            if entry.path().is_dir() {
                taken.insert(entry.file_name().to_string_lossy().to_ascii_lowercase());
            }
        }
        for entry in std::fs::read_dir(dir.join("entity")).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "xml") {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                for found in package.iter().flat_map(|package| package.captures_iter(&text)) {
                    taken.insert(found[1].to_ascii_lowercase());
                }
            }
        }
    }
    taken.remove("");
    taken
}

/// The first names of the webroot's paths, lower case: the files and directories of `webroot/screen/webroot/` (a
/// file's name up to its first `.`), and each `subscreens-item` name of `webroot.xml` and of a component's
/// `MoquiConf.xml` that mounts under it. A theme is served at `/<its name>/`, so it may not take one.
pub(crate) fn webroot_names(components: &Path) -> std::collections::BTreeSet<String> {
    static ITEM: std::sync::OnceLock<Option<regex::Regex>> = std::sync::OnceLock::new();
    let item = ITEM.get_or_init(|| regex::Regex::new(r#"subscreens-item name="([^"]*)""#).ok());
    let mut names = std::collections::BTreeSet::new();
    let screen = components.join("base/webroot/screen");
    for entry in std::fs::read_dir(screen.join("webroot"))
        .into_iter()
        .flatten()
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        names.insert(name.split('.').next().unwrap_or_default().to_ascii_lowercase());
    }
    let mut texts = vec![std::fs::read_to_string(screen.join("webroot.xml")).unwrap_or_default()];
    for dir in component_dirs(components) {
        let text = std::fs::read_to_string(dir.join("MoquiConf.xml")).unwrap_or_default();
        if text.contains("component://webroot/screen/webroot.xml\"") {
            texts.push(text);
        }
    }
    for text in &texts {
        for found in item.iter().flat_map(|item| item.captures_iter(text)) {
            names.insert(found[1].to_ascii_lowercase());
        }
    }
    names.remove("");
    names
}

/// Why `name` may not be a new component of `kind` beside the components under `components`, if it may not: its
/// package is another's ([`taken_packages`]), or a theme's name is a webroot path ([`webroot_names`]).
/// `./moq component-create` refuses the same (`template_name_refusal`).
pub(crate) fn name_refusal(components: &Path, name: &str, kind: Kind) -> Option<String> {
    let package = Names::of(name).package;
    if taken_packages(components).contains(&package) {
        return Some(format!(
            "{name} would put its services and entities in the package {package}, which another component's are in"
        ));
    }
    if kind == Kind::Theme && webroot_names(components).contains(&name.to_ascii_lowercase()) {
        return Some(format!("a theme is served at /{name}/, a path the webroot has already"));
    }
    None
}

/// What an App component from the component template has: all of it, its screens only, or its REST API only.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Part {
    All,
    Screens,
    Rest,
}

impl Part {
    /// `all` (or empty), `screens` or `rest`.
    pub(crate) fn of(text: &str) -> Option<Part> {
        match text {
            "" | "all" => Some(Part::All),
            "screens" => Some(Part::Screens),
            "rest" => Some(Part::Rest),
            _ => None,
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Part::All => "all",
            Part::Screens => "screens",
            Part::Rest => "rest",
        }
    }

    /// The template paths a component of this part leaves out.
    fn left_out(self) -> &'static [&'static str] {
        match self {
            Part::All => &[],
            Part::Rest => &[
                "screen",
                "MoquiConf.xml",
                "data/@Class@SecurityData.xml",
                "test/@Class@Screens.spec.js",
                "test/acceptance",
            ],
            Part::Screens => &[
                "service/@component@.rest.xml",
                "data/@Class@ApiSecurityData.xml",
                "test/@Class@Api.spec.js",
            ],
        }
    }
}

/// The directory of the component template that holds the kinds other than App, each a template of its own
/// (`kinds/<kind>`), which an App component leaves out.
pub(crate) const KINDS_DIR: &str = "kinds";

/// The kind of a new component (plans/design-dev-aide.md 12): an App (the template itself, with its part), data only,
/// native (a Rust crate `./moq build` links), a theme of the `/qapps` shell, a skill pack, or a dev tool (in
/// `runtime/components/dev`, `purpose="dev"`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Kind {
    App(Part),
    Data,
    Native,
    Theme,
    Skills,
    DevTool,
}

impl Kind {
    /// Every kind other than App, as `kinds/<name>` of the template holds them.
    #[cfg(test)]
    pub(crate) const OTHERS: [Kind; 5] = [Kind::Data, Kind::Native, Kind::Theme, Kind::Skills, Kind::DevTool];

    /// By name: `app` (or empty) with its `part`; a part's name alone (`all`, `screens`, `rest`) for an App; or `data`,
    /// `native`, `theme`, `skills` or `dev-tool`, which take no part.
    pub(crate) fn of(kind: &str, part: &str) -> Option<Kind> {
        let partless = |kind: Kind| (part.is_empty() || part == "all").then_some(kind);
        match kind {
            "" | "app" => Part::of(part).map(Kind::App),
            "data" => partless(Kind::Data),
            "native" => partless(Kind::Native),
            "theme" => partless(Kind::Theme),
            "skills" => partless(Kind::Skills),
            "dev-tool" => partless(Kind::DevTool),
            other => Part::of(other)
                .filter(|_| part.is_empty() || part == other)
                .map(Kind::App),
        }
    }

    pub(crate) fn name(self) -> &'static str {
        match self {
            Kind::App(_) => "app",
            Kind::Data => "data",
            Kind::Native => "native",
            Kind::Theme => "theme",
            Kind::Skills => "skills",
            Kind::DevTool => "dev-tool",
        }
    }

    /// Whether a component of this kind goes in `runtime/components/dev` wherever it is asked for.
    pub(crate) fn dev_only(self) -> bool {
        self == Kind::DevTool
    }

    /// The template directory a component of this kind is copied from: the template itself for an App, else its
    /// `kinds/<name>`.
    pub(crate) fn source(self, template: &Path) -> PathBuf {
        match self {
            Kind::App(_) => template.to_path_buf(),
            other => template.join(KINDS_DIR).join(other.name()),
        }
    }

    /// The paths of [`Self::source`] a component of this kind leaves out: an App's part, and the other kinds.
    fn left_out(self) -> Vec<&'static str> {
        match self {
            Kind::App(part) => part.left_out().iter().copied().chain([KINDS_DIR]).collect(),
            _ => Vec::new(),
        }
    }
}

/// A component named `name` in `target` from the component template in `template`: the files of the kind's template
/// with the placeholders filled in, less what the kind leaves out, and without the template's `disabled="true"`.
pub(crate) fn create(template: &Path, target: &Path, name: &str, kind: Kind) -> Result<(), LibError> {
    let names = Names::of(name);
    let fill = |text: &str| names.fill(text);
    let source = kind.source(template);
    if !source.join("component.xml").is_file() {
        return Err(LibError::new(format!(
            "the component template has no {} kind: {} is not there",
            kind.name(),
            source.display()
        )));
    }
    copy_template(&source, target, &fill, &fill)?;
    for path in kind.left_out() {
        let path = target.join(names.fill(path));
        let removed = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        removed.map_err(|err| LibError::new(format!("{}: {err}", path.display())))?;
    }
    // The template's own disabled="true", which keeps it from loading as a component.
    let manifest = target.join("component.xml");
    let text =
        std::fs::read_to_string(&manifest).map_err(|err| LibError::new(format!("{}: {err}", manifest.display())))?;
    crate::workspace::unfenced(
        moqui_core::entity::fence::Effect::Resource,
        &manifest.display().to_string(),
    )?;
    std::fs::write(&manifest, text.replace(" disabled=\"true\"", ""))
        .map_err(|err| LibError::new(format!("{}: {err}", manifest.display())))
}

/// A copy of the template directory `from` in `to`, each file name through `file_name` and each file's text through
/// `content`. The template's files are text.
pub(crate) fn copy_template(
    from: &Path,
    to: &Path,
    content: &dyn Fn(&str) -> String,
    file_name: &dyn Fn(&str) -> String,
) -> Result<(), LibError> {
    crate::workspace::unfenced(moqui_core::entity::fence::Effect::Resource, &to.display().to_string())?;
    std::fs::create_dir_all(to).map_err(|err| LibError::new(format!("{}: {err}", to.display())))?;
    let entries = std::fs::read_dir(from).map_err(|err| LibError::new(format!("{}: {err}", from.display())))?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let target = to.join(file_name(&name));
        if entry.path().is_dir() {
            copy_template(&entry.path(), &target, content, file_name)?;
        } else {
            let text = std::fs::read_to_string(entry.path()).map_err(|err| LibError::new(err.to_string()))?;
            std::fs::write(&target, content(&text))
                .map_err(|err| LibError::new(format!("{}: {err}", target.display())))?;
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "template_tests.rs"]
mod tests;
