if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Selectable] -->\n");
{
    const $info = sri.openFormList("Selectable");
    $v.formList($info, {"name":"form-list","attrs":{"name":"Selectable","list":"rowList","transition":"save","select-columns":"true","saved-finds":"true","show-text-button":"true","show-pdf-button":"true","show-page-size":"true"}}, [
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
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
            "title": () => ec.l10n.localize("Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
            "first": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
            } finally {
                sri.popContext();
            }
            $v.rowCellClose(false);
            },
            "last": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.display({"name":"display","attrs":{"text":"end"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
    "headerForm": true,
    "rowSelection": {"idField": "id", "parameter": "ids", "listMode": "", "actions": [{"dialog": {"name":"dialog","attrs":{"button-text":"Update All","title":"Update Selected"}}, "render": () => {
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-single[@name=UpdateSelected] -->\n");
sri.pushSingleFormMapContext(fieldValues);
try {
    {
        const $form = $v.formOpen({"name":"form-single","attrs":{"name":"UpdateSelected","transition":"save"}}, [{"name": "status", "field": {"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"UpdateSelected"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Status"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"UpdateSelected"}}}}}]}]},
{"name": "go", "field": {"name":"field","attrs":{"name":"go"},"parent":{"name":"form-single","attrs":{"name":"UpdateSelected"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-single","attrs":{"name":"UpdateSelected"}}}}}]}]}]);
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Status"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-single","attrs":{"name":"UpdateSelected"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-single","attrs":{"name":"UpdateSelected"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.formClose({"name":"form-single","attrs":{"name":"UpdateSelected","transition":"save"}}, $form);
    }
} finally {
    sri.popContext();
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-single[@name=UpdateSelected] -->\n");
}}]},
    "valueDescs": {
        "r": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}]}]},
{"name": "title", "field": {"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}]}]},
{"name": "note", "field": {"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}]}]}],
        "h": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}]}]}],
        "first": [{"name": "note", "field": {"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}]}]}],
        "second": [],
        "last": [{"name": "note", "field": {"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{"text":"end"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}]}]}]
    },
    "headerFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID"),
                                    "orderBy": "case-insensitive", "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        }}
    ],
    "allFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID"), "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}); } finally { sri.popContext(); } return $t.trim(); }},
        {"name": "title", "title": () => ec.l10n.localize("Title"), "summary": null},
        {"name": "note", "title": () => ec.l10n.localize("Note"), "summary": null}
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
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
        } finally {
            sri.popContext();
        }
        $v.rowCellClose(false);
        },
            "dialogField": () => {
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        },
            "summary": () => {  let $t = ""; sri.pushContext(); try {  $t += $v.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}}); } finally { sri.popContext(); } return $t.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
            "title": () => ec.l10n.localize("Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
            "first": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
            } finally {
                sri.popContext();
            }
            $v.rowCellClose(false);
            },
            "last": () => {
            $v.rowCellOpen("", "", false);
            sri.pushContext();
            try {
                $v.display({"name":"display","attrs":{"text":"end"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
                        $v.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Selectable"}}}}});
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Selectable] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Everyone] -->\n");
{
    const $info = sri.openFormList("Everyone");
    $v.formList($info, {"name":"form-list","attrs":{"name":"Everyone","list":"rowList","multi":"true","transition":"saveAll","paginate":"false"}}, [
        [{
            "name": "title",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": false,
            "title": () => ec.l10n.localize("Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "go",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": true,
            "title": () => ec.l10n.localize("Save All"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save All"},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}});
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
        "r": [{"name": "id", "field": {"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}}]}]},
{"name": "title", "field": {"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}}]}]},
{"name": "go", "field": {"name":"field","attrs":{"name":"go"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save All"},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}}]}]}],
        "h": [],
        "first": [],
        "second": [],
        "last": []
    },
    "headerFields": [
    ],
    "allFields": [
        {"name": "title", "title": () => ec.l10n.localize("Title"), "summary": null},
        {"name": "go", "title": () => ec.l10n.localize("Save All"), "summary": null}
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
                    $v.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}});
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
            "title": () => ec.l10n.localize("Title"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"title"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $v.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "go",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "sub": false,
            "submit": true,
            "title": () => ec.l10n.localize("Save All"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $v.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $v.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save All"},"parent":{"name":"field","attrs":{"name":"go"},"parent":{"name":"form-list","attrs":{"name":"Everyone"}}}}});
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Everyone] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-single[@name=Choices] -->\n");
sri.pushSingleFormMapContext(fv);
try {
    {
        const $form = $v.formOpen({"name":"form-single","attrs":{"name":"Choices","map":"fv","transition":"save"}}, [{"name": "pick", "field": {"name":"field","attrs":{"name":"pick"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"drop-down","attrs":{"allow-multiple":"true","show-not":"true","submit-on-select":"true"},"children":[{"name":"option","attrs":{"key":"A"}},{"name":"option","attrs":{"key":"B"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"pick"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}}]}]},
{"name": "dep", "field": {"name":"field","attrs":{"name":"dep"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{"default-transition":"treeItems"},"children":[{"name":"depends-on","attrs":{"field":"pick"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"dep"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}}]}]},
{"name": "who", "field": {"name":"field","attrs":{"name":"who"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display-entity","attrs":{"entity-name":"moqui.basic.Enumeration","text":"${description}","default-text":"nobody"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"who"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}}]}]},
{"name": "shown", "field": {"name":"field","attrs":{"name":"shown"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"shown"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}},{"name":"visible-when","attrs":{"field":"pick","value":"B,C"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"shown"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}}]}]},
{"name": "watch", "field": {"name":"field","attrs":{"name":"watch"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"display","attrs":{"dynamic-transition":"treeItems"},"children":[{"name":"depends-on","attrs":{"field":"pick","parameter":"p"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"watch"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}}]}]},
{"name": "between", "field": {"name":"field","attrs":{"name":"between"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"date-find","attrs":{"type":"time"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"between"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}}]}]},
{"name": "note", "field": {"name":"field","attrs":{"name":"note"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}, "subs": [{"cond": null, "sets": [], "widgets": [{"name":"text-area","attrs":{"editor-type":"md","rows":"5"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}}]}]}]);
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.dropDown({"name":"drop-down","attrs":{"allow-multiple":"true","show-not":"true","submit-on-select":"true"},"children":[{"name":"option","attrs":{"key":"A"}},{"name":"option","attrs":{"key":"B"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"pick"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textLine({"name":"text-line","attrs":{"default-transition":"treeItems"},"children":[{"name":"depends-on","attrs":{"field":"pick"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"dep"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.displayEntity({"name":"display-entity","attrs":{"entity-name":"moqui.basic.Enumeration","text":"${description}","default-text":"nobody"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"who"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.fieldBox({"name":"visible-when","attrs":{"field":"pick","value":"B,C"}}, "", false);
        sri.pushContext();
        try {
            $v.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"shown"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.display({"name":"display","attrs":{"dynamic-transition":"treeItems"},"children":[{"name":"depends-on","attrs":{"field":"pick","parameter":"p"}}],"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"watch"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.dateFind({"name":"date-find","attrs":{"type":"time"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"between"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.fieldBox(null, "", false);
        sri.pushContext();
        try {
            $v.textArea({"name":"text-area","attrs":{"editor-type":"md","rows":"5"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-single","attrs":{"name":"Choices","map":"fv"}}}}});
        } finally {
            sri.popContext();
        }
        sri.write("</div>");
        $v.formClose({"name":"form-single","attrs":{"name":"Choices","map":"fv","transition":"save"}}, $form);
    }
} finally {
    sri.popContext();
}
if (sri.doBoundaryComments()) sri.write("<!-- END   form-single[@name=Choices] -->\n");
$v.sectionPaginate("entries");
sri.renderSection("Rows");
{
    const $type = "default" || "default";
    sri.write("<m-container-box" + $v.A({id: null, type: $type, title: ec.resource.expand("Closed", ""),
        ":initial-open": "" === "closed" ? "false" : null}) + ">");
}
sri.write("<template v-slot:header>");
sri.write("</template>");
sri.write("<m-box-body>");
$v.label({"name":"label","attrs":{"text":"inside"}});
sri.write("</m-box-body>");
sri.write("</m-container-box>\n");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
