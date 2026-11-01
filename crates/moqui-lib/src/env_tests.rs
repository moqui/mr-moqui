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
fn property_beats_environment_and_underscore_falls_through_to_dots() {
    let key = "moqui_lib_env_probe_alpha";
    let dotted = "moqui.lib.env.probe.beta";
    clear_property(key);
    set_property(dotted, "from-dotted");
    assert_eq!(get_prop_or_env(key), "");
    assert_eq!(get_prop_or_env("moqui_lib_env_probe_beta"), "from-dotted");
    set_property(key, "from-prop");
    assert_eq!(get_prop_or_env(key), "from-prop");
    set_property(key, "");
    assert_eq!(get_prop_or_env(key), "");
    clear_property(key);
    clear_property(dotted);
}

#[test]
fn environment_is_visible_when_no_property_is_set() {
    let path = get_prop_or_env("PATH");
    assert!(!path.is_empty());
}

#[test]
fn dotenv_strips_one_pair_of_matching_quotes() {
    let pairs = parse_dotenv("A=\"x y\"\nB='q'\nC=\"\"\nD=\"\nE='mixed\"\nF= plain \nG=\"'in'\"");
    let pairs: Vec<(&str, &str)> = pairs
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect();
    assert_eq!(
        pairs,
        [
            ("A", "x y"),
            ("B", "q"),
            ("C", ""),
            ("D", "\""),
            ("E", "'mixed\""),
            ("F", "plain"),
            ("G", "'in'")
        ]
    );
}

#[test]
fn expand_replaces_names_only() {
    set_property("moqui_version", "3");
    set_property("nested.name", "inner");
    assert_eq!(expand("plain").unwrap(), "plain");
    assert_eq!(expand("v${moqui_version}").unwrap(), "v3");
    assert_eq!(expand("${missing_name_zz}").unwrap(), "");
    assert_eq!(expand("${nested.name}").unwrap(), "inner");
    assert_eq!(expand("${1+2}").unwrap(), "${1+2}");
    assert_eq!(expand("$${moqui_version}").unwrap(), "$3");
    assert_eq!(expand("${moqui_version").unwrap(), "${moqui_version");
    clear_property("moqui_version");
    clear_property("nested.name");
}

#[test]
fn expand_takes_the_first_term_that_is_not_empty() {
    set_property("moqui_lib_env_port", "5433");
    set_property("moqui_lib_env_zone", "America/Denver");
    assert_eq!(expand("${moqui_lib_env_port?:'3306'}").unwrap(), "5433");
    assert_eq!(expand("${moqui_lib_env_none_zz?:'3306'}").unwrap(), "3306");
    assert_eq!(
        expand("${moqui_lib_env_none_zz ?: moqui_lib_env_zone}").unwrap(),
        "America/Denver"
    );
    assert_eq!(
        expand("${moqui_lib_env_none_zz ?: moqui_lib_env_none_yy ?: \"x\"}").unwrap(),
        "x"
    );
    assert_eq!(expand("${moqui_lib_env_none_zz ?: moqui_lib_env_none_yy}").unwrap(), "");
    assert_eq!(expand("${a ?: 1+2}").unwrap(), "${a ?: 1+2}");
    assert_eq!(expand("${a ?: 'x}").unwrap(), "${a ?: 'x}");
    clear_property("moqui_lib_env_port");
    clear_property("moqui_lib_env_zone");
}

#[test]
fn dotenv_loses_to_a_property_and_to_the_environment() {
    load_dotenv("MOQUI_LIB_DOTENV_PROBE=from-file\nexport MOQUI_LIB_DOTENV_QUOTED=\"quoted value\"\n# comment\n");
    assert_eq!(get_prop_or_env("MOQUI_LIB_DOTENV_PROBE"), "from-file");
    assert_eq!(get_prop_or_env("MOQUI_LIB_DOTENV_QUOTED"), "quoted value");
    set_property("MOQUI_LIB_DOTENV_PROBE", "from-prop");
    assert_eq!(get_prop_or_env("MOQUI_LIB_DOTENV_PROBE"), "from-prop");
    clear_property("MOQUI_LIB_DOTENV_PROBE");
    let path = std::env::var("PATH").unwrap_or_default();
    load_dotenv(&format!("PATH={path}-not-this"));
    assert_eq!(get_prop_or_env("PATH"), path);
}

fn temp_env(label: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("moqui-lib-env-{label}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(".env")
}

#[test]
fn update_dotenv_file_replaces_in_place_appends_and_removes() {
    let path = temp_env("update");
    std::fs::write(
        &path,
        "# kept\nmoqui_lib_upd_a=old\nexport moqui_lib_upd_b=gone\nother=1\nmoqui_lib_upd_a=dup",
    )
    .unwrap();
    let set = [
        ("moqui_lib_upd_a".to_string(), "new".to_string()),
        ("moqui_lib_upd_c".to_string(), "added".to_string()),
    ];
    update_dotenv_file(&path, &set, &["moqui_lib_upd_b"]).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert_eq!(text, "# kept\nmoqui_lib_upd_a=new\nother=1\nmoqui_lib_upd_c=added\n");
    assert_eq!(get_prop_or_env("moqui_lib_upd_c"), "added");
    update_dotenv_file(&path, &[], &["moqui_lib_upd_a", "moqui_lib_upd_c"]).unwrap();
    assert_eq!(get_prop_or_env("moqui_lib_upd_c"), "");
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "# kept\nother=1\n");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[test]
fn update_dotenv_file_starts_a_new_file_with_the_header_and_refuses_line_breaks() {
    let path = temp_env("new");
    let set = [("moqui_lib_upd_new".to_string(), "v".to_string())];
    update_dotenv_file(&path, &set, &[]).unwrap();
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        format!("{DOTENV_HEADER}moqui_lib_upd_new=v\n")
    );
    let bad = [("moqui_lib_upd_bad".to_string(), "secret\nx=y".to_string())];
    let err = update_dotenv_file(&path, &bad, &[]).unwrap_err().to_string();
    assert!(!err.contains("secret"), "{err}");
    let dir = path.parent().unwrap().join("as-dir");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(update_dotenv_file(&dir, &set, &[])
        .unwrap_err()
        .to_string()
        .contains("directory"));
    let _ = std::fs::remove_dir_all(path.parent().unwrap());
}

#[cfg(unix)]
#[test]
fn update_dotenv_file_keeps_a_symlinked_env_a_symlink() {
    let path = temp_env("link");
    let dir = path.parent().unwrap().to_path_buf();
    let target = dir.join("secrets.env");
    std::fs::write(&target, "moqui_lib_upd_l=old\n").unwrap();
    std::os::unix::fs::symlink(&target, &path).unwrap();
    let set = [("moqui_lib_upd_l".to_string(), "new".to_string())];
    update_dotenv_file(&path, &set, &[]).unwrap();
    assert!(path.is_symlink());
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "moqui_lib_upd_l=new\n");
    let leftovers: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().contains(".part-"))
        .collect();
    assert!(leftovers.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}
