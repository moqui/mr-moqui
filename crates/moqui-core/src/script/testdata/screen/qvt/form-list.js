if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Rows] -->\n");
{
    const $info = sri.openFormList("Rows");
    $v.formList($info, {"name":"form-list","attrs":{"name":"Rows","list":"rowList"}}, [
        [{
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "true",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        $v.rowCellOpen("", "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "title",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("The Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "qty",
            "align": "right",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "amt",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "hide": () => (hideAmt),
            "title": () => ec.l10n.localize("Amt"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideAmt", "")) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{"currency-unit-field":"'USD'"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "status",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Status"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if (status == 'X') {
                    if ($m !== 2) {
                        $v.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $v.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $v.rowCellClose($m === 2);
                    }
                } else {
                    if ($m !== 2) {
                        $v.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $v.rowCellClose($m === 2);
                    }
                }
            }
        }],
        [{
            "name": "link",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Open"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": true,
            "title": () => ec.l10n.localize("Do"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }]
    ], {
    "views": null,
    "hasFirst": false,
    "hasSecond": false,
    "hasLast": false,
    "headerForm": true,
    "rowSelection": null,
    "valueDescs": {
        "r": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]},
{"name": "title", "field": {"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]},
{"name": "qty", "field": {"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]},
{"name": "amt", "field": {"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{"currency-unit-field":"'USD'"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]},
{"name": "status", "field": {"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": "status == 'X'", "sets": [], "widgets": [{"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]},{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]},
{"name": "link", "field": {"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]},
{"name": "h", "field": {"name":"field","attrs":{"name":"h"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"h"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]},
{"name": "b", "field": {"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]}],
        "h": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}]}]}],
        "first": [],
        "second": [],
        "last": []
    },
    "headerFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID"),
                                    "orderBy": "true", "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        }}
    ],
    "allFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID"), "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}); } finally { sri.popContext(); } return $t.trim(); }},
        {"name": "title", "title": () => ec.l10n.localize("The Title"), "summary": null},
        {"name": "qty", "title": () => ec.l10n.localize("Qty"), "summary": null},
        {"name": "amt", "title": () => ec.l10n.localize("Amt"), "summary": null},
        {"name": "status", "title": () => ec.l10n.localize("Status"), "summary": null},
        {"name": "link", "title": () => ec.l10n.localize("Open"), "summary": null},
        {"name": "b", "title": () => ec.l10n.localize("Do"), "summary": null}
    ],
    "summaryFields": [
        {
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "true",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        $v.rowCellOpen("", "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "title",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("The Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "qty",
            "align": "right",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "amt",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "hide": () => (hideAmt),
            "title": () => ec.l10n.localize("Amt"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideAmt", "")) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{"currency-unit-field":"'USD'"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "status",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Status"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if (status == 'X') {
                    if ($m !== 2) {
                        $v.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $v.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $v.rowCellClose($m === 2);
                    }
                } else {
                    if ($m !== 2) {
                        $v.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $v.rowCellClose($m === 2);
                    }
                }
            }
        },
        {
            "name": "link",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Open"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "h",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("H"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"h"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                }
            }
        },
        {
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": true,
            "title": () => ec.l10n.localize("Do"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }
    ]
});
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Rows] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Cols] -->\n");
{
    const $info = sri.openFormList("Cols");
    $v.formList($info, {"name":"form-list","attrs":{"name":"Cols","list":"rowList"}}, [
        [{
            "name": "a",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("A"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("B"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "c",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("C"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }]
    ], {
    "views": null,
    "hasFirst": false,
    "hasSecond": false,
    "hasLast": false,
    "headerForm": false,
    "rowSelection": null,
    "valueDescs": {
        "r": [{"name": "a", "field": {"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}}]}]},
{"name": "b", "field": {"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}}]}]},
{"name": "c", "field": {"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}}]}]}],
        "h": [],
        "first": [],
        "second": [],
        "last": []
    },
    "headerFields": [
    ],
    "allFields": [
        {"name": "a", "title": () => ec.l10n.localize("A"), "summary": null},
        {"name": "b", "title": () => ec.l10n.localize("B"), "summary": null},
        {"name": "c", "title": () => ec.l10n.localize("C"), "summary": null}
    ],
    "summaryFields": [
        {
            "name": "a",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("A"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("B"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "c",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("C"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }
    ]
});
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Cols] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
