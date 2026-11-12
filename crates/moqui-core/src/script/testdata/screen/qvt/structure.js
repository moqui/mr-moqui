if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
$v.label({"name":"label","attrs":{"text":"Title ${name}"}});
sri.write("<" + "div" + $v.A({id: $v.nodeId({"name":"container","attrs":{"id":"Box1"}}), class: null}) + ">");
$v.label({"name":"label","attrs":{"text":"inside"}});
$v.rowOpen("row m-row-gutter", "");
$v.colOpen({"name":"row-col","attrs":{"lg":"6"}}, false);
$v.label({"name":"label","attrs":{"text":"left"}});
sri.write("</div>");
$v.colOpen({"name":"row-col","attrs":{"lg":"6"}}, false);
$v.label({"name":"label","attrs":{"text":"right"}});
sri.write("</div>");
sri.write("</div>\n");
sri.write("</div>\n");
{
    const $type = "default" || "default";
    sri.write("<m-container-box" + $v.A({id: null, type: $type, title: ec.resource.expand("Header", ""),
        ":initial-open": "" === "closed" ? "false" : null}) + ">");
}
sri.write("<template v-slot:header>");
$v.label({"name":"label","attrs":{"text":"Head"}});
sri.write("</template>");
sri.write("<template v-slot:toolbar>");
$v.label({"name":"label","attrs":{"text":"toolbar"}});
sri.write("</template>");
sri.write("<m-box-body>");
$v.label({"name":"label","attrs":{"text":"Body"}});
sri.write("</m-box-body>");
$v.label({"name":"label","attrs":{"text":"Nopad"}});
sri.write("</m-container-box>\n");
sri.write("<div" + $v.A({id: null, class: "container-panel-outer"}) + ">");
sri.write("<div class=\"container-panel-header\">");
$v.label({"name":"label","attrs":{"text":"ph"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-middle\">");
sri.write("<div class=\"container-panel-left\" style=\"width:180px;\">");
$v.label({"name":"label","attrs":{"text":"pl"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-center-both\">");
$v.label({"name":"label","attrs":{"text":"pc"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-right\" style=\"width:180px;\">");
$v.label({"name":"label","attrs":{"text":"pr"}});
sri.write("</div>");
sri.write("</div>");
sri.write("<div class=\"container-panel-footer\">");
$v.label({"name":"label","attrs":{"text":"pf"}});
sri.write("</div>");
sri.write("</div>\n");
if ($v.containerDialog({"name":"container-dialog","attrs":{"id":"D1","button-text":"Open ${name}"}})) {
    $v.label({"name":"label","attrs":{"text":"in dialog"}});
    sri.write("</m-container-dialog>\n");
}
sri.write('<div class="m-ai-opt-out" data-ai-opt-out="section">');
sri.renderSection("First");
sri.write("</div>");
sri.write('<div class="m-ai-opt-out" data-ai-opt-out="section">');
sri.renderSection("Each");

sri.write("</div>");
{
    const $included = sri.getSectionIncludedNode({"name":"section-include","attrs":{"name":"First"}});
    if ($included.attrs.paginate === "true") $v.sectionPaginate($included.attrs.list);
    sri.renderSectionInclude({"name":"section-include","attrs":{"name":"First"}});
}
$v.subscreensTabs({"name":"subscreens-menu","attrs":{}});
sri.write("<m-subscreens-active></m-subscreens-active>");
$v.subscreensPanel({"name":"subscreens-panel","attrs":{}});
sri.renderIncludeScreen("component://x/screen/Other.xml", "true");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
