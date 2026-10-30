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

//! vCard 2.1, 3.0 (RFC 2426) and 4.0 (RFC 6350) read into contact maps, and contact maps written as 4.0 or 3.0.
//!
//! A card reads into the flat map a contact importer maps columns from, every value text: `fullName`, `prefix`,
//! `firstName`, `middleName`, `lastName`, `suffix`, `nickname`, `organizationName`, `department`, `jobTitle`,
//! `birthday` (`yyyy-MM-dd`, or `--MM-dd` without a year), numbered slots `email1`, `phone1`, `website1` with their
//! `email1.label` and `phone1.label` (Home, Work, Mobile, Fax, Home Fax, Main, Pager, Other, or the Apple label; `* `
//! before the preferred one), `address1.street`, `.street2`, `.city`, `.region`, `.postalCode`, `.country`, `.poBox`,
//! `.formatted` and `.label`, `notes`, `groups` (comma separated) and `uid` (the card's UID as written). A group card (4.0 KIND:group, Apple's
//! X-ADDRESSBOOKSERVER-KIND:group) is not a contact: its name goes in the groups of its members. An organization card
//! (KIND:org, Apple's X-ABShowAs:COMPANY) has no person names. Quoted-printable 2.1 values are read in their CHARSET.

use indexmap::IndexMap;
use moqui_lib::object::{to_plain_string, Value};

use super::line::{self, Property};

struct Card {
    version: String,
    properties: Vec<Property>,
}

fn cards(text: &str) -> Vec<Card> {
    let mut cards = Vec::new();
    let mut current: Option<Card> = None;
    // a 2.1 AGENT holds a card of its own, which is not read
    let mut nested = 0;
    for logical in line::unfold(text) {
        let Some(property) = line::parse_line(&logical) else {
            continue;
        };
        let is_card = property.value.trim().eq_ignore_ascii_case("VCARD");
        match property.name.as_str() {
            "BEGIN" if is_card => {
                if current.is_some() {
                    nested += 1;
                } else {
                    current = Some(Card {
                        version: "3.0".to_string(),
                        properties: Vec::new(),
                    });
                }
            }
            "END" if is_card => {
                if nested > 0 {
                    nested -= 1;
                } else if let Some(card) = current.take() {
                    cards.push(card);
                }
            }
            "VERSION" if nested == 0 => {
                if let Some(card) = current.as_mut() {
                    card.version = property.value.trim().to_string();
                }
            }
            _ if nested == 0 => {
                if let Some(card) = current.as_mut() {
                    card.properties.push(property);
                }
            }
            _ => {}
        }
    }
    cards
}

/// `_$!<Mobile>!$_` is Apple's name for a standard label.
fn apple_label(text: &str) -> String {
    let text = text.trim();
    text.strip_prefix("_$!<")
        .and_then(|rest| rest.strip_suffix(">!$_"))
        .unwrap_or(text)
        .to_string()
}

fn capitalized(text: &str) -> String {
    let mut chars = text.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().to_lowercase(),
        None => String::new(),
    }
}

impl Card {
    fn modern(&self) -> bool {
        !self.version.starts_with('2')
    }

