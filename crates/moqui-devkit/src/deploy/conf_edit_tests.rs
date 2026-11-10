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
fn a_save_replaces_the_file_whole_and_keeps_its_mode() {
    let dir = std::env::temp_dir().join(format!("moqui-conf-edit-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("MoquiConf.xml");
    std::fs::write(&path, "<moqui-conf/>").unwrap();
    // a mode other than a new file's, to see it kept
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640)).unwrap();
    }
    let permissions = std::fs::metadata(&path).unwrap().permissions();
    write_replacing(
        &path,
        "<moqui-conf>\n    <default-property name=\"a\" value=\"b\"/>\n</moqui-conf>\n",
    )
    .unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("name=\"a\""));
    assert_eq!(std::fs::metadata(&path).unwrap().permissions(), permissions);
    // no file of the save is left beside it
    let names: Vec<String> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    assert_eq!(names, ["MoquiConf.xml"]);
    assert_eq!(text_hash(&text), text_hash(&text.clone()));
    assert_ne!(text_hash(&text), text_hash("<moqui-conf/>"));
    std::fs::remove_dir_all(&dir).unwrap();
}
