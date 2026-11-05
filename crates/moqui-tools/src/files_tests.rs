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

use moqui_lib::object::Value;

use super::{infer_kind, names_of};

/// A column's kind is the one every non-empty value has: numbers and dates in text count, a mix is text.
#[test]
fn column_kinds_are_inferred_from_their_values() {
    let kind = |values: &[Value]| infer_kind(values.iter());
    assert_eq!(
        kind(&[Value::string("1"), Value::string(" 2.5"), Value::Null]),
        "number"
    );
    assert_eq!(kind(&[Value::I64(3), Value::F64(1.5)]), "number");
    assert_eq!(kind(&[Value::string("2026-11-05"), Value::string("")]), "date");
    assert_eq!(
        kind(&[Value::string("2026-11-05"), Value::string("2026-11-05T10:00:00")]),
        "date-time"
    );
    assert_eq!(kind(&[Value::string("1"), Value::string("one")]), "text");
    assert_eq!(kind(&[Value::Bool(true)]), "boolean");
    assert_eq!(kind(&[]), "text");
}

/// A blank header is the column's number, and a repeated one gets a suffix.
#[test]
fn header_names_are_unique_and_never_blank() {
    let header: Vec<String> = ["id", "", "id", "name"]
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    assert_eq!(names_of(&header, 5), ["id", "2", "id_2", "name", "5"]);
}
