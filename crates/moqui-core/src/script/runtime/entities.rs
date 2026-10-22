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

//! Entity finds, lists, and values as scripts see them.

use moqui_lib::sync::lock;

use rquickjs::class::{PropertyDescriptor, PropertyName};
use rquickjs::Atom;

use super::*;

pub(super) fn entity_find<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<Class<'js, HostFind>> {
    host_guard(&ctx, || {
        let facade = facade_of(&ctx)?;
        let find = facade.find(&name).map_err(|err| throw_lib(&ctx, &err))?;
        wrap_find(&ctx, find)
    })
}

/// `ec.entity.sequencedIdPrimary(seqName, staggerMax, bankSize)`: the next id of a sequence, as text.
pub(super) fn entity_sequenced_id_primary<'js>(
    ctx: Ctx<'js>,
    name: String,
    stagger: Opt<JsValue<'js>>,
    bank_size: Opt<JsValue<'js>>,
) -> rquickjs::Result<String> {
    host_guard(&ctx, || {
        let facade = facade_of(&ctx)?;
        let number = |value: &Opt<JsValue<'js>>| -> rquickjs::Result<Option<u64>> {
            match value
                .0
                .as_ref()
                .filter(|value| !value.is_null() && !value.is_undefined())
            {
                Some(value) => Ok(u64::try_from(js_i64(&ctx, value)?).ok()),
                None => Ok(None),
            }
        };
        crate::entity::sequenced_id_primary(&facade, &name, number(&stagger)?, number(&bank_size)?)
            .map_err(|err| throw_lib(&ctx, &err))
    })
}

pub(super) fn entity_temp_set_sequenced_id_primary<'js>(
    ctx: Ctx<'js>,
    name: String,
    next: JsValue<'js>,
    bank_size: JsValue<'js>,
) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        let facade = facade_of(&ctx)?;
        let next = js_i64(&ctx, &next)?;
        let bank_size = js_i64(&ctx, &bank_size)?;
        facade.temp_set_sequenced_id_primary(&name, next, bank_size);
        Ok(())
    })
}

pub(super) fn entity_temp_reset_sequenced_id_primary<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        facade_of(&ctx)?.temp_reset_sequenced_id_primary(&name);
        Ok(())
    })
}

pub(super) fn entity_make_value<'js>(ctx: Ctx<'js>, name: String) -> rquickjs::Result<JsValue<'js>> {
    host_guard(&ctx, || {
        let facade = facade_of(&ctx)?;
        let value = facade.make_value(&name).map_err(|err| throw_lib(&ctx, &err))?;
        entity_to_js(&ctx, HostEntity::new(value))
    })
}

/// `ec.entity.startTxCacheDb(hold)`: enter sim on this thread, as Java's `TransactionCacheDb`.
/// Only `hold=true` exists; a second start while one is active is refused, as in Java.
pub(super) fn entity_start_tx_cache_db(ctx: Ctx<'_>, hold: bool) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        if !hold {
            return Err(Exception::throw_message(
                &ctx,
                "A TX cache DB that flushes to production is no longer supported. Use hold=true, which never writes to production.",
            ));
        }
        if crate::entity::cache::sim_entered() {
            return Err(Exception::throw_message(
                &ctx,
                "A TX cache is already active on this ExecutionContext",
            ));
        }
        let facade = facade_of(&ctx)?;
        crate::entity::cache::enter_sim(&facade).map_err(|err| throw_lib(&ctx, &err))
    })
}

/// `ec.entity.stopTxCache()`: drop this thread's sim schema. No active cache is a no-op.
pub(super) fn entity_stop_tx_cache(ctx: Ctx<'_>) -> rquickjs::Result<()> {
    host_guard(&ctx, || {
        if !crate::entity::cache::sim_entered() {
            return Ok(());
        }
        let facade = facade_of(&ctx)?;
        crate::entity::cache::exit_sim(&facade).map_err(|err| throw_lib(&ctx, &err))
    })
}

/// `ec.entity.isTxCacheActive()`.
pub(super) fn entity_is_tx_cache_active(ctx: Ctx<'_>) -> rquickjs::Result<bool> {
    host_guard(&ctx, || Ok(crate::entity::cache::sim_entered()))
}

/// An entity value as a script sees it.
pub(super) fn entity_to_js<'js>(ctx: &Ctx<'js>, host: HostEntity) -> rquickjs::Result<JsValue<'js>> {
    Class::instance(ctx.clone(), host).and_then(|class| class.into_js(ctx))
}

pub(super) fn wrap_find<'js>(ctx: &Ctx<'js>, find: EntityFind) -> rquickjs::Result<Class<'js, HostFind>> {
    Class::instance(
        ctx.clone(),
        HostFind {
            inner: RefCell::new(Some(find)),
        },
    )
}

pub(super) fn take_find(ctx: &Ctx<'_>, slot: &RefCell<Option<EntityFind>>) -> rquickjs::Result<EntityFind> {
    slot.borrow_mut()
        .take()
        .ok_or_else(|| Exception::throw_message(ctx, "entity find was already used"))
}

/// The integer of a `limit` or `offset` argument, `what`. A number is itself. Text is what an entity-find's
/// `limit="${expr}"` expands to, read as Java's `as Integer`: trimmed and parsed, a whole number in decimal form too.
/// Null, undefined, empty text or text that is not a number is `None`, no limit, logged at debug: the find still runs.
fn find_count(ctx: &Ctx<'_>, what: &str, value: &JsValue<'_>) -> rquickjs::Result<Option<i64>> {
    if value.is_null() || value.is_undefined() {
        return Ok(None);
    }
    let Some(text) = value.as_string() else {
        return js_i64(ctx, value).map(Some);
    };
    let text = text.to_string()?;
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    if let Ok(number) = trimmed.parse::<i64>() {
        return Ok(Some(number));
    }
    #[allow(clippy::cast_possible_truncation)]
    match trimmed.parse::<f64>() {
        Ok(number) if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 => Ok(Some(number as i64)),
        _ => {
            tracing::debug!("entity find {what} {trimmed:?} is not an integer; no {what} applied");
            Ok(None)
        }
    }
}

pub(super) fn prefixed_field(prefix: &str, field: &str) -> String {
    if prefix.is_empty() {
        return field.to_string();
    }
    let mut name = String::with_capacity(prefix.len() + field.len());
    name.push_str(prefix);
    let mut chars = field.chars();
    if let Some(first) = chars.next() {
        name.push(first.to_ascii_uppercase());
        name.push_str(chars.as_str());
    }
    name
}

pub(super) enum FieldInclude {
    All,
    Pk,
    NonPk,
}

pub(super) fn field_include(value: &JsValue<'_>) -> FieldInclude {
    if value.is_null() || value.is_undefined() {
        return FieldInclude::All;
    }
    match value.as_bool() {
        Some(true) => FieldInclude::Pk,
        Some(false) => FieldInclude::NonPk,
        None => FieldInclude::All,
    }
}

pub(super) fn field_prefix(ctx: &Ctx<'_>, value: &JsValue<'_>) -> rquickjs::Result<String> {
    if value.is_null() || value.is_undefined() {
        return Ok(String::new());
    }
    js_string(ctx, value)
}

/// A find builder. Chain methods take the inner find so a second use fails.
#[derive(Trace, JsLifetime)]
#[rquickjs::class]
pub(super) struct HostFind {
    #[qjs(skip_trace)]
    pub(super) inner: RefCell<Option<EntityFind>>,
}

