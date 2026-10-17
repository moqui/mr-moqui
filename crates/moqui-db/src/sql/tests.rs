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

use std::path::Path;

use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use crate::dialect::{Dialect, Dialects};
use crate::model::EntityModel;

use super::{
    add_column, count, count_view, count_with, create_fk_index, create_foreign_key, create_index, create_table, delete,
    drop_fts_index, foreign_key_index_name, foreign_key_name, fts_index_name, fts_index_sql, fts_old_index_name,
    insert, insert_or_update, render_condition, select, select_view, select_view_with, select_with, update, CompareOp,
    Condition, FieldRef, LimitOffset, Nulls, OrderBy, PlainValues, SelectOptions, Statement,
};

fn workspace_root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn framework() -> (EntityModel, Dialects) {
    let root = workspace_root();
    let conf =
        MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).expect("conf");
    // Keep the pre-existing SQL snapshots as explicit legacy-mode coverage.
    conf.first_named("entity-facade")
        .unwrap()
        .set_attribute("sql-name-style", "legacy");
    let model =
        EntityModel::load_directory(&conf, &root.join("runtime/base-component/framework/entity")).expect("model");
    let dialects = Dialects::load(&conf).expect("dialects");
    (model, dialects)
}

fn values() -> PlainValues {
    PlainValues {
        now: Value::string("2026-10-13 12:00:00"),
    }
}

fn unquote(sql: &str) -> String {
    sql.replace(['"', '`'], "")
}

/// The statement binds one value per placeholder outside quoted text.
fn checked(statement: Statement) -> Statement {
    let mut quoted = false;
    let mut placeholders = 0;
    for ch in statement.sql.chars() {
        match ch {
            '\'' => quoted = !quoted,
            '?' if !quoted => placeholders += 1,
            _ => {}
        }
    }
    assert_eq!(
        placeholders,
        statement.params.len(),
        "placeholders and params differ: {} {:?}",
        statement.sql,
        statement.params
    );
    statement
}

#[test]
fn postgres_enumeration_insert_select_and_count() {
    let (model, dialects) = framework();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let postgres = dialects.get("postgres").unwrap();

    let inserted = insert(enumeration, postgres, &["enumId", "enumTypeId", "description"]).unwrap();
    assert_eq!(
        inserted,
        "INSERT INTO \"Enumeration\" (\"enumId\", \"enumTypeId\", \"description\") VALUES (?, ?, ?)"
    );

    let condition = Condition::equals("enumTypeId", Value::string("GEO"));
    let order = [OrderBy::asc("enumTypeId").nulls(Nulls::First)];
    let page = LimitOffset {
        limit: Some(20),
        offset: Some(0),
    };
    let selected = checked(select(enumeration, postgres, &[], Some(&condition), &order, page).unwrap());
    assert_eq!(
        selected.sql,
        "SELECT * FROM \"Enumeration\" WHERE \"enumTypeId\" = ? ORDER BY \"enumTypeId\" ASC NULLS FIRST LIMIT ? OFFSET ?"
    );
    assert_eq!(
        selected.params,
        vec![Value::string("GEO"), Value::I64(20), Value::I64(0)]
    );

    let counted = checked(count(enumeration, postgres, Some(&condition)).unwrap());
    assert_eq!(
        counted.sql,
        "SELECT COUNT(*) FROM \"Enumeration\" WHERE \"enumTypeId\" = ?"
    );
    assert_eq!(counted.params, vec![Value::string("GEO")]);

    let distinct = SelectOptions {
        distinct: true,
        fields: vec!["enumTypeId".to_string()],
        ..SelectOptions::default()
    };
    let counted = checked(count_with(enumeration, postgres, Some(&condition), &distinct).unwrap());
    assert_eq!(
        counted.sql,
        "SELECT COUNT(*) FROM (SELECT DISTINCT \"enumTypeId\" FROM \"Enumeration\" WHERE \"enumTypeId\" = ?) TEMP_NAME"
    );
    // With no field selected a distinct count is of whole rows, as Java's `makeCountFunction`.
    let whole_rows = SelectOptions {
        distinct: true,
        ..SelectOptions::default()
    };
    let counted = checked(count_with(enumeration, postgres, Some(&condition), &whole_rows).unwrap());
    assert_eq!(
        counted.sql,
        "SELECT COUNT(*) FROM (SELECT DISTINCT * FROM \"Enumeration\" WHERE \"enumTypeId\" = ?) TEMP_NAME"
    );
}

#[test]
fn order_by_writes_nulls_last_unless_never_nulls_and_parses_java_options() {
    let (model, dialects) = framework();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let postgres = dialects.get("postgres").unwrap();
    let mysql = dialects.get("mysql").unwrap();

    let parsed = OrderBy::parse("-^description").unwrap();
    assert!(parsed.descending);
    assert_eq!(parsed.case, Some(true));
    assert_eq!(parsed.field.name, "description");
    let parsed = OrderBy::parse(" sequenceNum DESC NULLS FIRST ").unwrap();
    assert!(parsed.descending);
    assert_eq!(parsed.nulls, Some(Nulls::First));
    assert_eq!(parsed.field.name, "sequenceNum");
    let parsed = OrderBy::parse("upper(enumCode) asc").unwrap();
    assert!(!parsed.descending);
    assert_eq!(parsed.case, Some(true));
    assert_eq!(parsed.field.name, "enumCode");
    assert!(OrderBy::parse("  ").is_none());
    assert!(OrderBy::parse("-").is_none());

    let order = [
        OrderBy::parse("-^description").unwrap(),
        OrderBy::parse("sequenceNum").unwrap(),
        OrderBy::parse("-sequenceNum").unwrap(),
    ];
    let sql = select(enumeration, postgres, &[], None, &order, LimitOffset::default())
        .unwrap()
        .sql;
    assert!(
        sql.ends_with(
            "ORDER BY UPPER(\"description\") DESC NULLS LAST, \"sequenceNum\" ASC NULLS LAST, \"sequenceNum\" DESC NULLS LAST"
        ),
        "{sql}"
    );
    // UPPER applies to text only; sequenceNum is a number.
    let order = [OrderBy::parse("^sequenceNum").unwrap()];
    let sql = select(enumeration, postgres, &[], None, &order, LimitOffset::default())
        .unwrap()
        .sql;
    assert!(sql.ends_with("ORDER BY \"sequenceNum\" ASC NULLS LAST"), "{sql}");

    let sql = select(
        enumeration,
        mysql,
        &[],
        None,
        &[OrderBy::desc("sequenceNum")],
        LimitOffset::default(),
    )
    .unwrap()
    .sql;
    assert!(sql.ends_with("ORDER BY `sequenceNum` DESC"), "{sql}");

    // Paging without an order sorts by the first column, as Java does.
    let page = LimitOffset {
        limit: Some(5),
        offset: None,
    };
    let sql = select(enumeration, postgres, &[], None, &[], page).unwrap().sql;
    assert!(sql.ends_with(" ORDER BY 1 LIMIT ? OFFSET ?"), "{sql}");
}