    fn all<'a>(&'a self, name: &'a str) -> impl Iterator<Item = &'a Property> + 'a {
        self.properties.iter().filter(move |property| property.name == name)
    }

    fn first(&self, name: &str) -> Option<&Property> {
        self.properties.iter().find(|property| property.name == name)
    }

    fn text(&self, property: &Property) -> String {
        line::unescape(&property.value).trim().to_string()
    }

    /// The structured parts of a value (split on `;`), each a list (split on `,` from 3.0 on), unescaped.
    fn parts(&self, property: &Property) -> Vec<Vec<String>> {
        line::split_unescaped(&property.value, ';')
            .into_iter()
            .map(|part| {
                let items = if self.modern() {
                    line::split_unescaped(&part, ',')
                } else {
                    vec![part]
                };
                items
                    .iter()
                    .map(|item| line::unescape(item).trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect()
            })
            .collect()
    }

    /// The X-ABLabel of the property's group (Apple's itemN).
    fn label_of(&self, property: &Property) -> Option<String> {
        let group = property.group.as_deref()?;
        self.properties
            .iter()
            .find(|other| {
                other.name == "X-ABLABEL" && other.group.as_deref().is_some_and(|g| g.eq_ignore_ascii_case(group))
            })
            .map(|other| apple_label(&line::unescape(&other.value)))
            .filter(|label| !label.is_empty())
    }

    fn kind(&self) -> String {
        self.first("KIND")
            .or_else(|| self.first("X-ADDRESSBOOKSERVER-KIND"))
            .map(|property| self.text(property).to_ascii_lowercase())
            .unwrap_or_default()
    }

    fn is_organization(&self) -> bool {
        matches!(self.kind().as_str(), "org" | "organization")
            || self
                .first("X-ABSHOWAS")
                .is_some_and(|property| self.text(property).eq_ignore_ascii_case("COMPANY"))
    }

    fn uid(&self) -> Option<String> {
        self.first("UID")
            .map(|property| self.text(property))
            .filter(|uid| !uid.is_empty())
    }
}

/// How preferred a property is: 4.0 PREF (1 is most), or TYPE=pref.
fn preference(property: &Property) -> Option<u32> {
    if let Some(pref) = property.param("PREF").and_then(|pref| pref.trim().parse::<u32>().ok()) {
        return Some(pref);
    }
    property.has_type("pref").then_some(1)
}

/// The index of the most preferred of the properties, if any is.
fn preferred(properties: &[&Property]) -> Option<usize> {
    properties
        .iter()
        .enumerate()
        .filter_map(|(index, property)| preference(property).map(|pref| (pref, index)))
        .min()
        .map(|(_, index)| index)
}

fn phone_label(property: &Property) -> String {
    let has = |kind: &str| property.has_type(kind);
    if has("fax") {
        if has("home") {
            "Home Fax".to_string()
        } else if has("work") {
            "Work Fax".to_string()
        } else {
            "Fax".to_string()
        }
    } else if has("cell") || has("mobile") || has("iphone") {
        "Mobile".to_string()
    } else if has("pager") {
        "Pager".to_string()
    } else if has("main") || has("x-main") {
        "Main".to_string()
    } else if has("home") {
        "Home".to_string()
    } else if has("work") {
        "Work".to_string()
    } else if has("other") {
        "Other".to_string()
    } else {
        String::new()
    }
}

fn place_label(property: &Property) -> String {
    ["home", "work", "other"]
        .iter()
        .find(|kind| property.has_type(kind))
        .map(|kind| capitalized(kind))
        .unwrap_or_default()
}

/// A birthday as `yyyy-MM-dd`, `--MM-dd` without a year (Apple's X-APPLE-OMIT-YEAR too), or the text as it is.
fn birthday(property: &Property, text: &str) -> String {
    if property
        .param("VALUE")
        .is_some_and(|value| value.eq_ignore_ascii_case("text"))
    {
        return text.to_string();
    }
    let date = text.split('T').next().unwrap_or(text).trim();
    let digits: String = date.chars().filter(char::is_ascii_digit).collect();
    if date.starts_with("--") && digits.len() == 4 {
        return format!("--{}-{}", &digits[0..2], &digits[2..4]);
    }
    if digits.len() == 8 && (date.len() == 8 || date.len() == 10) {
        let year = &digits[0..4];
        if property
            .param("X-APPLE-OMIT-YEAR")
            .is_some_and(|omit| omit.trim() == year)
        {
            return format!("--{}-{}", &digits[4..6], &digits[6..8]);
        }
        return format!("{year}-{}-{}", &digits[4..6], &digits[6..8]);
    }
    text.to_string()
}

