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

//! Database profiles from the `<database>` block. No SQL text and no driver.

use std::collections::BTreeMap;
use std::str::FromStr;

use moqui_lib::error::Error;
use moqui_lib::node::MNode;

/// JDBC `?` placeholder. Every profile uses this. The conf has no attribute for it.
pub const PLACEHOLDER: &str = "?";

/// One `<name-replace original replace>` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NameReplace {
    pub original: String,
    pub replace: String,
}

/// One `<database>` profile.
///
/// String fields are `None` when the attribute is absent. XSD defaults are not filled in.
/// `never_nulls`, `never_try_insert`, and `use_binary_type_for_blob` are false when absent.
/// `use_foreign_key_indexes`, `use_pk_constraint_names`, `use_foreign_keys`, `use_indexes`, `use_indexes_unique` and
/// `fts_ddl` are true unless the attribute is `false`.
/// `sql_types` holds only that database's `<database-type>` entries.
/// `placeholder` is always [`PLACEHOLDER`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialect {
    pub name: String,
    /// Preserve the pre-compatibility Rust quoted identifiers. Never migrate automatically.
    pub legacy_names: bool,
    pub placeholder: &'static str,
    pub fts_style: Option<String>,
    pub fts_config: Option<String>,
    /// `fts-ddl`: false leaves the full-text column and index of a `text-fts` field to a DBA.
    pub fts_ddl: bool,
    pub join_style: Option<String>,
    pub offset_style: Option<String>,
    pub from_lateral_style: Option<String>,
    pub never_nulls: bool,
    pub never_try_insert: bool,
    pub use_binary_type_for_blob: bool,
    pub constraint_name_clip_length: Option<u32>,
    pub lb_name: Option<String>,
    pub table_engine: Option<String>,
    pub character_set: Option<String>,
    pub collate: Option<String>,
    pub fk_style: Option<String>,
    /// `use-fk-initially-deferred`: a foreign key is created `INITIALLY DEFERRED`.
    pub use_fk_initially_deferred: bool,
    /// `always-use-constraint-keyword`: a `NOT NULL` column in a create table is `CONSTRAINT NOT NULL`.
    pub always_use_constraint_keyword: bool,
    pub default_test_query: Option<String>,
    pub result_fetch_size: Option<i32>,
    /// Text written in place of `FOR UPDATE`. Empty writes nothing.
    pub for_update: Option<String>,
    pub use_foreign_key_indexes: bool,
    pub use_pk_constraint_names: bool,
    /// `use-foreign-keys`: false creates no foreign keys.
    pub use_foreign_keys: bool,
    /// `use-indexes`: false creates no declared indexes.
    pub use_indexes: bool,
    /// `use-indexes-unique`: false creates a declared unique index as a plain one.
    pub use_indexes_unique: bool,
    /// `use-indexes-unique-where-not-null`: a unique index is `CREATE UNIQUE WHERE NOT NULL INDEX` (DB2).
    pub use_indexes_unique_where_not_null: bool,
    /// `<database-type type>` to `sql-type`. A missing type is left for the dictionary default.
    pub sql_types: BTreeMap<String, String>,
    pub name_replaces: Vec<NameReplace>,
}

impl Dialect {
    /// SQL type override for a dictionary type, if this database sets one.
    pub fn sql_type(&self, field_type: &str) -> Option<&str> {
        self.sql_types.get(field_type).map(String::as_str)
    }
}

/// `<database>` profiles keyed by `name`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dialects {
    by_name: BTreeMap<String, Dialect>,
}

impl Dialects {
    /// Load every `<database>` under `database-list`.
    ///
    /// `node` may be that element or a conf root that contains one.
    pub fn load(node: &MNode) -> Result<Self, Error> {
        let list = database_list(node)?;
        let legacy_names = legacy_names(node)?;
        let mut by_name = BTreeMap::new();
        for child in list.children_named("database") {
            let mut dialect = parse_database(&child)?;
            dialect.legacy_names = legacy_names;
            let name = dialect.name.clone();
            if by_name.contains_key(&name) {
                return Err(at(&child, format!("database \"{name}\" is duplicated")));
            }
            by_name.insert(name, dialect);
        }
        Ok(Self { by_name })
    }

    pub fn get(&self, name: &str) -> Option<&Dialect> {
        self.by_name.get(name)
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &Dialect)> {
        self.by_name.iter().map(|(name, dialect)| (name.as_str(), dialect))
    }

    pub fn len(&self) -> usize {
        self.by_name.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }
}

