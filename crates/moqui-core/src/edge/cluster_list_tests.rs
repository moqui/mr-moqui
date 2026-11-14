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

use super::usable_link_url;

#[test]
fn a_link_address_is_an_https_url_with_a_host() {
    for url in [
        "https://edge1.example.com",
        " https://edge1.example.com:8443/ ",
        "HTTPS://edge1.example.com",
        "https://203.0.113.5:443",
        "https://[2001:db8::1]:8443",
        "https://[2001:db8::1]",
    ] {
        assert_eq!(usable_link_url(url), Some(url.trim()), "{url}");
    }
    let long = format!("https://{}.example.com", "a".repeat(510));
    for url in [
        "",
        "http://edge1.example.com",
        "edge1.example.com",
        "https://",
        "https:///path",
        "https://:8443",
        "https://edge1.example.com:0",
        "https://edge1.example.com:70000",
        "https://edge1.example.com:port",
        "https://user@edge1.example.com",
        "https://edge 1.example.com",
        "https://edge1.example.com/\u{7}",
        "https://[2001:db8::1",
        "https://[2001:db8::1]x",
        "https://[]",
        long.as_str(),
    ] {
        assert_eq!(usable_link_url(url), None, "{url}");
    }
}