impl HostFind {
    /// A clone of the find this object holds, which stays usable.
    fn peek(&self, ctx: &Ctx<'_>) -> rquickjs::Result<EntityFind> {
        self.inner
            .borrow()
            .as_ref()
            .cloned()
            .ok_or_else(|| Exception::throw_message(ctx, "this find was already run"))
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostFind {
    fn condition<'js>(
        &self,
        ctx: Ctx<'js>,
        first: JsValue<'js>,
        second: Opt<JsValue<'js>>,
        third: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let mut find = take_find(&ctx, &self.inner)?;
            // `condition(field, operator, value)`: the operator is a name Java's factory knows, an empty one is equals.
            if let (Some(operator), Some(value)) = (second.0.as_ref(), third.0.as_ref()) {
                let field = js_string(&ctx, &first)?;
                let operator = if operator.is_null() || operator.is_undefined() {
                    String::new()
                } else {
                    js_string(&ctx, operator)?
                };
                let host = js_to_host(&ctx, value)?;
                let condition = comparison(&field, &operator, host).map_err(|err| throw_lib(&ctx, &err))?;
                return wrap_find(&ctx, find.condition(condition));
            }
            if second.0.is_none() {
                if first.is_null() || first.is_undefined() {
                    return wrap_find(&ctx, find);
                }
                if let Some(condition) = condition_of(&first) {
                    return wrap_find(&ctx, find.condition(condition));
                }
            }
            if let Some(value) = second.0 {
                let field = js_string(&ctx, &first)?;
                let host = js_to_host(&ctx, &value)?;
                let condition = comparison(&field, "equals", host).map_err(|err| throw_lib(&ctx, &err))?;
                return wrap_find(&ctx, find.condition(condition));
            }
            let Some(object) = first.as_object() else {
                return Err(Exception::throw_message(
                    &ctx,
                    "entity condition needs a field and a value",
                ));
            };
            // A map such as `context` carries names that are not fields. Only fields (a view's aliases) apply.
            let facade = facade_of(&ctx)?;
            let known: std::collections::BTreeSet<String> = facade
                .with_entity(find.entity_name(), |entity, _, _| {
                    Ok(entity.field_names().into_iter().map(str::to_string).collect())
                })
                .map_err(|err| throw_lib(&ctx, &err))?;
            for key in object.keys::<String>() {
                let key = key?;
                if !known.contains(&key) {
                    continue;
                }
                let property: JsValue = object.get(key.as_str())?;
                let host = js_to_host(&ctx, &property)?;
                let condition = comparison(&key, "equals", host).map_err(|err| throw_lib(&ctx, &err))?;
                find = find.condition(condition);
            }
            wrap_find(&ctx, find)
        })
    }

    /// Comma separated fields, each with an optional `-` for descending, `+`, or `^` for ignore case.
    fn order_by<'js>(&self, ctx: Ctx<'js>, field: String) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let mut find = take_find(&ctx, &self.inner)?;
            for part in field.split(',') {
                find = find.order_by_text(part);
            }
            wrap_find(&ctx, find)
        })
    }

    /// `limit(count)`: a number, or the text an expanded `limit="${expr}"` of an entity-find gives (see
    /// [`find_count`]). Null, empty or not a number leaves the find without a limit.
    fn limit<'js>(&self, ctx: Ctx<'js>, value: JsValue<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?;
            let find = match find_count(&ctx, "limit", &value)? {
                Some(number) => find.limit(number),
                None => find,
            };
            wrap_find(&ctx, find)
        })
    }

    /// `offset(offset)`, or `offset(pageIndex, pageSize)` for the offset `pageIndex * pageSize`, as Java's. The page
    /// size is not kept: `limit` sets it. The arguments are as `limit` takes them.
    fn offset<'js>(
        &self,
        ctx: Ctx<'js>,
        value: JsValue<'js>,
        page_size: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?;
            let Some(mut number) = find_count(&ctx, "offset", &value)? else {
                return wrap_find(&ctx, find);
            };
            if let Some(size) = page_size.0.as_ref().filter(|size| !size.is_undefined()) {
                if let Some(size) = find_count(&ctx, "page size", size)? {
                    number = number.saturating_mul(size);
                }
            }
            wrap_find(&ctx, find.offset(number))
        })
    }

    /// `conditionDate(fromField, thruField, moment)`: the rows in effect at the moment, or now when it is empty.
    fn condition_date<'js>(
        &self,
        ctx: Ctx<'js>,
        from_field: String,
        thru_field: String,
        moment: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let moment = match moment.0.as_ref() {
                Some(value) => js_to_host(&ctx, value)?,
                None => Value::Null,
            };
            let mut find = take_find(&ctx, &self.inner)?;
            if let Some(condition) = date_range_condition(&from_field, &thru_field, moment, false) {
                find = find.condition(condition);
            }
            wrap_find(&ctx, find)
        })
    }

    /// `getPageIndex()`: `offset / pageSize`, 0 with no offset.
    fn get_page_index<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let find = self.peek(&ctx)?;
            let size = find.limit_value().filter(|limit| *limit != 0).unwrap_or(20);
            i64_to_js(&ctx, find.offset_value().map_or(0, |offset| offset / size))
        })
    }

    /// `getPageSize()`: the limit, 20 with none.
    fn get_page_size<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || i64_to_js(&ctx, self.peek(&ctx)?.limit_value().unwrap_or(20)))
    }

    /// `updateAll(fieldsToSet)`: set the fields on each row found and update the ones that change. Returns how many
    /// rows were updated. The find is not cached, so every row is read and its entity ECA rules run.
    fn update_all<'js>(&self, ctx: Ctx<'js>, fields: JsValue<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let Value::Map(fields) = js_to_host(&ctx, &fields)? else {
                return Err(Exception::throw_message(&ctx, "updateAll needs a map of fields to set"));
            };
            let updated = take_find(&ctx, &self.inner)?
                .update_all(fields)
                .map_err(|err| throw_lib(&ctx, &err))?;
            i64_to_js(&ctx, updated)
        })
    }

    /// `useCache(flag)`: through the entity cache for any entity not marked `never`, as Java's `EntityFind.useCache`. A
    /// null or undefined flag leaves the entity's own setting.
    fn use_cache<'js>(&self, ctx: Ctx<'js>, flag: JsValue<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?;
            let find = if flag.is_null() || flag.is_undefined() {
                find
            } else {
                let on = flag.as_bool().unwrap_or_else(|| {
                    flag.as_string()
                        .and_then(|text| text.to_string().ok())
                        .is_some_and(|text| text == "true")
                });
                find.use_cache(on)
            };
            wrap_find(&ctx, find)
        })
    }

    fn for_update<'js>(&self, ctx: Ctx<'js>, flag: JsValue<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?.for_update(flag.as_bool().unwrap_or(true));
            wrap_find(&ctx, find)
        })
    }

    fn distinct<'js>(&self, ctx: Ctx<'js>, flag: JsValue<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?.distinct(flag.as_bool().unwrap_or(true));
            wrap_find(&ctx, find)
        })
    }

    fn disable_authz<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || wrap_find(&ctx, take_find(&ctx, &self.inner)?.disable_authz()))
    }

    /// `useClone(flag)`: read from a clone datasource of the entity's group when one is open.
    fn use_clone<'js>(&self, ctx: Ctx<'js>, flag: JsValue<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let on = flag.as_bool().unwrap_or_else(|| {
                flag.as_string()
                    .and_then(|text| text.to_string().ok())
                    .is_some_and(|text| text == "true")
            });
            wrap_find(&ctx, take_find(&ctx, &self.inner)?.use_clone(on))
        })
    }

    fn select_field<'js>(&self, ctx: Ctx<'js>, field: String) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?.select_field(&field);
            wrap_find(&ctx, find)
        })
    }

    /// The shown and hidden fields of a form-list, for its `entity-find` (generated code only).
    fn select_form_fields<'js>(&self, ctx: Ctx<'js>, fields: String) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?.select_form_fields(&fields);
            wrap_find(&ctx, find)
        })
    }

    fn list<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostList>> {
        host_guard(&ctx, || {
            let list = take_find(&ctx, &self.inner)?
                .list()
                .map_err(|err| throw_lib(&ctx, &err))?;
            Class::instance(ctx.clone(), HostList::of_list(list))
        })
    }

    fn one<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let found = take_find(&ctx, &self.inner)?
                .one()
                .map_err(|err| throw_lib(&ctx, &err))?;
            match found {
                Some(value) => entity_to_js(&ctx, HostEntity::new(value)),
                None => Null.into_js(&ctx),
            }
        })
    }

    fn one_master<'js>(&self, ctx: Ctx<'js>, master: Opt<String>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            match take_find(&ctx, &self.inner)?
                .one()
                .map_err(|err| throw_lib(&ctx, &err))?
            {
                Some(value) => {
                    let graph = crate::entity::query_rest::master_value(
                        &facade,
                        &value,
                        master.0.as_deref().unwrap_or("default"),
                    )
                    .map_err(|err| throw_lib(&ctx, &err))?;
                    host_to_js(&ctx, graph)
                }
                None => Null.into_js(&ctx),
            }
        })
    }

    fn count<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let number = take_find(&ctx, &self.inner)?
                .count()
                .map_err(|err| throw_lib(&ctx, &err))?;
            i64_to_js(&ctx, number)
        })
    }

    /// `getLimit()`: the limit of the find, or null. Does not use the find up.
    fn get_limit<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || match self.peek(&ctx)?.limit_value() {
            Some(limit) => i64_to_js(&ctx, limit),
            None => Null.into_js(&ctx),
        })
    }

    /// `getOffset()`: the offset of the find, or null. Does not use the find up.
    fn get_offset<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || match self.peek(&ctx)?.offset_value() {
            Some(offset) => i64_to_js(&ctx, offset),
            None => Null.into_js(&ctx),
        })
    }

    /// A copy of the find, so a page's `list()` and the `count()` of all rows can both run.
    fn copy<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = self.peek(&ctx)?;
            wrap_find(&ctx, find)
        })
    }

    /// Delete the rows the find matches. Returns how many, as Java's `deleteAll`.
    fn delete_all<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let deleted = take_find(&ctx, &self.inner)?
                .delete_all()
                .map_err(|err| throw_lib(&ctx, &err))?;
            i64_to_js(&ctx, deleted)
        })
    }

    /// A script reads the whole result. A streaming cursor is the host's `EntityFind::iterator`.
    fn iterator<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<Class<'js, HostList>> {
        self.list(ctx)
    }

    /// A condition on an aggregate alias of a view-entity. A null condition is skipped.
    fn having_condition<'js>(&self, ctx: Ctx<'js>, condition: JsValue<'js>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let mut find = take_find(&ctx, &self.inner)?;
            if let Some(condition) = condition_of(&condition) {
                find = find.having(condition);
            }
            wrap_find(&ctx, find)
        })
    }

    /// `searchFormMap(inputFields, defaultParameters, skipFields, defaultOrderBy, paginate)`.
    /// The defaults and `orderByField` it applied are put in the context, so the form shows them.
    fn search_form_map<'js>(&self, ctx: Ctx<'js>, args: Rest<JsValue<'js>>) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let map_arg = |index: usize| -> rquickjs::Result<std::collections::BTreeMap<String, Value>> {
                match args.0.get(index) {
                    Some(value) if !value.is_null() && !value.is_undefined() => match js_to_host(&ctx, value)? {
                        Value::Map(entries) => Ok(entries.into_iter().collect()),
                        _ => Err(Exception::throw_message(&ctx, "searchFormMap needs maps of fields")),
                    },
                    _ => Ok(std::collections::BTreeMap::new()),
                }
            };
            let text_arg = |index: usize| -> String {
                args.0
                    .get(index)
                    .and_then(|value| js_to_host(&ctx, value).ok())
                    .map(|value| moqui_lib::object::to_plain_string(&value))
                    .unwrap_or_default()
            };
            let input = map_arg(0)?;
            let defaults = map_arg(1)?;
            let paginate = args.0.get(4).and_then(JsValue::as_bool).unwrap_or(false);
            let found = take_find(&ctx, &self.inner)?
                .search_form_map(&input, &defaults, &text_arg(2), &text_arg(3), paginate)
                .map_err(|err| throw_lib(&ctx, &err))?;
            for (name, value) in found.context {
                put_name(&ctx, &name, value)?;
            }
            wrap_find(&ctx, found.find)
        })
    }

    fn require_search_form_parameters<'js>(
        &self,
        ctx: Ctx<'js>,
        require: Opt<bool>,
    ) -> rquickjs::Result<Class<'js, HostFind>> {
        host_guard(&ctx, || {
            let find = take_find(&ctx, &self.inner)?.require_search_form_parameters(require.0.unwrap_or(true));
            wrap_find(&ctx, find)
        })
    }
}

