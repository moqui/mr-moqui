if (sri.doBoundaryComments()) sri.write("<!-- BEGIN screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Rich] -->\n");
{
    const $info = sri.openFormList("Rich");
    $h.formList($info, {"name":"form-list","attrs":{"name":"Rich","list":"rowList","paginate":"true","show-csv-button":"true","show-all-button":"true","header-dialog":"true","saved-finds":"true","select-columns":"true","show-text-button":"true","show-pdf-button":"true","style":"wide","focus-field":"id"}}, [
        [{
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "case-insensitive",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        sri.pushContext();
        try {
            $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
        },
            "summary": () => { let $v = ""; sri.pushContext(); try {  $v += $h.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $v.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "title",
            "align": "center",
            "headerStyle": "head",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Title"),
            "find": () => {
        sri.pushContext();
        try {
            $h.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        },
            "dialogField": () => {
        {
            const $title = ec.l10n.localize("Title");
            const $open = $h.fieldOpen("title", $title, false, "head", false);
            sri.pushContext();
            try {
                $h.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
        },
            "summary": () => { let $v = ""; sri.pushContext(); try {  $v += $h.valueText({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $v.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("titled", "center", $m === 2);
                    sri.pushContext();
                    try {
                        $h.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"container-style":"titled"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
            "hide": () => (hideQty),
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideQty", "")) {
                    $h.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","hide":"hideQty"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $h.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                        $h.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "act",
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
                        $h.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"act"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}, ec.l10n.localize("Do"));
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
            $h.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        }
        }
    ],
    "hiddenHeaderCells": [
        () => {
        $h.hidden({"name":"hidden","attrs":{},"parent":{"name":"header-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        }
    ],
    "hiddenFirstCells": [
    ],
    "hiddenSecondCells": [
    ],
    "hiddenLastCells": [
    ],
    "allFields": [
        {"name": "id", "title": () => ec.l10n.localize("ID")},
        {"name": "title", "title": () => ec.l10n.localize("Title")},
        {"name": "qty", "title": () => ec.l10n.localize("Qty")},
        {"name": "status", "title": () => ec.l10n.localize("Status")},
        {"name": "link", "title": () => ec.l10n.localize("Open")},
        {"name": "act", "title": () => ec.l10n.localize("Do")}
    ],
    "summaryFields": [
        {
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "case-insensitive",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("ID"),
            "find": () => {
        sri.pushContext();
        try {
            $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                $h.textFind({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
        },
            "summary": () => { let $v = ""; sri.pushContext(); try {  $v += $h.valueText({"name":"text-find","attrs":{},"parent":{"name":"header-field","attrs":{"show-order-by":"case-insensitive"},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $v.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "title",
            "align": "center",
            "headerStyle": "head",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Title"),
            "find": () => {
        sri.pushContext();
        try {
            $h.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        } finally {
            sri.popContext();
        }
        },
            "dialogField": () => {
        {
            const $title = ec.l10n.localize("Title");
            const $open = $h.fieldOpen("title", $title, false, "head", false);
            sri.pushContext();
            try {
                $h.dropDown({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
            } finally {
                sri.popContext();
            }
            $h.fieldClose($open, false);
        }
        },
            "summary": () => { let $v = ""; sri.pushContext(); try {  $v += $h.valueText({"name":"drop-down","attrs":{},"children":[{"name":"option","attrs":{"key":"A"}}],"parent":{"name":"header-field","attrs":{"container-style":"head"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}); } finally { sri.popContext(); } return $v.trim(); },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("titled", "center", $m === 2);
                    sri.pushContext();
                    try {
                        $h.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{"container-style":"titled"},"parent":{"name":"field","attrs":{"name":"title","align":"center"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
            "hide": () => (hideQty),
            "title": () => ec.l10n.localize("Qty"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2 && !ec.resource.condition("hideQty", "")) {
                    $h.rowCellOpen("", "right", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{"format":"0.0"},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"qty","align":"right","hide":"hideQty"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $h.display({"name":"display","attrs":{"text":"Excluded"},"parent":{"name":"conditional-field","attrs":{"condition":"status == 'X'"},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                            $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"status"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
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
                        $h.link({"name":"link","attrs":{"url":"open","text":"Open ${id}","link-type":"anchor"},"parent":{"name":"default-field","attrs":{"title":"Open"},"parent":{"name":"field","attrs":{"name":"link"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}, true);
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "rowId",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Row ID"),
            "find": null,
            "dialogField": () => {
        $h.hidden({"name":"hidden","attrs":{},"parent":{"name":"header-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
        },
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.hidden({"name":"hidden","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"rowId"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}});
                }
            }
        },
        {
            "name": "act",
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
                        $h.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Do"},"parent":{"name":"field","attrs":{"name":"act"},"parent":{"name":"form-list","attrs":{"name":"Rich"}}}}}, ec.l10n.localize("Do"));
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Rich] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Multi] -->\n");
{
    const $info = sri.openFormList("Multi");
    $h.formList($info, {"name":"form-list","attrs":{"name":"Multi","list":"rowList","multi":"true","transition":"saveAll","paginate":"false"}}, [
        [{
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("ID"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "note",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Note"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }],
        [{
            "name": "all",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": true,
            "sub": false,
            "title": () => ec.l10n.localize("Save"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save"},"parent":{"name":"field","attrs":{"name":"all"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}}, ec.l10n.localize("Save"));
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
        {"name": "id", "title": () => ec.l10n.localize("ID")},
        {"name": "note", "title": () => ec.l10n.localize("Note")},
        {"name": "all", "title": () => ec.l10n.localize("Save")}
    ],
    "summaryFields": [
        {
            "name": "id",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("ID"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"id"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "note",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": false,
            "sub": false,
            "title": () => ec.l10n.localize("Note"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.textLine({"name":"text-line","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"note"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        },
        {
            "name": "all",
            "align": "",
            "headerStyle": "",
            "orderBy": "",
            "submit": true,
            "sub": false,
            "title": () => ec.l10n.localize("Save"),
            "find": null,
            "dialogField": () => {},
            "summary": null,
            "cell": ($m) => {
                if ($m !== 1) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.submit({"name":"submit","attrs":{},"parent":{"name":"default-field","attrs":{"title":"Save"},"parent":{"name":"field","attrs":{"name":"all"},"parent":{"name":"form-list","attrs":{"name":"Multi"}}}}}, ec.l10n.localize("Save"));
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Multi] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- BEGIN form-list[@name=Edge] -->\n");
{
    const $info = sri.openFormList("Edge");
    $h.formList($info, {"name":"form-list","attrs":{"name":"Edge","list":"rowList","skip-form":"true","skip-header":"true"}}, [
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
            "first": () => {
            sri.pushContext();
            try {
                $h.display({"name":"display","attrs":{"text":"first"},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            },
            "last": () => {
            sri.pushContext();
            try {
                $h.display({"name":"display","attrs":{"text":"last"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
                    } finally {
                        sri.popContext();
                    }
                    $h.rowCellClose($m === 2);
                }
            }
        }]
    ], {
    "tree": null,
    "hasFirst": true,
    "hasSecond": false,
    "hasLast": true,
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
        {"name": "a", "title": () => ec.l10n.localize("A")}
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
            "first": () => {
            sri.pushContext();
            try {
                $h.display({"name":"display","attrs":{"text":"first"},"parent":{"name":"first-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            },
            "last": () => {
            sri.pushContext();
            try {
                $h.display({"name":"display","attrs":{"text":"last"},"parent":{"name":"last-row-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
            } finally {
                sri.popContext();
            }
            },
            "cell": ($m) => {
                if ($m !== 2) {
                    $h.rowCellOpen("", "", $m === 2);
                    sri.pushContext();
                    try {
                        $h.display({"name":"display","attrs":{},"parent":{"name":"default-field","attrs":{},"parent":{"name":"field","attrs":{"name":"a"},"parent":{"name":"form-list","attrs":{"name":"Edge"}}}}});
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
if (sri.doBoundaryComments()) sri.write("<!-- END   form-list[@name=Edge] -->\n");
if (sri.doBoundaryComments()) sri.write("<!-- END   screen[@location=" + sri.getActiveScreenDef().location + "].widgets -->\n");
