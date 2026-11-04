// Layout metrics of moqui-html.css, read from a real page. Was css/moqui-html.test.js.
// The focus outline is a real element.focus() and the computed outline, not a synthetic focus event.
spec({ server: false, timeout: 60 })

function styled(page, body, width) {
    if (width) page.setViewport(width, 1000)
    page.setContent(`<!doctype html><html data-theme="light">${body}</html>`)
    page.addStyle("/css/moqui-html.css")
    return page.evaluate(() => window.measure())
}

function openStyled(body, width) {
    const page = browser.newPage({ viewport: { width: width || 1440, height: 1000 } })
    return { page, result: styled(page, body, width || 1440) }
}

feature("Java CSS metrics and untitled big-row layout are preserved", {
    expect() {
        const row = (id, width, title) => `<div style="width:${width}px"><div id="${id}" class="field big-row">
            ${title ? "<label>Title</label>" : ""}<div class="widget">
                <div class="big-item"><div class="field"><div class="widget"><input size="40"></div></div></div>
                <div class="big-item"><div class="field"><div class="widget"><button>Text Search</button></div></div></div>
            </div></div></div>`
        const { result } = openStyled(`
            <div class="field" id="text-field"><label>Text</label><div class="widget"><input id="text-control"></div></div>
            <div class="field" id="select-field"><label>Select</label><div class="widget"><select id="select-control"><option>One</option></select></div></div>
            <div class="field" id="static-field"><label>Static</label><div class="widget"><span class="display" id="static-control">Value</span></div></div>
            <div class="field"><label>Paragraph</label><div class="widget"><p id="plain-static-control">Value</p></div></div>
            <div class="field"><label>Inline</label><div class="widget"><span class="text-inline" id="inline-static-control">Value</span></div></div>
            <div class="field" id="link-field"><label>Link</label><div class="widget"><a id="link-control" href="#">Related value</a></div></div>
            <div class="field"><label>Message</label><div class="widget"><textarea id="textarea-control"></textarea></div></div>
            <div class="field"><label>Date</label><div class="widget"><input type="date" id="date-control"></div></div>
            <div class="field"><label>Check</label><div class="widget"><input type="checkbox" id="checkbox-control"></div></div>
            <div class="field" data-invalid><label>Invalid</label><div class="widget"><input id="invalid-control"></div></div>
            ${row("wide", 690)}${row("titled", 690, "Title")}${row("narrow", 250)}
            <table class="form-list"><thead><tr id="heading"><th><div class="title">ID <span class="order-by"><a href="#">▲</a><a class="active" href="#">▼</a></span></div><div class="title">Name</div></th></tr></thead>
            <tbody><tr id="product"><td id="cell"><div><a href="#">DEMO_1_1</a></div><div><a href="#">Demo Product One-One</a></div></td>
            <td><div><span class="display">Inventory</span></div><div><span class="display">Inventory - Finished Good</span></div></td></tr></tbody></table>
            <table class="form-list"><tbody><tr><td id="compact-cell"><div class="field" id="table-field"><label>Compact</label><div class="widget"><input id="table-control"><span class="display" id="table-display">Compact</span></div></div></td></tr></tbody></table>
            <button id="button">Find Options</button><button id="NewProductDialog-button" data-dialog="NewProductDialog">New Product</button><strong id="strong">Label</strong>
            <div id="invoice-status"><span class="shaded-area condensed"><ul class="float-plain condensed"><li><div><strong>Status</strong></div><div><strong>In-Process</strong></div></li><li><button>Finalize</button></li><li><button>Cancel</button></li></ul></span></div>
            <table class="form-list"><thead><tr class="nav-row"><th><nav><button id="find">Find Options</button><ul class="pagination"><li><span id="page">Page 1</span></li></ul><a id="csv" data-btn="default">CSV</a></nav></th></tr></thead></table>
            <section class="box" id="panel"><header id="panel-heading"><h5 id="panel-title">Payment Applications</h5><div class="toolbar" id="panel-toolbar"><button data-btn="primary" id="panel-action">New Payment</button></div></header><div class="body" id="panel-body">Applied Payments</div></section>
            <a data-btn="primary" id="small-action">Current PDF</a><a data-btn="plain" id="plain-action">Account</a><a data-btn="primary" data-size="lg" id="large-action">Large</a>
            <div id="grid" style="width:1440px;padding:0 15px"><div class="row">
                <div class="col" style="--lg:6" id="grid-left">
                    <div class="field"><label>Invoice Date</label><div class="widget"><input id="grid-normal"></div></div>
                    <div class="field-row"><div><div class="field"><label>Invoice ID</label><div class="widget"><input id="grid-pair"></div></div></div>
                        <div><div class="field"><label>Type</label><div class="widget"><select><option>Sale</option></select></div></div></div></div>
                </div><div class="col" style="--lg:6" id="grid-right"><section class="box" id="grid-panel"></section></div>
            </div></div>
            <nav class="tabs" id="mixed-tabs"><ul><li class="active"><a href="#" id="grid-tab">Invoice</a></li><li><a href="#" id="inactive-tab">Items</a></li><li class="disabled"><span>Orders</span></li></ul></nav>
            <div id="after-tabs">Following content</div>
            <nav class="tabs" id="plain-tabs"><ul><li><a href="#">Invoice</a></li><li><a href="#">Items</a></li></ul></nav>
            <nav class="tabs" id="wrapped-tabs" style="width:70px"><ul><li class="active"><a href="#">Invoice</a></li><li><a href="#">Items</a></li></ul></nav>
            <section class="box" id="table-panel"><div class="body-nopad">
                <div class="table-wrap"><table class="form-list" id="first-panel-table"><thead><tr><th>Payment Applications</th></tr></thead></table></div>
                <div class="table-wrap"><table class="form-list" id="last-panel-table"><thead><tr><th>GL Transactions</th></tr></thead></table></div>
            </div></section>
            <div class="table-wrap" id="overflow-wrapper" style="width:200px"><table class="form-list" style="min-width:400px"><thead><tr><th>Wide table</th></tr></thead></table></div>
            <table class="form-list" style="width:600px;table-layout:fixed">
                <thead><tr><th><div class="title" data-align="right" id="right-title">Total</div></th>
                    <th><div class="title" data-align="center" id="center-title">Count</div></th>
                    <th><div class="title" id="left-title">Account</div></th></tr></thead>
                <tbody><tr><td><div data-align="right" id="right-value"><span class="display currency">123.45</span></div></td>
                    <td><div data-align="center" id="center-value"><span class="display Integer">7</span></div></td>
                    <td><div id="left-value"><span class="display">Sales</span></div></td></tr></tbody>
            </table>
            <script>
            window.measure = () => {
                const measure = (id) => {
                    const row = document.getElementById(id);
                    const rect = (selector) => {
                        const box = row.querySelector(selector).getBoundingClientRect();
                        return { x: box.x, y: box.y, width: box.width };
                    };
                    return { width: row.getBoundingClientRect().width, widget: rect(":scope > .widget"), input: rect("input"), button: rect("button") };
                };
                const statusMetrics = () => {
                    const ul = document.querySelector("#invoice-status ul");
                    const items = [...ul.querySelectorAll("li")];
                    const rects = items.map((item) => item.getBoundingClientRect());
                    const ulStyle = getComputedStyle(ul);
                    const itemStyle = getComputedStyle(items[0]);
                    const shadeStyle = getComputedStyle(document.querySelector("#invoice-status span"));
                    return { listStyle: ulStyle.listStyleType, listDisplay: ulStyle.display, listMargin: ulStyle.margin,
                        listPadding: ulStyle.padding, itemPadding: itemStyle.padding, itemMargin: itemStyle.margin,
                        itemDisplay: itemStyle.display, itemFloat: itemStyle.cssFloat,
                        horizontal: rects.every((r, i) => i === 0 || (r.x > rects[i - 1].x && Math.abs(r.y - rects[0].y) < 1)),
                        spanDisplay: shadeStyle.display, spanWhitespace: shadeStyle.whiteSpace, spanPadding: shadeStyle.padding,
                        shadeBackground: shadeStyle.backgroundColor, shadeShadow: shadeStyle.boxShadow };
                };
                const textOffset = (id, alignment) => {
                    const element = document.getElementById(id);
                    const range = document.createRange();
                    range.selectNodeContents(element);
                    const text = range.getBoundingClientRect(), box = element.getBoundingClientRect();
                    const offset = alignment === "right" ? box.right - text.right : alignment === "center"
                        ? (text.left + text.right - box.left - box.right) / 2 : text.left - box.left;
                    return Math.round(offset * 1000) / 1000;
                };
                const controlMetrics = () => ["text-control", "select-control", "textarea-control", "date-control"].map((id) => {
                    const style = getComputedStyle(document.getElementById(id));
                    return { color: style.color, border: style.borderTopColor, shadow: style.boxShadow };
                });
                const lightControls = controlMetrics();
                const lightStatus = statusMetrics();
                document.documentElement.dataset.theme = "dark";
                const darkStatus = statusMetrics();
                const darkControls = controlMetrics();
                document.documentElement.dataset.theme = "light";
                document.getElementById("text-control").focus();
                const focusStyle = getComputedStyle(document.getElementById("text-control"));
                return {
                    invalidBorder: getComputedStyle(document.getElementById("invalid-control")).borderTopColor,
                    focusOutline: [focusStyle.outlineStyle, focusStyle.outlineWidth, focusStyle.outlineColor],
                    wide: measure("wide"), titled: measure("titled"), narrow: measure("narrow"),
                    formMetrics: {
                        textHeight: document.getElementById("text-control").getBoundingClientRect().height,
                        selectHeight: document.getElementById("select-control").getBoundingClientRect().height,
                        staticHeight: document.getElementById("static-control").getBoundingClientRect().height,
                        plainStaticHeight: document.getElementById("plain-static-control").getBoundingClientRect().height,
                        inlineStaticHeight: document.getElementById("inline-static-control").getBoundingClientRect().height,
                        linkHeight: document.getElementById("link-control").getBoundingClientRect().height,
                        fieldGap: getComputedStyle(document.getElementById("text-field")).marginBottom,
                        labelPadding: getComputedStyle(document.querySelector("#text-field > label")).paddingTop,
                        tableControlPadding: getComputedStyle(document.getElementById("table-control")).padding,
                        tableControlHeight: document.getElementById("table-control").getBoundingClientRect().height,
                        tableDisplayHeight: document.getElementById("table-display").getBoundingClientRect().height,
                        tableFieldGap: getComputedStyle(document.getElementById("table-field")).marginBottom,
                        bigRowGap: getComputedStyle(document.querySelector("#wide")).marginBottom,
                        bigItemGap: getComputedStyle(document.querySelector("#wide .big-item .field")).marginBottom
                    },
                    lightStatus, darkStatus, lightControls, darkControls,
                    checkboxShadow: getComputedStyle(document.getElementById("checkbox-control")).boxShadow,
                    alignmentMetrics: {
                        rightTitle: textOffset("right-title", "right"), rightValue: textOffset("right-value", "right"),
                        centerTitle: textOffset("center-title", "center"), centerValue: textOffset("center-value", "center"),
                        leftTitle: textOffset("left-title", "left"), leftValue: textOffset("left-value", "left")
                    },
                    tableMetrics: {
                        betweenTables: document.getElementById("last-panel-table").getBoundingClientRect().top - document.getElementById("first-panel-table").getBoundingClientRect().bottom,
                        belowLastTable: document.getElementById("table-panel").getBoundingClientRect().bottom - document.getElementById("last-panel-table").getBoundingClientRect().bottom,
                        overflowWidth: document.getElementById("overflow-wrapper").clientWidth,
                        scrollWidth: document.getElementById("overflow-wrapper").scrollWidth,
                        overflowX: getComputedStyle(document.getElementById("overflow-wrapper")).overflowX
                    },
                    tabMetrics: {
                        mixedHeight: document.querySelector("#mixed-tabs ul").getBoundingClientRect().height,
                        plainHeight: document.querySelector("#plain-tabs ul").getBoundingClientRect().height,
                        wrappedHeight: document.querySelector("#wrapped-tabs ul").getBoundingClientRect().height,
                        activeBottomGap: document.querySelector("#mixed-tabs ul").getBoundingClientRect().bottom - document.getElementById("grid-tab").getBoundingClientRect().bottom,
                        inactiveBottomGap: document.querySelector("#mixed-tabs ul").getBoundingClientRect().bottom - document.getElementById("inactive-tab").getBoundingClientRect().bottom,
                        followingGap: document.getElementById("after-tabs").getBoundingClientRect().top - document.querySelector("#mixed-tabs ul").getBoundingClientRect().bottom
                    },
                    gridMetrics: {
                        columnWidth: document.getElementById("grid-left").getBoundingClientRect().width,
                        columnPadding: getComputedStyle(document.getElementById("grid-left")).padding,
                        normalInput: document.getElementById("grid-normal").getBoundingClientRect().left - document.getElementById("grid").getBoundingClientRect().left,
                        pairInput: document.getElementById("grid-pair").getBoundingClientRect().left - document.getElementById("grid").getBoundingClientRect().left,
                        rightPanel: document.getElementById("grid-panel").getBoundingClientRect().left - document.getElementById("grid").getBoundingClientRect().left,
                        tabHeight: document.getElementById("grid-tab").getBoundingClientRect().height,
                        tabPadding: getComputedStyle(document.getElementById("grid-tab")).padding
                    },
                    widgetMetrics: {
                        actionHeight: document.getElementById("small-action").getBoundingClientRect().height,
                        actionFont: getComputedStyle(document.getElementById("small-action")).fontSize,
                        plainFont: getComputedStyle(document.getElementById("plain-action")).fontSize,
                        largeFont: getComputedStyle(document.getElementById("large-action")).fontSize,
                        headingPadding: getComputedStyle(document.getElementById("panel-heading")).padding,
                        headingFont: getComputedStyle(document.getElementById("panel-title")).fontSize,
                        headingWeight: getComputedStyle(document.getElementById("panel-title")).fontWeight,
                        headingBorder: getComputedStyle(document.getElementById("panel-heading")).borderBottomColor,
                        bodyPadding: getComputedStyle(document.getElementById("panel-body")).padding,
                        toolbarTop: document.getElementById("panel-toolbar").getBoundingClientRect().top - document.getElementById("panel-heading").getBoundingClientRect().top,
                        toolbarHeightIndependent: document.getElementById("panel-heading").getBoundingClientRect().height < document.getElementById("panel-toolbar").getBoundingClientRect().height + 20
                    },
                    javaMetrics: {
                        font: getComputedStyle(document.body).fontFamily, fontSize: getComputedStyle(document.body).fontSize,
                        lineHeight: getComputedStyle(document.body).lineHeight, background: getComputedStyle(document.body).backgroundColor,
                        color: getComputedStyle(document.body).color, cellPadding: getComputedStyle(document.getElementById("cell")).padding,
                        cellAlign: getComputedStyle(document.getElementById("cell")).verticalAlign,
                        rowHeight: document.getElementById("product").getBoundingClientRect().height,
                        headerHeight: document.getElementById("heading").getBoundingClientRect().height,
                        sortColor: getComputedStyle(document.querySelector(".order-by a.active")).color,
                        dialogFont: getComputedStyle(document.getElementById("NewProductDialog-button")).fontSize,
                        dialogPadding: getComputedStyle(document.getElementById("NewProductDialog-button")).padding,
                        buttonPadding: getComputedStyle(document.getElementById("button")).padding,
                        strongWeight: getComputedStyle(document.getElementById("strong")).fontWeight,
                        stripe: getComputedStyle(document.getElementById("product")).backgroundColor,
                        navMargin: getComputedStyle(document.getElementById("find")).margin,
                        pagePadding: getComputedStyle(document.getElementById("page")).padding,
                        paginationDisplay: getComputedStyle(document.querySelector(".pagination")).display,
                        pageFloat: getComputedStyle(document.getElementById("page")).cssFloat,
                        csvBeforePagination: document.getElementById("csv").getBoundingClientRect().x < document.getElementById("page").getBoundingClientRect().x
                    }
                };
            };
            </script>`)
        const { wide, titled, narrow, javaMetrics, formMetrics, lightStatus, darkStatus, widgetMetrics, gridMetrics, tableMetrics, alignmentMetrics, lightControls, darkControls, checkboxShadow, invalidBorder, focusOutline, tabMetrics } = result
        assert.deepEqual(lightControls, Array(4).fill({ color: "rgb(85, 85, 85)", border: "rgb(204, 204, 204)", shadow: "rgba(0, 0, 0, 0.075) 0px 1px 1px 0px inset" }))
        assert.deepEqual(darkControls, Array(4).fill({ color: "rgb(221, 227, 234)", border: "rgb(52, 61, 72)", shadow: "none" }))
        assert.equal(checkboxShadow, "none")
        assert.equal(invalidBorder, "rgb(217, 83, 79)")
        assert.deepEqual(focusOutline, ["solid", "2px", "rgb(51, 122, 183)"])
        assert.deepEqual(tabMetrics, { mixedHeight: 42, plainHeight: 42, wrappedHeight: 83, activeBottomGap: 0, inactiveBottomGap: 0, followingGap: 8 })
        for (const [field, offset] of Object.entries(alignmentMetrics)) assert.ok(Math.abs(offset) < 0.02, field + " alignment offset: " + offset)
        assert.deepEqual(tableMetrics, { betweenTables: 8, belowLastTable: 9, overflowWidth: 200, scrollWidth: 400, overflowX: "auto" })
        assert.deepEqual(gridMetrics, { columnWidth: 720, columnPadding: "0px 15px", normalInput: 135, pairInput: 135, rightPanel: 735, tabHeight: 42, tabPadding: "10px 6px" })
        assert.deepEqual(widgetMetrics, {
            actionHeight: 30, actionFont: "12px", plainFont: "14px", largeFont: "15.4px",
            headingPadding: "10px 15px", headingFont: "16px", headingWeight: "500",
            headingBorder: "rgba(0, 0, 0, 0)", bodyPadding: "8px", toolbarTop: 0, toolbarHeightIndependent: true,
        })
        assert.deepEqual(javaMetrics, {
            font: '"Helvetica Neue", Helvetica, Arial, sans-serif', fontSize: "14px", lineHeight: "20px",
            background: "rgb(255, 255, 255)", color: "rgb(34, 34, 34)", cellPadding: "5px", cellAlign: "middle",
            rowHeight: 51, headerHeight: 51, sortColor: "rgb(255, 0, 0)", dialogFont: "12px", dialogPadding: "5px 10px",
            buttonPadding: "6px 12px", strongWeight: "700", stripe: "rgba(0, 0, 0, 0)",
            navMargin: "6px 8px 0px 0px", pagePadding: "6px 12px",
            paginationDisplay: "inline-block", pageFloat: "left", csvBeforePagination: true,
        })
        assert.equal(wide.widget.width, wide.width)
        assert.ok(wide.input.width > 300, "size=40 input is not squeezed into the unused label column")
        assert.ok(Math.abs(wide.input.y - wide.button.y) < 10, "input and button share a line")
        assert.ok(titled.widget.x > wide.widget.x + 50, "titled rows still reserve label space")
        assert.ok(titled.widget.width < titled.width)
        assert.equal(narrow.widget.width, narrow.width)
        assert.ok(narrow.input.width <= narrow.width, "inputs stay constrained at narrow widths")
        assert.ok(narrow.button.y > narrow.input.y + 20, "controls may wrap when space is genuinely limited")
        assert.deepEqual(formMetrics, {
            textHeight: 34, selectHeight: 34, staticHeight: 34, plainStaticHeight: 34,
            inlineStaticHeight: 34, linkHeight: 34,
            fieldGap: "15px", labelPadding: "7px", tableControlPadding: "4px 8px", tableControlHeight: 30,
            tableDisplayHeight: 20, tableFieldGap: "0px", bigRowGap: "8px", bigItemGap: "0px",
        })
        for (const status of [lightStatus, darkStatus]) {
            assert.equal(status.listStyle, "none")
            assert.equal(status.listDisplay, "inline-block")
            assert.equal(status.listMargin, "0px")
            assert.equal(status.listPadding, "0px")
            assert.equal(status.itemPadding, "4px")
            assert.equal(status.itemMargin, "0px 4px 0px 0px")
            assert.equal(status.itemDisplay, "block")
            assert.equal(status.itemFloat, "left")
            assert.equal(status.horizontal, true)
            assert.equal(status.spanDisplay, "inline-block")
            assert.equal(status.spanWhitespace, "normal")
            assert.equal(status.spanPadding, "4px")
        }
        assert.equal(lightStatus.shadeBackground, "rgb(248, 248, 248)")
        assert.equal(lightStatus.shadeShadow, "rgb(248, 248, 248) 0px 0px 0px 1px inset, rgb(204, 204, 204) 0px 0px 0px 1px")
        assert.match(lightStatus.shadeShadow, /rgb\(204, 204, 204\)/)
        assert.equal(darkStatus.shadeBackground, "rgb(63, 63, 63)")
        assert.equal(darkStatus.shadeShadow, "rgb(8, 8, 8) 0px 0px 0px 1px inset, rgb(51, 51, 51) 0px 0px 0px 1px")
        assert.match(darkStatus.shadeShadow, /rgb\(51, 51, 51\)/)
    },
})