/// The exotic methods `#[rquickjs::class(exotic)]` calls, in the module it names `__impl_exotic_<Class>__`, written
/// here rather than by `#[rquickjs::exotic]` because that macro drops the receiver of a set. A set whose receiver is
/// another object, as `Object.create(value).x = 1` reaches the value through the prototype chain, defines the
/// property on the receiver, as an ordinary object's set does, rather than writing the field. Re-check when rquickjs
/// is upgraded.
macro_rules! exotic_module {
    ($module:ident, $class:ident) => {
        #[allow(non_snake_case)]
        mod $module {
            use super::*;
            use rquickjs::class::JsCell;

            pub(crate) struct ExoticImpl;

            impl ExoticImpl {
                pub fn exotic_get_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                    _receiver: JsValue<'js>,
                ) -> rquickjs::Result<JsValue<'js>> {
                    this.borrow().property(ctx, atom)
                }

                pub fn exotic_set_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                    receiver: JsValue<'js>,
                    value: JsValue<'js>,
                ) -> rquickjs::Result<bool> {
                    if is_receiver(this, &receiver) {
                        this.borrow().assign(ctx, atom, value)
                    } else {
                        define_on_receiver(atom, &receiver, value)
                    }
                }

                pub fn exotic_delete_property<'js>(
                    this: &JsCell<'js, $class>,
                    _ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                ) -> rquickjs::Result<bool> {
                    Ok(this.borrow().remove(atom))
                }

                pub fn exotic_has_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                ) -> rquickjs::Result<bool> {
                    this.borrow().has_member(ctx, atom)
                }

                pub fn exotic_get_own_property<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                    atom: Atom<'js>,
                ) -> rquickjs::Result<Option<PropertyDescriptor<'js>>> {
                    this.borrow().own_property(ctx, atom)
                }

                pub fn exotic_get_own_property_names<'js>(
                    this: &JsCell<'js, $class>,
                    ctx: &Ctx<'js>,
                ) -> rquickjs::Result<Vec<PropertyName<'js>>> {
                    this.borrow().own_names(ctx)
                }
            }
        }
    };
}

/// True when a set's receiver is the exotic object itself, not an object that has it on its prototype chain.
fn is_receiver<'js, C: rquickjs::class::JsClass<'js>>(
    this: &rquickjs::class::JsCell<'js, C>,
    receiver: &JsValue<'js>,
) -> bool {
    receiver
        .as_object()
        .and_then(Class::<C>::from_object)
        .is_some_and(|class| std::ptr::eq(class.get_cell(), this))
}

/// The ordinary set of a property that is not on the receiver: a new own data property of the receiver. A receiver
/// that is not an object takes no property.
fn define_on_receiver<'js>(atom: Atom<'js>, receiver: &JsValue<'js>, value: JsValue<'js>) -> rquickjs::Result<bool> {
    let result = match receiver.as_object() {
        Some(object) => object
            .prop(
                atom,
                rquickjs::object::Property::from(value.clone())
                    .writable()
                    .enumerable()
                    .configurable(),
            )
            .map(|()| true),
        None => Ok(false),
    };
    keep_assigned(value);
    result
}

/// A find result as scripts and the context stack hold it. Each row is its own shared cell, so a field set through
/// `list[0]`, `get`, `for...of` or an `<iterate>` entry is kept in the list, as on a Java `List` of values. The rows are
/// copied from the [`EntityList`] the find returned, so the entity cache's list is not changed.
pub(crate) struct ScriptList {
    cells: Vec<Arc<EntityCell>>,
}

impl ScriptList {
    pub(crate) fn of_list(list: EntityList) -> Self {
        Self::of_cells(
            list.into_values()
                .into_iter()
                .map(|value| Arc::new(Mutex::new(value)))
                .collect(),
        )
    }

    pub(crate) fn of_cells(cells: Vec<Arc<EntityCell>>) -> Self {
        Self { cells }
    }

    pub(crate) fn len(&self) -> usize {
        self.cells.len()
    }

    pub(crate) fn cells(&self) -> &[Arc<EntityCell>] {
        &self.cells
    }

    /// The rows as they are now.
    pub(crate) fn values(&self) -> Vec<EntityValue> {
        self.cells.iter().map(|cell| lock(cell).clone()).collect()
    }
}

/// A find result. `length`, an index and `get` are what iterate and inline scripts use, and it is iterable as a
/// Java `List` is to Groovy. The methods written in JavaScript are added by [`install_entity_prototypes`].
#[derive(Trace, JsLifetime)]
#[rquickjs::class(exotic)]
pub(super) struct HostList {
    #[qjs(skip_trace)]
    pub(super) inner: Arc<ScriptList>,
    /// The `EntityListIterator` cursor of `iterator()`: the current row's index, -1 before the first.
    #[qjs(skip_trace)]
    cursor: std::cell::Cell<i64>,
}

impl HostList {
    pub(super) fn of_list(list: EntityList) -> Self {
        Self {
            inner: Arc::new(ScriptList::of_list(list)),
            cursor: std::cell::Cell::new(-1),
        }
    }

    fn of_cells(cells: Vec<Arc<EntityCell>>) -> Self {
        Self::of_shared(Arc::new(ScriptList::of_cells(cells)))
    }

    /// A list over rows another list holds too.
    pub(super) fn of_shared(inner: Arc<ScriptList>) -> Self {
        Self {
            inner,
            cursor: std::cell::Cell::new(-1),
        }
    }

    /// The row at the cursor after moving it to `index`, or null when `index` is outside the rows (the cursor stops
    /// before the first or after the last).
    fn move_to<'js>(&self, ctx: &Ctx<'js>, index: i64) -> rquickjs::Result<JsValue<'js>> {
        let len = i64::try_from(self.inner.len()).unwrap_or(i64::MAX);
        let index = index.clamp(-1, len);
        self.cursor.set(index);
        if index < 0 || index >= len {
            return Null.into_js(ctx);
        }
        self.item(ctx, usize::try_from(index).unwrap_or(0))
    }

    fn length(&self, ctx: &Ctx<'_>) -> rquickjs::Result<i32> {
        i32::try_from(self.inner.len()).map_err(|_| Exception::throw_message(ctx, "list is too long"))
    }

    /// The position a property name stands for, when it is an index in the list.
    fn index_of(&self, name: &str) -> Option<usize> {
        let index: usize = name.parse().ok()?;
        (index < self.inner.len() && index.to_string() == name).then_some(index)
    }

    /// The row itself, not a copy: a field set on it is kept in the list.
    fn item<'js>(&self, ctx: &Ctx<'js>, index: usize) -> rquickjs::Result<JsValue<'js>> {
        match self.inner.cells().get(index) {
            Some(cell) => entity_to_js(ctx, HostEntity::of(Arc::clone(cell))),
            None => Null.into_js(ctx),
        }
    }
}

exotic_module!(__impl_exotic_HostList__, HostList);

