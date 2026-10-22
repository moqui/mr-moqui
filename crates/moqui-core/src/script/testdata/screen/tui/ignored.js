{
    let $text = ec.resource.expand("Menu");
    if ($text == null || $text === "null") $text = "";
    $openChildren({t:"buttonMenu",id:$uid("widgets/button-menu"),text:String($text)});
}
$closeChildren();
{
    const $made = sri.makeUrlByType("load", "transition", {"name":"dynamic-container","attrs":{"id":"Dyn","transition":"load"},"parent":{"name":"widgets","attrs":{}}}, "true");
    $emit({t:"dynamicContainer",id:$uid("Dyn"),url:$made ? ($made.addParameter("_dynamic_container_id", "Dyn").getUrlWithParams() || "") : ""});
}
{
    const $made = sri.makeUrlByType("load", "transition", {"name":"dynamic-dialog","attrs":{"id":"DynD","button-text":"Dyn","transition":"load"},"parent":{"name":"widgets","attrs":{}}}, "true");
    const $button = String(ec.resource.expand("Dyn") || "");
    $emit({t:"dynamicDialog",id:$uid("DynD"),url:$made ? ($made.getUrlWithParams() || "") : "",button:$button,title:$button});
}
$emit({t:"placeholder",id:$uid("widgets/include-content"),text:"include-content",url:String(ec.resource.expandNoL10n("component://x/y.html") || "")});
{
    const $made = sri.makeUrlByType("t", "transition", {"name":"tree","attrs":{"name":"T","transition":"t","root-node-name":"n"},"parent":{"name":"widgets","attrs":{}}}, "true");
    const $tree = {t:"tree",id:$uid("T"),url:$made ? ($made.path || "") : ""};
    const $open = "";
    if ($open != null && $open !== "" && $open !== "null") $tree.openPath = String($open);
    const $params = $strMap($made ? $made.getParameterMap() : null, true);
    if (Object.keys($params).length) $tree.parameters = $params;
    $emit($tree);
}
