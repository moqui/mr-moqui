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
fn a_missing_attribute_takes_its_default() {
    assert_eq!(ClientConf::parse("<client-conf/>"), ClientConf::default());
    let conf = ClientConf::default();
    assert_eq!(conf.device_mode, DeviceMode::Choose);
    assert_eq!(
        (
            conf.mirror_days,
            conf.prefetch_rooms,
            conf.prefetch_mail,
            conf.store_max_mb
        ),
        (90, 20, 100, 512)
    );
    assert_eq!(conf.store_max_bytes(), 512 * 1024 * 1024);
}

#[test]
fn the_attributes_are_read() {
    let conf = ClientConf::parse(
        r#"<client-conf device-mode="shared" mirror-days="30" prefetch-rooms="0" prefetch-mail=" 25 " store-max-mb="64"/>"#,
    );
    assert_eq!(
        conf,
        ClientConf {
            device_mode: DeviceMode::Shared,
            mirror_days: 30,
            prefetch_rooms: 0,
            prefetch_mail: 25,
            store_max_mb: 64
        }
    );
    assert_eq!(
        ClientConf::parse(r#"<client-conf device-mode="personal"/>"#).device_mode,
        DeviceMode::Personal
    );
}

#[test]
fn a_bad_value_or_text_takes_the_default() {
    let conf = ClientConf::parse(
        r#"<client-conf device-mode="kiosk" mirror-days="0" prefetch-rooms="-1" prefetch-mail="many" store-max-mb="0"
            unknown="x"/>"#,
    );
    assert_eq!(conf, ClientConf::default());
    assert_eq!(ClientConf::parse("<client-conf"), ClientConf::default());
    assert_eq!(
        ClientConf::parse(r#"<other device-mode="shared"/>"#),
        ClientConf::default()
    );
}
