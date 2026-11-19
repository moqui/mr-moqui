$lineChars = Number(lineCharacters || "132");
$pageLines = pageLines ? Number(pageLines) : 0;
$lineWrap = lineWrap === "true";
if (sri.isOverlaid()) sri.write("<div class=\"m-preview-placeholder text-caption\">(text, scripts and templates are not shown in a preview)</div>");
else {
    if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[inline][@template=true] -->");
    sri.writeInlineText("text csv xml");
    if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[inline][@template=true] -->");
}
if (sri.isOverlaid()) sri.write("<div class=\"m-preview-placeholder text-caption\">(text, scripts and templates are not shown in a preview)</div>");
else {
    if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[inline][@template=true] -->");
    sri.writeInlineText("typed text");
    if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[inline][@template=true] -->");
}
if (sri.isOverlaid()) sri.write("<div class=\"m-preview-placeholder text-caption\">(text, scripts and templates are not shown in a preview)</div>");
else {
    {
        const $location = ec.resource.expandNoL10n("component://x/template/Part.txt");
        if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[@location=" + $location + "][@template=false] -->");
        sri.write(sri.encodeHtml(sri.renderTextValue($location, "false")));
        if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[@location=" + $location + "][@template=false] -->");
    }
}
if (sri.isOverlaid()) sri.write("<div class=\"m-preview-placeholder text-caption\">(text, scripts and templates are not shown in a preview)</div>");
else {
    {
        const $location = ec.resource.expandNoL10n("component://x/template/${name}.txt");
        if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[@location=" + $location + "][@template=true] -->");
        sri.write(sri.renderText($location, ""));
        if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[@location=" + $location + "][@template=true] -->");
    }
}
if (sri.isOverlaid()) sri.write("<div class=\"m-preview-placeholder text-caption\">(text, scripts and templates are not shown in a preview)</div>");
else {
    if (sri.doBoundaryComments()) sri.write("<!-- BEGIN render-mode.text[inline][@template=false] -->");
    sri.write(sri.encodeHtml("a < b"));
    if (sri.doBoundaryComments()) sri.write("<!-- END   render-mode.text[inline][@template=false] -->");
}
if (sri.isOverlaid()) sri.write("<div class=\"m-preview-placeholder text-caption\">(text, scripts and templates are not shown in a preview)</div>");
else {
    sri.writeInlineText("quiet ${name}");
}
