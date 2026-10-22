{
    const $rawValue = sri.renderInlineText("for any mode ${name}");
    $emit(Object.assign({t:"html",id:$uid("widgets/render-mode[0]/text[1]")}, JSON.parse(sri.htmlText(String($rawValue == null ? "" : $rawValue)))));
}
$emit({t:"placeholder",id:$uid("widgets/render-mode[1]"),text:"render-mode",url:(() => { const $info = sri.getScreenUrlInfo(); const $names = $info && $info.fullPathNameList; if ($names && $names.length) return "/" + $names.join("/"); const $screen = sri.getActiveScreenDef(); return ($screen && $screen.location) || ""; })()});
$emit({t:"placeholder",id:$uid("widgets/text[0]"),text:"text",url:(() => { const $info = sri.getScreenUrlInfo(); const $names = $info && $info.fullPathNameList; if ($names && $names.length) return "/" + $names.join("/"); const $screen = sri.getActiveScreenDef(); return ($screen && $screen.location) || ""; })()});
$emit({t:"placeholder",id:$uid("widgets/text[1]"),text:"text",url:(() => { const $info = sri.getScreenUrlInfo(); const $names = $info && $info.fullPathNameList; if ($names && $names.length) return "/" + $names.join("/"); const $screen = sri.getActiveScreenDef(); return ($screen && $screen.location) || ""; })()});
{
    const $rawValue = sri.renderTextValue(ec.resource.expandNoL10n("component://x/template/Part.txt"), "false");
    $emit(Object.assign({t:"html",id:$uid("widgets/text[2]")}, JSON.parse(sri.htmlText(String($rawValue == null ? "" : $rawValue)))));
}
{
    const $rawValue = sri.renderTextValue(ec.resource.expandNoL10n("component://x/template/${name}.txt"), "true");
    $emit(Object.assign({t:"html",id:$uid("widgets/text[3]")}, JSON.parse(sri.htmlText(String($rawValue == null ? "" : $rawValue)))));
}
{
    const $rawValue = "a < b";
    $emit(Object.assign({t:"html",id:$uid("widgets/text[4]")}, JSON.parse(sri.htmlText(String($rawValue == null ? "" : $rawValue)))));
}
{
    const $rawValue = sri.renderInlineText("quiet ${name}");
    $emit(Object.assign({t:"html",id:$uid("widgets/text[5]")}, JSON.parse(sri.htmlText(String($rawValue == null ? "" : $rawValue)))));
}
