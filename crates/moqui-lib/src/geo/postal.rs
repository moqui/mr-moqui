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

//! Postal codes by country (plans/Maps.md §6.3): each code's normalized form (upper case, spaces and hyphens taken
//! out, leading zeros kept), its display spelling and its granularity. The rules are explicit for the countries whose
//! sources mix full and partial codes: the US (a five-digit ZIP code), Canada, Great Britain and the Netherlands (a
//! full code, or its first part as a declared prefix); every other country's code is taken whole. A prefix is never
//! made by cutting a longer code.

use super::{Point, Precision};

/// The longest normalized code (GeoNames' postal code column is 20 characters).
pub const MAX_CODE: usize = 20;

/// Whether a code is a country's whole code or its first part (`PostalCodeGranularity`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Granularity {
    Full,
    Prefix,
}

impl Granularity {
    #[must_use]
    pub fn enum_id(self) -> &'static str {
        match self {
            Self::Full => "GPG_FULL",
            Self::Prefix => "GPG_PREFIX",
        }
    }
}

/// A postal code as a country's rules read it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct PostalCode {
    /// Upper case, no spaces or hyphens, leading zeros kept: the lookup's and the Geo ID's form.
    pub normalized: String,
    /// The spelling shown: upper case, a country's space where it means something (`K1A 0B1`, `SW1A 1AA`, `1012 AB`).
    pub display: String,
    pub granularity: Granularity,
}

/// A postal code's row in a source: the code, its place name and administrative names and codes where the source has
/// them, its point, the source's accuracy value and the precision it stands for.
#[derive(Clone, Debug, PartialEq)]
pub struct PostalRecord {
    pub line: u64,
    /// The country as the row gives it: ISO alpha-2 (GeoNames, Census), or alpha-2 or alpha-3 (an operator's file).
    pub country: String,
    pub code: PostalCode,
    pub place_name: Option<String>,
    pub admin_name1: Option<String>,
    pub admin_code1: Option<String>,
    pub admin_name2: Option<String>,
    pub admin_code2: Option<String>,
    pub point: Option<Point>,
    pub accuracy: Option<i32>,
    pub precision: Precision,
}

/// ISO alpha-2 of the countries with rules of their own, from alpha-2 or alpha-3.
fn rules_country(country: &str) -> &str {
    match country.trim().to_ascii_uppercase().as_str() {
        "US" | "USA" => "US",
        "CA" | "CAN" => "CA",
        "GB" | "GBR" => "GB",
        "NL" | "NLD" => "NL",
        _ => "",
    }
}

/// `raw` as a postal code of `country` (ISO alpha-2 or alpha-3).
///
/// # Errors
///
/// An empty code, one with characters other than letters and digits (after spaces and hyphens), one longer than
/// [`MAX_CODE`], or one that is not of its country's shape: a US code that is not five digits (a ZIP+4 is refused, not
/// cut to its ZIP code), a Canadian, British or Dutch code that is neither a full code nor its first part.
pub fn normalize_postal_code(country: &str, raw: &str) -> Result<PostalCode, String> {
    let display: String = raw
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_uppercase();
    if display.is_empty() {
        return Err("the postal code is empty".to_string());
    }
    let normalized: String = display.chars().filter(|c| *c != ' ' && *c != '-').collect();
    if normalized.is_empty() || !normalized.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(format!(
            "postal code {display:?} has characters other than letters, digits, spaces and hyphens"
        ));
    }
    if normalized.len() > MAX_CODE {
        return Err(format!("postal code {display:?} is longer than {MAX_CODE} characters"));
    }
    let shape = |pattern: &str| shaped(&normalized, pattern);
    let full = |display: String| PostalCode {
        normalized: normalized.clone(),
        display,
        granularity: Granularity::Full,
    };
    let prefix = || PostalCode {
        normalized: normalized.clone(),
        display: normalized.clone(),
        granularity: Granularity::Prefix,
    };
    match rules_country(country) {
        "US" => {
            if shape("99999") {
                Ok(full(normalized.clone()))
            } else if shape("999999999") {
                Err(format!(
                    "postal code {display:?} is a ZIP+4 code; the five-digit ZIP code is its area"
                ))
            } else {
                Err(format!("postal code {display:?} is not a five-digit ZIP code"))
            }
        }
        "CA" => {
            if shape("A9A9A9") {
                Ok(full(spaced(&normalized, 3)))
            } else if shape("A9A") {
                Ok(prefix())
            } else {
                Err(format!(
                    "postal code {display:?} is not a Canadian code (A9A 9A9) or its first part (A9A)"
                ))
            }
        }
        "NL" => {
            if shape("9999AA") {
                Ok(full(spaced(&normalized, 4)))
            } else if shape("9999") {
                Ok(prefix())
            } else {
                Err(format!(
                    "postal code {display:?} is not a Dutch code (9999 AA) or its first part (9999)"
                ))
            }
        }
        "GB" => {
            let inward_at = normalized.len().saturating_sub(3);
            let whole = normalized.len() >= 5
                && normalized
                    .split_at_checked(inward_at)
                    .is_some_and(|(outward, inward)| gb_outward(outward) && shaped(inward, "9AA"));
            if whole || normalized == "GIR0AA" {
                Ok(full(spaced(&normalized, inward_at)))
            } else if gb_outward(&normalized) {
                Ok(prefix())
            } else {
                Err(format!(
                    "postal code {display:?} is not a British postcode or its outward code"
                ))
            }
        }
        _ => Ok(full(display.clone())),
    }
}

