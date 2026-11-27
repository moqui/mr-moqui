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

use super::link;

/// A directory link lists what the directory holds, and a file link reads as the file, from a canonical path too (on
/// Windows one with the `\\?\` prefix).
#[test]
fn a_directory_and_a_file_link_read_as_what_they_name() {
    let root = std::env::temp_dir().join(format!("moqui-fs-link-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let real = root.join("real");
    std::fs::create_dir_all(&real).unwrap();
    std::fs::write(real.join("a.txt"), "a").unwrap();
    let real = std::fs::canonicalize(&real).unwrap();

    link(&real, &root.join("dir-link")).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("dir-link/a.txt")).unwrap(), "a");
    link(&real.join("a.txt"), &root.join("file-link.txt")).unwrap();
    assert_eq!(std::fs::read_to_string(root.join("file-link.txt")).unwrap(), "a");
    assert!(
        link(&real, &root.join("dir-link")).is_err(),
        "a link over one already there"
    );

    // removing the links leaves what they name
    std::fs::remove_file(root.join("file-link.txt")).unwrap();
    assert!(real.join("a.txt").is_file());
    std::fs::remove_dir_all(&root).unwrap();
}
