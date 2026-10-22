if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
{
    const $map = null;
    let $text = $map ? ec.resource.expand("Menu", "", $map) : ec.resource.expand("Menu", "");
    if ($text === "null") $text = "";
    if ($text || false || false) {
        if ("" !== "false") $text = $v.t($text);
        let $icon = "";
        if (!$icon && $text) $icon = sri.getThemeIconClass($text) || "";
        const $badge = ec.resource.expand("", "");
        sri.write("<q-btn-dropdown" + $v.A(Object.assign({dense: true, unelevated: true, "no-caps": true}, $v.btnKind(null, null))) + "><template v-slot:label>" +
            ($icon ? $v.iconTag($icon, !!$text) : ""));
        // as Java: the image child in the place of the text
        sri.write($text);
        sri.write(($badge ? "<q-badge class=\"on-right\" transparent>" + $v.t($badge) + "</q-badge>" : "") + "</template><q-list>");
        sri.write("<q-item clickable><q-item-section>");
        sri.write("</q-item-section></q-item>");
        sri.write("</q-list></q-btn-dropdown>\n");
    }
}
$v.dynamicContainer({"name":"dynamic-container","attrs":{"id":"Dyn","transition":"load"}});
$v.dynamicDialog({"name":"dynamic-dialog","attrs":{"id":"DynD","button-text":"Dyn","transition":"load"}});
$v.tree({"name":"tree","attrs":{"name":"T","transition":"t","root-node-name":"n"}});
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
