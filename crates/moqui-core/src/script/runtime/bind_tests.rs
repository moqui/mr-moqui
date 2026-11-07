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
use crate::script::host::test_class::{EXPECTED, TEST_PATHS};

/// The test class answers each script as `script/oxide/bind_tests.rs` expects the oxide binder to: conversions, names
/// and lengths, getters, the shim, builders, callbacks, and the errors thrown.
#[test]
fn the_quickjs_binder_answers_the_host_table_scripts() -> Result<(), Error> {
    with_ctx(|ctx| {
        let ec: Object = ctx.globals().get("ec").map_err(js_error)?;
        install_paths(ctx, &ec, TEST_PATHS)?;
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

/// `ec.web`'s methods written in its shim keep the `name` and `length` they had as methods of the QuickJS class, and a
/// call with too few arguments is refused as a typed method's was.
#[test]
fn the_web_shim_methods_keep_their_names_and_lengths() -> Result<(), Error> {
    with_ctx(|ctx| {
        let script = "['sendError', 'sendTextResponse', 'sendResourceResponse', 'saveScreenLastInfo', \
            'getWebappRootUrl', 'sendJsonError'].map((name) => name + '/' + ec.web[name].name + '/' + \
            ec.web[name].length).join(' ') + ' ' + (() => { try { ec.web.sendError(); } catch (e) { return e.message; } })()";
        let got: String = ctx.eval(script).catch(ctx).map_err(|err| Error::new(err.to_string()))?;
        assert_eq!(
            got,
            "sendError/sendError/1 sendTextResponse/sendTextResponse/0 sendResourceResponse/sendResourceResponse/1 \
             saveScreenLastInfo/saveScreenLastInfo/0 getWebappRootUrl/getWebappRootUrl/0 sendJsonError/sendJsonError/2 \
             Error calling function with 0 argument(s) while 1 where expected"
        );
        Ok(())
    })
}
