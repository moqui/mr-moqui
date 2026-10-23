if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Rich] -->\n");
{
    const $info = sri.openFormList("Rich");
    $v.formList($info, {"name":"form-list","attrs":{"name":"Rich","list":"rowList","paginate":"true","show-csv-button":"true","show-all-button":"true","header-dialog":"true","saved-finds":"true","select-columns":"true","show-text-button":"true","show-pdf-button":"true","style":"wide","focus-field":"id"}}, [
        [{
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "case-insensitive",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        $v.rowCellOpen("", "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "title",
            "align": "center",
            "headerStyle": "head",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Title"),
            "find": () => {
        $v.rowCellOpen("", "center", false);
        sri.pushContext();
        try {
            $v.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "head", false);
        sri.pushContext();
        try {
            $v.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("titled", "center", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"container-style":"titled"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
            "hide": () => (hideQty),
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideQty", "")) {
                    $v.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","hide":"hideQty"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $v.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                        $v.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "act",
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
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"act"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
        "r": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "title", "field": {"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"container-style":"titled"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "qty", "field": {"name":"field","attrs":{"name":"qty","align":"right","hide":"hideQty"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","hide":"hideQty"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "status", "field": {"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": "status == 'X'", "sets": [], "widgets": [{"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]},{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "link", "field": {"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "rowId", "field": {"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "act", "field": {"name":"field","attrs":{"name":"act"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"act"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]}],
        "h": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "title", "field": {"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]},
{"name": "rowId", "field": {"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"hidden","attrs":{},"parent":{"name":"header-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}]}]}],
        "first": [],
        "second": [],
        "last": []
    },
    "headerFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID"),
                                    "orderBy": "case-insensitive", "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        }},
        {"name": "title", "title": () => ec.l10n.localize("Title"),
                                    "orderBy": "", "dialogField": () => {
        $v.fieldBox(null, "head", false);
        sri.pushContext();
        try {
            $v.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        }},
        {"name": "rowId", "title": () => ec.l10n.localize("Row ID"),
                                    "orderBy": "", "dialogField": () => {
        $v.hidden({"name":"hidden","attrs":{},"parent":{"name":"header-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        }}
    ],
    "allFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID"), "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $t.trim(); }},
        {"name": "title", "title": () => ec.l10n.localize("Title"), "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $t.trim(); }},
        {"name": "qty", "title": () => ec.l10n.localize("Qty"), "summary": null},
        {"name": "status", "title": () => ec.l10n.localize("Status"), "summary": null},
        {"name": "link", "title": () => ec.l10n.localize("Open"), "summary": null},
        {"name": "act", "title": () => ec.l10n.localize("Do"), "summary": null}
    ],
    "summaryFields": [
        {
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "case-insensitive",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        $v.rowCellOpen("", "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "title",
            "align": "center",
            "headerStyle": "head",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Title"),
            "find": () => {
        $v.rowCellOpen("", "center", false);
        sri.pushContext();
        try {
            $v.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "head", false);
        sri.pushContext();
        try {
            $v.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("titled", "center", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"container-style":"titled"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
            "hide": () => (hideQty),
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideQty", "")) {
                    $v.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","hide":"hideQty"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $v.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                        $v.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "rowId",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Row ID"),
            "find": null,
            "dialogField": () => {
        $v.hidden({"name":"hidden","attrs":{},"parent":{"name":"header-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        },
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
                }
            }
        },
        {
            "name": "act",
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
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"act"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Rich] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Multi] -->\n");
{
    const $info = sri.openFormList("Multi");
    $v.formList($info, {"name":"form-list","attrs":{"name":"Multi","list":"rowList","multi":"true","transition":"saveAll","paginate":"false"}}, [
        [{
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("ID"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "note",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Note"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "all",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": true,
            "title": () => ec.l10n.localize("Save"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save"},"parent":{"name":"field","attrs":{"name":"all"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
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
        "r": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}}]}]},
{"name": "note", "field": {"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}}]}]},
{"name": "all", "field": {"name":"field","attrs":{"name":"all"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save"},"parent":{"name":"field","attrs":{"name":"all"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}}]}]}],
        "h": [],
        "first": [],
        "second": [],
        "last": []
    },
    "headerFields": [
    ],
    "allFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID"), "summary": null},
        {"name": "note", "title": () => ec.l10n.localize("Note"), "summary": null},
        {"name": "all", "title": () => ec.l10n.localize("Save"), "summary": null}
    ],
    "summaryFields": [
        {
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("ID"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "note",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Note"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "all",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": true,
            "title": () => ec.l10n.localize("Save"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save"},"parent":{"name":"field","attrs":{"name":"all"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Multi] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Edge] -->\n");
{
    const $info = sri.openFormList("Edge");
    $v.formList($info, {"name":"form-list","attrs":{"name":"Edge","list":"rowList","skip-form":"true","skip-header":"true"}}, [
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
            "first": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.display({"name":"display","attrs":{"text":"first"},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            $v.rowCellClose(false);
            },
            "last": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.display({"name":"display","attrs":{"text":"last"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            $v.rowCellClose(false);
            },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }]
    ], {
    "views": null,
    "hasFirst": true,
    "hasSecond": false,
    "hasLast": true,
    "headerForm": false,
    "rowSelection": null,
    "valueDescs": {
        "r": [{"name": "a", "field": {"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}}]}]}],
        "h": [],
        "first": [{"name": "a", "field": {"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{"text":"first"},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}}]}]}],
        "second": [],
        "last": [{"name": "a", "field": {"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{"text":"last"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}}]}]}]
    },
    "headerFields": [
    ],
    "allFields": [
        {"name": "a", "title": () => ec.l10n.localize("A"), "summary": null}
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
            "first": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.display({"name":"display","attrs":{"text":"first"},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            $v.rowCellClose(false);
            },
            "last": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.display({"name":"display","attrs":{"text":"last"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            $v.rowCellClose(false);
            },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Edge] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
