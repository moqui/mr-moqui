/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The Workspace's git depth in the page: the conflict view's regions and sides, a remote's web page, a hunk's label.
"use strict";

const pure = moqui.devAide.pure;

const CONFLICTED = [
    "one",
    "<<<<<<< HEAD",
    "five (main)",
    "=======",
    "five (other)",
    ">>>>>>> other",
    "six",
    "<<<<<<< HEAD",
    "ten",
    "||||||| base",
    "TEN",
    "=======",
    "Ten",
    "eleven",
    ">>>>>>> other",
    "end"
].join("\n");

test("conflicts are read with their sides, the base when diff3 wrote it", () => {
    const regions = pure.conflictRegions(CONFLICTED);
    assert.equal(regions.length, 2);
    assert.deepEqual([regions[0].start, regions[0].end], [1, 5]);
    assert.deepEqual(regions[0].ours, ["five (main)"]);
    assert.deepEqual(regions[0].theirs, ["five (other)"]);
    assert.equal(regions[0].base, null);
    assert.deepEqual(regions[1].base, ["TEN"]);
    assert.deepEqual(regions[1].theirs, ["Ten", "eleven"]);
    assert.deepEqual(pure.conflictRegions("no markers\n"), []);
});

test("a side is taken into the result, one conflict at a time", () => {
    const ours = pure.chooseRegion(CONFLICTED, 0, "ours");
    assert.equal(pure.conflictRegions(ours).length, 1);
    assert.ok(ours.startsWith("one\nfive (main)\nsix\n"));
    const both = pure.chooseRegion(ours, 0, "both");
    assert.equal(both, "one\nfive (main)\nsix\nten\nTen\neleven\nend");
    assert.equal(pure.chooseRegion(both, 0, "theirs"), both);
    assert.equal(pure.chooseRegion(CONFLICTED, 1, "theirs").split("\n").slice(7, 9).join(","), "Ten,eleven");
});

test("a remote's page on its host", () => {
    assert.equal(pure.hostPage("https://github.com/moqui/mr-moqui.git"), "https://github.com/moqui/mr-moqui");
    assert.equal(pure.hostPage("git@github.com:moqui/mr-moqui.git"), "https://github.com/moqui/mr-moqui");
    assert.equal(pure.hostPage("ssh://git@gitea.example.com/team/site.git"), "https://gitea.example.com/team/site");
    assert.equal(pure.hostPage("https://token@git.example.com/a/b/"), "https://git.example.com/a/b");
    assert.equal(pure.hostPage("/srv/git/site.git"), null);
});

test("a hunk's label names its lines", () => {
    assert.equal(pure.hunkLabel({ oldStart: 1, oldLines: 4, newStart: 1, newLines: 5 }, false), "lines 1–5 (−4 +5)");
    assert.equal(pure.hunkLabel({ oldStart: 10, oldLines: 1, newStart: 11, newLines: 1 }, true), "staged line 11 (−1 +1)");
    assert.equal(pure.hunkLabel({ oldStart: 3, oldLines: 2, newStart: 2, newLines: 0 }, false), "removed lines near 2 (−2 +0)");
});