feature("dashboard screen body keeps the page shell while dashboard widgets use a grid", {
    expect() {
        const result = openStyled(`<body class="dev dashboard"><div id="top"><nav class="navbar">Navigation</nav></div><div id="content"><div class="inner">
            <div class="dashboard" style="width:650px"><div class="dashboard-item">One</div><div class="dashboard-item">Two</div><div class="dashboard-item" id="empty"></div></div>
            </div></div><div id="footer">Footer</div>
            <script>window.measure = () => { const s = getComputedStyle(document.body), g = getComputedStyle(document.querySelector("div.dashboard"));
                return { display: s.display, direction: s.flexDirection, margin: s.margin,
                    topWidth: document.getElementById("top").getBoundingClientRect().width, contentWidth: document.getElementById("content").getBoundingClientRect().width, footerWidth: document.getElementById("footer").getBoundingClientRect().width,
                    grid: g.display, columns: g.gridTemplateColumns.split(" ").length, gap: g.gap, empty: getComputedStyle(document.getElementById("empty")).display }; };</script>`, 1440).result
        assert.deepEqual(result, { display: "flex", direction: "column", margin: "0px", topWidth: 1440, contentWidth: 1440, footerWidth: 1440, grid: "grid", columns: 3, gap: "12px", empty: "none" })
    },
})

