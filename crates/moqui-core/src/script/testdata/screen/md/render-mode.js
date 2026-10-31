if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[inline][@template=true] -->");
sri.write(sri.renderInlineText("for any mode ${name}"));
if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[inline][@template=true] -->");
{
    const $location = ec.resource.expandNoL10n("component://x/template/Part.txt");
    if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[@location=" + $location + "][@template=false] -->");
    sri.write(sri.encodeHtml(sri.renderText($location, "false")));
    if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[@location=" + $location + "][@template=false] -->");
}
{
    const $location = ec.resource.expandNoL10n("component://x/template/${name}.txt");
    if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[@location=" + $location + "][@template=true] -->");
    sri.write(sri.renderText($location, ""));
    if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[@location=" + $location + "][@template=true] -->");
}
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[inline][@template=false] -->");
sri.write(sri.encodeHtml("a < b"));
if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[inline][@template=false] -->");
sri.write(sri.renderInlineText("quiet ${name}"));