/// A postal code a person typed, read for the postal lookup (plans/Maps.md §6.1, §6.3): the code by its country's rules,
/// and the declared prefix to fall back to when the full code has no row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LookupCode {
    pub code: PostalCode,
    /// The country's declared first part of a full code (a Canadian forward sortation area, a British outward code, a
    /// Dutch four-digit area): only for the countries whose sources publish such prefixes, never a cut of another
    /// country's code.
    pub prefix: Option<PostalCode>,
    /// The text was a US ZIP+4 code, read as its five-digit ZIP code: the ZIP+4 lies within that area, so the area's
    /// precision is all a match claims.
    pub zip_plus4: bool,
}

/// `raw` as a postal code of `country` for a lookup: as [`normalize_postal_code`], but a US ZIP+4 is read as its
/// ZIP code (a lookup of the area that holds it, flagged), and a full code of a partial-code country has its declared
/// prefix.
///
/// # Errors
///
/// As [`normalize_postal_code`].
pub fn lookup_code(country: &str, raw: &str) -> Result<LookupCode, String> {
    let compact: String = raw.chars().filter(|c| !c.is_whitespace() && *c != '-').collect();
    let zip_plus4 = rules_country(country) == "US" && shaped(&compact, "999999999");
    let code = if zip_plus4 {
        normalize_postal_code(country, compact.get(..5).unwrap_or_default())?
    } else {
        normalize_postal_code(country, raw)?
    };
    let prefix = declared_prefix(country, &code);
    Ok(LookupCode {
        code,
        prefix,
        zip_plus4,
    })
}

/// The declared prefix of a full code of Canada, Great Britain or the Netherlands (the first part their sources publish
/// alone); None for a prefix already, and for every other country.
#[must_use]
pub fn declared_prefix(country: &str, code: &PostalCode) -> Option<PostalCode> {
    if code.granularity != Granularity::Full {
        return None;
    }
    let cut = match rules_country(country) {
        "CA" => 3,
        "NL" => 4,
        "GB" if code.normalized != "GIR0AA" => code.normalized.len().checked_sub(3)?,
        _ => return None,
    };
    let head = code.normalized.get(..cut)?;
    normalize_postal_code(country, head)
        .ok()
        .filter(|prefix| prefix.granularity == Granularity::Prefix)
}

/// Whether `text` has the shape of `pattern`: `9` a digit, `A` a letter, one for one.
fn shaped(text: &str, pattern: &str) -> bool {
    text.len() == pattern.len()
        && text.chars().zip(pattern.chars()).all(|(c, p)| match p {
            '9' => c.is_ascii_digit(),
            'A' => c.is_ascii_alphabetic(),
            _ => c == p,
        })
}

/// A British outward code: A9, A99, AA9, AA99, A9A or AA9A.
fn gb_outward(text: &str) -> bool {
    ["A9", "A99", "AA9", "AA99", "A9A", "AA9A"]
        .iter()
        .any(|pattern| shaped(text, pattern))
}

/// `text` with a space before byte `at` (ASCII, checked by the caller's shape).
fn spaced(text: &str, at: usize) -> String {
    match text.split_at_checked(at) {
        Some((head, tail)) => format!("{head} {tail}"),
        None => text.to_string(),
    }
}
