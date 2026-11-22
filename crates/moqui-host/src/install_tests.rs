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
    install_into(&dest, Some(&source), false, false, &exe).unwrap();
    assert!(moqui_core::context::layout::is_runtime(&dest));
    assert!(!dest.join("db/value").exists());
    fs::write(source.join("MoquiConf.xml"), "<moqui-conf></moqui-conf>").unwrap();
    assert!(install_into(&dest, Some(&source), true, false, &exe)
        .unwrap_err()
        .to_string()
        .contains("MoquiConf.xml"));
    assert!(!dest.join("db/value").exists());
    install_into(&dest, Some(&source), true, true, &exe).unwrap();
    install_into(&dest, Some(&source), true, false, &exe).unwrap();
    assert_eq!(fs::read(dest.join("db/value")).unwrap(), b"data");
    assert!(!dest.join("cert/dev-ca/signer/issuer.key").exists());
    fs::remove_dir_all(base).unwrap();
}
