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
use std::time::{Duration, SystemTime};

use super::*;

/// A process id no Linux process has (above the largest `pid_max`).
const DEAD: u32 = 4_194_999;

fn made(dir: &Path, name: &str, age: Duration) -> PathBuf {
    let path = dir.join(name);
    std::fs::create_dir_all(&path).unwrap();
    let when = SystemTime::now() - age;
    std::fs::File::open(&path).unwrap().set_modified(when).unwrap();
    path
}

#[test]
fn the_process_id_is_the_second_number_from_the_end() {
    assert_eq!(pid_of("incoming-1234-7"), Some(1234));
    assert_eq!(pid_of("gc-99-1795128529823"), Some(99));
    assert_eq!(pid_of("git-repo-2-incoming-55-0"), Some(55));
    assert_eq!(pid_of("git-import-repo-2-77-1795128529823"), Some(77));
    assert_eq!(pid_of("incoming-x-7"), None);
    assert_eq!(pid_of("pack"), None);
}

/// SEC-GIT-92: a quarantine or scratch directory a dead process left, untouched for an hour, is removed; this process's,
/// a fresh one, the archives' and anything else are not.
#[test]
fn what_a_dead_process_left_an_hour_ago_is_removed() {
    let root = std::env::temp_dir().join(format!("moqui-git-leftovers-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let objects = root.join("git").join("repo").join("objects");
    let tmp = root.join("tmp");
    let old = LEFT_FOR + Duration::from_secs(60);
    let dead_quarantine = made(&objects, &format!("incoming-{DEAD}-0"), old);
    let dead_gc = made(&objects, &format!("gc-{DEAD}-1795128529823"), old);
    let own = made(&objects, &format!("incoming-{}-0", std::process::id()), old);
    let fresh = made(&objects, &format!("incoming-{DEAD}-1"), Duration::from_secs(60));
    let pack = made(&objects, "pack", old);
    let dead_db = made(&tmp, &format!("git-repo-incoming-{DEAD}-2"), old);
    let dead_import = made(&tmp, &format!("git-import-repo-{DEAD}-1795128529823"), old);
    let archive = made(&tmp, "git-archive", old);
    // old at its top, but a nested file is being written: a long import is not removed under it
    let busy = objects.join(format!("incoming-{DEAD}-3"));
    std::fs::create_dir_all(busy.join("deep").join("deeper")).unwrap();
    std::fs::write(busy.join("deep").join("deeper").join("part"), b"being written").unwrap();
    std::fs::File::open(&busy)
        .unwrap()
        .set_modified(SystemTime::now() - old)
        .unwrap();
    std::fs::File::open(busy.join("deep"))
        .unwrap()
        .set_modified(SystemTime::now() - old)
        .unwrap();
    sweep_at_start(&root);
    for gone in [&dead_quarantine, &dead_gc, &dead_db, &dead_import] {
        assert!(!gone.exists(), "{} is removed", gone.display());
    }
    for kept in [&own, &fresh, &pack, &archive, &busy] {
        assert!(kept.exists(), "{} is kept", kept.display());
    }
    std::fs::remove_dir_all(&root).unwrap();
}
