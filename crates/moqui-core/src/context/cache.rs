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

//! One `MCache` per `<cache name>`. No second cache implementation. Values are host values, so a
//! script that puts a map gets a map back, as with Java's object caches.

use moqui_lib::cache::{CacheConfig, CacheInfo, MCache, MCacheManager};
use moqui_lib::node::MNode;
use moqui_lib::object::Value;

use super::Error;

pub struct CacheFacade {
    manager: MCacheManager,
}

impl CacheFacade {
    pub(super) fn from_conf(conf: &MNode) -> Result<Self, Error> {
        let manager = MCacheManager::new();
        let Some(list) = conf.first_named("cache-list") else {
            return Ok(Self { manager });
        };
        let mut count = 0_usize;
        for node in list.named_iter("cache") {
            let Some(name) = node.attribute("name").filter(|name| !name.is_empty()) else {
                continue;
            };
            let mut config = CacheConfig::new();
            if let Some(millis) = expire_millis(&node, &name, "expire-time-idle")? {
                config = config.expire_access_ms(millis);
            }
            if let Some(millis) = expire_millis(&node, &name, "expire-time-live")? {
                config = config.expire_creation_ms(millis);
            }
            if let Some(max) = node
                .attribute("max-elements")
                .and_then(|text| text.trim().parse::<usize>().ok())
                .filter(|max| *max > 0)
            {
                config = config.max_entries(max);
            }
            manager.create::<String, Value>(name, config)?;
            count += 1;
        }
        tracing::info!("Initialized {count} caches");
        Ok(Self { manager })
    }

    pub fn names(&self) -> Result<Vec<String>, Error> {
        self.manager.cache_names().map_err(Error::from)
    }

    pub fn get(&self, name: &str) -> Result<Option<MCache<String, Value>>, Error> {
        self.manager.get(name).map_err(Error::from)
    }

    /// The cache of that name, made with the default settings (no size limit, no expiry) when no conf entry made it,
    /// as Java's `getCache` creates a default local cache on first use. A configured cache is returned as it is.
    pub fn get_or_create(&self, name: &str) -> Result<MCache<String, Value>, Error> {
        if let Some(cache) = self.manager.get(name)? {
            return Ok(cache);
        }
        tracing::info!("Creating default local cache {name}");
        // `create` returns the cache another thread made first under the same name.
        self.manager
            .create::<String, Value>(name.to_string(), CacheConfig::new())
            .map_err(Error::from)
    }

    /// The size, limits, and counters of each conf cache.
    pub fn infos(&self) -> Result<Vec<CacheInfo>, Error> {
        self.manager.infos().map_err(Error::from)
    }

    /// Clear one conf cache. `false` when there is no cache of that name.
    pub fn clear(&self, name: &str) -> Result<bool, Error> {
        self.manager.clear(name).map_err(Error::from)
    }

    /// Clear every conf cache.
    pub fn clear_all(&self) -> Result<(), Error> {
        self.manager.clear_all().map_err(Error::from)
    }
}

fn expire_millis(node: &MNode, cache_name: &str, attr: &str) -> Result<Option<i64>, Error> {
    let Some(text) = node.attribute(attr).filter(|text| !text.is_empty()) else {
        return Ok(None);
    };
    if text == "0" {
        return Ok(None);
    }
    let seconds: i64 = text.parse().map_err(|_| {
        Error::new(format!(
            "cache {cache_name} has invalid {attr} `{text}`, expected seconds"
        ))
    })?;
    if seconds <= 0 {
        return Ok(None);
    }
    Ok(Some(seconds.saturating_mul(1_000)))
}

#[cfg(test)]
mod tests {
    use moqui_lib::node::MNode;
    use moqui_lib::object::Value;

    use super::CacheFacade;

    /// A configured cache keeps its conf settings; a name with none is made on first use with the defaults, once.
    #[test]
    fn get_or_create_keeps_a_configured_cache_and_makes_an_unknown_one() {
        let conf = MNode::parse_text(
            "conf",
            r#"<moqui-conf><cache-list><cache name="known" max-elements="5" expire-time-idle="60"/></cache-list></moqui-conf>"#,
        )
        .unwrap();
        let facade = CacheFacade::from_conf(&conf).unwrap();
        let known = facade.get_or_create("known").unwrap();
        assert_eq!(known.info().max_entries, 5);
        assert_eq!(known.info().expire_idle_ms, 60_000);
        assert!(facade.get("other").unwrap().is_none());
        let other = facade.get_or_create("other").unwrap();
        other.put("k".to_string(), Value::string("v")).unwrap();
        let again = facade.get_or_create("other").unwrap();
        assert_eq!(again.get(&"k".to_string()).unwrap(), Some(Value::string("v")));
        assert_eq!(again.info().max_entries, 0);
        assert_eq!(again.info().expire_idle_ms, 0);
        assert!(facade.names().unwrap().contains(&"other".to_string()));
    }
}
