$formList("Rows", () => ({low: rowListPageRangeLow, high: rowListPageRangeHigh, count: rowListCount}), [
    [{
        "name": "id",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "header": () => ec.l10n.localize("ID"),
        "cell": () => {
            sri.write($e(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id","ai-opt-out":"true"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
        }
    }],
    [{
        "name": "title",
        "align": "",
        "printWidth": "30",
        "printWidthType": "characters",
        "header": () => ec.l10n.localize("The Title"),
        "cell": () => {
            sri.write($e(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","ai-opt-out":"true","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
        }
    }],
    [{
        "name": "qty",
        "align": "right",
        "printWidth": "20",
        "printWidthType": "",
        "header": () => ec.l10n.localize("Qty"),
        "cell": () => {
            sri.write($e(sri.getFieldValueString({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
        }
    }],
    [{
        "name": "amt",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "hide": () => (hideAmt),
        "header": () => ec.l10n.localize("Amt"),
        "cell": () => {
            sri.write($e(ec.l10n.formatCurrency(sri.getFieldValue({"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}, ""), 'USD')));
        }
    }],
    [{
        "name": "status",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "header": () => ec.l10n.localize("Status"),
        "cell": () => {
            if (status == 'X') {
                sri.write($e(ec.resource.expand("Excluded")));
            } else {
                sri.write($e(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            }
        }
    }],
    [{
        "name": "link",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "header": () => ec.l10n.localize("Open"),
        "cell": () => {
            {
                let $linkText = ec.resource.expand("Open ${id}");
                if ($linkText === "null") $linkText = "";
                const $url = sri.makeUrlByType("open", "transition", {"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}}, "true");
                sri.write('<a href="' + $e($url.getUrlWithParams()) + '">' + $e($linkText) + "</a>");
            }
        }
    }],
    [{
        "name": "b",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "header": () => ec.l10n.localize("Do"),
        "cell": () => {
        }
    }]
]);
$formList("Cols", () => ({low: rowListPageRangeLow, high: rowListPageRangeHigh, count: rowListCount}), [
    [{
        "name": "a",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "header": () => ec.l10n.localize("A"),
        "cell": () => {
            sri.write($e(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
        }
    },
    {
        "name": "b",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "header": () => ec.l10n.localize("B"),
        "cell": () => {
            sri.write($e(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
        }
    }],
    [{
        "name": "c",
        "align": "",
        "printWidth": "",
        "printWidthType": "",
        "header": () => ec.l10n.localize("C"),
        "cell": () => {
            sri.write($e(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
        }
    }]
]);
