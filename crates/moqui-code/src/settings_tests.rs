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

use super::{check, default_of, schema, store_of, Kind};
use serde_json::json;

#[test]
fn the_schema_parses_and_keeps_the_t3_defaults_with_the_moqui_changes() {
    assert!(schema().get("client").is_some_and(serde_json::Value::is_object));
    assert_eq!(default_of(Kind::Client, "sendShortcut"), json!("enter"));
    assert_eq!(default_of(Kind::Client, "notificationMode"), json!("off"));
    assert_eq!(default_of(Kind::Server, "defaultRuntimeMode"), json!("approval-required"));
    assert_eq!(default_of(Kind::Server, "branchNamePrefix"), json!("moqui"));
    assert_eq!(store_of("fontSizeCode"), Some("device"));
    assert_eq!(store_of("chatWidth"), Some("user"));
}

#[test]
fn values_are_checked_by_type_and_range() {
    assert!(check(Kind::Client, "chatWidth", &json!("wide")).is_ok());
    assert!(check(Kind::Client, "chatWidth", &json!("huge")).is_err());
    assert!(check(Kind::Client, "wordWrap", &json!("yes")).is_err());
    assert!(check(Kind::Client, "glassOpacity", &json!(101)).is_err());
    assert!(check(Kind::Client, "glassOpacity", &json!(50)).is_ok());
    assert!(check(Kind::Client, "browserCustomSearchUrl", &json!("x".repeat(2049))).is_err());
    assert!(check(Kind::Client, "favorites", &json!({})).is_err());
    assert!(check(Kind::Client, "noSuchSetting", &json!(1)).is_err());
    assert!(check(Kind::Policy, "limits.maxRuntimeMode", &json!("auto")).is_ok());
}
