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
    methods: &[
        method!("ec.cache", "getCache", [Text], None, leaf, get_cache),
        method!("ec.cache", "getCacheNames", [], None, leaf, cache_names),
        method!("ec.cache", "clearCachesByPrefix", [Text], clear_by_prefix),
    ],
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
        method!(
            "ec.cache.getCache()",
            "getAndPut",
            [Text, Value],
            None,
            leaf,
            get_and_put
        ),
        method!(
            "ec.cache.getCache()",
            "getAndRemove",
            [Text],
            None,
            leaf,
            get_and_remove
        ),
        method!(
            "ec.cache.getCache()",
            "putIfAbsent",
            [Text, Value],
            None,
            leaf,
            put_if_absent
        ),
        method!("ec.cache.getCache()", "replace", [Text, Value], None, leaf, replace),
        method!("ec.cache.getCache()", "getAll", [Value], None, leaf, get_all),
        method!("ec.cache.getCache()", "putAll", [Value], None, leaf, put_all),
        method!("ec.cache.getCache()", "removeAll", [OptValue], remove_all),
        method!("ec.cache.getCache()", "keySet", [], None, leaf, key_set),
        method!("ec.cache.getCache()", "getName", [], None, leaf, name),
        method!("ec.cache.getCache()", "size", [], None, leaf, size),
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

/// Java's `getCacheNames`: the conf caches made so far, sorted.
fn cache_names(_: &mut Call<'_>) -> Result<Ret, HostError> {
    let mut names = match with_ec(|ec| ec.factory().cache().names())? {
        Ok(names) => names,
        Err(err) => return Err(HostError::message(err.message())),
    };
    names.sort();
    Ok(names.into())
}

/// Java's `clearCachesByPrefix`: each conf cache whose name begins with the prefix. One that will not clear is logged
/// and the rest go on.
fn clear_by_prefix(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let prefix = call.text_or_empty(0);
    let cleared = with_ec(|ec| {
        let caches = ec.factory().cache();
        let names = caches.names().unwrap_or_default();
        for name in names.iter().filter(|name| name.starts_with(&prefix)) {
            if let Err(err) = caches.clear(name) {
                tracing::error!("Cache {name} was not cleared: {}", err.message());
            }
        }
    });
    cleared?;
    Ok(Ret::Undefined)
}

/// The keys of a list argument, as text.
fn keys_of(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        Some(Value::Null) | None => Vec::new(),
        Some(one) => vec![moqui_lib::object::to_plain_string(one)],
    }
}

/// `getAndPut(key, value)`: the value the key had, or null.
fn get_and_put(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let value = call.value_or_null(1);
    let old = with_cache(call, |cache| {
        let old = cache.get(&key)?;
        cache.put(key, value)?;
        Ok(old)
    })?;
    Ok(Ret::Value(old.unwrap_or(Value::Null)))
}

fn get_and_remove(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let old = with_cache(call, |cache| {
        let old = cache.get(&key)?;
        cache.remove(&key)?;
        Ok(old)
    })?;
    Ok(Ret::Value(old.unwrap_or(Value::Null)))
}

/// `putIfAbsent(key, value)`: whether the value was put.
fn put_if_absent(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let value = call.value_or_null(1);
    Ok(with_cache(call, |cache| cache.put_if_absent(key, value))?.into())
}

/// `replace(key, value)`: put only when the key is there; whether it was.
fn replace(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let value = call.value_or_null(1);
    let replaced = with_cache(call, |cache| {
        if !cache.contains_key(&key)? {
            return Ok(false);
        }
        cache.put(key, value)?;
        Ok(true)
    })?;
    Ok(replaced.into())
}

/// `getAll(keys)`: an object of the keys that are there.
fn get_all(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let keys = keys_of(call.value(0));
    let found = with_cache(call, |cache| {
        let mut found = moqui_lib::object::ValueMap::default();
        for key in keys {
            if let Some(value) = cache.get(&key)? {
                found.insert(key, value);
            }
        }
        Ok(found)
    })?;
    Ok(Ret::Value(Value::Map(found)))
}

fn put_all(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let Some(Value::Map(entries)) = call.value(0).cloned() else {
        return Ok(Ret::Undefined);
    };
    with_cache(call, |cache| {
        for (key, value) in entries {
            cache.put(key, value)?;
        }
        Ok(())
    })?;
    Ok(Ret::Undefined)
}

/// `removeAll(keys)`: those keys; `removeAll()`: every entry, as `clear()`.
fn remove_all(call: &mut Call<'_>) -> Result<Ret, HostError> {
    if !call.passed(0) || matches!(call.value(0), Some(Value::Null)) {
        return clear(call);
    }
    let keys = keys_of(call.value(0));
    with_cache(call, |cache| {
        for key in keys {
            cache.remove(&key)?;
        }
        Ok(())
    })?;
    Ok(Ret::Undefined)
}

fn key_set(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let keys: Vec<String> = with_cache(call, |cache| {
        Ok(cache.entry_infos().into_iter().map(|entry| entry.key).collect())
    })?;
    Ok(keys.into())
}

fn name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(call.this::<CacheName>()?.0.clone().into())
}

fn size(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let size = with_cache(call, |cache| Ok(cache.len()))?;
    Ok(Ret::Value(Value::I64(i64::try_from(size).unwrap_or(i64::MAX))))
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
