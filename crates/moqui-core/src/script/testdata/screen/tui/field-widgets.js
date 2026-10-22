sri.pushSingleFormMapContext(fv);
try {
    const $hidden = [];
    const $hiddenMap = sri.getFormHiddenParameters({"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"children":[{"name":"field","attrs":{"name":"firstName"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"text-line","attrs":{"size":"20"}}]}]},{"name":"field","attrs":{"name":"bio"},"children":[{"name":"default-field","attrs":{"title":"About"},"children":[{"name":"text-area","attrs":{"rows":"3"}}]}]},{"name":"field","attrs":{"name":"userGroupId"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"text-find","attrs":{"hide-options":"true"}}]}]},{"name":"field","attrs":{"name":"status"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"drop-down","attrs":{"no-current-selected-key":"N/A"},"children":[{"name":"option","attrs":{"key":"A","text":"Active"}},{"name":"option","attrs":{"key":"${k}"}}]}]}]},{"name":"field","attrs":{"name":"kind"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"radio","attrs":{"no-current-selected-key":"X"},"children":[{"name":"option","attrs":{"key":"X"}},{"name":"option","attrs":{"key":"Y","text":"Why"}}]}]}]},{"name":"field","attrs":{"name":"flag"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"check","attrs":{"no-current-selected-key":"${dflt}"},"children":[{"name":"option","attrs":{"key":"Y","text":"Yes"}}]}]}]},{"name":"field","attrs":{"name":"stamp"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"date-time","attrs":{}}]}]},{"name":"field","attrs":{"name":"day"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"date-time","attrs":{"type":"date","format":"MM/dd/yyyy"}}]}]},{"name":"field","attrs":{"name":"at"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"date-time","attrs":{"type":"time","default-value":"12:00"}}]}]},{"name":"field","attrs":{"name":"from"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"date-find","attrs":{}}]}]},{"name":"field","attrs":{"name":"amount"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"display","attrs":{"currency-unit-field":"uom"}}]}]},{"name":"field","attrs":{"name":"price"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"display","attrs":{"currency-unit-field":"uom","currency-hide-symbol":"true","text":"${price}"}}]}]},{"name":"field","attrs":{"name":"note"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"display","attrs":{"text":"Note ${note}","text-map":"{note: 'm'}"}}]}]},{"name":"field","attrs":{"name":"enumId"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"display-entity","attrs":{"entity-name":"moqui.basic.Enumeration","text":"${description}","default-text":"none"}}]}]},{"name":"field","attrs":{"name":"doc"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"file","attrs":{}}]}]},{"name":"field","attrs":{"name":"pw"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"password","attrs":{}}]}]},{"name":"field","attrs":{"name":"range"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"range-find","attrs":{}}]}]},{"name":"field","attrs":{"name":"clear"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"reset","attrs":{}}]}]},{"name":"field","attrs":{"name":"secret"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"hidden","attrs":{}}]}]},{"name":"field","attrs":{"name":"skip"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"ignored","attrs":{}}]}]},{"name":"field","attrs":{"name":"go"},"children":[{"name":"default-field","attrs":{"title":"Go Now"},"children":[{"name":"submit","attrs":{}}]}]},{"name":"field","attrs":{"name":"sel"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"drop-down","attrs":{},"children":[{"name":"entity-options","attrs":{"key":"${enumId}","text":"${description}"},"children":[{"name":"entity-find","attrs":{"entity-name":"moqui.basic.Enumeration"},"children":[{"name":"econdition","attrs":{"field-name":"enumTypeId","value":"T"}},{"name":"order-by","attrs":{"field-name":"description"}}]}]}]}]}]},{"name":"field","attrs":{"name":"lst"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"drop-down","attrs":{},"children":[{"name":"list-options","attrs":{"list":"opts","key":"${k}","text":"${t}"}}]}]}]},{"name":"field","attrs":{"name":"mix"},"children":[{"name":"conditional-field","attrs":{"condition":"mode == 'a'"},"children":[{"name":"text-line","attrs":{}}]},{"name":"conditional-field","attrs":{"condition":"mode == 'b'"},"children":[{"name":"hidden","attrs":{}}]},{"name":"default-field","attrs":{},"children":[{"name":"display","attrs":{}}]}]},{"name":"field","attrs":{"name":"withSet"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"set","attrs":{"field":"shown","value":"x${y}"}},{"name":"display","attrs":{"text":"${shown}"}}]}]},{"name":"field","attrs":{"name":"hidden1","hide":"true"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"display","attrs":{}}]}]},{"name":"field-layout","attrs":{},"children":[{"name":"field-row","attrs":{},"children":[{"name":"field-ref","attrs":{"name":"firstName"}},{"name":"field-ref","attrs":{"name":"bio"}}]}]}],"parent":{"name":"widgets","attrs":{}}}) || {};
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
    $openChildren({t:"form",id:$uid("AllFields"),name:"AllFields",kind:"single",multipart:true,transitionUrl:$transitionUrl,domId:(String("AllFields" ?? "") + $sec()),writes:$formWrites,method:"",hidden:$hidden,submitMessage:"",backgroundReloadId:"",backgroundHideId:""});
    $openChildren({t:"row",id:$uid("AllFields.l0")});
$emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"firstName"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"firstName"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("AllFields.firstName"),name:"firstName",title:String(ec.l10n.localize("First Name") || ""),w:{kind:"textLine",value:String(sri.getFieldValueString({"name":"text-line","attrs":{"size":"20"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"firstName"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}}) || "" || ""),size:20}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{"title":"About"},"parent":{"name":"field","attrs":{"name":"bio"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{"title":"About"},"parent":{"name":"field","attrs":{"name":"bio"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("AllFields.bio"),name:"bio",title:String(ec.l10n.localize("About") || ""),w:{kind:"textArea",value:String(sri.getFieldValueString({"name":"text-area","attrs":{"rows":"3"},"parent":{"name":"default-field","attrs":{"title":"About"},"parent":{"name":"field","attrs":{"name":"bio"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}}) || "" || ""),rows:3,editorType:""}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$closeChildren();
$closeChildren();
} finally {
    sri.popContext();
}
