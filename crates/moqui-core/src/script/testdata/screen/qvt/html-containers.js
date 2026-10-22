if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
sri.write("<" + "section" + $v.A({id: $v.nodeId({"name":"container","attrs":{"type":"section","id":"Wrap","style":"wide ${extra}"}}), class: ec.resource.expandNoL10n("wide ${extra}")}) + ">");
$v.label({"name":"label","attrs":{"text":"in wrap","type":"h4","style":"text-strong"}});
sri.write("</section>\n");
{
    const $type = "info" || "default";
    sri.write("<m-container-box" + $v.A({id: null, type: $type, title: ec.resource.expand("Boxed ${name}", ""),
        ":initial-open": "" === "closed" ? "false" : null}) + ">");
}
sri.write("<template v-slot:header>");
sri.write("</template>");
sri.write("<template v-slot:toolbar>");
$v.link({"name":"link","attrs":{"url":"add","text":"Add","link-type":"anchor"}}, false);
sri.write("</template>");
sri.write("<m-box-body height=\"200\">");
$v.label({"name":"label","attrs":{"text":"body"}});
sri.write("</m-box-body>");
sri.write("</m-container-box>\n");
$v.rowOpen("row", "gap");
$v.colOpen({"name":"row-col","attrs":{"xs":"12","md":"6"}}, false);
$v.label({"name":"label","attrs":{"text":"one"}});
sri.write("</div>");
$v.colOpen({"name":"row-col","attrs":{"xs":"12","md":"6","style":"end"}}, false);
$v.label({"name":"label","attrs":{"text":"two"}});
sri.write("</div>");
sri.write("</div>\n");
sri.write("<div" + $v.A({id: $v.nodeId({"name":"container-panel","attrs":{"id":"Pan"}}), class: "container-panel-outer"}) + ">");
sri.write("<div class=\"container-panel-header\">");
$v.label({"name":"label","attrs":{"text":"head"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-middle\">");
sri.write("<div class=\"container-panel-left\" style=\"width:120px;\">");
$v.label({"name":"label","attrs":{"text":"left"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-center-left\">");
$v.label({"name":"label","attrs":{"text":"center"}});
sri.write("</div>");
sri.write("</div>");
sri.write("</div>\n");
if ($v.containerDialog({"name":"container-dialog","attrs":{"id":"Dlg","button-text":"Open","icon":"fa fa-plus","condition":"allowed","width":"500"}})) {
    $v.label({"name":"label","attrs":{"text":"dialog body"}});
    sri.write("</m-container-dialog>\n");
}
$v.dynamicContainer({"name":"dynamic-container","attrs":{"id":"Dyn","transition":"load"},"children":[{"name":"parameter","attrs":{"name":"p","value":"1"}}]});
$v.dynamicDialog({"name":"dynamic-dialog","attrs":{"id":"DynDlg","button-text":"Edit","transition":"edit"},"children":[{"name":"parameter","attrs":{"name":"id","from":"rowId"}}]});
{
    const $map = null;
    let $text = $map ? ec.resource.expand("Actions", "", $map) : ec.resource.expand("Actions", "");
    if ($text === "null") $text = "";
    if ($text || false || true) {
        if ("" !== "false") $text = $v.t($text);
        let $icon = "fa fa-cog";
        if (!$icon && $text) $icon = sri.getThemeIconClass($text) || "";
        const $badge = ec.resource.expand("", "");
        sri.write("<q-btn-dropdown" + $v.A(Object.assign({dense: true, unelevated: true, "no-caps": true}, $v.btnKind("info", null))) + "><template v-slot:label>" +
            ($icon ? $v.iconTag($icon, !!$text) : ""));
        // as Java: the image child in the place of the text
        sri.write($text);
        sri.write(($badge ? "<q-badge class=\"on-right\" transparent>" + $v.t($badge) + "</q-badge>" : "") + "</template><q-list>");
        sri.write("<q-item clickable><q-item-section>");
        $v.link({"name":"link","attrs":{"url":"one","text":"One","link-type":"anchor"}}, false);
        sri.write("</q-item-section></q-item>");
        sri.write("<q-item clickable><q-item-section>");
        $v.link({"name":"link","attrs":{"url":"two","text":"Two","link-type":"anchor","confirmation":"Sure?"}}, false);
        sri.write("</q-item-section></q-item>");
        sri.write("</q-list></q-btn-dropdown>\n");
    }
}
$v.tree({"name":"tree","attrs":{"name":"Nodes","transition":"treeItems","open-path":"${path}"},"children":[{"name":"parameter","attrs":{"name":"root","value":"a"}}]});
$v.editable({"name":"editable","attrs":{"id":"Ed","transition":"save","text":"${name}","parameter-name":"name"}});
$v.link({"name":"link","attrs":{"url":"go","text":"Go","btn-type":"success","icon":"fa fa-go","badge":"3","tooltip":"Go now","target-window":"_blank"}}, false);
$v.link({"name":"link","attrs":{"url":"post","text":"Post","confirmation":"Really?","link-type":"hidden-form-link"}}, false);
$v.link({"name":"link","attrs":{"url":"http://example.com","url-type":"plain","text":"Out","link-type":"anchor-button"}}, false);
$v.label({"name":"label","attrs":{"text":"Line ${name}","type":"p","condition":"name","style":"muted","tooltip":"tip"}});
$v.image({"name":"image","attrs":{"url":"/img.png","alt":"Logo","width":"20","height":"20","style":"logo","hover":"true"}});
$v.subscreensTabs({"name":"subscreens-menu","attrs":{"id":"Menu"}});
$v.subscreensTabs({"name":"subscreens-menu","attrs":{"type":"popup","title":"Apps"}});
$v.subscreensPanel({"name":"subscreens-panel","attrs":{"id":"Tabs","dynamic":"true"}});
sri.renderSection("First");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
