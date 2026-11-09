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
fn path_helpers_split_at_the_last_slash() {
    assert_eq!(split_relative("a/b/c.xml"), ("a/b", "c.xml"));
    assert_eq!(split_relative("c.xml"), ("", "c.xml"));
    assert_eq!(parent_path("a/b"), "a");
    assert_eq!(parent_path("a"), "");
    assert_eq!(file_tail("a/b"), "b");
    assert_eq!(file_tail(r"a\b"), "b");
    assert_eq!(file_tail("b"), "b");
    assert_eq!(file_tail("a/"), "");
    assert_eq!(strip_extension_after_slash("a/b.c/d.tar.gz"), "a/b.c/d");
    assert_eq!(strip_extension_after_slash("d.xml"), "d");
    assert_eq!(strip_extension_after_slash("a.b/"), "a.b/");
    assert_eq!(strip_extension_after_slash("noext"), "noext");
}

#[test]
fn memory_tree_finds_children_under_a_matching_directory() {
    let root = MemoryResource::new_root("memory");
    let screen = root.make_directory("screen").unwrap();
    screen.make_file("sub.xml").unwrap().write_text("<screen/>").unwrap();
    let nested = screen.make_directory("nested").unwrap();
    nested.make_file("deep.xml").unwrap().write_text("deep").unwrap();
    let screen_file = root.make_file("screen.xml").unwrap();
    screen_file.write_text("<file/>").unwrap();

    let found = screen_file.find_child_file("sub.xml");
    assert!(found.exists() && found.is_file());
    assert_eq!(found.read_text().unwrap(), "<screen/>");
    let deep = screen_file.find_child_file("deep.xml");
    assert_eq!(deep.read_text().unwrap(), "deep");
    let nested_dir = screen_file.find_child_directory("nested");
    assert!(nested_dir.is_directory());
    let missing = screen_file.find_child_file("nope.xml");
    assert!(!missing.exists());
    let children = screen_file.children().unwrap();
    assert!(children.iter().any(|child| child.file_name() == "sub.xml"));
    assert!(screen_file.is_text());
    assert_eq!(content_type_for_name("page.xml"), Some("application/xml"));
}

#[cfg(windows)]
#[test]
fn windows_file_location_uses_slashes_and_drops_the_verbatim_prefix() {
    let resource = FileResource::new(r"\\?\D:\app\screen\webroot\Login.xml");
    assert_eq!(resource.location(), "D:/app/screen/webroot/Login.xml");
    assert_eq!(resource.file_name(), "Login.xml");
    assert_eq!(
        resource.path(),
        std::path::Path::new(r"D:\app\screen\webroot\Login.xml")
    );
    let child = FileResource::new(r"\\?\D:\app\screen\webroot")
        .child("Login.xml")
        .unwrap();
    assert_eq!(child.location(), "D:/app/screen/webroot/Login.xml");
    assert_eq!(child.file_name(), "Login.xml");
}

#[test]
fn file_resource_round_trips_in_a_temp_directory() {
    let dir = std::env::temp_dir().join(format!("moqui-lib-resource-{}-{}", std::process::id(), now_millis()));
    fs::create_dir_all(&dir).unwrap();
    let root = FileResource::new(dir.to_string_lossy());
    let file = root.make_file("note.txt").unwrap();
    file.write_text("hello").unwrap();
    assert_eq!(file.read_text().unwrap(), "hello");
    assert!(file.size().unwrap() >= 5);
    assert!(file.last_modified().unwrap() > 0);
    let listed = root.children().unwrap();
    assert_eq!(listed.len(), 1);
    assert!(file.delete().unwrap());
    assert!(!file.exists());
    let _ = fs::remove_dir_all(&dir);
}

#[cfg(unix)]
#[test]
fn file_write_replaces_whole_and_delete_reports_failures() {
    use std::os::unix::fs::PermissionsExt;
    let dir = std::env::temp_dir().join(format!("moqui-lib-replace-{}-{}", std::process::id(), now_millis()));
    fs::create_dir_all(&dir).unwrap();
    let root = FileResource::new(dir.to_string_lossy());
    let file = root.make_file("data.txt").unwrap();
    file.write_text("the first and longer text").unwrap();
    fs::set_permissions(dir.join("data.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    file.write_text("short").unwrap();
    assert_eq!(file.read_text().unwrap(), "short");
    let mode = fs::metadata(dir.join("data.txt")).unwrap().permissions().mode();
    assert_eq!(mode & 0o777, 0o640);
    // No temp file is left beside it.
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);

    // Through a symlink the target is written and the link stays.
    std::os::unix::fs::symlink(dir.join("data.txt"), dir.join("link.txt")).unwrap();
    FileResource::new(dir.join("link.txt").to_string_lossy())
        .write_text("by link")
        .unwrap();
    assert!(fs::symlink_metadata(dir.join("link.txt"))
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(file.read_text().unwrap(), "by link");

    // A directory that is not empty is an error, not a quiet false; a missing file is false.
    let sub = root.make_directory("sub").unwrap();
    sub.make_file("inner.txt").unwrap();
    assert!(sub.delete().is_err());
    assert!(!FileResource::new(dir.join("missing.txt").to_string_lossy())
        .delete()
        .unwrap());
    let _ = fs::remove_dir_all(&dir);
}
