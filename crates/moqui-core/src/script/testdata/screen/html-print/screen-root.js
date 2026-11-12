const $document = sri.once("html-print-document");
if ($document) {
    const $title = documentTitle ? String(documentTitle) : "";
    const $cssTitle = $title.replace(/[\\"\n\r<>]/g, " ");
    let $pages = "";
    for (const $name of Object.keys($masters)) {
        const $m = $masters[$name];
        $pages += "@page " + $name + " { size: " + $m[0] + " " + $m[1] + "; margin: " + $m[2] + "; }\n";
    }
    sri.write("<!DOCTYPE html>\n<html><head><meta charset=\"utf-8\"><title>" + $e($title) + "</title>\n<style>\n" + $pages +
        "@page { size: " + $layout[0] + " " + $layout[1] + "; margin: " + $layout[2] + ";" +
        ($title ? ' @top-center { content: "' + $cssTitle + '"; font-size: 14pt; }' : "") +
        ' @bottom-center { content: "- " counter(page) " -"; font-size: 8pt; } }\n' +
        "body { font-family: Helvetica, Arial, sans-serif; font-size: 9pt; margin: 0; }\ntable { border-collapse: collapse; table-layout: fixed; width: 100%; }\ntd, th { padding: 2pt; vertical-align: top; overflow-wrap: anywhere; }\ntable.form-list thead { display: table-header-group; border-bottom: thin solid black; }\ntable.form-list tr { break-inside: avoid; page-break-inside: avoid; }\ntable.form-list tbody tr:nth-child(odd) { background-color: #EEEEEE; }\ntable.form-list th { font-weight: bold; text-align: left; }\n.field-title { width: 20%; text-align: right; font-weight: bold; }\n.field-value { width: 80%; }\n.field-wide { width: 100%; }\n.box-header { font-size: 12pt; font-weight: bold; }\n.group-title { font-weight: bold; }\ntable.panel { border: 1px solid black; }\n.page-break-before { break-before: page; page-break-before: always; }\n.page-break-after { break-after: page; page-break-after: always; }\n.no-break { break-inside: avoid; page-break-inside: avoid; }\na { color: blue; }\n.dashboard { margin: 4pt 0; }\n.html-view { white-space: pre-wrap; overflow-wrap: anywhere; margin: 0 0 6pt; }\n.number-card { display: inline-block; vertical-align: top; min-width: 90pt; margin: 0 6pt 6pt 0; padding: 4pt 6pt; border: thin solid #999999; break-inside: avoid; }\n.number-card-title { font-size: 7pt; text-transform: uppercase; letter-spacing: 0.5pt; }\n.number-card-value { font-size: 14pt; font-weight: bold; }\n.nav-card { margin: 0 0 2pt 0; break-inside: avoid; }\n.nav-card-title { font-weight: bold; }\n" + "</style></head>\n<body class=\"" + $layoutName + "\">\n");
}
{
    const $label = ec.resource.expand("root ${name}");
    if (String($label).trim() !== "") sri.write("<div>" + $e($label) + "</div>\n");
}
sri.renderSubscreen();
if ($document) sri.write("</body></html>\n");
