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

//! Address matching of O2 (plans/archive/Maps-Overture.md §O2, plans/archive/Maps.md §6.1): the parts of an address a person typed
//! ([`parse`]), the keys an address point and a query are compared by ([`street_key`], [`number_key`], [`unit_key`],
//! [`postcode_key`], [`fold`]) and the ranking of the points found into candidates ([`rank`]). No database and no
//! context: the server's service reads the rows (`moqui-maps` `address_service::search_overture_addresses`) and hands
//! them here, and so does the apps' search of an offline map package's addresses (`moqui-client` `packages::addresses`,
//! plans/archive/Maps-Offline.md §2.5), which is why it is here, in moqui-lib, with no Parquet reader or codec beside it.
//!
//! **Normalization.** Case and the diacritics of Latin letters are folded (`crate::geo::admin1::fold_name`), dots
//! inside a word dropped (`St.`, `p.za`), other punctuation made a space, and each word of a street mapped to one
//! spelling by the tables of its country's languages ([`STREET_WORDS`]): English (with the directionals), Italian,
//! French, German, Spanish, Portuguese and Dutch; a German or Dutch street's `-straße`, `-strasse`, `-straat` or `-platz`
//! ending is its short form. The particles of the Romance languages, German and Dutch (`di`, `de'`, `du`, `von`, `van`)
//! are left out of a street of more words, so `Via de' Tornabuoni` and `Via Tornabuoni` meet.
//!
//! **Ranking** (§6.1: never call a match exact that is not): a number and a street matched is an address match; then the
//! postal code agreeing, then the locality; points of one address within [`SAME_PLACE_M`] are one candidate (its units or
//! entrances), and candidates farther apart within [`MARGIN`] of the best are choices (`ambiguous`), never the first
//! picked. A number found only without its suffix (`12` for `12r`) is an estimate, and a street with no such number is the
//! street's estimate with its points' extent. Overture's schema 2.0 does not say what an address point stands for, so a
//! candidate says its position kind is not stated.

use crate::geo::admin1::fold_name;
use crate::geo::postal::lookup_code;

/// Points of one address closer than this are one candidate: its units and entrances (the evaluation's same-address
/// spread: Boston's 99th percentile 30 m, Amsterdam's 28 m).
pub const SAME_PLACE_M: f64 = 30.0;
/// Candidates within this many points of the best are offered as choices.
pub const MARGIN: i32 = 15;
/// The longest text matched.
pub const MAX_QUERY: usize = 200;

/// A language whose street words are known.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Language {
    English,
    Italian,
    French,
    German,
    Spanish,
    Portuguese,
    Dutch,
}

/// The languages of a country's street names (ISO alpha-2); English for a country not listed.
#[must_use]
pub fn languages(country: &str) -> &'static [Language] {
    use Language::{Dutch, English, French, German, Italian, Portuguese, Spanish};
    match country.to_ascii_uppercase().as_str() {
        "CA" => &[English, French],
        "IT" | "SM" | "VA" => &[Italian],
        "FR" | "MC" => &[French],
        "BE" => &[French, Dutch, German],
        "CH" => &[German, French, Italian],
        "LU" => &[French, German],
        "DE" | "AT" | "LI" => &[German],
        "ES" | "MX" | "AR" | "CL" | "CO" | "PE" | "VE" | "EC" | "BO" | "PY" | "UY" | "CR" | "PA" | "GT" | "HN"
        | "SV" | "NI" | "DO" | "CU" | "PR" => &[Spanish],
        "PT" | "BR" | "AO" | "MZ" => &[Portuguese],
        "NL" => &[Dutch],
        _ => &[English],
    }
}

/// Whether a country writes the number before the street (`52 Hemenway Street`, `12 rue de Rivoli`); the others after it
/// (`Via dei Fossi 51`, `Kalverstraat 1`).
#[must_use]
pub fn number_first(country: &str) -> bool {
    matches!(
        country.to_ascii_uppercase().as_str(),
        "US" | "CA" | "GB" | "IE" | "AU" | "NZ" | "FR" | "MC" | "LU" | "PR" | "ZA" | "IN" | "PH"
    )
}

