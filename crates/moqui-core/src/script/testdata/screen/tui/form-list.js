{
    const $info = sri.openFormList("Rows");
    try {
        const $columns = [];
        $columns.push({title:[String(ec.l10n.localize("ID") || "")].filter(($part) => $part).join(" / "),orderBy:"id",fields:["id"]});
$columns.push({title:[String(ec.l10n.localize("The Title") || "")].filter(($part) => $part).join(" / "),printWidth:"30",printWidthType:"characters",fields:["title"]});
$columns.push({title:[String(ec.l10n.localize("Qty") || "")].filter(($part) => $part).join(" / "),printWidth:"20",printWidthType:"characters",align:"right",fields:["qty"]});
$columns.push({title:[String(ec.l10n.localize("Amt") || "")].filter(($part) => $part).join(" / "),align:"right",fields:["amt"]});
$columns.push({title:[String(ec.l10n.localize("Status") || "")].filter(($part) => $part).join(" / "),fields:["status"]});
$columns.push({title:[String(ec.l10n.localize("Open") || "")].filter(($part) => $part).join(" / "),fields:["link"]});
$columns.push({title:[String(ec.l10n.localize("Do") || "")].filter(($part) => $part).join(" / "),fields:["b"]});
const $headerFields = [];
        $headerFields.push((() => {
            let $required = false;
            const $max = 0;
            const $hint = sri.getFieldValidation({"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}});
            if ($hint && String($hint.classes || "").split(/\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules({"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:$uid("Rows.id"),name:"id",title:String(ec.l10n.localize("ID") || ""),w:{kind:"textFind",value:String(sri.getFieldValueString({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || "" || ""),op:(() => { const $v = ec.context.get("id_op"); return $v == null || $v === "" || $v === "null" ? String("contains") : String($v); })(),not:(() => { const $v = ec.context.get("id_not"); return $v == null || $v === "" || $v === "null" ? String("N") : String($v); })() === "Y",ic:(() => { const $v = ec.context.get("id_ic"); return $v == null || $v === "" || $v === "null" ? String("Y") : String($v); })() === "Y"}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            
            
            return $field;
        })());
const $rows = [];
        for (let $index = 0; $index < $info.count; $index++) {
            sri.startFormListRow($info, $index);
            try {
                const $rowKey = String($index);
                const $cells = [];
                let $parts = [];
let $start = 0;
$parts = [];
$start = $parts.length;
$addPart($parts, $start, String(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || ""));
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
$addPart($parts, $start, String(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || "" || ""));
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
$addPart($parts, $start, String(sri.getFieldValueString({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || ""));
$cells.push($cellOf($parts));
$parts = [];
if (!(hideAmt)) {
    $start = $parts.length;
    $addPart($parts, $start, String(ec.l10n.formatCurrency(sri.getFieldValue({"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}, ""), 'USD') || ""));
}
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
if (status == 'X') {
    $addPart($parts, $start, String(ec.resource.expand("Excluded") || ""));
} else {
    $addPart($parts, $start, String(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || ""));
}
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
$addPart($parts, $start, (() => {
            let $text = ec.resource.expand("Open ${id}");
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType("open", "transition", {"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}, "true");
            const $link = {t:"link",id:$uid("Rows.link." + $rowKey),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            $link.kind = "anchor";
            
            
            
            
            
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            
            return $link;
        })());
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
$addPart($parts, $start, String(ec.l10n.localize("Do") || ""));
$cells.push($cellOf($parts));
const $row = {id:$rowKey,cells:$cells};
                $rows.push($row);
            } finally {
                sri.endFormListRow();
            }
        }
        const $indexValue = "rowListPageIndex" ? ec.context.get("rowListPageIndex") : null;
        const $pageIndex = $indexValue == null || $indexValue === "" ? $whole(ec.context.get("pageIndex")) : $whole($indexValue);
        const $sizeValue = "rowListPageSize" ? ec.context.get("rowListPageSize") : null;
        const $pageSize = $sizeValue == null || $sizeValue === "" ? $whole(ec.context.get("pageSize")) : $whole($sizeValue);
        const $total = "rowListCount" ? ec.context.get("rowListCount") : null;
        const $count = $total == null || $total === "" ? $info.count : $whole($total);
        const $size = $pageSize > 0 ? $pageSize : $info.count;
        const $paging = {index:$pageIndex,size:$size,count:$count};
        for (const [$key, $suffix] of [["maxIndex", "PageMaxIndex"], ["rangeLow", "PageRangeLow"], ["rangeHigh", "PageRangeHigh"]]) {
            const $value = ec.context.get("rowList" + $suffix);
            if ($value != null && $value !== "") $paging[$key] = $whole($value);
        }
        const $list = {t:"formList",id:$uid("Rows"),name:"Rows",domId:(String("Rows" ?? "") + $sec()),columns:$columns,rows:$rows,paging:$paging,headerFields:$headerFields};
        $emit($list);
    } finally {
        sri.safeCloseList($info);
    }
}
{
    const $info = sri.openFormList("Cols");
    try {
        const $columns = [];
        $columns.push({title:[String(ec.l10n.localize("A") || ""),String(ec.l10n.localize("B") || "")].filter(($part) => $part).join(" / "),fields:["a","b"]});
$columns.push({title:[String(ec.l10n.localize("C") || "")].filter(($part) => $part).join(" / "),fields:["c"]});
const $headerFields = [];
        const $rows = [];
        for (let $index = 0; $index < $info.count; $index++) {
            sri.startFormListRow($info, $index);
            try {
                const $rowKey = String($index);
                const $cells = [];
                let $parts = [];
let $start = 0;
$parts = [];
$start = $parts.length;
$addPart($parts, $start, String(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || ""));
$start = $parts.length;
$addPart($parts, $start, String(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || ""));
$cells.push($cellOf($parts));
$parts = [];
$start = $parts.length;
$addPart($parts, $start, String(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}) || ""));
$cells.push($cellOf($parts));
const $row = {id:$rowKey,cells:$cells};
                $rows.push($row);
            } finally {
                sri.endFormListRow();
            }
        }
        const $indexValue = "rowListPageIndex" ? ec.context.get("rowListPageIndex") : null;
        const $pageIndex = $indexValue == null || $indexValue === "" ? $whole(ec.context.get("pageIndex")) : $whole($indexValue);
        const $sizeValue = "rowListPageSize" ? ec.context.get("rowListPageSize") : null;
        const $pageSize = $sizeValue == null || $sizeValue === "" ? $whole(ec.context.get("pageSize")) : $whole($sizeValue);
        const $total = "rowListCount" ? ec.context.get("rowListCount") : null;
        const $count = $total == null || $total === "" ? $info.count : $whole($total);
        const $size = $pageSize > 0 ? $pageSize : $info.count;
        const $paging = {index:$pageIndex,size:$size,count:$count};
        for (const [$key, $suffix] of [["maxIndex", "PageMaxIndex"], ["rangeLow", "PageRangeLow"], ["rangeHigh", "PageRangeHigh"]]) {
            const $value = ec.context.get("rowList" + $suffix);
            if ($value != null && $value !== "") $paging[$key] = $whole($value);
        }
        const $list = {t:"formList",id:$uid("Cols"),name:"Cols",domId:(String("Cols" ?? "") + $sec()),columns:$columns,rows:$rows,paging:$paging,headerFields:$headerFields};
        $emit($list);
    } finally {
        sri.safeCloseList($info);
    }
}