/// Rust-only compatibility switch, shared by the model and SQL dialects.
pub(crate) fn legacy_names(conf: &MNode) -> Result<bool, Error> {
    let raw = conf
        .first_named("entity-facade")
        .and_then(|node| node.attribute("sql-name-style"))
        .unwrap_or_else(|| "java".to_string());
    match moqui_lib::env::expand(&raw)?.as_str() {
        "java" => Ok(false),
        "legacy" => Ok(true),
        other => Err(Error::new(format!(
            "sql-name-style must be java or legacy, found {other}"
        ))),
    }
}

fn database_list(node: &MNode) -> Result<MNode, Error> {
    if node.name() == "database-list" {
        return Ok(node.clone());
    }
    if let Some(list) = node.first_named("database-list") {
        return Ok(list);
    }
    Err(at(node, format!("expected database-list, found {}", node.name())))
}

fn parse_database(node: &MNode) -> Result<Dialect, Error> {
    let name = required(node, "name")?;
    let mut sql_types = BTreeMap::new();
    let mut name_replaces = Vec::new();
    for child in node.children() {
        match child.name().as_str() {
            "database-type" => {
                let field_type = required(&child, "type")?;
                let sql_type = required(&child, "sql-type")?;
                if sql_types.contains_key(&field_type) {
                    return Err(at(
                        &child,
                        format!("database \"{name}\" repeats database-type \"{field_type}\""),
                    ));
                }
                sql_types.insert(field_type, sql_type);
            }
            "name-replace" => {
                name_replaces.push(NameReplace {
                    original: required(&child, "original")?,
                    replace: required(&child, "replace")?,
                });
            }
            _ => {}
        }
    }
    Ok(Dialect {
        name,
        legacy_names: false,
        placeholder: PLACEHOLDER,
        fts_style: optional(node, "fts-style"),
        fts_config: optional(node, "fts-config"),
        fts_ddl: !matches!(optional(node, "fts-ddl").as_deref(), Some("false")),
        join_style: optional(node, "join-style"),
        offset_style: optional(node, "offset-style"),
        from_lateral_style: optional(node, "from-lateral-style"),
        never_nulls: flag(node, "never-nulls")?,
        never_try_insert: flag(node, "never-try-insert")?,
        use_binary_type_for_blob: flag(node, "use-binary-type-for-blob")?,
        constraint_name_clip_length: optional_int(node, "constraint-name-clip-length")?,
        lb_name: optional(node, "lb-name"),
        table_engine: optional(node, "table-engine"),
        character_set: optional(node, "character-set"),
        collate: optional(node, "collate"),
        fk_style: optional(node, "fk-style"),
        use_fk_initially_deferred: matches!(optional(node, "use-fk-initially-deferred").as_deref(), Some("true")),
        always_use_constraint_keyword: matches!(
            optional(node, "always-use-constraint-keyword").as_deref(),
            Some("true")
        ),
        default_test_query: optional(node, "default-test-query"),
        result_fetch_size: optional_int(node, "result-fetch-size")?,
        // Present and empty means no row lock text at all (SQLite). Absent means `FOR UPDATE`.
        for_update: node.attribute("for-update").map(|value| value.trim().to_string()),
        use_foreign_key_indexes: !matches!(optional(node, "use-foreign-key-indexes").as_deref(), Some("false")),
        use_pk_constraint_names: !matches!(optional(node, "use-pk-constraint-names").as_deref(), Some("false")),
        use_foreign_keys: !matches!(optional(node, "use-foreign-keys").as_deref(), Some("false")),
        use_indexes: !matches!(optional(node, "use-indexes").as_deref(), Some("false")),
        use_indexes_unique: !matches!(optional(node, "use-indexes-unique").as_deref(), Some("false")),
        use_indexes_unique_where_not_null: matches!(
            optional(node, "use-indexes-unique-where-not-null").as_deref(),
            Some("true")
        ),
        sql_types,
        name_replaces,
    })
}