/// The street words of each language: each spelling and the one it is read as. A spelling missing from a table stays as
/// it is. The forms are shared across languages (`ave` for avenue, avenida and av.), so a country of two languages reads
/// one street one way.
pub const STREET_WORDS: [(Language, &[(&str, &str)]); 7] = [
    (
        Language::English,
        &[
            ("street", "st"),
            ("str", "st"),
            ("saint", "st"),
            ("avenue", "ave"),
            ("av", "ave"),
            ("boulevard", "blvd"),
            ("road", "rd"),
            ("drive", "dr"),
            ("lane", "ln"),
            ("court", "ct"),
            ("place", "pl"),
            ("square", "sq"),
            ("terrace", "ter"),
            ("terr", "ter"),
            ("parkway", "pkwy"),
            ("highway", "hwy"),
            ("circle", "cir"),
            ("trail", "trl"),
            ("crescent", "cres"),
            ("alley", "aly"),
            ("expressway", "expy"),
            ("freeway", "fwy"),
            ("mount", "mt"),
            ("fort", "ft"),
            ("plaza", "plz"),
            ("point", "pt"),
            ("heights", "hts"),
            ("center", "ctr"),
            ("centre", "ctr"),
            ("north", "n"),
            ("south", "s"),
            ("east", "e"),
            ("west", "w"),
            ("northeast", "ne"),
            ("northwest", "nw"),
            ("southeast", "se"),
            ("southwest", "sw"),
        ],
    ),
    (
        Language::Italian,
        &[
            ("vle", "viale"),
            ("pza", "piazza"),
            ("p", "piazza"),
            ("ple", "piazzale"),
            ("cso", "corso"),
            ("lgo", "largo"),
            ("vlo", "vicolo"),
            ("vic", "vicolo"),
            ("bgo", "borgo"),
            ("san", "s"),
            ("santo", "s"),
            ("santa", "s"),
            ("sant", "s"),
            ("ss", "s"),
        ],
    ),
    (
        Language::French,
        &[
            ("r", "rue"),
            ("avenue", "ave"),
            ("av", "ave"),
            ("boulevard", "blvd"),
            ("bd", "blvd"),
            ("bld", "blvd"),
            ("place", "pl"),
            ("chemin", "ch"),
            ("chem", "ch"),
            ("impasse", "imp"),
            ("all", "allee"),
            ("qu", "quai"),
            ("route", "rte"),
            ("square", "sq"),
            ("faubourg", "fbg"),
            ("fg", "fbg"),
            ("saint", "st"),
            ("sainte", "st"),
            ("ste", "st"),
        ],
    ),
    (
        Language::German,
        &[("strasse", "str"), ("platz", "pl"), ("sankt", "st")],
    ),
    (
        Language::Spanish,
        &[
            ("c", "calle"),
            ("cl", "calle"),
            ("avenida", "ave"),
            ("av", "ave"),
            ("avda", "ave"),
            ("plaza", "plz"),
            ("pza", "plz"),
            ("pl", "plz"),
            ("carretera", "ctra"),
            ("cno", "camino"),
            ("rda", "ronda"),
            ("travesia", "trva"),
            ("san", "s"),
            ("santa", "s"),
            ("santo", "s"),
        ],
    ),
    (
        Language::Portuguese,
        &[
            ("r", "rua"),
            ("avenida", "ave"),
            ("av", "ave"),
            ("pc", "praca"),
            ("pca", "praca"),
            ("travessa", "tv"),
            ("trav", "tv"),
            ("lgo", "largo"),
            ("estrada", "estr"),
            ("al", "alameda"),
            ("sao", "s"),
            ("santa", "s"),
            ("santo", "s"),
        ],
    ),
    (Language::Dutch, &[("straat", "str"), ("gr", "gracht")]),
];

/// The particles a street of more words is read without, by language.
const PARTICLES: [(Language, &[&str]); 7] = [
    (Language::English, &[]),
    (
        Language::Italian,
        &[
            "di", "del", "dello", "della", "dei", "degli", "delle", "dell", "de", "d", "da", "dal", "dalla", "dai", "e",
        ],
    ),
    (
        Language::French,
        &["de", "du", "des", "la", "le", "les", "l", "d", "et"],
    ),
    (
        Language::German,
        &[
            "am", "an", "der", "den", "die", "das", "im", "in", "zum", "zur", "von", "vom",
        ],
    ),
    (Language::Spanish, &["de", "del", "la", "las", "los", "el", "y"]),
    (Language::Portuguese, &["de", "da", "do", "das", "dos", "d", "e"]),
    (Language::Dutch, &["van", "de", "der", "den", "het", "t"]),
];

/// German and Dutch street endings written as one word with the name: each ending and its short form.
const ENDINGS: [(Language, &[(&str, &str)]); 2] = [
    (Language::German, &[("strasse", "str"), ("platz", "pl")]),
    (Language::Dutch, &[("straat", "str")]),
];

/// `text` folded for comparison: lower case, the diacritics of Latin letters taken off, a dot within a word dropped
/// (`St.` as `st`, `p.za` as `pza`), every other character that is not a letter or a digit a space, spaces collapsed.
#[must_use]
pub fn fold(text: &str) -> String {
    let folded = fold_name(text);
    let mut out = String::with_capacity(folded.len());
    let mut space = false;
    for c in folded.chars() {
        if c == '.' {
            continue;
        }
        if c.is_alphanumeric() {
            if space && !out.is_empty() {
                out.push(' ');
            }
            space = false;
            out.push(c);
        } else {
            space = true;
        }
    }
    out
}