#[test]
fn null_values_render_is_null_as_java_does() {
    let (_, dialects) = framework();
    let postgres = dialects.get("postgres").unwrap();
    let render = |condition: Condition| checked(render_condition(&condition, postgres).unwrap());

    let equals = render(Condition::equals("thruDate", Value::Null));
    assert_eq!(equals.sql, "\"thruDate\" IS NULL");
    assert!(equals.params.is_empty());
    assert_eq!(
        render(Condition::not_equals("thruDate", Value::Null)).sql,
        "\"thruDate\" IS NOT NULL"
    );
    assert_eq!(render(Condition::like("name", Value::Null)).sql, "\"name\" IS NULL");
    assert_eq!(
        render(Condition::not_like("name", Value::Null)).sql,
        "\"name\" IS NOT NULL"
    );
    assert_eq!(
        render(Condition::between("amount", Value::Null, Value::Null)).sql,
        "\"amount\" IS NULL"
    );
    assert_eq!(
        render(Condition::not_between("amount", Value::I32(1), Value::I32(9))).sql,
        "\"amount\" NOT BETWEEN ? AND ?"
    );
    let ignore = render(Condition::ignore_case(Condition::equals("name", Value::Null)));
    assert_eq!(ignore.sql, "\"name\" IS NULL");
    // Less-than a null binds the null, as Java does.
    let less = render(Condition::less("amount", Value::Null));
    assert_eq!(less.sql, "\"amount\" < ?");
    assert_eq!(less.params, vec![Value::Null]);
    let bound = render(Condition::and(vec![
        Condition::bound("jobName", CompareOp::Equals),
        Condition::is_null("jobRunId"),
    ]));
    assert_eq!(bound.sql, "(\"jobName\" = ? AND \"jobRunId\" IS NULL)");
}

#[test]
fn ignore_case_upper_cases_text_fields_and_values_only() {
    let (model, dialects) = framework();
    let postgres = dialects.get("postgres").unwrap();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let condition = Condition::and(vec![
        Condition::ignore_case(Condition::equals("description", Value::string("Alpha"))),
        Condition::ignore_case(Condition::equals("sequenceNum", Value::string("5"))),
    ]);
    let sql = checked(
        select(
            enumeration,
            postgres,
            &[],
            Some(&condition),
            &[],
            LimitOffset::default(),
        )
        .unwrap(),
    );
    assert!(
        sql.sql
            .ends_with("WHERE (UPPER(\"description\") = UPPER(?) AND \"sequenceNum\" = ?)"),
        "{}",
        sql.sql
    );
    // The number field's text value binds as a number.
    assert_eq!(sql.params, vec![Value::string("ALPHA"), Value::I64(5)]);

    let fields = render_condition(
        &Condition::ignore_case(Condition::compare_fields(
            "a",
            CompareOp::Equals,
            FieldRef::aliased("O", "b"),
        )),
        postgres,
    )
    .unwrap();
    assert_eq!(fields.sql, "UPPER(\"a\") = UPPER(\"O\".\"b\")");
}

#[test]
fn insert_or_update_follows_never_try_insert() {
    let (model, dialects) = framework();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let postgres = dialects.get("postgres").unwrap();
    let mysql = dialects.get("mysql").unwrap();

    let err = insert_or_update(enumeration, postgres, &["enumId", "description"]).unwrap_err();
    assert!(err.to_string().contains("try-insert is off"), "{err}");

    let mysql_sql = insert_or_update(enumeration, mysql, &["enumId", "description"]).unwrap();
    assert!(mysql_sql.contains("ON DUPLICATE KEY UPDATE"), "{mysql_sql}");
    assert!(
        mysql_sql.contains("`description` = VALUES(`description`)"),
        "{mysql_sql}"
    );

    let mut postgres_try = postgres.clone();
    postgres_try.never_try_insert = false;
    let conflict = insert_or_update(enumeration, &postgres_try, &["enumId", "description"]).unwrap();
    assert!(conflict.contains("ON CONFLICT"), "{conflict}");
    assert!(conflict.contains("DO UPDATE SET"), "{conflict}");
    assert!(conflict.contains("EXCLUDED.\"description\""), "{conflict}");
}

#[test]
fn matches_uses_the_profile_fts_style() {
    let (model, dialects) = framework();
    let fts = model.entity("moqui.test.TestFts").unwrap();
    let postgres = dialects.get("postgres").unwrap();
    let mysql = dialects.get("mysql").unwrap();
    let condition = Condition::matches("body", Value::string("warehouse"));

    let pg = checked(select(fts, postgres, &[], Some(&condition), &[], LimitOffset::default()).unwrap());
    // The `text-fts` field searches its generated column, which has the GIN index.
    assert!(
        pg.sql.contains("\"body_tsv\" @@ plainto_tsquery('english', ?)"),
        "{}",
        pg.sql
    );
    assert_eq!(pg.params, vec![Value::string("warehouse")]);

    let my = select(fts, mysql, &[], Some(&condition), &[], LimitOffset::default()).unwrap();
    assert!(
        my.sql.contains("MATCH (`body`) AGAINST (? IN NATURAL LANGUAGE MODE)"),
        "{}",
        my.sql
    );

    let mut simple = postgres.clone();
    simple.fts_config = Some("simple".to_string());
    // With no entity to say the field is `text-fts`, the vector is computed.
    let rendered = render_condition(&condition, &simple).unwrap();
    assert!(
        rendered
            .sql
            .contains("to_tsvector('simple', \"body\") @@ plainto_tsquery('simple', ?)"),
        "{}",
        rendered.sql
    );
}

#[test]
fn mysql_limit_order_and_name_replace() {
    let (model, dialects) = framework();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let mysql = dialects.get("mysql").unwrap();
    let order = [OrderBy::desc("sequenceNum").nulls(Nulls::First)];
    let both = checked(
        select(
            enumeration,
            mysql,
            &[FieldRef::new("enumId")],
            Some(&Condition::equals("enumTypeId", Value::string("X"))),
            &order,
            LimitOffset {
                limit: Some(10),
                offset: Some(5),
            },
        )
        .unwrap(),
    );
    assert!(both.sql.contains("LIMIT ?, ?"), "{}", both.sql);
    assert!(!both.sql.contains("NULLS"), "{}", both.sql);
    assert_eq!(both.params, vec![Value::string("X"), Value::I64(5), Value::I64(10)]);

    let offset_only = checked(
        select(
            enumeration,
            mysql,
            &[],
            None,
            &[],
            LimitOffset {
                limit: None,
                offset: Some(5),
            },
        )
        .unwrap(),
    );
    assert_eq!(offset_only.params, vec![Value::I64(5), Value::I64(i64::MAX)]);

    let renamed = insert(enumeration, mysql, &["condition"]).unwrap();
    assert!(renamed.contains("`THE_CONDITION`"), "{renamed}");
    assert!(!renamed.contains("condition"), "{renamed}");
}

#[test]
fn empty_conditions_are_omitted_and_empty_in_is_false() {
    let (model, dialects) = framework();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let postgres = dialects.get("postgres").unwrap();
    let page = LimitOffset::default();

    let omitted = select(enumeration, postgres, &[], Some(&Condition::and(vec![])), &[], page).unwrap();
    assert!(!omitted.sql.contains("WHERE"), "{}", omitted.sql);
    // An `or` of nothing is false, as `any` of nothing is.
    let or_none = checked(select(enumeration, postgres, &[], Some(&Condition::or(vec![])), &[], page).unwrap());
    assert!(or_none.sql.contains("WHERE 1 = 0"), "{}", or_none.sql);

    let empty_in = checked(
        select(
            enumeration,
            postgres,
            &[],
            Some(&Condition::is_in("enumId", vec![])),
            &[],
            page,
        )
        .unwrap(),
    );
    assert!(empty_in.sql.contains("WHERE 1 = 0"), "{}", empty_in.sql);

    let empty_not_in = render_condition(&Condition::not_in("enumId", vec![]), postgres).unwrap();
    assert_eq!(empty_not_in.sql, "1 = 1");

    let nested = checked(
        render_condition(
            &Condition::or(vec![
                Condition::and(vec![]),
                Condition::negate(Condition::equals("enumId", Value::string("A"))),
            ]),
            postgres,
        )
        .unwrap(),
    );
    assert_eq!(nested.sql, "NOT (\"enumId\" = ?)");
    assert_eq!(nested.params, vec![Value::string("A")]);

    // Negating nothing does not leave nothing: not true is false, and not false is true.
    let not_and = render_condition(&Condition::negate(Condition::and(vec![])), postgres).unwrap();
    assert_eq!(not_and.sql, "1 = 0");
    let not_or = render_condition(&Condition::negate(Condition::or(vec![])), postgres).unwrap();
    assert_eq!(not_or.sql, "1 = 1");
    let not_in_none = render_condition(&Condition::negate(Condition::is_in("enumId", vec![])), postgres).unwrap();
    assert_eq!(not_in_none.sql, "1 = 1");
    let or_kept = checked(
        render_condition(
            &Condition::or(vec![
                Condition::or(vec![]),
                Condition::equals("enumId", Value::string("B")),
            ]),
            postgres,
        )
        .unwrap(),
    );
    assert_eq!(or_kept.sql, "(1 = 0 OR \"enumId\" = ?)");
    let select_none = checked(
        select(
            enumeration,
            postgres,
            &[],
            Some(&Condition::negate(Condition::and(vec![]))),
            &[],
            page,
        )
        .unwrap(),
    );
    assert!(select_none.sql.contains("WHERE 1 = 0"), "{}", select_none.sql);
}

