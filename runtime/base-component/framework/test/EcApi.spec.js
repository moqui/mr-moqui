// The members of Moqui Java's ec API as a ported script calls them (docs/dev/JS-Core-API.md): the context stack, the
// method and property forms of a getter, the message lists. The Rust tests of each member are in
// crates/moqui-core/src/script/runtime/java_api_tests.rs; this spec is the same idioms through a real server context.
spec({});

// A spec reads a context name through the stack: bare names are a script's, not a spec's. The calls are in `when`,
// as every statement of a `then` is a condition.
let seen = {};
feature("ec.context is the context stack, with Java's Map and frame methods", {
    when() {
        ec.context.put("ecApiName", "Ada");
        seen.put = context.ecApiName;
        seen.has = ec.context.containsKey("ecApiName") && ec.context.keySet().includes("ecApiName");
        ec.context.push({ ecApiPushed: 7 });
        context.ecApiName = "above";
        seen.pushed = ec.context.get("ecApiPushed") === 7 && ec.context.get("ecApiName") === "above";
        seen.popped = ec.context.pop();
        seen.after = ec.context.get("ecApiName") === "Ada" && !ec.context.containsKey("ecApiPushed");
        seen.removed = ec.context.remove("ecApiName");
    },
    then() {
        ec.context === context && ec.getContext() === context;
        seen.put === "Ada" && seen.has && seen.pushed && seen.after && seen.removed === "Ada";
        seen.popped.ecApiPushed === 7 && seen.popped.ecApiName === "above";
        typeof ec.context.size() === "number" && typeof ec.context.getSharedMap() === "object";
    }
});

feature("a getter has its method form and a get method its property form", {
    expect() {
        ec.getUser() === ec.user && ec.getMessage() === ec.message && ec.getEntity() === ec.entity;
        ec.user.getUserId() === ec.user.userId && ec.user.getUsername() === ec.user.username;
        ec.factory.getMoquiVersion() === ec.factory.moquiVersion;
        ec.entity.getConditionFactory() === ec.entity.conditionFactory;
    }
});

feature("ec.message lists are arrays", {
    allowMessageErrors: true,
    when() {
        seen = { before: ec.message.errors.length };
        ec.message.addError("EcApi error");
        ec.message.addPublic("EcApi public", "warning");
        seen.errors = ec.message.getErrors().length;
        seen.messages = ec.message.messages.includes("EcApi public") && ec.message.publicMessages.includes("EcApi public");
        seen.infos = ec.message.messageInfos.some((info) => info.message === "EcApi public" && info.typeString === "warning");
        seen.text = ec.message.getMessagesString().includes("EcApi public\n");
        ec.message.clearAll();
    },
    then() {
        seen.errors === seen.before + 1 && seen.messages && seen.infos && seen.text;
        ec.message.errors.length === 0 && ec.message.messages.length === 0;
    }
});
