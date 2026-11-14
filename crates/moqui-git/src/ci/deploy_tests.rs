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

use super::{context, endpoint, REST_PATH};

#[test]
fn a_target_is_https_or_http_to_a_loopback_address_with_no_credentials() {
    assert_eq!(
        endpoint("https://demo.example.com/").unwrap(),
        format!("https://demo.example.com{REST_PATH}")
    );
    assert_eq!(
        endpoint("http://127.0.0.1:8080").unwrap(),
        format!("http://127.0.0.1:8080{REST_PATH}")
    );
    assert!(endpoint("http://[::1]:8080").is_ok());
    for refused in [
        "http://demo.example.com",
        "http://10.0.0.5",
        "ftp://demo.example.com",
        "demo.example.com",
        "https://user:secret@demo.example.com",
        "https:///path",
    ] {
        let err = endpoint(refused).unwrap_err();
        assert!(!err.contains("secret"), "{err}");
    }
    assert_eq!(context("prod"), "deploy/prod");
}