/// A street name's key for `country`: folded, each word read by the country's tables, and the particles left out when
/// other words remain.
#[must_use]
pub fn street_key(street: &str, country: &str) -> String {
    let langs = languages(country);
    let words: Vec<String> = fold(street)
        .split(' ')
        .filter(|word| !word.is_empty())
        .map(|word| street_word(word, langs))
        .collect();
    let particle = |word: &str| {
        PARTICLES
            .iter()
            .filter(|(lang, _)| langs.contains(lang))
            .any(|(_, list)| list.contains(&word))
    };
    let kept: Vec<&String> = words.iter().filter(|word| !particle(word)).collect();
    if kept.is_empty() {
        words.join(" ")
    } else {
        kept.into_iter().map(String::as_str).collect::<Vec<_>>().join(" ")
    }
}

fn street_word(word: &str, langs: &[Language]) -> String {
    for (lang, table) in &STREET_WORDS {
        if !langs.contains(lang) {
            continue;
        }
        if let Some((_, to)) = table.iter().find(|(from, _)| *from == word) {
            return (*to).to_string();
        }
    }
    for (lang, endings) in &ENDINGS {
        if !langs.contains(lang) {
            continue;
        }
        for (ending, short) in *endings {
            if let Some(stem) = word.strip_suffix(ending).filter(|stem| stem.len() >= 2) {
                return format!("{stem}{short}");
            }
        }
    }
    word.to_string()
}

/// A house number's key: folded, spaces taken out, a `/` or `-` before a letter dropped (`12/A`, `12 A` and `12a` meet),
/// and Italian `rosso` written `r` (Florence's red numbers).
#[must_use]
pub fn number_key(number: &str) -> String {
    let folded = fold_name(number).replace("rosso", "r").replace("nero", "");
    let kept: Vec<char> = folded
        .chars()
        .filter(|c| c.is_alphanumeric() || *c == '/' || *c == '-')
        .collect();
    let mut out = String::with_capacity(kept.len());
    for (index, c) in kept.iter().enumerate() {
        if (*c == '/' || *c == '-') && kept.get(index + 1).is_some_and(|next| next.is_alphabetic()) {
            continue;
        }
        out.push(*c);
    }
    out
}

/// The digits a house number starts with (`12` of `12r`, `12bis`, `12/a`), when it has others after them.
#[must_use]
pub fn number_base(key: &str) -> Option<String> {
    let digits: String = key.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty() && digits.len() < key.len()).then_some(digits)
}

/// The words that say a unit is one (taken off its key).
const UNIT_WORDS: &[&str] = &[
    "apt",
    "apartment",
    "unit",
    "suite",
    "ste",
    "fl",
    "floor",
    "rm",
    "room",
    "no",
    "nr",
    "int",
    "interno",
    "scala",
    "sc",
    "app",
    "appartement",
    "apto",
    "whg",
    "wohnung",
    "piso",
    "puerta",
    "bus",
    "boite",
    "bte",
];

/// A unit's key: folded, its designator word left out, the rest joined (`Apt 4B`, `#4b` and `4 B` meet).
#[must_use]
pub fn unit_key(unit: &str) -> String {
    fold(unit)
        .split(' ')
        .filter(|word| !word.is_empty() && !UNIT_WORDS.contains(word))
        .collect::<Vec<_>>()
        .concat()
}

/// A postal code's key for `country`: its country's rules (`crate::geo::postal`: a ZIP+4 read as its ZIP code, a
/// British, Canadian or Dutch code without its space), else upper case letters and digits.
#[must_use]
pub fn postcode_key(postcode: &str, country: &str) -> String {
    match lookup_code(country, postcode) {
        Ok(code) => code.code.normalized,
        Err(_) => postcode
            .chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_uppercase(),
    }
}

/// An address as a person typed it, in parts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Query {
    /// The house number as typed (`52`, `12r`).
    pub number: Option<String>,
    /// The words of the street, and in a text of one part what follows it (the locality), folded: the matcher tries the
    /// longest street first ([`Query::streets`]).
    pub words: Vec<String>,
    /// Whether `words` is the street alone (a part of its own, or the words before a number written after the street).
    pub street_only: bool,
    pub unit: Option<String>,
    pub postcode: Option<String>,
    /// The locality, folded, when a part of its own says it.
    pub locality: Option<String>,
}

/// Words that name a country rather than a locality (folded), passed over as a part of their own.
const COUNTRY_WORDS: &[&str] = &[
    "us",
    "usa",
    "united states",
    "united states of america",
    "italia",
    "italy",
    "france",
    "deutschland",
    "germany",
    "nederland",
    "netherlands",
    "the netherlands",
    "espana",
    "spain",
    "portugal",
    "belgique",
    "belgie",
    "belgium",
    "schweiz",
    "suisse",
    "svizzera",
    "switzerland",
    "osterreich",
    "austria",
    "canada",
    "united kingdom",
    "uk",
];

