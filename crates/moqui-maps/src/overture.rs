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

//! Overture's themes in its schema 2.x (plans/Maps-Overture.md §3.2): the columns each theme reads, and a region read
//! from one GeoParquet file ([`read_region`]): the row groups whose `bbox` statistics meet the region, only the
//! projected columns of each, one column at a time, and the rows whose own box meets it, each a [`Place`] or a
//! [`Building`] handed to the caller with its source datasets and licenses. A row with no ID or a geometry that does not
//! read is counted as rejected and the rest go on. The caller is asked between row groups whether to go on.

use std::collections::BTreeMap;

use moqui_lib::error::Error;

use crate::parquet::{double_range, ColumnData, Limits, ParquetFile, RangeSource};
use crate::stac::{meets, Bbox};
use crate::wkb::{self, Geometry};

/// A theme O1 imports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Theme {
    Places,
    Buildings,
}

const BBOX: [&[&str]; 4] = [
    &["bbox", "xmin"],
    &["bbox", "xmax"],
    &["bbox", "ymin"],
    &["bbox", "ymax"],
];
const SOURCE_DATASET: &[&str] = &["sources", "list", "element", "dataset"];
const SOURCE_LICENSE: &[&str] = &["sources", "list", "element", "license"];

impl Theme {
    /// The `OvertureTheme` row's ID (`PLACES`, `BUILDINGS`).
    #[must_use]
    pub fn parse(id: &str) -> Option<Self> {
        match id {
            "PLACES" => Some(Self::Places),
            "BUILDINGS" => Some(Self::Buildings),
            _ => None,
        }
    }

    /// Overture's theme and type: the catalog's theme folder and collection.
    #[must_use]
    pub fn overture(self) -> (&'static str, &'static str) {
        match self {
            Self::Places => ("places", "place"),
            Self::Buildings => ("buildings", "building"),
        }
    }

    /// The columns a file must have: the ID, the geometry, its covering box and the sources' licenses.
    #[must_use]
    pub fn required(self) -> Vec<&'static [&'static str]> {
        let mut out: Vec<&[&str]> = vec![&["id"], &["geometry"], SOURCE_LICENSE];
        out.extend(BBOX);
        if self == Self::Places {
            out.push(&["names", "primary"]);
        }
        out
    }

    /// The other columns read when the file has them.
    #[must_use]
    pub fn optional(self) -> Vec<&'static [&'static str]> {
        match self {
            Self::Places => vec![
                &["version"],
                SOURCE_DATASET,
                &["confidence"],
                &["basic_category"],
                &["taxonomy", "primary"],
                &["operating_status"],
                &["brand", "names", "primary"],
                &["addresses", "list", "element", "freeform"],
                &["addresses", "list", "element", "locality"],
                &["addresses", "list", "element", "postcode"],
                &["addresses", "list", "element", "region"],
                &["addresses", "list", "element", "country"],
            ],
            Self::Buildings => vec![
                &["version"],
                SOURCE_DATASET,
                &["names", "primary"],
                &["class"],
                &["subtype"],
                &["height"],
                &["num_floors"],
            ],
        }
    }
}

/// The first address of a place.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Address {
    pub freeform: Option<String>,
    pub locality: Option<String>,
    pub postcode: Option<String>,
    pub region: Option<String>,
    pub country: Option<String>,
}

/// A place read.
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    pub gers_id: String,
    pub version: Option<i64>,
    pub name: String,
    pub basic_category: Option<String>,
    pub taxonomy: Option<String>,
    pub lon: f64,
    pub lat: f64,
    pub confidence: Option<f64>,
    pub operating_status: Option<String>,
    pub brand: Option<String>,
    pub address: Address,
    pub datasets: Vec<String>,
    pub licenses: Vec<String>,
}

/// A building read.
#[derive(Clone, Debug, PartialEq)]
pub struct Building {
    pub gers_id: String,
    pub version: Option<i64>,
    pub name: Option<String>,
    pub class: Option<String>,
    pub subtype: Option<String>,
    pub height: Option<f64>,
    pub num_floors: Option<i64>,
    pub polygons: Vec<Vec<Vec<[f64; 2]>>>,
    pub datasets: Vec<String>,
    pub licenses: Vec<String>,
}

/// A row read.
#[derive(Clone, Debug, PartialEq)]
pub enum Row {
    Place(Place),
    Building(Building),
}