feature("float-box cards sit in a row, a narrow card is capped and the active card is outlined", {
    expect() {
        const { list, card, active } = openStyled(`<div style="width:690px"><ul class="float-box" id="cards"><li class="box-narrow" id="card"><strong>Demo Bank Account</strong><address>Demo Bank (Checking)<br>Ziziwork Retail</address><button>Update</button></li><li class="active" id="active-card">Selected</li></ul></div>
            <script>window.measure = () => { const style = (id) => { const s = getComputedStyle(document.getElementById(id)); return { display: s.display, list: s.listStyleType, padding: s.padding, margin: s.margin, shadow: s.boxShadow, max: s.maxWidth, top: document.getElementById(id).getBoundingClientRect().top }; };
                return { list: style("cards"), card: style("card"), active: style("active-card") }; };</script>`).result
        assert.equal(list.display, "inline-block")
        assert.equal(list.list, "none")
        assert.equal(list.padding, "0px")
        assert.equal(card.padding, "8px")
        assert.equal(card.margin, "0px 8px 8px 0px")
        assert.equal(card.max, "300px")
        assert.equal(card.top, active.top)
        assert.notEqual(card.shadow, "none")
        assert.notEqual(active.shadow, card.shadow)
    },
})

feature("a diagram-sequence process map has its lanes, arrows and narrow-screen visibility at desktop and mobile widths", {
    expect() {
        const lanes = ["Customers", "Internal", "Suppliers"].map((name, i) => `<div class="col" style="--sm:4" id="lane-${i}"><p class="visible-xs-block" id="mobile-${i}">&nbsp;</p><div class="diagram-sequence-actor" id="actor-${i}">${name}</div><div class="diagram-sequence-item" id="item-${i}">Order<i class="fa left" id="left-${i}"></i><i class="fa right" id="right-${i}"></i></div><div class="diagram-sequence-item smaller" id="small-${i}">Return</div><p class="hidden-xs" id="desktop-${i}">&nbsp;</p>${i === 1 ? "<p>Extra process step</p>" : ""}</div>`).join("")
        const body = `<div class="row diagram-sequence" id="diagram">${lanes}</div>
            <script>window.measure = () => { const metric = (id) => { const e = document.getElementById(id), s = getComputedStyle(e), r = e.getBoundingClientRect(); return { x: r.x, y: r.y, width: r.width, height: r.height, font: s.fontSize, weight: s.fontWeight, display: s.display, margin: s.margin, borderLeft: s.borderLeft, borderRight: s.borderRight, borderBottom: s.borderBottom, position: s.position, left: s.left, right: s.right, bottom: s.bottom, before: getComputedStyle(e, "::before").content }; };
                return { diagram: metric("diagram"), lanes: [0, 1, 2].map((i) => metric("lane-" + i)), actor: metric("actor-0"), item: metric("item-0"), small: metric("small-0"), left: metric("left-0"), right: metric("right-0"), mobile: metric("mobile-0"), desktop: metric("desktop-0") }; };</script>`
        const page = browser.newPage({ viewport: { width: 1440, height: 1000 } })
        for (const width of [1440, 600]) {
            const m = styled(page, body, width)
            assert.equal(m.diagram.width, Math.min(width, 900))
            assert.equal(m.diagram.x, (width - Math.min(width, 900)) / 2)
            assert.ok(m.lanes.every((lane) => Math.abs(lane.width - (width >= 768 ? 300 : 600)) < 0.1), JSON.stringify(m.lanes))
            if (width >= 768) {
                assert.equal(m.lanes[0].y, m.lanes[1].y)
                assert.ok(m.lanes[1].height > m.lanes[0].height)
            } else {
                assert.ok(m.lanes[1].y > m.lanes[0].y)
            }
            assert.equal(m.lanes[0].borderLeft, "1px dashed rgb(127, 127, 127)")
            assert.equal(m.lanes[2].borderRight, "1px dashed rgb(127, 127, 127)")
            assert.equal(m.actor.weight, "700")
            assert.equal(m.item.position, "relative")
            assert.equal(m.item.borderBottom, "2px solid rgb(127, 127, 127)")
            assert.equal(m.left.position, "absolute")
            assert.equal(m.left.left, "-2px")
            assert.equal(m.left.bottom, "-8px")
            assert.equal(m.left.before, '""')
            assert.equal(m.right.right, "-2px")
            assert.equal(m.right.before, '""')
            assert.equal(m.mobile.display, width >= 768 ? "none" : "block")
            assert.equal(m.desktop.display, width >= 768 ? "block" : "none")
        }
    },
})