/// Whether `word` is a house number: a digit first, at most ten letters, digits, `/` and `-`, and not an ordinal (`5th`,
/// `1st`: a street's name).
fn looks_like_number(word: &str) -> bool {
    let mut chars = word.chars();
    let ordinal = ["st", "nd", "rd", "th"].iter().any(|ending| {
        word.strip_suffix(ending)
            .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()))
    });
    chars.next().is_some_and(|c| c.is_ascii_digit())
        && word.len() <= 10
        && !ordinal
        && word.chars().all(|c| c.is_ascii_alphanumeric() || c == '/' || c == '-')
}

/// Whether `word` reads as a unit's name after a unit word: one with a digit, or of one or two characters (`4B`, `A`,
/// `200`), not a word of a street (`Ste Catherine`).
fn looks_like_unit(word: &str) -> bool {
    word.chars().any(|c| c.is_ascii_digit()) || word.chars().count() <= 2
}

/// Whether `word` (or `word next`) is a postal code of `country`'s shape, or of any common shape when none is known.
fn postcode_at(words: &[String], index: usize, country: &str) -> Option<(String, usize)> {
    let word = words.get(index)?;
    let upper = word.to_ascii_uppercase();
    let next = words.get(index + 1).map(|next| next.to_ascii_uppercase());
    let digits = |text: &str, n: usize| text.len() == n && text.bytes().all(|b| b.is_ascii_digit());
    let letters = |text: &str, n: usize| text.len() == n && text.bytes().all(|b| b.is_ascii_alphabetic());
    let country = country.to_ascii_uppercase();
    let any = country.is_empty();
    // a Dutch code: four digits and two letters, written apart or together
    if any || country == "NL" {
        if digits(&upper, 4) && next.as_deref().is_some_and(|next| letters(next, 2)) {
            return Some((format!("{upper} {}", next.unwrap_or_default()), 2));
        }
        let (head, tail) = (upper.get(..4).unwrap_or_default(), upper.get(4..).unwrap_or_default());
        if upper.len() == 6 && digits(head, 4) && letters(tail, 2) {
            return Some((upper, 1));
        }
    }
    // a Canadian code: A9A 9A9
    let canadian = |text: &str| {
        text.len() == 3
            && text.as_bytes()[0].is_ascii_alphabetic()
            && text.as_bytes()[1].is_ascii_digit()
            && text.as_bytes()[2].is_ascii_alphabetic()
    };
    if (any || country == "CA") && canadian(&upper) {
        if let Some(next) = next.as_deref().filter(|next| next.len() == 3) {
            return Some((format!("{upper} {next}"), 2));
        }
    }
    // a British code: an outward code with a letter and a digit, and an inward code 9AA
    if country == "GB" {
        let inward = |text: &str| {
            text.len() == 3 && text.as_bytes()[0].is_ascii_digit() && letters(text.get(1..).unwrap_or_default(), 2)
        };
        if (2..=4).contains(&upper.len())
            && upper.as_bytes()[0].is_ascii_alphabetic()
            && upper.bytes().any(|b| b.is_ascii_digit())
            && next.as_deref().is_some_and(inward)
        {
            return Some((format!("{upper} {}", next.unwrap_or_default()), 2));
        }
    }
    // five digits (US, IT, FR, DE, ES), a ZIP+4, or four (BE, CH, AT, NL's first part, DK)
    let zip4 = upper.len() == 10
        && digits(upper.get(..5).unwrap_or_default(), 5)
        && upper.get(5..6) == Some("-")
        && digits(upper.get(6..).unwrap_or_default(), 4);
    if zip4 && (any || country == "US") {
        return Some((upper, 1));
    }
    let four = matches!(
        country.as_str(),
        "BE" | "CH" | "AT" | "NL" | "DK" | "NO" | "LU" | "AU" | "NZ" | "HU"
    );
    if digits(&upper, 5) && (any || !four) || digits(&upper, 4) && (four || any) {
        return Some((upper, 1));
    }
    None
}

