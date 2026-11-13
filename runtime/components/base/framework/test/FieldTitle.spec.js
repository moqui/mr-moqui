// Field titles in the HTML and QVT form macros. Moved from template/screen/FieldTitle.test.js; no database.
spec({ database: false });

function readModule(file) {
    const location = file === "../TemplateSupport.js"
        ? "component://framework/template/TemplateSupport.js"
        : "component://framework/template/screen/" + file;
    return runInContext(ec.resource.getLocationText(location), {});
}

const T = readModule("../TemplateSupport.js");
const S = readModule("ScreenSupport.js")(T);
const node = (name, attrs = {}, children = []) => {
    const result = { name, attrs, children };
    children.forEach(child => { child.parent = result; });
    return result;
};
const modules = {
    Html: readModule("ScreenHtml.js")(T, S),
    Qvt: readModule("ScreenQvt.js")(T, S)
};

function renderSubmit(mode, attrs, subName = "default-field") {
    const module = modules[mode];
    const submit = node("submit", { icon: "fa fa-trash", confirmation: "Delete role?" });
    node("form-single", { name: "RolesForm" }, [
        node("field", { name: "submitButton" }, [node(subName, attrs, [submit])])
    ]);
    let html = "";
    const context = {
        sri: { write: text => { html += text; }, getThemeIconClass: () => "" },
        ec: { context: { get: () => null }, resource: { expand: text => text }, l10n: { localize: text => text } }
    };
    runInContext(module.runtime, context);
    context[mode === "Html" ? "$h" : "$v"] = context[`$mq${mode}`];
    runInContext(module.macros.submit(submit, []), context);
    return html;
}

function explicitEmptyTitles(mode) {
    for (const subName of ["default-field", "conditional-field"]) {
        const html = renderSubmit(mode, { title: "" }, subName);
        assert.match(html, /fa fa-trash/);
        assert.match(html, /data-confirm="Delete role\?"/);
        assert.match(html, /name="submitButton"/);
        assert.doesNotMatch(html, /Submit Button/);
        assert.equal(html.replace(/<[^>]*>/g, "").trim(), "");
    }
}

function missingTitles(mode) {
    assert.equal(renderSubmit(mode, {}).replace(/<[^>]*>/g, "").trim(), "Submit Button");
    assert.equal(renderSubmit(mode, { title: "Delete Role" }).replace(/<[^>]*>/g, "").trim(), "Delete Role");
}

function headerTitles(mode) {
    for (const [headerAttrs, defaultAttrs, expected] of [
        [{ title: "" }, { title: "Default Title" }, ""],
        [{}, { title: "" }, ""],
        [{}, { title: "Default Title" }, "Default Title"],
        [{ title: "Header Title" }, { title: "" }, "Header Title"],
        [{}, {}, "Submit Button"]
    ]) {
        const form = node("form-list", { name: "RolesForm" }, [node("field", { name: "submitButton" }, [
            node("header-field", headerAttrs, [node("display")]),
            node("default-field", defaultAttrs, [node("display")])
        ])]);
        let config;
        const context = {
            sri: { doBoundaryComments: () => false, openFormList: () => ({}), pushContext() {}, popContext() {} },
            ec: { l10n: { localize: text => text }, resource: { expand: text => text } }
        };
        context[mode === "Html" ? "$h" : "$v"] = { formList: (info, form, columns, cfg) => { config = cfg; } };
        runInContext(modules[mode].macros["form-list"](form, []), context);
        assert.equal(config.allFields[0].title(), expected);
    }
}

feature("Html: explicit empty default/conditional titles keep delete buttons icon-only", { expect() { explicitEmptyTitles("Html"); } });
feature("Qvt: explicit empty default/conditional titles keep delete buttons icon-only", { expect() { explicitEmptyTitles("Qvt"); } });
feature("Html: missing titles still derive words; nonempty titles still render", { expect() { missingTitles("Html"); } });
feature("Qvt: missing titles still derive words; nonempty titles still render", { expect() { missingTitles("Qvt"); } });
feature("Html: compiled header titles distinguish empty, missing and default fallback", { expect() { headerTitles("Html"); } });
feature("Qvt: compiled header titles distinguish empty, missing and default fallback", { expect() { headerTitles("Qvt"); } });