/// Property access on a list: a prototype method first, then `length` and the index of a value.
impl HostList {
    fn property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(ctx, || {
            if let Some(found) = prototype_member::<HostList>(ctx, &atom)? {
                return Ok(found);
            }
            let Some(name) = atom_name(&atom)? else {
                return Undefined.into_js(ctx);
            };
            if name == "length" {
                return self.length(ctx)?.into_js(ctx);
            }
            match self.index_of(&name) {
                Some(index) => self.item(ctx, index),
                None => Undefined.into_js(ctx),
            }
        })
    }

    /// A list is read-only: an assignment is ignored, as on a frozen array.
    fn assign<'js>(&self, _ctx: &Ctx<'js>, _atom: Atom<'js>, value: JsValue<'js>) -> Result<bool, JsError> {
        keep_assigned(value);
        Ok(false)
    }

    fn remove<'js>(&self, _atom: Atom<'js>) -> bool {
        false
    }

    fn has_member<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<bool, JsError> {
        if prototype_has::<HostList>(ctx, &atom)? {
            return Ok(true);
        }
        Ok(atom_name(&atom)?.is_some_and(|name| name == "length" || self.index_of(&name).is_some()))
    }

    fn own_property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<Option<PropertyDescriptor<'js>>, JsError> {
        let Some(name) = atom_name(&atom)? else {
            return Ok(None);
        };
        if name == "length" {
            let length = self.length(ctx)?.into_js(ctx)?;
            return Ok(Some(PropertyDescriptor::new_value(length, false, false, false)));
        }
        match self.index_of(&name) {
            Some(index) => Ok(Some(PropertyDescriptor::new_value(
                self.item(ctx, index)?,
                true,
                true,
                false,
            ))),
            None => Ok(None),
        }
    }

    fn own_names<'js>(&self, ctx: &Ctx<'js>) -> Result<Vec<PropertyName<'js>>, JsError> {
        (0..self.inner.len())
            .map(|index| {
                let index = u32::try_from(index).map_err(|_| Exception::throw_message(ctx, "list is too long"))?;
                Ok(PropertyName {
                    atom: Atom::from_u32(ctx.clone(), index)?,
                    is_enumerable: true,
                })
            })
            .collect()
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostList {
    /// Java's `EntityListIterator.next`: the row after the current one, null past the last.
    fn next<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || self.move_to(&ctx, self.cursor.get() + 1))
    }

    fn has_next(&self) -> bool {
        self.cursor.get() + 1 < i64::try_from(self.inner.len()).unwrap_or(i64::MAX)
    }

    /// Java's `previous`: the row before the current one, null before the first.
    fn previous<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || self.move_to(&ctx, self.cursor.get() - 1))
    }

    fn has_previous(&self) -> bool {
        self.cursor.get() > 0
    }

    /// Java's `absolute`, 1-based; from the end when negative; 0 is before the first. True when on a row.
    fn absolute(&self, row: i32) -> bool {
        let len = i64::try_from(self.inner.len()).unwrap_or(i64::MAX);
        let index = match i64::from(row) {
            0 => -1,
            row if row > 0 => row - 1,
            row => len + row,
        }
        .clamp(-1, len);
        self.cursor.set(index);
        index >= 0 && index < len
    }

    /// Java's `relative`: move `rows` from the current row. True when on a row.
    fn relative(&self, rows: i32) -> bool {
        let len = i64::try_from(self.inner.len()).unwrap_or(i64::MAX);
        let index = (self.cursor.get() + i64::from(rows)).clamp(-1, len);
        self.cursor.set(index);
        index >= 0 && index < len
    }

    /// Java's `currentIndex`: the current row, 1-based; 0 before the first.
    fn current_index(&self) -> i64 {
        self.cursor.get() + 1
    }

    fn first<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || self.move_to(&ctx, 0))
    }

    fn last<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let len = i64::try_from(self.inner.len()).unwrap_or(i64::MAX);
            self.move_to(&ctx, len - 1)
        })
    }

    fn before_first(&self) {
        self.cursor.set(-1);
    }

    fn after_last(&self) {
        self.cursor.set(i64::try_from(self.inner.len()).unwrap_or(i64::MAX));
    }

    /// Java's `getCompleteList`: every row.
    fn get_complete_list<'js>(&self, ctx: Ctx<'js>, _close_after: Opt<bool>) -> rquickjs::Result<Class<'js, HostList>> {
        host_guard(&ctx, || {
            Class::instance(ctx.clone(), HostList::of_cells(self.inner.cells().to_vec()))
        })
    }

    /// Java's `setFetchSize` and `close`: the rows are already here.
    fn set_fetch_size(&self, _rows: i32) {}

    fn close(&self) {}

    fn get<'js>(&self, ctx: Ctx<'js>, index: i32) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            if index < 0 {
                return Null.into_js(&ctx);
            }
            let index =
                usize::try_from(index).map_err(|_| Exception::throw_message(&ctx, "list index is too large"))?;
            self.item(&ctx, index)
        })
    }

    fn get_partial_list<'js>(
        &self,
        ctx: Ctx<'js>,
        offset: i32,
        size: i32,
        _count: Opt<bool>,
    ) -> rquickjs::Result<Class<'js, HostList>> {
        host_guard(&ctx, || {
            let start = usize::try_from(offset.max(0)).unwrap_or(0);
            let take = usize::try_from(size.max(0)).unwrap_or(0);
            // The part shares the rows, as a Java sublist shares its elements.
            let cells = self.inner.cells().iter().skip(start).take(take).cloned().collect();
            Class::instance(ctx.clone(), HostList::of_cells(cells))
        })
    }

    fn filter_by_date<'js>(
        &self,
        ctx: Ctx<'js>,
        from_name: String,
        thru_name: String,
        valid: JsValue<'js>,
        ignore_if_empty: Opt<bool>,
    ) -> rquickjs::Result<Class<'js, HostList>> {
        host_guard(&ctx, || {
            let valid = js_to_host(&ctx, &valid)?;
            let empty = moqui_lib::object::is_empty(&valid);
            let cells = if empty && ignore_if_empty.0.unwrap_or(false) {
                self.inner.cells().to_vec()
            } else {
                let valid = if empty { now_timestamp() } else { timestamp_of(&valid) };
                self.inner
                    .cells()
                    .iter()
                    .filter(|cell| {
                        let row = lock(cell);
                        let from = timestamp_of(row.get(&from_name).unwrap_or(&Value::Null));
                        let thru = timestamp_of(row.get(&thru_name).unwrap_or(&Value::Null));
                        let started = from.is_null()
                            || moqui_lib::object::compare_obj(&from, &valid)
                                .is_ok_and(|ordering| ordering != Ordering::Greater);
                        let open = thru.is_null()
                            || moqui_lib::object::compare_obj(&thru, &valid)
                                .is_ok_and(|ordering| ordering == Ordering::Greater);
                        started && open
                    })
                    .cloned()
                    .collect()
            };
            Class::instance(ctx.clone(), HostList::of_cells(cells))
        })
    }
}

/// One entity value. Field storage stays on this object, not on the context stack.
///
/// Fields read and write as properties too, as Groovy's map property syntax does over Java `EntityValueBase`.
#[derive(Trace, JsLifetime)]
#[rquickjs::class(exotic)]
pub(super) struct HostEntity {
    #[qjs(skip_trace)]
    pub(super) inner: Arc<EntityCell>,
}

/// An entity value a script and the context stack share by reference.
pub(super) type EntityCell = Mutex<EntityValue>;

impl HostEntity {
    pub(super) fn new(value: EntityValue) -> Self {
        Self::of(Arc::new(Mutex::new(value)))
    }

    pub(super) fn of(inner: Arc<EntityCell>) -> Self {
        Self { inner }
    }

    fn val(&self) -> MutexGuard<'_, EntityValue> {
        lock(&self.inner)
    }

    /// A copy of the value, for a write: the write runs entity ECA rules, and a rule that reads this same value
    /// must not find the cell locked. The writes take the value by reference and set nothing on it.
    fn snapshot(&self) -> EntityValue {
        self.val().clone()
    }

    /// Run `write` on a [`Self::snapshot`]; after it, the value holds what the database does, so a later update writes
    /// only the fields set after this write.
    fn write(
        &self,
        ctx: &Ctx<'_>,
        write: impl FnOnce(&EntityValue, &EntityFacade) -> Result<(), moqui_lib::error::Error>,
    ) -> rquickjs::Result<()> {
        let facade = facade_of(ctx)?;
        let written = self.snapshot();
        write(&written, &facade).map_err(|err| throw_lib(ctx, &err))?;
        self.val().note_written(&written);
        Ok(())
    }

    /// What `name` is on the entity of this value, or None when no entity definition can be reached.
    fn member(&self, name: &str) -> Option<Member> {
        let facade = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten()?;
        let entity_name = self.val().entity_name().to_string();
        facade
            .with_entity(&entity_name, |entity, _, _| {
                if entity.has_field(name) {
                    return Ok(Member::Field);
                }
                Ok(match entity.relationship(name) {
                    Some(relationship) if relationship.relationship_type.starts_with("one") => Member::One,
                    Some(_) => Member::Many,
                    None => Member::Unknown,
                })
            })
            .ok()
    }

    /// A field as a script reads it, in the user's locale when it is a localized field, and a view's `pq-expression`
    /// alias evaluated, as Java's `get`. A lookup or an expression runs with the value unlocked: a lookup is a find, and
    /// an expression may read this same value.
    fn field_value(&self, name: &str) -> Option<Value> {
        let value = self.val();
        if !value.localizes(name) {
            return value.get(name).cloned();
        }
        let Some(facade) = ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() else {
            return value.get(name).cloned();
        };
        let read = value.read_localized(&facade, name);
        drop(value);
        match read {
            Ok(found) => found,
            Err(read) => read.run(&facade),
        }
    }

    /// [`Self::field_value`], except for a `pq-expression` alias with the name of a relationship: as Java's `get`, the
    /// relationship comes first, so that is None.
    fn held_value(&self, name: &str) -> Option<Value> {
        let pq_alias = self.val().is_pq_alias(name);
        if pq_alias && matches!(self.member(name), Some(Member::One | Member::Many)) {
            return None;
        }
        self.field_value(name)
    }

    /// The fields as a plain map, the localized ones in the user's locale, with a view's `pq-expression` aliases, as
    /// Java's `getMap`.
    fn field_map(&self) -> std::collections::BTreeMap<String, Value> {
        let value = self.val();
        if !value.has_l10n() {
            return crate::entity::string_map(value.fields());
        }
        let copy = value.clone();
        drop(value);
        match ExecutionContext::with_current(|ec| ec.factory().entity()).flatten() {
            Some(facade) => crate::entity::string_map(&copy.localized_fields(&facade)),
            None => crate::entity::string_map(copy.fields()),
        }
    }

    /// Whether `name` is a field: one the value holds, or one its entity defines.
    fn is_field(&self, name: &str) -> bool {
        self.val().fields().contains_key(name) || matches!(self.member(name), Some(Member::Field))
    }

    /// Set a field, as `put` in Java: a name that is not a field of the entity is an error. When no entity
    /// definition can be reached, any name is set.
    fn set_field(&self, ctx: &Ctx<'_>, field: String, value: &JsValue<'_>) -> rquickjs::Result<()> {
        let host = js_to_host(ctx, value)?;
        let known = self.val().fields().contains_key(field.as_str());
        if !known
            && self
                .member(&field)
                .is_some_and(|member| !matches!(member, Member::Field))
        {
            let entity_name = self.val().entity_name().to_string();
            return Err(Exception::throw_message(
                ctx,
                &format!("The field name {field} is not valid for entity {entity_name}"),
            ));
        }
        self.val().set(field, host);
        Ok(())
    }

    /// A field value, null for a field the value does not hold, or the related value or list for a relationship
    /// name, as Java `EntityValueBase.get` resolves a name. Undefined for any other name.
    fn read_member<'js>(&self, ctx: &Ctx<'js>, name: &str) -> rquickjs::Result<JsValue<'js>> {
        let held = self.held_value(name);
        if let Some(value) = held {
            return host_to_js(ctx, value);
        }
        match self.member(name) {
            Some(Member::Field) => Null.into_js(ctx),
            Some(Member::One) => match self
                .related_find(ctx, name)?
                .one()
                .map_err(|err| throw_lib(ctx, &err))?
            {
                Some(value) => entity_to_js(ctx, HostEntity::new(value)),
                None => Null.into_js(ctx),
            },
            Some(Member::Many) => {
                let list = self
                    .related_find(ctx, name)?
                    .list()
                    .map_err(|err| throw_lib(ctx, &err))?;
                Class::instance(ctx.clone(), HostList::of_list(list)).and_then(|class| class.into_js(ctx))
            }
            Some(Member::Unknown) | None => Undefined.into_js(ctx),
        }
    }
}

exotic_module!(__impl_exotic_HostEntity__, HostEntity);

