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
        pairing_key: pairing.then(|| "fp".to_string()),
        close: Mutex::new(None),
    }
}

const CALLS: [Call; 7] = [
    Call::Pair,
    Call::Hello,
    Call::Status,
    Call::Bootstrap,
    Call::Unseal,
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
    // sealed: Hello and status, the bootstrap bundle, unseal
    assert_eq!(
        allowed(&conn(false, true, true)),
        [Call::Hello, Call::Status, Call::Bootstrap, Call::Unseal]
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
            Call::Event,
            Call::Serve
        ]
    );
}
