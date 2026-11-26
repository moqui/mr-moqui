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

//! Overture's STAC catalog (plans/Maps-Overture.md §3.1), read with no network of its own: the root's releases
//! ([`releases`]), a release's schema version and themes ([`release`]), a theme's collection of a type ([`theme`]),
//! its items and the extent of each ([`collection`]), and an item's GeoParquet asset ([`item`]). Links are resolved
//! against the document they are in; the caller fetches each document and keeps every one on the catalog's origin.

use serde_json::Value;

/// A box: west, south, east, north, in degrees.
pub type Bbox = [f64; 4];

/// Whether two boxes meet (edges included).
#[must_use]
pub fn meets(a: &Bbox, b: &Bbox) -> bool {
    a[0] <= b[2] && b[0] <= a[2] && a[1] <= b[3] && b[1] <= a[3]
}

fn bbox_of(value: &Value) -> Option<Bbox> {
    let list = value.as_array()?;
    let number = |index: usize| list.get(index).and_then(Value::as_f64).filter(|n| n.is_finite());
    // a 3D box is [w, s, minz, e, n, maxz]
    let (w, s, e, n) = if list.len() == 6 {
        (number(0)?, number(1)?, number(3)?, number(4)?)
    } else {
        (number(0)?, number(1)?, number(2)?, number(3)?)
    };
    Some([w, s, e, n])
}

/// `href` resolved against the URL of the document it is in.
#[must_use]
pub fn resolve(base: &str, href: &str) -> String {
    if href.starts_with("https://") || href.starts_with("http://") {
        return href.to_string();
    }
    let (origin, path) = match base.split_once("://").map(|(scheme, rest)| {
        let (host, path) = rest.split_once('/').unwrap_or((rest, ""));
        (format!("{scheme}://{host}"), path.to_string())
    }) {
        Some(parts) => parts,
        None => return href.to_string(),
    };
    if let Some(absolute) = href.strip_prefix('/') {
        return format!("{origin}/{absolute}");
    }
    let path = path.split(['?', '#']).next().unwrap_or_default();
    let mut segments: Vec<&str> = path.split('/').collect();
    // the document's own name
    segments.pop();
    for part in href.split('/') {
        match part {
            "." | "" => {}
            ".." => {
                segments.pop();
            }
            other => segments.push(other),
        }
    }
    format!("{origin}/{}", segments.join("/"))
}

/// The scheme and host of `url`, lower case.
#[must_use]
pub fn origin(url: &str) -> Option<String> {
    let (scheme, rest) = url.split_once("://")?;
    let host = rest.split(['/', '?', '#']).next()?;
    (!host.is_empty()).then(|| format!("{}://{}", scheme.to_ascii_lowercase(), host.to_ascii_lowercase()))
}

fn links<'a>(document: &'a Value, rel: &'a str) -> impl Iterator<Item = &'a Value> + 'a {
    document
        .get("links")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(move |link| link.get("rel").and_then(Value::as_str) == Some(rel))
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
}

/// A release the root catalog lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    pub version: String,
    pub href: String,
    pub latest: bool,
}

/// The release version a child link names: its `title`'s first word, else its folder (`.../2026-11-18.0/catalog.json`).
fn release_version(link: &Value) -> Option<String> {
    let from_title = text(link, "title")
        .and_then(|title| title.split_whitespace().next())
        .filter(|word| is_version(word));
    let from_href = text(link, "href").and_then(|href| {
        let mut parts = href.trim_end_matches('/').rsplit('/');
        let last = parts.next()?;
        if last.ends_with(".json") {
            parts.next()
        } else {
            Some(last)
        }
    });
    from_title
        .or(from_href.filter(|part| is_version(part)))
        .map(str::to_string)
}

/// Whether `text` is a release version as Overture writes them (`2026-11-18.0`): digits, dashes and dots.
#[must_use]
pub fn is_version(text: &str) -> bool {
    !text.is_empty()
        && text.len() <= 40
        && text.starts_with(|c: char| c.is_ascii_digit())
        && text.chars().all(|c| c.is_ascii_digit() || c == '-' || c == '.')
}

/// The releases of the root catalog at `url`, the newest first.
#[must_use]
pub fn releases(url: &str, root: &Value) -> Vec<Release> {
    let latest = text(root, "latest");
    let mut out: Vec<Release> = links(root, "child")
        .filter_map(|link| {
            let version = release_version(link)?;
            Some(Release {
                href: resolve(url, text(link, "href")?),
                latest: latest == Some(version.as_str())
                    || link.get("latest").and_then(Value::as_bool).unwrap_or(false),
                version,
            })
        })
        .collect();
    out.sort_by(|a, b| b.version.cmp(&a.version));
    out.dedup_by(|a, b| a.version == b.version);
    out
}

