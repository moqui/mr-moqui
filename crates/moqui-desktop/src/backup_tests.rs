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

use jiff::tz::TimeZone;
use jiff::Timestamp;

use super::*;

fn stamp(text: &str) -> Timestamp {
    text.parse().unwrap()
}

#[test]
fn the_settings_file_reads_back_what_it_keeps() {
    let settings = Settings {
        folder: Some(PathBuf::from("/Users/me/Documents/MR-Moqui Backups")),
        scheduled: false,
        told: true,
        key_told: true,
    };
    assert_eq!(Settings::parse(&settings.to_text()), settings);
    assert_eq!(Settings::parse(&Settings::default().to_text()), Settings::default());
}

#[test]
fn the_settings_default_to_the_nightly_backup_in_the_default_folder() {
    assert_eq!(Settings::parse(""), Settings::default());
    assert!(Settings::default().scheduled);
    let odd = Settings::parse("# a comment\nfolder = \nscheduled=maybe\ncolour=blue\nno equals sign\n");
    assert_eq!(odd, Settings::default());
}

#[test]
fn the_default_folder_is_in_documents_except_in_a_development_run() {
    let documents = Path::new("/home/me/Documents");
    assert_eq!(
        default_folder(false, Some(documents)),
        Some(documents.join(DEFAULT_FOLDER))
    );
    assert_eq!(default_folder(true, Some(documents)), None);
    assert_eq!(default_folder(false, None), None);
}

#[test]
fn the_node_gets_the_schedule_and_the_folder_unless_it_is_the_frameworks() {
    let chosen = Settings {
        folder: Some(PathBuf::from("/b")),
        scheduled: true,
        told: false,
        key_told: false,
    };
    assert_eq!(
        properties(&chosen, Some(PathBuf::from("/d"))),
        vec![
            ("moqui_conf__database_backup__scheduled", "true".to_string()),
            ("moqui_conf__database_backup__dir", "/b".to_string())
        ]
    );
    let default = Settings::default();
    assert_eq!(
        properties(&default, Some(PathBuf::from("/d")))[1],
        ("moqui_conf__database_backup__dir", "/d".to_string())
    );
    let off = Settings {
        folder: None,
        scheduled: false,
        told: false,
        key_told: false,
    };
    assert_eq!(
        properties(&off, None),
        vec![("moqui_conf__database_backup__scheduled", "false".to_string())]
    );
}

#[test]
fn the_folder_is_the_chosen_then_the_default_then_the_runtimes() {
    let runtime = Path::new("/rt");
    let chosen = Settings {
        folder: Some(PathBuf::from("/b")),
        scheduled: true,
        told: false,
        key_told: false,
    };
    assert_eq!(
        folder_of(&chosen, Some(PathBuf::from("/d")), runtime),
        PathBuf::from("/b")
    );
    assert_eq!(
        folder_of(&Settings::default(), Some(PathBuf::from("/d")), runtime),
        PathBuf::from("/d")
    );
    assert_eq!(
        folder_of(&Settings::default(), None, runtime),
        PathBuf::from("/rt/db/backup")
    );
}

#[test]
fn a_folder_in_or_around_the_database_is_refused() {
    let runtime = Path::new("/no/such/rt");
    assert!(refuse_folder(Path::new("/no/such/rt/db/embedded/backups"), runtime).is_some());
    assert!(refuse_folder(Path::new("/no/such/rt"), runtime).is_some());
    assert!(refuse_folder(Path::new("/no/such/rt/db/backup"), runtime).is_none());
    assert!(refuse_folder(Path::new("/elsewhere"), runtime).is_none());
}

#[test]
fn an_app_backup_is_named_outside_the_frameworks_ladder() {
    let name = backup_name(stamp("2026-11-03T14:05:09Z"), MANUAL, false);
    assert_eq!(name, "moqui-20261103T140509Z-manual.dump");
    assert!(!moqui_core::backup::is_archive_name(&name));
    assert_eq!(
        backup_name(stamp("2026-11-03T14:05:09Z"), BEFORE_RESTORE, false),
        "moqui-20261103T140509Z-before-restore.dump"
    );
}