fn optional(node: &MNode, attr: &str) -> Option<String> {
    node.attribute(attr).and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn required(node: &MNode, attr: &str) -> Result<String, Error> {
    optional(node, attr).ok_or_else(|| at(node, format!("{} is missing {attr}", node.name())))
}

fn flag(node: &MNode, attr: &str) -> Result<bool, Error> {
    match optional(node, attr).as_deref() {
        None => Ok(false),
        Some("true") => Ok(true),
        Some("false") => Ok(false),
        Some(value) => Err(at(node, format!("{attr} must be true or false, found \"{value}\""))),
    }
}

fn optional_int<T>(node: &MNode, attr: &str) -> Result<Option<T>, Error>
where
    T: FromStr,
{
    let Some(value) = optional(node, attr) else {
        return Ok(None);
    };
    value
        .parse()
        .map(Some)
        .map_err(|_| at(node, format!("{attr} is not a valid integer, found \"{value}\"")))
}

fn at(node: &MNode, message: impl Into<String>) -> Error {
    Error::at(message, node.file_location(), node.line(), node.column())
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn workspace_root() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
    }

    fn load_default_conf() -> Dialects {
        let path = workspace_root().join("runtime/base-component/framework/conf/MoquiDefaultConf.xml");
        let root = MNode::parse_file(&path).expect("parse MoquiDefaultConf.xml");
        let from_root = Dialects::load(&root).expect("load conf root");
        let list = root.first_named("database-list").expect("database-list");
        let from_list = Dialects::load(&list).expect("load database-list");
        assert_eq!(from_root, from_list);
        from_root
    }

    fn profile<'a>(dialects: &'a Dialects, name: &str) -> &'a Dialect {
        dialects.get(name).unwrap_or_else(|| panic!("missing database {name}"))
    }

    #[test]
    fn default_conf_profiles() {
        let dialects = load_default_conf();
        assert!(!dialects.is_empty());
        let names: Vec<&str> = dialects.iter().map(|(name, _)| name).collect();
        assert_eq!(
            names,
            ["db2", "db2i", "derby", "h2", "hsql", "mssql", "mysql", "mysql8", "oracle", "postgres", "sqlite"]
        );
        assert_eq!(dialects.len(), names.len());
        for (name, dialect) in dialects.iter() {
            assert_eq!(dialect.name, name);
            assert_eq!(dialect.placeholder, PLACEHOLDER);
        }
        assert_eq!(PLACEHOLDER, "?");

        let postgres = profile(&dialects, "postgres");
        assert_eq!(postgres.fts_style.as_deref(), Some("postgres-tsvector"));
        assert_eq!(postgres.fts_config.as_deref(), Some("english"));
        assert_eq!(postgres.join_style.as_deref(), Some("ansi"));
        assert_eq!(postgres.from_lateral_style.as_deref(), Some("lateral"));
        assert!(postgres.never_try_insert);
        assert!(!postgres.never_nulls);
        assert!(postgres.offset_style.is_none());
        assert!(postgres.use_binary_type_for_blob);
        assert_eq!(postgres.constraint_name_clip_length, Some(60));
        assert_eq!(postgres.lb_name.as_deref(), Some("postgresql"));
        assert_eq!(postgres.default_test_query.as_deref(), Some("SELECT 1"));
        assert_eq!(postgres.result_fetch_size, Some(50));
        assert!(postgres.table_engine.is_none());
        assert_eq!(postgres.sql_type("number-float"), Some("FLOAT8"));
        assert_eq!(postgres.sql_type("text-medium"), Some("TEXT"));
        assert_eq!(postgres.sql_type("binary-very-long"), Some("BYTEA"));
        assert!(postgres.sql_type("date-time").is_none());
        assert!(postgres.sql_type("id").is_none());
        assert!(postgres.name_replaces.is_empty());

        let mysql = profile(&dialects, "mysql");
        assert_eq!(mysql.fts_style.as_deref(), Some("mysql-fulltext"));
        assert!(mysql.fts_config.is_none());
        assert_eq!(mysql.join_style.as_deref(), Some("ansi-no-parenthesis"));
        assert_eq!(mysql.offset_style.as_deref(), Some("limit"));
        assert!(mysql.never_nulls);
        assert!(!mysql.never_try_insert);
        assert!(mysql.from_lateral_style.is_none());
        assert!(!mysql.use_binary_type_for_blob);
        assert_eq!(mysql.table_engine.as_deref(), Some("InnoDB"));
        assert_eq!(mysql.character_set.as_deref(), Some("utf8"));
        assert_eq!(mysql.collate.as_deref(), Some("utf8_general_ci"));
        assert_eq!(mysql.fk_style.as_deref(), Some("name_fk"));
        assert_eq!(mysql.constraint_name_clip_length, Some(60));
        assert_eq!(mysql.sql_type("date-time"), Some("DATETIME(3)"));
        assert_eq!(mysql.sql_type("text-very-long"), Some("LONGTEXT"));
        assert!(mysql.sql_type("text-medium").is_none());
        assert_eq!(
            mysql.name_replaces,
            vec![NameReplace {
                original: "CONDITION".to_string(),
                replace: "THE_CONDITION".to_string(),
            }]
        );

        let mysql8 = profile(&dialects, "mysql8");
        assert_eq!(mysql8.from_lateral_style.as_deref(), Some("lateral"));
        assert_eq!(mysql8.sql_type("text-very-long"), Some("LONGTEXT"));
        assert_eq!(mysql8.lb_name.as_deref(), Some("mysql"));
        assert_eq!(mysql8.offset_style.as_deref(), Some("limit"));

        let h2 = profile(&dialects, "h2");
        assert_eq!(h2.fts_style.as_deref(), Some("h2-native"));
        assert!(h2.offset_style.is_none());
        assert!(h2.join_style.is_none());
        assert!(h2.sql_types.is_empty());
        assert_eq!(
            h2.name_replaces,
            vec![NameReplace {
                original: "VALUE".to_string(),
                replace: "THE_VALUE".to_string(),
            }]
        );

        let oracle = profile(&dialects, "oracle");
        assert_eq!(oracle.fts_style.as_deref(), Some("oracle-text"));
        assert_eq!(oracle.from_lateral_style.as_deref(), Some("apply"));
        assert_eq!(oracle.sql_type("text-medium"), Some("VARCHAR2(255)"));
        assert!(oracle.offset_style.is_none());

        assert_eq!(profile(&dialects, "derby").fts_style.as_deref(), Some("none"));
        assert!(profile(&dialects, "derby").join_style.is_none());
        assert_eq!(profile(&dialects, "db2").offset_style.as_deref(), Some("limit"));
        assert_eq!(profile(&dialects, "mssql").offset_style.as_deref(), Some("fetch"));
        assert_eq!(profile(&dialects, "hsql").sql_type("binary-very-long"), Some("OBJECT"));

        let sqlite = profile(&dialects, "sqlite");
        assert_eq!(sqlite.for_update.as_deref(), Some(""));
        assert_eq!(sqlite.offset_style.as_deref(), Some("limit"));
        assert_eq!(sqlite.sql_type("date-time"), Some("TEXT"));
        assert!(postgres.for_update.is_none());
    }

    #[test]
    fn absent_attributes_stay_absent() {
        let root = MNode::parse_text(
            "sample",
            r#"<moqui-conf>
                <database-list>
                    <dictionary-type type="text-medium" java-type="java.lang.String" default-sql-type="VARCHAR(255)"/>
                    <database name="custom">
                        <database-type type="number-float" sql-type="FLOAT8"/>
                    </database>
                    <database name="flags" never-nulls="false" never-try-insert="false"
                            use-binary-type-for-blob="false" offset-style="fetch"/>
                </database-list>
            </moqui-conf>"#,
        )
        .expect("parse");
        let dialects = Dialects::load(&root).expect("load");
        let custom = profile(&dialects, "custom");
        assert_eq!(custom.placeholder, "?");
        assert!(custom.offset_style.is_none());
        assert!(custom.from_lateral_style.is_none());
        assert!(custom.fts_style.is_none());
        assert!(custom.join_style.is_none());
        assert!(!custom.never_nulls);
        assert!(!custom.never_try_insert);
        assert!(!custom.use_binary_type_for_blob);
        assert!(custom.constraint_name_clip_length.is_none());
        assert!(custom.result_fetch_size.is_none());
        assert_eq!(custom.sql_type("number-float"), Some("FLOAT8"));
        assert!(custom.sql_type("text-medium").is_none());

        let flags = profile(&dialects, "flags");
        assert!(!flags.never_nulls);
        assert!(!flags.never_try_insert);
        assert!(!flags.use_binary_type_for_blob);
        assert_eq!(flags.offset_style.as_deref(), Some("fetch"));
    }

    #[test]
    fn bad_database_list_is_an_error() {
        let missing = MNode::parse_text("missing", "<moqui-conf/>").expect("parse");
        let err = Dialects::load(&missing).expect_err("missing list");
        assert!(err.message().contains("database-list"), "{err}");

        let duplicated = MNode::parse_text(
            "dup",
            r#"<database-list>
                <database name="postgres"/>
                <database name="postgres" offset-style="limit"/>
            </database-list>"#,
        )
        .expect("parse");
        let err = Dialects::load(&duplicated).expect_err("duplicate");
        assert!(err.message().contains("duplicated"), "{err}");
        assert!(err.message().contains("postgres"), "{err}");

        let bad_flag = MNode::parse_text(
            "flag",
            r#"<database-list><database name="mysql" never-nulls="yes"/></database-list>"#,
        )
        .expect("parse");
        let err = Dialects::load(&bad_flag).expect_err("flag");
        assert!(err.message().contains("never-nulls"), "{err}");

        let bad_type = MNode::parse_text(
            "type",
            r#"<database-list><database name="mysql"><database-type type="id"/></database></database-list>"#,
        )
        .expect("parse");
        let err = Dialects::load(&bad_type).expect_err("sql-type");
        assert!(err.message().contains("sql-type"), "{err}");

        let bad_int = MNode::parse_text(
            "int",
            r#"<database-list><database name="mysql" constraint-name-clip-length="60px"/></database-list>"#,
        )
        .expect("parse");
        let err = Dialects::load(&bad_int).expect_err("clip");
        assert!(err.message().contains("constraint-name-clip-length"), "{err}");
    }
}
