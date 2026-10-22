//! Physical naming compatibility. The older SQL snapshots also exercise explicit legacy mode.
use std::path::Path;

use moqui_lib::{node::MNode, object::Value};

use super::*;
use crate::{
    dialect::Dialects,
    model::{camel_case_to_underscored, EntityModel},
};

fn fixture(style: &str) -> (EntityModel, Dialects) {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime/base-component/framework");
    let conf = MNode::parse_file(root.join("conf/MoquiDefaultConf.xml")).unwrap();
    conf.first_named("entity-facade")
        .unwrap()
        .set_attribute("sql-name-style", style);
    let entities = MNode::parse_text(
        "names",
        r#"<entities>
        <entity entity-name="UserAccount" package="test">
            <field name="userId" type="id" is-pk="true"/>
            <field name="userName" type="text-medium"/>
            <field name="URLValue" type="text-medium"/>
        </entity>
        <entity entity-name="LoginRecord" package="test" table-name="CUSTOM_LOGIN">
            <field name="loginId" column-name="CUSTOM_ID" type="id" is-pk="true"/>
            <field name="userId" column-name="ACCOUNT_REF" type="id"/>
            <field name="value" type="text-medium"/>
            <field name="mixedOverride" column-name="MixedCase" type="text-medium"/>
            <relationship type="one" related="test.UserAccount"><key-map field-name="userId"/></relationship>
            <index name="LOGIN_USER"><index-field name="userId"/></index>
        </entity>
        <view-entity entity-name="LoginUser" package="test">
            <member-entity entity-alias="L" entity-name="test.LoginRecord"/>
            <member-entity entity-alias="U" entity-name="test.UserAccount" join-from-alias="L">
                <key-map field-name="userId"/>
            </member-entity>
            <alias entity-alias="U" name="userId"/>
            <alias entity-alias="L" name="loginId"/>
        </view-entity>
    </entities>"#,
    )
    .unwrap();
    (
        EntityModel::load(&conf, &[entities]).unwrap(),
        Dialects::load(&conf).unwrap(),
    )
}

#[test]
fn java_conversion_splits_each_capital_not_acronym_words() {
    for (logical, physical) in [
        ("", ""),
        ("UserAccount", "USER_ACCOUNT"),
        ("userId", "USER_ID"),
        ("URLValue", "U_R_L_VALUE"),
        ("userID", "USER_I_D"),
        ("XML2JSON", "X_M_L2_J_S_O_N"),
        ("already_snake", "ALREADY_SNAKE"),
        ("a_B", "A__B"),
        ("straße", "STRAßE"),
        ("a𐐨B", "A𐐨_B"),
    ] {
        assert_eq!(camel_case_to_underscored(logical), physical);
    }
}