feature("a remote single select takes the field width, an empty one stays as tall, and it has a caret, focus and invalid states", {
    expect() {
        const wrapper = (id, text, hidden) => `<div class="ts-wrapper single ${hidden ? "input-hidden " : ""}${text ? "has-items" : ""}" id="${id}"><div class="ts-control">${text ? `<div class="item">${text}</div>` : ""}<input type="text" size="1"${hidden ? ' style="position:absolute;left:-10000px;opacity:0"' : ""}></div></div>`
        const page = browser.newPage({ viewport: { width: 1440, height: 1000 } })
        page.setContent(`<!doctype html><html data-theme="light"><div style="width:840px;padding:0 15px"><div class="field"><label>Owner</label><div class="widget">${wrapper("owner", "ZIRET: Ziziwork Retail & Wholesale", true)}</div></div><div class="field"><label>Parent</label><div class="widget">${wrapper("empty", "", true)}</div></div><div class="field" data-invalid><label>Invalid</label><div class="widget">${wrapper("invalid", "Value", true)}</div></div></div>
            <script>window.measure = () => { const metric = (id) => { const e = document.querySelector("#" + id + " .ts-control"), s = getComputedStyle(e), r = e.getBoundingClientRect(); return { width: r.width, widget: (w => w.getBoundingClientRect().width - parseFloat(getComputedStyle(w).paddingLeft) - parseFloat(getComputedStyle(w).paddingRight))(e.closest(".widget")), height: r.height, border: s.borderTopColor, arrow: getComputedStyle(e, "::after").borderTopWidth, outline: s.outlineStyle }; };
                const light = { owner: metric("owner"), empty: metric("empty"), invalid: metric("invalid") };
                const owner = document.getElementById("owner"); owner.classList.add("focus", "dropdown-active"); const active = metric("owner");
                active.arrowUp = getComputedStyle(owner.querySelector(".ts-control"), "::after").borderBottomWidth;
                return { light, active }; };</script></html>`)
        page.addStyle("/css/moqui-html.css")
        page.addStyle("/libs/tom-select/tom-select.css")
        const { light, active } = page.evaluate(() => window.measure())
        for (const metric of [light.owner, light.empty, light.invalid]) {
            assert.ok(Math.abs(metric.width - metric.widget) < 0.05, JSON.stringify(metric))
            assert.equal(metric.arrow, "4px")
            assert.equal(metric.outline, "none")
        }
        assert.ok(light.empty.height >= 30)
        assert.equal(light.empty.height, light.owner.height)
        assert.equal(light.invalid.border, "rgb(217, 83, 79)")
        assert.notEqual(light.owner.border, light.invalid.border)
        assert.equal(active.outline, "solid")
        assert.equal(active.arrow, "0px")
        assert.equal(active.arrowUp, "4px")
    },
})

