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

use super::{finish, take, Taken};
use crate::context::ExecutionContextFactory;

/// SEC-DAT-90: a replay key is the table's, so it is taken once whatever member asks (two factories on one database
/// stand for two members): a second take is a replay until the first request is worked or given back, and a key past
/// its time is taken again.
#[test]
fn a_replay_key_is_taken_once_across_members() {
    let facade = crate::testing::open();
    let first = ExecutionContextFactory::test_context();
    first.factory().set_entity(facade.clone());
    let second = ExecutionContextFactory::test_context();
    second.factory().set_entity(facade.clone());
    let on = |ctx: &crate::context::ExecutionContext, key: &str, hold: i64| {
        let _entered = ctx.enter();
        take(key, hold).unwrap()
    };
    assert_eq!(on(&first, "remote:1:sig", 600_000), Taken::New);
    assert_eq!(
        on(&second, "remote:1:sig", 600_000),
        Taken::Working,
        "the other member's take"
    );
    {
        let _entered = second.enter();
        finish("remote:1:sig", true);
    }
    assert_eq!(on(&first, "remote:1:sig", 600_000), Taken::Done);
    // a request not kept gives its key back, so the network's retry goes through
    assert_eq!(on(&first, "remote:2:sig", 600_000), Taken::New);
    {
        let _entered = first.enter();
        finish("remote:2:sig", false);
    }
    assert_eq!(on(&second, "remote:2:sig", 600_000), Taken::New);
    // a key past its time is taken again
    assert_eq!(on(&first, "remote:3:sig", -1), Taken::New);
    assert_eq!(on(&second, "remote:3:sig", 600_000), Taken::New);
}
