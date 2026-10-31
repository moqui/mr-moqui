/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The Artifact Browser in the page: columns by generation, cards at their rows pushed past the one above, the vertical
// orientation, a connector's curve, a card's sections and a cycle.
"use strict";

const pure = moqui.devAide.pure;

const cards = [
    { id: "c", generation: 0 },
    { id: "in", generation: -1, parentId: "c" },
    { id: "out", generation: 1, parentId: "c" },
    // two rows of "out" expanded: the second row's card first in the list, but placed after the first's
    { id: "o2", generation: 2, parentId: "out", anchor: 200, order: 5 },
    { id: "o1", generation: 2, parentId: "out", anchor: 40, order: 1 }
];
const sizes = { c: { width: 260, height: 100 }, in: { width: 280, height: 300 }, out: { width: 280, height: 400 },
    o1: { width: 280, height: 250 }, o2: { width: 280, height: 100 } };

test("columns go by generation; a card sits at its row and is pushed past the card above it", () => {
    const placed = pure.browserLayout(cards, sizes, { columnWidth: 280, gap: 80, space: 16 });
    assert.deepEqual([placed.in.x, placed.c.x, placed.out.x, placed.o1.x], [-360, 0, 360, 720]);
    // o1 at its row (40, less the 12 that puts the row by the card's title), o2 wanted at 188 but pushed below o1
    assert.equal(placed.o1.y, 28);
    assert.equal(placed.o2.y, 28 + 250 + 16);
});

test("down, the generations stack by their tallest card and the cards go across", () => {
    const placed = pure.browserLayout(cards, sizes, { columnWidth: 280, gap: 40, space: 16, vertical: true });
    assert.equal(placed.c.y, 0);
    assert.equal(placed.out.y, 100 + 40);
    assert.equal(placed.o1.y, 140 + 400 + 40);
    assert.equal(placed.in.y, -300 - 40);
    // across: o1 at its row's offset, o2 past it
    assert.equal(placed.o2.x, placed.o1.x + 280 + 16);
});

test("a connector curves from the row to the near edge, either way", () => {
    const from = { x: 0, y: 0, width: 280, height: 300 }, to = { x: 360, y: 100, width: 280, height: 80 };
    assert.equal(pure.connectorPath(from, 50, to, false, false), "M280 50 C320 50 320 116 360 116");
    const before = { x: -360, y: 0, width: 280, height: 80 };
    assert.equal(pure.connectorPath(from, 50, before, true, false), "M0 50 C-40 50 -40 16 -80 16");
    assert.ok(pure.connectorPath(from, 30, { x: 0, y: 400, width: 280, height: 80 }, false, true).startsWith("M30 300 C30 350"));
});

test("a card's sections by kind, one row per artifact, filtered", () => {
    const refs = [
        { refType: "service-call", calls: 3, other: { id: "service:a.B.get#C", kind: "service", shortName: "get#C", name: "a.B.get#C", component: "site" } },
        { refType: "seca", calls: 0, other: { id: "service:a.B.get#C", kind: "service", shortName: "get#C", name: "a.B.get#C", component: "site" } },
        { refType: "entity-find", calls: 0, other: { id: "entity:x.Y", kind: "entity", shortName: "Y", name: "x.Y", component: "framework" } },
        { refType: "link", calls: 2, observedOnly: true, toKind: "screen", toName: "Thing" }
    ];
    const all = pure.browserSections(refs, {});
    assert.deepEqual(all.map((section) => section.kind), ["entity", "screen", "service"]);
    const service = all[2].rows[0];
    assert.deepEqual([service.calls, service.how, all[2].rows.length], [3, "service-call, seca", 1]);
    assert.equal(all[1].rows[0].resolved, false);
    assert.deepEqual(pure.browserSections(refs, { hideFramework: true, observed: false }).map((section) => section.kind), ["service"]);
    assert.deepEqual(pure.browserSections(refs, { kinds: ["entity"] }).map((section) => section.kind), ["entity"]);
});

test("rows of one name in a section are told apart by their owner", () => {
    const field = (entity) => ({ refType: "entity-field", other: { id: "field:mantle.order." + entity + ".orderId", kind: "field", shortName: "orderId",
        name: "mantle.order." + entity + ".orderId", component: "mantle" } });
    const rows = pure.browserSections([field("OrderHeader"), field("OrderItem"), field("OrderItem"),
        { refType: "entity-field", other: { id: "field:mantle.order.OrderHeader.statusId", kind: "field", shortName: "statusId", name: "mantle.order.OrderHeader.statusId" } }], {})[0].rows;
    assert.deepEqual(rows.map((row) => row.shortName), ["OrderHeader.orderId", "OrderItem.orderId", "statusId"]);
});

test("an artifact already on the path from the center is a cycle", () => {
    const path = [{ id: "c", artifactId: "a" }, { id: "x", artifactId: "b", parentId: "c" }, { id: "y", artifactId: "c", parentId: "x" }];
    assert.equal(pure.onPath(path, path[2], "a"), true);
    assert.equal(pure.onPath(path, path[2], "c"), true);
    assert.equal(pure.onPath(path, path[1], "c"), false);
});