feature("a row editor gives up control width to keep its actions in view, while a table too wide for its panel still scrolls", {
    expect() {
        const options = "<option></option>" + ["AF - Afghanistan", "GS - South Georgia and the South Sandwich Islands", "UM - United States Minor Outlying Islands"].map((text) => `<option>${text}</option>`).join("")
        const row = `<tr><td><div><a href="#">_NA_: Not Applicable</a></div><div><a href="#">Local gateway for order shipping rate</a></div></td>
            <td><div><select class="country">${options}</select></div><div><input type="text" size="25"></div></td>
            <td><div><input type="text" size="10"></div><div><select><option></option><option>Third Party</option></select></div></td>
            <td><div><input type="text" size="6"></div></td><td><div><button type="submit" class="update">Update</button></div></td></tr>`
        const table = (id, width, body) => `<div style="width:${width}px"><div class="table-wrap" id="${id}"><table class="form-list"><tbody>${body}</tbody></table></div></div>`
        const wideRow = `<tr>${Array.from({ length: 14 }, (_, i) => `<td>Column ${i} with some text</td>`).join("")}<td><button class="update">Update</button></td></tr>`
        const { narrow, roomy, wide, natural } = openStyled(`${table("narrow", 672, row)}${table("roomy", 1392, row)}${table("wide", 672, wideRow)}
            <div><select id="natural">${options}</select></div>
            <script>window.measure = () => { const m = (id) => { const w = document.getElementById(id), r = w.getBoundingClientRect(), b = w.querySelector(".update").getBoundingClientRect(), s = w.querySelector("select");
                return { overflow: getComputedStyle(w).overflowX, scrolls: w.scrollWidth > w.clientWidth, actionInView: b.right <= r.right, select: s ? s.getBoundingClientRect().width : 0 }; };
                return { narrow: m("narrow"), roomy: m("roomy"), wide: m("wide"), natural: document.getElementById("natural").getBoundingClientRect().width }; };</script>`).result
        assert.deepEqual([narrow.scrolls, narrow.actionInView], [false, true])
        assert.ok(narrow.select < natural && narrow.select >= 56, JSON.stringify(narrow))
        assert.equal(roomy.select, natural)
        assert.deepEqual([wide.overflow, wide.scrolls, wide.actionInView], ["auto", true, false])
    },
})