/// The contacts of a vCard file, one map per card (group cards are not contacts), keyed as the module doc says.
pub(crate) fn parse(text: &str) -> Vec<IndexMap<String, Value>> {
    let cards = cards(text);
    // the groups each member is in, by UID
    let mut groups_of: IndexMap<String, Vec<String>> = IndexMap::new();
    for card in cards.iter().filter(|card| card.kind() == "group") {
        let name = card
            .first("FN")
            .map(|property| card.text(property))
            .or_else(|| {
                card.first("N")
                    .and_then(|property| card.parts(property).first().and_then(|part| part.first().cloned()))
            })
            .unwrap_or_default();
        if name.is_empty() {
            continue;
        }
        for member in card.all("MEMBER").chain(card.all("X-ADDRESSBOOKSERVER-MEMBER")) {
            let uid = card.text(member);
            let uid = uid.strip_prefix("urn:uuid:").unwrap_or(&uid).to_string();
            groups_of.entry(uid).or_default().push(name.clone());
        }
    }
    cards
        .iter()
        .filter(|card| card.kind() != "group")
        .map(|card| {
            let uid = card
                .uid()
                .map(|uid| uid.strip_prefix("urn:uuid:").unwrap_or(&uid).to_string());
            let member_of = uid
                .as_ref()
                .and_then(|uid| groups_of.get(uid).cloned())
                .unwrap_or_default();
            let mut contact = contact(card, member_of);
            // the card's own UID as written, which a CardDAV server keeps for the card
            if let Some(uid) = card
                .uid()
                .map(|uid| uid.trim().to_string())
                .filter(|uid| !uid.is_empty())
            {
                contact.insert("uid".to_string(), Value::String(uid));
            }
            contact
        })
        .collect()
}

