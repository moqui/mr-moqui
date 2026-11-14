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

use super::{sign, verify, Attestation, Refusal, KIND};

const SECRET: &str = "a-target-issued-secret-0123456789";
const NOW: i64 = 1_800_000_000_000;
const MAX_AGE: i64 = 300_000;

pub(crate) fn attestation(issued_at: i64) -> Attestation {
    Attestation {
        kind: KIND.to_string(),
        v: 1,
        repository_url: "https://source.example.com/git/site.git".to_string(),
        ref_name: "refs/heads/main".to_string(),
        commit_id: "0123456789abcdef0123456789abcdef01234567".to_string(),
        component: "site".to_string(),
        pipeline: "release".to_string(),
        pipeline_run_id: "100".to_string(),
        run_num: 7,
        run_status: "success".to_string(),
        environment: "prod".to_string(),
        deployment_id: "D100".to_string(),
        issued_at,
        source: "https://source.example.com".to_string(),
    }
}

#[test]
fn a_good_attestation_verifies_and_a_forged_altered_stale_or_unpassed_one_does_not() {
    let signed = sign(SECRET, &attestation(NOW - 1_000)).unwrap();
    assert!(signed.signature.starts_with("sha256=") && signed.signature.len() == 71);
    let good = verify(SECRET, &signed.text, &signed.signature, NOW, MAX_AGE).unwrap();
    assert_eq!(good.branch(), "main");
    assert_eq!(good, attestation(NOW - 1_000));
    // the secret is never in what is sent
    assert!(!signed.text.contains(SECRET) && !signed.signature.contains(SECRET));

    // another secret: forged, or signed for another target
    assert_eq!(
        verify(
            "another-target-secret-0123456789",
            &signed.text,
            &signed.signature,
            NOW,
            MAX_AGE
        ),
        Err(Refusal::Signature)
    );
    // one byte of the text changed: another commit
    let altered = signed
        .text
        .replace("0123456789abcdef01234567\"", "0123456789abcdef01234568\"");
    assert_ne!(altered, signed.text);
    assert_eq!(
        verify(SECRET, &altered, &signed.signature, NOW, MAX_AGE),
        Err(Refusal::Signature)
    );
    for bad in [
        "",
        "sha256=",
        "sha256=zz",
        "md5=00",
        &signed.signature[..signed.signature.len() - 2],
    ] {
        assert_eq!(
            verify(SECRET, &signed.text, bad, NOW, MAX_AGE),
            Err(Refusal::Signature),
            "{bad}"
        );
    }
    // stale, and from the future
    let old = sign(SECRET, &attestation(NOW - MAX_AGE - 1)).unwrap();
    assert_eq!(
        verify(SECRET, &old.text, &old.signature, NOW, MAX_AGE),
        Err(Refusal::Stale)
    );
    let ahead = sign(SECRET, &attestation(NOW + 120_000)).unwrap();
    assert_eq!(
        verify(SECRET, &ahead.text, &ahead.signature, NOW, MAX_AGE),
        Err(Refusal::Future)
    );
    // signed, but of a run that did not pass, or of a tag
    let mut failed = attestation(NOW);
    failed.run_status = "failure".to_string();
    let failed = sign(SECRET, &failed).unwrap();
    assert!(matches!(
        verify(SECRET, &failed.text, &failed.signature, NOW, MAX_AGE),
        Err(Refusal::Malformed(_))
    ));
    let mut tag = attestation(NOW);
    tag.ref_name = "refs/tags/v1".to_string();
    let tag = sign(SECRET, &tag).unwrap();
    assert!(matches!(
        verify(SECRET, &tag.text, &tag.signature, NOW, MAX_AGE),
        Err(Refusal::Malformed(_))
    ));
    // a target with no secret, or a short one, takes nothing; a short secret signs nothing
    assert_eq!(
        verify("", &signed.text, &signed.signature, NOW, MAX_AGE),
        Err(Refusal::NoSecret)
    );
    assert!(sign("short", &attestation(NOW)).is_err());
}
