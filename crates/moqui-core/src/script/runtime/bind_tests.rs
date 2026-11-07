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

/// Every shim of the host table installs when the engine starts (a shim that fails fails the start), and makes what it
/// is for: `tools`' members of `ec`, `EdiHandler`, `sri` and `SftpClient`.
#[test]
fn every_host_shim_installs() -> Result<(), Error> {
    with_ctx(|ctx| {
        let got: String = ctx
            .eval("[typeof ec.artifactExecution.runAsUser, typeof EdiHandler, typeof sri, typeof SftpClient].join()")
            .catch(ctx)
            .map_err(|err| Error::new(err.to_string()))?;
        assert_eq!(got, "function,function,object,function");
        Ok(())
    })
}

/// `sri`, moved onto the host table's `sri` shim, keeps the members of the QuickJS object it was, in its order, with
/// their `length`s and property flags (each function's `name` is now its member name; the QuickJS functions had none),
/// and a call with too few arguments is refused as a typed function's was.
#[test]
fn sri_keeps_its_members_and_lengths() -> Result<(), Error> {
    const MEMBERS: &str = "sendRedirectAndStopRender/1 baseLinkUrl/0 getBaseLinkUrl/0 setFocusField/1 uniqueId/1 \
        htmlText/1 write/1 stamp/1 renderSection/1 renderSectionInclude/1 renderSubscreen/0 renderIncludeScreen/1 \
        renderDynamicForm/1 renderUserColumnsForm/1 renderText/1 renderTextValue/1 renderInlineText/1 \
        doBoundaryComments/0 getRenderMode/0 encodeHtml/0 getActiveScreenDef/0 getScreenUrlInfo/0 pushContext/0 \
        popContext/0 setInContext/1 pushSingleFormMapContext/1 getSingleFormMap/1 holdNodeLits/1 \
        getFieldValueString/1 getFieldValue/1 getFieldValuePlainString/1 getFieldValueClass/1 getFieldValueTexts/1 \
        getNamedValuePlain/2 getFieldEntityValue/1 getFieldOptions/1 openFormList/1 startFormListRow/2 \
        endFormListRow/0 formListSubRowCount/2 startFormListSubRow/3 endFormListSubRow/0 safeCloseList/1 \
        formListColumnCharWidths/2 getFormNode/1 getFormInstance/1 getFormHiddenParameters/1 \
        getSavedFindFullLocation/1 getUserFormListFinds/1 getUserDefaultFormListFindId/0 \
        getFormListFindScreenScheduled/1 appendToAfterScreenWriter/1 appendToScriptWriter/1 once/1 \
        getAfterScreenWriterText/0 getScriptWriterText/0 getThemeIconClass/1 getCurrentThemeId/0 getThemeValues/1 \
        getSectionIncludedNode/1 getFieldValidation/1 getFieldValidationJsRules/1 getFieldValidationRules/1 \
        getFieldTransitionValue/2 getMenuItems/0 getMenuData/1 getExtItems/1 getShellData/0 getShellProviders/0 \
        getShellSearch/0 staticUrl/1 staticDisableKey/1 staticCheck/1 staticNote/3 xlsxSheet/1 xlsxHeader/1 \
        xlsxCell/2 xlsxCellCount/0 xlsxEndRow/0 buildUrl/1 buildUrlFromTarget/1 getScreenUrlInstance/0 \
        getCurrentScreenUrl/0 makeUrlByType/2 isAnchorLink/1 activeInCurrentMenu/get";
    with_ctx(|ctx| {
        let script = "Object.keys(sri).map((n) => { const d = Object.getOwnPropertyDescriptor(sri, n); \
            if (d.get) return d.enumerable && !d.configurable ? n + '/get' : n + '/bad-getter'; \
            const flags = d.enumerable && d.configurable && d.writable && sri[n].name === n; \
            return n + '/' + sri[n].length + (flags ? '' : '/bad'); }).join(' ')";
        let got: String = ctx.eval(script).catch(ctx).map_err(|err| Error::new(err.to_string()))?;
        assert_eq!(got, MEMBERS.split_whitespace().collect::<Vec<_>>().join(" "));
        let refused = "['renderSection', 'getNamedValuePlain', 'getFieldOptions', 'staticUrl', 'xlsxCell']\
            .map((n) => { try { sri[n](); return n + ' ran'; } catch (e) { return e.message; } }).join('; ')";
        let got: String = ctx.eval(refused).catch(ctx).map_err(|err| Error::new(err.to_string()))?;
        assert_eq!(
            got,
            "Error calling function with 0 argument(s) while 1 where expected; \
             Error calling function with 0 argument(s) while 2 where expected; \
             Error calling function with 0 argument(s) while 1 where expected; \
             Error calling function with 0 argument(s) while 1 where expected; \
             Error calling function with 0 argument(s) while 2 where expected"
        );
        Ok(())
    })
}

/// `sri.holdNodeLits` attaches each literal's conversion and freezes it, a held literal (and its parent and children)
/// reads back as the element it was, and an argument that is not an element object is refused as before.
#[test]
fn sri_holds_node_literals() -> Result<(), Error> {
    with_ctx(|ctx| {
        let got: String = ctx
            .eval(crate::script::host::SRI_HOLD_SCRIPT)
            .catch(ctx)
            .map_err(|err| Error::new(err.to_string()))?;
        assert_eq!(got, crate::script::host::SRI_HOLD_EXPECTED);
        Ok(())
    })
}
