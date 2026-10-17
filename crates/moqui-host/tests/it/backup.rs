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

//! The embedded backup service and its SFTP mirror, on a seeded server with the built-in SSH server.

#![cfg(all(unix, feature = "embedded-postgres", feature = "postgres", feature = "axum"))]

use std::collections::BTreeMap;
use std::path::Path;

use base64::Engine;
use jiff::{SignedDuration, Timestamp};
use moqui_core::backup::{archive_name, Tier};
use moqui_core::sftp::SftpClient;
use moqui_lib::object::{to_plain_string, Value};

const HOST_SEED: [u8; 32] = [11; 32];

#[test]
fn the_backup_service_mirrors_a_rotated_dump() {
    let seed = base64::engine::general_purpose::STANDARD.encode(HOST_SEED);
    let gate = crate::common::start_with_properties("backup", None, &[("ssh_port", "0"), ("ssh_host_key", &seed)]);
    let address = gate.server.ssh_address().expect("the SSH server listens");
    let factory = std::sync::Arc::clone(gate.server.factory());
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();

    let first = call(&[("systemMessageRemoteId", "EmbeddedDbBackup")]);
    assert_eq!(text(&first, "skipped"), "N", "{first:?}");
    assert_eq!(text(&first, "mirrored"), "N", "{first:?}");
    assert_eq!(text(&first, "backupKind"), "dump", "{first:?}");
    let archive = text(&first, "archivePath");
    assert!(Path::new(&archive).is_file(), "{archive}");
    let backup_dir = Path::new(&archive).parent().unwrap().to_path_buf();

    let (old_name, promoted_name) = previous_archive();
    std::fs::write(backup_dir.join(&old_name), b"previous week").unwrap();

    moqui_core::service::sync(
        "update#moqui.service.message.SystemMessageRemote",
        &map(&[
            ("systemMessageRemoteId", "EmbeddedDbBackup"),
            ("sendUrl", &format!("127.0.0.1:{}", address.port())),
            ("username", "john.doe"),
            ("password", "moqui"),
        ]),
    )
    .unwrap_or_else(|err| panic!("update remote: {err}"));

    let host = format!("127.0.0.1:{}", address.port());
    let mut client = SftpClient::new(&host, "john.doe", None).unwrap();
    client.password("moqui").connect().unwrap();
    client.mkdirs("moqui-backup").unwrap();
    client
        .put_bytes(&format!("moqui-backup/{old_name}"), "", b"previous week")
        .unwrap();
    client.put_bytes("moqui-backup/notes.txt", "", b"leave this").unwrap();
    client
        .put_bytes("moqui-backup/moqui-20200101T000000Z-d.dump", "", b"remote only")
        .unwrap();
    client.close();

    let second = call(&[("systemMessageRemoteId", "EmbeddedDbBackup"), ("dailyCount", "1")]);
    assert_eq!(text(&second, "skipped"), "N", "{second:?}");
    assert_eq!(text(&second, "mirrored"), "Y", "{second:?}");
    assert_eq!(text(&second, "backupKind"), "dump", "{second:?}");
    let new_archive = Path::new(&text(&second, "archivePath"))
        .file_name()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert!(text(&second, "promoted").contains(&promoted_name), "{second:?}");
    assert!(backup_dir.join(&promoted_name).is_file(), "{}", backup_dir.display());
    assert!(!backup_dir.join(&old_name).exists());

    let mut client = SftpClient::new(&host, "john.doe", None).unwrap();
    client.password("moqui").connect().unwrap();
    let names = remote_names(&mut client);
    assert!(
        names.iter().any(|name| name == &promoted_name),
        "remote names {names:?}"
    );
    assert!(names.iter().any(|name| name == &new_archive), "remote names {names:?}");
    assert!(names.iter().any(|name| name == "notes.txt"), "remote names {names:?}");
    assert!(!names.iter().any(|name| name == &old_name), "remote names {names:?}");
    assert!(
        !names.iter().any(|name| name == "moqui-20200101T000000Z-d.dump"),
        "remote names {names:?}"
    );
    client.close();
}

fn call(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    moqui_core::service::sync("org.moqui.impl.ServerServices.backup#EmbeddedDatabase", &map(pairs))
        .unwrap_or_else(|err| panic!("backup: {err}"))
}

fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_string(), Value::string(*value)))
        .collect()
}

fn text(values: &BTreeMap<String, Value>, key: &str) -> String {
    values.get(key).map(to_plain_string).unwrap_or_default()
}

/// A daily from a completed ISO week, and the name the ladder renames it to. A week that is also a
/// completed month or year climbs in the same pass.
fn previous_archive() -> (String, String) {
    let zone = jiff::tz::TimeZone::system();
    let now = Timestamp::now();
    let now_date = now.to_zoned(zone.clone()).date();
    let now_week = now_date.iso_week_date();
    let mut previous = now;
    for days in 1..21 {
        let stamp = now.checked_sub(SignedDuration::from_secs(days * 24 * 3600)).unwrap();
        let date = stamp.to_zoned(zone.clone()).date();
        let week = date.iso_week_date();
        if (week.year(), week.week()) != (now_week.year(), now_week.week()) {
            previous = stamp;
            break;
        }
    }
    let prev_date = previous.to_zoned(zone).date();
    let letter = if prev_date.year() != now_date.year() {
        'y'
    } else if prev_date.month() != now_date.month() {
        'm'
    } else {
        'w'
    };
    let old_name = archive_name("moqui", previous, Tier::Daily, true);
    let promoted = old_name.replace("-d.dump", &format!("-{letter}.dump"));
    (old_name, promoted)
}

fn remote_names(client: &mut SftpClient) -> Vec<String> {
    client
        .ls("moqui-backup")
        .unwrap()
        .into_iter()
        .map(|(_, entry)| entry.name)
        .collect()
}
