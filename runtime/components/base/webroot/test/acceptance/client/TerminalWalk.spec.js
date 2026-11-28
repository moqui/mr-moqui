/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

// The browser terminal's walk (plans/archive/Terminal-Fancy.md §14): a shell on the server, floated, through another
// app and a reload, and back in a pane, the same shell all along.
spec({ setupSpec() { login("john.doe") }, timeout: 60 })

const shown = (selector) => `[...document.querySelectorAll('${selector} .xterm-rows')].map(rows => rows.textContent).join('\\n')`

feature("a shell on the server floats through another app and a reload, and comes back to a pane", {
    expect() {
        const page = browser.newPage({ user: "john.doe" })
        // the header's account photo is Commons' (mr-boss), which this server of the webroot alone does not have
        page.allowProblems(/userPhoto\?userId=/)
        page.goto("/qapps/Terminal").status == 200
        // a clean start: no layout and no floating windows of an earlier run
        const cleared = page.evaluate("localStorage.clear() || true")
        page.goto("/qapps/Terminal")
        page.waitFor("!!document.querySelector('.m-terminal-pane')", { timeout: 30 })
        page.locator("#TerminalNew").click()
        page.locator("#TerminalNew-host-shell").click()
        // The host shell chooses its own prompt, and may set its own title (OSC 0 or 2, as Ubuntu's bash prompt does), so
        // the pane is found by its session's kind and profile, not by the profile's title "Shell". Wait for its output
        // and for the New menu to release focus.
        page.waitFor(() => {
            const pane = document.querySelector(".m-terminal-pane-focused")
            const shell = Object.values(moqui.terminal.state.sessions)
                .find((info) => info.kind === "host" && info.profile === "shell" && info.state === "running")
            return !!shell && !!shell.title && !!pane?.querySelector(".m-terminal-pane-header")?.textContent.includes(shell.title)
                && !!pane.querySelector(".xterm-rows")?.textContent.trim() && !document.querySelector(".q-menu")
        }, { timeout: 30 })
        page.locator(".m-terminal-pane-focused .xterm-screen").click()
        page.keyboard.type("export WALK=walk-$((20+22)); echo $WALK\r")
        page.waitFor("(" + shown(".m-terminal-pane") + ").includes('walk-42')", { timeout: 20 })

        // float it, go to another app, and reload there: the window stays, with the same shell
        page.getByRole("button", { name: "Float" }).click()
        page.waitFor("(" + shown(".m-float") + ").includes('walk-42')", { timeout: 20 })
        page.goto("/qapps/tools")
        page.waitFor("(" + shown(".m-float") + ").includes('walk-42')", { timeout: 20 })
        page.goto("/qapps/tools")
        page.waitFor("(" + shown(".m-float") + ").includes('walk-42')", { timeout: 30 })

        // back in a pane of the Terminal: the variable is still set
        page.locator("[data-float-action=dock]").click()
        page.waitFor("location.pathname.endsWith('/Terminal') && (" + shown(".m-terminal-pane") + ").includes('walk-42')", { timeout: 30 })
        const typing = page.evaluate("[...document.querySelectorAll('.m-terminal-pane .xterm-helper-textarea')].pop().focus() || true")
        page.keyboard.type("echo again-$WALK\r")
        page.waitFor("(" + shown(".m-terminal-pane") + ").includes('again-walk-42')", { timeout: 20 })

        // the end: the session ends, and is gone from the list
        page.getByRole("button", { name: "End session" }).click()
        page.waitFor("!(" + shown(".m-terminal-pane") + ").includes('again-walk-42')", { timeout: 20 })
    },
})