#[test]
fn update_and_delete_bind_the_condition_after_the_set_values() {
    let (model, dialects) = framework();
    let postgres = dialects.get("postgres").unwrap();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let updated = update(
        enumeration,
        postgres,
        &["description"],
        Some(&Condition::equals("enumId", Value::string("A"))),
    )
    .unwrap();
    assert_eq!(
        updated.sql,
        "UPDATE \"Enumeration\" SET \"description\" = ? WHERE \"enumId\" = ?"
    );
    assert_eq!(updated.params, vec![Value::string("A")]);
    let deleted = checked(delete(enumeration, postgres, Some(&Condition::is_null("description"))).unwrap());
    assert_eq!(deleted.sql, "DELETE FROM \"Enumeration\" WHERE \"description\" IS NULL");
}

#[test]
fn column_name_is_written_and_read_back_under_the_field_name() {
    let (model, dialects) = framework();
    let postgres = dialects.get("postgres").unwrap();
    let entity = model
        .entities()
        .iter()
        .find(|entity| entity.fields.iter().any(|field| field.column_name != field.name))
        .expect("an entity with column-name");
    let field = entity
        .fields
        .iter()
        .find(|field| field.column_name != field.name)
        .unwrap();
    assert_eq!(field.column_name, "BAR_SCORE");
    let selected = select(
        entity,
        postgres,
        &[],
        Some(&Condition::greater(field.name.as_str(), Value::I32(1))),
        &[],
        LimitOffset::default(),
    )
    .unwrap();
    assert!(selected.sql.contains("\"BAR_SCORE\" AS \"score\""), "{}", selected.sql);
    assert!(selected.sql.contains("WHERE \"BAR_SCORE\" > ?"), "{}", selected.sql);
    let inserted = insert(entity, postgres, &["score"]).unwrap();
    assert!(inserted.contains("(\"BAR_SCORE\")"), "{inserted}");
    let table = create_table(entity, &model, postgres).unwrap();
    assert!(table.contains("\"BAR_SCORE\" NUMERIC"), "{table}");
}

#[test]
fn fts_ddl_is_javas_generated_column_and_named_index() {
    let (model, dialects) = framework();
    let mut fts = model.entity("moqui.test.TestFts").unwrap().clone();
    let body = fts.fields.iter_mut().find(|field| field.name == "body").unwrap();
    body.column_name = "BODY_TEXT".to_string();
    let body = fts.fields.iter().find(|field| field.name == "body").unwrap();
    let postgres = dialects.get("postgres").unwrap();
    let mysql = dialects.get("mysql").unwrap();

    let pg = fts_index_sql(&fts, body, postgres).unwrap();
    assert_eq!(
        pg,
        vec![
            "ALTER TABLE \"TestFts\" ADD COLUMN IF NOT EXISTS \"BODY_TEXT_tsv\" tsvector GENERATED ALWAYS AS \
             (to_tsvector('english', coalesce(\"BODY_TEXT\", ''))) STORED"
                .to_string(),
            "CREATE INDEX IF NOT EXISTS \"fts_TestFts_BODY_TEXT\" ON \"TestFts\" USING GIN (\"BODY_TEXT_tsv\")"
                .to_string(),
        ]
    );
    let my = fts_index_sql(&fts, body, mysql).unwrap();
    assert_eq!(
        my,
        vec!["ALTER TABLE `TestFts` ADD FULLTEXT INDEX `TestFts_BODY_TEXT_FTS` (`BODY_TEXT`)".to_string()]
    );
    let table = create_table(&fts, &model, postgres).unwrap();
    assert!(table.ends_with(&pg.join(";\n")), "{table}");
    assert!(fts_index_sql(&fts, body, dialects.get("h2").unwrap())
        .unwrap()
        .is_empty());

    // Java's name: characters other than letters, digits and `_` become `_`, clipped to 60.
    let mut long = fts.clone();
    long.table_name = format!("A-{}", "B".repeat(70));
    assert_eq!(
        fts_index_name(&long, body, postgres),
        format!("fts_A_{}", "B".repeat(54))
    );

    // The index this port made before, which a check now drops.
    assert_eq!(fts_old_index_name(&fts, body, postgres), "TestFts_body_FTS");
    assert_eq!(
        drop_fts_index(&fts, "TestFts_body_FTS", postgres),
        "DROP INDEX IF EXISTS \"TestFts_body_FTS\""
    );
    assert_eq!(
        drop_fts_index(&fts, "TestFts_body_FTS", mysql),
        "ALTER TABLE `TestFts` DROP INDEX `TestFts_body_FTS`"
    );
}

#[test]
fn fts_ddl_false_leaves_the_full_text_column_and_index_out() {
    let (model, dialects) = framework();
    let fts = model.entity("moqui.test.TestFts").unwrap();
    let body = fts.field("body").unwrap();
    for name in ["postgres", "mysql"] {
        let mut dialect = dialects.get(name).unwrap().clone();
        dialect.fts_ddl = false;
        assert!(fts_index_sql(fts, body, &dialect).unwrap().is_empty(), "{name}");
        let table = create_table(fts, &model, &dialect).unwrap();
        assert!(!table.contains("tsv") && !table.contains("FULLTEXT"), "{table}");
    }
}

#[test]
fn a_text_fts_entity_selects_its_fields_not_the_generated_column() {
    let (model, dialects) = framework();
    let fts = model.entity("moqui.test.TestFts").unwrap();
    let sql = select(
        fts,
        dialects.get("postgres").unwrap(),
        &[],
        None,
        &[],
        LimitOffset::default(),
    )
    .unwrap();
    assert!(sql.sql.starts_with("SELECT \"testFtsId\", "), "{}", sql.sql);
    assert!(!sql.sql.contains('*'), "{}", sql.sql);
}

#[test]
fn fk_initially_deferred_and_constraint_keyword_follow_the_database() {
    let (model, dialects) = framework();
    let mut postgres = dialects.get("postgres").unwrap().clone();
    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let relationship = enumeration.relationship("type").unwrap();
    let related = model.entity(&relationship.related).unwrap();
    let plain = create_foreign_key(enumeration, relationship, related, &postgres).unwrap();
    assert!(!plain.contains("DEFERRED"), "{plain}");
    postgres.use_fk_initially_deferred = true;
    let deferred = create_foreign_key(enumeration, relationship, related, &postgres).unwrap();
    assert_eq!(deferred, format!("{plain} INITIALLY DEFERRED"));

    let fts = model.entity("moqui.test.TestFts").unwrap();
    assert!(!create_table(fts, &model, &postgres)
        .unwrap()
        .contains("CONSTRAINT NOT NULL"));
    postgres.always_use_constraint_keyword = true;
    let table = create_table(fts, &model, &postgres).unwrap();
    assert!(
        table.contains("\"testFtsId\" VARCHAR(40) CONSTRAINT NOT NULL"),
        "{table}"
    );
}

