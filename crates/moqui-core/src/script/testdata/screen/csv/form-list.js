{
    const $info = sri.openFormList("Rows");
    $hasPrev = false;
    $sep();
    sri.write($csv(ec.l10n.localize("ID")));
    $sep();
    sri.write($csv(ec.l10n.localize(ec.resource.expand("The Title"))));
    $sep();
    sri.write($csv(ec.l10n.localize("Qty")));
    if (!(hideAmt)) {
        $sep();
        sri.write($csv(ec.l10n.localize("Amt")));
    }
    $sep();
    sri.write($csv(ec.l10n.localize("Status")));
    $sep();
    sri.write($csv(ec.l10n.localize(ec.resource.expand("Open"))));
    sri.write("\n");
    for (let $index = 0; $index < $info.count; $index++) {
        sri.startFormListRow($info, $index);
        try {
            $hasPrev = false;
            $sep();
            sri.write($csv(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            $sep();
            sri.write($csv(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            $sep();
            sri.write($csv(sri.getFieldValueString({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            if (!(hideAmt)) {
                $sep();
                sri.write($csv(ec.l10n.formatCurrency(sri.getFieldValue({"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}, ""), 'USD')));
            }
            $sep();
            if (status == 'X') {
                sri.write($csv(ec.resource.expand("Excluded")));
            } else {
                sri.write($csv(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            }
            $sep();
            {
                let $linkText = ec.resource.expand("Open ${id}");
                if ($linkText === "null") $linkText = "";
                sri.write($csv($linkText));
            }
            sri.write("\n");
        } finally {
            sri.endFormListRow();
        }
    }
    sri.safeCloseList($info);
}
{
    const $info = sri.openFormList("Cols");
    $hasPrev = false;
    $sep();
    sri.write($csv(ec.l10n.localize("A")));
    $sep();
    sri.write($csv(ec.l10n.localize("B")));
    $sep();
    sri.write($csv(ec.l10n.localize("C")));
    sri.write("\n");
    for (let $index = 0; $index < $info.count; $index++) {
        sri.startFormListRow($info, $index);
        try {
            $hasPrev = false;
            $sep();
            sri.write($csv(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            $sep();
            sri.write($csv(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            $sep();
            sri.write($csv(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            sri.write("\n");
        } finally {
            sri.endFormListRow();
        }
    }
    sri.safeCloseList($info);
}
