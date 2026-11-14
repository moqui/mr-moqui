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

use super::pick_url;

/// A run's page: the application's URL when its service gives one, else (no URL: a repository it does not own) the
/// `run-url` template's; never something that is not an http(s) URL.
#[test]
fn the_run_url_falls_back_to_the_template_when_the_service_gives_none() {
    let template = || "https://ci.example.com/runs/7".to_string();
    assert_eq!(
        pick_url(Some("https://app.example.com/run/7".into()), template).as_deref(),
        Some("https://app.example.com/run/7")
    );
    assert_eq!(
        pick_url(Some(String::new()), template).as_deref(),
        Some("https://ci.example.com/runs/7")
    );
    assert_eq!(
        pick_url(Some("  ".into()), template).as_deref(),
        Some("https://ci.example.com/runs/7")
    );
    assert_eq!(
        pick_url(None, template).as_deref(),
        Some("https://ci.example.com/runs/7")
    );
    assert_eq!(pick_url(Some("javascript:x".into()), template), None);
    assert_eq!(pick_url(None, String::new), None);
}
