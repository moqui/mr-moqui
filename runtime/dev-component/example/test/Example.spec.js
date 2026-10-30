// The Rust parts of example: a type="rust" service, the tool from JavaScript, and the tool from XML Actions.
// ./moq test example (a dev component, so its specs run on a dev instance). The crate is linked into the binaries with
// the dev tools only, so in another (./moq test --lite) these are skipped.

spec({
    requires: ["devtools"],
    setup() { ec.artifactExecution.disableAuthz() },
    cleanup() { ec.artifactExecution.enableAuthz() },
})

feature("a rust service runs through its registered location", {
    when() {
        return call("example.ExampleServices.echo#Message", { message: "ping" })
    },
    then(out) {
        out.message == "ping"
    },
})

feature("the tool's methods and properties from JavaScript", {
    when() {
        const tool = ec.factory.getTool("ExampleTool")
        return { greeting: tool.greeting, text: tool.greet("Ada"), other: ec.factory.getTool("ExampleTool", "Hi").greet("Ada") }
    },
    then({ greeting, text, other }) {
        greeting == "Hello"
        text == "Hello, Ada!"
        other == "Hi, Ada!"
    },
})

feature("a tool a method returns is a tool too, with state the factory keeps", {
    when() {
        const counter = ec.factory.getTool("ExampleTool").counter()
        const before = counter.value()
        return { before, after: counter.increment(5), again: ec.factory.getTool("ExampleTool").counter().value() }
    },
    then({ before, after, again }) {
        after == before + 5
        again == after
    },
})

feature("XML Actions keep the tool in a field and call it", {
    when() {
        return call("example.ExampleServices.greet#WithTool", { name: "Grace", greeting: "Welcome" })
    },
    then(out) {
        out.text == "Welcome, Grace!"
        out.count > 0
    },
})

feature("a closure service reads the tool factory's state", {
    when() {
        const counter = ec.getTool("ExampleTool").counter()
        const value = counter.increment(3)
        return { value, out: call("example.ExampleServices.get#CounterValue", {}) }
    },
    then({ value, out }) {
        out.count == value
    },
})

feature("the tool factory by name, and null for none", {
    when() {
        const factory = ec.factory.getToolFactory("ExampleTool")
        return { name: factory.name, text: factory.getInstance("Hey").greet("Lin"), none: ec.factory.getToolFactory("NoSuchTool") }
    },
    then({ name, text, none }) {
        name == "ExampleTool"
        text == "Hey, Lin!"
        none == null
    },
})

feature("an unknown tool is an error", {
    when() {
        try {
            ec.factory.getTool("NoSuchTool")
            return { message: "" }
        } catch (err) {
            return { message: String(err.message || err) }
        }
    },
    then({ message }) {
        message.includes("no tool factory NoSuchTool")
    },
})
