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

//! Second factor codes sent at once, on embedded Postgres, whose row locks the unit tests' database does not have
//! (plans/Security-Review-Medium.md SEC-WEB-87): every wrong code is counted, none lost to another's write, and of two
//! right submissions of one single-use code only one verifies.

#![cfg(all(unix, feature = "embedded-postgres", feature = "axum"))]

use std::sync::{Arc, Barrier};

use moqui_core::context::ExecutionContextFactory;
use moqui_lib::object::Value;

use crate::common::start_with_data;

/// The codes of the burst; more than the failures an account has left (`user-facade/login@max-failures` 3, so 4).
const BURST: usize = 12;

fn data(hash: &str) -> String {
    format!(
        r#"<entity-facade-xml type="seed">
    <moqui.security.UserAccount userId="CODE_BURST" username="code.burst" userFullName="Code Burst"/>
    <moqui.security.UserAuthcFactor factorId="CODE_BURST_1" userId="CODE_BURST" factorTypeEnumId="UafSingleUse"
        factorOption="{hash}" fromDate="2020-01-01 00:00:00"/>
</entity-facade-xml>"#
    )
}

/// `body` on `threads` threads at once, each in a context of its own; their results.
fn at_once<T: Send + 'static>(
    factory: &Arc<ExecutionContextFactory>,
    threads: usize,
    body: impl Fn() -> T + Send + Sync + 'static,
) -> Vec<T> {
    let body = Arc::new(body);
    let start = Arc::new(Barrier::new(threads));
    let handles: Vec<_> = (0..threads)
        .map(|_| {
            let (factory, start, body) = (Arc::clone(factory), Arc::clone(&start), Arc::clone(&body));
            std::thread::spawn(move || {
                let context = factory.new_context();
                let _entered = context.enter();
                start.wait();
                body()
            })
        })
        .collect();
    handles.into_iter().map(|handle| handle.join().unwrap()).collect()
}

fn account_field(factory: &Arc<ExecutionContextFactory>, field: &str) -> Value {
    let context = factory.new_context();
    let _entered = context.enter();
    let _authz = context.disable_authz();
    let account = factory
        .entity()
        .unwrap()
        .find("moqui.security.UserAccount")
        .unwrap()
        .eq("userId", Value::string("CODE_BURST"))
        .one()
        .unwrap()
        .expect("the account");
    account.get(field).cloned().unwrap_or(Value::Null)
}

#[test]
fn codes_sent_at_once_are_each_counted_and_a_right_code_taken_once() {
    if !crate::common::own_process() {
        return;
    }
    let hash = moqui_core::user::single_use_hash("70316001").unwrap();
    let gate = start_with_data("second-factor-codes", &data(&hash));
    let factory = Arc::clone(gate.server.factory());

    let right = at_once(&factory, 2, || {
        moqui_core::user::check_counted_code("CODE_BURST", "70316001").unwrap()
    });
    assert_eq!(right.iter().flatten().count(), 1, "{right:?}");
    let counted = moqui_lib::object::to_plain_string(&account_field(&factory, "successiveFailedLogins"));
    assert_eq!(counted, "1", "the second counted");

    let wrong = at_once(&factory, BURST, || {
        moqui_core::user::check_counted_code("CODE_BURST", "22222222").unwrap()
    });
    assert!(wrong.iter().all(Option::is_none));
    let counted = moqui_lib::object::to_plain_string(&account_field(&factory, "successiveFailedLogins"));
    assert_eq!(counted, (BURST + 1).to_string(), "each code counted once");
    assert_eq!(
        moqui_lib::object::to_plain_string(&account_field(&factory, "disabled")),
        "Y"
    );
}
