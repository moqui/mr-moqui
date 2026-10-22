sri.pushSingleFormMapContext(fieldValues);
try {
    const $hidden = [];
    const $hiddenMap = sri.getFormHiddenParameters({"name":"form-single","attrs":{"name":"LinkForm"},"children":[{"name":"field","attrs":{"name":"a"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"link","attrs":{"url":"go","text":"Go ${a}","link-type":"anchor"}}]}]},{"name":"field","attrs":{"name":"b"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"link","attrs":{"url":"go","text":"Hidden ${b}","link-type":"hidden-form-link","condition":"b != null"}}]}]},{"name":"field","attrs":{"name":"c"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"link","attrs":{"url":"go","text":"Plain","link-type":"auto"}}]}]},{"name":"field","attrs":{"name":"d"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"link","attrs":{"url":"go","entity-name":"moqui.basic.Enumeration","link-type":"anchor","text":"${description}"}}]}]},{"name":"field","attrs":{"name":"e"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"link","attrs":{"url":"go","text":"Mapped ${v}","text-map":"{v: 1}","link-type":"anchor"}}]}]}],"parent":{"name":"widgets","attrs":{}}}) || {};
    const $hiddenKeys = Object.keys($hiddenMap);
    for (let $hi = 0; $hi < $hiddenKeys.length; $hi++) {
        const $key = $hiddenKeys[$hi];
        const $value = $hiddenMap[$key];
        if ($value == null || $value === "") continue;
        if ($secret.has(String($key).toLowerCase())) continue;
        $hidden.push({name:String($key),value:String($value)});
    }
    let $transitionUrl = "";
    let $formWrites = false;
    $openChildren({t:"form",id:$uid("LinkForm"),name:"LinkForm",kind:"single",transitionUrl:$transitionUrl,domId:(String("LinkForm" ?? "") + $sec()),writes:$formWrites,method:"",hidden:$hidden,submitMessage:"",backgroundReloadId:"",backgroundHideId:""});
    $emit((() => {
            let $text = ec.resource.expand("Go ${a}");
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("go", "transition", {"name":"link","attrs":{"url":"go","text":"Go ${a}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, "true");
            const $link = {t:"link",id:$uid("LinkForm.a"),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            $link.kind = "anchor";
            
            
            
            
            
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"go","text":"Go ${a}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
if (b != null) {
$emit((() => {
            let $text = ec.resource.expand("Hidden ${b}");
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("go", "transition", {"name":"link","attrs":{"url":"go","text":"Hidden ${b}","link-type":"hidden-form-link","condition":"b != null"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, "true");
            const $link = {t:"link",id:$uid("LinkForm.b"),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            $link.kind = "hidden-form-link";
            
            
            
            
            $link.method = "post";
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"go","text":"Hidden ${b}","link-type":"hidden-form-link","condition":"b != null"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
}
$emit((() => {
            let $text = ec.resource.expand("Plain");
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("go", "transition", {"name":"link","attrs":{"url":"go","text":"Plain","link-type":"auto"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, "true");
            const $link = {t:"link",id:$uid("LinkForm.c"),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            $link.kind = "auto";
            
            
            
            
            
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"go","text":"Plain","link-type":"auto"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
$emit((() => {
            let $text = sri.getFieldEntityValue({"name":"link","attrs":{"url":"go","entity-name":"moqui.basic.Enumeration","link-type":"anchor","text":"${description}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}});
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("go", "transition", {"name":"link","attrs":{"url":"go","entity-name":"moqui.basic.Enumeration","link-type":"anchor","text":"${description}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, "true");
            const $link = {t:"link",id:$uid("LinkForm.d"),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            $link.kind = "anchor";
            
            
            
            
            
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"go","entity-name":"moqui.basic.Enumeration","link-type":"anchor","text":"${description}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"d"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
$emit((() => {
            let $text = ec.resource.expand("Mapped ${v}", "", {v: 1});
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("go", "transition", {"name":"link","attrs":{"url":"go","text":"Mapped ${v}","text-map":"{v: 1}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"e"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, "true");
            const $link = {t:"link",id:$uid("LinkForm.e"),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            $link.kind = "anchor";
            
            
            
            
            
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"go","text":"Mapped ${v}","text-map":"{v: 1}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"e"},"parent":{"name":"form-single","attrs":{"name":"LinkForm"},"parent":{"name":"widgets","attrs":{}}}}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
$closeChildren();
} finally {
    sri.popContext();
}
$emit((() => {
            let $text = ec.resource.expand("Standalone");
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("go", "transition", {"name":"link","attrs":{"url":"go","text":"Standalone"},"parent":{"name":"widgets","attrs":{}}}, "true");
            const $link = {t:"link",id:$uid("widgets/link"),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            
            
            
            
            
            
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"go","text":"Standalone"},"parent":{"name":"widgets","attrs":{}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
if (name) {
$emit({t:"text",id:$uid("widgets/label[0]"),text:ec.resource.expand("Cond ${name}")});
}
$emit({t:"text",id:$uid("widgets/label[1]"),text:ec.resource.expand("Mapped ${v}")});
$emit({t:"image",id:$uid("widgets/image"),alt:String(ec.resource.expand("An image") || ""),url:String(ec.resource.expandNoL10n("/img.png") || "")});