#[test]
fn java_names_cover_crud_schema_keys_and_joins_without_changing_logical_aliases() {
    let (model, dialects) = fixture("java");
    let pg = dialects.get("postgres").unwrap();
    let user = model.entity("UserAccount").unwrap();
    let login = model.entity("LoginRecord").unwrap();
    assert_eq!(user.table_name, "USER_ACCOUNT");
    assert_eq!(user.field("userId").unwrap().column_name, "USER_ID");
    assert_eq!(
        user.field("lastUpdatedStamp").unwrap().column_name,
        "LAST_UPDATED_STAMP"
    );
    assert_eq!(login.table_name, "CUSTOM_LOGIN");
    assert_eq!(login.field("loginId").unwrap().column_name, "CUSTOM_ID");
    assert_eq!(
        insert(login, pg, &["mixedOverride"]).unwrap(),
        "INSERT INTO \"custom_login\" (\"mixedcase\") VALUES (?)"
    );
    assert_eq!(
        insert(user, pg, &["userId", "URLValue"]).unwrap(),
        "INSERT INTO \"user_account\" (\"user_id\", \"u_r_l_value\") VALUES (?, ?)"
    );
    let condition = Condition::equals("userId", Value::string("U1"));
    assert_eq!(
        update(user, pg, &["userName"], Some(&condition)).unwrap().sql,
        "UPDATE \"user_account\" SET \"user_name\" = ? WHERE \"user_id\" = ?"
    );
    assert_eq!(
        delete(user, pg, Some(&condition)).unwrap().sql,
        "DELETE FROM \"user_account\" WHERE \"user_id\" = ?"
    );
    let selected = select(
        user,
        pg,
        &[],
        Some(&condition),
        &[OrderBy::asc("userName")],
        LimitOffset::default(),
    )
    .unwrap();
    assert!(selected.sql.contains("\"user_id\" AS \"userId\""), "{}", selected.sql);
    assert!(selected.sql.contains("ORDER BY \"user_name\""), "{}", selected.sql);
    assert_eq!(
        count(user, pg, Some(&condition)).unwrap().sql,
        "SELECT COUNT(*) FROM \"user_account\" WHERE \"user_id\" = ?"
    );
    let ddl = create_table(user, &model, pg).unwrap();
    assert!(ddl.starts_with("CREATE TABLE \"user_account\" (\"user_id\""), "{ddl}");
    assert!(ddl.contains("PRIMARY KEY (\"user_id\")"), "{ddl}");
    assert!(add_column(user, user.field("userName").unwrap(), &model, pg)
        .unwrap()
        .starts_with("ALTER TABLE \"user_account\" ADD \"user_name\""));
    assert_eq!(
        create_index(login, &login.indexes[0], pg).unwrap(),
        "CREATE INDEX \"login_user\" ON \"custom_login\" (\"account_ref\")"
    );
    let rel = login.relationship("UserAccount").unwrap();
    let fk = create_foreign_key(login, rel, user, pg).unwrap();
    assert!(
        fk.contains("FOREIGN KEY (\"account_ref\") REFERENCES \"user_account\" (\"user_id\")"),
        "{fk}"
    );
    assert!(create_fk_index(login, rel, pg)
        .unwrap()
        .ends_with("ON \"custom_login\" (\"account_ref\")"));
    assert!(insert_or_update(user, pg, &["userId", "userName"])
        .unwrap()
        .contains("ON CONFLICT (\"user_id\") DO UPDATE SET \"user_name\" = EXCLUDED.\"user_name\""));
    let view = model.entity("LoginUser").unwrap();
    let joined = select_view(
        view,
        &model,
        pg,
        &PlainValues { now: Value::Null },
        None,
        &[],
        LimitOffset::default(),
    )
    .unwrap()
    .sql;
    // A member's table alias folds as Java's unquoted `CUSTOM_LOGIN L` does on Postgres, so raw SQL can name it.
    assert!(joined.contains("\"l\".\"account_ref\" = \"u\".\"user_id\""), "{joined}");
    assert!(joined.contains("AS \"userId\""), "{joined}");
    let mysql = dialects.get("mysql").unwrap();
    assert_eq!(
        insert(user, mysql, &["userId"]).unwrap(),
        "INSERT INTO `USER_ACCOUNT` (`USER_ID`) VALUES (?)"
    );
    let mysql_ddl = create_table(user, &model, mysql).unwrap();
    assert!(mysql_ddl.contains("CONSTRAINT `PK_USER_ACCOUNT`"), "{mysql_ddl}");
    let mut unnamed_pk = mysql.clone();
    unnamed_pk.use_pk_constraint_names = false;
    assert!(!create_table(user, &model, &unnamed_pk).unwrap().contains("CONSTRAINT"));
    assert!(mysql_ddl.contains("PRIMARY KEY (`USER_ID`)"), "{mysql_ddl}");
    // Oracle folds an unquoted name to upper case. The logical alias stays as written.
    let oracle = dialects.get("oracle").unwrap();
    let selected = select(
        login,
        oracle,
        &[FieldRef::new("value")],
        None,
        &[],
        LimitOffset::default(),
    )
    .unwrap()
    .sql;
    assert!(selected.contains("\"VALUE\" AS \"value\""), "{selected}");
    // Replacements affect columns only, never table names or logical aliases.
    let mut replaced = pg.clone();
    replaced.name_replaces = vec![
        crate::dialect::NameReplace {
            original: "USER_ID".into(),
            replace: "ACCOUNT_KEY".into(),
        },
        crate::dialect::NameReplace {
            original: "ACCOUNT_KEY".into(),
            replace: "FINAL_KEY".into(),
        },
        crate::dialect::NameReplace {
            original: "USER_ACCOUNT".into(),
            replace: "WRONG_TABLE".into(),
        },
    ];
    assert_eq!(
        insert(user, &replaced, &["userId"]).unwrap(),
        "INSERT INTO \"user_account\" (\"final_key\") VALUES (?)"
    );
    replaced.name_replaces.push(crate::dialect::NameReplace {
        original: "CUSTOM_ID".into(),
        replace: "OVERRIDE_KEY".into(),
    });
    assert_eq!(
        insert(login, &replaced, &["loginId"]).unwrap(),
        "INSERT INTO \"custom_login\" (\"override_key\") VALUES (?)"
    );
}