#[test]
fn ddl_table_index_column_and_foreign_key() {
    let (model, dialects) = framework();
    let postgres = dialects.get("postgres").unwrap();
    let mysql = dialects.get("mysql").unwrap();

    let fts = model.entity("moqui.test.TestFts").unwrap();
    let pg_table = create_table(fts, &model, postgres).unwrap();
    assert!(pg_table.contains("\"body\" TEXT"), "{pg_table}");
    assert!(pg_table.contains("USING GIN (\"body_tsv\")"), "{pg_table}");
    assert!(pg_table.contains("PRIMARY KEY"), "{pg_table}");

    let my_table = create_table(fts, &model, mysql).unwrap();
    assert!(my_table.contains("FULLTEXT"), "{my_table}");
    assert!(my_table.contains("ENGINE=InnoDB"), "{my_table}");
    assert!(my_table.contains("`body` LONGTEXT"), "{my_table}");

    let factor = model.entity("moqui.security.UserAuthcFactor").unwrap();
    let factor_sql = create_table(factor, &model, postgres).unwrap();
    assert!(factor_sql.contains("\"userId\" VARCHAR(40) NOT NULL"), "{factor_sql}");
    assert!(factor_sql.contains("\"factorId\" VARCHAR(40) NOT NULL"), "{factor_sql}");

    let body = fts.field("body").unwrap();
    let added = add_column(fts, body, &model, postgres).unwrap();
    assert_eq!(added, "ALTER TABLE \"TestFts\" ADD \"body\" TEXT");

    let account = model.entity("moqui.security.UserAccount").unwrap();
    let index = account
        .indexes
        .iter()
        .find(|index| index.name == "USERACCT_USERNAME")
        .unwrap();
    let index_sql = create_index(account, index, postgres).unwrap();
    assert_eq!(
        index_sql,
        "CREATE UNIQUE INDEX \"USERACCT_USERNAME\" ON \"UserAccount\" (\"username\")"
    );

    let enumeration = model.entity("moqui.basic.Enumeration").unwrap();
    let relationship = enumeration.relationship("type").unwrap();
    let related = model.entity(&relationship.related).unwrap();
    let fk = create_foreign_key(enumeration, relationship, related, postgres).unwrap();
    assert_eq!(
        fk,
        "ALTER TABLE \"Enumeration\" ADD CONSTRAINT \"EnumerationEnumerationType\" FOREIGN KEY (\"enumTypeId\") REFERENCES \"EnumerationType\" (\"enumTypeId\")"
    );
    let fk_index = create_fk_index(enumeration, relationship, postgres).unwrap();
    assert_eq!(
        fk_index,
        "CREATE INDEX \"IDXEnumerationEnumerationType\" ON \"Enumeration\" (\"enumTypeId\")"
    );

    let parent = enumeration.relationship("parent").unwrap();
    let err = create_foreign_key(enumeration, parent, enumeration, postgres).unwrap_err();
    assert!(err.to_string().contains("type=one"), "{err}");
}

#[test]
fn foreign_key_names_follow_java() {
    let (model, dialects) = framework();
    let mut postgres = dialects.get("postgres").unwrap().clone();
    let conversion = model.entity("moqui.basic.UomConversion").unwrap();
    let to_uom = conversion.relationship("To#moqui.basic.Uom").unwrap();
    assert_eq!(foreign_key_name(conversion, to_uom, &postgres), "UomConversionToUom");
    // The related name ends the way the entity name does, so that end is reduced to capitals.
    assert_eq!(
        foreign_key_index_name(conversion, to_uom, &postgres),
        "IDXUomConversionToUom"
    );
    let flow = model.entity("moqui.basic.StatusFlowTransition").unwrap();
    let from_status = flow.relationship("To#moqui.basic.StatusItem").unwrap();
    postgres.constraint_name_clip_length = Some(20);
    // StatusFlowTransitionToStatusItem loses vowels from the end until it fits.
    assert_eq!(foreign_key_name(flow, from_status, &postgres), "SttsFlwTrnstnTSttstm");
    let index_name = foreign_key_index_name(flow, from_status, &postgres);
    assert!(index_name.starts_with("IDX") && index_name.len() <= 20, "{index_name}");
}

#[test]
fn framework_views_join_with_java_keywords() {
    let (model, dialects) = framework();
    let postgres = dialects.get("postgres").unwrap();
    let page = LimitOffset::default();
    let values = values();

    let parent = model.entity("moqui.basic.ParentEnum").unwrap();
    let joined = select_view(parent, &model, postgres, &values, None, &[], page)
        .unwrap()
        .sql;
    assert!(joined.contains(" INNER JOIN "), "{joined}");
    assert!(
        unquote(&joined).contains("ENUM.parentEnumId = ENUMP.enumId"),
        "{joined}"
    );

    let optional = model.entity("moqui.basic.EnumAndParent").unwrap();
    let left = select_view(optional, &model, postgres, &values, None, &[], page)
        .unwrap()
        .sql;
    assert!(left.contains(" LEFT OUTER JOIN "), "{left}");

    let only_rel = model.entity("moqui.basic.EnumerationAndType").unwrap();
    let rendered = select_view(only_rel, &model, postgres, &values, None, &[], page)
        .unwrap()
        .sql;
    assert!(rendered.contains("(\"Enumeration\" \"ENUM\" INNER JOIN"), "{rendered}");
    let plain = unquote(&rendered);
    assert!(plain.contains("ENUM.enumTypeId = ETP.enumTypeId"), "{rendered}");

    let mysql = dialects.get("mysql").unwrap();
    let rendered = select_view(only_rel, &model, mysql, &values, None, &[], page)
        .unwrap()
        .sql;
    assert!(!rendered.contains('('), "ansi-no-parenthesis: {rendered}");

    // Every alias-all skips lastUpdatedStamp, as Java does.
    for view in model.entities().iter().filter(|entity| entity.is_view) {
        for alias in view.aliases.iter().filter(|alias| alias.from_alias_all) {
            assert_ne!(alias.field_name(), "lastUpdatedStamp", "{}", view.full_name);
        }
    }
}

#[test]
fn every_framework_view_renders_select_and_count() {
    let (model, dialects) = framework();
    let values = values();
    for name in ["postgres", "mysql8"] {
        let dialect = dialects.get(name).unwrap();
        for view in model.entities().iter().filter(|entity| entity.is_view) {
            let selected = select_view(view, &model, dialect, &values, None, &[], LimitOffset::default())
                .unwrap_or_else(|err| panic!("{} on {name}: {err}", view.full_name));
            checked(selected);
            let counted = count_view(view, &model, dialect, &values, None, &SelectOptions::default())
                .unwrap_or_else(|err| panic!("{} count on {name}: {err}", view.full_name));
            checked(counted);
        }
    }
}

fn sample_postgres() -> Dialect {
    let (_, dialects) = framework();
    dialects.get("postgres").unwrap().clone()
}

fn sample_model(entities: &str) -> EntityModel {
    let root = workspace_root();
    let conf =
        MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).expect("conf");
    conf.first_named("entity-facade")
        .unwrap()
        .set_attribute("sql-name-style", "legacy");
    let doc = MNode::parse_text("entities", &format!("<entities>{entities}</entities>")).unwrap();
    EntityModel::load(&conf, &[doc]).unwrap()
}