feature("nested columns use their own breakpoints without inheriting parent widths", {
    expect() {
        const body = `<div style="width:min(900px,100%);padding:0 15px"><div class="row"><div class="col" style="--lg:7"><div class="row" id="nested"><div class="col" style="--sm:7">Date</div><div class="col" style="--sm:5">Available</div></div></div></div></div>
            <script>window.measure = () => { const e = document.getElementById("nested"), r = e.getBoundingClientRect(); return { width: r.width, cells: [...e.children].map((c) => { const b = c.getBoundingClientRect(); return { x: b.x - r.x, y: b.y - r.y, width: b.width }; }) }; };</script>`
        const page = browser.newPage({ viewport: { width: 1440, height: 1000 } })
        const wide = styled(page, body, 1440)
        assert.equal(wide.cells[0].y, wide.cells[1].y)
        assert.ok(Math.abs(wide.cells[0].width - wide.width * 7 / 12) < 0.02)
        assert.ok(Math.abs(wide.cells[1].width - wide.width * 5 / 12) < 0.02)
        const narrow = styled(page, body, 600)
        assert.ok(narrow.cells.every((cell) => cell.width === narrow.width && cell.x === 0))
        assert.ok(narrow.cells[1].y > narrow.cells[0].y)
    },
})

// From WebrootVueCss.spec.js, merged here to share a spec run.
// Computed Quasar and record-header styles. Was css/WebrootVue.qvt.test.js.