/// `text` read as an address of `country` (ISO alpha-2, or empty when not known): its postal code (by the country's shape,
/// the last one found), its unit (after a unit word or `#`), its number (first in a number-first country, else the number
/// after the street) and the street's words; a part after the street that is not a postal code, a two-letter region or
/// a country is its locality. None when nothing of a street is left.
#[must_use]
pub fn parse(text: &str, country: &str) -> Option<Query> {
    let text: String = text.chars().take(MAX_QUERY).collect();
    let mut parts: Vec<Vec<String>> = text
        .split([',', ';', '\n'])
        .map(|part| {
            // keep a # as a word of its own (fold makes it a space)
            let marked = part.replace('#', " unit ");
            fold(&marked)
                .split(' ')
                .filter(|w| !w.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .filter(|words: &Vec<String>| !words.is_empty())
        .collect();
    if parts.is_empty() {
        return None;
    }
    let mut query = Query::default();
    // the postal code: the last word (or two) of its shape, never the first word of the first part (a house number)
    'found: for part_index in (0..parts.len()).rev() {
        let part = &parts[part_index];
        for index in (0..part.len()).rev() {
            if part_index == 0 && index == 0 {
                continue;
            }
            if let Some((code, taken)) = postcode_at(part, index, country) {
                // in the street's part, where the number follows the street, the first number is the house number
                let earlier_number = part[..index].iter().any(|word| looks_like_number(word));
                if part_index == 0 && !earlier_number && !number_first(country) {
                    continue;
                }
                parts[part_index].drain(index..index + taken);
                query.postcode = Some(code);
                break 'found;
            }
        }
    }
    parts.retain(|part| !part.is_empty());
    // a unit: a unit word and a unit's name after it, in any part (`ste` is a saint where French is written)
    let french = languages(country).contains(&Language::French);
    for part in &mut parts {
        if let Some(index) = (0..part.len().saturating_sub(1)).find(|index| {
            let word = part[*index].as_str();
            UNIT_WORDS.contains(&word)
                && !(french && word == "ste")
                && (word == "unit" || looks_like_unit(&part[*index + 1]))
        }) {
            query.unit = Some(part[index + 1].clone());
            part.drain(index..index + 2);
            break;
        }
    }
    parts.retain(|part| !part.is_empty());
    // the street's part: the first with a letter
    let street_at = parts
        .iter()
        .position(|part| part.iter().any(|word| word.chars().any(char::is_alphabetic)))?;
    let mut street = parts[street_at].clone();
    // a house number in a part of its own before the street (`52, Hemenway Street`)
    if street_at > 0 {
        if let Some(word) = parts[..street_at].iter().flatten().find(|word| looks_like_number(word)) {
            query.number = Some(word.clone());
        }
    }
    let joins_number = |word: &str| {
        !number_first(country)
            && (word.len() == 1 && word.chars().all(char::is_alphabetic)
                || ["bis", "ter", "quater", "rosso", "r"].contains(&word))
    };
    if query.number.is_none() {
        if street.first().is_some_and(|word| looks_like_number(word)) && street.len() > 1 {
            query.number = Some(street.remove(0));
            if street.len() > 1 && ["bis", "ter", "quater"].contains(&street[0].as_str()) {
                let suffix = street.remove(0);
                query.number = query.number.map(|number| number + &suffix);
            }
        } else if let Some(index) = street
            .iter()
            .rposition(|word| looks_like_number(word))
            .filter(|index| *index > 0)
            // where a comma ends the street, its number ends it too (`Via 4 Novembre 12`, not `Piazza 1 Maggio`)
            .filter(|index| {
                parts.len() == street_at + 1
                    || *index + 1 == street.len()
                    || *index + 2 == street.len() && street.last().is_some_and(|word| joins_number(word))
            })
        {
            let mut number = street[index].clone();
            let mut end = index + 1;
            if street.get(end).is_some_and(|word| joins_number(word)) {
                number.push_str(&street[end]);
                end += 1;
            }
            query.number = Some(number);
            // the words after a number written after the street are the locality
            let after: Vec<String> = street.drain(index..).skip(end - index).collect();
            if !after.is_empty() && query.locality.is_none() {
                query.locality = Some(after.join(" "));
            }
            query.street_only = true;
        }
    }
    if street.is_empty() {
        return None;
    }
    if parts.len() > street_at + 1 {
        query.street_only = true;
    }
    // the locality: the first later part that is not a two-letter region or a country
    for part in parts.iter().skip(street_at + 1) {
        let joined = part.join(" ");
        let region = part.len() == 1 && joined.len() == 2 && joined.chars().all(char::is_alphabetic);
        if region || COUNTRY_WORDS.contains(&joined.as_str()) {
            continue;
        }
        // `Boston MA`: a two-letter region after the locality's words
        let mut words = part.clone();
        if words.len() > 1
            && words
                .last()
                .is_some_and(|word| word.len() == 2 && word.chars().all(char::is_alphabetic))
        {
            words.pop();
        }
        query.locality = Some(words.join(" "));
        break;
    }
    query.words = street;
    Some(query)
}

impl Query {
    /// The street keys to try for `country`, the longest first, each with the locality the rest of the words would be:
    /// the street alone when the text said where it ends, else every first part of the words (at least one).
    #[must_use]
    pub fn streets(&self, country: &str) -> Vec<(String, Option<String>)> {
        let mut out: Vec<(String, Option<String>)> = Vec::new();
        let ends = if self.street_only {
            self.words.len()..=self.words.len()
        } else {
            1..=self.words.len()
        };
        for end in ends.rev() {
            let street = self.words[..end].join(" ");
            let rest = self.words[end..].join(" ");
            let mut rest_words: Vec<&str> = rest.split(' ').filter(|word| !word.is_empty()).collect();
            if rest_words.last().is_some_and(|word| word.len() == 2) && rest_words.len() > 1 {
                rest_words.pop();
            }
            let locality = if rest_words.is_empty() {
                self.locality.clone()
            } else {
                Some(rest_words.join(" "))
            };
            let key = street_key(&street, country);
            if !key.is_empty() && !out.iter().any(|(seen, _)| *seen == key) {
                out.push((key, locality));
            }
        }
        out
    }
}

/// An address point of the index, as the service read it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Point {
    pub address_id: String,
    pub country: String,
    pub street: String,
    pub number: String,
    pub number_key: String,
    pub unit: String,
    pub unit_key: String,
    pub postcode: String,
    pub postcode_key: String,
    pub locality: String,
    pub region: String,
    /// Every address level and the postal city, folded, for the locality's comparison (`locality` is the last level).
    pub places: Vec<String>,
    pub lon: f64,
    pub lat: f64,
    pub gers_id: String,
    pub release: String,
    pub datasets: String,
    pub licenses: String,
}

/// How a candidate matched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tier {
    /// The number and the street: an address match.
    Address,
    /// The street, and the number without its suffix: an estimate.
    NumberBase,
    /// The street, no such number: the street's estimate.
    Street,
}