/// A release's catalog: its version, schema version and the theme catalogs by name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseCatalog {
    pub version: String,
    pub schema_version: String,
    pub themes: Vec<(String, String)>,
}

/// Read a release's catalog at `url`.
///
/// # Errors
///
/// No release or schema version.
pub fn release(url: &str, document: &Value) -> Result<ReleaseCatalog, String> {
    let version = text(document, "release:version")
        .or_else(|| text(document, "id"))
        .ok_or_else(|| "the release catalog names no release:version".to_string())?;
    let schema_version = text(document, "schema:version")
        .ok_or_else(|| format!("the release catalog of {version} names no schema:version"))?;
    let themes = links(document, "child")
        .filter_map(|link| Some((text(link, "title")?.to_string(), resolve(url, text(link, "href")?))))
        .collect();
    Ok(ReleaseCatalog {
        version: version.to_string(),
        schema_version: schema_version.to_string(),
        themes,
    })
}

/// The collection of `kind` (`place`, `building`) a theme catalog at `url` lists.
#[must_use]
pub fn theme(url: &str, document: &Value, kind: &str) -> Option<String> {
    links(document, "child")
        .find(|link| {
            text(link, "title") == Some(kind)
                || text(link, "href").is_some_and(|href| href.contains(&format!("/{kind}/")))
        })
        .and_then(|link| Some(resolve(url, text(link, "href")?)))
}

/// An item of a collection: its document and, when the collection's extent gives it, its box.
#[derive(Clone, Debug, PartialEq)]
pub struct ItemRef {
    pub href: String,
    pub bbox: Option<Bbox>,
}

/// A theme's collection: its schema version, columns and items.
#[derive(Clone, Debug, PartialEq)]
pub struct Collection {
    pub schema_version: Option<String>,
    pub columns: Vec<String>,
    pub items: Vec<ItemRef>,
}

/// Read a collection at `url`: its items in link order, item `i` given the extent's box `i + 1` (the first box is the
/// whole collection's) when the extent has one for every item.
#[must_use]
pub fn collection(url: &str, document: &Value) -> Collection {
    let boxes: Vec<Option<Bbox>> = document
        .pointer("/extent/spatial/bbox")
        .and_then(Value::as_array)
        .map(|list| list.iter().map(bbox_of).collect())
        .unwrap_or_default();
    let hrefs: Vec<String> = links(document, "item")
        .filter_map(|link| Some(resolve(url, text(link, "href")?)))
        .collect();
    let per_item = boxes.len() == hrefs.len() + 1;
    let items = hrefs
        .into_iter()
        .enumerate()
        .map(|(index, href)| ItemRef {
            href,
            bbox: if per_item {
                boxes.get(index + 1).copied().flatten()
            } else {
                None
            },
        })
        .collect();
    let columns = document
        .get("table:columns")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|column| text(column, "name").map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    Collection {
        schema_version: text(document, "schema:version").map(str::to_string),
        columns,
        items,
    }
}

/// An item: its box and its GeoParquet file.
#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub bbox: Bbox,
    pub href: String,
    pub size: Option<u64>,
}

/// Read an item at `url`: its box and the https asset of the Parquet file (the `aws` asset first, else the first of
/// type `application/vnd.apache.parquet` with the role `data`).
///
/// # Errors
///
/// No box or no Parquet asset over https.
pub fn item(url: &str, document: &Value) -> Result<Item, String> {
    let bbox = document
        .get("bbox")
        .and_then(bbox_of)
        .ok_or_else(|| format!("the item {url} has no bbox"))?;
    let assets = document
        .get("assets")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("the item {url} has no assets"))?;
    let parquet = |asset: &Value| {
        let href = text(asset, "href")?;
        let typed = text(asset, "type").is_none_or(|kind| kind.contains("parquet"));
        (typed && resolve(url, href).starts_with("https://")).then(|| resolve(url, href))
    };
    let chosen = assets
        .get("aws")
        .and_then(|asset| parquet(asset).map(|href| (href, asset)))
        .or_else(|| {
            assets
                .values()
                .find_map(|asset| parquet(asset).map(|href| (href, asset)))
        })
        .ok_or_else(|| format!("the item {url} has no Parquet file over https"))?;
    Ok(Item {
        bbox,
        href: chosen.0,
        size: chosen.1.get("file:size").and_then(Value::as_u64),
    })
}

/// Whether the schema version (`2.0.0`) is one this reader takes: major version 2.
#[must_use]
pub fn schema_supported(version: &str) -> bool {
    version.split('.').next() == Some("2")
}

#[cfg(test)]
#[path = "stac_tests.rs"]
mod tests;