#[test]
fn unknown_naming_mode_is_rejected_not_silently_selected() {
    let conf = MNode::parse_text(
        "bad-mode",
        r#"<moqui-conf>
        <entity-facade sql-name-style="typo"/>
        <database-list><database name="postgres"/></database-list>
    </moqui-conf>"#,
    )
    .unwrap();
    assert!(Dialects::load(&conf)
        .unwrap_err()
        .to_string()
        .contains("sql-name-style"));
    assert!(EntityModel::load(&conf, &[]).is_err());
}

#[test]
fn legacy_is_explicit_and_keeps_existing_quoted_schema_and_overrides() {
    let (model, dialects) = fixture("legacy");
    let pg = dialects.get("postgres").unwrap();
    assert_eq!(
        insert(model.entity("UserAccount").unwrap(), pg, &["userId"]).unwrap(),
        "INSERT INTO \"UserAccount\" (\"userId\") VALUES (?)"
    );
    assert_eq!(
        insert(model.entity("LoginRecord").unwrap(), pg, &["loginId"]).unwrap(),
        "INSERT INTO \"CUSTOM_LOGIN\" (\"CUSTOM_ID\") VALUES (?)"
    );
    let mut replaced = pg.clone();
    replaced.name_replaces = vec![
        crate::dialect::NameReplace {
            original: "UserAccount".into(),
            replace: "OLD_ACCOUNTS".into(),
        },
        crate::dialect::NameReplace {
            original: "userId".into(),
            replace: "OLD_KEY".into(),
        },
        crate::dialect::NameReplace {
            original: "OLD_KEY".into(),
            replace: "NOT_CHAINED".into(),
        },
    ];
    assert_eq!(
        insert(model.entity("UserAccount").unwrap(), &replaced, &["userId"]).unwrap(),
        "INSERT INTO \"OLD_ACCOUNTS\" (\"OLD_KEY\") VALUES (?)"
    );
}

/// Java writes a view's member tables with their entity-alias unquoted (`CUSTOM_LOGIN L`), so raw SQL such as a
/// `makeConditionWhere` names a member by that alias in any case. The alias is quoted as the database folds it unquoted.
#[test]
fn a_raw_condition_names_a_view_member_by_its_java_alias() {
    let (model, dialects) = fixture("java");
    let view = model.entity("LoginUser").unwrap();
    let raw = Condition::where_sql("L.ACCOUNT_REF IS NOT NULL");
    let sql = |name: &str| {
        select_view(
            view,
            &model,
            dialects.get(name).unwrap(),
            &PlainValues { now: Value::Null },
            Some(&raw),
            &[],
            LimitOffset::default(),
        )
        .unwrap()
        .sql
    };
    let pg = sql("postgres");
    assert!(
        pg.contains(
            "\"custom_login\" \"l\" INNER JOIN \"user_account\" \"u\" ON \"l\".\"account_ref\" = \"u\".\"user_id\""
        ),
        "{pg}"
    );
    assert!(pg.contains("\"l\".\"custom_id\" AS \"loginId\""), "{pg}");
    assert!(pg.ends_with("(L.ACCOUNT_REF IS NOT NULL)"), "{pg}");
    let oracle = sql("oracle");
    assert!(
        oracle.contains("\"CUSTOM_LOGIN\" \"L\" INNER JOIN \"USER_ACCOUNT\" \"U\""),
        "{oracle}"
    );
    let mysql = sql("mysql");
    assert!(
        mysql.contains("`CUSTOM_LOGIN` `L` INNER JOIN `USER_ACCOUNT` `U`"),
        "{mysql}"
    );
    // The legacy style quotes names as written, the aliases with them.
    let (model, dialects) = fixture("legacy");
    let legacy = select_view(
        model.entity("LoginUser").unwrap(),
        &model,
        dialects.get("postgres").unwrap(),
        &PlainValues { now: Value::Null },
        None,
        &[],
        LimitOffset::default(),
    )
    .unwrap()
    .sql;
    assert!(legacy.contains("\"CUSTOM_LOGIN\" \"L\""), "{legacy}");
}
