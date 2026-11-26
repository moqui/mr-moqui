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

test("Run locally shows the combination it runs, and names a secret without setting it", () => {
    const job = { pipeline: "check", job: "db", image: "moqui-build", matrixKeys: ["postgres", "mysql"], secrets: ["DEPLOY_KEY"],
        combinations: [
            { matrixKey: "postgres", steps: [{ name: null, run: "./moq test --db postgres" }], env: [["MATRIX_DB", "postgres"]] },
            { matrixKey: "mysql", steps: [{ name: "Test", run: "./moq test --db mysql", continueOnError: true }], env: [["MATRIX_DB", "mysql"]],
                script: "#!/bin/sh\nsh -c './moq test --db mysql'\n", digest: "d-mysql" }] };
    const mysql = pure.localRunLines(job, "mysql");
    assert.deepEqual(mysql.steps, [{ seq: 1, name: "Test", run: "./moq test --db mysql", continueOnError: true }]);
    assert.deepEqual(mysql.env, ["MATRIX_DB=mysql"]);
    assert.deepEqual(mysql.secrets, ["DEPLOY_KEY"]);
    assert.equal(mysql.digest, "d-mysql");
    assert.ok(mysql.script.includes("--db mysql"));
    assert.equal(pure.localRunLines(job, "").steps[0].run, "./moq test --db postgres");
    assert.deepEqual(pure.localRunLines(null, "").steps, []);
});

test("Run locally in the job's image shows its own script, and a run ends by its state", () => {
    const job = { pipeline: "check", job: "unit", image: "busybox", combinations: [{ matrixKey: "", steps: [], env: [],
        script: "#!/bin/sh\n# This machine\n", digest: "d-host", imageScript: "#!/bin/sh\n# In the job's image\n", imageDigest: "d-image" }] };
    const lines = pure.localRunLines(job, "");
    assert.equal(lines.imageDigest, "d-image");
    assert.ok(lines.imageScript.includes("job's image"));
    assert.equal(lines.digest, "d-host");
    assert.ok(pure.localRunEnded("exited") && pure.localRunEnded("failed") && pure.localRunEnded("missing"));
    assert.ok(!pure.localRunEnded("starting") && !pure.localRunEnded("running"));
});

test("Open on the server fills the server's page templates, each value as path parts", () => {
    const pages = { file: "https://team.example.com/code/team/site/-/blob/{ref}/{file}#L{line}",
        commit: "https://team.example.com/code/team/site/-/commit/{commitId}", branch: "https://team.example.com/code/team/site/-/tree/{branch}" };
    assert.equal(pure.serverPage(pages, "file", { ref: "feature/x", file: "screen/My Screen.xml", line: 12 }),
        "https://team.example.com/code/team/site/-/blob/feature/x/screen/My%20Screen.xml#L12");
    // with no line the line's fragment goes
    assert.equal(pure.serverPage(pages, "file", { ref: "main", file: "a.xml" }), "https://team.example.com/code/team/site/-/blob/main/a.xml");
    assert.equal(pure.serverPage(pages, "commit", { commitId: "0123abcd" }), "https://team.example.com/code/team/site/-/commit/0123abcd");
    assert.equal(pure.serverPage(pages, "commit", { commitId: "../../evil" }), null);
    assert.equal(pure.serverPage(pages, "branch", { branch: "a?b#c" }), "https://team.example.com/code/team/site/-/tree/a%3Fb%23c");
    assert.equal(pure.serverPage(pages, "branch", { branch: "" }), null);
    assert.equal(pure.serverPage({ file: "javascript:alert(1)//{file}" }, "file", { file: "x" }), null);
    assert.equal(pure.serverPage(null, "file", { file: "x" }), null);
    assert.equal(pure.serverBranch("origin/feature/x", "origin"), "feature/x");
    assert.equal(pure.serverBranch("fork/feature", "origin"), "");
    assert.equal(pure.serverBranch(null, "origin"), "");
});
