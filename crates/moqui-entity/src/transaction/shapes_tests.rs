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

use super::*;

#[test]
fn statements_are_classified_by_their_text() {
    assert!(Kind::of_sql("SELECT A, B FROM ENUMERATION WHERE ENUM_ID = ?") == Kind::Read);
    assert!(Kind::of_sql("  select count(*) from order_item where order_id = ?") == Kind::Count);
    assert!(Kind::of_sql("SELECT * FROM PARTY WHERE PARTY_ID = ? FOR UPDATE") == Kind::Lock);
    assert!(Kind::of_sql("UPDATE SEQUENCE_VALUE_ITEM SET SEQ_NUM = ? WHERE SEQ_NAME = ?") == Kind::Sequence);
    assert!(Kind::of_sql("INSERT INTO ORDER_HEADER (ORDER_ID) VALUES (?)") == Kind::Write);
    assert!(Kind::of_sql("SET TIME ZONE 'UTC'") == Kind::Other);
}

#[test]
fn a_shape_is_named_by_its_verb_and_table() {
    assert_eq!(
        head_of("SELECT A, B FROM ENUMERATION WHERE ENUM_ID = ?"),
        "SELECT ENUMERATION"
    );
    assert_eq!(
        head_of("insert into \"order_item\" (a) values (?)"),
        "INSERT order_item"
    );
    assert_eq!(head_of("UPDATE PARTY SET X = ?"), "UPDATE PARTY");
    assert_eq!(head_of("SET TIME ZONE 'UTC'"), "SET TIME ZONE");
}
