// Computed Quasar and record-header styles. Was css/WebrootVue.qvt.test.js.
spec({ server: false })

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
