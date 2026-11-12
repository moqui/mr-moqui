sri.write("\n");
{
    // the rows come one at a time, each made as it is written (docs/ui/Screen-Output.md)
    const $info = sri.openFormList("Rows", true);
    sri.write("    <Rows>\n");
    for (let $index = 0; sri.startFormListRow($info, $index); $index++) {
        sri.write("        <RowsEntry");
        try {
            sri.write(" id=\"");
            sri.write($attr(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id","ai-opt-out":"true"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            sri.write('"');
            sri.write(" title=\"");
            sri.write($attr(sri.getFieldValueString({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","ai-opt-out":"true","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            sri.write('"');
            sri.write(" qty=\"");
            sri.write($attr(sri.getFieldValueString({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            sri.write('"');
            if (!(hideAmt)) {
                sri.write(" amt=\"");
                sri.write($attr(ec.l10n.formatCurrency(sri.getFieldValue({"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}, ""), 'USD', 2)));
                sri.write('"');
            }
            if (status == 'X') {
                sri.write(" status=\"");
                sri.write($attr(ec.resource.expand("Excluded")));
                sri.write('"');
            } else {
                sri.write(" status=\"");
                sri.write($attr(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
                sri.write('"');
            }
            sri.write(" link=\"");
            sri.write(ec.resource.expand("Open ${id}"));
            sri.write('"');
            sri.write("/>\n");
        } finally {
            sri.endFormListRow();
        }
    }
    sri.safeCloseList($info);
    sri.write("    </Rows>\n");
}
{
    // the rows come one at a time, each made as it is written (docs/ui/Screen-Output.md)
    const $info = sri.openFormList("Cols", true);
    sri.write("    <Cols>\n");
    for (let $index = 0; sri.startFormListRow($info, $index); $index++) {
        sri.write("        <ColsEntry");
        try {
            sri.write(" a=\"");
            sri.write($attr(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            sri.write('"');
            sri.write(" b=\"");
            sri.write($attr(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            sri.write('"');
            sri.write(" c=\"");
            sri.write($attr(sri.getFieldValueString({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols","list":"rowList"},"parent":{"name":"widgets","attrs":{}}}}}})));
            sri.write('"');
            sri.write("/>\n");
        } finally {
            sri.endFormListRow();
        }
    }
    sri.safeCloseList($info);
    sri.write("    </Cols>\n");
}
sri.write("\n");
