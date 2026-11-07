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
use crate::script::host::test_class::{EXPECTED, TEST};

/// The test class answers each script as `script/oxide/bind_tests.rs` expects the oxide binder to: conversions, names
/// and lengths, getters, the shim, builders, callbacks, and the errors thrown.
#[test]
fn the_quickjs_binder_answers_the_host_table_scripts() -> Result<(), Error> {
    with_ctx(|ctx| {
        let test = instance(ctx, HostObject::new(()), &TEST, Prototypes::Running).map_err(js_error)?;
        let ec: Object = ctx.globals().get("ec").map_err(js_error)?;
        ec.set("test", test).map_err(js_error)?;
        for (script, expected) in EXPECTED {
            let got: String = ctx
                .eval(format!("String({script})"))
                .catch(ctx)
                .map_err(|err| Error::new(format!("{script}: {err}")))?;
            assert_eq!(&got, expected, "{script}");
        }
        Ok(())
    })
}
