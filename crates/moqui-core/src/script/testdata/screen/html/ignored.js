if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
{
    const $map = null;
    let $text = $map ? ec.resource.expand("Menu", "", $map) : ec.resource.expand("Menu", "");
    if ($text === "null") $text = "";
    if ($text || false || false) {
        if ("" !== "false" || sri.isOverlaid()) $text = $h.e($text);
        let $icon = "";
        if (!$icon && $text) $icon = sri.getThemeIconClass($text) || "";
        const $badge = ec.resource.expand("", "");
        sri.write("<details class=\"menu\"" + $h.A({id: null}) + "><summary" + $h.A({"data-btn": "primary", class: null}) + ">" +
            ($icon ? "<i class=\"" + $h.e($icon) + "\"></i> " : "") + $text + ($badge ? " <span class=\"badge\">" + $h.e($badge) + "</span>" : "") + "</summary><ul>");
        sri.write("<li>");
        sri.write("</li>");
        sri.write("</ul></details>\n");
    }
}
$h.dynamicContainer({"name":"dynamic-container","attrs":{"id":"Dyn","transition":"load"}});
$h.dynamicDialog({"name":"dynamic-dialog","attrs":{"id":"DynD","button-text":"Dyn","transition":"load"}});
$h.tree({"name":"tree","attrs":{"name":"T","transition":"t","root-node-name":"n"}});
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