/// What a name that is not a method stands for on an entity value.
enum Member {
    Field,
    One,
    Many,
    Unknown,
}

/// Property access on an entity value. A prototype method wins over a field of the same name; a field name reads and
/// writes the field; a relationship name reads the related value, or list for a `many` relationship. Any other name
/// reads as undefined, where Java throws, because the engine and scripts look up names a value does not have
/// (`toJSON`, `then`, `Symbol.toPrimitive`); writing one is an error, as in Java.
impl HostEntity {
    fn property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(ctx, || {
            if let Some(found) = prototype_member::<HostEntity>(ctx, &atom)? {
                return Ok(found);
            }
            match atom_name(&atom)? {
                Some(name) => self.read_member(ctx, &name),
                None => Undefined.into_js(ctx),
            }
        })
    }

    fn assign<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>, value: JsValue<'js>) -> Result<bool, JsError> {
        let result = host_guard(ctx, || {
            let Some(name) = atom_name(&atom)? else {
                return Err(Exception::throw_message(
                    ctx,
                    "an entity value field name is not a symbol",
                ));
            };
            self.set_field(ctx, name, &value)?;
            Ok(true)
        });
        keep_assigned(value);
        result
    }

    /// A field is not removed from a value, as Java `EntityValueBase` has no remove.
    fn remove<'js>(&self, _atom: Atom<'js>) -> bool {
        false
    }

    /// `name in value`: a prototype member or a field of the entity.
    fn has_member<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<bool, JsError> {
        if prototype_has::<HostEntity>(ctx, &atom)? {
            return Ok(true);
        }
        Ok(atom_name(&atom)?.is_some_and(|name| self.is_field(&name)))
    }

    fn own_property<'js>(&self, ctx: &Ctx<'js>, atom: Atom<'js>) -> Result<Option<PropertyDescriptor<'js>>, JsError> {
        let Some(name) = atom_name(&atom)? else {
            return Ok(None);
        };
        if !self.is_field(&name) {
            return Ok(None);
        }
        let held = self.field_value(&name).unwrap_or(Value::Null);
        let value = host_to_js(ctx, held)?;
        Ok(Some(PropertyDescriptor::new_value(value, true, true, true)))
    }

    /// The fields the value holds, so `Object.keys`, `for...in`, spread and `JSON.stringify` see the field map.
    fn own_names<'js>(&self, ctx: &Ctx<'js>) -> Result<Vec<PropertyName<'js>>, JsError> {
        let names: Vec<String> = self.val().fields().keys().map(|name| name.to_string()).collect();
        names
            .iter()
            .map(|name| {
                Ok(PropertyName {
                    atom: Atom::from_str(ctx.clone(), name)?,
                    is_enumerable: true,
                })
            })
            .collect()
    }
}

