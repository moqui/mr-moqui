sri.pushSingleFormMapContext(fv);
try {
    sri.write('<table class="field-row"><tr>');
    sri.write("<td>");
    sri.write('<table class="field"><tr><td class="field-title">' + $e(ec.l10n.localize("First Name")) + '</td><td class="field-value">');
    sri.write($e(sri.getFieldValueString({"name":"text-line","attrs":{"size":"20"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"firstName"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("</td></tr></table>\n");
    sri.write("</td>");
    sri.write("<td>");
    sri.write('<table class="field"><tr><td class="field-title">' + $e(ec.l10n.localize("About")) + '</td><td class="field-value">');
    sri.write($e(sri.getFieldValueString({"name":"text-area","attrs":{"rows":"3"},"parent":{"name":"default-field","attrs":{"title":"About"},"parent":{"name":"field","attrs":{"name":"bio","ai-opt-out":"true"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("</td></tr></table>\n");
    sri.write("</td>");
    sri.write("</tr></table>\n");
} finally {
    sri.popContext();
}
