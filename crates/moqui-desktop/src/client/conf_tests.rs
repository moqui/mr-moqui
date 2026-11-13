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

use std::path::{Path, PathBuf};

use moqui_client::conf::{ClientConf, DeviceMode};

use super::read;

fn runtime(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-conf-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A runtime with the framework's default conf (`choose`) and a component directory.
fn framework(dir: &Path) {
    write(
        &dir.join("components/base/framework/component.xml"),
        r#"<component name="moqui-framework"/>"#,
    );
    write(
        &dir.join("components/base/framework/conf/MoquiDefaultConf.xml"),
        r#"<moqui-conf>
            <client-conf device-mode="choose" mirror-days="90" prefetch-rooms="20" prefetch-mail="100" store-max-mb="512"/>
            <component-list><component-dir location="components/addon"/></component-list>
        </moqui-conf>"#,
    );
}

#[test]
fn the_runtime_conf_wins_over_a_components_and_the_default() {
    let dir = runtime("merged");
    framework(&dir);
    write(
        &dir.join("components/addon/app/component.xml"),
        r#"<component name="app"/>"#,
    );
    write(
        &dir.join("components/addon/app/MoquiConf.xml"),
        r#"<moqui-conf><client-conf device-mode="shared" prefetch-rooms="5"/></moqui-conf>"#,
    );
    let conf = read(&dir);
    assert_eq!(conf.device_mode, DeviceMode::Shared);
    assert_eq!(conf.prefetch_rooms, 5);
    assert_eq!(conf.mirror_days, 90);
    // the build's own conf on top: personal
    write(
        &dir.join("MoquiConf.xml"),
        r#"<moqui-conf><client-conf device-mode="personal" store-max-mb="64"/></moqui-conf>"#,
    );
    let conf = read(&dir);
    assert_eq!(conf.device_mode, DeviceMode::Personal);
    assert_eq!(conf.prefetch_rooms, 5);
    assert_eq!(conf.store_max_mb, 64);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn no_element_or_no_framework_gives_the_defaults() {
    let dir = runtime("defaults");
    assert_eq!(read(&dir), ClientConf::default());
    write(
        &dir.join("components/base/framework/component.xml"),
        r#"<component name="moqui-framework"/>"#,
    );
    write(
        &dir.join("components/base/framework/conf/MoquiDefaultConf.xml"),
        "<moqui-conf/>",
    );
    assert_eq!(read(&dir), ClientConf::default());
    // a conf that does not parse: logged, the defaults
    write(
        &dir.join("components/base/framework/conf/MoquiDefaultConf.xml"),
        "<moqui-conf>",
    );
    assert_eq!(read(&dir), ClientConf::default());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_repositorys_default_conf_asks() {
    let repository = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../runtime");
    let text = std::fs::read_to_string(repository.join("components/base/framework/conf/MoquiDefaultConf.xml")).unwrap();
    assert!(text.contains(r#"<client-conf device-mode="choose""#));
}
