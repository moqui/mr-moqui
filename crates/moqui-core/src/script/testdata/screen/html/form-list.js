if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Rows] -->\n");
{
    const $info = sri.openFormList("Rows");
    $h.formList($info, {"name":"form-list","attrs":{"name":"Rows","list":"rowList"}}, [
        [{
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "true",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        sri.pushContext();
        try {
            $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        } finally {
            sri.popContext();
        }
        },
            "dialogField": () => {
        {
            const $title = ec.l10n.localize("ID");
            const $open = $h.fieldOpen("id", $title, false, "", false);
            sri.pushContext();
            try {
                $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
        },
            "summary": () => { let $v = ""; sri.pushContext(); try {  $v += $h.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}); } finally { sri.popContext(); } return $v.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "title",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("The Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "qty",
            "align": "right",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "amt",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "hide": () => (hideAmt),
            "title": () => ec.l10n.localize("Amt"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideAmt", "")) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{"currency-unit-field":"'USD'"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "status",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Status"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if (status == 'X') {
                    if ($m !== 2) {
                        $h.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $h.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $h.rowCellClose($m === 2);
                    }
                } else {
                    if ($m !== 2) {
                        $h.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $h.rowCellClose($m === 2);
                    }
                }
            }
        }],
        [{
            "name": "link",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Open"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": true,
            "sub": false,
            "title": () => ec.l10n.localize("Do"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}, ec.l10n.localize("Do"));
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }]
    ], {
    "tree": null,
    "hasFirst": false,
    "hasSecond": false,
    "hasLast": false,
    "headerForm": true,
    "hiddenCells": [
        () => {
        const $m = 0;
        if ($m !== 2) {
            $h.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"h"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        }
        }
    ],
    "hiddenHeaderCells": [
    ],
    "hiddenFirstCells": [
    ],
    "hiddenSecondCells": [
    ],
    "hiddenLastCells": [
    ],
    "allFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID")},
        {"name": "title", "title": () => ec.l10n.localize("The Title")},
        {"name": "qty", "title": () => ec.l10n.localize("Qty")},
        {"name": "amt", "title": () => ec.l10n.localize("Amt")},
        {"name": "status", "title": () => ec.l10n.localize("Status")},
        {"name": "link", "title": () => ec.l10n.localize("Open")},
        {"name": "b", "title": () => ec.l10n.localize("Do")}
    ],
    "summaryFields": [
        {
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "true",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        sri.pushContext();
        try {
            $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
        } finally {
            sri.popContext();
        }
        },
            "dialogField": () => {
        {
            const $title = ec.l10n.localize("ID");
            const $open = $h.fieldOpen("id", $title, false, "", false);
            sri.pushContext();
            try {
                $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
        },
            "summary": () => { let $v = ""; sri.pushContext(); try {  $v += $h.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"true"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}); } finally { sri.popContext(); } return $v.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "title",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("The Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"The Title"},"parent":{"name":"field","attrs":{"name":"title","print-width":"30","print-width-type":"characters"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "qty",
            "align": "right",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","print-width":"20"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "amt",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "hide": () => (hideAmt),
            "title": () => ec.l10n.localize("Amt"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideAmt", "")) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{"currency-unit-field":"'USD'"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"amt","hide":"hideAmt"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "status",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Status"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if (status == 'X') {
                    if ($m !== 2) {
                        $h.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $h.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $h.rowCellClose($m === 2);
                    }
                } else {
                    if ($m !== 2) {
                        $h.rowCellOpen("", "", $m === 2);
                        sri.pushContext();
                        try {
                            $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                        } finally {
                            sri.popContext();
                        }
                        $h.rowCellClose($m === 2);
                    }
                }
            }
        },
        {
            "name": "link",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Open"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "h",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("H"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"h"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}});
                }
            }
        },
        {
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": true,
            "sub": false,
            "title": () => ec.l10n.localize("Do"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Rows"}}}}}, ec.l10n.localize("Do"));
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
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
    $h.formList($info, {"name":"form-list","attrs":{"name":"Cols","list":"rowList"}}, [
        [{
            "name": "a",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("A"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("B"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "c",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("C"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }]
    ], {
    "tree": null,
    "hasFirst": false,
    "hasSecond": false,
    "hasLast": false,
    "headerForm": false,
    "hiddenCells": [
    ],
    "hiddenHeaderCells": [
    ],
    "hiddenFirstCells": [
    ],
    "hiddenSecondCells": [
    ],
    "hiddenLastCells": [
    ],
    "allFields": [
        {"name": "a", "title": () => ec.l10n.localize("A")},
        {"name": "b", "title": () => ec.l10n.localize("B")},
        {"name": "c", "title": () => ec.l10n.localize("C")}
    ],
    "summaryFields": [
        {
            "name": "a",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("A"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "b",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("B"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"b"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "c",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("C"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"c"},"parent":{"name":"form-list","attrs":{"name":"Cols"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }
    ]
});
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Cols] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
