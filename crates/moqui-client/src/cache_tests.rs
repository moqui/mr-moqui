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
fn answers_are_kept_per_account_for_the_shells_paths_only() {
    let dir = std::env::temp_dir().join(format!("moqui-client-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cache = Cache::new(&dir);
    cache.put("a", "/shellData", b"{\"apps\":[]}");
    cache.put("a", "/qapps/clientConfig", b"{}");
    cache.put("a", "/apps/Other", b"secret");
    cache.put("../b", "/shellData", b"x");
    assert_eq!(cache.get("a", "/shellData").as_deref(), Some(&b"{\"apps\":[]}"[..]));
    assert_eq!(cache.get("a", "/qapps/clientConfig").as_deref(), Some(&b"{}"[..]));
    assert_eq!(cache.get("a", "/apps/Other"), None);
    assert_eq!(cache.get("b", "/shellData"), None);
    cache.forget("a");
    assert_eq!(cache.get("a", "/shellData"), None);
    let _ = std::fs::remove_dir_all(&dir);
}
