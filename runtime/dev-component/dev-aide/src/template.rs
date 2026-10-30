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
//! `runtime/dev-component/template` and the dev-aide's Site template. The rules for the component template, its
//! placeholders and its parts, are in `runtime/dev-component/README.md`; `./moq component-create` follows the same ones.

use std::path::Path;

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

/// What a new component from the component template has: all of it, its screens only, or its REST API only.
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
            ],
            Part::Screens => &[
                "service/@component@.rest.xml",
                "data/@Class@ApiSecurityData.xml",
                "test/@Class@Api.spec.js",
            ],
        }
    }
}

/// A component named `name` in `target` from the component template in `template`: its files with the placeholders
/// filled in, less what `part` leaves out, and without the template's `disabled="true"`.
pub(crate) fn create(template: &Path, target: &Path, name: &str, part: Part) -> Result<(), LibError> {
    let names = Names::of(name);
    let fill = |text: &str| names.fill(text);
    copy_template(template, target, &fill, &fill)?;
    for path in part.left_out() {
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
