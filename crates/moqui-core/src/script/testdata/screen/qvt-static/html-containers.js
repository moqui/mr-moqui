$s.containerOpen({"name":"container","attrs":{"type":"section","id":"Wrap","style":"wide ${extra}"}});
$s.label({"name":"label","attrs":{"text":"in wrap","type":"h4","style":"text-strong"}});
sri.write("</section>\n");
$s.boxOpen({"name":"container-box","attrs":{"type":"info"}}, {"name":"box-header","attrs":{"title":"Boxed ${name}"}});
sri.write("<template v-slot:header>");
sri.write("</template>");
sri.write("<template v-slot:toolbar>");
$s.link({"name":"link","attrs":{"url":"add","text":"Add","link-type":"anchor"}}, false);
sri.write("</template>");
sri.write("<m-box-body height=\"200\">");
$s.label({"name":"label","attrs":{"text":"body"}});
sri.write("</m-box-body>");
sri.write("</m-container-box>\n");
$s.rowOpen("row m-row-gutter", "gap");
$s.colOpen({"name":"row-col","attrs":{"xs":"12","md":"6"}}, false);
$s.label({"name":"label","attrs":{"text":"one"}});
sri.write("</div>");
$s.colOpen({"name":"row-col","attrs":{"xs":"12","md":"6","style":"end"}}, false);
$s.label({"name":"label","attrs":{"text":"two"}});
sri.write("</div>");
sri.write("</div>\n");
sri.write("<div id=\"Pan\" class=\"container-panel-outer\">");
sri.write("<div class=\"container-panel-header\">");
$s.label({"name":"label","attrs":{"text":"head"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-middle\">");
sri.write("<div class=\"container-panel-left\" style=\"width:120px;\">");
$s.label({"name":"label","attrs":{"text":"left"}});
sri.write("</div>");
sri.write("<div class=\"container-panel-center-left\">");
$s.label({"name":"label","attrs":{"text":"center"}});
sri.write("</div>");
sri.write("</div>");
sri.write("</div>\n");
{
    const $c = $s.containerDialog({"name":"container-dialog","attrs":{"id":"Dlg","button-text":"Open","icon":"fa fa-plus","condition":"allowed","width":"500"}});
$s.label({"name":"label","attrs":{"text":"dialog body"}});
    sri.write("</m-container-dialog>" + $c + "\n");
}
$s.dynamicContainer({"name":"dynamic-container","attrs":{"id":"Dyn","transition":"load"},"children":[{"name":"parameter","attrs":{"name":"p","value":"1"}}]});
$s.dynamicDialog({"name":"dynamic-dialog","attrs":{"id":"DynDlg","button-text":"Edit","transition":"edit"},"children":[{"name":"parameter","attrs":{"name":"id","from":"rowId"}}]});
{
const $bm = $s.buttonMenuOpen({"name":"button-menu","attrs":{"text":"Actions","icon":"fa fa-cog","btn-type":"info"}}, null);
if ($bm) {
sri.write($bm);
sri.write($s.buttonMenuItem({"name":"link","attrs":{"url":"one","text":"One","link-type":"anchor"}}));
$s.link({"name":"link","attrs":{"url":"one","text":"One","link-type":"anchor"}}, false);
sri.write("</q-item-section></q-item>");
sri.write($s.buttonMenuItem({"name":"link","attrs":{"url":"two","text":"Two","link-type":"anchor","confirmation":"Sure?"}}));
$s.link({"name":"link","attrs":{"url":"two","text":"Two","link-type":"anchor","confirmation":"Sure?"}}, false);
sri.write("</q-item-section></q-item>");
sri.write($s.buttonMenuClose({"name":"button-menu","attrs":{"text":"Actions","icon":"fa fa-cog","btn-type":"info"}}));
}
}
$s.tree({"name":"tree","attrs":{"name":"Nodes","transition":"treeItems","open-path":"${path}"},"children":[{"name":"parameter","attrs":{"name":"root","value":"a"}}]});
$s.editable({"name":"editable","attrs":{"id":"Ed","transition":"save","text":"${name}","parameter-name":"name"}});
$s.link({"name":"link","attrs":{"url":"go","text":"Go","btn-type":"success","icon":"fa fa-go","badge":"3","tooltip":"Go now","target-window":"_blank"}}, false);
$s.link({"name":"link","attrs":{"url":"post","text":"Post","confirmation":"Really?","link-type":"hidden-form-link"}}, false);
$s.link({"name":"link","attrs":{"url":"http://example.com","url-type":"plain","text":"Out","link-type":"anchor-button"}}, false);
$s.label({"name":"label","attrs":{"text":"Line ${name}","type":"p","condition":"name","style":"muted","tooltip":"tip"}});
$s.image({"name":"image","attrs":{"url":"/img.png","alt":"Logo","width":"20","height":"20","style":"logo","hover":"true"}});
$s.subscreensTabs({"name":"subscreens-menu","attrs":{"id":"Menu"}});
$s.subscreensTabs({"name":"subscreens-menu","attrs":{"type":"popup","title":"Apps"}});
$s.subscreensPanel({"name":"subscreens-panel","attrs":{"id":"Tabs","dynamic":"true"}});
$s.label({"name":"label","attrs":{"text":"first"}});
