sri.write("<div id=\"c1\" class=\"page-break-before\">");
if (who) {
    {
        const $label = ec.resource.expand("First ${who}");
        if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
    }
}
sri.write("</div>\n");
sri.write('<div class="box-header">');
{
    const $label = ec.resource.expand("Boxed");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.write("</div>\n");
{
    const $label = ec.resource.expand("inside");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
{
    const $label = ec.resource.expand("nopad");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
{
    const $label = ec.resource.expand("col one");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
{
    const $label = ec.resource.expand("col two");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.write("<div>");
{
    const $label = ec.resource.expand("head");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.write("</div>\n");
sri.write('<table class="panel"><tr>');
sri.write("<td>");
{
    const $label = ec.resource.expand("left");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.write("</td>");
sri.write("<td>");
{
    const $label = ec.resource.expand("center");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.write("</td>");
sri.write("<td>");
{
    const $label = ec.resource.expand("right");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.write("</td>");
sri.write("</tr></table>\n");
sri.write("<div>");
{
    const $label = ec.resource.expand("foot");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.write("</div>\n");
sri.pushSingleFormMapContext(item);
try {
    sri.write('<div class="group-title">' + $e(ec.l10n.localize("Group")) + "</div>\n");
    sri.write('<table class="field-row"><tr>');
    sri.write("<td>");
    sri.write('<table class="field"><tr><td class="field-title">' + $e(ec.l10n.localize("A")) + '</td><td class="field-value">');
    sri.write($e(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{"title":"A"},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-single","attrs":{"name":"LayoutForm","map":"item"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("</td></tr></table>\n");
    sri.write("</td>");
    sri.write("<td>");
    sri.write('<table class="field"><tr><td class="field-title">' + $e(ec.l10n.localize("B")) + '</td><td class="field-value">');
    sri.write($e(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-single","attrs":{"name":"LayoutForm","map":"item"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("</td></tr></table>\n");
    sri.write("</td>");
    sri.write("</tr></table>\n");
    sri.write('<table class="field-row"><tr>');
    sri.write('<td class="field-title">' + $e(ec.resource.expand("Big ${who}")) + "</td>");
    sri.write("<td>");
    sri.write('<table class="field"><tr><td class="field-title">' + $e(ec.l10n.localize("C")) + '</td><td class="field-value">');
    sri.write($e(sri.getFieldValueString({"name":"field","attrs":{"name":"c"},"parent":{"name":"form-single","attrs":{"name":"LayoutForm","map":"item"},"parent":{"name":"widgets","attrs":{}}}}, "", "yyyy-MM-dd", "date")));
    sri.write("</td></tr></table>\n");
    sri.write("</td>");
    sri.write("<td>");
    sri.write('<table class="field"><tr><td class="field-title">' + $e(ec.l10n.localize("D")) + '</td><td class="field-value">');
    {
        const $options = sri.getFieldOptions({"name":"drop-down","attrs":{"no-current-selected-key":"N"},"children":[{"name":"option","attrs":{"key":"N","text":"No"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LayoutForm","map":"item"},"parent":{"name":"widgets","attrs":{}}}}}});
        let $current = sri.getFieldValueString({"name":"drop-down","attrs":{"no-current-selected-key":"N"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LayoutForm","map":"item"},"parent":{"name":"widgets","attrs":{}}}}}});
        if (!$current) $current = "N";
        const $text = $options.get($current);
        sri.write($e($text != null ? $text : $current));
    }
    sri.write("</td></tr></table>\n");
    sri.write("</td>");
    sri.write("</tr></table>\n");
    sri.write("<div>Error: could not find field with name missing referred to in a field-ref.name attribute.</div>\n");
} finally {
    sri.popContext();
}
{
    const $url = sri.makeUrlByType("/img/logo.png", "plain", null, "true");
    sri.write('<img src="' + $e($url.url) + '" alt="' + $e("Logo") + '"' + ' id="' + $e("logo") + '"' + ' width="' + $e("10") + '"' + ' height="' + $e("20") + '"' + "/>\n");
}
{
    const $url = sri.makeUrlByType("logo", "plain", null, "true");
    sri.write('<img src="' + $e($url.url) + '" alt="' + $e("Content") + '"' + "/>\n");
}
if (who) {
    sri.write("<div>");
    {
        let $linkText = ec.resource.expand("Go ${who}");
        if ($linkText === "null") $linkText = "";
        const $url = sri.makeUrlByType("go", "transition", {"name":"link","attrs":{"url":"go","text":"Go ${who}","condition":"who","parameter-map":"{a: 1}"},"parent":{"name":"widgets","attrs":{}}}, "true");
        sri.write('<a href="' + $e($url.getUrlWithParams()) + '">' + $e($linkText) + "</a>");
    }
    sri.write("</div>\n");
}
sri.write("<div>");
{
    let $linkText = ec.resource.expand("Plain");
    if ($linkText === "null") $linkText = "";
    const $url = sri.makeUrlByType(".", "plain", {"name":"link","attrs":{"url":".","text":"Plain","url-type":"plain","url-noparam":"true"},"parent":{"name":"widgets","attrs":{}}}, "true");
    sri.write('<a href="' + $e($url.url) + '">' + $e($linkText) + "</a>");
}
sri.write("</div>\n");
sri.write("<div>");
{
    let $linkText = sri.getFieldEntityValue({"name":"link","attrs":{"entity-name":"moqui.basic.Enumeration","text":"${description}"},"parent":{"name":"widgets","attrs":{}}});
    if ($linkText === "null") $linkText = "";
    const $url = sri.makeUrlByType("", "transition", {"name":"link","attrs":{"entity-name":"moqui.basic.Enumeration","text":"${description}"},"parent":{"name":"widgets","attrs":{}}}, "true");
    sri.write('<a href="' + $e($url.getUrlWithParams()) + '">' + $e($linkText) + "</a>");
}
sri.write("</div>\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[inline][@template=true] -->");
sri.writeInlineText("print only");
if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[inline][@template=true] -->");
sri.renderSubscreen();
sri.renderIncludeScreen("component://x/Y.xml", "");