/// What reading a file came to.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    pub files: u64,
    pub row_groups_read: u64,
    pub row_groups_skipped: u64,
    pub bytes: u64,
    pub rows_read: u64,
    pub rows_kept: u64,
    pub rows_rejected: u64,
    /// Rows by source dataset and license, of the rows kept.
    pub licenses: BTreeMap<(String, String), u64>,
    /// The first rejections, each with why.
    pub rejections: Vec<String>,
}

/// The rejections a run keeps the reasons of.
const MAX_REJECTIONS: usize = 20;
/// The positions one building may have.
const MAX_POSITIONS: usize = 100_000;

impl Counts {
    fn reject(&mut self, why: String) {
        self.rows_rejected += 1;
        if self.rejections.len() < MAX_REJECTIONS {
            self.rejections.push(why);
        }
    }

    /// Add another file's counts.
    pub fn add(&mut self, other: Self) {
        self.files += other.files;
        self.row_groups_read += other.row_groups_read;
        self.row_groups_skipped += other.row_groups_skipped;
        self.bytes += other.bytes;
        self.rows_read += other.rows_read;
        self.rows_kept += other.rows_kept;
        self.rows_rejected += other.rows_rejected;
        for (key, count) in other.licenses {
            *self.licenses.entry(key).or_default() += count;
        }
        for why in other.rejections {
            if self.rejections.len() < MAX_REJECTIONS {
                self.rejections.push(why);
            }
        }
    }
}

/// A column read for a row group, or none where the file lacks it.
struct Columns {
    by_path: Vec<(Vec<&'static str>, ColumnData)>,
}

impl Columns {
    fn get(&self, path: &[&str]) -> Option<&ColumnData> {
        self.by_path
            .iter()
            .find(|(own, _)| own.as_slice() == path)
            .map(|(_, data)| data)
    }

    fn scalar_text(&self, path: &[&str], rows: usize) -> Vec<Option<String>> {
        let Some(data) = self.get(path) else {
            return vec![None; rows];
        };
        data.scalars()
            .into_iter()
            .map(|index| index.and_then(|i| data.values.text(i)))
            .collect()
    }

    fn scalar_number(&self, path: &[&str], rows: usize) -> Vec<Option<f64>> {
        let Some(data) = self.get(path) else {
            return vec![None; rows];
        };
        data.scalars()
            .into_iter()
            .map(|index| index.and_then(|i| data.values.number(i)))
            .collect()
    }