#[rquickjs::methods(rename_all = "camelCase")]
impl HostEntity {
    /// `get(name)`: a field, or as Java's `EntityValue.get` the related value or list for a relationship name.
    fn get<'js>(&self, ctx: Ctx<'js>, field: String) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            // the held value is cloned so the value is not locked while a relationship find runs
            let held = self.held_value(&field);
            if let Some(value) = held {
                return value_to_js(&ctx, &value);
            }
            let found = self.read_member(&ctx, &field)?;
            if found.is_undefined() {
                Null.into_js(&ctx)
            } else {
                Ok(found)
            }
        })
    }

    fn resolve_entity_name(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        self.get_entity_name(ctx)
    }

    fn get_entity_name(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || Ok(self.val().entity_name().to_string()))
    }

    /// `getEntityNamePretty()` (Java's `resolveEntityNamePretty`): the entity's short name in words, `OrderPart` as
    /// `Order Part`.
    #[qjs(rename = "getEntityNamePretty")]
    fn entity_name_pretty(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            let name = self.val().entity_name().to_string();
            facade
                .with_entity(&name, |entity, _, _| {
                    Ok(moqui_lib::string::camel_case_to_pretty(&entity.entity_name))
                })
                .map_err(|err| throw_lib(&ctx, &err))
        })
    }

    #[qjs(rename = "resolveEntityNamePretty")]
    fn resolve_entity_name_pretty(&self, ctx: Ctx<'_>) -> rquickjs::Result<String> {
        self.entity_name_pretty(ctx)
    }

    /// `refresh()`: read the row of this value's primary key again and replace the fields with it. False, and the
    /// value as it was, when there is no such row.
    fn refresh(&self, ctx: Ctx<'_>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            let current = self.snapshot();
            let keys = facade
                .pk_map(current.entity_name(), current.fields())
                .map_err(|err| throw_lib(&ctx, &err))?;
            match facade
                .find_by_pk(current.entity_name(), &keys)
                .map_err(|err| throw_lib(&ctx, &err))?
            {
                Some(fresh) => {
                    *self.val() = fresh;
                    Ok(true)
                }
                None => Ok(false),
            }
        })
    }

    /// `entrySet()`: each field as an entry with `key` and `value`, and `getKey()` and `getValue()`.
    fn entry_set<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let entries: Vec<(String, Value)> = self.field_map().into_iter().collect();
            let list = rquickjs::Array::new(ctx.clone())?;
            for (index, (name, value)) in entries.into_iter().enumerate() {
                let entry = Object::new(ctx.clone())?;
                let key = host_to_js(&ctx, Value::string(&name))?;
                let held = host_to_js(&ctx, value)?;
                entry.set("key", key.clone())?;
                entry.set("value", held.clone())?;
                entry.set("getKey", Function::new(ctx.clone(), move || key.clone()))?;
                entry.set("getValue", Function::new(ctx.clone(), move || held.clone()))?;
                list.set(index, entry)?;
            }
            Ok(list.into_value())
        })
    }

    /// `findRelatedFk(skipEntities)`: every row of the entities that hold a foreign key to this value (a `many`
    /// relationship whose reverse is a `one` of the related entity), as one list. The names to skip are full names,
    /// short aliases, or entity names, in an array or a set.
    fn find_related_fk<'js>(&self, ctx: Ctx<'js>, skip: Opt<JsValue<'js>>) -> rquickjs::Result<Class<'js, HostList>> {
        host_guard(&ctx, || {
            let skip: Vec<String> = match skip
                .0
                .as_ref()
                .filter(|value| !value.is_null() && !value.is_undefined())
            {
                Some(value) => host_texts(js_to_host(&ctx, value)?),
                None => Vec::new(),
            };
            let facade = facade_of(&ctx)?;
            let parent = self.val().entity_name().to_string();
            let names: Vec<String> = facade
                .with_entity(&parent, |entity, _, model| {
                    let mut names = Vec::new();
                    for relationship in &entity.relationships {
                        let Some(related) = model.entity_full(&relationship.related) else {
                            continue;
                        };
                        // The reverse: a one relationship of the related entity back to this one, with the same title.
                        let reverse_is_one = related.relationships.iter().any(|back| {
                            back.relationship_type.starts_with("one")
                                && back.title == relationship.title
                                && (back.related == entity.full_name || back.related_entity_name == entity.entity_name)
                        });
                        let skipped = skip.iter().any(|name| {
                            *name == related.full_name
                                || *name == related.entity_name
                                || related.short_alias.as_deref() == Some(name.as_str())
                        });
                        if reverse_is_one && !skipped {
                            names.push(relationship.name.clone());
                        }
                    }
                    Ok(names)
                })
                .map_err(|err| throw_lib(&ctx, &err))?;
            let mut rows = Vec::new();
            for name in names {
                let list = self
                    .related_find(&ctx, &name)?
                    .list()
                    .map_err(|err| throw_lib(&ctx, &err))?;
                rows.extend(list.into_values());
            }
            Class::instance(ctx.clone(), HostList::of_list(EntityList::new(rows)))
        })
    }

    /// `cloneValue()`: a separate value with the same entity and fields, as Java's `EntityValue.cloneValue`. Changes to
    /// either do not reach the other.
    fn clone_value<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let copy = self.val().clone();
            entity_to_js(&ctx, HostEntity::new(copy))
        })
    }

    /// `equals(other)`: the same entity with the same field values, as Java's `EntityValueBase.equals`; a script's `==`
    /// on two values is identity, so a service that asks whether an edit changed anything calls this.
    fn equals<'js>(&self, ctx: Ctx<'js>, other: JsValue<'js>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || {
            let other = js_to_host(&ctx, &other)?;
            let (Some(name), Some(theirs)) = (
                super::values::value_entity_name(&other),
                super::values::value_map_stored(&other),
            ) else {
                return Ok(false);
            };
            let mine = self.val();
            if mine.entity_name() != name {
                return Ok(false);
            }
            let fields = mine.fields();
            let comparable = |value: &Value| -> String {
                if value.is_null() {
                    String::new()
                } else {
                    moqui_lib::object::to_plain_string(value)
                }
            };
            let same = |name: &str| {
                comparable(fields.get(name).unwrap_or(&Value::Null))
                    == comparable(theirs.get(name).unwrap_or(&Value::Null))
            };
            Ok(fields.keys().all(|name| same(name)) && theirs.keys().all(|name| same(name)))
        })
    }

    /// `getMap()`: the fields as a plain map, the localized ones in the user's locale.
    fn get_map<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let map: indexmap::IndexMap<String, Value> = self.field_map().into_iter().collect();
            host_to_js(&ctx, Value::Map(map))
        })
    }

    fn set(&self, ctx: Ctx<'_>, field: String, value: JsValue<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || self.set_field(&ctx, field, &value))
    }

    fn set_fields(
        &self,
        ctx: Ctx<'_>,
        map: JsValue<'_>,
        set_if_empty: Opt<bool>,
        prefix: JsValue<'_>,
        include: JsValue<'_>,
    ) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let set_empty = set_if_empty.0.unwrap_or(false);
            let prefix = field_prefix(&ctx, &prefix)?;
            let include = field_include(&include);
            let Some(object) = map.as_object() else {
                return Err(Exception::throw_message(&ctx, "setFields needs an object"));
            };
            let facade = facade_of(&ctx)?;
            let entity_name = self.val().entity_name().to_string();
            let specs = facade
                .with_entity(&entity_name, |entity, _, _| {
                    Ok(entity
                        .fields
                        .iter()
                        .map(|field| (field.name.clone(), field.is_pk))
                        .collect::<Vec<_>>())
                })
                .map_err(|err| throw_lib(&ctx, &err))?;
            for (name, is_pk) in specs {
                let keep = match include {
                    FieldInclude::All => true,
                    FieldInclude::Pk => is_pk,
                    FieldInclude::NonPk => !is_pk,
                };
                if !keep {
                    continue;
                }
                let key = prefixed_field(&prefix, &name);
                if !object.contains_key(key.as_str())? {
                    continue;
                }
                let property: JsValue = object.get(key.as_str())?;
                let host = js_to_host(&ctx, &property)?;
                let empty = host.is_null() || matches!(&host, Value::String(text) if text.is_empty());
                if !set_empty && empty {
                    continue;
                }
                self.val().set(name, host);
            }
            Ok(())
        })
    }

    fn create(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || self.write(&ctx, |value, facade| value.create(facade)))
    }

    /// `update()`: write the fields that changed since the value was read or last written, as Java's `update`, so a
    /// field another transaction changed in between is left as it wrote it.
    fn update(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || self.write(&ctx, |value, facade| value.update(facade)))
    }

    fn delete(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            self.snapshot().delete(&facade).map_err(|err| throw_lib(&ctx, &err))
        })
    }

    fn create_or_update(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            self.write(&ctx, |value, facade| value.create_or_update(facade))
        })
    }

    /// `getPrimaryKeys()`: the primary key fields of the value, as a map.
    fn get_primary_keys<'js>(&self, ctx: Ctx<'js>) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            let value = self.val();
            let names: Vec<String> = facade
                .with_entity(value.entity_name(), |entity, _, _| {
                    Ok(entity.pk_field_names().iter().map(|name| (*name).to_string()).collect())
                })
                .map_err(|err| throw_lib(&ctx, &err))?;
            let map: indexmap::IndexMap<String, Value> = names
                .into_iter()
                .map(|name| {
                    let field = value.get(&name).cloned().unwrap_or(Value::Null);
                    (name, field)
                })
                .collect();
            host_to_js(&ctx, Value::Map(map))
        })
    }

    fn set_sequenced_id_primary(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            self.val()
                .set_sequenced_id_primary(&facade)
                .map_err(|err| throw_lib(&ctx, &err))
        })
    }

    fn set_sequenced_id_secondary(&self, ctx: Ctx<'_>) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            self.val()
                .set_sequenced_id_secondary(&facade)
                .map_err(|err| throw_lib(&ctx, &err))
        })
    }

    fn find_related<'js>(
        &self,
        ctx: Ctx<'js>,
        name: String,
        map: Opt<JsValue<'js>>,
        order: Opt<JsValue<'js>>,
        _cache: Opt<JsValue<'js>>,
        for_update: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<Class<'js, HostList>> {
        host_guard(&ctx, || {
            let lock = for_update.0.as_ref().and_then(JsValue::as_bool) == Some(true);
            let mut find = self.related_find(&ctx, &name)?.for_update(lock);
            if let Some(Value::Map(entries)) = map.0.as_ref().map(|value| js_to_host(&ctx, value)).transpose()? {
                for (key, value) in entries {
                    find = find.eq(&key, value);
                }
            }
            if let Some(Value::List(fields)) = order.0.as_ref().map(|value| js_to_host(&ctx, value)).transpose()? {
                for field in fields {
                    find = find.order_by_text(&moqui_lib::object::to_plain_string(&field));
                }
            }
            let list = find.list().map_err(|err| throw_lib(&ctx, &err))?;
            Class::instance(ctx.clone(), HostList::of_list(list))
        })
    }

    fn find_related_one<'js>(
        &self,
        ctx: Ctx<'js>,
        name: String,
        _cache: Opt<JsValue<'js>>,
        for_update: Opt<JsValue<'js>>,
    ) -> rquickjs::Result<JsValue<'js>> {
        host_guard(&ctx, || {
            let lock = for_update.0.as_ref().and_then(JsValue::as_bool) == Some(true);
            let found = self
                .related_find(&ctx, &name)?
                .for_update(lock)
                .one()
                .map_err(|err| throw_lib(&ctx, &err))?;
            match found {
                Some(value) => entity_to_js(&ctx, HostEntity::new(value)),
                None => Null.into_js(&ctx),
            }
        })
    }

    /// `deleteWithRelated(relationships)`: delete the rows of the named `many` relationships (relationship names or short
    /// aliases), then this value. When rows of any other entity still hold a foreign key to this value nothing is
    /// deleted and the result is false, so a caller can report that the value is in use.
    fn delete_with_related<'js>(&self, ctx: Ctx<'js>, relationships: Opt<JsValue<'js>>) -> rquickjs::Result<bool> {
        host_guard(&ctx, || {
            let wanted: Vec<String> = match relationships
                .0
                .as_ref()
                .filter(|value| !value.is_null() && !value.is_undefined())
            {
                Some(value) => host_texts(js_to_host(&ctx, value)?),
                None => Vec::new(),
            };
            let facade = facade_of(&ctx)?;
            let parent = self.val().entity_name().to_string();
            // The relationships to delete through: the named ones that lead to many rows.
            let chosen: Vec<(String, String)> = facade
                .with_entity(&parent, |entity, _, _| {
                    Ok(entity
                        .relationships
                        .iter()
                        .filter(|relationship| {
                            relationship.relationship_type.starts_with("many")
                                && wanted.iter().any(|name| relationship.matches(name))
                        })
                        .map(|relationship| (relationship.name.clone(), relationship.related.clone()))
                        .collect())
                })
                .map_err(|err| throw_lib(&ctx, &err))?;
            let skip = rquickjs::Array::new(ctx.clone())?;
            for (index, (_, related)) in chosen.iter().enumerate() {
                skip.set(index, related.clone())?;
            }
            let others = self.find_related_fk(ctx.clone(), Opt(Some(skip.into_value())))?;
            if others.borrow().inner.len() > 0 {
                return Ok(false);
            }
            let finds = chosen
                .iter()
                .map(|(name, _)| self.related_find(&ctx, name))
                .collect::<rquickjs::Result<Vec<_>>>()?;
            // One transaction around the lot, so a failure part way leaves nothing deleted; it joins the caller's.
            let parent_value = self.snapshot();
            facade
                .transactions_for(&parent)
                .and_then(|tx| {
                    tx.run(crate::transaction::TransactionAttribute::UseOrBegin, None, || {
                        for find in finds {
                            for value in find.list()?.values() {
                                value.delete(&facade)?;
                            }
                        }
                        parent_value.delete(&facade)
                    })
                })
                .map_err(|err| throw_lib(&ctx, &err))?;
            Ok(true)
        })
    }

    fn delete_related(&self, ctx: Ctx<'_>, name: String) -> rquickjs::Result<()> {
        host_guard(&ctx, || {
            let facade = facade_of(&ctx)?;
            let list = self
                .related_find(&ctx, &name)?
                .list()
                .map_err(|err| throw_lib(&ctx, &err))?;
            for value in list.values() {
                value.delete(&facade).map_err(|err| throw_lib(&ctx, &err))?;
            }
            Ok(())
        })
    }
}

impl HostEntity {
    fn related_find(&self, ctx: &Ctx<'_>, relationship: &str) -> rquickjs::Result<EntityFind> {
        let facade = facade_of(ctx)?;
        let record = self.val().clone();
        let (entity, conditions) =
            crate::service::call::related_conditions(&facade, record.entity_name(), relationship, &record)
                .map_err(|err| throw_lib(ctx, &err))?;
        let mut find = facade.find(&entity).map_err(|err| throw_lib(ctx, &err))?;
        for (field, value) in conditions {
            find = find.eq(&field, value);
        }
        Ok(find)
    }
}

/// A member of the class prototype or of `Object.prototype`. An exotic class's own get runs instead of the
/// prototype lookup, so it looks there itself first.
pub(super) fn prototype_member<'js, C: rquickjs::class::JsClass<'js>>(
    ctx: &Ctx<'js>,
    atom: &Atom<'js>,
) -> rquickjs::Result<Option<JsValue<'js>>> {
    let Some(prototype) = Class::<C>::prototype(ctx)? else {
        return Ok(None);
    };
    let found: JsValue = prototype.get(atom.clone())?;
    Ok((!found.is_undefined()).then_some(found))
}

pub(super) fn prototype_has<'js, C: rquickjs::class::JsClass<'js>>(
    ctx: &Ctx<'js>,
    atom: &Atom<'js>,
) -> Result<bool, JsError> {
    match Class::<C>::prototype(ctx)? {
        Some(prototype) => prototype.contains_key(atom.clone()),
        None => Ok(false),
    }
}

/// Hand back the reference to an assigned value that the exotic set took. rquickjs 0.14 wraps the value QuickJS
/// passes to `set_property` as an owned value, and QuickJS frees it again after the call, so without this each
/// assignment would free the value twice. Re-check when rquickjs is upgraded: once the wrapper borrows the value,
/// this would keep one reference too many (a leak, not a crash). The value is not forgotten whole: it holds a
/// reference to the context too.
pub(super) fn keep_assigned(value: JsValue<'_>) {
    // SAFETY: the context and the value are live for this call; this adds one reference to the value, which the
    // free QuickJS does after the exotic set takes back.
    unsafe {
        rquickjs::qjs::JS_DupValue(value.ctx().as_raw().as_ptr(), value.as_raw());
    }
}

/// The text of a property name, or None for a symbol.
pub(super) fn atom_name(atom: &Atom<'_>) -> rquickjs::Result<Option<String>> {
    if atom.to_value()?.is_symbol() {
        return Ok(None);
    }
    atom.to_string().map(Some)
}

