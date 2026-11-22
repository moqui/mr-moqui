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
fn install_preflights_conflicts_and_keeps_signers_private() {
    let base = private_fs::new_dir_in(&std::env::temp_dir(), "moqui-install").unwrap();
    let source = base.join("source");
    let dest = base.join("installed");
    for dir in ["components/base", "cert/dev-ca/signer", "db"] {
        fs::create_dir_all(source.join(dir)).unwrap();
    }
    let exe = base.join("binary");
    fs::write(&exe, "binary").unwrap();
    fs::write(source.join("cert/dev-ca/signer/issuer.key"), "private").unwrap();
    fs::write(source.join("db/value"), "data").unwrap();
    fs::write(source.join("MoquiConf.xml"), "<moqui-conf/>").unwrap();
    install_into(&dest, Some(&source), false, false, &exe, "v1").unwrap();
    assert!(moqui_core::context::layout::is_runtime(&dest));
    assert!(!dest.join("db/value").exists());
    fs::write(source.join("MoquiConf.xml"), "<moqui-conf></moqui-conf>").unwrap();
    assert!(install_into(&dest, Some(&source), true, false, &exe, "v1")
        .unwrap_err()
        .to_string()
        .contains("MoquiConf.xml"));
    assert!(!dest.join("db/value").exists());
    install_into(&dest, Some(&source), true, true, &exe, "v1").unwrap();
    install_into(&dest, Some(&source), true, false, &exe, "v1").unwrap();
    assert_eq!(fs::read(dest.join("db/value")).unwrap(), b"data");
    assert!(!dest.join("cert/dev-ca/signer/issuer.key").exists());
    fs::remove_dir_all(base).unwrap();
}

#[test]
fn a_new_install_removes_the_files_its_source_no_longer_has_and_records_the_version() {
    let base = private_fs::new_dir_in(&std::env::temp_dir(), "moqui-install-prune").unwrap();
    let source = base.join("source");
    let dest = base.join("installed");
    fs::create_dir_all(source.join("components/base/old")).unwrap();
    fs::create_dir_all(source.join("db")).unwrap();
    let exe = base.join("binary");
    fs::write(&exe, "binary").unwrap();
    fs::write(source.join("components/base/old/gone.xml"), "old").unwrap();
    fs::write(source.join("components/base/kept.xml"), "kept").unwrap();
    fs::write(source.join("db/value"), "data").unwrap();
    fs::write(source.join("db/instance_id"), "source-id").unwrap();
    install_into(&dest, Some(&source), true, false, &exe, "v1").unwrap();
    assert!(dest.join("components/base/old/gone.xml").is_file());
    assert!(
        !dest.join("db/instance_id").exists(),
        "the copy gets an instance id of its own"
    );
    assert!(check_version_of(&dest, "v1").is_ok());
    assert!(check_version_of(&dest, "v2").unwrap_err().to_string().contains("v1"));

    // gone from the source: removed with the directory it leaves empty; the data is not pruned
    fs::remove_dir_all(source.join("components/base/old")).unwrap();
    fs::remove_dir_all(source.join("db")).unwrap();
    install_into(&dest, Some(&source), false, false, &exe, "v2").unwrap();
    assert!(!dest.join("components/base/old").exists());
    assert!(dest.join("components/base/kept.xml").is_file());
    assert_eq!(fs::read(dest.join("db/value")).unwrap(), b"data");
    assert!(check_version_of(&dest, "v2").is_ok());

    // a file changed since it was installed is a conflict before it is removed
    fs::write(source.join("components/base/extra.xml"), "extra").unwrap();
    install_into(&dest, Some(&source), false, false, &exe, "v2").unwrap();
    fs::write(dest.join("components/base/extra.xml"), "edited").unwrap();
    fs::remove_file(source.join("components/base/extra.xml")).unwrap();
    let err = install_into(&dest, Some(&source), false, false, &exe, "v2").unwrap_err();
    assert!(err.to_string().contains("extra.xml"), "{err}");
    install_into(&dest, Some(&source), false, true, &exe, "v2").unwrap();
    assert!(!dest.join("components/base/extra.xml").exists());
    fs::remove_dir_all(base).unwrap();
}

#[cfg(unix)]
#[test]
fn a_symbolic_link_is_left_out_with_a_warning_and_a_running_source_refuses_with_data() {
    let base = private_fs::new_dir_in(&std::env::temp_dir(), "moqui-install-link").unwrap();
    let source = base.join("source");
    let dest = base.join("installed");
    fs::create_dir_all(source.join("components/base")).unwrap();
    fs::create_dir_all(source.join("db/embedded/data")).unwrap();
    let exe = base.join("binary");
    fs::write(&exe, "binary").unwrap();
    fs::write(source.join("components/base/real.xml"), "real").unwrap();
    std::os::unix::fs::symlink(base.join("elsewhere"), source.join("components/base/linked")).unwrap();
    let warnings = install_into(&dest, Some(&source), false, false, &exe, "v1").unwrap();
    assert!(dest.join("components/base/real.xml").is_file());
    assert!(!dest.join("components/base/linked").exists());
    assert!(warnings.iter().any(|w| w.contains("symbolic link")), "{warnings:?}");

    fs::write(
        source.join("db/embedded/data/postmaster.pid"),
        format!(
            "{}
/data
",
            std::process::id()
        ),
    )
    .unwrap();
    let err = install_into(&dest, Some(&source), true, false, &exe, "v1").unwrap_err();
    assert!(err.to_string().contains("stopped runtime"), "{err}");
    fs::remove_dir_all(base).unwrap();
}
