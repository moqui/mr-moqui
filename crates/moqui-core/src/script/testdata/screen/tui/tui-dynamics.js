sri.pushSingleFormMapContext(fieldValues);
try {
    const $hidden = [];
    const $hiddenMap = sri.getFormHiddenParameters({"name":"form-single","attrs":{"name":"Edit","transition":"save"},"children":[{"name":"field","attrs":{"name":"partyId"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"text-line","attrs":{"ac-transition":"searchParties","ac-min-length":"2"},"children":[{"name":"depends-on","attrs":{"field":"orgId"}}]}]}]},{"name":"field","attrs":{"name":"city"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"text-line","attrs":{"prefix":"@","disabled":"!canEdit","default-transition":"getCity"}}]}]},{"name":"field","attrs":{"name":"note"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"editable","attrs":{"text":"${note}","transition":"saveNote","widget-type":"select"},"children":[{"name":"option","attrs":{"key":"a"}}]}]}]},{"name":"field","attrs":{"name":"upload"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"file","attrs":{}}]}]}],"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}) || {};
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
    const $made = sri.makeUrlByType("save", "transition", {"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}, "true");
$transitionUrl = $made ? ($made.url || "") : "";
$formWrites = !!$made && (!$made.isScreenUrl() || $made.isTransition()) && !$made.isReadOnly();
$openChildren({t:"form",id:$uid("Edit"),name:"Edit",kind:"single",multipart:true,transitionUrl:$transitionUrl,domId:(String("Edit" ?? "") + $sec()),writes:$formWrites,method:"post",hidden:$hidden,submitMessage:"",backgroundReloadId:"",backgroundHideId:""});
    $emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"partyId"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"partyId"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Edit.partyId"),name:"partyId",title:String(ec.l10n.localize("Party ID") || ""),w:(() => {
            const $value = String(sri.getFieldValueString({"name":"text-line","attrs":{"ac-transition":"searchParties","ac-min-length":"2"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"partyId"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || "" || "");
            let $label = $value;
            if ($value) {
                const $found = sri.getFieldTransitionValue("searchParties", {"name":"text-line","attrs":{"ac-transition":"searchParties","ac-min-length":"2"},"children":[{"name":"depends-on","attrs":{"field":"orgId"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"partyId"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}, $value, "label", false);
                if ($found != null && $found !== "" && $found !== "null") $label = String($found);
            }
            const $w = $withRemote({kind:"dropDown",value:$value,options:$value ? [{value:$value,label:$label}] : [],serverSearch:true,allowEmpty:true}, $remote(sri.makeUrlByType("searchParties", "transition", {"name":"text-line","attrs":{"ac-transition":"searchParties","ac-min-length":"2"},"children":[{"name":"depends-on","attrs":{"field":"orgId"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"partyId"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}, "false"), {"orgId":"orgId"}), "optionsUrl", "optionsParameters");
            $w.minLength = 2;
            return $w;
        })()};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"city"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"city"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Edit.city"),name:"city",title:String(ec.l10n.localize("City") || ""),w:(() => { const $w = {kind:"textLine",value:String(sri.getFieldValueString({"name":"text-line","attrs":{"prefix":"@","disabled":"!canEdit","default-transition":"getCity"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"city"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || "" || "")}; $w.prefix = String(ec.resource.expand("@") || ""); if ((!canEdit)) $w.disabled = true; $withRemote($w, $remote(sri.makeUrlByType("getCity", "transition", {"name":"text-line","attrs":{"prefix":"@","disabled":"!canEdit","default-transition":"getCity"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"city"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}, "false"), {}), "defaultUrl", "defaultParameters"); return $w; })()};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
{
    const $node = (() => {
            const $w = (() => {
            let $text = ec.resource.expand("${note}", "");
            $text = $text == null || $text === "null" ? "" : String($text);
            if (!$text.trim() && !(false)) return null;
            const $made = sri.makeUrlByType("saveNote", "transition", {"name":"editable","attrs":{"text":"${note}","transition":"saveNote","widget-type":"select"},"children":[{"name":"option","attrs":{"key":"a"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}, "true");
            const $w = {kind:"editable",text:$text,url:$made ? ($made.url || "") : "",parameterName:"value",widgetType:"select"};
            const $params = $strMap($made ? $made.getParameterMap() : null, true);
            if (Object.keys($params).length) $w.parameters = $params;
            const $map = sri.getFieldOptions({"name":"editable","attrs":{"text":"${note}","transition":"saveNote","widget-type":"select"},"children":[{"name":"option","attrs":{"key":"a"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}});
            const $options = [];
            if ($map && $map.forEach) $map.forEach(($label, $key) => $options.push({value:String($key),label:String($label == null ? "" : $label)}));
            if ($options.length) $w.options = $options;
            return $w;
        })();
            if ($w == null) return null;
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Edit.note"),name:"note",title:String(ec.l10n.localize("Note") || ""),w:$w};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })();
    if ($node) $emit($node);
}
$emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"upload"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"upload"},"parent":{"name":"form-single","attrs":{"name":"Edit","transition":"save"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Edit.upload"),name:"upload",title:String(ec.l10n.localize("Upload") || ""),w:{kind:"file"}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$closeChildren();
} finally {
    sri.popContext();
}
{
    const $info = sri.openFormList("Rows");
    try {
        const $uiType = String(ec.context.get("_uiType") || "");
        const $set = $uiType === "mobile" ? 1 : 0;
        let $listForm = false;
        let $transitionUrl = "";
        let $listWrites = false;
        const $listHidden = [];
        {
            const $made = sri.makeUrlByType("saveRows", "transition", null, "false");
            if ($made && !$made.getDisableLink()) {
                $listForm = true;
                $transitionUrl = $made.url || "";
                $listWrites = $writes($made);
                const $hiddenMap = Object.assign({}, $made.getParameterMap() || {}, sri.getFormHiddenParameters({"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"}}) || {});
                $hiddenMap.moquiFormName = "Rows";
                $hiddenMap._isMulti = "true";
                $hiddenMap.pageIndex = String(ec.context.get("pageIndex") || "0");
                const $order = ec.context.get("orderByField");
                if ($order != null && $order !== "") $hiddenMap.orderByField = String($order);
                for (const $key of Object.keys($hiddenMap)) {
                    const $value = $hiddenMap[$key];
                    if ($value == null || $value === "" || $secret.has(String($key).toLowerCase())) continue;
                    $listHidden.push({name:String($key),value:String($value)});
                }
            }
        }
        const $columns = [];
        if ($set === 1) {
$columns.push({title:[String(ec.l10n.localize("Name") || "")].filter(($part) => $part).join(" / "),fields:["name"]});
} else {
$columns.push({title:[String(ec.l10n.localize("Name") || ""),String(ec.l10n.localize("Qty") || "")].filter(($part) => $part).join(" / "),fields:["name","qty"]});
$columns.push({title:[String(ec.l10n.localize("When") || "")].filter(($part) => $part).join(" / "),fields:["when"]});
$columns.push({title:[String(ec.l10n.localize("Go") || "")].filter(($part) => $part).join(" / "),fields:["go"]});
}
const $headerFields = [];
        const $rows = [];
        for (let $index = 0; $index < $info.count; $index++) {
            sri.startFormListRow($info, $index);
            try {
                const $rowKey = String($index);
                const $cells = [];
                if ($set === 1) {
let $parts = [];
let $start = 0;
$parts = [];
$start = $parts.length;
$addPart($parts, $start, ($listForm ? (() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Rows.name." + $rowKey),name:"name_" + $index,title:String(ec.l10n.localize("Name") || ""),w:{kind:"textLine",value:String(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || "" || "")}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })() : String(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || "" || "")));
$cells.push($cellOf($parts));
} else {
let $parts = [];
let $start = 0;
$parts = [];
$start = $parts.length;
$addPart($parts, $start, ($listForm ? (() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Rows.name." + $rowKey),name:"name_" + $index,title:String(ec.l10n.localize("Name") || ""),w:{kind:"textLine",value:String(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || "" || "")}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })() : String(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"name"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || "" || "")));
$start = $parts.length;
$addPart($parts, $start, String(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || ""));
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
$addPart($parts, $start, $range((() => { const $raw = (() => { const $v = ec.context.get("when_from"); return $v == null || $v === "" || $v === "null" ? String("") : String($v); })(); if (!$raw) return ""; try { return String(ec.l10n.format($raw, "yyyy-MM-dd HH:mm")); } catch ($err) { return $raw; } })(), (() => { const $raw = (() => { const $v = ec.context.get("when_thru"); return $v == null || $v === "" || $v === "null" ? String("") : String($v); })(); if (!$raw) return ""; try { return String(ec.l10n.format($raw, "yyyy-MM-dd HH:mm")); } catch ($err) { return $raw; } })()));
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
$addPart($parts, $start, ($listForm ? "" : String(ec.l10n.localize("Go") || "")));
$cells.push($cellOf($parts));
}
const $row = {id:$rowKey,cells:$cells};
                if ($listForm) {
                    const $rowHidden = [];
                    $rowHidden.push({name:"id_" + $index,value:String(sri.getFieldValuePlainString({"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}, "") || "")});
$rowHidden.push({name:"qty_" + $index,value:String(sri.getFieldValuePlainString({"name":"field","attrs":{"name":"qty"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}, "") || "")});
if ($rowHidden.length) $row.hidden = $rowHidden;
                }
                const $selectId = ec.context.get("id");
                if ($selectId != null && $selectId !== "") $row.selectId = String($selectId);
                $rows.push($row);
            } finally {
                sri.endFormListRow();
            }
        }
        const $indexValue = "rowsPageIndex" ? ec.context.get("rowsPageIndex") : null;
        const $pageIndex = $indexValue == null || $indexValue === "" ? $whole(ec.context.get("pageIndex")) : $whole($indexValue);
        const $sizeValue = "rowsPageSize" ? ec.context.get("rowsPageSize") : null;
        const $pageSize = $sizeValue == null || $sizeValue === "" ? $whole(ec.context.get("pageSize")) : $whole($sizeValue);
        const $total = "rowsCount" ? ec.context.get("rowsCount") : null;
        const $count = $total == null || $total === "" ? $info.count : $whole($total);
        const $size = $pageSize > 0 ? $pageSize : $info.count;
        const $paging = {index:$pageIndex,size:$size,count:$count};
        for (const [$key, $suffix] of [["maxIndex", "PageMaxIndex"], ["rangeLow", "PageRangeLow"], ["rangeHigh", "PageRangeHigh"]]) {
            const $value = ec.context.get("rows" + $suffix);
            if ($value != null && $value !== "") $paging[$key] = $whole($value);
        }
        const $list = {t:"formList",id:$uid("Rows"),name:"Rows",domId:(String("Rows" ?? "") + $sec()),columns:$columns,rows:$rows,paging:$paging,headerFields:$headerFields};
        $list.rowSelection = {idField:"id",parameter:"id"};
        if ($listForm) {
            $list.transitionUrl = $transitionUrl;
            if ($listWrites) $list.writes = true;
            $list.multi = true;
            if ($listHidden.length) $list.hidden = $listHidden;
            if ($info.count > 0) {
                const $buttons = [];
                $buttons.push((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Rows.go"),name:"go",title:String(ec.l10n.localize("Go") || ""),w:{kind:"submit",text:String(ec.l10n.localize("Go") || "")}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
if ($buttons.length) $list.buttons = $buttons;
            }
            }
        try {
            const $saveUrl = sri.buildUrl("formSaveFind");
            if ($saveUrl && !$saveUrl.getDisableLink()) {
                const $base = sri.getScreenUrlInstance().path;
                const $active = String(ec.context.get("formListFindId") || "");
                const $default = String(sri.getUserDefaultFormListFindId() || "");
                const $finds = [];
                for (const $find of Array.from(sri.getUserFormListFinds("Rows") || [])) {
                    const $id = String(($find.formListFind && $find.formListFind.formListFindId) || "");
                    if (!$id) continue;
                    $finds.push({id:$id,name:String($find.description || $id),url:$base + "?formListFindId=" + encodeURIComponent($id),active:$id === $active,default:$id === $default,mine:String($find.isByUserId) === "true"});
                }
                $list.savedFinds = {saveUrl:$saveUrl.path,formLocation:String(sri.getSavedFindFullLocation("Rows") || ""),parameters:$findParameters(),clearUrl:$base + "?formListFindId=_clear",finds:$finds};
            }
        } catch ($err) {
            ec.logger.error("Leaving the saved finds of form-list Rows out: " + $err);
        }
        try {
            const $columnsUrl = sri.buildUrl("formSelectColumns");
            if ($columnsUrl && !$columnsUrl.getDisableLink()) {
                $list.selectColumns = {url:$columnsUrl.path,formLocation:"Rows",parameters:$findParameters(),shown:($set === 1 ? [[{name:"name",title:String(ec.l10n.localize("Name") || "")}]].map(($col) => $col.filter(($choice) => $choice)).filter(($col) => $col.length) : [[{name:"name",title:String(ec.l10n.localize("Name") || "")},{name:"qty",title:String(ec.l10n.localize("Qty") || "")}],[{name:"when",title:String(ec.l10n.localize("When") || "")}],[{name:"go",title:String(ec.l10n.localize("Go") || "")}]].map(($col) => $col.filter(($choice) => $choice)).filter(($col) => $col.length)),hidden:($set === 1 ? [{name:"qty",title:String(ec.l10n.localize("Qty") || "")},{name:"when",title:String(ec.l10n.localize("When") || "")},{name:"go",title:String(ec.l10n.localize("Go") || "")}] : [])};
            }
        } catch ($err) {
            ec.logger.error("Leaving the column choice of form-list Rows out: " + $err);
        }
        try {
            const $exports = [];
            $exports.push({kind:"csv",label:String(ec.l10n.localize("CSV")),url:$screenUrlWith([["renderMode","csv"],["pageNoLimit","true"],["lastStandalone","true"],["saveFilename","Rows.csv"]])});
{
    const $noLimit = ec.context.get("pageNoLimit");
    const $all = $noLimit != null && $noLimit !== "" && String($noLimit) !== "false";
    $exports.push({kind:"all",label:String(ec.l10n.localize($all ? "Paginate" : "Show All")),url:$all ? $screenUrlWith([], "pageNoLimit") : $screenUrlWith([["pageNoLimit", "true"]])});
}
if ($exports.length) $list.exports = $exports;
        } catch ($err) {
            ec.logger.error("Leaving the exports of form-list Rows out: " + $err);
        }
        $emit($list);
        $openChildren({t:"dialog",id:$uid("Rows.action0"),title:String(ec.resource.expand("Bulk") || ""),button:String(ec.resource.expand("Bulk") || ""),list:$list.id});
        {
        sri.pushSingleFormMapContext(fieldValues);
try {
    const $hidden = [];
    const $hiddenMap = sri.getFormHiddenParameters({"name":"form-single","attrs":{"name":"Bulk","transition":"bulk"},"children":[{"name":"field","attrs":{"name":"status"},"children":[{"name":"default-field","attrs":{},"children":[{"name":"text-line","attrs":{}}]}]}],"parent":{"name":"action","attrs":{},"parent":{"name":"row-selection","attrs":{"id-field":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}) || {};
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
    const $made = sri.makeUrlByType("bulk", "transition", {"name":"form-single","attrs":{"name":"Bulk","transition":"bulk"},"parent":{"name":"action","attrs":{},"parent":{"name":"row-selection","attrs":{"id-field":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}, "true");
$transitionUrl = $made ? ($made.url || "") : "";
$formWrites = !!$made && (!$made.isScreenUrl() || $made.isTransition()) && !$made.isReadOnly();
$openChildren({t:"form",id:$uid("Bulk"),name:"Bulk",kind:"single",transitionUrl:$transitionUrl,domId:(String("Bulk" ?? "") + $sec()),writes:$formWrites,method:"post",hidden:$hidden,submitMessage:"",backgroundReloadId:"",backgroundHideId:""});
    $emit((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"Bulk","transition":"bulk"},"parent":{"name":"action","attrs":{},"parent":{"name":"row-selection","attrs":{"id-field":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"Bulk","transition":"bulk"},"parent":{"name":"action","attrs":{},"parent":{"name":"row-selection","attrs":{"id-field":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Bulk.status"),name:"status",title:String(ec.l10n.localize("Status") || ""),w:{kind:"textLine",value:String(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"Bulk","transition":"bulk"},"parent":{"name":"action","attrs":{},"parent":{"name":"row-selection","attrs":{"id-field":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rows","transition":"saveRows","multi":"true","saved-finds":"true","select-columns":"true","show-csv-button":"true","show-all-button":"true"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}}}}}}) || "" || "")}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
$closeChildren();
} finally {
    sri.popContext();
}
}
        $closeChildren();
            } finally {
        sri.safeCloseList($info);
    }
}
{
    const $barId = $uid("Bar");
    const $rawStatus = ec.resource.expandNoL10n("${statusId}");
    const $statusId = $rawStatus == null ? "" : String($rawStatus).trim();
    let $bar = null;
    if ($statusId && $statusId !== "null") {
        try {
            const $readOnly = String("") === "true";
            const $out = ec.service.sync().name("org.moqui.impl.StatusServices.get#StatusBar").parameters({statusId:$statusId,statusTypeId:"" || null,statusFlowId:"" || null,stageStatusIds:$ids(ec.resource.expression("stageIds", "")),allowedToStatusIds:$readOnly ? [] : $ids("A,B")}).disableAuthz().call() || {};
            $bar = {readOnly:$readOnly,current:$out.current || null,stages:Array.from($out.stages || []),transitions:$readOnly ? [] : Array.from($out.transitions || [])};
        } catch ($err) {
            ec.logger.error("Leaving the stages of status-bar Bar out for status " + $statusId + ": " + $err);
        }
    }
    const $text = $bar && $bar.current && $bar.current.label != null && $bar.current.label !== "" ? String($bar.current.label) : String(((id) => { const v = String(id === null || id === undefined ? "" : id).trim(); if (!v || v === "null") return ""; const item = ec.entity.find("moqui.basic.StatusItem").condition("statusId", v).useCache(true).disableAuthz().one(); return item && item.description ? String(item.description) : v; })(ec.resource.expandNoL10n("${statusId}")) || "");
    const $stages = $bar ? $bar.stages.map(($stage) => ({statusId:$stage.statusId == null ? "" : String($stage.statusId),label:$stage.label == null ? "" : String($stage.label),state:$stage.state == null ? "" : String($stage.state)})) : [];
    $openChildren({t:"statusBar",id:$barId,text:$text,stages:$stages});
    if ($bar && !$bar.readOnly && $bar.transitions.length) {
        const $made = sri.makeUrlByType("setStatus", "transition", {"name":"status-bar","attrs":{"id":"Bar","status-id":"${statusId}","transition":"setStatus","stages":"${stageIds}","allowed-status-ids":"A,B","title":"Order ${orderId}"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}, "true");
        if ($made && !$made.getDisableLink()) {
            const $title = String(ec.resource.expand("Order ${orderId}") || "");
            const $confirm = String("") !== "false";
            for (const $tr of $bar.transitions) {
                const $to = $tr.toStatusId == null ? "" : String($tr.toStatusId);
                if (!$to) continue;
                const $label = $tr.label == null || $tr.label === "" ? $to : String($tr.label);
                const $link = {t:"link",id:$uid($barId + "." + $to),text:$label,url:$made.cloneUrlInstance().addParameter("statusId", $to).getUrlWithParams() || "",method:"post",kind:"hidden-form-link",btnType:$tr.isNext ? "primary" : ($tr.isDanger ? "danger" : "")};
                if ($confirm) $link.confirm = $label + ($title ? ": " + $title : "") + "?";
                if ($writes($made)) $link.writes = true;
                $emit($link);
            }
        }
    }
    $closeChildren();
}
{
    const $made = sri.makeUrlByType("actions", "transition", {"name":"tree","attrs":{"name":"Cats","open-path":"${path}"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}, "true");
    const $tree = {t:"tree",id:$uid("Cats"),url:$made ? ($made.path || "") + "/Cats" : ""};
    const $open = ec.resource.expandNoL10n("${path}");
    if ($open != null && $open !== "" && $open !== "null") $tree.openPath = String($open);
    const $params = $strMap($made ? $made.getParameterMap() : null, true);
    if (Object.keys($params).length) $tree.parameters = $params;
    $emit($tree);
}
if (showMenu) {
{
    let $text = ec.resource.expand("More");
    if ($text == null || $text === "null") $text = "";
    $openChildren({t:"buttonMenu",id:$uid("widgets/button-menu"),text:String($text)});
}
$emit((() => {
            let $text = ec.resource.expand("A");
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("/a", "plain", {"name":"link","attrs":{"url":"/a","url-type":"plain","text":"A"},"parent":{"name":"button-menu","attrs":{"text":"More","condition":"showMenu"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}, "true");
            const $link = {t:"link",id:$uid("widgets/button-menu/link"),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            
            
            
            
            
            
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"/a","url-type":"plain","text":"A"},"parent":{"name":"button-menu","attrs":{"text":"More","condition":"showMenu"},"parent":{"name":"widgets","attrs":{},"parent":{"name":"screen","attrs":{"require-authentication":"false"}}}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
$closeChildren();
}
{
                const $menu = sri.getMenuItems() || [];
        const $items = [];
        for (let $mi = 0; $mi < $menu.length; $mi++) {
            const $item = $menu[$mi];
            $items.push({title:$item.title || "",url:$item.url || "",active:!!$item.inPath});
        }
$openRender({t:"subscreens",id:$uid("Steps"),kind:"wizard",items:$items,finishText:String(ec.resource.expand("Done") || "")});
                sri.renderSubscreen();
                $closeRender();
            }