#[test]
fn the_stamp_is_read_from_ladder_and_app_backups() {
    assert_eq!(
        backup_stamp("moqui-20261103T020000Z-d.dump"),
        Some(stamp("2026-11-03T02:00:00Z"))
    );
    assert_eq!(
        backup_stamp("moqui-20261103T140509Z-before-restore.dump"),
        Some(stamp("2026-11-03T14:05:09Z"))
    );
    assert_eq!(backup_stamp("moqui-20261103T020000Z-d"), None);
    assert_eq!(backup_stamp("moqui-20261103T020000Z-d.dump.partial"), None);
    assert_eq!(backup_stamp("search-20261103T020000Z-d.dump"), None);
    assert_eq!(backup_stamp("moqui-yesterday-d.dump"), None);
    assert_eq!(backup_stamp("notes.txt"), None);
    assert_eq!(backup_stamp("moqui_content-20261103T020000Z-manual.dump"), None);
}

#[test]
fn another_databases_dump_is_not_restored_from() {
    assert!(not_a_restore_dump("moqui_content-20261103T140509Z-manual.dump"));
    assert!(not_a_restore_dump("search-20261103T020000Z-d.dump"));
    assert!(!not_a_restore_dump("moqui-20261103T140509Z-manual.dump"));
    assert!(!not_a_restore_dump("moqui.dump"));
    assert!(!not_a_restore_dump("my-backup.dump"));
}

#[test]
fn the_last_backup_is_the_newest_in_the_folder() {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-backup-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    assert_eq!(last_backup(&dir), None);
    for name in [
        "moqui-20261101T020000Z-d.dump",
        "moqui-20261103T140509Z-manual.dump",
        "moqui-20261104T020000Z-d.dump.partial",
        "readme.txt",
    ] {
        std::fs::write(dir.join(name), "").unwrap();
    }
    assert_eq!(last_backup(&dir), Some(stamp("2026-11-03T14:05:09Z")));
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(last_backup(&dir), None);
}

#[test]
fn the_last_backup_line_is_in_local_time() {
    assert_eq!(last_label(None, &TimeZone::UTC), "Last backup: none");
    assert_eq!(
        last_label(Some(stamp("2026-11-03T14:05:09Z")), &TimeZone::UTC),
        "Last backup: 2026-11-03 14:05"
    );
    let offset = TimeZone::fixed(jiff::tz::offset(-5));
    assert_eq!(
        last_label(Some(stamp("2026-11-03T14:05:09Z")), &offset),
        "Last backup: 2026-11-03 09:05"
    );
}

#[test]
fn save_db_and_load_save_take_the_cluster_and_the_dump() {
    let runtime = Path::new("/rt");
    let dump = Path::new("/b/moqui-x-manual.dump.partial");
    assert_eq!(
        tool_args(false, runtime, dump, None),
        vec!["save-db", "/rt/db/embedded", "/b/moqui-x-manual.dump.partial"]
    );
    assert_eq!(tool_args(true, runtime, dump, None)[0], "load-save");
}

#[test]
fn a_dump_is_written_beside_its_name_as_a_partial() {
    assert_eq!(
        partial_of(Path::new("/b/moqui-x-manual.dump")),
        PathBuf::from("/b/moqui-x-manual.dump.partial")
    );
}

