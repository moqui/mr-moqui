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

use moqui_lib::object::Value;

use crate::context::{ExecutionContext, ExecutionContextFactory};
use crate::user::session::{current_user_id, set_user};

fn authz_off() -> bool {
    ExecutionContext::with_current(ExecutionContext::authz_disabled).unwrap_or(false)
}

/// The work runs with no user and authorization off; the caller's user and authorization come back after it returns
/// and after it panics, and the errors it added reach the caller.
#[test]
fn as_system_runs_without_the_user_and_puts_the_caller_back() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    set_user("SYS_CALLER", "caller");
    context.set_effective_time(Some(1_000));
    assert!(!authz_off());
    let inside = super::as_system(|| {
        ExecutionContext::with_current(|ec| ec.message().add_error("written as the system"));
        (
            current_user_id().unwrap(),
            authz_off(),
            ExecutionContext::with_current(ExecutionContext::effective_time).flatten(),
        )
    })
    .unwrap();
    assert_eq!(inside, (None, true, Some(1_000)));
    assert_eq!(current_user_id().unwrap().as_deref(), Some("SYS_CALLER"));
    assert!(!authz_off());
    assert_eq!(context.message().errors(), ["written as the system"]);

    let panicked = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let _ = super::as_system(|| panic!("the work failed"));
    }));
    assert!(panicked.is_err());
    assert_eq!(current_user_id().unwrap().as_deref(), Some("SYS_CALLER"));
    assert!(!authz_off());
}

const TRY_SYSTEM: &str = "(function () { try { return ec.artifactExecution.runAsSystem(() => 'allowed') } \
                          catch (e) { return 'refused: ' + e.message } })()";

/// A script reaches the system scope only from a service's own code: not with no artifact frame, not under a screen's
/// frame, and not in a render a service began.
#[test]
fn run_as_system_is_only_for_a_service_s_own_code() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    set_user("SYS_CALLER", "caller");
    let attempt = || crate::script::eval_value(TRY_SYSTEM).unwrap();
    assert!(
        attempt()
            .as_str()
            .is_some_and(|text| text.starts_with("refused: runAsSystem is only")),
        "{:?}",
        attempt()
    );
    context.push("AT_XML_SCREEN", "component://test/screen/Page.xml", "view");
    assert_ne!(attempt(), Value::string("allowed"));
    context.push("AT_SERVICE", "test.Services.save#Page", "update");
    assert_eq!(attempt(), Value::string("allowed"));
    context.pop().unwrap();
    context.pop().unwrap();
}

/// In the scope a script sees no user and authorization off; a throw in the body reaches the caller and leaves the
/// caller's user and authorization as they were.
#[test]
fn run_as_system_restores_the_caller_after_a_throw() {
    let context = ExecutionContextFactory::test_context();
    let _entered = context.enter();
    set_user("SYS_CALLER", "caller");
    context.push("AT_SERVICE", "test.Services.save#Page", "update");
    let seen = crate::script::eval_value(
        "(function () {
            const inside = ec.artifactExecution.runAsSystem(() =>
                (ec.user.userId || 'none') + ',' + ec.artifactExecution.authzDisabled())
            let caught = ''
            try { ec.artifactExecution.runAsSystem(() => { throw new Error('boom') }) } catch (e) { caught = e.message }
            return [inside, caught, ec.user.userId, ec.artifactExecution.authzDisabled()].join('|')
        })()",
    )
    .unwrap();
    assert_eq!(seen, Value::string("none,true|boom|SYS_CALLER|false"));
    context.pop().unwrap();
}