    fn list_text(&self, path: &[&str], rows: usize) -> Vec<Vec<Option<String>>> {
        let Some(data) = self.get(path) else {
            return vec![Vec::new(); rows];
        };
        data.lists()
            .into_iter()
            .map(|row| {
                row.into_iter()
                    .map(|index| index.and_then(|i| data.values.text(i)))
                    .collect()
            })
            .collect()
    }
}

/// Whether a leaf path is one `theme` reads, and whether its statistics are kept (the box's).
fn wanted(theme: Theme, path: &[String]) -> Option<bool> {
    let matches = |candidate: &[&str]| candidate.len() == path.len() && candidate.iter().zip(path).all(|(a, b)| a == b);
    if BBOX.iter().any(|candidate| matches(candidate)) {
        return Some(true);
    }
    theme
        .required()
        .into_iter()
        .chain(theme.optional())
        .any(matches)
        .then_some(false)
}

/// The footer of `source` read for `theme`, its required columns checked.
///
/// # Errors
///
/// The footer does not read, or a required column is missing.
pub fn open(source: &dyn RangeSource, theme: Theme, limits: &Limits) -> Result<ParquetFile, Error> {
    let file = ParquetFile::open(source, limits, &|path| wanted(theme, path))?;
    let missing: Vec<String> = theme
        .required()
        .into_iter()
        .filter(|path| file.meta.leaf(path).is_none())
        .map(|path| path.join("."))
        .collect();
    if !missing.is_empty() {
        return Err(Error::new(format!(
            "the file has none of the columns {} that Overture's schema 2 {} have",
            missing.join(", "),
            theme.overture().0
        )));
    }
    let wrong: Vec<String> = theme
        .required()
        .into_iter()
        .chain(theme.optional())
        .filter_map(|path| type_refusal(&file, path))
        .collect();
    if !wrong.is_empty() {
        return Err(Error::new(format!(
            "the file's columns are not of the types of Overture's schema 2: {}",
            wrong.join(", ")
        )));
    }
    Ok(file)
}

/// The rows a column chunk holds: its entries, or for a repeated column the entries that start a row.
fn rows_of(data: &ColumnData) -> usize {
    if data.max_rep == 0 {
        data.entries()
    } else {
        data.rep.iter().filter(|level| **level == 0).count()
    }
}

/// The physical types the columns read must have (a number column may be of any number type).
fn type_refusal(file: &ParquetFile, path: &[&str]) -> Option<String> {
    use crate::parquet::Physical;
    let leaf = file.meta.leaves.get(file.meta.leaf(path)?)?;
    let numeric = matches!(
        path.last().copied(),
        Some("xmin" | "xmax" | "ymin" | "ymax" | "confidence" | "height" | "num_floors" | "version")
    );
    let fits = if numeric {
        matches!(
            leaf.physical,
            Physical::Double | Physical::Float | Physical::Int32 | Physical::Int64
        )
    } else {
        leaf.physical == Physical::ByteArray
    };
    (!fits).then(|| format!("{} is {:?}", path.join("."), leaf.physical))
}

/// Whether row group `group`'s box statistics meet `region` (true when it has none to say).
#[must_use]
pub fn group_meets(file: &ParquetFile, group: usize, region: &Bbox) -> bool {
    let Some(row_group) = file.meta.row_groups.get(group) else {
        return false;
    };
    let range = |path: &[&str]| {
        let leaf = file.meta.leaf(path)?;
        let chunk = row_group.chunk(leaf)?;
        double_range(chunk.statistics.as_ref(), file.meta.leaves.get(leaf)?.physical)
    };
    let (Some(xmin), Some(xmax), Some(ymin), Some(ymax)) =
        (range(BBOX[0]), range(BBOX[1]), range(BBOX[2]), range(BBOX[3]))
    else {
        return true;
    };
    // the group's rows lie within [least xmin, greatest xmax] by [least ymin, greatest ymax]
    meets(&[xmin.0, ymin.0, xmax.1, ymax.1], region)
}

/// Read the rows of `file` (opened with [`open`]) that meet `region`, handing each to `sink` (which answers false to
/// stop, a limit reached). `go_on` is asked before each row group with the counts so far, and false cancels the read
/// (`Ok(false)`).
///
/// # Errors
///
/// A column chunk that does not read: the file cannot be trusted.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)]
pub fn read_region(
    source: &dyn RangeSource,
    file: &ParquetFile,
    theme: Theme,
    region: &Bbox,
    limits: &Limits,
    counts: &mut Counts,
    sink: &mut dyn FnMut(Row) -> Result<bool, Error>,
    go_on: &mut dyn FnMut(&Counts) -> bool,
) -> Result<bool, Error> {
    let paths: Vec<&'static [&'static str]> = theme
        .required()
        .into_iter()
        .chain(theme.optional())
        .filter(|path| file.meta.leaf(path).is_some())
        .collect();
    for group in 0..file.meta.row_groups.len() {
        if !go_on(counts) {
            return Ok(false);
        }
        if !group_meets(file, group, region) {
            counts.row_groups_skipped += 1;
            continue;
        }
        counts.row_groups_read += 1;
        let rows = usize::try_from(file.meta.row_groups[group].num_rows).unwrap_or(usize::MAX);
        if rows > limits.max_values {
            return Err(Error::new(format!(
                "row group {group} claims {} rows, more than {} allowed",
                file.meta.row_groups[group].num_rows, limits.max_values
            )));
        }
        // the box first: a group whose rows all miss the region reads nothing else; every column read has the group's
        // rows, and all of them together are held to the group's decoded budget
        let mut columns = Columns { by_path: Vec::new() };
        let mut decoded = 0usize;
        let mut read = |path: &[&'static str]| -> Result<Option<ColumnData>, Error> {
            let Some(leaf) = file.meta.leaf(path) else {
                return Ok(None);
            };
            let data = file.read_column(source, group, leaf, limits)?;
            if rows_of(&data) != rows {
                return Err(Error::new(format!(
                    "the Parquet column {} has {} rows in a row group of {rows}",
                    path.join("."),
                    rows_of(&data)
                )));
            }
            decoded = decoded.saturating_add(data.values.decoded_bytes() + 2 * (data.def.len() + data.rep.len()));
            if decoded > limits.max_group_decoded {
                return Err(Error::new(format!(
                    "row group {group} decodes to more than {} bytes",
                    limits.max_group_decoded
                )));
            }
            Ok(Some(data))
        };
        for path in BBOX {
            if let Some(data) = read(path)? {
                columns.by_path.push((path.to_vec(), data));
            }
        }
        let boxes: Vec<[Option<f64>; 4]> = {
            let parts: Vec<Vec<Option<f64>>> = BBOX.iter().map(|path| columns.scalar_number(path, rows)).collect();
            (0..rows)
                .map(|row| {
                    [
                        parts[0].get(row).copied().flatten(),
                        parts[1].get(row).copied().flatten(),
                        parts[2].get(row).copied().flatten(),
                        parts[3].get(row).copied().flatten(),
                    ]
                })
                .collect()
        };
        counts.rows_read += rows as u64;
        let inside: Vec<bool> = boxes
            .iter()
            .map(|b| match b {
                [Some(xmin), Some(xmax), Some(ymin), Some(ymax)] => meets(&[*xmin, *ymin, *xmax, *ymax], region),
                // a row with no box is decided by its geometry
                _ => true,
            })
            .collect();
        if !inside.iter().any(|keep| *keep) {
            continue;
        }
        for path in &paths {
            if BBOX.contains(path) {
                continue;
            }
            if let Some(data) = read(path)? {
                columns.by_path.push((path.to_vec(), data));
            }
        }
        let ids = columns.scalar_text(&["id"], rows);
        let versions = columns.scalar_number(&["version"], rows);
        let geometries: Vec<Option<Vec<u8>>> = match columns.get(&["geometry"]) {
            Some(data) => data
                .scalars()
                .into_iter()
                .map(|index| index.and_then(|i| data.values.bytes(i).map(<[u8]>::to_vec)))
                .collect(),
            None => vec![None; rows],
        };
        let datasets = columns.list_text(SOURCE_DATASET, rows);
        let licenses = columns.list_text(SOURCE_LICENSE, rows);
        let names = columns.scalar_text(&["names", "primary"], rows);
        let extra = match theme {
            Theme::Places => Extra::Places {
                confidence: columns.scalar_number(&["confidence"], rows),
                category: columns.scalar_text(&["basic_category"], rows),
                taxonomy: columns.scalar_text(&["taxonomy", "primary"], rows),
                status: columns.scalar_text(&["operating_status"], rows),
                brand: columns.scalar_text(&["brand", "names", "primary"], rows),
                freeform: columns.list_text(&["addresses", "list", "element", "freeform"], rows),
                locality: columns.list_text(&["addresses", "list", "element", "locality"], rows),
                postcode: columns.list_text(&["addresses", "list", "element", "postcode"], rows),
                region: columns.list_text(&["addresses", "list", "element", "region"], rows),
                country: columns.list_text(&["addresses", "list", "element", "country"], rows),
            },
            Theme::Buildings => Extra::Buildings {
                class: columns.scalar_text(&["class"], rows),
                subtype: columns.scalar_text(&["subtype"], rows),
                height: columns.scalar_number(&["height"], rows),
                floors: columns.scalar_number(&["num_floors"], rows),
            },
        };
        drop(columns);
        for (row, keep) in inside.iter().enumerate() {
            if !keep {
                continue;
            }
            let Some(id) = ids.get(row).cloned().flatten().filter(|id| !id.trim().is_empty()) else {
                counts.reject(format!("a row of row group {group} with no id"));
                continue;
            };
            let geometry = match geometries.get(row).and_then(Option::as_deref) {
                Some(bytes) => wkb::read(bytes, MAX_POSITIONS),
                None => Err("no geometry".to_string()),
            };
            let geometry = match geometry {
                Ok(geometry) => geometry,
                Err(why) => {
                    counts.reject(format!("{id}: {why}"));
                    continue;
                }
            };
            let first = |list: &[Vec<Option<String>>]| list.get(row).and_then(|items| items.first().cloned().flatten());
            // the sources' datasets and licenses element by element, then each list without repeats
            let raw = |list: &[Vec<Option<String>>]| list.get(row).cloned().unwrap_or_default();
            let (raw_datasets, raw_licenses) = (raw(&datasets), raw(&licenses));
            let mut pairs: Vec<(String, String)> = (0..raw_datasets.len().max(raw_licenses.len()))
                .map(|index| {
                    let part = |list: &[Option<String>]| list.get(index).cloned().flatten().unwrap_or_default();
                    (part(&raw_datasets), part(&raw_licenses))
                })
                .collect();
            pairs.sort_unstable();
            pairs.dedup();
            let unique = |list: Vec<Option<String>>| -> Vec<String> {
                let mut out: Vec<String> = list.into_iter().flatten().filter(|text| !text.is_empty()).collect();
                out.sort_unstable();
                out.dedup();
                out
            };
            let (row_datasets, row_licenses) = (unique(raw_datasets), unique(raw_licenses));
            #[allow(clippy::cast_possible_truncation)]
            let version = versions.get(row).copied().flatten().map(|value| value as i64);
            let name = names
                .get(row)
                .cloned()
                .flatten()
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty());
            let read = match (&extra, geometry) {
                (
                    Extra::Places {
                        confidence,
                        category,
                        taxonomy,
                        status,
                        brand,
                        freeform,
                        locality,
                        postcode,
                        region: area,
                        country,
                    },
                    Geometry::Point([lon, lat]),
                ) => {
                    // a place's own point decides: its box came first
                    if !meets(&[lon, lat, lon, lat], region) {
                        continue;
                    }
                    let Some(name) = name else {
                        counts.reject(format!("{id}: no name"));
                        continue;
                    };
                    Row::Place(Place {
                        gers_id: id,
                        version,
                        name,
                        basic_category: category.get(row).cloned().flatten(),
                        taxonomy: taxonomy.get(row).cloned().flatten(),
                        lon,
                        lat,
                        confidence: confidence.get(row).copied().flatten().filter(|value| value.is_finite()),
                        operating_status: status.get(row).cloned().flatten(),
                        brand: brand.get(row).cloned().flatten(),
                        address: Address {
                            freeform: first(freeform),
                            locality: first(locality),
                            postcode: first(postcode),
                            region: first(area),
                            country: first(country),
                        },
                        datasets: row_datasets,
                        licenses: row_licenses,
                    })
                }
                (
                    Extra::Buildings {
                        class,
                        subtype,
                        height,
                        floors,
                    },
                    Geometry::Polygons(polygons),
                ) =>
                {
                    #[allow(clippy::cast_possible_truncation)]
                    Row::Building(Building {
                        gers_id: id,
                        version,
                        name,
                        class: class.get(row).cloned().flatten(),
                        subtype: subtype.get(row).cloned().flatten(),
                        height: height
                            .get(row)
                            .copied()
                            .flatten()
                            .filter(|value| value.is_finite() && *value >= 0.0),
                        num_floors: floors.get(row).copied().flatten().map(|value| value as i64),
                        polygons,
                        datasets: row_datasets,
                        licenses: row_licenses,
                    })
                }
                (Extra::Places { .. }, _) => {
                    counts.reject(format!("{id}: a place that is not a point"));
                    continue;
                }
                (Extra::Buildings { .. }, _) => {
                    counts.reject(format!("{id}: a building that is not a polygon"));
                    continue;
                }
            };
            if pairs.is_empty() {
                pairs.push((String::new(), String::new()));
            }
            for pair in pairs {
                *counts.licenses.entry(pair).or_default() += 1;
            }
            counts.rows_kept += 1;
            if !sink(read)? {
                return Ok(true);
            }
        }
    }
    Ok(true)
}

/// The columns of a theme beyond the ones every theme has, by row.
enum Extra {
    Places {
        confidence: Vec<Option<f64>>,
        category: Vec<Option<String>>,
        taxonomy: Vec<Option<String>>,
        status: Vec<Option<String>>,
        brand: Vec<Option<String>>,
        freeform: Vec<Vec<Option<String>>>,
        locality: Vec<Vec<Option<String>>>,
        postcode: Vec<Vec<Option<String>>>,
        region: Vec<Vec<Option<String>>>,
        country: Vec<Vec<Option<String>>>,
    },
    Buildings {
        class: Vec<Option<String>>,
        subtype: Vec<Option<String>>,
        height: Vec<Option<f64>>,
        floors: Vec<Option<f64>>,
    },
}

#[cfg(test)]
#[path = "overture_tests.rs"]
mod tests;
