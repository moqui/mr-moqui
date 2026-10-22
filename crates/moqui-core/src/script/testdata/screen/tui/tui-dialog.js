$openChildren({t:"dialog",id:$uid("New"),title:ec.resource.expand("Create"),button:ec.resource.expand("New ${kind}")});
$emit({t:"text",id:$uid("widgets/New/label"),text:ec.resource.expand("in dialog")});
sri.pushSingleFormMapContext(fieldValues);
try {
    const $hidden = [];
    const $hiddenMap = sri.getFormHiddenParameters({"name":"form-single","attrs":{"name":"NewForm","transition":"create","focus-field":"name","background-hide-id":"New","submit-message":"Saved"},"children":[{"name":"field","attrs":{"name":"name"},"children":[{"name":"default-field","attrs":{"title":"Name"},"children":[{"name":"text-line","attrs":{}}]}]}],"parent":{"name":"container-dialog","attrs":{"id":"New","button-text":"New ${kind}","title":"Create"},"parent":{"name":"widgets","attrs":{}}}}) || {};
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
    const $made = sri.makeUrlByType("create", "transition", {"name":"form-single","attrs":{"name":"NewForm","transition":"create","focus-field":"name","background-hide-id":"New","submit-message":"Saved"},"parent":{"name":"container-dialog","attrs":{"id":"New","button-text":"New ${kind}","title":"Create"},"parent":{"name":"widgets","attrs":{}}}}, "true");
$transitionUrl = $made ? ($made.url || "") : "";
$formWrites = !!$made && (!$made.isScreenUrl() || $made.isTransition()) && !$made.isReadOnly();
const $focusName = ec.resource.expand("name", "");
if ($focusName && $focusName !== "null") sri.setFocusField("NewForm" + "." + $focusName);
$openChildren({t:"form",id:$uid("NewForm"),name:"NewForm",kind:"single",transitionUrl:$transitionUrl,domId:(String("NewForm" ?? "") + $sec()),writes:$formWrites,method:"post",hidden:$hidden,submitMessage:ec.resource.expand("Saved"),backgroundReloadId:"",backgroundHideId:"New"});
    $emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{"title":"Name"},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-single","attrs":{"name":"NewForm","transition":"create","focus-field":"name","background-hide-id":"New","submit-message":"Saved"},"parent":{"name":"container-dialog","attrs":{"id":"New","button-text":"New ${kind}","title":"Create"},"parent":{"name":"widgets","attrs":{}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{"title":"Name"},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-single","attrs":{"name":"NewForm","transition":"create","focus-field":"name","background-hide-id":"New","submit-message":"Saved"},"parent":{"name":"container-dialog","attrs":{"id":"New","button-text":"New ${kind}","title":"Create"},"parent":{"name":"widgets","attrs":{}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("NewForm.name"),name:"name",title:String(ec.l10n.localize("Name") || ""),w:{kind:"textLine",value:String(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Name"},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-single","attrs":{"name":"NewForm","transition":"create","focus-field":"name","background-hide-id":"New","submit-message":"Saved"},"parent":{"name":"container-dialog","attrs":{"id":"New","button-text":"New ${kind}","title":"Create"},"parent":{"name":"widgets","attrs":{}}}}}}}) || "" || "")}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$closeChildren();
} finally {
    sri.popContext();
}
$closeChildren();
