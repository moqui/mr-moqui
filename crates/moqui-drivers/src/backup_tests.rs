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

use std::path::PathBuf;

use super::*;

struct DirGuard(PathBuf);

impl Drop for DirGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let name = format!("moqui-backup-{}-{nanos}", std::process::id());
    if Path::new("/tmp").is_dir() {
        PathBuf::from("/tmp").join(name)
    } else {
        std::env::temp_dir().join(name)
    }
}

fn request(mode: BackupMode, backup_dir: PathBuf, daily: u32) -> BackupRequest {
    BackupRequest {
        mode,
        backup_dir: Some(backup_dir),
        daily,
        weekly: 5,
        monthly: 12,
        yearly: None,
        full_every: 7,
        // A spread checkpoint on the idle test cluster takes about 30 seconds for each pg_basebackup.
        fast_checkpoint: true,
        timeout: Duration::from_secs(180),
    }
}

#[test]
fn a_backup_directory_inside_the_data_directory_is_refused() {
    let base = temp_dir();
    let _guard = DirGuard(base.clone());
    let data = base.join("data");
    std::fs::create_dir_all(&data).unwrap();
    let target = ToolTarget {
        prefix: "moqui".to_string(),
        bin_dir: base.join("bin"),
        username: "postgres".to_string(),
        password: "secret".to_string(),
        port: 5432,
        host: data.display().to_string(),
        database: "moqui".to_string(),
        data_dir: data.clone(),
    };
    let err = run(&[target], &base, &request(BackupMode::Dump, data.join("backup"), 7)).unwrap_err();
    let text = err.to_string();
    assert!(text.contains("outside"), "{text}");
    assert!(!data.join("backup").exists());
}

#[cfg(unix)]
#[test]
fn dump_round_trip_rotates_the_same_week_and_incremental_when_the_tools_exist() {
    use std::os::unix::fs::PermissionsExt;
    use std::slice::from_ref;

    use super::super::embedded::EmbeddedCluster;

    let base = temp_dir();
    let _guard = DirGuard(base.clone());
    let (cluster, driver) = EmbeddedCluster::start(base.join("embedded"), 20).unwrap();
    let mut conn = driver.connect_postgres().unwrap();
    conn.execute("CREATE TABLE backup_kept (n int)", &[]).unwrap();
    conn.execute("INSERT INTO backup_kept (n) VALUES (?)", &[Value::I32(7)])
        .unwrap();
    drop(conn);

    let target = cluster.tool_connect("moqui");
    let backup = base.join("backup");
    std::fs::create_dir_all(&backup).unwrap();
    std::fs::write(backup.join("moqui-20000101T000000Z-d.dump.partial"), b"partial").unwrap();
    let older = Timestamp::now().checked_sub(SignedDuration::from_secs(2)).unwrap();
    let same_week = week_key(older) == week_key(Timestamp::now());
    let old_name = archive_name("moqui", older, Tier::Daily, true);
    if same_week {
        std::fs::write(backup.join(&old_name), b"older").unwrap();
    }

    let outcome = run(from_ref(&target), &base, &request(BackupMode::Dump, backup.clone(), 1)).unwrap();
    assert!(outcome.errors.is_empty(), "{:?}", outcome.errors);
    assert_eq!(outcome.backup_kind, "dump");
    assert!(outcome.archive_path.is_file(), "{}", outcome.archive_path.display());
    let mode = std::fs::metadata(&outcome.archive_path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    assert!(!backup.join("moqui-20000101T000000Z-d.dump.partial").exists());
    assert!(outcome.keep.iter().all(|kept| !kept.name.ends_with(".partial")));
    if same_week {
        assert!(!backup.join(&old_name).exists(), "{old_name} stayed");
        assert!(
            outcome.removed.iter().any(|name| name == &old_name),
            "removed {:?}",
            outcome.removed
        );
        let weekly = std::fs::read_dir(&backup)
            .unwrap()
            .flatten()
            .any(|entry| entry.file_name().to_string_lossy().contains("-w"));
        assert!(!weekly, "a same-week daily was promoted");
    }

    cluster.load_save(&outcome.archive_path).unwrap();
    let mut conn = driver.connect_postgres().unwrap();
    let rows = conn.query_once("SELECT n FROM backup_kept", &[]).unwrap();
    assert_eq!(rows[0].get("n"), Some(&Value::I32(7)));
    drop(conn);

    let combine = tool_bin(&target.bin_dir, "pg_combinebackup");
    let verify = tool_bin(&target.bin_dir, "pg_verifybackup");
    if !combine.is_file() || !verify.is_file() {
        eprintln!("skipping incremental backup: pg_combinebackup or pg_verifybackup is not in the theseus install");
        return;
    }
    let incr = base.join("backup-incr");
    let first = run(
        from_ref(&target),
        &base,
        &request(BackupMode::Incremental, incr.clone(), 7),
    )
    .unwrap();
    if first.errors.iter().any(|err| err.contains("pg_hba")) {
        assert!(
            first
                .errors
                .iter()
                .any(|err| err.contains("leaves that file unchanged")),
            "{:?}",
            first.errors
        );
        return;
    }
    assert!(first.errors.is_empty(), "{:?}", first.errors);
    assert_eq!(first.backup_kind, "full");
    assert!(first.archive_path.join("backup_manifest").is_file());
    assert!(first.archive_path.join("PG_VERSION").is_file());
    let sidecar = std::fs::read_to_string(first.archive_path.join(SIDECAR)).unwrap();
    assert!(sidecar.contains("kind=full"), "{sidecar}");
    let full_name = first.archive_path.file_name().unwrap().to_string_lossy().into_owned();

    let second = run(from_ref(&target), &base, &request(BackupMode::Incremental, incr, 7)).unwrap();
    if second.errors.iter().any(|err| err.contains("pg_hba")) {
        assert!(
            second
                .errors
                .iter()
                .any(|err| err.contains("leaves that file unchanged")),
            "{:?}",
            second.errors
        );
        return;
    }
    assert!(second.errors.is_empty(), "{:?}", second.errors);
    assert_eq!(second.backup_kind, "incremental", "{:?}", second.errors);
    let sidecar = std::fs::read_to_string(second.archive_path.join(SIDECAR)).unwrap();
    assert!(sidecar.contains("kind=incremental"), "{sidecar}");
    assert!(sidecar.contains(&format!("parent={full_name}")), "{sidecar}");

    let combined = base.join("combined");
    let output = Command::new(&combine)
        .args([
            "-o",
            combined.to_str().unwrap(),
            first.archive_path.to_str().unwrap(),
            second.archive_path.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    assert!(combined.join("PG_VERSION").is_file());
}

#[cfg(unix)]
fn week_key(stamp: Timestamp) -> (i16, i8) {
    let week = stamp.to_zoned(TimeZone::system()).date().iso_week_date();
    (week.year(), week.week())
}