/// Members of an entity list that are simplest in JavaScript: iteration, Groovy's `size()` and `isEmpty()`, and the
/// array methods the type declarations name. Not enumerable, as the Rust methods are not.
const LIST_PROTOTYPE_JS: &str = r#"(function (list) {
    const define = (name, value) => Object.defineProperty(list, name, { value, writable: true, configurable: true });
    define(Symbol.iterator, function* () { for (let i = 0; i < this.length; i++) yield this.get(i); });
    define("size", function () { return this.length; });
    define("isEmpty", function () { return this.length === 0; });
    define("map", function (callback, self) { return Array.from(this).map(callback, self); });
    define("filter", function (callback, self) { return Array.from(this).filter(callback, self); });
    define("forEach", function (callback, self) { Array.from(this).forEach(callback, self); });
    define("toJSON", function () { return Array.from(this); });
    // The rest of the Array prototype runs on an array copy of the rows. The rows are the same values, so a field set
    // on one is kept in the list; a method that reorders (`sort`, `reverse`) returns the reordered copy and leaves the
    // list as it was.
    for (const name of ["find", "findIndex", "findLast", "findLastIndex", "some", "every", "reduce", "reduceRight",
            "sort", "reverse", "includes", "indexOf", "lastIndexOf", "slice", "concat", "join", "flatMap", "flat",
            "entries", "keys", "values", "at", "toSorted", "toReversed"]) {
        define(name, function (...args) { return Array.prototype[name].apply(Array.from(this), args); });
    }
})"#;

/// Add the JavaScript members of the entity list prototype. Lists made after this share it.
pub(super) fn install_entity_prototypes(ctx: &Ctx<'_>) -> Result<(), Error> {
    let list = caught(ctx, "entity list prototype", Class::<HostList>::prototype(ctx))?
        .ok_or_else(|| Error::new("entity list has no prototype"))?;
    let fill: Function = eval_js(ctx, LIST_PROTOTYPE_JS)?;
    caught(ctx, "entity list prototype", fill.call::<_, ()>((list,)))
}

#[cfg(test)]
mod tests {
    use crate::context::{ExecutionContext, ExecutionContextFactory};
    use crate::script::template::{render_trusted, Escape};
    use crate::script::{clear_context_stack, eval_condition, run_script};

    fn holds(expression: &str) {
        assert!(eval_condition(expression).unwrap(), "{expression}");
    }

    /// A context with the in-memory database: types T1 and T2, and enumerations E1 to E3 of T1.
    fn enumerations() -> ExecutionContext {
        clear_context_stack();
        let context = ExecutionContextFactory::test_context();
        context.factory().set_entity(crate::testing::open());
        context
    }

