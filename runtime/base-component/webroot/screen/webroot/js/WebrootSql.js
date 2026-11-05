/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

/*
 * The pure helpers of the SQL editor (Tools › SQL, plans/Tools-Database.md §8, §7.5) as moqui.sql, loaded by
 * component://tools/screen/Tools/Sql.qvue and tested in the page by test/client/pages/WebrootSql.page.js:
 *   - blank(text, dialect) and statements(text, dialect): the script with its comments and quoted runs blanked, and its
 *     statements split on the semicolons left, with their offsets, as the server's blank_sql and split_sql_as
 *     (crates/moqui-entity/src/entity/exec.rs): Postgres E'' escapes, nested comments and dollar quotes; MySQL and
 *     MariaDB backslash escapes (not in ``), # comments, and -- only before a space;
 *   - statementAt(text, position, dialect): the statement the cursor is in (or the one before it, at a script's end);
 *   - parameterNames(text, dialect): the :name parameters, not ::casts or names in quotes or comments;
 *   - tsv, csv, json and inserts(columns, records, table, dialect): a result as text to copy or download;
 *   - planRows(format, plan, relationRows): an EXPLAIN plan as rows of a tree (depth, node, detail, estimated and actual
 *     rows per loop, loops, time), each with its marks: a sequential scan over 10,000 rows (read, with ANALYZE; else the
 *     relation's estimate), an estimate per loop off by more than ten times, a sort, hash or aggregate that spilled to disk.
 */
