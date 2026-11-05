/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// The SQL editor's helpers (moqui.sql from /js/WebrootSql.js) in the page, with fixtures shaped like the answers of
// org.moqui.impl.DatabaseServices.run#Sql (columns and records) and explain#Sql (a Postgres FORMAT JSON plan).
"use strict";

const sql = moqui.sql;

test("a script splits on semicolons outside quotes, comments and dollar quotes", () => {
    const text = "select 1;\nselect ';' as x; -- a; comment\n$$a;b$$;\nselect a #> '{b}' from t";
    assert.deepEqual(sql.statements(text).map((s) => s.text), ["select 1", "select ';' as x", "-- a; comment\n$$a;b$$", "select a #> '{b}' from t"]);
    // MySQL: backticks, # comments, no dollar quotes
    assert.deepEqual(sql.statements("select `a;b` from t # c;\n; select 2", "mysql").map((s) => s.text), ["select `a;b` from t # c;", "select 2"]);
    assert.deepEqual(sql.statements("  ;; -- only a comment\n"), []);
    // as the server's split_sql_as: E'' escapes, nested comments, a $ inside a word, MySQL -- before a space only, no
    // backslash escapes in MySQL backticks
    assert.deepEqual(sql.statements("select E'a\\';b'; select 2").map((s) => s.text), ["select E'a\\';b'", "select 2"]);
    assert.deepEqual(sql.statements("select 'a\\'; select 2").map((s) => s.text), ["select 'a\\'", "select 2"], "no escapes outside E''");
    assert.deepEqual(sql.statements("select /* a /* b; */ c; */ 1; select 2").map((s) => s.text), ["select /* a /* b; */ c; */ 1", "select 2"]);
    assert.deepEqual(sql.statements("select a$b; select $1; select 2").map((s) => s.text), ["select a$b", "select $1", "select 2"]);
    assert.deepEqual(sql.statements("select 1 --x;\nselect 2", "mysql").map((s) => s.text), ["select 1 --x", "select 2"]);
    assert.deepEqual(sql.statements("select 1 -- x;\n; select 2", "mysql").map((s) => s.text), ["select 1 -- x;", "select 2"]);
    assert.deepEqual(sql.statements("select `a\\`; select 2", "mysql").map((s) => s.text), ["select `a\\`", "select 2"]);
    assert.deepEqual(sql.statements("select 'a\\';b'; select 2", "mysql").map((s) => s.text), ["select 'a\\';b'", "select 2"]);
    const at = sql.statements("select 1; select 2")[1];
    assert.equal("select 1; select 2".slice(at.from, at.to), "select 2");
});

test("the statement at the cursor, or the last one before it", () => {
    assert.equal(sql.statementAt("select 1; select 2", 3).text, "select 1");
    assert.equal(sql.statementAt("select 1; select 2", 12).text, "select 2");
    assert.equal(sql.statementAt("select 1;\n\n", 11).text, "select 1");
    assert.equal(sql.statementAt("", 0), null);
});

test(":name parameters, not casts or names in quotes and comments", () => {
    assert.deepEqual(sql.parameterNames("select :a, b::int, ':no' from t where c = :b and d = :a -- :c\n"), ["a", "b"]);
});

test("a result copies as TSV, CSV, JSON and INSERT statements", () => {
    const columns = ["enum_id", "description"];
    const records = [["A", "it's, \"quoted\""], ["B", null]];
    assert.equal(sql.tsv(columns, records), "enum_id\tdescription\nA\tit's, \"quoted\"\nB\t");
    assert.equal(sql.tsv(["a"], [['"starts quoted']]), 'a\n"""starts quoted"', "a cell starting with a quote is quoted");
    assert.equal(sql.csv(columns, records), "enum_id,description\r\nA,\"it's, \"\"quoted\"\"\"\r\nB,\r\n");
    assert.deepEqual(JSON.parse(sql.json(columns, records)), [{ enum_id: "A", description: "it's, \"quoted\"" }, { enum_id: "B", description: null }]);
    assert.equal(sql.inserts(columns, [[1, "it's"], [null, true]], "enumeration"),
        "INSERT INTO enumeration (enum_id, description) VALUES (1, 'it''s');\nINSERT INTO enumeration (enum_id, description) VALUES (NULL, TRUE);");
    // schema-qualified, MySQL quoting and backslashes, NaN and Infinity as text
    assert.equal(sql.inserts(["a b"], [[NaN], [Infinity], ["x\\y"]], 'app."my t"', "mysql"),
        "INSERT INTO app.`my t` (`a b`) VALUES ('NaN');\nINSERT INTO app.`my t` (`a b`) VALUES ('Infinity');\nINSERT INTO app.`my t` (`a b`) VALUES ('x\\\\y');");
    assert.equal(sql.inserts(["a"], [["x\\y"]], "public.t"), "INSERT INTO public.t (a) VALUES ('x\\y');");
});

test("a plan is a tree that marks a large seq scan, a misestimate per loop and a spill", () => {
    const plan = JSON.stringify([{ Plan: { "Node Type": "Sort", "Sort Key": ["e.description"], "Sort Method": "external merge",
        "Sort Space Type": "Disk", "Plan Rows": 5, "Actual Rows": 500, "Actual Loops": 1, "Total Cost": 12.5, "Actual Total Time": 3.2,
        Plans: [{ "Node Type": "Seq Scan", "Relation Name": "enumeration", Alias: "e", "Plan Rows": 20, "Actual Rows": 10, "Rows Removed by Filter": 19000, "Actual Loops": 1 },
            // 1 row a loop over 1,000 loops is as estimated: per loop, not in total
            { "Node Type": "Index Scan", "Relation Name": "enumeration_type", "Index Name": "enumeration_type_pk", "Plan Rows": 1, "Actual Rows": 1, "Actual Loops": 1000 },
            { "Node Type": "HashAggregate", "HashAgg Batches": 5, "Plan Rows": 10, "Actual Rows": 10, "Actual Loops": 1 },
            { "Node Type": "Seq Scan", "Relation Name": "never_run", "Plan Rows": 90000, "Actual Rows": 0, "Actual Loops": 0 }] } }]);
    const rows = sql.planRows("json", plan);
    assert.deepEqual(rows.map((row) => [row.depth, row.node, row.marks]), [[0, "Sort", ["misestimate", "spill"]], [1, "Seq Scan", ["seqScan"]],
        [1, "Index Scan", []], [1, "HashAggregate", ["spill"]], [1, "Seq Scan", []]]);
    assert.equal(rows[1].detail, "on enumeration e");
    assert.equal(rows[2].actualRows, 1);
    assert.equal(rows[2].loops, 1000);
    assert.equal(rows[4].actualRows, null, "a node that never ran has no actual rows");
    assert.equal(rows[0].timeMs, 3.2);
    // without ANALYZE a seq scan is marked by the relation's estimated rows (get#TableStats)
    const estimate = JSON.stringify([{ Plan: { "Node Type": "Seq Scan", "Relation Name": "enumeration", "Plan Rows": 3 } }]);
    assert.deepEqual(sql.planRows("json", estimate, { enumeration: 50000 })[0].marks, ["seqScan"]);
    assert.deepEqual(sql.planRows("json", estimate, { enumeration: 500 })[0].marks, []);
    assert.deepEqual(sql.planRows("text", "-> Table scan on t"), [], "a text plan is shown as it is");
});
