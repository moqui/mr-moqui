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

// Shared support for the JavaScript macro templates (XmlActions.js now, the
// screen macros later). This file is one expression that evaluates to the
// support object. The host passes that object to each macro file, which is a
// function of it, so nothing is left in the global scope.
//
// The model: one macro function per XML element, called as macro(node, children)
// where node is {name, attrs, text, children} and children is the already
// rendered text of the child elements. A macro prepares values in local
// constants, then returns one template. Macros come in kinds, and the kind
// wrappers below check the result:
//
//   stmt   one or more whole statements or lines. The text ends with a newline.
//          A multi-line statement is written with the js tag so the template
//          reads like its output.
//   expr   a fragment used inside a larger line. No trailing newline.
//   chain  a method call fragment appended to a call chain, such as
//          ".condition(...)". The text is empty or starts with a dot.
//   macro  a macro that is a statement or an expression depending on its
//          children (compare, expression). The wrapper only adds error context.
//
// Every wrapper names the element and its attributes in any error. The debug
// option adds a comment line naming the element before each statement.
(function () {
    "use strict";

    // Text between these marks is user code, such as the body of a script. A
    // multi-line string in that code must not gain indentation, so the js tag
    // does not indent lines inside a raw region. The root macro strips the marks.
    const RAW_START = "\u0001";
    const RAW_END = "\u0002";
    const RAW_MARKS = /[\u0001\u0002]/g;

    const options = { debug: false };

    const attr = (node, name) => {
        const attrs = node.attrs || {};
        return attrs[name] == null ? "" : String(attrs[name]);
    };

    const has = (node, name) => !!(node.attrs && Object.prototype.hasOwnProperty.call(node.attrs, name));

    // The attribute as written, or the JS literal null when it is absent.
    const attrOrNull = (node, name) => has(node, name) ? attr(node, name) : "null";

    // A JS string literal for the text.
    const jsString = (text) => JSON.stringify(text == null ? "" : String(text));

    const childrenNamed = (node, name) => node.children.filter((kid) => kid.name === name);

    const child = (node, name) => childrenNamed(node, name)[0] || null;

    const sourcesOf = (list) => list.map((item) => item.source || "").join("");

    // The sources of statements in a body. A child that renders as an expression, such as a
    // bare expression element, is ended as a statement so it does not run into the next one.
    const bodyOf = (list) => list.map((item) => {
        const source = item.source || "";
        if (source === "" || source.endsWith("\n")) return source;
        return /[;}]$/.test(source) ? source + "\n" : source + ";\n";
    }).join("");

    // Join only the pieces that are present, for chains with optional calls.
    const joinPresent = (pieces) => pieces.filter(Boolean).join("");

    // Mark user code that must be emitted unchanged. Single-line text needs no mark.
    const raw = (text) => text.indexOf("\n") < 0 ? text : RAW_START + text + RAW_END;

    const unraw = (text) => text.replace(RAW_MARKS, "");

    // Leading spaces of each line that starts in the static parts of a template.
    // A line whose first content is a value counts by the spaces before the value.
    const lineIndents = (parts) => {
        const indents = [];
        parts.forEach((part, index) => {
            const segments = part.split("\n");
            segments.forEach((segment, k) => {
                if (k === 0 && index > 0) return;
                const spaces = segment.length - segment.trimStart().length;
                const blank = spaces === segment.length;
                const last = k === segments.length - 1;
                if (!blank) indents.push(spaces);
                else if (last && index < parts.length - 1) indents.push(spaces);
            });
        });
        return indents;
    };

    const dedent = (parts) => {
        const indent = Math.min(...lineIndents(parts), Infinity);
        if (indent === Infinity || indent === 0) return parts;
        return parts.map((part, index) => part.split("\n").map((segment, k) => {
            if (k === 0 && index > 0) return segment;
            const spaces = segment.length - segment.trimStart().length;
            return segment.slice(Math.min(indent, spaces));
        }).join("\n"));
    };

    // Put spaces at the start of each line of the text except its first and
    // except lines inside a raw region.
    const reindent = (text, spaces) => {
        let inRaw = false;
        return text.split("\n").map((line, k) => {
            const prefix = (k > 0 && !inRaw && line !== "") ? spaces : "";
            for (const ch of line) {
                if (ch === RAW_START) inRaw = true;
                else if (ch === RAW_END) inRaw = false;
            }
            return prefix + line;
        }).join("\n");
    };

    const valueText = (value, index) => {
        const type = typeof value;
        if (type === "string") return value;
        if (type === "number" || type === "boolean") return String(value);
        throw new TypeError(`js template value ${index} is ${value === null ? "null" : type}`);
    };

    // A template tag for multi-line generated code. The template is written at
    // any indentation and is dedented, and the first and last lines are dropped
    // when blank. A value alone on its line is indented to that line, so a block
    // of lines goes into a body at the right depth; if it is empty the line is
    // removed, which handles optional lines. Values must be strings, numbers or
    // booleans, so an undefined that would print as "undefined" is an error.
    const js = (strings, ...values) => {
        const parts = Array.from(strings);
        // A template that starts on its own line is a block of lines and ends with a newline.
        const block = parts[0].startsWith("\n");
        if (block) parts[0] = parts[0].slice(1);
        const lastPart = parts.length - 1;
        parts[lastPart] = parts[lastPart].replace(/\n[ ]*$/, "");
        const text = dedent(parts);
        let out = text[0];
        values.forEach((value, index) => {
            const str = valueText(value, index);
            const next = text[index + 1];
            const lineStart = out.lastIndexOf("\n") + 1;
            const before = out.slice(lineStart);
            const alone = /^ *$/.test(before) && (next === "" ? index + 1 === lastPart : next.startsWith("\n"));
            if (!alone) {
                out += str + next;
            } else if (str === "") {
                out = out.slice(0, lineStart) + (next.startsWith("\n") ? next.slice(1) : next);
            } else {
                const body = str.endsWith("\n") ? str.slice(0, -1) : str;
                out += reindent(body, before) + next;
            }
        });
        return block && !out.endsWith("\n") ? out + "\n" : out;
    };

    // The element as written, on one line, for error and debug text.
    const describe = (node) => {
        const attrs = Object.keys(node.attrs || {}).map((key) => ` ${key}="${node.attrs[key]}"`).join("");
        const text = `<${node.name}${attrs}>`.replace(/\s+/g, " ");
        return text.length > 160 ? text.slice(0, 157) + "..." : text;
    };

    const checks = {
        stmt: (out) => out === "" || out.endsWith("\n") ? null : "a statement must end with a newline",
        expr: (out) => out.endsWith("\n") ? "an expression must not end with a newline" : null,
        chain: (out) => out === "" || out.startsWith(".") ? null : "a call chain fragment must be empty or start with a dot",
        macro: () => null
    };

    const kind = (name) => (fn) => (node, children) => {
        let out;
        try {
            out = fn(node, children);
            if (typeof out !== "string") throw new TypeError(`macro returned ${out === null ? "null" : typeof out}`);
            if (name === "stmt" && out !== "" && !out.endsWith("\n")) out += "\n";
            const problem = checks[name](out);
            if (problem) throw new Error(problem);
        } catch (e) {
            throw new Error(`${describe(node)}: ${e.message}`);
        }
        if (options.debug && name === "stmt" && out !== "") out = `// ${describe(node)}\n${out}`;
        return out;
    };

    // A macro with no output, for elements a parent macro reads directly.
    const noop = () => "";

    // A table of noop macros by element name.
    const noops = (names) => Object.fromEntries(names.map((name) => [name, noop]));

    return {
        options, attr, has, attrOrNull, jsString, child, childrenNamed, sourcesOf, bodyOf, joinPresent,
        raw, unraw, js, noop, noops,
        stmt: kind("stmt"), expr: kind("expr"), chain: kind("chain"), macro: kind("macro")
    };
})()
