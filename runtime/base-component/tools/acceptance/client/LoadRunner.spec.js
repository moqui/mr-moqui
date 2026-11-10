/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The Tools › Load walk of plans/LoadRunner.md §14 in the browser on the qvt shell (./moq accept tools --spec
// LoadRunner): a LOAD_RUNNER holder (john.doe, in ADMIN_ADV) opens Load scripts, starts ServiceCall from New run, sees
// the request count grow on the run's page, stops it, finds it in Runs as Stopped, makes a second run and compares the
// two from Runs' row selection; then deletes both from their pages.
spec({ timeout: 180 })

const SCRIPT = "component://tools/test/load/ServiceCall.load.js"

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
        page.goto("/qapps/tools/Load/Scripts").status == 200
        page.waitFor(() => document.body.innerText.includes("Service call"))
        page.getByRole("link", { name: "Service call" }).click()
        page.waitFor(() => !!document.querySelector("#NewRunTitle"))
        page.text().includes("This server's services, called in process")

        // the first run: 2 users for up to a minute, followed until the count has grown, then stopped
        const first = startRun(page, { users: 2, rampUp: 0, duration: 60 }, "walk A")
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
        page.waitFor(() => document.body.innerText.includes("walk A"))
        page.evaluate(() => Array.from(document.querySelectorAll("tr, .tr")).find((row) => row.innerText.includes("walk A")).innerText).includes("Stopped")

        // a second run of 1 user and 2 iterations, which completes on its own
        const second = startRun(page, { users: 1, rampUp: 0, iterations: 2 }, "walk B")
        page.waitFor(() => document.querySelector("#LoadRunStatus").textContent.includes("Completed"), { timeout: 30 })
        first != second

        // both selected in Runs, and Compare
        page.goto("/qapps/tools/Load/Runs").status == 200
        page.waitFor(() => document.body.innerText.includes("walk B"))
        page.evaluate(() => {
            for (const label of ["walk A", "walk B"]) Array.from(document.querySelectorAll("tr, .tr")).find((row) => row.innerText.includes(label)).querySelector(".q-checkbox").click()
            return true
        })
        // the selection's actions are in the list header's Row Actions menu
        page.locator("#RunList_table .thead .q-btn").click()
        page.getByRole("button", { name: "Compare (2 to 4 runs)" }).click()
        page.waitForURL(/\/Load\/Compare/)
        page.waitFor(() => document.body.innerText.includes("Compare load runs"))
        const compared = page.text()
        compared.includes("walk A") && compared.includes("walk B")
        compared.includes("Summary")
        compared.includes("org.moqui.impl.BasicServices.echo#Data")

        // the runs deleted from their pages
        deleteRun(page, first)
        deleteRun(page, second)
        !page.text().includes("walk A")
    },
})
