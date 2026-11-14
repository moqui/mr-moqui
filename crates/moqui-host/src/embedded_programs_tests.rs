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

use std::path::Path;

use super::{copy_dir, install};

/// A gzipped tar of `entries` (path and contents), the paths written as given.
fn tar_gz(entries: &[(&str, &str)]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast()));
    for (path, text) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(text.len() as u64);
        header.set_mode(0o755);
        header.set_entry_type(tar::EntryType::Regular);
        // set_path refuses `..`; the raw name lets a test hold an entry that escapes
        let name = &mut header.as_old_mut().name;
        name[..path.len()].copy_from_slice(path.as_bytes());
        header.set_cksum();
        builder.append(&header, text.as_bytes()).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-embedded-programs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn an_archive_is_unpacked_without_its_top_directory_once() {
    let root = scratch("unpack");
    let bytes = tar_gz(&[
        ("opensearch-3.4.0/bin/opensearch", "#!/bin/sh\n"),
        ("opensearch-3.4.0/config/opensearch.yml", "cluster.name: x\n"),
    ]);
    let dir = root.join("install/opensearch/3.4.0");
    let done = |dir: &Path| dir.join("bin/opensearch").exists();
    install(&bytes, &dir, "OpenSearch", &done).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("bin/opensearch")).unwrap(),
        "#!/bin/sh\n"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.join("bin/opensearch"))
            .unwrap()
            .permissions()
            .mode();
        assert_ne!(mode & 0o111, 0, "the program stays executable");
    }
    let leftovers: Vec<_> = std::fs::read_dir(dir.parent().unwrap())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(
        leftovers,
        vec![std::ffi::OsString::from("3.4.0")],
        "no unpack directory is left"
    );

    std::fs::write(dir.join("config/opensearch.yml"), "edited").unwrap();
    install(&bytes, &dir, "OpenSearch", &done).unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("config/opensearch.yml")).unwrap(),
        "edited",
        "an install that is there is kept"
    );

    let seeded = root.join("runtime/opensearch/config");
    copy_dir(&dir.join("config"), &seeded).unwrap();
    assert_eq!(
        std::fs::read_to_string(seeded.join("opensearch.yml")).unwrap(),
        "edited"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn an_entry_outside_the_archive_directory_is_refused() {
    let root = scratch("escape");
    let bytes = tar_gz(&[("top/bin/chrome", "x"), ("top/../../escaped", "x")]);
    let dir = root.join("install/chromium/1");
    let err = install(&bytes, &dir, "Chromium", &|dir: &Path| dir.join("bin/chrome").exists())
        .unwrap_err()
        .to_string();
    assert!(err.contains("outside its directory"), "{err}");
    assert!(!dir.exists());
    assert!(!root.join("install/escaped").exists());
    let _ = std::fs::remove_dir_all(&root);
}