const ORDERS: &str = r#"
    <entity entity-name="Order" package="sample">
        <field name="orderId" type="id" is-pk="true"/>
        <field name="customerId" type="id"/>
        <field name="statusId" type="id"/>
    </entity>
    <entity entity-name="OrderItem" package="sample">
        <field name="orderId" type="id" is-pk="true"/>
        <field name="itemSeqId" type="id" is-pk="true"/>
        <field name="quantity" type="number-decimal"/>
        <field name="unitAmount" type="number-decimal"/>
        <field name="productId" type="id"/>
        <field name="fromDate" type="date-time"/>
        <field name="thruDate" type="date-time"/>
        <relationship type="one" related="sample.Order"/>
    </entity>
    <entity entity-name="Product" package="sample">
        <field name="productId" type="id" is-pk="true"/>
        <field name="productName" type="text-medium"/>
    </entity>
    <entity entity-name="ItemBilling" package="sample">
        <field name="billingId" type="id" is-pk="true"/>
        <field name="orderId" type="id"/>
        <field name="itemSeqId" type="id"/>
        <field name="amount" type="number-decimal"/>
        <field name="billingDate" type="date-time"/>
    </entity>
"#;

fn totals_model() -> EntityModel {
    sample_model(&format!(
        r#"{ORDERS}
        <view-entity entity-name="OrderTotal" package="sample">
            <member-entity entity-alias="O" entity-name="sample.Order"/>
            <member-entity entity-alias="OI" entity-name="sample.OrderItem" join-from-alias="O">
                <key-map field-name="orderId"/>
            </member-entity>
            <member-entity entity-alias="PR" entity-name="sample.Product" join-from-alias="OI" join-optional="true">
                <key-map field-name="productId"/>
            </member-entity>
            <alias name="orderId" entity-alias="O"/>
            <alias name="customerId" entity-alias="O"/>
            <alias name="productName" entity-alias="PR"/>
            <alias name="totalQuantity" entity-alias="OI" field="quantity" function="sum"/>
            <alias name="itemCount" entity-alias="OI" field="itemSeqId" function="count"/>
            <alias name="productCount" entity-alias="OI" field="productId" function="count-distinct"/>
            <alias name="upperCustomer" entity-alias="O" field="customerId" function="upper"/>
        </view-entity>
        <view-entity entity-name="OrderAndItem" package="sample">
            <member-entity entity-alias="O" entity-name="sample.Order"/>
            <member-relationship entity-alias="OI" join-from-alias="O" relationship="sample.OrderItem"/>
            <alias-all entity-alias="O"/>
            <alias name="quantity" entity-alias="OI"/>
            <alias name="itemFromDate" entity-alias="OI" field="fromDate"/>
            <alias name="itemThruDate" entity-alias="OI" field="thruDate"/>
        </view-entity>"#
    ))
}

#[test]
fn aggregate_aliases_group_the_other_columns_and_having_names_the_aggregate() {
    let model = totals_model();
    let postgres = sample_postgres();
    let view = model.entity("sample.OrderTotal").unwrap();
    let options = SelectOptions {
        having: Some(Condition::greater("totalQuantity", Value::I32(5))),
        ..SelectOptions::default()
    };
    let sql = checked(
        select_view_with(
            view,
            &model,
            &postgres,
            &values(),
            Some(&Condition::equals("customerId", Value::string("C1"))),
            &[OrderBy::desc("totalQuantity")],
            LimitOffset::default(),
            &options,
        )
        .unwrap(),
    );
    assert_eq!(sql.params, vec![Value::string("C1"), Value::I32(5)]);
    let plain = unquote(&sql.sql);
    assert!(plain.contains("SUM(OI.quantity) AS totalQuantity"), "{plain}");
    assert!(plain.contains("COUNT(OI.itemSeqId) AS itemCount"), "{plain}");
    assert!(
        plain.contains("COUNT(DISTINCT OI.productId) AS productCount"),
        "{plain}"
    );
    assert!(plain.contains("UPPER(O.customerId) AS upperCustomer"), "{plain}");
    assert!(
        plain.contains("GROUP BY O.orderId, O.customerId, PR.productName, UPPER(O.customerId)"),
        "{plain}"
    );
    assert!(plain.contains("WHERE O.customerId = ?"), "{plain}");
    assert!(plain.contains("HAVING SUM(OI.quantity) > ?"), "{plain}");
    assert!(plain.contains("ORDER BY SUM(OI.quantity) DESC NULLS LAST"), "{plain}");
}

#[test]
fn a_field_subset_groups_by_the_selected_fields_and_joins_only_used_members() {
    let model = totals_model();
    let postgres = sample_postgres();
    let view = model.entity("sample.OrderTotal").unwrap();
    let options = SelectOptions {
        fields: vec!["customerId".to_string(), "totalQuantity".to_string()],
        ..SelectOptions::default()
    };
    let sql = select_view_with(
        view,
        &model,
        &postgres,
        &values(),
        None,
        &[],
        LimitOffset::default(),
        &options,
    )
    .unwrap()
    .sql;
    let plain = unquote(&sql);
    assert_eq!(
        plain,
        "SELECT O.customerId, SUM(OI.quantity) AS totalQuantity FROM (Order O INNER JOIN OrderItem OI ON O.orderId = OI.orderId) GROUP BY O.customerId"
    );

    // Only the first member is read, so nothing is joined.
    let options = SelectOptions {
        fields: vec!["customerId".to_string()],
        ..SelectOptions::default()
    };
    let sql = select_view_with(
        view,
        &model,
        &postgres,
        &values(),
        None,
        &[],
        LimitOffset::default(),
        &options,
    )
    .unwrap()
    .sql;
    assert_eq!(unquote(&sql), "SELECT O.customerId FROM Order O GROUP BY O.customerId");

    // A condition on a member joins it, and the join path back to the first member.
    let sql = select_view_with(
        view,
        &model,
        &postgres,
        &values(),
        Some(&Condition::equals("productName", Value::string("Widget"))),
        &[],
        LimitOffset::default(),
        &options,
    )
    .unwrap()
    .sql;
    assert!(
        unquote(&sql).contains(
            "FROM (Order O INNER JOIN OrderItem OI ON O.orderId = OI.orderId LEFT OUTER JOIN Product PR ON OI.productId = PR.productId)"
        ),
        "{sql}"
    );

    let unknown = SelectOptions {
        fields: vec!["nope".to_string()],
        ..SelectOptions::default()
    };
    assert!(select_view_with(
        view,
        &model,
        &postgres,
        &values(),
        None,
        &[],
        LimitOffset::default(),
        &unknown
    )
    .is_err());
}

#[test]
fn view_count_wraps_a_grouped_or_distinct_select() {
    let model = totals_model();
    let postgres = sample_postgres();
    let totals = model.entity("sample.OrderTotal").unwrap();
    let counted = checked(
        count_view(
            totals,
            &model,
            &postgres,
            &values(),
            Some(&Condition::equals("customerId", Value::string("C1"))),
            &SelectOptions {
                fields: vec!["customerId".to_string(), "totalQuantity".to_string()],
                ..SelectOptions::default()
            },
        )
        .unwrap(),
    );
    assert_eq!(
        unquote(&counted.sql),
        "SELECT COUNT(*) FROM (SELECT O.customerId, SUM(OI.quantity) AS totalQuantity FROM (Order O INNER JOIN OrderItem OI ON O.orderId = OI.orderId) WHERE O.customerId = ? GROUP BY O.customerId) TEMP_NAME"
    );

    let plain_view = model.entity("sample.OrderAndItem").unwrap();
    let counted = count_view(
        plain_view,
        &model,
        &postgres,
        &values(),
        None,
        &SelectOptions::default(),
    )
    .unwrap();
    assert_eq!(
        unquote(&counted.sql),
        "SELECT COUNT(*) FROM (Order O INNER JOIN OrderItem OI ON O.orderId = OI.orderId)"
    );
    let distinct = count_view(
        plain_view,
        &model,
        &postgres,
        &values(),
        None,
        &SelectOptions {
            distinct: true,
            fields: vec!["customerId".to_string()],
            ..SelectOptions::default()
        },
    )
    .unwrap();
    assert_eq!(
        unquote(&distinct.sql),
        "SELECT COUNT(*) FROM (SELECT DISTINCT O.customerId FROM Order O) TEMP_NAME"
    );
}