fn contact(card: &Card, member_of: Vec<String>) -> IndexMap<String, Value> {
    let mut out: IndexMap<String, Value> = IndexMap::new();
    let put = |out: &mut IndexMap<String, Value>, key: String, value: String| {
        let value = value.trim().to_string();
        if !value.is_empty() {
            out.insert(key, Value::String(value));
        }
    };
    let organization = card.is_organization();
    let full_name = card.first("FN").map(|property| card.text(property)).unwrap_or_default();
    let name_parts = card.first("N").map(|property| card.parts(property)).unwrap_or_default();
    let name_part = |index: usize| name_parts.get(index).map(|items| items.join(" ")).unwrap_or_default();
    let org_parts = card
        .first("ORG")
        .map(|property| card.parts(property))
        .unwrap_or_default();
    let mut organization_name = org_parts.first().map(|items| items.join(", ")).unwrap_or_default();
    if organization && organization_name.is_empty() {
        organization_name = full_name.clone();
    }
    if !organization {
        put(&mut out, "fullName".into(), full_name);
        put(&mut out, "prefix".into(), name_part(3));
        put(&mut out, "firstName".into(), name_part(1));
        put(&mut out, "middleName".into(), name_part(2));
        put(&mut out, "lastName".into(), name_part(0));
        put(&mut out, "suffix".into(), name_part(4));
    }
    let nicknames: Vec<String> = card
        .all("NICKNAME")
        .flat_map(|property| card.parts(property).into_iter().flatten())
        .collect();
    put(&mut out, "nickname".into(), nicknames.join(", "));
    put(&mut out, "organizationName".into(), organization_name);
    let department: Vec<String> = org_parts.iter().skip(1).flatten().cloned().collect();
    put(&mut out, "department".into(), department.join(", "));
    let titles: Vec<String> = card
        .all("TITLE")
        .map(|property| card.text(property))
        .filter(|title| !title.is_empty())
        .collect();
    put(&mut out, "jobTitle".into(), titles.join(", "));
    if let Some(property) = card.first("BDAY") {
        let text = card.text(property);
        put(&mut out, "birthday".into(), birthday(property, &text));
    }

    // emails and phones, each a slot with its label, the preferred one marked with *
    let emails: Vec<&Property> = card
        .all("EMAIL")
        .filter(|property| !card.text(property).is_empty())
        .collect();
    let preferred_email = preferred(&emails);
    for (index, property) in emails.iter().enumerate() {
        let slot = index + 1;
        let address = card.text(property);
        let address = address.strip_prefix("mailto:").unwrap_or(&address).to_string();
        put(&mut out, format!("email{slot}"), address);
        let label = card.label_of(property).unwrap_or_else(|| place_label(property));
        let star = if preferred_email == Some(index) { "* " } else { "" };
        put(&mut out, format!("email{slot}.label"), format!("{star}{label}"));
    }
    let phones: Vec<&Property> = card
        .all("TEL")
        .filter(|property| !card.text(property).is_empty())
        .collect();
    let preferred_phone = preferred(&phones);
    for (index, property) in phones.iter().enumerate() {
        let slot = index + 1;
        let mut number = card.text(property);
        if let Some(uri) = number.strip_prefix("tel:") {
            // tel:+1-555-555-0100;ext=12
            let mut parts = uri.split(';');
            let base = parts.next().unwrap_or_default().replace('-', " ");
            let extension = parts
                .find_map(|part| part.strip_prefix("ext="))
                .map(|ext| format!(" ext. {ext}"))
                .unwrap_or_default();
            number = format!("{base}{extension}");
        }
        put(&mut out, format!("phone{slot}"), number);
        let label = card.label_of(property).unwrap_or_else(|| phone_label(property));
        let star = if preferred_phone == Some(index) { "* " } else { "" };
        put(&mut out, format!("phone{slot}.label"), format!("{star}{label}"));
    }

    // addresses: PO box, extended, street, city, region, postal code, country; a street of several lines keeps the
    // first and moves the rest to street2
    let addresses: Vec<&Property> = card.all("ADR").collect();
    let mut labels: Vec<&Property> = card.all("LABEL").collect();
    let mut slot = 0;
    for property in &addresses {
        let parts = card.parts(property);
        let part = |index: usize| parts.get(index).cloned().unwrap_or_default();
        let street_lines: Vec<String> = part(2)
            .iter()
            .flat_map(|street| {
                street
                    .split('\n')
                    .map(|line| line.trim().to_string())
                    .collect::<Vec<_>>()
            })
            .filter(|line| !line.is_empty())
            .collect();
        let mut street2: Vec<String> = street_lines.iter().skip(1).cloned().collect();
        street2.extend(part(1));
        let formatted = property.param("LABEL").map(str::to_string).or_else(|| {
            // a 3.0 or 2.1 LABEL of the same kind
            let kind = place_label(property);
            let found = labels.iter().position(|label| place_label(label) == kind)?;
            Some(card.text(labels.remove(found)))
        });
        let values = [
            ("street", street_lines.first().cloned().unwrap_or_default()),
            ("street2", street2.join(", ")),
            ("city", part(3).join(" ")),
            ("region", part(4).join(" ")),
            ("postalCode", part(5).join(" ")),
            ("country", part(6).join(" ")),
            ("poBox", part(0).join(" ")),
            ("formatted", formatted.unwrap_or_default()),
        ];
        if values.iter().all(|(_, value)| value.trim().is_empty()) {
            continue;
        }
        slot += 1;
        for (name, value) in values {
            put(&mut out, format!("address{slot}.{name}"), value);
        }
        let label = card.label_of(property).unwrap_or_else(|| place_label(property));
        put(&mut out, format!("address{slot}.label"), label);
    }
    // a LABEL without an ADR is an address of its own
    for label in labels {
        slot += 1;
        put(&mut out, format!("address{slot}.formatted"), card.text(label));
        put(&mut out, format!("address{slot}.label"), place_label(label));
    }

    for (index, property) in card
        .all("URL")
        .filter(|property| !card.text(property).is_empty())
        .enumerate()
    {
        put(&mut out, format!("website{}", index + 1), card.text(property));
    }
    let notes: Vec<String> = card
        .all("NOTE")
        .map(|property| card.text(property))
        .filter(|note| !note.is_empty())
        .collect();
    put(&mut out, "notes".into(), notes.join("\n"));
    let mut groups: Vec<String> = Vec::new();
    for property in card.all("CATEGORIES") {
        let items: Vec<String> = if card.modern() {
            card.parts(property).into_iter().flatten().collect()
        } else {
            card.text(property)
                .split([',', ';'])
                .map(|item| item.trim().to_string())
                .collect()
        };
        for item in items {
            // Google's own labels, as its CSV writes them: the importer skips a label with a star and reads starred as a
            // favorite
            let item = match item.to_ascii_lowercase().as_str() {
                "mycontacts" => "* myContacts".to_string(),
                "starred" => "* starred".to_string(),
                _ => item,
            };
            if !item.is_empty() && !groups.iter().any(|have| have.eq_ignore_ascii_case(&item)) {
                groups.push(item);
            }
        }
    }
    for group in member_of {
        if !groups.iter().any(|have| have.eq_ignore_ascii_case(&group)) {
            groups.push(group);
        }
    }
    put(&mut out, "groups".into(), groups.join(", "));
    out
}

