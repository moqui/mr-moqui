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
    };
    assert_eq!(
        properties(&chosen, Some(PathBuf::from("/d"))),
        vec![
            ("embedded_db_backup_scheduled", "true".to_string()),
            ("embedded_db_backup_dir", "/b".to_string())
        ]
    );
    let default = Settings::default();
    assert_eq!(
        properties(&default, Some(PathBuf::from("/d")))[1],
        ("embedded_db_backup_dir", "/d".to_string())
    );
    let off = Settings {
        folder: None,
        scheduled: false,
    };
    assert_eq!(
        properties(&off, None),
        vec![("embedded_db_backup_scheduled", "false".to_string())]
    );
}

#[test]
fn the_folder_is_the_chosen_then_the_default_then_the_runtimes() {
    let runtime = Path::new("/rt");
    let chosen = Settings {
        folder: Some(PathBuf::from("/b")),
        scheduled: true,
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
    let name = backup_name(stamp("2026-11-03T14:05:09Z"), MANUAL);
    assert_eq!(name, "moqui-20261103T140509Z-manual.dump");
    assert!(!moqui_core::backup::is_archive_name(&name));
    assert_eq!(
        backup_name(stamp("2026-11-03T14:05:09Z"), BEFORE_RESTORE),
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
        tool_args(false, runtime, dump),
        vec!["save-db", "/rt/db/embedded", "/b/moqui-x-manual.dump.partial"]
    );
    assert_eq!(tool_args(true, runtime, dump)[0], "load-save");
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
    };
    save(&dir, &settings).unwrap();
    assert_eq!(load(&dir), settings);
    std::fs::remove_dir_all(&dir).unwrap();
}
