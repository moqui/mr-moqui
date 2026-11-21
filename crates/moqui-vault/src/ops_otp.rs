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

//! The module's one-time code calls (docs/security/Vaults.md, One-time codes): a vault item's codes are made here from
//! its seed, which never goes back to the page, so a list of every code an open vault holds keeps no seed in the page.
//!
//! - `item_otp`: the current and next code of an item's seed, and the time until it changes.
//! - `otp_parse`: text a user scanned or pasted (an `otpauth://totp/` URI, a bare secret, or a Google Authenticator
//!   export) read into the seeds it holds, each as the `otpauth` URI an item keeps, with its current code to compare.
//! - [`otp_header`]: what `item_summary` says of an item's seed: its issuer, account, algorithm, digits and period.

use serde_json::{json, Value};

use super::shapes::{bytes_of, i64_of, label_of, scrub, str_of, u64_of};
use super::Session;
use crate::otp::{self, Seed};
use crate::vault::{self, ItemRef};
use crate::Error;

/// The field an item keeps its seed in when the page names none: the login type's.
const DEFAULT_FIELD: &str = "totp";

/// The seed of a parsed item body, from the field the page names (`field`), else the first of the TOTP fields of
/// every type (`otp_fields`) that holds one, else `totp`. `None` when the item has no seed.
fn seed_text<'a>(body: &'a Value, args: &Value) -> Option<&'a str> {
    let fields = body.get("fields")?;
    let named = args.get("field").and_then(Value::as_str);
    let listed = args
        .get("otp_fields")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    named
        .into_iter()
        .chain(listed)
        .chain(std::iter::once(DEFAULT_FIELD))
        .find_map(|name| {
            fields
                .get(name)
                .and_then(Value::as_str)
                .filter(|text| !text.trim().is_empty())
        })
}

/// What a list shows of an item's seed, with no key: `None` when the item has none, `{"error"}` when it does not read.
pub(super) fn otp_header(body: &Value, args: &Value) -> Option<Value> {
    let text = seed_text(body, args)?;
    Some(match otp::parse(text) {
        Ok(seed) => params_json(&seed),
        Err(err) => json!({ "error": err.to_string() }),
    })
}

fn params_json(seed: &Seed) -> Value {
    json!({
        "issuer": seed.issuer, "account": seed.account, "algorithm": seed.algorithm.as_str(),
        "digits": seed.digits, "period": seed.period,
    })
}

fn codes_json(seed: &Seed, now: i64) -> Value {
    let codes = otp::codes(seed, now);
    json!({
        "code": codes.code, "next": codes.next, "remaining_ms": codes.remaining_ms,
        "period": seed.period, "digits": seed.digits,
    })
}

impl Session {
    /// `item_otp`: the item's codes at `now` (epoch milliseconds, the page's clock).
    pub(super) fn item_otp(&self, args: &Value) -> Result<Value, Error> {
        let item = ItemRef {
            vault_id: str_of(args, "vault_id")?,
            item_id: str_of(args, "item_id")?,
            key_version: u64_of(args, "key_version")?,
            label: label_of(args)?,
        };
        let now = i64_of(args, "now")?;
        let vault_key = self.vault_key(item.vault_id, item.key_version)?;
        let item_key = vault::unwrap_item_key(vault_key, &bytes_of(args, "wrapped_item_key")?, item)?;
        let body = vault::open_item_body(&item_key, item, &bytes_of(args, "sealed_body")?)?;
        let mut parsed: Value = serde_json::from_slice(&body).map_err(|_| Error::Format("a body is UTF-8 JSON"))?;
        let answer = match seed_text(&parsed, args) {
            Some(text) => otp::parse(text).map(|seed| codes_json(&seed, now)),
            None => Err(Error::Format("the item has no one-time code seed")),
        };
        scrub(&mut parsed);
        answer
    }
}

/// `otp_parse`: the seeds of `text`, each `{issuer, account, algorithm, digits, period, seed, code, next,
/// remaining_ms}` at `now`. A seed comes back as its `otpauth` URI for the page to seal as an item's field, as a
/// typed password is: the page had the text already.
pub(super) fn otp_parse(args: &Value) -> Result<Value, Error> {
    let text = str_of(args, "text")?;
    let now = i64_of(args, "now")?;
    // the prefix compared in place: a lower-cased copy of the text would be a copy of its secrets the scrub misses
    let migration = text
        .trim_start()
        .get(.."otpauth-migration:".len())
        .is_some_and(|start| start.eq_ignore_ascii_case("otpauth-migration:"));
    let seeds = if migration {
        otp::migration(text)?
    } else {
        vec![otp::parse(text)?]
    };
    let entries: Vec<Value> = seeds
        .iter()
        .map(|seed| {
            let mut entry = params_json(seed);
            if let (Value::Object(entry), Value::Object(codes)) = (&mut entry, codes_json(seed, now)) {
                entry.extend(codes);
                entry.insert("seed".to_string(), Value::String(otp::uri(seed)));
            }
            entry
        })
        .collect();
    Ok(json!({ "entries": entries }))
}