#[test]
fn the_settings_are_kept_in_the_config_directory() {
    let dir = std::env::temp_dir().join(format!("moqui-desktop-backup-settings-{}", std::process::id()));
    assert_eq!(load(&dir), Settings::default());
    let settings = Settings {
        folder: Some(PathBuf::from("/b")),
        scheduled: true,
        told: false,
        key_told: false,
    };
    save(&dir, &settings).unwrap();
    assert_eq!(load(&dir), settings);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// SEC-CLI-114: the first start with the nightly backup on tells the user what it holds and where it goes, once; a
/// development run and a switched-off backup do not.
#[test]
fn the_nightly_backup_is_told_once() {
    let mut settings = Settings::default();
    assert!(notice_due(&settings, false));
    assert!(
        !notice_due(&settings, true),
        "a development run keeps the framework's folder"
    );
    settings.told = true;
    assert!(!notice_due(&settings, false));
    assert!(!notice_due(
        &Settings {
            scheduled: false,
            ..Settings::default()
        },
        false
    ));
    assert!(Settings::parse(&settings.to_text()).told);
    assert!(
        NOTICE.contains("encrypted with a backup key")
            && NOTICE.contains("Save backup key")
            && NOTICE.contains("Nightly backup off")
    );
}

/// SEC-CLI-114: a backup the app takes sealed is `.dump.age`, the tray's last backup and its partial read it as the
/// plain one, and the tools are handed the key as wanted: the save the public key on its command line, the restore
/// `--identity -` with the key itself on its stdin, never in its arguments.
#[test]
fn a_sealed_backup_is_named_dump_age_and_its_key_never_goes_on_a_command_line() {
    let taken = stamp("2026-11-03T14:05:09Z");
    let name = backup_name(taken, MANUAL, true);
    assert_eq!(name, "moqui-20261103T140509Z-manual.dump.age");
    assert_eq!(backup_stamp(&name), Some(taken));
    assert_eq!(
        backup_stamp("moqui-20261103T020000Z-d.dump.age"),
        Some(stamp("2026-11-03T02:00:00Z"))
    );
    assert_eq!(backup_stamp("moqui-20261103T020000Z-d.age"), None);
    assert!(not_a_restore_dump("moqui_content-20261103T140509Z-manual.dump.age"));
    assert_eq!(
        partial_of(Path::new("/b").join(&name).as_path()),
        Path::new("/b/moqui-20261103T140509Z-manual.dump.age.partial")
    );

    let runtime = Path::new("/rt");
    let dump = Path::new("/b/moqui-x-manual.dump.age");
    assert_eq!(
        tool_args(false, runtime, dump, Some("age1recipient")),
        vec![
            "save-db",
            "/rt/db/embedded",
            "/b/moqui-x-manual.dump.age",
            "--recipient",
            "age1recipient"
        ]
    );
    let restore = tool_args(true, runtime, dump, Some("AGE-SECRET-KEY-1SECRET"));
    assert_eq!(
        restore,
        vec![
            "load-save",
            "/rt/db/embedded",
            "/b/moqui-x-manual.dump.age",
            "--identity",
            "-"
        ]
    );
    assert!(
        !restore.iter().any(|arg| arg.contains("SECRET")),
        "the key is on the command line"
    );
    // the node's dumps are sealed to the key the tray's take theirs to
    let source = include_str!("backup.rs");
    assert!(source.contains("properties.push((RECIPIENT_PROPERTY, recipient))"));
    assert!(source.contains("\"moqui_conf__database_backup__recipient\""));
}

/// The user told of the nightly backup before its backups were sealed is told of the key once; one told by the new
/// notice is not told again, and nothing is said of a key while the backups are not sealed or in a development run.
#[test]
fn the_backup_key_is_told_once_to_a_user_told_of_the_backups_before() {
    let before = Settings {
        told: true,
        ..Settings::default()
    };
    assert!(key_notice_due(&before, true, false));
    assert!(!key_notice_due(&before, false, false), "the backups are not sealed");
    assert!(!key_notice_due(&before, true, true), "a development run");
    let told = Settings {
        key_told: true,
        ..before.clone()
    };
    assert!(!key_notice_due(&told, true, false));
    assert!(
        !key_notice_due(&Settings::default(), true, false),
        "the first notice tells of the key itself"
    );
    assert!(KEY_NOTICE.contains("Save backup key"));
}
