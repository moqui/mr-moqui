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

use super::{clean_tag, model_fields};
use moqui_lib::object::Value;
use serde_json::json;

#[test]
fn tags_are_one_trimmed_line_of_64_characters_at_most() {
    assert_eq!(clean_tag("  review "), Some("review".into()));
    assert_eq!(clean_tag(""), None);
    assert_eq!(clean_tag("two\nlines"), None);
    assert_eq!(clean_tag(&"x".repeat(64)), Some("x".repeat(64)));
    assert_eq!(clean_tag(&"x".repeat(65)), None);
}

#[test]
fn a_model_selection_picks_the_engine() {
    let native = model_fields(&json!({"instanceId": "native", "model": "gpt"})).unwrap();
    assert!(native.contains(&("engineEnumId", Value::string("AgengNative"))));
    assert!(native.contains(&("modelId", Value::string("gpt"))));
    let harness = model_fields(&json!({"instanceId": "claude", "model": "opus", "effort": "high"})).unwrap();
    assert!(harness.contains(&("engineEnumId", Value::string("AgengHarness"))));
    assert!(harness.contains(&("providerInstanceId", Value::string("claude"))));
    assert!(harness.contains(&("harnessModel", Value::string("opus"))));
    assert!(harness.contains(&("reasoningLevel", Value::string("high"))));
    assert!(model_fields(&json!("claude")).is_err());
}
