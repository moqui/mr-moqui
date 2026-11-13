/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The Tools › Load walk of plans/archive/LoadRunner.md §14 in the browser on the qvt shell (./moq accept tools --spec
// LoadRunner): a LOAD_RUNNER holder (john.doe, in ADMIN_ADV) opens Load scripts, starts ServiceCall from New run, sees
// the request count grow on the run's page, stops it, finds it in Runs as Stopped, makes a second run and compares the
// two from Runs' row selection; then deletes both from their pages. Each run's label is unique to this walk, and what
// the walk leaves (a failure part way) is stopped and deleted at its end.
spec({ timeout: 180 })

const SCRIPT = "component://tools/test/load/ServiceCall.load.js"

// stop and delete runs over the page's own client: what a failed walk left behind
function cleanUp(page, runIds) {
    page.evaluate(async (runIds) => {
        const post = (name, params) => moqui.request("/apps/tools/Load/" + name, { method: "POST", params }).catch(() => null)
        for (const runId of runIds) {
            await post("stopRun", { runId, now: "true" })
            for (let i = 0; i < 40; i++) {
                const status = await moqui.request("/apps/tools/Load/runStatus", { params: { runId } }).then((result) => result.json || {}, () => ({}))
                if (!status.running) break
                await new Promise((done) => setTimeout(done, 250))
            }
            await post("deleteRun", { runId })
        }
        return true
    }, runIds)
}

function loadPage() {
    const page = browser.newPage({ user: "john.doe" })
    page.allowProblems(/response: 404 Not Found .*\/apps\/Commons\/Settings\/Account\/userPhoto\?userId=EX_JOHN_DOE$/)
    return page
}

// the number on a run page's card, read in the page
const CARD = (title) => {
    const card = Array.from(document.querySelectorAll("#LoadRunCards .q-card")).find((found) => found.querySelector(".text-caption").textContent.trim() == title)
    return card ? Number(card.querySelector(".text-h6").textContent.replace(/[^0-9]/g, "")) : -1
}

// New run's form for ServiceCall with these scenario settings and label; Start; the run page's runId
function startRun(page, settings, label) {
    page.goto("/qapps/tools/Load/NewRun?scriptLocation=" + encodeURIComponent(SCRIPT)).status == 200
    page.waitFor(() => !!document.querySelector("#NewRunTitle"))
    for (const [key, value] of Object.entries(settings)) page.fill(`input[name="s_calls_${key}"]`, String(value))
    page.fill('input[name="label"]', label)
    page.getByRole("button", { name: "Start", exact: true }).click()
    page.waitForURL(/\/Load\/Run\?runId=/)
    page.waitFor(() => !!document.querySelector("#LoadRunHeader"))
    return decodeURIComponent(/[?&]runId=([^&]+)/.exec(page.url())[1])
}

// a run's page: More, Delete, and the confirmation
function deleteRun(page, runId) {
    page.goto("/qapps/tools/Load/Run?runId=" + encodeURIComponent(runId)).status == 200
    page.waitFor(() => !!document.querySelector("#LoadRunHeader"))
    page.getByRole("button", { name: "More" }).click()
    page.getByText("Delete", { exact: true }).click()
    page.locator(".q-dialog").getByRole("button", { name: "Delete", exact: true }).click()
    page.waitForURL(/\/Load\/Runs/)
}

feature("a holder starts ServiceCall, follows and stops it, finds it in Runs and compares it with a second run", {
    expect() {
        const page = loadPage()
        const stamp = String(Date.now())
        const labelA = "walk A " + stamp
        const labelB = "walk B " + stamp
        const started = []
        try {
            page.goto("/qapps/tools/Load/Scripts").status == 200
            page.waitFor(() => document.body.innerText.includes("Service call"))
            page.getByRole("link", { name: "Service call" }).click()
            page.waitFor(() => !!document.querySelector("#NewRunTitle"))
            page.text().includes("This server's services, called in process")

            // the first run: 2 users for up to a minute, followed until the count has grown, then stopped
            const first = startRun(page, { users: 2, rampUp: 0, duration: 60 }, labelA)
            started.push(first)
            page.locator("#LoadRunStatus").text().includes("Running")
            page.waitFor(() => !!document.querySelector("#LoadRunCards"))
            page.evaluate(`window.loadCard = ${CARD.toString()}; true`)
            page.waitFor(() => window.loadCard("Requests") > 0, { timeout: 20 })
            const seen = page.evaluate(() => window.loadCard("Requests"))
            // the page asks again every 2 seconds while the run runs
            page.waitFor((seen) => window.loadCard("Requests") > seen, { arg: seen, timeout: 20 })
            page.getByRole("button", { name: "Stop", exact: true }).click()
            page.waitFor(() => document.querySelector("#LoadRunStatus").textContent.includes("Stopped"), { timeout: 30 })
            page.getByRole("button", { name: "Run again" }).count() == 1

            // in Runs as Stopped
            page.goto("/qapps/tools/Load/Runs").status == 200
            page.waitFor((label) => document.body.innerText.includes(label), { arg: labelA })
            page.evaluate((label) => Array.from(document.querySelectorAll("tr, .tr")).find((row) => row.innerText.includes(label)).innerText, labelA).includes("Stopped")

            // a second run of 1 user and 2 iterations, which completes on its own
            const second = startRun(page, { users: 1, rampUp: 0, duration: "", iterations: 2 }, labelB)
            started.push(second)
            page.waitFor(() => document.querySelector("#LoadRunStatus").textContent.includes("Completed"), { timeout: 30 })
            first != second

            // both selected in Runs, and Compare from the list header's Row Actions menu
            page.goto("/qapps/tools/Load/Runs").status == 200
            page.waitFor((label) => document.body.innerText.includes(label), { arg: labelB })
            page.evaluate((labels) => {
                for (const label of labels) Array.from(document.querySelectorAll("tr, .tr")).find((row) => row.innerText.includes(label)).querySelector(".q-checkbox").click()
                return true
            }, [labelA, labelB])
            // the Row Actions button: the header's button with the more_vert icon
            page.evaluate(() => { Array.from(document.querySelectorAll("#RunList_table .thead .q-btn"))
                .find((button) => button.querySelector(".q-icon") && button.querySelector(".q-icon").textContent.trim() == "more_vert").setAttribute("data-walk", "row-actions"); return true })
            page.click('[data-walk="row-actions"]')
            page.getByRole("button", { name: "Compare (2 to 4 runs)" }).click()
            page.waitForURL(/\/Load\/Compare/)
            page.waitFor(() => document.body.innerText.includes("Compare load runs"))
            const compared = page.text()
            compared.includes(labelA) && compared.includes(labelB)
            compared.includes("Summary")
            compared.includes("org.moqui.impl.BasicServices.echo#Data")

            // the runs deleted from their pages
            deleteRun(page, first)
            deleteRun(page, second)
            !page.text().includes(labelA)
            started.length = 0
        } finally {
            if (started.length) cleanUp(page, started)
        }
    },
})