impl Tier {
    /// The GeoPointPrecision a candidate of the tier has (plans/archive/Maps.md §6.1).
    #[must_use]
    pub fn precision(self) -> &'static str {
        match self {
            Self::Address => "GPP_ADDRESS_MATCH",
            Self::NumberBase | Self::Street => "GPP_PLACE_ESTIMATE",
        }
    }

    #[must_use]
    pub fn match_kind(self) -> &'static str {
        match self {
            Self::Address | Self::NumberBase => "address",
            Self::Street => "street",
        }
    }
}

/// A candidate: its point (the best of its points), how many points it stands for and their extent.
#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub tier: Tier,
    pub score: i32,
    /// Its best point; for several points of which none is the unit asked for, without a unit or GERS ID (they would be
    /// one unit's), and for a street the point nearest the middle of its points. Its sources are those of all its points.
    pub point: Point,
    pub points: usize,
    /// How many of its points are a unit's.
    pub units: usize,
    /// west, south, east, north of its points, when more than one.
    pub extent: Option<[f64; 4]>,
    /// Whether the postal code asked for agrees with the point's (None when either has none).
    pub postcode_agrees: Option<bool>,
    pub locality_agrees: Option<bool>,
    pub unit_agrees: Option<bool>,
}

impl Candidate {
    /// Whether the postal code or the locality asked for differs from the point's.
    #[must_use]
    pub fn disagrees(&self) -> bool {
        self.postcode_agrees == Some(false) || self.locality_agrees == Some(false)
    }

    /// Its GeoPointPrecision (plans/archive/Maps.md §6.1): an address match only for a number and street whose postal code and
    /// locality do not differ from what was asked; otherwise an estimate.
    #[must_use]
    pub fn precision(&self) -> &'static str {
        if self.tier == Tier::Address && self.disagrees() {
            "GPP_PLACE_ESTIMATE"
        } else {
            self.tier.precision()
        }
    }
}

/// The candidates of a query, the best first.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Ranked {
    pub candidates: Vec<Candidate>,
    pub ambiguous: bool,
}

/// Metres between two points, near enough for a few hundred.
#[must_use]
pub fn distance_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let dx = (a.0 - b.0) * 111_320.0 * ((a.1 + b.1) / 2.0).to_radians().cos();
    let dy = (a.1 - b.1) * 110_570.0;
    (dx * dx + dy * dy).sqrt()
}

/// Whether what a query asked for agrees with a point's: each None when either has none.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Agreement {
    postcode: Option<bool>,
    locality: Option<bool>,
    unit: Option<bool>,
}

/// Score `point` for `query`: the postal code, the locality and the unit asked for, against the point's.
fn score(query: &Query, locality: Option<&str>, point: &Point) -> (i32, Agreement) {
    let mut score = 100;
    let postcode = query
        .postcode
        .as_deref()
        .filter(|_| !point.postcode_key.is_empty())
        .map(|code| postcode_key(code, &point.country) == point.postcode_key);
    match postcode {
        Some(true) => score += 20,
        Some(false) => score -= 40,
        None => {}
    }
    let locality = locality
        .filter(|_| !point.places.is_empty())
        .map(|locality| point.places.iter().any(|place| place == locality));
    match locality {
        Some(true) => score += 10,
        Some(false) => score -= 20,
        None => {}
    }
    let unit = match (&query.unit, point.unit_key.is_empty()) {
        (Some(unit), false) => Some(unit_key(unit) == point.unit_key),
        (Some(_), true) => Some(false),
        (None, _) => None,
    };
    match unit {
        Some(true) => score += 5,
        Some(false) => score -= 3,
        // no unit asked: the address itself before its units
        None if !point.unit_key.is_empty() => score -= 1,
        None => {}
    }
    (
        score,
        Agreement {
            postcode,
            locality,
            unit,
        },
    )
}

