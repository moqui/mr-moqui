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

fn verify(
    root: &CertificateDer<'_>,
    issuer: &CertificateDer<'_>,
    leaf: &CertificateDer<'_>,
    name: &'static str,
) -> Result<(), rustls::Error> {
    use rustls::client::danger::ServerCertVerifier as _;
    let mut roots = rustls::RootCertStore::empty();
    roots.add(root.clone().into_owned())?;
    let verifier = rustls::client::WebPkiServerVerifier::builder_with_provider(
        std::sync::Arc::new(roots),
        std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
    )
    .build()
    .unwrap();
    verifier
        .verify_server_cert(
            leaf,
            std::slice::from_ref(issuer),
            &rustls::pki_types::ServerName::try_from(name).unwrap(),
            &[],
            rustls::pki_types::UnixTime::now(),
        )
        .map(|_| ())
}

#[test]
fn the_retained_signing_key_can_only_issue_loopback_server_identities() {
    let dir = private_fs::new_dir_in(&std::env::temp_dir(), "moqui-dev-issuer").unwrap();
    let shared = dir.join("shared");
    let runtime = dir.join("checkout/cert");
    std::thread::scope(|scope| {
        for _ in 0..2 {
            scope.spawn(|| ensure(&runtime, Some(shared.clone()), true).unwrap());
        }
    });
    let second = dir.join("second/cert");
    ensure(&second, Some(shared.clone()), false).unwrap();
    let signer = shared.join("cert/dev-ca/signer");
    assert!(!signer.join("ca.key").exists());
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(
            fs::metadata(signer.join("issuer.key")).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let root = CertificateDer::from_pem_file(runtime.join("dev-ca/ca.crt")).unwrap();
    let issuer = CertificateDer::from_pem_file(signer.join("issuer.crt")).unwrap();
    for checkout in [&runtime, &second] {
        let leaf = CertificateDer::from_pem_file(checkout.join("localhost.crt")).unwrap();
        for host in ["localhost", "127.0.0.1", "::1"] {
            verify(&root, &issuer, &leaf, host).unwrap();
        }
        assert!(!checkout.join("dev-ca/signer/issuer.key").exists());
    }
    let before = fs::read_to_string(second.join("localhost.crt")).unwrap();
    let renewal = UNIX_EPOCH + Duration::from_secs(until(&before).unwrap() - 29 * DAY);
    issue(&second, &shared.join("cert/dev-ca"), false, renewal).unwrap();
    assert_ne!(fs::read_to_string(second.join("localhost.crt")).unwrap(), before);
    let key = PrivateKeyDer::from_pem_file(signer.join("issuer.key")).unwrap();
    let signer = Issuer::new(authority(true), KeyPair::try_from(&key).unwrap());
    let other = CertificateParams::new(vec!["outside.example".into()])
        .unwrap()
        .signed_by(&KeyPair::generate().unwrap(), &signer)
        .unwrap();
    assert!(verify(&root, &issuer, other.der(), "outside.example").is_err());
    let manual = "custom material";
    fs::write(runtime.join("default.crt"), manual).unwrap();
    assert!(ensure(&runtime, Some(shared), false).is_err());
    assert_eq!(fs::read_to_string(runtime.join("default.crt")).unwrap(), manual);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn an_older_self_signed_pair_is_refused_naming_replace_and_replace_moves_it_aside() {
    let dir = private_fs::new_dir_in(&std::env::temp_dir(), "moqui-dev-replace").unwrap();
    let runtime = dir.join("checkout/cert");
    fs::create_dir_all(&runtime).unwrap();
    fs::write(runtime.join("localhost.crt"), "old self-signed").unwrap();
    fs::write(runtime.join("localhost.key"), "old key").unwrap();
    // establishing the shared CA (./moq trust-dev-cert) fails, naming the way out
    let err = ensure(&runtime, Some(dir.join("shared")), true)
        .unwrap_err()
        .to_string();
    assert!(err.contains("moqui dev-cert --replace"), "{err}");
    assert_eq!(
        fs::read_to_string(runtime.join("localhost.crt")).unwrap(),
        "old self-signed"
    );

    let replace = Options {
        establish: true,
        replace: true,
        rotate: false,
    };
    ensure_with(&runtime, Some(dir.join("shared")), replace).unwrap();
    let aside: Vec<_> = fs::read_dir(&runtime)
        .unwrap()
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("replaced-"))
        .collect();
    assert_eq!(aside.len(), 1);
    assert_eq!(
        fs::read_to_string(aside[0].path().join("localhost.key")).unwrap(),
        "old key"
    );
    let leaf = fs::read_to_string(runtime.join("localhost.crt")).unwrap();
    let issuer = fs::read_to_string(dir.join("shared/cert/dev-ca/signer/issuer.crt")).unwrap();
    assert!(leaf.ends_with(&issuer), "a managed leaf");
    // a managed pair is not moved again
    ensure_with(&runtime, Some(dir.join("shared")), replace).unwrap();
    assert_eq!(fs::read_to_string(runtime.join("localhost.crt")).unwrap(), leaf);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn the_issuer_lives_two_years_promotion_leaves_one_signer_and_rotate_makes_a_new_root() {
    let dir = private_fs::new_dir_in(&std::env::temp_dir(), "moqui-dev-rotate").unwrap();
    let runtime = dir.join("checkout/cert");
    let shared = dir.join("shared");
    ensure(&runtime, None, false).unwrap();
    assert!(runtime.join("dev-ca/signer/issuer.key").is_file());
    let issuer = fs::read_to_string(runtime.join("dev-ca/signer/issuer.crt")).unwrap();
    let root = fs::read_to_string(runtime.join("dev-ca/ca.crt")).unwrap();
    let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    let issuer_days = (until(&issuer).unwrap() - now) / DAY;
    assert!((725..=730).contains(&issuer_days), "{issuer_days}");
    assert!((until(&root).unwrap() - now) / DAY > 3600);

    // the checkout's signer moves to the shared runtime, and its own copy of the key is gone
    ensure(&runtime, Some(shared.clone()), true).unwrap();
    assert!(!runtime.join("dev-ca/signer").exists());
    assert_eq!(
        fs::read_to_string(shared.join("cert/dev-ca/signer/issuer.crt")).unwrap(),
        issuer
    );

    let rotate = Options {
        rotate: true,
        ..Options::default()
    };
    ensure_with(&runtime, Some(shared.clone()), rotate).unwrap();
    let new_root = fs::read_to_string(shared.join("cert/dev-ca/ca.crt")).unwrap();
    assert_ne!(new_root, root);
    assert_eq!(fs::read_to_string(runtime.join("dev-ca/ca.crt")).unwrap(), new_root);
    let new_issuer = fs::read_to_string(shared.join("cert/dev-ca/signer/issuer.crt")).unwrap();
    assert!(fs::read_to_string(runtime.join("localhost.crt"))
        .unwrap()
        .ends_with(&new_issuer));
    fs::remove_dir_all(dir).unwrap();
}
