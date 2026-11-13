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

//! Where the visual views write an extension (D17): the site component's file of a kind, the one already extending
//! what is edited when there is one, else the component's own (`entity/MySiteEntities.xml`,
//! `data/MySiteSeedData.xml`, `screen-extend/<path>`, `service/MySite.secas.xml`), made on the view's first save.

use std::path::Path;

use serde::Serialize;
use serde_json::{json, Value as Json};

use crate::roots::{Root, RootComponent};
use crate::workspace::Context;

/// A file of the site component, by its root and its path in it.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SiteFile {
    pub root_id: String,
    pub path: String,
    pub component: String,
    pub exists: bool,
}

impl SiteFile {
    pub(crate) fn json(&self) -> Json {
        serde_json::to_value(self).unwrap_or(Json::Null)
    }
}

/// The default site component, with its root.
pub(crate) fn site_component(context: &Context) -> Option<(&Root, &RootComponent)> {
    let name = context.settings.default_site()?;
    context.roots.iter().find_map(|root| {
        root.components
            .iter()
            .find(|component| component.site && component.name == name)
            .map(|component| (root, component))
    })
}

/// `my-site` as `MySite`, as the Site template names its files.
pub(crate) fn class_of(name: &str) -> String {
    name.split(['-', '_'])
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars.next().map_or_else(String::new, |first| {
                first.to_ascii_uppercase().to_string() + chars.as_str()
            })
        })
        .collect()
}

/// The site's file in `dir` (relative to the component) to write into: the first whose text has `mentions`, else
/// `default_name`, else the first file whose name ends with `suffix`, else `default_name` made new.
pub(crate) fn site_file(
    context: &Context,
    dir: &str,
    default_name: &str,
    suffix: &str,
    mentions: Option<&str>,
) -> Option<SiteFile> {
    let (root, component) = site_component(context)?;
    let base = if component.path.is_empty() {
        dir.to_string()
    } else {
        format!("{}/{dir}", component.path)
    };
    let absolute = root.path.join(&base);
    let mut names: Vec<String> = std::fs::read_dir(&absolute)
        .map(|entries| {
            entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_file())
                .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
                .filter(|name| name.ends_with(suffix))
                .collect()
        })
        .unwrap_or_default();
    names.sort();
    let chosen = mentions
        .and_then(|needle| {
            names.iter().find(|name| {
                std::fs::read_to_string(absolute.join(name.as_str())).is_ok_and(|text| text.contains(needle))
            })
        })
        .or_else(|| names.iter().find(|name| name.as_str() == default_name))
        .or_else(|| names.first())
        .cloned();
    let (name, exists) = match chosen {
        Some(name) => (name, true),
        None => (default_name.to_string(), false),
    };
    Some(SiteFile {
        root_id: root.id.clone(),
        path: format!("{base}/{name}"),
        component: component.name.clone(),
        exists,
    })
}

/// The site's file at `relative` in the component (`screen-extend/example/screen/Gallery/ExampleDetail.xml`).
pub(crate) fn site_path(context: &Context, relative: &str) -> Option<SiteFile> {
    let (root, component) = site_component(context)?;
    let path = if component.path.is_empty() {
        relative.to_string()
    } else {
        format!("{}/{relative}", component.path)
    };
    Some(SiteFile {
        root_id: root.id.clone(),
        exists: root.path.join(&path).is_file(),
        path,
        component: component.name.clone(),
    })
}

/// Where a file at `path` opens in the Workspace: its root and its path there, and whether it is the site's.
pub(crate) fn located(context: &Context, path: &str) -> Json {
    match crate::roots::locate(&context.roots, Path::new(path)) {
        Some((root, relative)) => json!({
            "rootId": root.id,
            "path": relative,
            "site": root.is_site_path(&relative),
            "component": root.component_of(&relative).map(|component| component.name.clone()),
        }),
        None => json!({ "path": path, "site": false }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_site_name_is_its_class() {
        assert_eq!(class_of("my-site"), "MySite");
        assert_eq!(class_of("acme_site"), "AcmeSite");
    }
}