/// `list`'s comma separated items with `more`'s added, each once.
fn joined(list: &str, more: &str) -> String {
    let mut items: Vec<&str> = list.split(", ").filter(|item| !item.is_empty()).collect();
    for item in more.split(", ").filter(|item| !item.is_empty()) {
        if !items.contains(&item) {
            items.push(item);
        }
    }
    items.join(", ")
}

/// Rank `points` (the rows of one street key, of `tier`) for `query` with `locality` (folded): each scored, those of one
/// place (within [`SAME_PLACE_M`] of a better one, or for the street's tier of one locality) made one candidate, and the
/// candidates within [`MARGIN`] of the best kept, the best first and the nearest to `near` (the view's middle) next.
/// Several candidates, an estimate of a number and a candidate whose postal code or locality differs from what was asked
/// are ambiguous: choices, none chosen.
#[must_use]
pub fn rank(query: &Query, locality: Option<&str>, tier: Tier, points: Vec<Point>, near: Option<(f64, f64)>) -> Ranked {
    let mut scored: Vec<(i32, Agreement, Point)> = points
        .into_iter()
        .map(|point| {
            let (score, agreement) = score(query, locality, &point);
            (score, agreement, point)
        })
        .collect();
    let distance = |point: &Point| near.map_or(0.0, |near| distance_m(near, (point.lon, point.lat)));
    scored.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(distance(&a.2).total_cmp(&distance(&b.2)))
            .then(a.2.address_id.cmp(&b.2.address_id))
    });
    let mut candidates: Vec<Candidate> = Vec::new();
    // each candidate's points, for a street's point nearest their middle
    let mut members: Vec<Vec<(f64, f64)>> = Vec::new();
    for (score, agreement, point) in scored {
        let same = candidates.iter().position(|candidate| match tier {
            Tier::Street => {
                // a street's points are one candidate a locality (or a postal code where the source has no locality)
                fold(&candidate.point.locality) == fold(&point.locality)
                    && (!point.locality.is_empty() || candidate.point.postcode_key == point.postcode_key)
            }
            _ => distance_m((candidate.point.lon, candidate.point.lat), (point.lon, point.lat)) <= SAME_PLACE_M,
        });
        match same.and_then(|index| candidates.get_mut(index).zip(members.get_mut(index))) {
            Some((candidate, positions)) => {
                candidate.points += 1;
                if !point.unit_key.is_empty() {
                    candidate.units += 1;
                }
                candidate.point.datasets = joined(&candidate.point.datasets, &point.datasets);
                candidate.point.licenses = joined(&candidate.point.licenses, &point.licenses);
                let extent = candidate.extent.unwrap_or([
                    candidate.point.lon,
                    candidate.point.lat,
                    candidate.point.lon,
                    candidate.point.lat,
                ]);
                candidate.extent = Some([
                    extent[0].min(point.lon),
                    extent[1].min(point.lat),
                    extent[2].max(point.lon),
                    extent[3].max(point.lat),
                ]);
                positions.push((point.lon, point.lat));
            }
            None => {
                members.push(vec![(point.lon, point.lat)]);
                candidates.push(Candidate {
                    tier,
                    score,
                    units: usize::from(!point.unit_key.is_empty()),
                    point,
                    points: 1,
                    extent: None,
                    postcode_agrees: agreement.postcode,
                    locality_agrees: agreement.locality,
                    unit_agrees: agreement.unit,
                });
            }
        }
    }
    for (candidate, positions) in candidates.iter_mut().zip(&members) {
        // a street's candidate is its point nearest the middle of its points, so it lies on the street
        if tier == Tier::Street {
            if let Some([w, s, e, n]) = candidate.extent {
                let middle = ((w + e) / 2.0, (s + n) / 2.0);
                if let Some(nearest) = positions
                    .iter()
                    .min_by(|a, b| distance_m(middle, **a).total_cmp(&distance_m(middle, **b)))
                {
                    (candidate.point.lon, candidate.point.lat) = *nearest;
                }
            }
        }
        // several points none of which is the unit asked for, or a unit asked for that is not there: the address, not
        // one unit of it (whose unit and GERS ID its best point would otherwise give)
        if candidate.points > 1 && candidate.unit_agrees != Some(true) || candidate.unit_agrees == Some(false) {
            candidate.point.unit.clear();
            candidate.point.unit_key.clear();
            candidate.point.gers_id.clear();
        }
    }
    let best = candidates.first().map_or(0, |candidate| candidate.score);
    candidates.retain(|candidate| candidate.score >= best - MARGIN);
    let ambiguous = candidates.len() > 1 || tier == Tier::NumberBase || candidates.iter().any(Candidate::disagrees);
    Ranked { candidates, ambiguous }
}