#[test]
fn member_relationship_and_alias_all_expand_at_load() {
    let model = totals_model();
    let view = model.entity("sample.OrderAndItem").unwrap();
    let member = view.member("OI").unwrap();
    assert_eq!(member.entity_name, "sample.OrderItem");
    let names: Vec<&str> = view.aliases.iter().map(|alias| alias.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "quantity",
            "itemFromDate",
            "itemThruDate",
            "orderId",
            "customerId",
            "statusId"
        ]
    );
    let order_id = view.alias("orderId").unwrap();
    assert!(order_id.from_alias_all);
    assert!(order_id.is_pk);
    assert_eq!(order_id.java_type, "java.lang.String");
    assert_eq!(view.alias("quantity").unwrap().java_type, "java.math.BigDecimal");
}

#[test]
fn an_aggregate_in_where_or_a_having_without_one_is_an_error() {
    let model = totals_model();
    let postgres = sample_postgres();
    let totals = model.entity("sample.OrderTotal").unwrap();
    let err = select_view(
        totals,
        &model,
        &postgres,
        &values(),
        Some(&Condition::greater("totalQuantity", Value::I32(5))),
        &[],
        LimitOffset::default(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("having"), "{err}");
    let plain = model.entity("sample.OrderAndItem").unwrap();
    let options = SelectOptions {
        having: Some(Condition::greater("quantity", Value::I32(1))),
        ..SelectOptions::default()
    };
    let err = select_view_with(
        plain,
        &model,
        &postgres,
        &values(),
        None,
        &[],
        LimitOffset::default(),
        &options,
    )
    .unwrap_err();
    assert!(err.to_string().contains("no aggregate"), "{err}");
}

#[test]
fn a_distinct_select_also_selects_its_order_by_fields() {
    let model = totals_model();
    let postgres = sample_postgres();
    let order = model.entity("sample.Order").unwrap();
    let distinct = SelectOptions {
        distinct: true,
        ..SelectOptions::default()
    };
    let sql = select_with(
        order,
        &postgres,
        &[FieldRef::new("customerId")],
        None,
        &[OrderBy::asc("orderId"), OrderBy::asc("customerId")],
        LimitOffset::default(),
        &distinct,
    )
    .unwrap();
    assert!(
        sql.sql.starts_with("SELECT DISTINCT \"customerId\", \"orderId\" FROM"),
        "{}",
        sql.sql
    );
}

#[test]
fn distinct_and_for_update_are_written_and_cannot_be_combined() {
    let model = totals_model();
    let postgres = sample_postgres();
    let order = model.entity("sample.Order").unwrap();
    let distinct = SelectOptions {
        distinct: true,
        ..SelectOptions::default()
    };
    let sql = select_with(
        order,
        &postgres,
        &[FieldRef::new("customerId")],
        None,
        &[],
        LimitOffset::default(),
        &distinct,
    )
    .unwrap();
    assert!(
        sql.sql.starts_with("SELECT DISTINCT \"customerId\" FROM"),
        "{}",
        sql.sql
    );
    let locking = SelectOptions {
        for_update: true,
        ..SelectOptions::default()
    };
    let sql = select_with(
        order,
        &postgres,
        &[],
        Some(&Condition::equals("orderId", Value::string("1"))),
        &[],
        LimitOffset {
            limit: Some(1),
            offset: None,
        },
        &locking,
    )
    .unwrap();
    assert!(sql.sql.ends_with(" LIMIT ? OFFSET ? FOR UPDATE"), "{}", sql.sql);
    // An empty for-update (SQLite) writes no lock text.
    let unlocked = Dialect {
        for_update: Some(String::new()),
        ..postgres.clone()
    };
    let sql = select_with(
        order,
        &unlocked,
        &[],
        Some(&Condition::equals("orderId", Value::string("1"))),
        &[],
        LimitOffset::default(),
        &locking,
    )
    .unwrap();
    assert!(
        !sql.sql.contains("FOR UPDATE") && !sql.sql.ends_with(' '),
        "{}",
        sql.sql
    );
    let both = SelectOptions {
        distinct: true,
        for_update: true,
        ..SelectOptions::default()
    };
    assert!(select_with(order, &postgres, &[], None, &[], LimitOffset::default(), &both).is_err());
    let view = model.entity("sample.OrderAndItem").unwrap();
    let sql = select_view_with(
        view,
        &model,
        &postgres,
        &values(),
        None,
        &[],
        LimitOffset::default(),
        &locking,
    )
    .unwrap();
    assert!(sql.sql.ends_with(" FOR UPDATE"), "{}", sql.sql);
    assert!(!sql.sql.contains("GROUP BY"), "{}", sql.sql);
    let totals = model.entity("sample.OrderTotal").unwrap();
    assert!(select_view_with(
        totals,
        &model,
        &postgres,
        &values(),
        None,
        &[],
        LimitOffset::default(),
        &locking
    )
    .is_err());
}

#[test]
fn view_entity_condition_adds_where_having_order_and_distinct() {
    let model = sample_model(&format!(
        r#"{ORDERS}
        <view-entity entity-name="OpenItemTotal" package="sample">
            <member-entity entity-alias="O" entity-name="sample.Order"/>
            <member-entity entity-alias="OI" entity-name="sample.OrderItem" join-from-alias="O">
                <key-map field-name="orderId"/>
                <entity-condition><econdition field-name="productId" operator="not-equals" value="SKIP"/></entity-condition>
            </member-entity>
            <alias name="customerId" entity-alias="O"/>
            <alias name="statusId" entity-alias="O"/>
            <alias name="totalQuantity" entity-alias="OI" field="quantity" function="sum"/>
            <entity-condition distinct="true">
                <date-filter entity-alias="OI"/>
                <econdition field-name="statusId" operator="in" value="OPEN,HELD"/>
                <econditions combine="or">
                    <econdition field-name="customerId" value="C1"/>
                    <econdition field-name="customerId" operator="is-null" or-null="true"/>
                </econditions>
                <having-econditions>
                    <econdition field-name="totalQuantity" operator="greater" value="0"/>
                </having-econditions>
                <order-by field-name="-totalQuantity"/>
            </entity-condition>
        </view-entity>"#
    ));
    let postgres = sample_postgres();
    let view = model.entity("sample.OpenItemTotal").unwrap();
    let sql = checked(
        select_view(
            view,
            &model,
            &postgres,
            &values(),
            None,
            &[OrderBy::asc("customerId")],
            LimitOffset::default(),
        )
        .unwrap(),
    );
    let plain = unquote(&sql.sql);
    assert!(plain.starts_with("SELECT DISTINCT O.customerId"), "{plain}");
    assert!(
        plain.contains("INNER JOIN OrderItem OI ON O.orderId = OI.orderId AND OI.productId <> ?"),
        "{plain}"
    );
    assert!(
        plain.contains(
            "WHERE (((OI.fromDate IS NULL OR OI.fromDate <= ?) AND (OI.thruDate IS NULL OR OI.thruDate > ?)) AND O.statusId IN (?, ?) AND (O.customerId = ? OR (O.customerId IS NULL OR O.customerId IS NULL)))"
        ),
        "{plain}"
    );
    assert!(plain.contains("HAVING SUM(OI.quantity) > ?"), "{plain}");
    assert!(
        plain.ends_with("ORDER BY O.customerId ASC NULLS LAST, SUM(OI.quantity) DESC NULLS LAST"),
        "{plain}"
    );
    assert_eq!(sql.params[0], Value::string("SKIP"));
    assert_eq!(sql.params[3], Value::string("OPEN"));
    assert_eq!(sql.params[4], Value::string("HELD"));
    assert!(matches!(sql.params.last(), Some(Value::Decimal(_))), "{:?}", sql.params);
}

#[test]
fn complex_alias_case_and_java_column_references() {
    let model = sample_model(&format!(
        r#"{ORDERS}
        <view-entity entity-name="ItemValue" package="sample">
            <member-entity entity-alias="OI" entity-name="sample.OrderItem"/>
            <member-entity entity-alias="PR" entity-name="sample.Product" join-from-alias="OI" join-optional="true">
                <key-map field-name="productId"/>
            </member-entity>
            <alias name="orderId" entity-alias="OI"/>
            <alias name="lineTotal" type="number-decimal"><complex-alias operator="*">
                <complex-alias-field entity-alias="OI" field="quantity" default-value="0"/>
                <complex-alias-field entity-alias="OI" field="unitAmount"/>
            </complex-alias></alias>
            <alias name="orderTotal" function="sum" type="number-decimal"><complex-alias operator="*">
                <complex-alias-field entity-alias="OI" field="quantity"/>
                <complex-alias-field entity-alias="OI" field="unitAmount"/>
            </complex-alias></alias>
            <alias name="label" type="text-medium"><complex-alias function="concat_ws">
                <complex-alias expression="' '"/>
                <complex-alias-field entity-alias="PR" field="productName"/>
                <complex-alias-field entity-alias="OI" field="productId"/>
            </complex-alias></alias>
            <alias name="size" type="text-short"><case>
                <when expression="OI.QUANTITY &gt; 10"><complex-alias expression="'big'"/></when>
                <else><complex-alias expression="'small'"/></else>
            </case></alias>
        </view-entity>"#
    ));
    let postgres = sample_postgres();
    let view = model.entity("sample.ItemValue").unwrap();
    let sql = select_view(view, &model, &postgres, &values(), None, &[], LimitOffset::default())
        .unwrap()
        .sql;
    assert!(
        sql.contains("(COALESCE(\"OI\".\"quantity\",0) * \"OI\".\"unitAmount\") AS \"lineTotal\""),
        "{sql}"
    );
    assert!(
        sql.contains("SUM(\"OI\".\"quantity\" * \"OI\".\"unitAmount\") AS \"orderTotal\""),
        "{sql}"
    );
    assert!(
        sql.contains("concat_ws(' ', \"PR\".\"productName\", \"OI\".\"productId\") AS \"label\""),
        "{sql}"
    );
    assert!(
        sql.contains("CASE WHEN \"OI\".\"quantity\" > 10 THEN 'big' ELSE 'small' END AS \"size\""),
        "{sql}"
    );
    assert!(sql.contains("GROUP BY \"OI\".\"orderId\", (COALESCE("), "{sql}");
}

#[test]
fn a_complex_alias_operator_outside_the_allow_list_is_an_error() {
    let view = |operator: &str| {
        sample_model(&format!(
            r#"{ORDERS}
            <view-entity entity-name="ItemValue" package="sample">
                <member-entity entity-alias="OI" entity-name="sample.OrderItem"/>
                <alias name="orderId" entity-alias="OI"/>
                <alias name="combined" type="text-medium"><complex-alias operator="{operator}">
                    <complex-alias-field entity-alias="OI" field="quantity"/>
                    <complex-alias-field entity-alias="OI" field="unitAmount"/>
                </complex-alias></alias>
            </view-entity>"#
        ))
    };
    let postgres = sample_postgres();
    for operator in ["-", "/", "||", ","] {
        let model = view(operator);
        let entity = model.entity("sample.ItemValue").unwrap();
        let sql = select_view(entity, &model, &postgres, &values(), None, &[], LimitOffset::default()).unwrap();
        assert!(sql.sql.contains(&format!(" {operator} ")), "{}", sql.sql);
    }
    let model = view("; DROP TABLE x; --");
    let entity = model.entity("sample.ItemValue").unwrap();
    let err = select_view(entity, &model, &postgres, &values(), None, &[], LimitOffset::default()).unwrap_err();
    assert!(err.to_string().contains("is not supported"), "{err}");
}

#[test]
fn a_view_member_is_a_sub_select_of_that_view() {
    let model = sample_model(&format!(
        r#"{ORDERS}
        <view-entity entity-name="ItemSum" package="sample">
            <member-entity entity-alias="OI" entity-name="sample.OrderItem"/>
            <alias name="orderId" entity-alias="OI"/>
            <alias name="quantity" entity-alias="OI" function="sum"/>
            <entity-condition><econdition field-name="productId" entity-alias="OI" operator="is-not-null"/></entity-condition>
        </view-entity>
        <view-entity entity-name="OrderAndSum" package="sample">
            <member-entity entity-alias="O" entity-name="sample.Order"/>
            <member-entity entity-alias="IS" entity-name="sample.ItemSum" join-from-alias="O" join-optional="true">
                <key-map field-name="orderId"/>
            </member-entity>
            <alias name="orderId" entity-alias="O"/>
            <alias name="quantity" entity-alias="IS"/>
        </view-entity>"#
    ));
    let postgres = sample_postgres();
    let view = model.entity("sample.OrderAndSum").unwrap();
    assert_eq!(view.alias("quantity").unwrap().java_type, "java.math.BigDecimal");
    let sql = select_view(view, &model, &postgres, &values(), None, &[], LimitOffset::default())
        .unwrap()
        .sql;
    assert_eq!(
        unquote(&sql),
        "SELECT O.orderId, IS.quantity FROM (Order O LEFT OUTER JOIN (SELECT OI.orderId, SUM(OI.quantity) AS quantity FROM OrderItem OI WHERE OI.productId IS NOT NULL GROUP BY OI.orderId) IS ON O.orderId = IS.orderId)"
    );
}

fn billing_model() -> EntityModel {
    sample_model(&format!(
        r#"{ORDERS}
        <view-entity entity-name="ItemAndBilled" package="sample">
            <member-entity entity-alias="OI" entity-name="sample.OrderItem"/>
            <member-entity entity-alias="IB" entity-name="sample.ItemBilling" join-from-alias="OI" join-optional="true" sub-select="true">
                <key-map field-name="orderId"/><key-map field-name="itemSeqId"/>
            </member-entity>
            <alias name="orderId" entity-alias="OI"/>
            <alias name="itemSeqId" entity-alias="OI"/>
            <alias name="quantity" entity-alias="OI"/>
            <alias name="billedAmount" entity-alias="IB" field="amount" function="sum"/>
            <alias name="billingDate" entity-alias="IB"/>
        </view-entity>"#
    ))
}

#[test]
fn a_sub_select_member_aggregates_inside_and_takes_its_conditions() {
    let model = billing_model();
    let view = model.entity("sample.ItemAndBilled").unwrap();
    let options = SelectOptions {
        fields: vec![
            "orderId".to_string(),
            "itemSeqId".to_string(),
            "billedAmount".to_string(),
        ],
        ..SelectOptions::default()
    };
    let condition = Condition::and(vec![
        Condition::equals("orderId", Value::string("O1")),
        Condition::greater("billingDate", Value::string("2026-01-01")),
    ]);

    // Without from-lateral-style the sub-select shows its join fields and joins on them. The sum is
    // taken inside, so the outer select has no GROUP BY.
    let mut plain = sample_postgres();
    plain.from_lateral_style = None;
    let sql = checked(
        select_view_with(
            view,
            &model,
            &plain,
            &values(),
            Some(&condition),
            &[],
            LimitOffset::default(),
            &options,
        )
        .unwrap(),
    );
    assert_eq!(
        unquote(&sql.sql),
        "SELECT OI.orderId, OI.itemSeqId, IB.billedAmount FROM (OrderItem OI LEFT OUTER JOIN (SELECT SUM(IB.amount) AS billedAmount, IB.orderId AS orderId, IB.itemSeqId AS itemSeqId FROM ItemBilling IB WHERE IB.billingDate > ? GROUP BY IB.orderId, IB.itemSeqId) IB ON OI.orderId = IB.orderId AND OI.itemSeqId = IB.itemSeqId) WHERE OI.orderId = ?"
    );
    assert_eq!(sql.params, vec![billing_from(), Value::string("O1")]);
    // With LATERAL the keys move inside and the join is ON 1=1.
    let lateral = sample_postgres();
    assert_eq!(lateral.from_lateral_style.as_deref(), Some("lateral"));
    let sql = checked(
        select_view_with(
            view,
            &model,
            &lateral,
            &values(),
            Some(&condition),
            &[],
            LimitOffset::default(),
            &options,
        )
        .unwrap(),
    );
    assert_eq!(
        unquote(&sql.sql),
        "SELECT OI.orderId, OI.itemSeqId, IB.billedAmount FROM (OrderItem OI LEFT OUTER JOIN LATERAL (SELECT SUM(IB.amount) AS billedAmount FROM ItemBilling IB WHERE OI.orderId = IB.orderId AND OI.itemSeqId = IB.itemSeqId AND IB.billingDate > ?) IB ON 1=1) WHERE OI.orderId = ?"
    );
}

#[test]
fn bad_views_fail_to_load() {
    let root = workspace_root();
    let conf =
        MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).expect("conf");
    let load = |entities: &str| {
        let doc = MNode::parse_text("entities", &format!("<entities>{ORDERS}{entities}</entities>")).unwrap();
        EntityModel::load(&conf, &[doc]).map(|_| ()).unwrap_err().to_string()
    };
    let err = load(
        r#"<view-entity entity-name="OnlyRel" package="sample">
            <member-relationship entity-alias="R" join-from-alias="L" relationship="missing"/>
        </view-entity>"#,
    );
    assert!(err.contains("could not find member-entity L"), "{err}");
    let err = load(
        r#"<view-entity entity-name="BadField" package="sample">
            <member-entity entity-alias="O" entity-name="sample.Order"/>
            <alias name="nope" entity-alias="O"/>
        </view-entity>"#,
    );
    assert!(err.contains("does not exist"), "{err}");
    let err = load(
        r#"<view-entity entity-name="Loop" package="sample">
            <member-entity entity-alias="L" entity-name="sample.Loop"/>
            <alias name="x" entity-alias="L"/>
        </view-entity>"#,
    );
    assert!(err.contains("member of itself"), "{err}");
}

