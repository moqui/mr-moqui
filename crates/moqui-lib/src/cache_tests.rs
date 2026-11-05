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
fn expiry_uses_a_strict_less_than_boundary() {
    let clock = ManualClock::new(1_000);
    let cache = MCache::<String, String>::new(
        "entity.definition",
        CacheConfig::new()
            .expire_access_ms(1_000)
            .clock(Arc::clone(&clock) as Arc<dyn Clock>)
            .background(false),
    );
    cache.put("a".to_string(), "one".to_string()).unwrap();
    // Same instant as the put, so the hit does not move last-access time.
    assert_eq!(cache.get(&"a".to_string()).unwrap().as_deref(), Some("one"));
    clock.set(2_000);
    assert!(cache.contains_key(&"a".to_string()).unwrap());
    clock.set(2_001);
    assert_eq!(cache.get(&"a".to_string()).unwrap(), None);
    let stats = cache.stats();
    assert_eq!(stats.hits, 1);
    assert_eq!(stats.expires, 1);
    assert_eq!(stats.misses, 1);
}

#[test]
fn put_if_absent_stores_once_among_racing_threads_and_again_after_expiry() {
    let clock = ManualClock::new(0);
    let cache = MCache::<String, u32>::new(
        "replay",
        CacheConfig::new()
            .expire_creation_ms(100)
            .clock(Arc::clone(&clock) as Arc<dyn Clock>)
            .background(false),
    );
    let barrier = Arc::new(std::sync::Barrier::new(8));
    let stored: u32 = (0..8_u32)
        .map(|thread| {
            let cache = cache.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                u32::from(cache.put_if_absent("key".to_string(), thread).unwrap())
            })
        })
        .collect::<Vec<_>>()
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .sum();
    assert_eq!(stored, 1);
    let first = cache.get(&"key".to_string()).unwrap();
    assert!(!cache.put_if_absent("key".to_string(), 99).unwrap());
    assert_eq!(cache.get(&"key".to_string()).unwrap(), first);
    clock.set(101);
    assert!(cache.put_if_absent("key".to_string(), 99).unwrap());
    assert_eq!(cache.get(&"key".to_string()).unwrap(), Some(99));
}

/// `put_if` stores only when its check holds, and leaves an entry already there when it does not.
#[test]
fn put_if_stores_only_when_the_check_holds() {
    let cache = MCache::<String, u32>::new("fenced", CacheConfig::new().background(false));
    assert!(!cache.put_if("key".to_string(), 1, || false).unwrap());
    assert_eq!(cache.get(&"key".to_string()).unwrap(), None);
    assert!(cache.put_if("key".to_string(), 2, || true).unwrap());
    assert!(!cache.put_if("key".to_string(), 3, || false).unwrap());
    assert_eq!(cache.get(&"key".to_string()).unwrap(), Some(2));
}

#[test]
fn fresh_get_and_eviction_order() {
    let clock = ManualClock::new(10);
    let cache = MCache::<&'static str, i32>::new(
        "screen",
        CacheConfig::new()
            .max_entries(2)
            .clock(Arc::clone(&clock) as Arc<dyn Clock>)
            .background(false),
    );
    cache.put("old", 1).unwrap();
    clock.advance(5);
    cache.put("new", 2).unwrap();
    assert_eq!(cache.get(&"old").unwrap(), Some(1));
    clock.advance(5);
    // No background pass, so the put past the limit evicts.
    cache.put("third", 3).unwrap();
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.evict_overflow(), 0);
    assert_eq!(cache.get(&"new").unwrap(), None);
    assert_eq!(cache.get(&"old").unwrap(), Some(1));
    assert_eq!(cache.get(&"third").unwrap(), Some(3));
    assert_eq!(cache.stats().evictions, 1);
    cache.remove(&"third").unwrap();
    clock.set(100);
    cache.put("aged", 7).unwrap();
    clock.set(200);
    assert_eq!(cache.get_if_fresh(&"aged", 150).unwrap(), None);
    cache.close().unwrap();
    assert!(cache.get(&"new").is_err());
    assert!(cache.close().is_err());
}

#[test]
fn concurrent_gets_while_evicting() {
    // A background pass that is never due, so the puts do not evict and the explicit pass does.
    let cache = MCache::<u32, Vec<u32>>::new("lists", CacheConfig::new().max_entries(100).max_check_seconds(3_600));
    for key in 0..1_000 {
        cache.put(key, vec![key; 8]).unwrap();
    }
    std::thread::scope(|scope| {
        for reader in 0..4_u32 {
            let cache = cache.clone();
            scope.spawn(move || {
                for round in 0..2_000_u32 {
                    let key = (round * 7 + reader) % 1_000;
                    if let Some(value) = cache.get(&key).unwrap() {
                        assert_eq!(value, vec![key; 8]);
                    }
                }
            });
        }
        scope.spawn(|| {
            cache.evict_overflow();
        });
    });
    assert_eq!(cache.len(), 100);
    let stats = cache.stats();
    assert_eq!(stats.gets, 8_000);
    assert_eq!(stats.hits + stats.misses, stats.gets);
    assert_eq!(stats.evictions, 900);
}

#[test]
fn expired_entries_do_not_count_toward_the_limit() {
    let clock = ManualClock::new(0);
    let cache = MCache::<u32, u32>::new(
        "ttl",
        CacheConfig::new()
            .max_entries(2)
            .expire_creation_ms(10)
            .clock(Arc::clone(&clock) as Arc<dyn Clock>)
            .background(false),
    );
    cache.put(1, 1).unwrap();
    cache.put(2, 2).unwrap();
    clock.set(100);
    // Past the limit with no background pass: the put drops expired first, then overflow.
    cache.put(3, 3).unwrap();
    assert_eq!(cache.len(), 1);
    assert_eq!((cache.stats().expires, cache.stats().evictions), (2, 0));
    assert_eq!(cache.get(&3).unwrap(), Some(3));
}

