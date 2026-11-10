if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
sri.write("<" + "section" + $h.A({id: $h.nodeId({"name":"container","attrs":{"type":"section","id":"Wrap","style":"wide ${extra}"}}), class: ec.resource.expandNoL10n("wide ${extra}")}) + ">");
$h.label({"name":"label","attrs":{"text":"in wrap","type":"h4","style":"text-strong"}});
sri.write("</section>\n");
sri.write("<section" + $h.A({id: null, class: "box", "data-type": "info"}) + "><header>");
sri.write("<h5>" + $h.e(ec.resource.expand("Boxed ${name}", "")) + "</h5>");
sri.write("<div class=\"toolbar\">");
$h.link({"name":"link","attrs":{"url":"add","text":"Add","link-type":"anchor"}}, false);
sri.write("</div>");
sri.write("</header>");
sri.write("<div class=\"body\" style=\"max-height:200px;overflow-y:auto\">");
$h.label({"name":"label","attrs":{"text":"body"}});
sri.write("</div>");
sri.write("</section>\n");
$h.rowOpen("row", "gap");
$h.colOpen({"name":"row-col","attrs":{"xs":"12","md":"6"}});
$h.label({"name":"label","attrs":{"text":"one"}});
sri.write("</div>");
$h.colOpen({"name":"row-col","attrs":{"xs":"12","md":"6","style":"end"}});
$h.label({"name":"label","attrs":{"text":"two"}});
sri.write("</div>");
sri.write("</div>\n");
sri.write("<div" + $h.A({id: $h.nodeId({"name":"container-panel","attrs":{"id":"Pan"}}), class: "panel-layout"}) + ">");
sri.write("<header>");
$h.label({"name":"label","attrs":{"text":"head"}});
sri.write("</header>");
sri.write("<div class=\"panel-middle\">");
sri.write("<aside class=\"left\" style=\"width:120px\">");
$h.label({"name":"label","attrs":{"text":"left"}});
sri.write("</aside>");
sri.write("<main>");
$h.label({"name":"label","attrs":{"text":"center"}});
sri.write("</main>");
sri.write("</div>");
sri.write("</div>\n");
if ($h.containerDialog({"name":"container-dialog","attrs":{"id":"Dlg","button-text":"Open","icon":"fa fa-plus","condition":"allowed","width":"500"}})) {
    $h.label({"name":"label","attrs":{"text":"dialog body"}});
    sri.write("</div></dialog>\n");
}
$h.dynamicContainer({"name":"dynamic-container","attrs":{"id":"Dyn","transition":"load"},"children":[{"name":"parameter","attrs":{"name":"p","value":"1"}}]});
$h.dynamicDialog({"name":"dynamic-dialog","attrs":{"id":"DynDlg","button-text":"Edit","transition":"edit"},"children":[{"name":"parameter","attrs":{"name":"id","from":"rowId"}}]});
{
    const $map = null;
    let $text = $map ? ec.resource.expand("Actions", "", $map) : ec.resource.expand("Actions", "");
    if ($text === "null") $text = "";
    if ($text || false || true) {
        if ("" !== "false") $text = $h.e($text);
        let $icon = "fa fa-cog";
        if (!$icon && $text) $icon = sri.getThemeIconClass($text) || "";
        const $badge = ec.resource.expand("", "");
        sri.write("<details class=\"menu\"" + $h.A({id: null}) + "><summary" + $h.A({"data-btn": "info", class: null}) + ">" +
            ($icon ? "<i class=\"" + $h.e($icon) + "\"></i> " : "") + $text + ($badge ? " <span class=\"badge\">" + $h.e($badge) + "</span>" : "") + "</summary><ul>");
        sri.write("<li>");
        $h.link({"name":"link","attrs":{"url":"one","text":"One","link-type":"anchor"}}, false);
        sri.write("</li>");
        sri.write("<li>");
        $h.link({"name":"link","attrs":{"url":"two","text":"Two","link-type":"anchor","confirmation":"Sure?"}}, false);
        sri.write("</li>");
        sri.write("</ul></details>\n");
    }
}
$h.tree({"name":"tree","attrs":{"name":"Nodes","transition":"treeItems","open-path":"${path}"},"children":[{"name":"parameter","attrs":{"name":"root","value":"a"}}]});
$h.editable({"name":"editable","attrs":{"id":"Ed","transition":"save","text":"${name}","parameter-name":"name"}});
$h.link({"name":"link","attrs":{"url":"go","text":"Go","btn-type":"success","icon":"fa fa-go","badge":"3","tooltip":"Go now","target-window":"_blank"}}, false);
$h.link({"name":"link","attrs":{"url":"post","text":"Post","confirmation":"Really?","link-type":"hidden-form-link"}}, false);
$h.link({"name":"link","attrs":{"url":"http://example.com","url-type":"plain","text":"Out","link-type":"anchor-button"}}, false);
$h.label({"name":"label","attrs":{"text":"Line ${name}","type":"p","condition":"name","style":"muted","tooltip":"tip"}});
$h.image({"name":"image","attrs":{"url":"/img.png","alt":"Logo","width":"20","height":"20","style":"logo","hover":"true"}});
$h.subscreensMenu({"name":"subscreens-menu","attrs":{"id":"Menu"}});
$h.subscreensMenu({"name":"subscreens-menu","attrs":{"type":"popup","title":"Apps"}});
$h.subscreensPanel({"name":"subscreens-panel","attrs":{"id":"Tabs","dynamic":"true"}});
sri.renderSection("First");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[inline][@template=true] -->");
sri.writeInlineText("<b>raw</b>");
if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[inline][@template=true] -->");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
