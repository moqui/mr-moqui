// An acceptance spec: moqui test --acceptance runs it, and a regression run never reads test/acceptance/.
spec({ database: false })

feature("an acceptance spec runs only with --acceptance", {
    expect() {
        1 + 1 == 2
    },
})
