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

//! The data-document calls on `ec.entity`: `getDataDocuments` and the document writers. `ec.search` is in the host
//! table (`script/host/search.rs`).

use super::*;

fn facade_lib<T>(ctx: &Ctx<'_>, result: Result<T, moqui_lib::error::Error>) -> rquickjs::Result<T> {
    result.map_err(|err| throw_lib(ctx, &err))
}

fn timestamp_at(value: Option<&Value>) -> Option<jiff::Timestamp> {
    value.and_then(crate::search::postgres::compile::parse_date_bound)
}

/// `ec.entity.getDataDocuments(dataDocumentId, condition?, fromUpdateStamp?, thruUpdateStamp?)`.
/// The condition is one from `conditionFactory` or a map of alias to value.
pub(super) fn entity_get_data_documents<'js>(
    ctx: Ctx<'js>,
    data_document_id: String,
    rest: Rest<JsValue<'js>>,
) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        flush_current(&ctx, true)?;
        let facade = facade_of(&ctx)?;
        let condition = match rest.0.first() {
            None => None,
            Some(value) if value.is_null() || value.is_undefined() => None,
            Some(value) => match condition_of(value) {
                Some(condition) => Some(condition),
                None => match js_to_host(&ctx, value)? {
                    Value::Map(map) => join(
                        map.into_iter()
                            .map(|(field, value)| Condition::equals(field.as_str(), value))
                            .collect(),
                        "and",
                    ),
                    _ => {
                        return Err(Exception::throw_message(
                            &ctx,
                            "getDataDocuments takes a condition or a map",
                        ))
                    }
                },
            },
        };
        let stamp = |index: usize| -> rquickjs::Result<Option<jiff::Timestamp>> {
            match rest.0.get(index) {
                None => Ok(None),
                Some(value) => Ok(timestamp_at(Some(&js_to_host(&ctx, value)?))),
            }
        };
        let (from, thru) = (stamp(1)?, stamp(2)?);
        let documents = facade_lib(
            &ctx,
            crate::entity::document::data_documents(&facade, &data_document_id, condition, from, thru),
        )?;
        host_to_js(&ctx, Value::List(documents.into_iter().map(Value::Map).collect()))
    })
}
