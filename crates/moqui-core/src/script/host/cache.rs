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

//! `ec.cache`, and the named framework cache `getCache(name)` returns. A name the conf does not define is made with the
//! default settings on first use, as Java's `getCache`.

use moqui_lib::object::{HostObject, Value};

use super::{method, with_ec, Call, HostClass, HostError, Ret};

pub(super) static CACHE_FACADE: HostClass = HostClass {
    name: "host:CacheFacade",
    methods: &[method!("ec.cache", "getCache", [Text], None, leaf, get_cache)],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

pub(super) static CACHE: HostClass = HostClass {
    name: "host:Cache",
    methods: &[
        method!("ec.cache.getCache()", "get", [Text], None, leaf, get),
        method!("ec.cache.getCache()", "put", [Text, Value], None, leaf, put),
        method!("ec.cache.getCache()", "remove", [Text], None, leaf, remove),
        method!("ec.cache.getCache()", "containsKey", [Text], None, leaf, contains_key),
        method!("ec.cache.getCache()", "clear", [], clear),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

/// The state of a cache object: its name.
struct CacheName(String);

fn get_cache(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text(0).unwrap_or_default().to_string();
    Ok(Ret::Object(HostObject::new(CacheName(name)), &CACHE))
}

/// `body` over the cache the receiver names.
fn with_cache<T>(
    call: &Call<'_>,
    body: impl FnOnce(&moqui_lib::cache::MCache<String, Value>) -> Result<T, moqui_lib::error::Error>,
) -> Result<T, HostError> {
    let name = call.this::<CacheName>()?;
    let found = with_ec(|ec| ec.factory().cache().get_or_create(&name.0))?;
    match found {
        Ok(cache) => body(&cache).map_err(HostError::Lib),
        Err(err) => Err(HostError::message(err.message())),
    }
}

fn get(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text(0).unwrap_or_default().to_string();
    let found = with_cache(call, |cache| cache.get(&key))?;
    Ok(Ret::Value(found.unwrap_or(Value::Null)))
}

fn put(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text(0).unwrap_or_default().to_string();
    let value = call.value_or_null(1);
    with_cache(call, |cache| cache.put(key, value))?;
    Ok(Ret::Undefined)
}

fn remove(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text(0).unwrap_or_default().to_string();
    Ok(with_cache(call, |cache| cache.remove(&key))?.into())
}

fn contains_key(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text(0).unwrap_or_default().to_string();
    Ok(with_cache(call, |cache| cache.contains_key(&key))?.into())
}

/// Clear the cache. A name that is not a conf cache may be the screen definitions or an entity list cache.
fn clear(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.this::<CacheName>()?;
    let cleared = with_ec(|ec| ec.factory().cache().clear(&name.0))?;
    if let Err(err) = cleared {
        return Err(HostError::message(err.message()));
    }
    // Also the screen or entity list cache of that name: a `get` of it made an empty conf cache, which the line above
    // cleared.
    clear_other_cache(&name.0).map_err(HostError::Lib)?;
    Ok(Ret::Undefined)
}

/// Clear a cache that is not a conf cache: the screen definitions or an entity list. `false` when there is none.
fn clear_other_cache(name: &str) -> Result<bool, moqui_lib::error::Error> {
    let Some(factory) = crate::context::ExecutionContext::with_current(|ec| std::sync::Arc::clone(ec.factory())) else {
        return Ok(false);
    };
    if crate::screen::ScreenFacade::of(&factory).clear_named(name) {
        return Ok(true);
    }
    match factory.entity() {
        Some(facade) => crate::entity::cache::clear_list_cache(&facade, name),
        None => Ok(false),
    }
}
