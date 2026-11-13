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

// The Markdown helpers of the Autodoc templates (plans/archive/Autodoc.md, docs/dev/Autodoc.md). A template takes them with
//     <#assign md = ec.resource.script("component://framework/template/autodoc/AutodocLib.js")>
// Names and other data are text, escaped here; a description is Markdown its author wrote, used as it is.

const text = v => (v === null || v === undefined ? "" : String(v))

// plain text in Markdown: the characters that would start emphasis, code, a link or HTML escaped
const plain = v => text(v).replace(/([\\`*_[\]<>])/g, "\\$1")

// inline code that holds any run of backticks
const code = v => {
    const t = text(v)
    if (!t) return ""
    const longest = (t.match(/`+/g) || []).reduce((max, run) => Math.max(max, run.length), 0)
    const ticks = "`".repeat(longest + 1)
    const pad = t.startsWith("`") || t.endsWith("`") ? " " : ""
    return ticks + pad + t + pad + ticks
}

// a link to a doc when it has one, else the label as plain text
const link = (label, href) => (href ? "[" + plain(label) + "](" + href + ")" : plain(label))

// a link whose label is code
const codeLink = (label, href) => (href ? "[" + code(label) + "](" + href + ")" : code(label))

// one table cell: one line, its pipes escaped (also inside code, as GitHub tables want)
const cell = v =>
    text(v)
        .replace(/\r?\n+/g, " ")
        .replace(/(^|[^\\])\|/g, "$1\\|")
        .trim()

// a table of headers and rows (each a list of cells already in Markdown); nothing when there are no rows
const table = (headers, rows) => {
    if (!rows || !rows.length) return ""
    const line = cells => "| " + cells.map(cell).join(" | ") + " |"
    return [line(headers), "|" + headers.map(() => "---").join("|") + "|"].concat(rows.map(line)).join("\n") + "\n"
}

// a fenced block that holds any run of backticks
const fence = (lang, body) => {
    const t = text(body).replace(/\s+$/, "")
    if (!t) return ""
    const longest = (t.match(/`{3,}/g) || []).reduce((max, run) => Math.max(max, run.length), 0)
    const ticks = "`".repeat(Math.max(3, longest + 1))
    return ticks + (lang || "") + "\n" + t + "\n" + ticks + "\n"
}

const mermaid = diagram => fence("mermaid", diagram)

const yes = flag => (flag ? "yes" : "")

const join = (list, separator) =>
    (list || []).filter(item => item !== null && item !== undefined && item !== "").join(separator || ", ")

// the line under a doc's title: up to its package or tree, and to the root
const nav = (up, rootHref) => {
    const parts = []
    if (up && up.href) parts.push("Up: " + link(up.title, up.href))
    if (rootHref && (!up || up.href !== rootHref)) parts.push(link("All artifacts", rootHref))
    return parts.join(" · ")
}

// where an artifact is defined: its component, its location and line
const defined = (component, location, line) => {
    if (!location) return ""
    return code(location) + (line ? " line " + line : "") + (component ? " (" + plain(component) + ")" : "")
}

// what uses an artifact or what it uses: a table per group of names and how they refer
const usage = groups => {
    if (!groups || !groups.length) return "None.\n"
    return groups
        .map(group => {
            const rows = group.items.map(item => [
                link(item.title, item.href) + (item.resolved ? "" : " (not found)"),
                code(item.name === item.title ? "" : item.name),
                plain(item.via) +
                    (item.calls ? " · observed " + item.calls + "×" : "") +
                    (item.observed ? " · run time only" : "")
            ])
            return "**" + plain(group.label) + "** (" + group.count + ")\n\n" + table(["Name", "Full name", "How"], rows)
        })
        .join("\n")
}

;({text, plain, code, link, codeLink, cell, table, fence, mermaid, yes, join, nav, defined, usage})
