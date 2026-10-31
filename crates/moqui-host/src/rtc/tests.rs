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

use std::collections::HashMap;

use super::media_config_from;

fn config(pairs: &[(&str, &str)]) -> Result<Option<super::MediaConfig>, String> {
    let map: HashMap<String, String> = pairs
        .iter()
        .map(|(k, v)| ((*k).to_string(), (*v).to_string()))
        .collect();
    media_config_from(&|name: &str| map.get(name).cloned().unwrap_or_default())
}

#[test]
fn media_is_off_without_a_port() {
    assert_eq!(config(&[]), Ok(None));
    assert_eq!(config(&[("rtc_media_port", "  ")]), Ok(None));
}

#[test]
fn media_binds_loopback_by_default_and_advertises_its_bind() {
    let c = config(&[("rtc_media_port", "10000")]).expect("valid").expect("on");
    assert_eq!(c.bind.to_string(), "127.0.0.1:10000");
    assert_eq!(c.public, c.bind);
}

#[test]
fn a_public_address_is_an_ip_or_an_ip_and_port() {
    let c = config(&[
        ("rtc_media_port", "10000"),
        ("rtc_media_host", "0.0.0.0"),
        ("rtc_public_address", "203.0.113.5"),
    ])
    .expect("valid")
    .expect("on");
    assert_eq!(c.bind.to_string(), "0.0.0.0:10000");
    assert_eq!(c.public.to_string(), "203.0.113.5:10000");
    let c = config(&[("rtc_media_port", "10000"), ("rtc_public_address", "203.0.113.5:443")])
        .expect("valid")
        .expect("on");
    assert_eq!(c.public.to_string(), "203.0.113.5:443");
}

#[test]
fn a_bad_port_or_address_is_an_error() {
    assert!(config(&[("rtc_media_port", "media")]).is_err());
    assert!(config(&[("rtc_media_port", "10000"), ("rtc_public_address", "not an address")]).is_err());
}
