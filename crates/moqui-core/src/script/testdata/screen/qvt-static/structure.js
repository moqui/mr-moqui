$s.label({"name":"label","attrs":{"text":"Title ${name}"}});
$s.containerOpen({"name":"container","attrs":{"id":"Box1"}});
$s.label({"name":"label","attrs":{"text":"inside"}});
$s.rowOpen("row m-row-gutter", "");
$s.colOpen({"name":"row-col","attrs":{"lg":"6"}}, false);
$s.label({"name":"label","attrs":{"text":"left"}});
sri.write("</div>");
$s.colOpen({"name":"row-col","attrs":{"lg":"6"}}, false);
$s.label({"name":"label","attrs":{"text":"right"}});
sri.write("</div>");
sri.write("</div>\n");
sri.write("</div>\n");
$s.boxOpen({"name":"container-box","attrs":{}}, {"name":"box-header","attrs":{"title":"Header"}});
sri.write("<template v-slot:header>");
$s.label({"name":"label","attrs":{"text":"Head"}});
sri.write("</template>");
sri.write("<template v-slot:toolbar>");
$s.label({"name":"label","attrs":{"text":"toolbar"}});
sri.write("</template>");
sri.write("<m-box-body>");
$s.label({"name":"label","attrs":{"text":"Body"}});
sri.write("</m-box-body>");
$s.label({"name":"label","attrs":{"text":"Nopad"}});
sri.write("</m-container-box>\n");
sri.write("<div class=\"container-panel-outer\">");
sri.write("<div class=\"container-panel-header\">");
$s.label({"name":"label","attrs":{"text":"ph"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-middle\">");
sri.write("<div class=\"container-panel-left\" style=\"width:180px;\">");
$s.label({"name":"label","attrs":{"text":"pl"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-center-both\">");
$s.label({"name":"label","attrs":{"text":"pc"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-right\" style=\"width:180px;\">");
$s.label({"name":"label","attrs":{"text":"pr"}});
sri.write("</div>");
sri.write("</div>");
sri.write("<div class=\"container-panel-footer\">");
$s.label({"name":"label","attrs":{"text":"pf"}});
sri.write("</div>");
sri.write("</div>\n");
{
    const $c = $s.containerDialog({"name":"container-dialog","attrs":{"id":"D1","button-text":"Open ${name}"}});
$s.label({"name":"label","attrs":{"text":"in dialog"}});
    sri.write($c ? "</m-container-dialog></template>\n" : "</m-container-dialog>\n");
}
sri.write('<div class="m-ai-opt-out" data-ai-opt-out="section">');
$s.label({"name":"label","attrs":{"text":"first"}});

sri.write("</div>");
sri.write('<div class="m-ai-opt-out" data-ai-opt-out="section">');
{
sri.write($s.sectionIterateOpen({"name":"section-iterate","attrs":{"name":"Each","list":"rows","entry":"row","ai-opt-out":"true"}}));
$s.label({"name":"label","attrs":{"text":"${row}"}});
sri.write("</m-scope></template>");
}

sri.write("</div>");
sri.staticNote("section-include", "section-include", "section-include First was not resolved");
$s.subscreensTabs({"name":"subscreens-menu","attrs":{}});
sri.write("<m-subscreens-active></m-subscreens-active>");
$s.subscreensPanel({"name":"subscreens-panel","attrs":{}});
sri.staticNote("include-screen", "include-screen", "include-screen component://x/screen/Other.xml was not resolved");