    fn seed() {
        run_script(
            r#"for (const [id, text] of [["T1", "Type 1"], ["T2", "Type 2"]]) {
                const type = ec.entity.makeValue("moqui.basic.EnumerationType");
                type.enumTypeId = id;
                type.description = text;
                type.create();
            }
            for (const [id, text, n] of [["E2", "second", 2], ["E1", "first", 1], ["E3", "third", 3]]) {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = id;
                value.enumTypeId = "T1";
                value.description = text;
                value.sequenceNum = n;
                value.create();
            }"#,
        )
        .unwrap();
    }

    #[test]
    fn tx_cache_db_refuses_flush_mode_and_needs_an_overlay() {
        let context = enumerations();
        let _entered = context.enter();
        holds("ec.entity.isTxCacheActive() === false");
        let flush = run_script("ec.entity.startTxCacheDb(false)").unwrap_err();
        assert!(flush.to_string().contains("no longer supported"), "{flush}");
        let none = run_script("ec.entity.startTxCacheDb(true)").unwrap_err();
        assert!(none.to_string().contains("not configured"), "{none}");
        holds("ec.entity.isTxCacheActive() === false");
        run_script("ec.entity.stopTxCache()").unwrap();
    }

    #[test]
    fn a_field_reads_and_writes_as_a_property() {
        let context = enumerations();
        let _entered = context.enter();
        holds(
            r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = "X1";
                value["description"] = "by index";
                value.sequenceNum = 4;
                return value.get("enumId") === "X1" && value.description === "by index" && value.sequenceNum === 4
                    && value.sequenceNum === value.get("sequenceNum");
            })()"#,
        );
        // A field the value does not hold is null, and optional chaining goes through it.
        holds(
            r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.set("enumCode", "C");
                return value.parentEnumId === null && value.parentEnumId?.length === undefined && value.enumCode === "C";
            })()"#,
        );
    }

    /// As Java's `get` and `getMap`: a script reads a localized field in the user's locale; the stored value is what
    /// an update writes.
    #[test]
    fn a_script_reads_localized_fields_and_writes_the_stored_ones() {
        let context = enumerations();
        let _entered = context.enter();
        let _authz = context.disable_authz();
        seed();
        run_script(
            r#"const message = ec.entity.makeValue("moqui.basic.LocalizedMessage");
            message.original = "first"; message.locale = "fr"; message.localized = "premier";
            message.create();"#,
        )
        .unwrap();
        crate::user::session::set_locale(Some("fr"));
        holds(
            r#"(function () {
                const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one();
                const map = value.getMap();
                const json = JSON.parse(JSON.stringify(value));
                value.sequenceNum = 9;
                value.update();
                const stored = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").useCache(false).one();
                return value.description === "premier" && value.get("description") === "premier"
                    && map.description === "premier" && json.description === "premier"
                    && stored.sequenceNum === 9 && stored.description === "premier";
            })()"#,
        );
        crate::user::session::set_locale(Some("de"));
        holds(
            r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").useCache(false).one().description
                === "first""#,
        );
        crate::user::session::set_locale(None);
    }

    /// The allocations the outermost script runtime holds now, after a collection. Not while a script runs: that
    /// holds the runtime's lock.
    fn live_allocations() -> i64 {
        let engine = super::super::ENGINES
            .with(|engines| engines.borrow().first().cloned())
            .expect("a started engine");
        engine.runtime.run_gc();
        engine.runtime.memory_usage().malloc_count
    }

    /// `keep_assigned` matches what rquickjs does with an assigned value: an extra reference each time would keep
    /// every assigned string alive.
    #[test]
    fn a_field_assigned_many_times_does_not_grow_the_runtime() {
        let context = enumerations();
        let _entered = context.enter();
        let assign = |count: u32| {
            run_script(&format!(
                r#"(function () {{
                    const value = ec.entity.makeValue("moqui.basic.Enumeration");
                    for (let i = 0; i < {count}; i++) {{ value.description = "text " + i; }}
                }})()"#
            ))
            .unwrap();
        };
        assign(1_000);
        let before = live_allocations();
        assign(100_000);
        let after = live_allocations();
        assert!(
            after - before < 1_000,
            "{} allocations kept after 100k assignments",
            after - before
        );
    }

    #[test]
    fn an_eeca_action_reads_the_value_being_written() {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            clear_context_stack();
            let facade = crate::testing::open_with(
                &crate::testing::framework_dir(),
                crate::testing::Options {
                    entity_eca: true,
                    data_feed: false,
                    tape: None,
                },
            );
            crate::entity::load_feed::add_eca_rules(
                &facade,
                r#"<eecas>
                    <eeca id="TestReadsShared" entity="moqui.test.TestEntity" on-create="true" on-update="true">
                        <actions><script>
                            var foo = ec.entity.makeValue("moqui.test.Foo");
                            foo.fooId = "F-" + shared.testMedium;
                            foo.fooText = shared.testId;
                            foo.create();
                        </script></actions>
                    </eeca>
                </eecas>"#,
            )
            .unwrap();
            let context = ExecutionContextFactory::test_context();
            context.factory().set_entity(facade.clone());
            let _entered = context.enter();
            let written = run_script(
                r#"shared = ec.entity.makeValue("moqui.test.TestEntity");
                shared.testId = "T1";
                shared.testMedium = "first";
                shared.create();
                shared.testMedium = "second";
                shared.update();"#,
            )
            .map_err(|err| err.to_string());
            let foos = facade
                .find("moqui.test.Foo")
                .and_then(|find| find.list())
                .map(|list| {
                    list.values()
                        .iter()
                        .map(|row| format!("{:?}={:?}", row.get("fooId"), row.get("fooText")))
                        .collect::<Vec<_>>()
                })
                .map_err(|err| err.to_string());
            let _ = done_tx.send(written.and(foos));
        });
        let foos = done_rx
            .recv_timeout(std::time::Duration::from_secs(30))
            .expect("the writes finished: the rule's read of the value did not deadlock")
            .unwrap();
        assert_eq!(foos.len(), 2, "{foos:?}");
        assert!(
            foos.iter().any(|foo| foo.contains("F-first") && foo.contains("T1")),
            "{foos:?}"
        );
        assert!(foos.iter().any(|foo| foo.contains("F-second")), "{foos:?}");
    }

    #[test]
    fn an_unknown_name_reads_undefined_and_does_not_write() {
        let context = enumerations();
        let _entered = context.enter();
        holds(r#"ec.entity.makeValue("moqui.basic.Enumeration").noSuchField === undefined"#);
        holds(r#"ec.entity.makeValue("moqui.basic.Enumeration").get("noSuchField") === null"#);
        for script in [
            r#"ec.entity.makeValue("moqui.basic.Enumeration").noSuchField = 1"#,
            r#"ec.entity.makeValue("moqui.basic.Enumeration").set("noSuchField", 1)"#,
            r#"ec.entity.makeValue("moqui.basic.Enumeration").update = 1"#,
        ] {
            let err = run_script(script).unwrap_err();
            assert!(
                err.message()
                    .contains("is not valid for entity moqui.basic.Enumeration"),
                "{script}: {err}"
            );
        }
    }

    #[test]
    fn a_set_through_the_prototype_chain_defines_the_property_on_the_receiver() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        holds(
            r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = "X1";
                const child = Object.create(value);
                child.enumId = "C1";
                child.noSuchField = 2;
                const other = {};
                Reflect.set(value, "description", "other", other);
                return value.enumId === "X1" && value.get("enumId") === "X1" && child.enumId === "C1"
                    && Object.keys(child).join() === "enumId,noSuchField" && child.noSuchField === 2
                    && value.description === null && other.description === "other"
                    && Reflect.set(value, "enumId", "X2", value) && value.enumId === "X2";
            })()"#,
        );
        holds(
            r#"(function () {
                const list = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list();
                const child = Object.create(list);
                child.extra = "e";
                child[0] = "zero";
                list.extra = "ignored";
                return child.extra === "e" && child[0] === "zero" && list[0].enumId === "E1"
                    && list.extra === undefined && child.length === 3;
            })()"#,
        );
    }

    #[test]
    fn clone_value_is_a_separate_copy() {
        let ctx = enumerations();
        let _entered = ctx.enter();
        holds(
            r#"(() => { const a = ec.entity.makeValue("moqui.basic.Enumeration"); a.enumId = "X"; a.description = "one";
                const b = a.cloneValue(); b.description = "two";
                return a.description === "one" && b.description === "two" && b.enumId === "X"; })()"#,
        );
    }

    #[test]
    fn equals_compares_the_entity_and_field_values_not_identity() {
        let ctx = enumerations();
        let _entered = ctx.enter();
        holds(
            r#"(() => { const a = ec.entity.makeValue("moqui.basic.Enumeration"); a.enumId = "X"; a.description = "one";
                const b = a.cloneValue(); const c = a.cloneValue(); c.description = "two";
                const other = ec.entity.makeValue("moqui.basic.EnumerationType"); other.enumTypeId = "X"; other.description = "one";
                return a !== b && a.equals(b) && !a.equals(c) && !a.equals(other) && !a.equals(null) && !a.equals({}); })()"#,
        );
    }

    #[test]
    fn a_method_name_wins_over_a_field_name() {
        let context = enumerations();
        let _entered = context.enter();
        holds(
            r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                return typeof value.get === "function" && typeof value.findRelatedOne === "function"
                    && typeof value.toString === "function" && "update" in value && "enumId" in value
                    && !("noSuchField" in value) && value[Symbol.iterator] === undefined;
            })()"#,
        );
    }

    #[test]
    fn a_relationship_name_reads_the_related_value_or_list() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        holds(
            r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().type.description === "Type 1""#,
        );
        holds(
            r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one()["moqui.basic.EnumerationType"].enumTypeId === "T1""#,
        );
        holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().parent === null"#);
        holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().groupMembers.length === 0"#);
        // get(name) reads a relationship the same way, as Java's EntityValue.get does
        holds(
            r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().get("moqui.basic.EnumerationType").enumTypeId === "T1""#,
        );
        holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().get("parent") === null"#);
        holds(
            r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().get("noSuchName") === null"#,
        );
    }

    #[test]
    fn filter_map_list_copies_or_filters_an_entity_list() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        crate::script::run_actions(
            r#"<actions>
                <entity-find entity-name="moqui.basic.Enumeration" list="all"/>
                <filter-map-list list="all" to-list="firsts"><field-map field-name="enumId" value="E1"/></filter-map-list>
                <set field="allBefore" from="all.length"/>
                <filter-map-list list="all"><field-map field-name="enumId" value="E2"/></filter-map-list>
                <set field="seen" from="allBefore + ':' + firsts.length + firsts[0].enumId + ':' + all.length + all[0].enumId"/>
            </actions>"#,
        )
        .unwrap();
        let seen = crate::script::context_value("seen").map(|value| moqui_lib::object::to_plain_string(&value));
        assert_eq!(seen.as_deref(), Some("3:1E1:1E2"));
    }

    #[test]
    fn keys_json_spread_and_for_in_see_the_fields() {
        let context = enumerations();
        let _entered = context.enter();
        holds(
            r#"(function () {
                const value = ec.entity.makeValue("moqui.basic.Enumeration");
                value.enumId = "X1";
                value.description = "d";
                const seen = [];
                for (const key in value) seen.push(key);
                const copy = { ...value };
                return Object.keys(value).join() === "description,enumId" && seen.join() === "description,enumId"
                    && JSON.stringify(value) === '{"description":"d","enumId":"X1"}'
                    && copy.enumId === "X1" && Object.keys(copy).length === 2
                    && Object.prototype.hasOwnProperty.call(value, "enumId")
                    && Object.getOwnPropertyDescriptor(value, "description").value === "d";
            })()"#,
        );
    }

    #[test]
    fn a_list_indexes_iterates_and_sizes() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        holds(
            r#"(function () {
                const list = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list();
                const ids = [];
                for (const value of list) ids.push(value.enumId);
                return list[0].enumId === "E1" && list[2].description === "third" && list[3] === undefined
                    && list.length === 3 && list.size() === 3 && !list.isEmpty() && "1" in list && !("3" in list)
                    && ids.join() === "E1,E2,E3" && list.map((value) => value.sequenceNum).join() === "1,2,3"
                    && list.filter((value) => value.sequenceNum > 1).length === 2
                    && JSON.parse(JSON.stringify(list))[1].enumId === "E2" && Object.keys(list).join() === "0,1,2";
            })()"#,
        );
    }

    /// A script's `update()` of a value it found writes only the fields it changed, by property, `set` or `setFields`:
    /// another value read with it keeps the field it changed, a field set to what it held is not written, and after an
    /// update a field set back to what it was read with is.
    #[test]
    fn a_script_update_writes_only_the_fields_it_changed() {
        clear_context_stack();
        let tape = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let context = ExecutionContextFactory::test_context();
        context
            .factory()
            .set_entity(crate::testing::open_with_tape(std::sync::Arc::clone(&tape)));
        let _entered = context.enter();
        let _authz = context.disable_authz();
        seed();
        let updates = || -> Vec<String> {
            moqui_lib::sync::lock(&tape)
                .iter()
                .filter(|sql| sql.starts_with("UPDATE"))
                .map(|sql| sql.split(" WHERE ").next().unwrap_or_default().to_string())
                .collect()
        };
        holds(
            r#"(function () {
                const read = () => ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").useCache(false).one();
                const a = read(), b = read(), c = read();
                a.description = "by property";
                a.update();
                b.setFields({ sequenceNum: 7, description: "first" }, true, null, null);
                b.update();
                c.set("enumCode", "C1");
                c.description = c.description;
                c.update();
                const stored = read();
                return stored.description === "by property" && stored.sequenceNum === 7 && stored.enumCode === "C1";
            })()"#,
        );
        let written = updates();
        assert_eq!(written.len(), 3, "{written:?}");
        assert!(
            written[0].contains("description") && !written[0].contains("sequence_num"),
            "{written:?}"
        );
        assert!(
            written[1].contains("sequence_num") && !written[1].contains("description"),
            "{written:?}"
        );
        assert!(
            written[2].contains("enum_code") && !written[2].contains("description"),
            "{written:?}"
        );
        holds(
            r#"(function () {
                const value = ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E2").useCache(false).one();
                value.description = "changed";
                value.update();
                value.update();
                value.description = "second";
                value.update();
                return ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E2").useCache(false).one()
                    .description === "second";
            })()"#,
        );
        assert_eq!(updates().len(), 5, "{:?}", updates());
    }

    #[test]
    fn a_field_set_on_a_list_row_is_kept_in_the_list() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        holds(
            r#"(() => {
                const list = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list();
                list[0].description = "changed";
                list.get(1).description = "by get";
                for (const value of list) value.enumCode = "C" + value.sequenceNum;
                list.forEach((value) => { value.optionValue = "f"; });
                const part = list.getPartialList(2, 1, false);
                part[0].description = "by part";
                return list[0].description === "changed" && list.get(1).description === "by get"
                    && list[2].enumCode === "C3" && list[1].optionValue === "f" && list[2].description === "by part";
            })()"#,
        );
        // the list kept on the context has the change, and the database and a new find do not
        run_script(
            r#"enums = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list(); enums[0].description = "kept""#,
        )
        .unwrap();
        holds(r#"enums[0].description === "kept" && ec.context.get("enums")[0].description === "kept""#);
        holds(r#"ec.entity.find("moqui.basic.Enumeration").condition("enumId", "E1").one().description === "first""#);
    }

    #[test]
    fn a_cached_find_list_is_not_changed_by_a_script() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        holds(
            r#"(() => {
                const first = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").orderBy("sequenceNum").useCache(true).list();
                first[0].description = "scribbled";
                const again = ec.entity.find("moqui.basic.Enumeration").condition("enumTypeId", "T1").orderBy("sequenceNum").useCache(true).list();
                return again[0].description === "first";
            })()"#,
        );
    }

    #[test]
    fn an_iterate_entry_is_the_row_of_the_list() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        crate::script::actions::run_actions(
            r#"<actions>
                <entity-find entity-name="moqui.basic.Enumeration" list="enumList"><order-by field-name="sequenceNum"/></entity-find>
                <iterate list="enumList" entry="enumValue"><set field="enumValue.description" from="'it ' + enumValue.enumId"/></iterate>
            </actions>"#,
        )
        .unwrap();
        holds(r#"enumList[0].description === "it E1" && enumList[2].description === "it E3""#);
    }

    #[test]
    fn a_map_list_read_through_context_get_keeps_its_writes() {
        clear_context_stack();
        run_script(r#"rows = [{a: 1}, {a: 2}]"#).unwrap();
        run_script(r#"ec.context.get("rows")[0].a = 5; ec.context.get("rows").push({a: 3})"#).unwrap();
        holds(r#"rows.length === 3 && rows[0].a === 5"#);
    }

    #[test]
    fn a_template_loop_reads_fields_by_dot() {
        let context = enumerations();
        let _entered = context.enter();
        seed();
        run_script(r#"enumList = ec.entity.find("moqui.basic.Enumeration").orderBy("sequenceNum").list()"#).unwrap();
        let text = render_trusted(
            "<#list enumList as row>${row.enumId}=${row.description} (${row.type.description});</#list>",
            Escape::None,
        )
        .unwrap();
        assert_eq!(text, "E1=first (Type 1);E2=second (Type 1);E3=third (Type 1);");
    }
}
