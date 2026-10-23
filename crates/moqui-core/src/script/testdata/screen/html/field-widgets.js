if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-single[@name=AllFields] -->\n");
sri.pushSingleFormMapContext(fv);
try {
    $h.formOpen({"name":"form-single","attrs":{"name":"AllFields","map":"fv"}}, true, ["firstName","bio","userGroupId","status","kind","flag","stamp","day","at","from","amount","price","note","enumId","doc","pw","range","clear","secret","skip","go","sel","lst","mix","withSet","hidden1"]);
    $h.rowOpen("field-row", "");
    sri.write("<div>");
    {
        const $title = ec.l10n.localize("First Name");
        const $open = $h.fieldOpen("firstName", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textLine({"name":"text-line","attrs":{"size":"20"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"firstName"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    sri.write("</div>");
    sri.write("<div>");
    {
        const $title = ec.l10n.localize("About");
        const $open = $h.fieldOpen("bio", $title, false, "", false);
        sri.pushContext();
        try {
            $h.textArea({"name":"text-area","attrs":{"rows":"3"},"parent":{"name":"default-field","attrs":{"title":"About"},"parent":{"name":"field","attrs":{"name":"bio"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        $h.fieldClose($open, false);
    }
    sri.write("</div>");
    sri.write("</div>\n");
    $h.formClose({"name":"form-single","attrs":{"name":"AllFields","map":"fv"}});
} finally {
    sri.popContext();
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-single[@name=AllFields] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