// ===== writing

#[derive(Default)]
struct Item {
    value: String,
    types: Vec<&'static str>,
}

#[derive(Default)]
struct Address {
    po_box: String,
    extended: String,
    street: String,
    city: String,
    region: String,
    postal_code: String,
    country: String,
    label: String,
    types: Vec<&'static str>,
}

fn text_of(map: &IndexMap<String, Value>, keys: &[&str]) -> String {
    keys.iter()
        .filter_map(|key| map.get(*key))
        .map(|value| to_plain_string(value).trim().to_string())
        .find(|text| !text.is_empty() && text != "null")
        .unwrap_or_default()
}

fn flag_of(map: &IndexMap<String, Value>, key: &str) -> bool {
    match map.get(key) {
        Some(Value::Bool(flag)) => *flag,
        Some(value) => matches!(
            to_plain_string(value).trim().to_ascii_lowercase().as_str(),
            "true" | "y" | "yes" | "1"
        ),
        None => false,
    }
}

fn list_of<'a>(map: &'a IndexMap<String, Value>, key: &str) -> Vec<&'a IndexMap<String, Value>> {
    match map.get(key) {
        Some(Value::List(items)) => items
            .iter()
            .filter_map(|item| match item {
                Value::Map(entry) => Some(entry),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The TYPE values of a label from a read card or a CSV (`* Mobile`, `Work Fax`, `Home`).
fn types_of_label(label: &str, default: &'static str) -> Vec<&'static str> {
    let label = label.trim_start_matches('*').trim().to_ascii_lowercase();
    let mut types = Vec::new();
    if label.contains("mobile") || label.contains("cell") || label.contains("iphone") {
        types.push("cell");
    }
    if label.contains("fax") {
        types.push("fax");
    }
    if label.contains("pager") {
        types.push("pager");
    }
    if label.contains("home") {
        types.push("home");
    }
    if label.contains("work") || label.contains("business") || label.contains("office") {
        types.push("work");
    }
    if label.contains("main") || label.contains("primary") {
        types.push("voice");
    }
    if types.is_empty() {
        types.push(default);
    }
    types
}

struct Contact {
    uid: String,
    organization: bool,
    full_name: String,
    names: [String; 5],
    nickname: String,
    organization_name: String,
    department: String,
    title: String,
    birthday: String,
    notes: String,
    emails: Vec<Item>,
    phones: Vec<Item>,
    addresses: Vec<Address>,
    urls: Vec<String>,
    groups: Vec<String>,
}

/// A contact from the map of an export (emails, phones, addresses and websites as lists of contact mech maps with
/// a contactMechPurposeId) or the flat map `parse` gives (email1, phone1.label, address1.city, ...).
fn contact_of(map: &IndexMap<String, Value>) -> Contact {
    let text = |keys: &[&str]| text_of(map, keys);
    let party_id = text(&["partyId"]);
    let uid = match text(&["uid", "universalId"]) {
        uid if !uid.is_empty() => uid,
        _ if !party_id.is_empty() => format!("urn:moqui:mantle.party.Party:{party_id}"),
        _ => String::new(),
    };
    let names = [
        text(&["lastName"]),
        text(&["firstName"]),
        text(&["middleName"]),
        text(&["personalTitle", "prefix"]),
        text(&["suffix"]),
    ];
    let organization_name = text(&["organizationName"]);
    let organization = flag_of(map, "isOrganization")
        || (names.iter().all(String::is_empty)
            && text(&["fullName", "name"]).is_empty()
            && !organization_name.is_empty());
    let mut full_name = text(&["name", "fullName"]);
    if full_name.is_empty() {
        full_name = [&names[3], &names[1], &names[2], &names[0], &names[4]]
            .iter()
            .filter(|part| !part.is_empty())
            .map(|part| part.as_str())
            .collect::<Vec<_>>()
            .join(" ");
    }
    if full_name.is_empty() {
        full_name = if organization_name.is_empty() {
            party_id.clone()
        } else {
            organization_name.clone()
        };
    }

    let mut emails: Vec<Item> = list_of(map, "emails")
        .into_iter()
        .map(|email| {
            let purpose = text_of(email, &["contactMechPurposeId"]);
            let kind = match purpose.as_str() {
                "EmailPrimary" => "home",
                "EmailOther" => "other",
                _ => "work",
            };
            Item {
                value: text_of(email, &["emailAddress", "infoString"]),
                types: vec![kind],
            }
        })
        .collect();
    let mut phones: Vec<Item> = list_of(map, "phones")
        .into_iter()
        .map(|phone| {
            let purpose = text_of(phone, &["contactMechPurposeId"]);
            let kind = match purpose.as_str() {
                "PhoneMobile" => "cell",
                "PhoneHome" => "home",
                "PhoneWork" => "work",
                "PhoneFax" => "fax",
                _ => "voice",
            };
            let country = text_of(phone, &["countryCode"]);
            let mut value = [
                if country.is_empty() {
                    String::new()
                } else {
                    format!("+{}", country.trim_start_matches('+'))
                },
                text_of(phone, &["areaCode"]),
                text_of(phone, &["contactNumber"]),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" ");
            let extension = text_of(phone, &["extension"]);
            if !extension.is_empty() {
                value.push_str(&format!(" ext. {extension}"));
            }
            Item {
                value,
                types: vec![kind],
            }
        })
        .collect();
    let mut addresses: Vec<Address> = list_of(map, "addresses")
        .into_iter()
        .map(|address| {
            let purpose = text_of(address, &["contactMechPurposeId"]);
            let kind = match purpose.as_str() {
                "PostalHome" | "PostalPrimary" => "home",
                _ => "work",
            };
            Address {
                extended: text_of(address, &["address2"]),
                street: text_of(address, &["address1"]),
                city: text_of(address, &["city"]),
                region: text_of(
                    address,
                    &["stateGeoCodeAlpha2", "stateGeoName", "region", "stateProvinceGeoId"],
                ),
                postal_code: text_of(address, &["postalCode"]),
                country: text_of(address, &["countryName", "country", "countryGeoId"]),
                label: text_of(address, &["formatted", "label"]),
                types: vec![kind],
                ..Default::default()
            }
        })
        .collect();
    let mut urls: Vec<String> = list_of(map, "websites")
        .into_iter()
        .map(|website| text_of(website, &["webAddress", "infoString"]))
        .filter(|url| !url.is_empty())
        .collect();

    // the flat slots of a read card
    let slots = |prefix: &str| -> Vec<usize> {
        let mut numbers: Vec<usize> = map
            .keys()
            .filter_map(|key| key.strip_prefix(prefix))
            .filter_map(|rest| rest.split('.').next())
            .filter_map(|number| number.parse::<usize>().ok())
            .collect();
        numbers.sort_unstable();
        numbers.dedup();
        numbers
    };
    let mut preferred_email = None;
    for slot in slots("email") {
        let value = text(&[&format!("email{slot}")]);
        if value.is_empty() {
            continue;
        }
        let label = text(&[&format!("email{slot}.label")]);
        if label.starts_with('*') && preferred_email.is_none() {
            preferred_email = Some(emails.len());
        }
        emails.push(Item {
            value,
            types: types_of_label(&label, "other"),
        });
    }
    if let Some(index) = preferred_email.filter(|index| *index > 0) {
        let item = emails.remove(index);
        emails.insert(0, item);
    }
    let mut preferred_phone = None;
    for slot in slots("phone") {
        let value = text(&[&format!("phone{slot}")]);
        if value.is_empty() {
            continue;
        }
        let label = text(&[&format!("phone{slot}.label")]);
        if label.starts_with('*') && preferred_phone.is_none() {
            preferred_phone = Some(phones.len());
        }
        phones.push(Item {
            value,
            types: types_of_label(&label, "voice"),
        });
    }
    if let Some(index) = preferred_phone.filter(|index| *index > 0) {
        let item = phones.remove(index);
        phones.insert(0, item);
    }
    for slot in slots("address") {
        let part = |name: &str| text(&[&format!("address{slot}.{name}")]);
        let address = Address {
            po_box: part("poBox"),
            extended: part("street2"),
            street: part("street"),
            city: part("city"),
            region: part("region"),
            postal_code: part("postalCode"),
            country: part("country"),
            label: part("formatted"),
            types: match part("label").to_ascii_lowercase() {
                label if label.contains("home") => vec!["home"],
                label if label.contains("work") || label.contains("business") => vec!["work"],
                _ => Vec::new(),
            },
        };
        if [
            &address.street,
            &address.city,
            &address.postal_code,
            &address.po_box,
            &address.label,
        ]
        .iter()
        .any(|v| !v.is_empty())
        {
            addresses.push(address);
        }
    }
    for slot in slots("website") {
        let url = text(&[&format!("website{slot}")]);
        if !url.is_empty() {
            urls.push(url);
        }
    }

    let mut groups: Vec<String> = match map.get("groupNames") {
        Some(Value::List(items)) => items
            .iter()
            .map(to_plain_string)
            .map(|item| item.trim().to_string())
            .collect(),
        _ => Vec::new(),
    };
    groups.extend(text(&["groups"]).split([',', ';']).map(|item| item.trim().to_string()));
    // a favorite is Google's starred label; the starred labels of a read card are Google's own
    for group in &mut groups {
        if let Some(label) = group.strip_prefix('*') {
            *group = label.trim().to_string();
            if !group.eq_ignore_ascii_case("starred") {
                group.clear();
            }
        }
    }
    if flag_of(map, "isFavorite") {
        groups.insert(0, "starred".to_string());
    }
    groups.retain(|group| !group.is_empty());
    let mut seen = Vec::new();
    groups.retain(|group| {
        let key = group.to_lowercase();
        let new = !seen.contains(&key);
        seen.push(key);
        new
    });
    let department = text(&["department"]);
    Contact {
        uid,
        organization,
        full_name,
        names,
        nickname: text(&["nickname"]),
        organization_name,
        department,
        title: text(&["jobTitle", "title"]),
        birthday: text(&["birthDate", "birthday"]),
        notes: text(&["comments", "notes"]),
        emails,
        phones,
        addresses,
        urls,
        groups,
    }
}

/// `1985-04-12` as 4.0 `19850412` or 3.0 `1985-04-12`; `--04-12` as `--0412` or `--04-12`; anything else as it is.
fn bday(text: &str, modern: bool) -> String {
    let date = text.split(['T', ' ']).next().unwrap_or(text);
    let digits: String = date.chars().filter(char::is_ascii_digit).collect();
    if date.starts_with("--") && digits.len() == 4 {
        return if modern {
            format!("--{digits}")
        } else {
            format!("--{}-{}", &digits[0..2], &digits[2..4])
        };
    }
    if digits.len() == 8 {
        return if modern {
            digits
        } else {
            format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8])
        };
    }
    text.to_string()
}

/// The contacts as vCard text, version `4.0` (the default) or `3.0`: CRLF line ends, lines folded at 75 octets,
/// the first email and phone preferred.
pub(crate) fn write(contacts: &[Value], version: &str) -> Result<String, String> {
    let modern = match version.trim() {
        "" | "4" | "4.0" => true,
        "3" | "3.0" => false,
        other => return Err(format!("vCard version {other} is not written (4.0 or 3.0)")),
    };
    let escape = line::escape;
    let mut cards = Vec::new();
    for value in contacts {
        let Value::Map(map) = value else {
            tracing::warn!("vCard write: a contact that is not a map is left out");
            continue;
        };
        let contact = contact_of(map);
        let mut lines = vec![
            "BEGIN:VCARD".to_string(),
            format!("VERSION:{}", if modern { "4.0" } else { "3.0" }),
        ];
        if !contact.uid.is_empty() {
            lines.push(format!("UID:{}", escape(&contact.uid)));
        }
        if contact.organization && modern {
            lines.push("KIND:org".to_string());
        }
        lines.push(format!("FN:{}", escape(&contact.full_name)));
        if !contact.organization {
            lines.push(format!(
                "N:{}",
                contact
                    .names
                    .iter()
                    .map(|part| escape(part))
                    .collect::<Vec<_>>()
                    .join(";")
            ));
        } else if !modern {
            // 3.0 needs N; Apple shows the card as a company
            lines.push("N:;;;;".to_string());
            lines.push("X-ABShowAs:COMPANY".to_string());
        }
        if !contact.nickname.is_empty() {
            lines.push(format!("NICKNAME:{}", escape(&contact.nickname)));
        }
        if !contact.organization_name.is_empty() || !contact.department.is_empty() {
            let mut org = escape(&contact.organization_name);
            if !contact.department.is_empty() {
                org.push(';');
                org.push_str(&escape(&contact.department));
            }
            lines.push(format!("ORG:{org}"));
        }
        if !contact.title.is_empty() {
            lines.push(format!("TITLE:{}", escape(&contact.title)));
        }
        if !contact.birthday.is_empty() {
            lines.push(format!("BDAY:{}", bday(&contact.birthday, modern)));
        }
        let typed = |name: &str, item: &Item, first: bool, extra: Option<&str>| -> String {
            if modern {
                let pref = if first { ";PREF=1" } else { "" };
                format!("{name};TYPE={}{pref}:{}", item.types.join(","), escape(&item.value))
            } else {
                let mut types: Vec<String> = extra.into_iter().map(str::to_string).collect();
                types.extend(item.types.iter().map(|kind| kind.to_ascii_uppercase()));
                if first {
                    types.push("PREF".to_string());
                }
                format!("{name};TYPE={}:{}", types.join(","), escape(&item.value))
            }
        };
        for (index, email) in contact
            .emails
            .iter()
            .filter(|email| !email.value.is_empty())
            .enumerate()
        {
            lines.push(typed("EMAIL", email, index == 0, Some("INTERNET")));
        }
        for (index, phone) in contact
            .phones
            .iter()
            .filter(|phone| !phone.value.is_empty())
            .enumerate()
        {
            lines.push(typed("TEL", phone, index == 0, None));
        }
        for address in &contact.addresses {
            let types = if modern {
                address.types.join(",")
            } else {
                address
                    .types
                    .iter()
                    .map(|kind| kind.to_ascii_uppercase())
                    .collect::<Vec<_>>()
                    .join(",")
            };
            let mut name = "ADR".to_string();
            if !types.is_empty() {
                name.push_str(&format!(";TYPE={types}"));
            }
            if modern && !address.label.is_empty() {
                name.push_str(&format!(";LABEL={}", line::param_value(&address.label)));
            }
            let parts = [
                &address.po_box,
                &address.extended,
                &address.street,
                &address.city,
                &address.region,
                &address.postal_code,
                &address.country,
            ];
            lines.push(format!(
                "{name}:{}",
                parts.iter().map(|part| escape(part)).collect::<Vec<_>>().join(";")
            ));
            if !modern && !address.label.is_empty() {
                let label_type = if types.is_empty() {
                    String::new()
                } else {
                    format!(";TYPE={types}")
                };
                lines.push(format!("LABEL{label_type}:{}", escape(&address.label)));
            }
        }
        for url in &contact.urls {
            lines.push(format!("URL:{}", escape(url)));
        }
        if !contact.groups.is_empty() {
            lines.push(format!(
                "CATEGORIES:{}",
                contact
                    .groups
                    .iter()
                    .map(|group| escape(group))
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
        if !contact.notes.is_empty() {
            lines.push(format!("NOTE:{}", escape(&contact.notes)));
        }
        lines.push("END:VCARD".to_string());
        cards.push(
            lines
                .iter()
                .map(|text| line::fold(text))
                .collect::<Vec<_>>()
                .join("\r\n"),
        );
    }
    Ok(if cards.is_empty() {
        String::new()
    } else {
        cards.join("\r\n") + "\r\n"
    })
}
