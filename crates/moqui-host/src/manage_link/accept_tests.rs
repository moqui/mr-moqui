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

fn conn(pairing: bool, helloed: bool, sealed: bool) -> Conn {
    Conn {
        helloed: AtomicBool::new(helloed),
        sealed: AtomicBool::new(sealed),
        instance: "prod-1".to_string(),
        bundle_key: Vec::new(),
        generation: 1,
        remote: "127.0.0.1:1".parse().unwrap(),
        fingerprint: "fp".to_string(),
        pairing_key: pairing.then(|| "fp".to_string()),
        close: Mutex::new(None),
        challenge: Vec::new(),
        verified: AtomicBool::new(false),
    }
}

const CALLS: [Call; 11] = [
    Call::Pair,
    Call::Hello,
    Call::Status,
    Call::Bootstrap,
    Call::Unseal,
    Call::SealedShares,
    Call::Holders,
    Call::Shares,
    Call::ShareConfirmations,
    Call::Event,
    Call::Serve,
];

fn allowed(conn: &Conn) -> Vec<Call> {
    CALLS.into_iter().filter(|call| refuse(*call, conn).is_ok()).collect()
}

#[test]
fn the_manager_answers_a_sealed_instance_only_its_sealed_calls() {
    // a pairing connection serves Pair alone
    assert_eq!(allowed(&conn(true, false, true)), [Call::Pair]);
    // a link connection: Hello first
    assert_eq!(allowed(&conn(false, false, true)), [Call::Hello]);
    // sealed: Hello and status, the bootstrap bundle, unseal by transit or by recovery shares
    assert_eq!(
        allowed(&conn(false, true, true)),
        [
            Call::Hello,
            Call::Status,
            Call::Bootstrap,
            Call::Unseal,
            Call::SealedShares
        ]
    );
    let refused = refuse(Call::Serve, &conn(false, true, true)).unwrap_err();
    assert!(refused.message().starts_with("sealed:"), "{refused:?}");
    // unsealed: every call of a link connection
    assert_eq!(
        allowed(&conn(false, true, false)),
        [
            Call::Hello,
            Call::Status,
            Call::Bootstrap,
            Call::Unseal,
            Call::SealedShares,
            Call::Holders,
            Call::Shares,
            Call::ShareConfirmations,
            Call::Event,
            Call::Serve
        ]
    );
}

#[test]
fn only_status_and_seal_run_on_a_link_whose_unsealed_the_manager_did_not_take() {
    // M6: a claim of unsealed without the manager's own unseal or the instance's proof is taken as sealed for these
    for kind in [
        CommandKind::Restart,
        CommandKind::Drain,
        CommandKind::Undrain,
        CommandKind::PutSecret,
        CommandKind::DeployComponent,
        CommandKind::ExportSeal,
        CommandKind::SurfaceReport,
        CommandKind::Approvers,
    ] {
        assert!(kind.needs_verified_unseal(), "{kind:?}");
    }
    assert!(!CommandKind::Status.needs_verified_unseal());
    assert!(!CommandKind::Seal.needs_verified_unseal());
}

#[test]
fn a_link_from_a_new_address_waits_for_an_operator_or_the_wait() {
    let (old, new): (IpAddr, IpAddr) = ("10.0.0.1".parse().unwrap(), "10.0.0.2".parse().unwrap());
    let wait = Duration::from_secs(300);
    // first seen, or from the same address: taken
    assert!(admit_address(None, None, new, wait, false).is_ok());
    assert!(admit_address(Some(new), None, new, wait, false).is_ok());
    assert!(admit_address(None, Some((new, Duration::from_secs(1))), new, wait, false).is_ok());
    // another address while its link is live, or soon after it went down: refused
    let live = admit_address(Some(old), None, new, wait, false).unwrap_err();
    assert!(live.contains("live from 10.0.0.1"), "{live}");
    assert!(admit_address(None, Some((old, Duration::from_secs(10))), new, wait, false).is_err());
    // after the wait, or once an operator confirmed (or it paired): taken
    assert!(admit_address(None, Some((old, Duration::from_secs(301))), new, wait, false).is_ok());
    assert!(admit_address(Some(old), None, new, wait, true).is_ok());
}
