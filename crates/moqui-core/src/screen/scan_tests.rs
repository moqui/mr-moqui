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

use indexmap::IndexMap;
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::{find_in_form, matches_of, ScanInput};

fn item(pairs: &[(&str, &str)]) -> Value {
    Value::Map(
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
            .collect::<IndexMap<_, _>>(),
    )
}

#[test]
fn a_scan_input_reads_its_resolver_types_and_context_names() {
    let form = MNode::parse_text(
        "form",
        r#"<form-single name="F"><field name="a"><default-field><text-line/></default-field></field>
            <field name="b"><conditional-field condition="x"><display/></conditional-field><default-field>
                <scan-input resolver=" r.S.resolve#X " allowed-types="asset, lot,,">
                    <parameter name="facilityId" from="f"/><depends-on field="loc" parameter="locationId"/><depends-on field="bin"/>
                </scan-input></default-field></field></form-single>"#,
    )
    .unwrap();
    assert!(find_in_form(&form, "a").is_none());
    assert!(find_in_form(&form, "c").is_none());
    let input = ScanInput::of(&find_in_form(&form, "b").unwrap()).unwrap();
    assert_eq!(
        input,
        ScanInput {
            resolver: "r.S.resolve#X".to_string(),
            allowed_types: vec!["asset".to_string(), "lot".to_string()],
            context_names: vec!["facilityId".to_string(), "locationId".to_string(), "bin".to_string()],
        }
    );
    let none = MNode::parse_text("s", r#"<scan-input resolver=""/>"#).unwrap();
    assert!(ScanInput::of(&none).is_none(), "no resolver, no scan");
}

#[test]
fn the_matches_keep_the_allowed_types_and_need_an_id() {
    let list = Value::List(vec![
        item(&[
            ("identityType", "asset"),
            ("id", "A1"),
            ("label", "Asset 1"),
            ("detail", "Bin 7"),
        ]),
        item(&[("identityType", "location"), ("id", "L1")]),
        item(&[("identityType", "lot"), ("label", "no id")]),
        item(&[("identityType", "lot"), ("id", "LOT1")]),
        Value::string("not a map"),
    ]);
    let ids = |matches: &[Value]| -> Vec<String> {
        matches
            .iter()
            .map(|value| match value {
                Value::Map(map) => moqui_lib::object::to_plain_string(map.get("id").unwrap()),
                _ => String::new(),
            })
            .collect()
    };
    let allowed = vec!["asset".to_string(), "lot".to_string()];
    let kept = matches_of(Some(&list), &allowed);
    assert_eq!(ids(&kept), ["A1", "LOT1"]);
    // a label defaults to the id, a detail to empty
    let Value::Map(lot) = &kept[1] else { panic!() };
    assert_eq!(lot.get("label"), Some(&Value::string("LOT1")));
    assert_eq!(lot.get("detail"), Some(&Value::string("")));
    // no types: any
    assert_eq!(ids(&matches_of(Some(&list), &[])), ["A1", "L1", "LOT1"]);
    assert!(matches_of(None, &[]).is_empty());
    assert!(matches_of(Some(&Value::string("x")), &[]).is_empty());
    let many = Value::List((0..80).map(|n| item(&[("id", &n.to_string())])).collect());
    assert_eq!(matches_of(Some(&many), &[]).len(), 50);
}