(function (root) {
    "use strict";

    const moqui = root.moqui || (root.moqui = {});
    const sql = moqui.sql = moqui.sql || {};

    // ---- statements ----

    const isMysql = (dialect) => /mysql|maria/i.test(String(dialect || ""));
    const isWord = (c) => c !== undefined && (/[A-Za-z0-9_$]/.test(c) || c.charCodeAt(0) >= 0x80);

    function blockCommentEnd(text, at, nests) {
        let depth = 1;
        while (at < text.length && depth > 0) {
            if (nests && text[at] === "/" && text[at + 1] === "*") { depth++; at += 2; }
            else if (text[at] === "*" && text[at + 1] === "/") { depth--; at += 2; }
            else at++;
        }
        return Math.min(at, text.length);
    }
    function quotedEnd(text, at, quote, escapes) {
        while (at < text.length) {
            if (escapes && text[at] === "\\") at += 2;
            else if (text[at] === quote) { if (text[at + 1] !== quote) return at + 1; at += 2; }
            else at++;
        }
        return text.length;
    }
    function dollarTag(text, at) {
        const match = /^\$([^$]*)\$/.exec(text.slice(at, at + 66));
        if (!match) return null;
        return /^(?:[_\p{L}][_\p{L}0-9]*)?$/u.test(match[1]) ? match[0] : null;
    }

    // The text with each comment made spaces and the inside of each quoted text, quoted name and dollar-quoted body made
    // "_", at the same positions, as the server's blank_sql (crates/moqui-entity/src/entity/exec.rs): what is split and
    // what parameters are read from.
    sql.blank = (text, dialect) => {
        const source = String(text || "");
        const mysql = isMysql(dialect);
        const out = source.split("");
        const fill = (from, to, c) => { for (let k = from; k < Math.min(to, out.length); k++) out[k] = c; };
        let at = 0;
        while (at < source.length) {
            const c = source[at], next = source[at + 1];
            if (c === "-" && next === "-" && (!mysql || at + 2 >= source.length || /\s/.test(source[at + 2]))) {
                const end = source.indexOf("\n", at); const stop = end < 0 ? source.length : end;
                fill(at, stop, " "); at = stop;
            } else if (c === "#" && mysql) {
                const end = source.indexOf("\n", at); const stop = end < 0 ? source.length : end;
                fill(at, stop, " "); at = stop;
            } else if (c === "/" && next === "*") {
                const end = blockCommentEnd(source, at + 2, !mysql);
                fill(at, end, " "); at = end;
            } else if (c === "'" || c === '"' || c === "`") {
                // MySQL: backslash escapes in '' and "", none in ``; Postgres: only in E'' (an E that ends a longer word is not one)
                const escapes = mysql ? c !== "`" : c === "'" && at > 0 && /[Ee]/.test(source[at - 1]) && !(at > 1 && isWord(source[at - 2]));
                const end = quotedEnd(source, at + 1, c, escapes);
                const innerEnd = end <= source.length && source[end - 1] === c && end > at + 1 ? end - 1 : end;
                fill(at + 1, innerEnd, "_"); at = end;
            } else if (c === "$" && !mysql && !(at > 0 && isWord(source[at - 1]))) {
                const tag = dollarTag(source, at);
                if (tag) {
                    const body = at + tag.length;
                    const close = source.indexOf(tag, body); const stop = close < 0 ? source.length : close;
                    fill(body, stop, "_"); at = Math.min(stop + tag.length, source.length);
                } else at++;
            } else at++;
        }
        return out.join("");
    };

    // [{text, from, to}]: each statement trimmed, with its offsets in text; empty and comment-only ones left out.
    sql.statements = (text, dialect) => {
        const source = String(text || "");
        const blank = sql.blank(source, dialect);
        const out = [];
        let start = 0;
        const push = (end) => {
            if (!blank.slice(start, end).trim()) return;
            const piece = source.slice(start, end);
            const lead = piece.length - piece.replace(/^\s+/, "").length;
            const trimmed = piece.trim();
            out.push({ text: trimmed, from: start + lead, to: start + lead + trimmed.length });
        };
        for (let i = 0; i < blank.length; i++) if (blank[i] === ";") { push(i); start = i + 1; }
        push(source.length);
        return out;
    };

    sql.statementAt = (text, position, dialect) => {
        const list = sql.statements(text, dialect);
        if (!list.length) return null;
        const at = Number(position) || 0;
        const inside = list.find((statement) => at >= statement.from && at <= statement.to);
        if (inside) return inside;
        const before = list.filter((statement) => statement.to <= at);
        return before.length ? before[before.length - 1] : list[0];
    };

    sql.parameterNames = (text, dialect) => {
        const blank = sql.blank(text, dialect);
        const names = [];
        const pattern = /(^|[^:A-Za-z0-9_]):([A-Za-z_][A-Za-z_0-9]*)/g;
        let match;
        while ((match = pattern.exec(blank))) {
            if (!names.includes(match[2])) names.push(match[2]);
        }
        return names;
    };

    // ---- results as text ----

    const cellText = (value) => (value == null ? "" : typeof value === "object" ? JSON.stringify(value) : String(value));

    // a cell that starts with a quote is quoted, so a spreadsheet does not read the quote as the start of a quoted cell
    const tsvCell = (value) => {
        const text = cellText(value).replace(/[\t\r\n]+/g, " ");
        return text.startsWith('"') ? '"' + text.replace(/"/g, '""') + '"' : text;
    };
    sql.tsv = (columns, records) => [columns.map(tsvCell).join("\t")]
        .concat((records || []).map((record) => record.map(tsvCell).join("\t"))).join("\n");

    const csvCell = (value) => {
        const text = cellText(value);
        return /[",\r\n]/.test(text) ? '"' + text.replace(/"/g, '""') + '"' : text;
    };
    sql.csv = (columns, records) => [columns.map(csvCell).join(",")]
        .concat((records || []).map((record) => record.map(csvCell).join(","))).join("\r\n") + "\r\n";

    sql.json = (columns, records) => JSON.stringify((records || []).map((record) => {
        const row = {};
        columns.forEach((column, index) => { row[column] = record[index] == null ? null : record[index]; });
        return row;
    }), null, 2);

    // An identifier as the dialect quotes one: as it is when it needs no quotes, else "" (Postgres) or `` (MySQL, MariaDB).
    const quoteName = (name, mysql) => {
        const text = String(name);
        if (/^[A-Za-z_][A-Za-z_0-9]*$/.test(text)) return text;
        return mysql ? "`" + text.replace(/`/g, "``") + "`" : '"' + text.replace(/"/g, '""') + '"';
    };
    // schema.table as two identifiers; a name with a dot in it is typed quoted ("my.table")
    const quoteTable = (table, mysql) => {
        const parts = [];
        const pattern = /"((?:[^"]|"")*)"|`((?:[^`]|``)*)`|([^.]+)/g;
        let match;
        while ((match = pattern.exec(String(table)))) {
            parts.push(match[1] != null ? match[1].replace(/""/g, '"') : match[2] != null ? match[2].replace(/``/g, "`") : match[3].trim());
        }
        return parts.filter((part) => part !== "").map((part) => quoteName(part, mysql)).join(".");
    };
    const literal = (value, mysql) => {
        if (value == null) return "NULL";
        if (typeof value === "number") return Number.isFinite(value) ? String(value) : "'" + String(value) + "'";
        if (typeof value === "bigint") return String(value);
        if (typeof value === "boolean") return value ? "TRUE" : "FALSE";
        let text = cellText(value).replace(/'/g, "''");
        // MySQL and MariaDB read a backslash in a string as an escape
        if (mysql) text = text.replace(/\\/g, "\\\\");
        return "'" + text + "'";
    };
    // INSERT statements of a result into table (typed by the user: a result does not say which table its columns are
    // from), in the dialect of the database (sql.statements' dialect names)
    sql.inserts = (columns, records, table, dialect) => {
        const mysql = isMysql(dialect);
        const into = "INSERT INTO " + quoteTable(table || "result_table", mysql) + " (" + columns.map((column) => quoteName(column, mysql)).join(", ") + ") VALUES (";
        return (records || []).map((record) => into + record.map((value) => literal(value, mysql)).join(", ") + ");").join("\n");
    };

    // ---- plans ----

    // thresholds of the marks
    sql.planLimits = { seqScanRows: 10000, misestimate: 10 };

    // A Postgres plan node and its children. Rows are per loop, as Postgres gives both its estimate and the actual rows; a
    // node that never ran (Actual Loops 0) is not marked. A seq scan's rows are, with ANALYZE, the rows it read (returned
    // plus removed by its filter), else the catalog's estimate of the relation's rows (relationRows, from get#TableStats).
    function postgresRows(node, depth, out, relationRows) {
        const estimated = node["Plan Rows"];
        const loops = node["Actual Loops"];
        const analyzed = node["Actual Rows"] != null;
        const ran = analyzed && loops > 0;
        const actual = analyzed ? node["Actual Rows"] : null;
        const marks = [];
        if (/Seq Scan/.test(node["Node Type"] || "")) {
            const relation = node["Relation Name"] ? String(node["Relation Name"]).toLowerCase() : "";
            const scanned = analyzed ? (ran ? actual + (node["Rows Removed by Filter"] || 0) : null)
                : relationRows && relationRows[relation] != null ? Number(relationRows[relation]) : null;
            if (scanned != null && scanned > sql.planLimits.seqScanRows) marks.push("seqScan");
        }
        if (ran && estimated != null) {
            const high = Math.max(actual, estimated, 1), low = Math.max(Math.min(actual, estimated), 1);
            if (high / low > sql.planLimits.misestimate) marks.push("misestimate");
        }
        if (node["Sort Space Type"] === "Disk" || (node["Hash Batches"] || 0) > 1 || (node["Original Hash Batches"] || 0) > 1
            || (node["HashAgg Batches"] || 0) > 1 || (node["Disk Usage"] || 0) > 0 || (node["Temp Written Blocks"] || 0) > 0
            || /external/i.test(node["Sort Method"] || "")) marks.push("spill");
        const detail = [node["Relation Name"] ? "on " + node["Relation Name"] + (node.Alias && node.Alias !== node["Relation Name"] ? " " + node.Alias : "") : "",
            node["Index Name"] ? "using " + node["Index Name"] : "", node["Join Type"] ? node["Join Type"] + " join" : "",
            node["Filter"] ? "filter " + node["Filter"] : "", node["Index Cond"] ? "cond " + node["Index Cond"] : "",
            node["Hash Cond"] ? "hash " + node["Hash Cond"] : "", node["Sort Key"] ? "by " + [].concat(node["Sort Key"]).join(", ") : "",
            node["Sort Method"] ? node["Sort Method"] + (node["Sort Space Used"] != null ? " " + node["Sort Space Used"] + " kB" : "") : "",
            analyzed && !ran ? "never executed" : ""]
            .filter(Boolean).join("; ");
        out.push({ depth, node: node["Node Type"] || "?", detail, estimatedRows: estimated == null ? null : estimated,
            actualRows: ran ? actual : null, loops: analyzed ? loops : null, totalCost: node["Total Cost"] == null ? null : node["Total Cost"],
            timeMs: node["Actual Total Time"] == null ? null : node["Actual Total Time"], marks });
        for (const child of node.Plans || []) postgresRows(child, depth + 1, out, relationRows);
    }

    // MySQL and MariaDB FORMAT=JSON: query_block with nested tables, nested_loop and ordering operations. Rows are per scan;
    // MariaDB ANALYZE gives r_rows per loop beside rows.
    function mysqlRows(node, depth, out) {
        if (!node || typeof node !== "object") return;
        if (Array.isArray(node)) { node.forEach((item) => mysqlRows(item, depth, out)); return; }
        if (node.table && node.table.table_name) {
            const table = node.table;
            const estimated = Number(table.rows_examined_per_scan || table.rows || 0) || null;
            const marks = [];
            if (table.access_type === "ALL" && estimated > sql.planLimits.seqScanRows) marks.push("seqScan");
            const actual = table.r_rows != null ? Number(table.r_rows) : null;
            const ran = actual != null && (table.r_loops == null || Number(table.r_loops) > 0);
            if (ran && estimated != null) {
                const high = Math.max(actual, estimated, 1), low = Math.max(Math.min(actual, estimated), 1);
                if (high / low > sql.planLimits.misestimate) marks.push("misestimate");
            }
            out.push({ depth, node: "table " + table.table_name, detail: [table.access_type ? "access " + table.access_type : "",
                table.key ? "using " + table.key : "", table.attached_condition ? "filter " + table.attached_condition : ""].filter(Boolean).join("; "),
                estimatedRows: estimated, actualRows: ran ? actual : null, loops: table.r_loops != null ? Number(table.r_loops) : null,
                totalCost: null, timeMs: null, marks });
            return;
        }
        for (const [key, value] of Object.entries(node)) {
            if (!value || typeof value !== "object") continue;
            if (["query_block", "nested_loop", "ordering_operation", "grouping_operation", "filesort", "temporary_table",
                "duplicates_removal", "union_result", "materialized_from_subquery"].includes(key)) {
                out.push({ depth, node: key.replace(/_/g, " "), detail: "", estimatedRows: null, actualRows: null, loops: null, totalCost: null,
                    timeMs: null, marks: [] });
                mysqlRows(value, depth + 1, out);
            } else mysqlRows(value, depth, out);
        }
    }

    // relationRows: the catalog's estimated rows of each table, by lower-case name, for a plan run without ANALYZE
    sql.planRows = (format, plan, relationRows) => {
        let parsed = plan;
        if (typeof plan === "string") {
            if (format !== "json") return [];
            try { parsed = JSON.parse(plan); } catch (_) { return []; }
        }
        const out = [];
        const first = Array.isArray(parsed) ? parsed[0] : parsed;
        if (first && first.Plan) postgresRows(first.Plan, 0, out, relationRows);
        else if (first) mysqlRows(first, 0, out);
        return out;
    };
})(typeof window !== "undefined" ? window : globalThis);
