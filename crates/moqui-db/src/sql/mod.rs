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

//! Condition tree and SQL text. No connection and no driver.
//!
//! Physical identifiers use the model's Java-style names, database folding and column
//! `name-replace` rules. Logical aliases are quoted verbatim. Postgres quotes with `"`;
//! MySQL quotes with backticks. Placeholders stay `?`.
//!
//! Every builder that writes a placeholder returns a [`Statement`] whose `params` are the values
//! for those placeholders, in order. Insert and update `SET` columns are the exception: the caller
//! binds those first, then the statement's params.

mod condition;
mod ident;
mod statement;
mod view;

use moqui_lib::error::Error;
use moqui_lib::object::{basic_convert, Value};

pub use condition::{comparison, render_condition, CompareOp, Condition, FieldRef, LimitOffset, Nulls, OrderBy};
pub use ident::{
    is_mysql, is_sqlite, legacy_name, physical_column_name, physical_name, quote_column, quote_ident,
    quote_physical_ident,
};
pub use statement::{
    add_column, count, count_with, create_fk_index, create_foreign_key, create_index, create_table, delete,
    drop_fts_index, foreign_key_index_name, foreign_key_name, fts_index_name, fts_index_sql, fts_old_index_name,
    insert, insert_or_update, select, select_with, update, SelectOptions,
};
pub use view::{count_view, select_view, select_view_with};

/// SQL text and the values for its placeholders, in order.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Statement {
    pub sql: String,
    pub params: Vec<Value>,
}

/// What a find needs from the running framework to evaluate a view's `entity-condition` and the
/// `${}` in its SQL expressions. Core passes its execution context; tests use [`PlainValues`].
pub trait ConditionValues {
    /// What a `date-filter` without `valid-date` compares with.
    fn now(&self) -> Value;
    /// Expand `${}` in a definition value or SQL expression. Only called on text that has `${`.
    fn expand(&self, text: &str) -> Result<String, Error>;
    /// Convert definition text to a Java type, as Java `convertFieldString` does.
    fn convert(&self, text: &str, java_type: &str) -> Result<Value, Error>;
}

/// [`ConditionValues`] with a fixed now, plain conversion, and no expansion.
#[derive(Debug, Clone)]
pub struct PlainValues {
    pub now: Value,
}

impl ConditionValues for PlainValues {
    fn now(&self) -> Value {
        self.now.clone()
    }

    fn expand(&self, text: &str) -> Result<String, Error> {
        Err(Error::new(format!("no context to expand {text}")))
    }

    fn convert(&self, text: &str, java_type: &str) -> Result<Value, Error> {
        if text.is_empty() || text == "null" {
            return Ok(Value::Null);
        }
        let converted = basic_convert(&Value::string(text), java_type);
        Ok(converted.value)
    }
}

#[cfg(test)]
mod names_tests;
#[cfg(test)]
mod tests;