#[test]
fn put_bounds_a_cache_without_a_background_pass() {
    let cache = MCache::<u32, u32>::new("screen.url", CacheConfig::new().max_entries(100).background(false));
    for key in 0..10_000 {
        cache.put(key, key).unwrap();
        assert!(cache.len() <= 110);
    }
    for key in 10_000..20_000 {
        cache.put_if_absent(key, key).unwrap();
        assert!(cache.len() <= 110);
    }
    assert!(cache.len() >= 100);
    assert_eq!(cache.get(&19_999).unwrap(), Some(19_999));
}

#[test]
fn a_second_put_in_the_same_millisecond_wins() {
    let clock = ManualClock::new(5);
    let cache = MCache::<&'static str, i32>::new(
        "same",
        CacheConfig::new()
            .clock(Arc::clone(&clock) as Arc<dyn Clock>)
            .background(false),
    );
    cache.put("key", 1).unwrap();
    cache.put("key", 2).unwrap();
    assert_eq!(cache.get(&"key").unwrap(), Some(2));
    // A put never moves the update time back, so `get_if_fresh` keeps its meaning.
    assert_eq!(cache.entry_infos()[0].updated, 5);
}

/// Threads whose name starts with the evict thread's (Linux truncates names to 15 bytes).
#[cfg(target_os = "linux")]
fn evict_thread_count() -> usize {
    let prefix = &EVICTOR_THREAD_NAME[..15];
    std::fs::read_dir("/proc/self/task")
        .unwrap()
        .filter_map(|task| std::fs::read_to_string(task.ok()?.path().join("comm")).ok())
        .filter(|name| name.trim_end() == prefix)
        .count()
}

#[cfg(target_os = "linux")]
#[test]
fn bounded_caches_share_one_evict_thread() {
    let caches: Vec<MCache<u32, u32>> = (0..20)
        .map(|index| {
            MCache::new(
                format!("shared{index}"),
                CacheConfig::new().max_entries(5).max_check_seconds(1),
            )
        })
        .collect();
    for cache in &caches {
        for key in 0..20 {
            cache.put(key, key).unwrap();
        }
    }
    let started = std::time::Instant::now();
    // Another test's thread may be exiting just as this one starts, so allow it a moment.
    while (evict_thread_count() != 1 || caches.iter().any(|cache| cache.len() > 5))
        && started.elapsed() < Duration::from_secs(5)
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(evict_thread_count(), 1);
    assert!(caches.iter().all(|cache| cache.len() == 5));
}

#[test]
fn setting_a_limit_starts_the_background_pass() {
    let cache = MCache::<u32, u32>::new("late", CacheConfig::new().max_check_seconds(1));
    for key in 0..5 {
        cache.put(key, key).unwrap();
    }
    cache.set_max_entries(2);
    let started = std::time::Instant::now();
    while cache.len() > 2 && started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(cache.len(), 2);
}

#[test]
fn manager_returns_the_existing_cache_and_drop_does_not_wait() {
    let manager = MCacheManager::new();
    let clock = ManualClock::new(0);
    let first = manager
        .create::<String, i32>(
            "local",
            CacheConfig::new()
                .max_entries(1)
                .max_check_seconds(30)
                .clock(clock)
                .background(true),
        )
        .unwrap();
    let again = manager.get::<String, i32>("local").unwrap().unwrap();
    again.put("k".to_string(), 1).unwrap();
    assert_eq!(first.get(&"k".to_string()).unwrap(), Some(1));
    let started = std::time::Instant::now();
    drop(first);
    drop(again);
    manager.destroy("local").unwrap();
    assert!(started.elapsed() < Duration::from_millis(50));
    assert!(manager.destroy("local").is_err());
}

#[test]
fn manager_lists_and_clears_caches_of_any_type() {
    let manager = MCacheManager::new();
    let text = manager
        .create::<String, String>(
            "l10n.message",
            CacheConfig::new().expire_access_ms(5_000).background(false),
        )
        .unwrap();
    let numbers = manager
        .create::<i32, i64>("other", CacheConfig::new().max_entries(10).background(false))
        .unwrap();
    text.put("a".to_string(), "one".to_string()).unwrap();
    assert_eq!(text.get(&"a".to_string()).unwrap().as_deref(), Some("one"));
    numbers.put(1, 2).unwrap();
    numbers.put(3, 4).unwrap();
    let infos = manager.infos().unwrap();
    assert_eq!(infos.len(), 2);
    assert_eq!(
        (infos[0].name.as_str(), infos[0].size, infos[0].expire_idle_ms),
        ("l10n.message", 1, 5_000)
    );
    assert_eq!(
        (infos[1].name.as_str(), infos[1].size, infos[1].max_entries),
        ("other", 2, 10)
    );
    assert_eq!(infos[0].stats.hits, 1);
    let entries = text.entry_infos();
    assert_eq!((entries[0].key.as_str(), entries[0].value.as_str()), ("a", "one"));
    assert!(manager.clear("other").unwrap());
    assert!(!manager.clear("missing").unwrap());
    assert_eq!(numbers.len(), 0);
    manager.clear_all().unwrap();
    assert_eq!(text.len(), 0);
}