const themeHtml = `<!doctype html><html><body>
    <a id="link" href="#">Link</a>
    <span id="success" class="text-success">Success</span>
    <span id="danger" class="text-danger">Danger</span>
    <span id="sort" class="form-order-by"><a id="active" class="active" href="#">▲</a> <a id="descending" href="#">▼</a></span>
    <div class="q-menu"><a id="menu" href="#">Menu</a></div>
    <div class="q-toolbar"><a id="toolbar" href="#">Toolbar</a></div>
    <p id="paragraph" class="text-inline">Promotion note</p>
    <span id="inline" class="text-inline">Inline label</span>
    <button id="dense" class="q-btn q-btn--dense"><span class="q-btn__content">Find Options</span></button>
    <a id="flat" class="q-btn q-btn--dense q-btn--flat m-link"><span class="q-btn__content">Product</span></a>
    <button id="round" class="q-btn q-btn--dense q-btn--round">Icon</button>
    <span id="muted" class="m-muted">Muted caption</span>
    <div id="chip" class="q-chip m-tonal">Approved</div>
    <div id="container" class="q-table__container q-table__card" style="width:600px">
        <div class="table q-table"><div class="thead"><div class="tr">
            <div id="selHead" class="th m-sel">x</div><div class="th">Name</div><div class="th">Description</div>
        </div></div></div>
    </div>
    </body></html>`