/// Framework, mantle, and the other component entities in the Java tree, when it is next to this one. The tool
/// components ported into the framework are skipped.
#[test]
fn component_views_render_when_the_java_tree_is_present() {
    let root = workspace_root();
    let components = root.join("../moqui/runtime/component");
    if !components.is_dir() {
        return;
    }
    let conf =
        MNode::parse_file(root.join("runtime/base-component/framework/conf/MoquiDefaultConf.xml")).expect("conf");
    let mut documents = Vec::new();
    let mut directories = vec![root.join("runtime/base-component/framework/entity")];
    // The tool components whose entities are in the framework now would be loaded twice.
    let in_framework = ["moqui-sso", "moqui-sftp", "moqui-poi", "moqui-image", "moqui-cups"];
    let mut entries: Vec<_> = std::fs::read_dir(&components)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| !in_framework.iter().any(|name| entry.file_name() == *name))
        .map(|entry| entry.path().join("entity"))
        .filter(|path| path.is_dir())
        .collect();
    entries.sort();
    directories.extend(entries);
    for directory in directories {
        let mut paths: Vec<_> = std::fs::read_dir(&directory)
            .unwrap()
            .filter_map(Result::ok)
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ext| ext == "xml"))
            .collect();
        paths.sort();
        for path in paths {
            let node = MNode::parse_file(&path).unwrap();
            if node.name() == "entities" {
                documents.push(node);
            }
        }
    }
    let model = EntityModel::load(&conf, &documents).expect("component model");
    let dialects = Dialects::load(&conf).unwrap();
    let values = ExpandingValues;
    let mut failures = Vec::new();
    // Its UOM member joins on uomTypeEnumId, which ProductUomDimension does not have; Java's
    // getColumnName fails on it the same way.
    let broken = ["mantle.product.ProductUomDimensionAndType"];
    for name in ["postgres", "mysql8"] {
        let dialect = dialects.get(name).unwrap();
        for view in model
            .entities()
            .iter()
            .filter(|entity| entity.is_view && !broken.contains(&entity.full_name.as_str()))
        {
            match select_view(view, &model, dialect, &values, None, &[], LimitOffset::default()) {
                Ok(statement) => {
                    checked(statement);
                }
                Err(err) => failures.push(format!("{} on {name}: {err}", view.full_name)),
            }
            if let Err(err) = count_view(view, &model, dialect, &values, None, &SelectOptions::default()) {
                failures.push(format!("{} count on {name}: {err}", view.full_name));
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Expands `${...}` to a fixed word, so expressions that need the framework still render.
struct ExpandingValues;

impl super::ConditionValues for ExpandingValues {
    fn now(&self) -> Value {
        Value::string("2026-10-13 12:00:00")
    }

    fn expand(&self, text: &str) -> Result<String, moqui_lib::error::Error> {
        let mut out = String::new();
        let mut rest = text;
        while let Some(start) = rest.find("${") {
            out.push_str(&rest[..start]);
            let Some(end) = rest[start..].find('}') else {
                break;
            };
            out.push('1');
            rest = &rest[start + end + 1..];
        }
        out.push_str(rest);
        Ok(out)
    }

    fn convert(&self, text: &str, java_type: &str) -> Result<Value, moqui_lib::error::Error> {
        PlainValues { now: self.now() }.convert(text, java_type)
    }
}

/// Index names share one namespace per schema, and an existing name is skipped on create, so a
/// collision would quietly drop an index.
#[test]
fn foreign_key_and_index_names_are_unique() {
    let (model, dialects) = framework();
    let postgres = dialects.get("postgres").unwrap();
    let mut indexes = std::collections::HashSet::new();
    let mut constraints = std::collections::HashSet::new();
    for entity in model.entities().iter().filter(|entity| !entity.is_view) {
        for index in &entity.indexes {
            assert!(indexes.insert(index.name.clone()), "{}", index.name);
        }
        for relationship in entity
            .relationships
            .iter()
            .filter(|relationship| relationship.relationship_type == "one" && !relationship.is_auto_reverse)
        {
            let index = foreign_key_index_name(entity, relationship, postgres);
            assert!(indexes.insert(index.clone()), "{index}");
            let constraint = (
                entity.table_name.clone(),
                foreign_key_name(entity, relationship, postgres),
            );
            assert!(constraints.insert(constraint.clone()), "{constraint:?}");
        }
    }
}

/// The `billingDate` condition text as the timestamp it binds: text compared to a timestamp field is converted.
fn billing_from() -> Value {
    moqui_lib::object::basic_convert(&Value::string("2026-01-01"), "java.sql.Timestamp").value
}
