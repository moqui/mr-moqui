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
fn a_set_cookie_sets_and_an_expired_one_removes() {
    assert_eq!(
        parse("JSESSIONID=abc123; Path=/; HttpOnly; SameSite=Lax"),
        Some(("JSESSIONID".to_string(), "abc123".to_string(), false))
    );
    assert_eq!(parse("v=1; Max-Age=0").map(|(_, _, gone)| gone), Some(true));
    assert_eq!(parse("v=; Path=/").map(|(_, _, gone)| gone), Some(true));
    assert_eq!(parse("v=2; max-age=3600").map(|(_, _, gone)| gone), Some(false));
    assert_eq!(parse("bad name=1"), None);
    assert_eq!(parse("novalue"), None);

    let jar = Jar::in_memory();
    assert_eq!(jar.header(), None);
    jar.take(["s=1; Path=/; HttpOnly", "visitor=v9; Max-Age=31536000"]);
    assert_eq!(jar.header().as_deref(), Some("s=1; visitor=v9"));
    jar.take(["s=2; Path=/"]);
    jar.take(["visitor=; Max-Age=0"]);
    assert_eq!(jar.header().as_deref(), Some("s=2"));
}

#[test]
fn the_cookies_are_kept_across_a_restart_in_a_file_only_the_user_reads() {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-cookies-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let file = dir.join("node-cookies");
    Jar::open(file.clone()).take(["s=abc; Path=/"]);
    assert_eq!(Jar::open(file.clone()).header().as_deref(), Some("s=abc"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
    }
    let _ = std::fs::remove_dir_all(&dir);
}
