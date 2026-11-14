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

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::{
    exists, is_dir, is_file, is_from_overlay, modified, mount, normalize, open, read, read_dir, read_to_string,
    unmount, Overlay,
};

/// Files in memory, by `/`-separated path.
struct Files(BTreeMap<&'static str, &'static str>);

impl Files {
    fn new(files: &[(&'static str, &'static str)]) -> Arc<Self> {
        Arc::new(Self(files.iter().copied().collect()))
    }
}

impl Overlay for Files {
    fn read(&self, path: &str) -> std::io::Result<Option<Vec<u8>>> {
        Ok(self.0.get(path).map(|text| text.as_bytes().to_vec()))
    }

    fn is_file(&self, path: &str) -> bool {
        self.0.contains_key(path)
    }

    fn is_dir(&self, path: &str) -> bool {
        let prefix = if path.is_empty() {
            String::new()
        } else {
            format!("{path}/")
        };
        self.0.keys().any(|key| key.starts_with(&prefix))
    }

    fn children(&self, dir: &str) -> Vec<(String, bool)> {
        let prefix = if dir.is_empty() {
            String::new()
        } else {
            format!("{dir}/")
        };
        let mut children: BTreeMap<String, bool> = BTreeMap::new();
        for key in self.0.keys() {
            if let Some(rest) = key.strip_prefix(&prefix) {
                match rest.split_once('/') {
                    Some((name, _)) => children.insert(name.to_string(), true),
                    None => children.insert(rest.to_string(), false),
                };
            }
        }
        children.into_iter().collect()
    }
}

/// An empty directory of its own for one test, unmounted and removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("moqui-vfs-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        unmount(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn names(dir: &Path) -> Vec<(String, bool)> {
    read_dir(dir)
        .unwrap()
        .into_iter()
        .map(|entry| (entry.file_name().to_string_lossy().into_owned(), entry.is_dir()))
        .collect()
}

#[test]
fn a_file_not_on_disk_is_read_from_the_overlay_and_one_on_disk_wins() {
    let scratch = Scratch::new("read");
    let root = &scratch.0;
    std::fs::create_dir_all(root.join("conf")).unwrap();
    std::fs::write(root.join("conf/Mine.xml"), "disk").unwrap();
    mount(
        root,
        Files::new(&[
            ("conf/Mine.xml", "built in"),
            ("conf/Other.xml", "other"),
            ("data/Seed.xml", "seed"),
        ]),
    );
    assert_eq!(read_to_string(root.join("conf/Mine.xml")).unwrap(), "disk");
    assert_eq!(read_to_string(root.join("conf/Other.xml")).unwrap(), "other");
    assert_eq!(read(root.join("data/Seed.xml")).unwrap(), b"seed");
    let mut text = String::new();
    open(root.join("data/Seed.xml"))
        .unwrap()
        .read_to_string(&mut text)
        .unwrap();
    assert_eq!(text, "seed");
    assert_eq!(
        read(root.join("conf/None.xml")).unwrap_err().kind(),
        std::io::ErrorKind::NotFound
    );

    assert!(is_file(root.join("conf/Other.xml")));
    assert!(is_file(root.join("conf/Mine.xml")));
    assert!(!is_file(root.join("data")));
    assert!(is_dir(root.join("data")));
    assert!(exists(root.join("data/Seed.xml")));
    assert!(!exists(root.join("data/None.xml")));
    assert!(is_from_overlay(root.join("conf/Other.xml")));
    assert!(!is_from_overlay(root.join("conf/Mine.xml")));
    assert!(modified(root.join("conf/Mine.xml")).is_some());
    assert_eq!(modified(root.join("conf/Other.xml")), None);
}

#[test]
fn a_directory_lists_the_disk_and_the_overlay_together() {
    let scratch = Scratch::new("list");
    let root = &scratch.0;
    std::fs::create_dir_all(root.join("conf/edge")).unwrap();
    std::fs::write(root.join("conf/Mine.xml"), "disk").unwrap();
    mount(
        root,
        Files::new(&[
            ("conf/Mine.xml", "x"),
            ("conf/Dev.xml", "x"),
            ("conf/edge/Edge.xml", "x"),
            ("conf/front/F.xml", "x"),
        ]),
    );
    assert_eq!(
        names(&root.join("conf")),
        vec![
            ("Dev.xml".to_string(), false),
            ("Mine.xml".to_string(), false),
            ("edge".to_string(), true),
            ("front".to_string(), true),
        ]
    );
    assert_eq!(names(&root.join("conf/front")), vec![("F.xml".to_string(), false)]);
    assert_eq!(
        read_dir(root.join("conf/front")).unwrap()[0].path(),
        root.join("conf/front/F.xml")
    );
    assert!(read_dir(root.join("nothing")).is_err());
}

#[test]
fn a_mount_below_a_directory_makes_its_way_there_a_directory() {
    let scratch = Scratch::new("below");
    let root = &scratch.0;
    mount(
        &root.join("components/base/framework"),
        Files::new(&[("component.xml", "<component/>")]),
    );
    assert!(is_dir(root.join("components")));
    assert!(is_dir(root.join("components/base/framework")));
    assert_eq!(names(&root.join("components")), vec![("base".to_string(), true)]);
    assert_eq!(
        names(&root.join("components/base")),
        vec![("framework".to_string(), true)]
    );
    assert!(is_file(root.join("components/base/framework/component.xml")));
    unmount(&root.join("components/base/framework"));
    assert!(!is_dir(root.join("components")));
}

#[test]
fn a_path_is_matched_by_name_whatever_way_it_is_written() {
    let scratch = Scratch::new("paths");
    let root = &scratch.0;
    std::fs::create_dir_all(root.join("conf")).unwrap();
    mount(root, Files::new(&[("conf/Dev.xml", "dev")]));
    assert_eq!(read_to_string(root.join("conf/./../conf/Dev.xml")).unwrap(), "dev");
    assert_eq!(normalize(Path::new("/a/./b/../c")), PathBuf::from("/a/c"));
    assert!(normalize(Path::new("rel")).is_absolute());
}

#[test]
fn two_mounts_keep_to_their_own_directories_and_a_remount_replaces() {
    let first = Scratch::new("first");
    let second = Scratch::new("second");
    mount(&first.0, Files::new(&[("a.txt", "first")]));
    mount(&second.0, Files::new(&[("a.txt", "second")]));
    assert_eq!(read_to_string(first.0.join("a.txt")).unwrap(), "first");
    assert_eq!(read_to_string(second.0.join("a.txt")).unwrap(), "second");
    mount(&first.0, Files::new(&[("b.txt", "again")]));
    assert!(!is_file(first.0.join("a.txt")));
    assert_eq!(read_to_string(first.0.join("b.txt")).unwrap(), "again");
    assert!(unmount(&second.0));
    assert!(!unmount(&second.0));
    assert!(!is_file(second.0.join("a.txt")));
}
