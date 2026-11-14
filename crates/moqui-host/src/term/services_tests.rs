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

use super::{input_refusal, opening_refusal, MAX_INPUT};
use crate::term::session::Kind;

/// A host session's first input is a command line: capped, with no NUL.
#[test]
fn a_terminals_first_input_is_a_short_command_line() {
    assert!(input_refusal("sh '/rt/tmp/dev-aide-run/x.sh'\n").is_none());
    assert!(input_refusal(&"x".repeat(MAX_INPUT + 1)).is_some());
    assert!(input_refusal("sh x\0rm").is_some());
}

/// A command line or a host program is never opened under an agent's read fence or in a sim, whatever its input; the
/// apps are.
#[test]
fn no_host_session_opens_under_a_fence_or_in_a_sim() {
    assert!(matches!(opening_refusal(Kind::Host, "sh x\n"), Ok(None)));
    {
        let _fenced = moqui_core::entity::fence::enter("tool try_service");
        assert!(opening_refusal(Kind::Host, "sh x\n").is_err());
        assert!(opening_refusal(Kind::Command, "").is_err());
        assert!(matches!(opening_refusal(Kind::Apps, ""), Ok(None)));
    }
    let _sim = moqui_core::entity::fence::enter_sim("try_service of a test");
    assert!(opening_refusal(Kind::Host, "sh x\n").is_err());
}