/// Title case for a name the source wrote in capitals (`HEMENWAY STREET` as `Hemenway Street`, `VIA DELL'ANGUILLARA` as
/// `Via Dell'Anguillara`); a name in mixed case stays as written.
#[must_use]
pub fn display(text: &str) -> String {
    if text.chars().any(char::is_lowercase) {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut start = true;
    for c in text.chars() {
        if start {
            out.extend(c.to_uppercase());
        } else {
            out.extend(c.to_lowercase());
        }
        start = matches!(c, ' ' | '-' | '/' | '\'' | '(');
    }
    out
}

/// A candidate's label: number and street in the country's order, the unit, then the locality, region and postal code.
#[must_use]
pub fn label(point: &Point, tier: Tier) -> String {
    let street = display(&point.street);
    let mut line = match (tier, point.number.is_empty()) {
        (Tier::Street, _) | (_, true) => street,
        _ if number_first(&point.country) => format!("{} {street}", point.number),
        _ => format!("{street} {}", point.number),
    };
    if tier != Tier::Street && !point.unit.is_empty() {
        line.push_str(" #");
        line.push_str(&point.unit);
    }
    let locality = display(&point.locality);
    let place = match point.country.as_str() {
        "US" | "CA" | "AU" => [locality, point.region.clone(), point.postcode.clone()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
        _ => [point.postcode.clone(), locality]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" "),
    };
    [line, place]
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The most street keys tried for one country's reading of a text.
pub const MAX_STREETS: usize = 8;

/// The order of the tiers, the best first.
#[must_use]
pub fn tier_order(tier: Tier) -> u8 {
    match tier {
        Tier::Address => 0,
        Tier::NumberBase => 1,
        Tier::Street => 2,
    }
}

/// Whether one country's best (`tier`, `ranked`) is better than the best so far: a better tier, or the same tier with a
/// higher score.
#[must_use]
pub fn better(tier: Tier, ranked: &Ranked, best: Option<(Tier, &Ranked)>) -> bool {
    match best {
        None => true,
        Some((best_tier, best_ranked)) => {
            tier_order(tier) < tier_order(best_tier)
                || tier == best_tier
                    && ranked.candidates.first().map_or(i32::MIN, |c| c.score)
                        > best_ranked.candidates.first().map_or(i32::MIN, |c| c.score)
        }
    }
}

/// What [`search_tiers`] reads points with: `(street key, number key)` to the points of the street (of the number).
pub type RowsOf<'a, E> = dyn FnMut(&str, Option<&str>) -> Result<Vec<Point>, E> + 'a;

/// The best tier found for one country's reading of a text: for each street key of the query (at most
/// [`MAX_STREETS`]), the number on it, then the number without its suffix (a suffix the source keeps as the unit, Italy's
/// `12/A`, is the address), then the street alone. `rows(street, number)` gives the points of a street key of `country`,
/// those of one number key when given: the server's index (`moqui-maps` `address_service`) and an offline package's
/// rows (`moqui-client` `packages::addresses`, plans/archive/Maps-Offline.md §2.5) are both searched by this one walk.
///
/// # Errors
///
/// What `rows` answers.
pub fn search_tiers<E>(
    query: &Query,
    country: &str,
    near: Option<(f64, f64)>,
    rows: &mut RowsOf<'_, E>,
) -> Result<Option<(Tier, Ranked)>, E> {
    let number = query.number.as_deref().map(number_key).filter(|key| !key.is_empty());
    for (street, locality) in query.streets(country).into_iter().take(MAX_STREETS) {
        let locality = locality.as_deref().map(fold);
        if let Some(number) = number.as_deref() {
            let found = rows(&street, Some(number))?;
            if !found.is_empty() {
                return Ok(Some((
                    Tier::Address,
                    rank(query, locality.as_deref(), Tier::Address, found, near),
                )));
            }
            if let Some(base) = number_base(number) {
                let found = rows(&street, Some(&base))?;
                let suffix = number
                    .get(base.len()..)
                    .unwrap_or_default()
                    .trim_matches(['/', '-'])
                    .to_string();
                if query.unit.is_none() && !suffix.is_empty() && found.iter().any(|point| point.unit_key == suffix) {
                    let as_unit = Query {
                        unit: Some(suffix.clone()),
                        ..query.clone()
                    };
                    let units: Vec<Point> = found.into_iter().filter(|point| point.unit_key == suffix).collect();
                    return Ok(Some((
                        Tier::Address,
                        rank(&as_unit, locality.as_deref(), Tier::Address, units, near),
                    )));
                }
                if !found.is_empty() {
                    return Ok(Some((
                        Tier::NumberBase,
                        rank(query, locality.as_deref(), Tier::NumberBase, found, near),
                    )));
                }
            }
        }
        let found = rows(&street, None)?;
        if !found.is_empty() {
            return Ok(Some((
                Tier::Street,
                rank(query, locality.as_deref(), Tier::Street, found, near),
            )));
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "address_tests.rs"]
mod tests;
