/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The emoji picker (js/WebrootEmoji.js), in the page: moqui.emoji's search and recent emoji, then m-emoji-picker mounted in
// a Vue application with Quasar over the vendored data file.
"use strict";
const emoji = moqui.emoji;
const settle = async () => {
    for (let i = 0; i < 4; i++) { await Vue.nextTick(); await new Promise((resolve) => setTimeout(resolve, 0)); }
};
const until = async (check, what) => {
    const end = Date.now() + 5000;
    for (;;) {
        const value = check();
        if (value) return value;
        if (Date.now() > end) throw new Error("waited for " + what);
        await new Promise((resolve) => setTimeout(resolve, 20));
    }
};
const GROUPS = [{ slug: "smileys_emotion", name: "Smileys & Emotion", emoji: [["😀", "grinning face"], ["😂", "face with tears of joy"], ["😃", "grinning face with big eyes"]] },
    { slug: "people_body", name: "People & Body", emoji: [["👍", "thumbs up"], ["👎", "thumbs down"], ["👋", "waving hand"]] }];

test("search: every word typed starts a word of the name, ':' and '_' are spaces, the emoji itself matches first", () => {
    assert.deepEqual(emoji.search(GROUPS, "thumbs").map((one) => one[0]), ["👍", "👎"]);
    assert.deepEqual(emoji.search(GROUPS, ":thumbs_up").map((one) => one[0]), ["👍"]);
    assert.deepEqual(emoji.search(GROUPS, "gri big").map((one) => one[0]), ["😃"]);
    assert.deepEqual(emoji.search(GROUPS, "ears").map((one) => one[0]), [], "a word's middle is no match");
    assert.deepEqual(emoji.search(GROUPS, "👋").map((one) => one[0]), ["👋"]);
    assert.deepEqual(emoji.search(GROUPS, "  "), []);
    assert.equal(emoji.search(GROUPS, "face", 2).length, 2, "at most the limit");
});

test("recent emoji: newest first, each once, at most RECENT_MAX, per user under the storage scope; storage that throws keeps nothing", () => {
    const priorScope = moqui.storageScope;
    moqui.storageScope = "acct-1";
    try {
        localStorage.removeItem(emoji.key("ann"));
        assert.deepEqual(emoji.recent("ann"), []);
        emoji.remember("ann", "👍");
        emoji.remember("ann", "😂");
        assert.deepEqual(emoji.remember("ann", "👍"), ["👍", "😂"]);
        assert.deepEqual(emoji.recent("bob"), [], "another user's are their own");
        moqui.storageScope = "acct-2";
        assert.deepEqual(emoji.recent("ann"), [], "another account's are its own");
        moqui.storageScope = "acct-1";
        for (let n = 0; n < emoji.RECENT_MAX + 5; n++) emoji.remember("ann", String.fromCodePoint(0x1F600 + n));
        assert.equal(emoji.recent("ann").length, emoji.RECENT_MAX);
        localStorage.setItem(emoji.key("ann"), "not json");
        assert.deepEqual(emoji.recent("ann"), []);
        const getItem = Storage.prototype.getItem;
        const setItem = Storage.prototype.setItem;
        Storage.prototype.getItem = () => { throw new Error("blocked"); };
        Storage.prototype.setItem = () => { throw new Error("blocked"); };
        try {
            assert.deepEqual(emoji.recent("ann"), []);
            assert.deepEqual(emoji.remember("ann", "🎉"), ["🎉"], "the pick still goes on");
        } finally {
            Storage.prototype.getItem = getItem;
            Storage.prototype.setItem = setItem;
        }
    } finally {
        localStorage.removeItem(emoji.key("ann"));
        moqui.storageScope = priorScope;
    }
});

test("m-emoji-picker: the vendored data's groups as tabs, Recent first once there is one, search across them, Enter and a click pick", async () => {
    const groups = await emoji.load();
    assert.ok(groups.length >= 8 && groups.every((group) => group.slug && group.name && group.emoji.length), "the data file's groups");
    assert.ok(groups.some((group) => group.emoji.some((one) => one[0] === "👍" && one[1] === "thumbs up")));
    localStorage.removeItem(emoji.key("ann"));
    const picks = [];
    const host = document.createElement("div");
    document.body.appendChild(host);
    const app = Vue.createApp({ data: () => ({ open: true }), methods: { pick(one) { picks.push(one); } },
        template: '<m-emoji-picker v-if="open" user-id="ann" @pick="pick"></m-emoji-picker>' });
    app.use(Quasar);
    app.component("m-emoji-picker", moqui.qvtComponents["m-emoji-picker"]);
    const vm = app.mount(host);
    try {
        const tabs = () => Array.from(host.querySelectorAll(".m-emoji-tabs [data-group]")).map((el) => el.getAttribute("data-group"));
        const cells = () => Array.from(host.querySelectorAll(".m-emoji-cell")).map((el) => el.getAttribute("data-emoji"));
        await until(() => cells().length, "the first group");
        assert.deepEqual(tabs(), groups.map((group) => group.slug), "no Recent before a pick");
        assert.deepEqual(cells(), groups[0].emoji.map((one) => one[0]));
        assert.equal(document.activeElement, host.querySelector("input"), "the search line has the focus");
        host.querySelector('[data-group="food_drink"]').click();
        await settle();
        assert.deepEqual(cells(), groups.find((group) => group.slug === "food_drink").emoji.map((one) => one[0]));
        assert.equal(host.querySelector('[data-group="food_drink"]').getAttribute("aria-selected"), "true");
        // a search across the groups hides the tabs; Enter picks its first emoji
        const input = host.querySelector("input");
        input.value = "thumbs up";
        input.dispatchEvent(new Event("input", { bubbles: true }));
        await settle();
        assert.equal(host.querySelector(".m-emoji-tabs"), null);
        assert.equal(cells()[0], "👍");
        input.dispatchEvent(new KeyboardEvent("keydown", { key: "Enter", bubbles: true }));
        await settle();
        assert.deepEqual(picks, ["👍"]);
        input.value = "zzzz no such";
        input.dispatchEvent(new Event("input", { bubbles: true }));
        await settle();
        assert.ok(host.querySelector(".m-emoji-none"), "says nothing matches");
        // a click picks; the picks are the user's recent ones, the newest first, in Recent, the first tab of the next picker
        input.value = "";
        input.dispatchEvent(new Event("input", { bubbles: true }));
        await settle();
        host.querySelector('[data-group="animals_nature"]').click();
        await settle();
        const cat = host.querySelector(".m-emoji-cell");
        const catEmoji = cat.getAttribute("data-emoji");
        cat.click();
        await settle();
        assert.deepEqual(picks, ["👍", catEmoji]);
        assert.deepEqual(emoji.recent("ann"), [catEmoji, "👍"]);
        vm.open = false;
        await settle();
        vm.open = true;
        await until(() => cells().length, "the picker again");
        assert.equal(tabs()[0], "recent");
        assert.deepEqual(cells(), [catEmoji, "👍"], "Recent is shown first");
        assert.equal(host.querySelector(".m-emoji-cell").getAttribute("aria-label"), groups.find((group) => group.slug === "animals_nature").emoji[0][1], "with its name");
    } finally {
        app.unmount();
        host.remove();
        localStorage.removeItem(emoji.key("ann"));
    }
});
