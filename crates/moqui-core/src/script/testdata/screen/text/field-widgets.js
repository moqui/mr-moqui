$lineChars = Number(lineCharacters || "132");
$pageLines = pageLines ? Number(pageLines) : 0;
$lineWrap = lineWrap === "true";
sri.pushSingleFormMapContext(fv);
try {
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("First Name"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(sri.getFieldValueString({"name":"text-line","attrs":{"size":"20"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"firstName"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("About"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(sri.getFieldValueString({"name":"text-area","attrs":{"rows":"3"},"parent":{"name":"default-field","attrs":{"title":"About"},"parent":{"name":"field","attrs":{"name":"bio","ai-opt-out":"true"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("User Group ID"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(sri.getFieldValueString({"name":"text-find","attrs":{"hide-options":"true"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"userGroupId"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Status"), $lineChars * 0.2, true) + ": ");
    {
        const $options = sri.getFieldOptions({"name":"drop-down","attrs":{"no-current-selected-key":"N/A"},"children":[{"name":"option","attrs":{"key":"A","text":"Active"}},{"name":"option","attrs":{"key":"${k}"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        let $current = sri.getFieldValueString({"name":"drop-down","attrs":{"no-current-selected-key":"N/A"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        if (!$current) $current = "N/A";
        const $text = $options.get($current);
        sri.write($pad($text != null ? $text : $current));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Kind"), $lineChars * 0.2, true) + ": ");
    {
        const $options = sri.getFieldOptions({"name":"radio","attrs":{"no-current-selected-key":"X"},"children":[{"name":"option","attrs":{"key":"X"}},{"name":"option","attrs":{"key":"Y","text":"Why"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"kind"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        let $current = sri.getFieldValueString({"name":"radio","attrs":{"no-current-selected-key":"X"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"kind"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        if (!$current) $current = "X";
        const $text = $options.get($current);
        sri.write($pad($text != null ? $text : $current));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Flag"), $lineChars * 0.2, true) + ": ");
    {
        const $options = sri.getFieldOptions({"name":"check","attrs":{"no-current-selected-key":"${dflt}"},"children":[{"name":"option","attrs":{"key":"Y","text":"Yes"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"flag"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        let $current = sri.getFieldValueString({"name":"check","attrs":{"no-current-selected-key":"${dflt}"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"flag"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        if (!$current) $current = ec.resource.expandNoL10n("${dflt}");
        const $text = $options.get($current);
        sri.write($pad($text != null ? $text : $current));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Stamp"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(sri.getFieldValueString({"name":"field","attrs":{"name":"stamp"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}, "", "yyyy-MM-dd HH:mm", "date-time")));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Day"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(sri.getFieldValueString({"name":"field","attrs":{"name":"day"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}, "", "MM/dd/yyyy", "date")));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("At"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(sri.getFieldValueString({"name":"field","attrs":{"name":"at"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}, "12:00", "HH:mm", "time")));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("From"), $lineChars * 0.2, true) + ": ");
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Amount"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(ec.l10n.formatCurrency(sri.getFieldValue({"name":"field","attrs":{"name":"amount"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}, ""), uom)));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Price"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(ec.l10n.formatCurrencyNoSymbol(ec.resource.expand("${price}"), uom)));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Note"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(ec.resource.expand("Note ${note}", "", {note: 'm'})));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Enum ID"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(sri.getFieldEntityValue({"name":"display-entity","attrs":{"entity-name":"moqui.basic.Enumeration","text":"${description}","default-text":"none"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"enumId"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Doc"), $lineChars * 0.2, true) + ": ");
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Pw"), $lineChars * 0.2, true) + ": ");
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Range"), $lineChars * 0.2, true) + ": ");
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Clear"), $lineChars * 0.2, true) + ": ");
    sri.write("\n");
    sri.write("\n");
    sri.write("\n");
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Sel"), $lineChars * 0.2, true) + ": ");
    {
        const $options = sri.getFieldOptions({"name":"drop-down","attrs":{},"children":[{"name":"entity-options","attrs":{"key":"${enumId}","text":"${description}"},"children":[{"name":"entity-find","attrs":{"entity-name":"moqui.basic.Enumeration"},"children":[{"name":"econdition","attrs":{"field-name":"enumTypeId","value":"T"}},{"name":"order-by","attrs":{"field-name":"description"}}]}]}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"sel"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        let $current = sri.getFieldValueString({"name":"drop-down","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"sel"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        if (!$current) $current = "";
        const $text = $options.get($current);
        sri.write($pad($text != null ? $text : $current));
    }
    sri.write("\n");
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("Lst"), $lineChars * 0.2, true) + ": ");
    {
        const $options = sri.getFieldOptions({"name":"drop-down","attrs":{},"children":[{"name":"list-options","attrs":{"list":"opts","key":"${k}","text":"${t}"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"lst"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        let $current = sri.getFieldValueString({"name":"drop-down","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"lst"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}});
        if (!$current) $current = "";
        const $text = $options.get($current);
        sri.write($pad($text != null ? $text : $current));
    }
    sri.write("\n");
    if (mode == 'a') {
        $cellWidth = $lineChars * 0.8;
        sri.write($pad(ec.l10n.localize("Mix"), $lineChars * 0.2, true) + ": ");
        sri.write($pad(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"conditional-field","attrs":{"condition":"mode == 'a'"},"parent":{"name":"field","attrs":{"name":"mix"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    } else if (mode == 'b') {
    } else {
        $cellWidth = $lineChars * 0.8;
        sri.write($pad(ec.l10n.localize("Mix"), $lineChars * 0.2, true) + ": ");
        sri.write($pad(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"mix"},"parent":{"name":"form-single","attrs":{"name":"AllFields","map":"fv"},"parent":{"name":"widgets","attrs":{}}}}}})));
    }
    sri.write("\n");
    sri.setInContext({"name":"set","attrs":{"field":"shown","value":"x${y}"}});
    $cellWidth = $lineChars * 0.8;
    sri.write($pad(ec.l10n.localize("With Set"), $lineChars * 0.2, true) + ": ");
    sri.write($pad(ec.resource.expand("${shown}")));
    sri.write("\n");
    sri.write("\n");
} finally {
    sri.popContext();
}
$cellWidth = 0;
sri.write("\n");