function header(id) {
    return `<div id="${id}" class="m-record-header">
        <div class="row"><div class="col-12 col-md m-record-title"><h5 id="${id}Title">Call the supplier</h5></div>
            <div class="col-12 col-md-auto m-record-status"><div id="${id}Bar" class="m-status-bar row items-center">
                <div id="${id}Stages" class="m-status-stages row items-center"><span>Open</span><span>Done</span></div>
                <div class="m-status-actions row items-center"><button>Complete</button></div></div></div></div>
        <div id="${id}Foot" class="row m-record-foot"><div class="col-12 col-md">Due</div>
            <div id="${id}Actions" class="col-12 col-md-auto row items-center m-record-actions"><button>Reassign</button></div></div></div>`
}

function themePage() {
    const page = browser.newPage({ viewport: { width: 1280, height: 800 } })
    page.setContent(themeHtml)
    page.addStyle("/libs/quasar/quasar.prod.css")
    page.addStyle({ content: ":root { --q-primary:rgb(1,2,3); --q-positive:rgb(4,5,6); --q-negative:rgb(7,8,9); }" })
    page.addStyle("/css/WebrootVue.qvt.css")
    page.addStyle({ content: ".q-menu, .q-toolbar { color:rgb(10,11,12); }" })
    return page.evaluate(() => {
        const style = (id) => getComputedStyle(document.getElementById(id))
        return {
            link: style("link").color, success: style("success").color, danger: style("danger").color,
            active: style("active").color,
            menu: style("menu").color, toolbar: style("toolbar").color,
            paragraph: style("paragraph").display, inline: style("inline").display,
            densePadding: style("dense").padding, flatPadding: style("flat").padding,
            flatMinHeight: style("flat").minHeight, roundPadding: style("round").padding,
            sort: style("sort").display, sortLink: style("active").display,
            sortSameLine: document.getElementById("active").getBoundingClientRect().top ===
                document.getElementById("descending").getBoundingClientRect().top,
            muted: style("muted").color, chipColor: style("chip").color,
            chipTinted: style("chip").backgroundColor !== "rgb(224, 224, 224)",
            containerOverflow: style("container").overflowX,
            selNarrow: document.getElementById("selHead").getBoundingClientRect().width < 60,
        }
    })
}

function recordHeader(width) {
    const page = browser.newPage({ viewport: { width, height: 800 } })
    page.setContent(`<!doctype html><html><body><div id="card" class="q-card">Card</div>${header("light")}${header("dark")}</body></html>`)
    page.addStyle("/libs/quasar/quasar.prod.css")
    page.addStyle("/css/WebrootVue.qvt.css")
    return page.evaluate(() => {
        const style = (id) => getComputedStyle(document.getElementById(id))
        const light = {
            page: getComputedStyle(document.body).backgroundColor, card: style("card").backgroundColor,
            header: style("light").backgroundColor, headerBorder: style("light").borderTopColor,
            titleMargin: style("lightTitle").marginTop,
            titleLineHeight: style("lightTitle").lineHeight, footBorder: style("lightFoot").borderTopStyle,
            barDirection: style("lightBar").flexDirection, stagesJustify: style("lightStages").justifyContent,
            actionsJustify: style("lightActions").justifyContent,
        }
        // dark mode is Quasar's class on the body, where the theme's variables are set
        document.body.classList.add("body--dark")
        return Object.assign(light, { darkHeaderBorder: style("dark").borderTopColor })
    })
}

feature("Quasar theme colors, labels and sort layout match Java", {
    expect() {
        assert.deepEqual(themePage(), {
            link: "rgb(1, 2, 3)", success: "rgb(4, 5, 6)", danger: "rgb(7, 8, 9)",
            active: "rgb(1, 2, 3)",
            menu: "rgb(10, 11, 12)", toolbar: "rgb(10, 11, 12)",
            paragraph: "block", inline: "inline",
            densePadding: "4px 8px", flatPadding: "0px 4px", flatMinHeight: "21px", roundPadding: "0px",
            sort: "inline", sortLink: "inline", sortSameLine: true,
            muted: "rgb(97, 97, 97)", chipColor: "rgb(1, 2, 3)", chipTinted: true, containerOverflow: "auto", selNarrow: true,
        })
    },
})

feature("a light page under white cards, and the record header with its status-bar at the right from 1024px", {
    expect() {
        const common = {
            page: "rgb(244, 246, 248)", card: "rgb(255, 255, 255)", header: "rgb(255, 255, 255)",
            headerBorder: "rgba(0, 0, 0, 0.12)", darkHeaderBorder: "rgba(255, 255, 255, 0.28)",
            titleMargin: "0px", titleLineHeight: "24px", footBorder: "solid",
        }
        const wide = recordHeader(1280)
        const narrow = recordHeader(600)
        for (const [name, value] of Object.entries(common)) {
            assert.equal(wide[name], value, name)
            assert.equal(narrow[name], value, name)
        }
        assert.equal(wide.barDirection, "column")
        assert.equal(wide.stagesJustify, "flex-end")
        assert.equal(wide.actionsJustify, "flex-end")
        assert.equal(narrow.barDirection, "row")
        assert.equal(narrow.stagesJustify, "normal")
        assert.equal(narrow.actionsJustify, "normal")
    },
})
