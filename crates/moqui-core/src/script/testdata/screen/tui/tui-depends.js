sri.pushSingleFormMapContext(fieldValues);
try {
    const $hidden = [];
    const $hiddenMap = sri.getFormHiddenParameters({"name":"form-single","attrs":{"name":"Find","transition":"search"},"children":[{"name":"field","attrs":{"name":"countryGeoId"},"children":[{"name":"default-field","attrs":{"title":"Country"},"children":[{"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"US","text":"United States"}}]}]}]},{"name":"field","attrs":{"name":"stateGeoId"},"children":[{"name":"default-field","attrs":{"title":"State"},"children":[{"name":"drop-down","attrs":{},"children":[{"name":"dynamic-options","attrs":{"transition":"getGeoCountryStates","value-field":"geoId","label-field":"geoName"},"children":[{"name":"depends-on","attrs":{"field":"countryGeoId"}}]}]}]}]}],"parent":{"name":"widgets","attrs":{}}}) || {};
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
    const $made = sri.makeUrlByType("search", "transition", {"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}, "true");
$transitionUrl = $made ? ($made.url || "") : "";
$formWrites = !!$made && (!$made.isScreenUrl() || $made.isTransition()) && !$made.isReadOnly();
$openChildren({t:"form",id:$uid("Find"),name:"Find",kind:"single",transitionUrl:$transitionUrl,domId:(String("Find" ?? "") + $sec()),writes:$formWrites,method:"post",hidden:$hidden,submitMessage:"",backgroundReloadId:"",backgroundHideId:""});
    $emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{"title":"Country"},"parent":{"name":"field","attrs":{"name":"countryGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{"title":"Country"},"parent":{"name":"field","attrs":{"name":"countryGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Find.countryGeoId"),name:"countryGeoId",title:String(ec.l10n.localize("Country") || ""),w:(() => {
        const $value = String(sri.getFieldValueString({"name":"drop-down","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Country"},"parent":{"name":"field","attrs":{"name":"countryGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}}}) || "" || "");
        const $options = [];
        const $map = sri.getFieldOptions({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"US","text":"United States"}}],"parent":{"name":"default-field","attrs":{"title":"Country"},"parent":{"name":"field","attrs":{"name":"countryGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}}});
        if ($map && $map.forEach) $map.forEach(($label, $key) => $options.push({value:String($key),label:String($label == null ? "" : $label)}));
        if ($value && !$options.some(($opt) => $opt.value === $value)) $options.push({value:$value,label:$value});
        return {kind:"dropDown",value:$value,options:$options};
    })()};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{"title":"State"},"parent":{"name":"field","attrs":{"name":"stateGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{"title":"State"},"parent":{"name":"field","attrs":{"name":"stateGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Find.stateGeoId"),name:"stateGeoId",title:String(ec.l10n.localize("State") || ""),w:(() => {
            const $value = String(sri.getFieldValueString({"name":"drop-down","attrs":{},"parent":{"name":"default-field","attrs":{"title":"State"},"parent":{"name":"field","attrs":{"name":"stateGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}}}) || "" || "");
            let $label = $value;
            if ($value) {
                const $found = sri.getFieldTransitionValue("getGeoCountryStates", {"name":"dynamic-options","attrs":{"transition":"getGeoCountryStates","value-field":"geoId","label-field":"geoName"},"children":[{"name":"depends-on","attrs":{"field":"countryGeoId"}}],"parent":{"name":"drop-down","attrs":{},"parent":{"name":"default-field","attrs":{"title":"State"},"parent":{"name":"field","attrs":{"name":"stateGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}}}}, $value, "geoName", false);
                if ($found) $label = String($found);
            }
            const $w = $withRemote({kind:"dropDown",value:$value,options:$value ? [{value:$value,label:$label}] : []}, $remote(sri.makeUrlByType("getGeoCountryStates", "transition", {"name":"dynamic-options","attrs":{"transition":"getGeoCountryStates","value-field":"geoId","label-field":"geoName"},"children":[{"name":"depends-on","attrs":{"field":"countryGeoId"}}],"parent":{"name":"drop-down","attrs":{},"parent":{"name":"default-field","attrs":{"title":"State"},"parent":{"name":"field","attrs":{"name":"stateGeoId"},"parent":{"name":"form-single","attrs":{"name":"Find","transition":"search"},"parent":{"name":"widgets","attrs":{}}}}}}}, "false"), {"countryGeoId":"countryGeoId"}), "optionsUrl", "optionsParameters");
            return $w;
        })()};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$closeChildren();
} finally {
    sri.popContext();
}
