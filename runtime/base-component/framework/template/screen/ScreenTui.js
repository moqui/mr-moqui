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

// The tui render mode: one JSON array per section. A parent puts a nested section in a value
// (`"c":` and then the child array). Exit widgets are real nodes. Anything else is a structural
// node or a placeholder, still valid JSON. A form macro writes its fields; widget macros do not
// also write them.
(function (T, S) {
    "use strict";

    const { attr, has, jsString, child, childrenNamed, stmt } = T;

    // Field and parameter names never written, compared without case: CREDENTIAL_PARAMETER_NAMES in
    // crates/moqui-core/src/web/mod.rs. Field names are checked as the code is made, hidden parameter names as it runs.
    const SECRET = new Set(["password", "oldpassword", "newpassword", "newpasswordverify", "currentpassword", "resetpassword", "authpassword", "api_key", "login_key", "passwordsalt", "passwordhashtype", "passwordbase64", "passwordverify"]);
    const prelude = `
let $comma = false;
const $stack = [];
// While $captured runs, the writes go to its array rather than the output: a node a list cell holds as a value.
let $sink = null;
const $w = (text) => { if ($sink) $sink.push(text); else sri.write(text); };
const $sep = () => { if ($comma) $w(","); $comma = true; };
const $clean = (value) => {
    const text = String(value == null ? "" : value);
    let out = "";
    for (const ch of text) {
        const code = ch.codePointAt(0);
        if (code === 9 || code === 10 || (code >= 32 && code !== 127 && (code < 128 || code > 159))) out += ch;
    }
    return out;
};
const $scrub = (value) => {
    if (typeof value === "string") return JSON.stringify($clean(value));
    if (value === null || value === undefined) return "null";
    if (typeof value === "number" || typeof value === "boolean") return JSON.stringify(value);
    if (Array.isArray(value)) return "[" + value.map($scrub).join(",") + "]";
    if (typeof value === "object") {
        let out = "{";
        let first = true;
        for (const key of Object.keys(value)) {
            if (!first) out += ",";
            first = false;
            out += JSON.stringify($clean(key)) + ":" + $scrub(value[key]);
        }
        return out + "}";
    }
    return JSON.stringify($clean(String(value)));
};
const $emit = (value) => { $sep(); $w($scrub(value)); };
const $openRender = (fields) => {
    // A nested render writes to the output itself, so it cannot be captured.
    if ($sink) throw new Error("a section, include or subscreen in a list cell's dialog is not supported");
    $sep();
    $w($scrub(fields).slice(0, -1) + ',"c":');
};
const $closeRender = () => { $w("}"); };
const $openChildren = (fields) => {
    $sep();
    $w($scrub(fields).slice(0, -1) + ',"c":[');
    $stack.push($comma);
    $comma = false;
};
const $closeChildren = () => {
    $w("]");
    $comma = $stack.pop();
    $w("}");
};
// The nodes the statements of body write, as values: a container-dialog of a list row. A failure is an error node in
// their place, and the document stays whole.
const $captured = (body) => {
    const $was = {sink: $sink, comma: $comma, depth: $stack.length};
    $sink = [];
    $comma = false;
    let $nodes;
    try {
        $w("[");
        body();
        $w("]");
        $nodes = JSON.parse($sink.join(""));
    } catch ($err) {
        $nodes = [{t: "error", text: "A dialog of a list row did not render: " + String($err && $err.message || $err)}];
    } finally {
        $sink = $was.sink;
        $comma = $was.comma;
        $stack.length = $was.depth;
    }
    return $nodes;
};
const $whole = (value) => {
    const n = Number(value);
    return Number.isFinite(n) && n >= 0 ? Math.trunc(n) : 0;
};
const $uid = (id) => sri.uniqueId(String(id));
// The section entry index a qvt form id ends with inside a section-iterate (ScreenQvt.js sec()), so a form's domId is the
// id qvt gives the same form.
const $sec = () => {
    const $index = ec.context.get("sectionEntryIndex");
    return $index === null || $index === undefined || $index === "" ? "" : "_" + $index;
};
// A url that runs a transition which is not read-only (the read-only attribute qvt writes, ScreenQvt.js): an agent's
// press of it waits for the user's approval; the server's gate decides.
const $writes = ($made) => !!$made && $made.isTransition() && !$made.isReadOnly();
const $secret = new Set(${JSON.stringify([...SECRET])});
// A map of strings: null values out, and empty ones too unless kept.
const $strMap = ($map, $keepEmpty) => {
    const $out = {};
    if (!$map) return $out;
    for (const $key of Object.keys($map)) {
        const $value = $map[$key];
        if ($value == null || (!$keepEmpty && $value === "")) continue;
        $out[String($key)] = String($value);
    }
    return $out;
};
// As qvt's remote(): the URL without its query, its fixed parameters less the depends-on ones, and {parameter: field}.
const $remote = ($made, $depends) => {
    const $params = {};
    const $map = $made ? ($made.getParameterMap() || {}) : {};
    for (const $key of Object.keys($map)) {
        const $fixed = $map[$key];
        if ($depends[$key] === undefined && $fixed != null && $fixed !== "") $params[$key] = String($fixed);
    }
    return {url: $made ? ($made.url || "") : "", params: $params, dependsOn: $depends};
};
const $withRemote = ($w, $r, $urlKey, $paramsKey) => {
    if ($r.url) $w[$urlKey] = $r.url;
    if (Object.keys($r.dependsOn).length) $w.dependsOn = $r.dependsOn;
    if (Object.keys($r.params).length) $w[$paramsKey] = $r.params;
    return $w;
};
// A list cell: its parts, a field's texts joined on one line and a node a part of its own; one part is the cell itself.
const $addPart = ($parts, $start, $piece) => {
    if ($piece == null) return;
    if (typeof $piece === "object") {
        $parts.push($piece);
        return;
    }
    const $text = String($piece);
    if (!$text) return;
    if ($parts.length > $start && typeof $parts[$parts.length - 1] === "string") $parts[$parts.length - 1] += " " + $text;
    else $parts.push($text);
};
const $cellOf = ($parts) => ($parts.length === 0 ? "" : $parts.length === 1 ? $parts[0] : $parts);
const $range = ($from, $thru) => ($from && $thru ? $from + " – " + $thru : $from ? $from + " –" : $thru ? "– " + $thru : "");
// A list of ids: an array, a list, or a comma list.
const $ids = ($value) => {
    if ($value == null) return [];
    const $list = typeof $value === "string" ? $value.split(",") : Array.from($value);
    return $list.map(($id) => ($id == null ? "" : String($id).trim())).filter(($id) => $id && $id !== "null");
};
// The current screen's parameters a saved find or a column choice keeps.
const $findParameters = () => {
    const $url = sri.getScreenUrlInstance().cloneUrlInstance();
    for (const $name of ["pageIndex", "moquiFormName", "moquiSessionToken", "lastStandalone", "formListFindId", "renderMode"]) $url.removeParameter($name);
    return $strMap($url.getParameterMap(), true);
};
// The screen as a PDF under /fop, Java's MoquiFopServlet, with every row.
const $fopUrl = () => "/fop" + sri.getScreenUrlInstance().cloneUrlInstance().removeParameter("renderMode")
    .removeParameter("lastStandalone").addParameter("pageNoLimit", "true").pathWithParams;
const $screenUrlWith = ($pairs, $remove) => {
    const $url = sri.getScreenUrlInstance().cloneUrlInstance();
    if ($remove) $url.removeParameter($remove);
    for (const $pair of $pairs) $url.addParameter($pair[0], $pair[1]);
    return $url.getUrlWithParams();
};
$w("[");
`;
    const epilogue = `
$w("]");
`;

    const typeNames = (type) => (type || "").split(",").map((name) => name.trim()).filter((name) => name);
    const exprOf = (text) => (text && String(text).includes("${") ? S.expandExpr(text) : jsString(text || ""));
    const exprNoL10n = (text) => (text && String(text).includes("${") ? S.expandNoL10nExpr(text) : jsString(text || ""));
    // A title as the other modes show it: expanded and localized.
    const titleOf = (text) => (text ? S.expandExpr(text) : '""');
    const guarded = (node, code) => {
        const condition = attr(node, "condition");
        if (!condition || !code) return code;
        return `if (${condition}) {\n${code}}\n`;
    };

    const elementPath = (node) => {
        const parts = [];
        let current = node;
        while (current && current.name !== "screen") {
            const siblings = current.parent ? current.parent.children.filter((kid) => kid.name === current.name) : [current];
            const label = attr(current, "id") || attr(current, "name") || current.name;
            const index = siblings.indexOf(current);
            const numbered = siblings.length > 1 && !attr(current, "id") && !attr(current, "name");
            parts.push(numbered ? label + "[" + index + "]" : label);
            current = current.parent;
        }
        return parts.reverse().join("/");
    };
    const idOf = (node) => attr(node, "id") || attr(node, "name") || elementPath(node);
    const formNodeOf = (node) => {
        let current = node;
        while (current) {
            if (current.name === "form-single" || current.name === "form-list") return current;
            current = current.parent;
        }
        return null;
    };
    const fieldNodeOf = (node) => {
        let current = node;
        while (current) {
            if (current.name === "field") return current;
            current = current.parent;
        }
        return null;
    };
    const formNameOf = (node) => attr(node, "name") || (node.name === "form-list" ? "list" : "form");
    // The id qvt gives a form: its name, expanded when it holds ${}, and the section entry index (ScreenQvt.js formOpen and
    // formList).
    const domIdExpr = (node) => `(String(${exprNoL10n(attr(node, "name"))} ?? "") + $sec())`;
    const fieldIdOf = (field) => {
        if (attr(field, "id")) return attr(field, "id");
        const form = field.parent;
        const formName = form ? formNameOf(form) : "form";
        return formName + "." + (attr(field, "name") || "field");
    };
    const inHeader = (node) => {
        let current = node;
        while (current) {
            if (current.name === "header-field") return true;
            if (current.name === "form-single" || current.name === "form-list") return false;
            current = current.parent;
        }
        return false;
    };
    const maxlengthOf = (sub, widget) => {
        const text = attr(widget, "maxlength") || attr(sub, "maxlength") || "";
        return /^\d+$/.test(text) ? text : "0";
    };
    const fallbackKey = (widget) => {
        if (!has(widget, "no-current-selected-key")) return '""';
        if (widget.name === "check") return S.expandNoL10nExpr(attr(widget, "no-current-selected-key"));
        // an expression (a widget template's `${noCurrentSelectedKey || ''}`) is expanded, as the other renderers do
        return exprNoL10n(attr(widget, "no-current-selected-key"));
    };
    const screenUrlExpr = `(() => { const $info = sri.getScreenUrlInfo(); const $names = $info && $info.fullPathNameList; if ($names && $names.length) return "/" + $names.join("/"); const $screen = sri.getActiveScreenDef(); return ($screen && $screen.location) || ""; })()`;

    const linkIdExpr = (node, inList) => {
        if (attr(node, "id")) return jsString(attr(node, "id"));
        const field = fieldNodeOf(node);
        if (field) {
            const base = fieldIdOf(field);
            if (inList) return jsString(base + ".") + " + $rowKey";
            return jsString(base);
        }
        return jsString(idOf(node));
    };
    // A link to a transition that runs actions posts, as qvt's m-form and Java's hidden form do: the server refuses its
    // parameters on a GET url.
    const linkExpr = (node, inList) => {
        const kind = attr(node, "link-type") || "";
        const confirm = attr(node, "confirmation") || "";
        return `(() => {
            let $text = ${S.linkTextExpr(node)};
            if ($text == null || $text === "null") $text = "";
            const $made = sri.makeUrlByType(${jsString(attr(node, "url") || "")}, ${jsString(attr(node, "url-type") || "transition")}, ${S.nodeLit(node, true)}, ${jsString(attr(node, "expand-transition-url") || "true")});
            const $link = {t:"link",id:$uid(${linkIdExpr(node, inList)}),text:String($text),url:$made ? ($made.getUrlWithParams() || "") : ""};
            if ($made && $made.getDisableLink()) $link.disabled = true;
            if ($writes($made)) $link.writes = true;
            ${kind ? `$link.kind = ${jsString(kind)};` : ""}
            ${attr(node, "btn-type") ? `$link.btnType = ${jsString(attr(node, "btn-type"))};` : ""}
            ${attr(node, "icon") ? `$link.icon = ${jsString(attr(node, "icon"))};` : ""}
            ${attr(node, "badge") ? `$link.badge = String(${S.expandNoL10nExpr(attr(node, "badge"))} || "");` : ""}
            ${attr(node, "tooltip") ? `$link.tooltip = String(${S.expandExpr(attr(node, "tooltip"))} || "");` : ""}
            ${kind === "hidden-form-link" ? `$link.method = "post";` : ""}
            if ($made && !$link.method && $made.isTransition() && !sri.isAnchorLink(${S.nodeLit(node, true)}, $made)
                && $made.getTransitionMethod() !== "get") $link.method = "post";
            ${confirm ? `$link.confirm = String(${titleOf(confirm)} || "");` : ""}
            return $link;
        })()`;
    };
    const linkStatement = (node, inList) => guarded(node, `$emit(${linkExpr(node, inList)});\n`);

    const valueCall = (widget) => `sri.getFieldValueString(${S.nodeLit(widget, false)}) || ${fallbackKey(widget)}`;
    const inlineOptions = (widget, kind) => `(() => {
        const $value = String(${valueCall(widget)} || "");
        const $options = [];
        const $map = sri.getFieldOptions(${S.nodeLit(widget, true)});
        if ($map && $map.forEach) $map.forEach(($label, $key) => $options.push({value:String($key),label:String($label == null ? "" : $label)}));
        if ($value && !$options.some(($opt) => $opt.value === $value)) $options.push({value:$value,label:$value});
        return {kind:${jsString(kind)},value:$value,options:$options};
    })()`;
    // As qvt's remote(): the URL without its query, its fixed parameters, and {parameter: field} for depends-on.
    const remoteExpr = (holder, transition) => {
        const depends = {};
        childrenNamed(holder, "depends-on").forEach((dep) => {
            const field = attr(dep, "field");
            if (field) depends[attr(dep, "parameter") || field] = field;
        });
        return `$remote(sri.makeUrlByType(${jsString(transition || "")}, "transition", ${S.nodeLit(holder, true)}, "false"), ${JSON.stringify(depends)})`;
    };
    const digits = (text) => (/^\d+$/.test(text || "") ? String(Number(text)) : "");
    const dynamicOptions = (widget) => {
        const dynamic = child(widget, "dynamic-options");
        const labelField = attr(dynamic, "label-field") || "label";
        const extra = [];
        if (attr(dynamic, "server-search") === "true") extra.push("$w.serverSearch = true;");
        if (attr(dynamic, "depends-optional") === "true") extra.push("$w.dependsOptional = true;");
        if (digits(attr(dynamic, "min-length"))) extra.push(`$w.minLength = ${digits(attr(dynamic, "min-length"))};`);
        return `(() => {
            const $value = String(${valueCall(widget)} || "");
            let $label = $value;
            if ($value) {
                const $found = sri.getFieldTransitionValue(${jsString(attr(dynamic, "transition") || "")}, ${S.nodeLit(dynamic, true)}, $value, ${jsString(labelField)}, false);
                if ($found) $label = String($found);
            }
            const $w = $withRemote({kind:"dropDown",value:$value,options:$value ? [{value:$value,label:$label}] : []}, ${remoteExpr(dynamic, attr(dynamic, "transition"))}, "optionsUrl", "optionsParameters");
            ${extra.map((line) => line + "\n            ").join("")}return $w;
        })()`;
    };
    // A text-line with ac-transition is a drop-down that searches the transition, as qvt writes it.
    const acDropDown = (widget) => {
        const transition = attr(widget, "ac-transition");
        const initial = attr(widget, "ac-initial-text");
        const extra = [];
        if (attr(widget, "ac-use-actual") === "true") extra.push("$w.combo = true;");
        if (digits(attr(widget, "ac-min-length"))) extra.push(`$w.minLength = ${digits(attr(widget, "ac-min-length"))};`);
        if (attr(widget, "depends-optional") === "true") extra.push("$w.dependsOptional = true;");
        const found = initial
            ? `ec.resource.expand(${jsString(initial)}, "")`
            : `sri.getFieldTransitionValue(${jsString(transition)}, ${S.nodeLit(widget, true)}, $value, "label", false)`;
        return `(() => {
            const $value = String(${valueCall(widget)} || "");
            let $label = $value;
            if ($value) {
                const $found = ${found};
                if ($found != null && $found !== "" && $found !== "null") $label = String($found);
            }
            const $w = $withRemote({kind:"dropDown",value:$value,options:$value ? [{value:$value,label:$label}] : [],serverSearch:true,allowEmpty:true}, ${remoteExpr(widget, transition)}, "optionsUrl", "optionsParameters");
            ${extra.map((line) => line + "\n            ").join("")}return $w;
        })()`;
    };
    // Click to edit (qvt's m-editable): nothing for an empty text unless display-if-empty.
    const editableExpr = (widget) => {
        const widgetType = attr(widget, "widget-type") || "textarea";
        const ifEmpty = attr(widget, "display-if-empty");
        const ifEmptyExpr = ifEmpty.includes("${") ? `String(${S.expandNoL10nExpr(ifEmpty)}) === "true"` : String(ifEmpty === "true");
        const value = has(widget, "value")
            ? `const $value = ${exprNoL10n(attr(widget, "value"))};\n            if ($value != null && $value !== "null") $w.value = String($value);\n            `
            : "";
        const options = widgetType === "select"
            ? `const $map = sri.getFieldOptions(${S.nodeLit(widget, true)});
            const $options = [];
            if ($map && $map.forEach) $map.forEach(($label, $key) => $options.push({value:String($key),label:String($label == null ? "" : $label)}));
            if ($options.length) $w.options = $options;
            `
            : "";
        return `(() => {
            let $text = ec.resource.expand(${jsString(attr(widget, "text"))}, "");
            $text = $text == null || $text === "null" ? "" : String($text);
            if (!$text.trim() && !(${ifEmptyExpr})) return null;
            const $made = sri.makeUrlByType(${jsString(attr(widget, "transition"))}, "transition", ${S.nodeLit(widget, true)}, "true");
            const $w = {kind:"editable",text:$text,url:$made ? ($made.url || "") : "",parameterName:${jsString(attr(widget, "parameter-name") || "value")},widgetType:${jsString(widgetType)}};
            const $params = $strMap($made ? $made.getParameterMap() : null, true);
            if (Object.keys($params).length) $w.parameters = $params;
            ${value}${options}return $w;
        })()`;
    };
    // A find value from the request: `name` plus a suffix (`_from`, `_op`), else the default, as qvt's fields-initial.
    const findValue = (field, suffix, fallback) =>
        `(() => { const $v = ec.context.get(${jsString((attr(field, "name") || "") + suffix)}); return $v == null || $v === "" || $v === "null" ? String(${fallback || '""'}) : String($v); })()`;
    const dateDefault = (widget) => (attr(widget, "type") === "time" ? "HH:mm" : attr(widget, "type") === "date" ? "yyyy-MM-dd" : "yyyy-MM-dd HH:mm");
    const flag = (widget, name) => attr(widget, name) === "true";
    const widgetExpr = (widget) => {
        const field = fieldNodeOf(widget);
        const secret = field && SECRET.has((attr(field, "name") || "").toLowerCase());
        if (widget.name === "password") return '{kind:"password"}';
        if (secret) {
            if (widget.name === "display" || widget.name === "display-entity") return '{kind:"display",text:""}';
            const kind = widget.name === "text-find" ? "textFind" : widget.name === "hidden" ? "hidden" : "textLine";
            return `{kind:${jsString(kind)},value:""}`;
        }
        if (widget.name === "text-line" || widget.name === "lookup") {
            if (attr(widget, "ac-transition")) return acDropDown(widget);
            const extra = [
                attr(widget, "input-type") ? `inputType:${jsString(attr(widget, "input-type"))}` : "",
                attr(widget, "mask") ? `mask:${jsString(attr(widget, "mask"))}` : "",
                /^\d+$/.test(attr(widget, "size")) ? `size:${attr(widget, "size")}` : ""
            ].filter(Boolean).map((part) => "," + part).join("");
            const base = `{kind:"textLine",value:String(${valueCall(widget)} || "")${extra}}`;
            const steps = [];
            if (attr(widget, "prefix")) steps.push(`$w.prefix = String(${S.expandExpr(attr(widget, "prefix"))} || "");`);
            // disabled is a condition, as qvt's cond(); written as a ${} text it is that text being "true"
            const disabled = attr(widget, "disabled");
            if (disabled) steps.push(`if (${disabled.includes("${") ? `String(${S.expandNoL10nExpr(disabled)}) === "true"` : `(${disabled})`}) $w.disabled = true;`);
            if (attr(widget, "default-transition")) {
                steps.push(`$withRemote($w, ${remoteExpr(widget, attr(widget, "default-transition"))}, "defaultUrl", "defaultParameters");`);
            }
            if (!steps.length) return base;
            return `(() => { const $w = ${base}; ${steps.join(" ")} return $w; })()`;
        }
        if (widget.name === "text-area") {
            // D12: rich text is shown, never posted, so the stored HTML is not overwritten from the terminal.
            const editor = attr(widget, "editor-type");
            const readOnly = flag(widget, "read-only") || ["html", "rich", "rich-light"].includes(editor);
            const rows = /^\d+$/.test(attr(widget, "rows")) ? attr(widget, "rows") : "0";
            return `{kind:"textArea",value:String(${valueCall(widget)} || ""),rows:${rows},editorType:${jsString(editor)}${readOnly ? ",readOnly:true" : ""}}`;
        }
        if (widget.name === "text-find") {
            const ignoreCase = !attr(widget, "ignore-case") || flag(widget, "ignore-case");
            return `{kind:"textFind",value:String(${valueCall(widget)} || ""),op:${findValue(field, "_op", jsString(attr(widget, "default-operator") || "contains"))},not:${findValue(field, "_not", '"N"')} === "Y",ic:${findValue(field, "_ic", jsString(ignoreCase ? "Y" : "N"))} === "Y"${flag(widget, "hide-options") ? ",hideOptions:true" : ""}}`;
        }
        if (widget.name === "drop-down") {
            const base = child(widget, "dynamic-options") ? dynamicOptions(widget) : inlineOptions(widget, "dropDown");
            const flags = [
                ["allow-multiple", "multiple"], ["combo-box", "combo"], ["submit-on-select", "submitOnSelect"], ["show-not", "showNot"]
            ].filter(([name]) => attr(widget, name) === "true").map(([, key]) => `$w.${key} = true;`);
            if (attr(widget, "allow-empty") === "true") flags.push("$w.allowEmpty = true;");
            if (attr(widget, "show-not") === "true") flags.push(`$w.not = ${findValue(field, "_not", '"N"')} === "Y";`);
            if (!flags.length) return base;
            return `(() => { const $w = ${base}; ${flags.join(" ")} return $w; })()`;
        }
        if (widget.name === "radio") return inlineOptions(widget, "radio");
        if (widget.name === "check") return inlineOptions(widget, "check");
        if (widget.name === "date-time") {
            const type = attr(widget, "type") || "date-time";
            return `{kind:"dateTime",value:String(sri.getFieldValueString(${S.fieldLit(widget)}, ${jsString(attr(widget, "default-value"))}, ${jsString(S.dateTimeFormat(widget))}, ${jsString(type)}) || ""),type:${jsString(type)},format:${jsString(S.dateTimeFormat(widget))}}`;
        }
        if (widget.name === "date-find") {
            const format = dateDefault(widget);
            const formatted = (suffix, fallback) => `(() => { const $raw = ${findValue(field, suffix, jsString(fallback))}; if (!$raw) return ""; try { return String(ec.l10n.format($raw, ${jsString(format)})); } catch ($err) { return $raw; } })()`;
            return `{kind:"dateFind",type:${jsString(attr(widget, "type") || "timestamp")},format:${jsString(format)},from:${formatted("_from", attr(widget, "default-value-from"))},thru:${formatted("_thru", attr(widget, "default-value-thru"))}}`;
        }
        if (widget.name === "date-period") {
            return `{kind:"datePeriod",period:${findValue(field, "_period")}.toLowerCase(),poffset:${findValue(field, "_poffset")},pdate:${findValue(field, "_pdate")},from:${findValue(field, "_from")},thru:${findValue(field, "_thru")},allowEmpty:${attr(widget, "allow-empty") !== "false"},time:${flag(widget, "time")}}`;
        }
        if (widget.name === "range-find") {
            return `{kind:"rangeFind",from:${findValue(field, "_from", jsString(attr(widget, "default-value-from")))},thru:${findValue(field, "_thru", jsString(attr(widget, "default-value-thru")))}}`;
        }
        if (widget.name === "file") return `{kind:"file"${flag(widget, "multiple") ? ",multiple:true" : ""}${attr(widget, "accept") ? `,accept:${jsString(attr(widget, "accept"))}` : ""}}`;
        if (widget.name === "display" || widget.name === "display-entity") {
            const text = widget.name === "display" ? S.displayValueExpr(widget) : `sri.getFieldEntityValue(${S.nodeLit(widget, false)})`;
            // also-hidden (the default) posts the plain value with the form, as qvt's display does.
            const alsoHidden = !attr(widget, "also-hidden") || flag(widget, "also-hidden");
            const base = `{kind:"display",text:String(${text} || "")${alsoHidden ? `,value:String(sri.getFieldValuePlainString(${S.fieldLit(widget)}, "") || "")` : ""}}`;
            // dynamic-transition: the text and value asked for again when a depends-on field changes, as qvt's m-display.
            if (!attr(widget, "dynamic-transition")) return base;
            return `$withRemote(${base}, ${remoteExpr(widget, attr(widget, "dynamic-transition"))}, "valueUrl", "valueParameters")`;
        }
        if (widget.name === "editable") return editableExpr(widget);
        if (widget.name === "hidden") return `{kind:"hidden",value:String(${valueCall(widget)} || "")}`;
        if (widget.name === "submit" || widget.name === "reset") {
            return `{kind:${jsString(widget.name)},text:String(${S.localizedTitle(widget.parent)} || "")${widget.name === "submit" && attr(widget, "confirmation") ? `,confirm:String(${titleOf(attr(widget, "confirmation"))} || "")` : ""}}`;
        }
        if (widget.name === "label") {
            return `{kind:"label",text:String(${S.expandExpr(attr(widget, "text") || "")} || ""),style:${jsString(attr(widget, "type") || "span")}}`;
        }
        if (widget.name === "image") {
            return `{kind:"image",alt:String(${S.expandExpr(attr(widget, "alt") || "")} || ""),url:String(${S.expandNoL10nExpr(attr(widget, "url") || "")} || "")}`;
        }
        return "";
    };
    // A field node; an editable with nothing to show is null. opts gives a list row's id and name: idExpr, nameExpr, and
    // suffixExpr for the visible-when field of a multi row.
    const fieldObject = (field, sub, widget, opts) => {
        const w = widgetExpr(widget);
        if (!w) return "";
        const o = opts || {};
        const when = child(sub, "visible-when");
        const required = attr(field, "required") === "true" || attr(sub, "required") === "true";
        const nullable = widget.name === "editable";
        const idExpr = o.idExpr || `$uid(${jsString(fieldIdOf(field))})`;
        const nameExpr = o.nameExpr || jsString(attr(field, "name") || "field");
        const whenField = when ? jsString(attr(when, "field")) + (o.suffixExpr ? " + " + o.suffixExpr : "") : "";
        return `(() => {
            ${nullable ? `const $w = ${w};\n            if ($w == null) return null;\n            ` : ""}let $required = ${required ? "true" : "false"};
            const $max = ${maxlengthOf(sub, widget)};
            const $hint = sri.getFieldValidation(${S.nodeLit(sub, false)});
            if ($hint && String($hint.classes || "").split(/\\s+/).indexOf("required") >= 0) $required = true;
            const $rules = [];
            const $found = sri.getFieldValidationRules(${S.nodeLit(sub, false)}) || [];
            for (let $ri = 0; $ri < $found.length; $ri++) {
                if ($found[$ri].kind === "required") $required = true;
                else $rules.push($found[$ri]);
            }
            const $field = {t:"field",id:${idExpr},name:${nameExpr},title:String(${S.localizedTitle(sub)} || ""),w:${nullable ? "$w" : w}};
            if ($required) $field.required = true;
            if ($max > 0) $field.maxlength = $max;
            if ($rules.length) $field.rules = $rules;
            ${attr(sub, "tooltip") ? `$field.tooltip = String(${S.expandExpr(attr(sub, "tooltip"))} || "");` : ""}
            ${when ? `$field.visibleWhen = {field:${whenField},value:String(${exprOf(attr(when, "value"))} || "")};` : ""}
            return $field;
        })()`;
    };
    // Children of a field sub-node that are not drawn: set, ignored, and the visible-when rule the field carries.
    const notWidget = (widget) => widget.name === "set" || widget.name === "ignored" || widget.name === "visible-when";
    const placeholderEmit = (id, text, url) =>
        `$emit({t:"placeholder",id:$uid(${jsString(id)}),text:${jsString(text)}${url ? `,url:${url}` : ""}});\n`;
    const emitWidget = (field, sub, widget) => {
        if (widget.name === "link") return linkStatement(widget, false);
        const expr = fieldObject(field, sub, widget);
        if (!expr) return placeholderEmit(fieldIdOf(field) + "." + widget.name, widget.name);
        if (widget.name === "editable") return `{\n    const $node = ${expr};\n    if ($node) $emit($node);\n}\n`;
        return `$emit(${expr});\n`;
    };
    const emitSub = (field, sub) => {
        let code = S.setCode(sub);
        sub.children.forEach((widget) => {
            if (notWidget(widget)) return;
            code += emitWidget(field, sub, widget);
        });
        return code;
    };
    const emitField = (field) => {
        if (!field || attr(field, "hide") === "true") return "";
        return S.fieldChain(field, (sub) => emitSub(field, sub));
    };
    const layoutCode = (form) => {
        const fields = childrenNamed(form, "field");
        const layout = child(form, "field-layout");
        if (!layout) return fields.map(emitField).join("");
        const byName = {};
        fields.forEach((field) => { byName[attr(field, "name")] = field; });
        const used = new Set();
        let seq = 0;
        const walk = (node) => {
            let code = "";
            node.children.forEach((kid) => {
                if (kid.name === "field-ref") {
                    const name = attr(kid, "name");
                    used.add(name);
                    code += emitField(byName[name]);
                } else if (kid.name === "fields-not-referenced") {
                    fields.forEach((field) => {
                        const name = attr(field, "name");
                        if (!used.has(name)) {
                            used.add(name);
                            code += emitField(field);
                        }
                    });
                } else if (kid.name === "field-row" || kid.name === "field-row-big" || kid.name === "field-col-row") {
                    const inner = walk(kid);
                    code += `$openChildren({t:"row",id:$uid(${jsString(formNameOf(form) + ".l" + seq++)})});\n${inner}$closeChildren();\n`;
                } else if (kid.name === "field-col") {
                    const inner = walk(kid);
                    code += `$openChildren({t:"col",id:$uid(${jsString(formNameOf(form) + ".l" + seq++)}),xs:${jsString(attr(kid, "xs"))},sm:${jsString(attr(kid, "sm"))},md:${jsString(attr(kid, "md"))},lg:${jsString(attr(kid, "lg"))}});\n${inner}$closeChildren();\n`;
                } else if (kid.name === "field-group" || kid.name === "field-accordion") {
                    const inner = walk(kid);
                    code += `$openChildren({t:"box",id:$uid(${jsString(formNameOf(form) + ".l" + seq++)}),title:${titleOf(attr(kid, "title"))},type:${jsString(kid.name)}});\n${inner}$closeChildren();\n`;
                }
            });
            return code;
        };
        return walk(layout);
    };

    const cellText = (widget) => {
        if (widget.name === "password") return '""';
        const field = fieldNodeOf(widget);
        if (field && SECRET.has((attr(field, "name") || "").toLowerCase())) return '""';
        if (widget.name === "link") {
            const expr = linkExpr(widget, true);
            const condition = attr(widget, "condition");
            return condition ? `(() => { if (!(${condition})) return ""; return ${expr}; })()` : expr;
        }
        if (widget.name === "hidden" || widget.name === "ignored") return '""';
        if (widget.name === "drop-down" && child(widget, "dynamic-options")) {
            const dynamic = child(widget, "dynamic-options");
            return `(() => {
                const $value = String(${valueCall(widget)} || "");
                if (!$value) return "";
                const $found = sri.getFieldTransitionValue(${jsString(attr(dynamic, "transition") || "")}, ${S.nodeLit(dynamic, true)}, $value, ${jsString(attr(dynamic, "label-field") || "label")}, false);
                return $found ? String($found) : $value;
            })()`;
        }
        if (widget.name === "drop-down" || widget.name === "radio" || widget.name === "check") {
            return `(() => {
                const $value = String(${valueCall(widget)} || "");
                const $map = sri.getFieldOptions(${S.nodeLit(widget, true)});
                const $label = $map && $map.get ? $map.get($value) : null;
                return $label != null ? String($label) : $value;
            })()`;
        }
        if (widget.name === "date-time") {
            return `String(sri.getFieldValueString(${S.fieldLit(widget)}, ${jsString(attr(widget, "default-value"))}, ${jsString(S.dateTimeFormat(widget))}, ${jsString(attr(widget, "type") || "date-time")}) || "")`;
        }
        if (widget.name === "display") return `String(${S.displayValueExpr(widget)} || "")`;
        if (widget.name === "display-entity") return `String(sri.getFieldEntityValue(${S.nodeLit(widget, false)}) || "")`;
        if (widget.name === "text-line" && attr(widget, "ac-transition")) {
            return `(() => {
                const $value = String(${valueCall(widget)} || "");
                if (!$value) return "";
                const $found = sri.getFieldTransitionValue(${jsString(attr(widget, "ac-transition"))}, ${S.nodeLit(widget, true)}, $value, "label", false);
                return $found ? String($found) : $value;
            })()`;
        }
        if (["text-line", "lookup", "text-area", "text-find"].includes(widget.name)) {
            return `String(${valueCall(widget)} || "")`;
        }
        if (widget.name === "submit" || widget.name === "reset") return `String(${S.localizedTitle(widget.parent)} || "")`;
        if (widget.name === "date-find") {
            const format = dateDefault(widget);
            const formatted = (suffix) => `(() => { const $raw = ${findValue(field, suffix)}; if (!$raw) return ""; try { return String(ec.l10n.format($raw, ${jsString(format)})); } catch ($err) { return $raw; } })()`;
            return `$range(${formatted("_from")}, ${formatted("_thru")})`;
        }
        if (widget.name === "date-period" || widget.name === "range-find") return `$range(${findValue(field, "_from")}, ${findValue(field, "_thru")})`;
        if (widget.name === "label") return `String(${S.expandExpr(attr(widget, "text") || "")} || "")`;
        if (widget.name === "image") return `String(${S.expandExpr(attr(widget, "alt") || "")} || "")`;
        // file, and anything else: no text (never the element's name)
        return '""';
    };
    // The widgets of a row form: an input posts with its row, as qvt's row m-form or the multi m-form.
    const ROW_INPUTS = ["text-line", "lookup", "text-area", "drop-down", "check", "radio", "date-time", "text-find", "password"];
    const rowFieldOpts = (field, multi) => {
        const name = attr(field, "name") || "field";
        return {
            idExpr: `$uid(${jsString(fieldIdOf(field) + ".")} + $rowKey)`,
            nameExpr: multi ? `${jsString(name + "_")} + $index` : jsString(name),
            suffixExpr: multi ? `"_" + $index` : ""
        };
    };
    // One widget of a list cell: a text, a link, or a field node (an editable; an input or a button of a row form).
    const cellPiece = (field, sub, widget, list) => {
        if (widget.name === "editable") return fieldObject(field, sub, widget, rowFieldOpts(field, false)) || '""';
        // A container-dialog of a row (an Edit per row): the dialog node its statements write, with the row's values,
        // its id made the row's; the client offers it on F10 and Enter.
        if (widget.name === "container-dialog") {
            if (!widget.source) return '""';
            return `(() => {
                const $dialog = $captured(() => {\n${widget.source}})[0];
                if (!$dialog) return "";
                if ($dialog.t === "dialog") $dialog.id = $uid(${jsString(idOf(widget) + ".")} + $rowKey);
                return $dialog;
            })()`;
        }
        // A dynamic-dialog of a row (a View per row): its url with the row's parameters; the client offers it on F10.
        if (widget.name === "dynamic-dialog") {
            const condition = attr(widget, "condition");
            return `(() => {
                ${condition ? `if (!(${condition})) return "";` : ""}
                const $made = sri.makeUrlByType(${jsString(attr(widget, "transition") || "")}, "transition", ${S.nodeLit(widget, true)}, "true");
                const $button = String(${S.expandExpr(attr(widget, "button-text") || "")} || "");
                return {t:"dynamicDialog",id:$uid(${jsString(idOf(widget) + ".")} + $rowKey),url:$made ? ($made.getUrlWithParams() || "") : "",button:$button,title:${attr(widget, "title") ? `String(${S.expandExpr(attr(widget, "title"))} || "") || $button` : "$button"}};
            })()`;
        }
        if (list.hasForm) {
            if (ROW_INPUTS.includes(widget.name)) {
                return `($listForm ? ${fieldObject(field, sub, widget, rowFieldOpts(field, list.multi))} : ${cellText(widget)})`;
            }
            if (widget.name === "submit" || widget.name === "reset") {
                // a multi form has its buttons once, in the list's buttons
                if (list.multi) return `($listForm ? "" : ${cellText(widget)})`;
                return `($listForm ? ${fieldObject(field, sub, widget, rowFieldOpts(field, false))} : ${cellText(widget)})`;
            }
        }
        return cellText(widget);
    };
    const columnCells = (columns, list) => {
        let code = `let $parts = [];\nlet $start = 0;\n`;
        columns.forEach((column) => {
            code += `$parts = [];\n`;
            column.forEach((field) => {
                if (attr(field, "hide") === "true") return;
                const body = S.fieldChain(field, (sub) => {
                    let inner = S.setCode(sub);
                    sub.children.forEach((widget) => {
                        if (notWidget(widget)) return;
                        inner += `$addPart($parts, $start, ${cellPiece(field, sub, widget, list)});\n`;
                    });
                    return inner;
                });
                if (body) code += S.hideGuard(field, `$start = $parts.length;\n${body}`);
            });
            code += `$cells.push($cellOf($parts));\n`;
        });
        return code;
    };
    // The values a row form posts that it does not show: its hidden fields, and the also-hidden values of its displays.
    const rowSubs = (field) => field.children.filter((sub) => sub.name === "default-field" || sub.name === "conditional-field");
    const postsHidden = (widget) => widget.name === "hidden" ||
        ((widget.name === "display" || widget.name === "display-entity") && (!attr(widget, "also-hidden") || flag(widget, "also-hidden")));
    const isButton = (widget) => widget.name === "submit" || widget.name === "reset";
    const rowHiddenCode = (fields, multi) => fields.map((field) => {
        const name = attr(field, "name");
        if (!name || SECRET.has(name.toLowerCase())) return "";
        if (!rowSubs(field).some((sub) => sub.children.some(postsHidden))) return "";
        const nameExpr = multi ? `${jsString(name + "_")} + $index` : jsString(name);
        return S.fieldChain(field, (sub) => {
            const kept = sub.children.filter(postsHidden);
            if (!kept.length) return "";
            return S.setCode(sub) + kept.map((widget) =>
                `$rowHidden.push({name:${nameExpr},value:String(sri.getFieldValuePlainString(${S.fieldLit(widget)}, ${jsString(widget.name === "hidden" ? attr(widget, "default-value") : "")}) || "")});\n`).join("");
        });
    }).join("");
    // The submit and reset buttons of a multi form, once for the list.
    const listButtonsCode = (fields) => fields.map((field) => {
        if (attr(field, "hide") === "true") return "";
        if (!rowSubs(field).some((sub) => sub.children.some(isButton))) return "";
        const code = S.fieldChain(field, (sub) => {
            const buttons = sub.children.filter(isButton);
            if (!buttons.length) return "";
            return S.setCode(sub) + buttons.map((widget) => `$buttons.push(${fieldObject(field, sub, widget)});\n`).join("");
        });
        return S.hideGuard(field, code);
    }).join("");
    const headerFields = (form) => {
        let code = "";
        childrenNamed(form, "field").forEach((field) => {
            const sub = child(field, "header-field");
            if (!sub || attr(field, "hide") === "true") return;
            code += S.setCode(sub);
            sub.children.forEach((widget) => {
                if (notWidget(widget)) return;
                if (widget.name === "link") {
                    code += `$headerFields.push(${linkExpr(widget, false)});\n`;
                    return;
                }
                const expr = fieldObject(field, sub, widget);
                if (expr && widget.name === "editable") {
                    code += `{\n    const $node = ${expr};\n    if ($node) $headerFields.push($node);\n}\n`;
                    return;
                }
                code += expr
                    ? `$headerFields.push(${expr});\n`
                    : `$headerFields.push({t:"placeholder",id:$uid(${jsString(fieldIdOf(field) + "." + widget.name)}),text:${jsString(widget.name)}});\n`;
            });
        });
        return code;
    };

    const formSingle = stmt((node) => {
        const name = formNameOf(node);
        const id = attr(node, "id") || name;
        const transition = attr(node, "transition") || "";
        const map = attr(node, "map") || "fieldValues";
        const focus = attr(node, "focus-field") || "";
        const body = layoutCode(node);
        const transitionCode = transition
            ? `const $made = sri.makeUrlByType(${jsString(transition)}, "transition", ${S.nodeLit(node, false)}, "true");\n$transitionUrl = $made ? ($made.url || "") : "";\n$formWrites = !!$made && (!$made.isScreenUrl() || $made.isTransition()) && !$made.isReadOnly();\n`
            : "";
        const focusCode = focus
            ? `const $focusName = ec.resource.expand(${jsString(focus)}, "");\nif ($focusName && $focusName !== "null") sri.setFocusField(${jsString(name)} + "." + $focusName);\n`
            : "";
        // A form with a file field posts as multipart.
        const multipart = childrenNamed(node, "field").some((field) => field.children.some((sub) => !!child(sub, "file")));
        return `sri.pushSingleFormMapContext(${map});
try {
    const $hidden = [];
    const $hiddenMap = sri.getFormHiddenParameters(${S.nodeLit(node, true)}) || {};
    const $hiddenKeys = Object.keys($hiddenMap);
    for (let $hi = 0; $hi < $hiddenKeys.length; $hi++) {
        const $key = $hiddenKeys[$hi];
        const $value = $hiddenMap[$key];
        if ($value == null || $value === "") continue;
        if ($secret.has(String($key).toLowerCase())) continue;
        $hidden.push({name:String($key),value:String($value)});
    }
    let $transitionUrl = "";
    let $formWrites = false;
    ${transitionCode}${focusCode}$openChildren({t:"form",id:$uid(${jsString(id)}),name:${jsString(name)},kind:"single",${multipart ? "multipart:true," : ""}transitionUrl:$transitionUrl,domId:${domIdExpr(node)},writes:$formWrites,method:${jsString(transition ? "post" : "")},hidden:$hidden,submitMessage:${attr(node, "submit-message") ? S.expandExpr(attr(node, "submit-message")) : '""'},backgroundReloadId:${exprOf(attr(node, "background-reload-id") || "")},backgroundHideId:${exprOf(attr(node, "background-hide-id") || "")}});
    ${body}$closeChildren();
} finally {
    sri.popContext();
}
`;
    });

    // A column of numbers: a currency or number-formatted display, or a number input.
    const numeric = (field) => {
        const sub = child(field, "default-field");
        if (!sub) return false;
        return sub.children.some((widget) =>
            (widget.name === "display" && (attr(widget, "currency-unit-field") || /[#0]/.test(attr(widget, "format")))) ||
            (widget.name === "text-line" && attr(widget, "input-type") === "number"));
    };
    const fieldTitleExpr = (field) => {
        const sub = S.headerSub(field);
        return sub ? `String(${S.localizedTitle(sub)} || "")` : jsString(attr(field, "name") || "");
    };
    const columnChoice = (field) => `{name:${jsString(attr(field, "name") || "")},title:${fieldTitleExpr(field)}}`;
    // The form-list as the host reads its hidden-parameters: its attributes and only that child.
    const listLit = (node) => {
        const hidden = child(node, "hidden-parameters");
        return hidden
            ? `{"name":"form-list","attrs":${JSON.stringify(node.attrs || {})},"children":[${S.nodeLit(hidden, true, false)}]}`
            : S.bareLit(node);
    };
    const exportCode = (node) => {
        const formName = attr(node, "name") || "list";
        const push = (kind, label, pairs) =>
            `$exports.push({kind:${jsString(kind)},label:String(ec.l10n.localize(${jsString(label)})),url:$screenUrlWith(${JSON.stringify(pairs)})});\n`;
        let code = "";
        if (flag(node, "show-csv-button")) {
            code += push("csv", "CSV", [["renderMode", "csv"], ["pageNoLimit", "true"], ["lastStandalone", "true"], ["saveFilename", formName + ".csv"]]);
        }
        if (flag(node, "show-xlsx-button")) {
            code += `if (ec.screen.isRenderModeValid("xlsx")) ${push("xlsx", "XLS", [["renderMode", "xlsx"], ["pageNoLimit", "true"], ["lastStandalone", "true"], ["saveFilename", formName + ".xlsx"]])}`;
        }
        if (flag(node, "show-text-button")) {
            code += push("text", "Text", [["renderMode", "text"], ["pageNoLimit", "true"], ["lastStandalone", "true"], ["lineCharacters", "132"],
                ["pageLines", "88"], ["lineWrap", "true"], ["saveFilename", formName + ".txt"]]);
        }
        if (flag(node, "show-pdf-button")) code += `$exports.push({kind:"pdf",label:String(ec.l10n.localize("Print")),url:$fopUrl()});\n`;
        if (flag(node, "show-all-button")) {
            code += `{
    const $noLimit = ec.context.get("pageNoLimit");
    const $all = $noLimit != null && $noLimit !== "" && String($noLimit) !== "false";
    $exports.push({kind:"all",label:String(ec.l10n.localize($all ? "Paginate" : "Show All")),url:$all ? $screenUrlWith([], "pageNoLimit") : $screenUrlWith([["pageNoLimit", "true"]])});
}
`;
        }
        return code;
    };
    const formList = stmt((node) => {
        const name = formNameOf(node);
        const formName = attr(node, "name");
        const id = attr(node, "id") || name;
        const listName = attr(node, "list") || "";
        const listKey = /^[A-Za-z_][A-Za-z0-9_]*$/.test(listName) ? listName : "";
        const idField = attr(node, "id-field") || "";
        const fields = childrenNamed(node, "field");
        // The columns of each type (columns type="mobile"), chosen by the request's _uiType as qvt does; the untyped ones otherwise.
        const types = childrenNamed(node, "columns").map((kid) => attr(kid, "type")).filter((type, index, all) => type && all.indexOf(type) === index);
        const sets = [S.formListColumns(node)].concat(types.map((type) => S.formListColumns(node, type)));
        const setChoice = types.length
            ? `const $uiType = String(ec.context.get("_uiType") || "");\n        const $set = ${types.map((type, index) => `$uiType === ${jsString(type)} ? ${index + 1} : `).join("")}0;\n        `
            : "";
        const bySet = (make) => {
            if (!types.length) return make(sets[0]);
            let code = "";
            types.forEach((type, index) => {
                code += `${index === 0 ? "if" : "} else if"} ($set === ${index + 1}) {\n${make(sets[index + 1])}`;
            });
            return code + `} else {\n${make(sets[0])}}\n`;
        };
        const bySetExpr = (make) => (types.length
            ? `(${types.map((type, index) => `$set === ${index + 1} ? ${make(sets[index + 1])} : `).join("")}${make(sets[0])})`
            : make(sets[0]));
        // A row form for each row, or one multi form of every row, when the list has a transition.
        const hasForm = !!attr(node, "transition") && attr(node, "skip-form") !== "true";
        const multi = hasForm && attr(node, "multi") === "true";
        const list = { hasForm, multi };
        const formCode = hasForm ? `let $listForm = false;
        let $transitionUrl = "";
        let $listWrites = false;
        const $listHidden = [];
        {
            const $made = sri.makeUrlByType(${jsString(attr(node, "transition"))}, "transition", null, "false");
            if ($made && !$made.getDisableLink()) {
                $listForm = true;
                $transitionUrl = $made.url || "";
                $listWrites = $writes($made);
                const $hiddenMap = Object.assign({}, $made.getParameterMap() || {}, sri.getFormHiddenParameters(${listLit(node)}) || {});
                $hiddenMap.moquiFormName = ${jsString(formName)};
                ${multi ? `$hiddenMap._isMulti = "true";\n                ` : ""}$hiddenMap.pageIndex = String(ec.context.get("pageIndex") || "0");
                const $order = ec.context.get("orderByField");
                if ($order != null && $order !== "") $hiddenMap.orderByField = String($order);
                for (const $key of Object.keys($hiddenMap)) {
                    const $value = $hiddenMap[$key];
                    if ($value == null || $value === "" || $secret.has(String($key).toLowerCase())) continue;
                    $listHidden.push({name:String($key),value:String($value)});
                }
            }
        }
        ` : "";
        const rowHidden = hasForm ? rowHiddenCode(fields, multi) : "";
        const rowHiddenBlock = rowHidden ? `if ($listForm) {
                    const $rowHidden = [];
                    ${rowHidden}if ($rowHidden.length) $row.hidden = $rowHidden;
                }
                ` : "";
        const buttons = multi ? listButtonsCode(fields) : "";
        const listFormBlock = hasForm ? `if ($listForm) {
            $list.transitionUrl = $transitionUrl;
            if ($listWrites) $list.writes = true;
            ${multi ? "$list.multi = true;\n            " : ""}if ($listHidden.length) $list.hidden = $listHidden;
            ${buttons ? `if ($info.count > 0) {
                const $buttons = [];
                ${buttons}if ($buttons.length) $list.buttons = $buttons;
            }
            ` : ""}}
        ` : "";
        const fail = (what) => `ec.logger.error(${jsString(`Leaving the ${what} of form-list ${formName} out: `)} + $err);`;
        const savedFindsBlock = attr(node, "saved-finds") === "true" ? `try {
            const $saveUrl = sri.buildUrl("formSaveFind");
            if ($saveUrl && !$saveUrl.getDisableLink()) {
                const $base = sri.getScreenUrlInstance().path;
                const $active = String(ec.context.get("formListFindId") || "");
                const $default = String(sri.getUserDefaultFormListFindId() || "");
                const $finds = [];
                for (const $find of Array.from(sri.getUserFormListFinds(${jsString(formName)}) || [])) {
                    const $id = String(($find.formListFind && $find.formListFind.formListFindId) || "");
                    if (!$id) continue;
                    $finds.push({id:$id,name:String($find.description || $id),url:$base + "?formListFindId=" + encodeURIComponent($id),active:$id === $active,default:$id === $default,mine:String($find.isByUserId) === "true"});
                }
                $list.savedFinds = {saveUrl:$saveUrl.path,formLocation:String(sri.getSavedFindFullLocation(${jsString(formName)}) || ""),parameters:$findParameters(),clearUrl:$base + "?formListFindId=_clear",finds:$finds};
            }
        } catch ($err) {
            ${fail("saved finds")}
        }
        ` : "";
        const shownExpr = (columns) => `[${columns.map((column) => `[${column.map((field) =>
            (attr(field, "hide") ? `((${attr(field, "hide")}) ? null : ${columnChoice(field)})` : columnChoice(field))).join(",")}]`).join(",")}].map(($col) => $col.filter(($choice) => $choice)).filter(($col) => $col.length)`;
        const notShownExpr = (columns) => {
            const shown = new Set(columns.flat().map((field) => attr(field, "name")));
            return `[${fields.filter((field) => !S.listFieldHiddenWidget(field) && !shown.has(attr(field, "name"))).map(columnChoice).join(",")}]`;
        };
        const selectColumnsBlock = attr(node, "select-columns") === "true" ? `try {
            const $columnsUrl = sri.buildUrl("formSelectColumns");
            if ($columnsUrl && !$columnsUrl.getDisableLink()) {
                $list.selectColumns = {url:$columnsUrl.path,formLocation:${jsString(formName)},parameters:$findParameters(),shown:${bySetExpr(shownExpr)},hidden:${bySetExpr(notShownExpr)}};
            }
        } catch ($err) {
            ${fail("column choice")}
        }
        ` : "";
        const exports = exportCode(node);
        const exportsBlock = exports ? `try {
            const $exports = [];
            ${exports}if ($exports.length) $list.exports = $exports;
        } catch ($err) {
            ${fail("exports")}
        }
        ` : "";
        // The row-selection actions with a dialog: each a dialog of its form, after the list it acts on.
        const selection = child(node, "row-selection");
        const actionDialogs = selection ? childrenNamed(selection, "action").map((action, index) => {
            const dialog = child(action, "dialog");
            const form = child(action, "form-single");
            if (!dialog || !form) return "";
            const button = attr(dialog, "button-text");
            const dialogId = attr(dialog, "id") || id + ".action" + index;
            return `$openChildren({t:"dialog",id:$uid(${jsString(dialogId)}),title:String(${titleOf(attr(dialog, "title") || button)} || ""),button:String(${titleOf(button)} || ""),list:$list.id});
        {
        ${form.source || ""}}
        $closeChildren();
        `;
        }).join("") : "";
        const titles = (columns) => columns.map((column) => {
            const parts = column.map((field) => {
                const sub = S.headerSub(field);
                return sub ? `String(${S.localizedTitle(sub)} || "")` : jsString(attr(field, "name") || "");
            });
            // orderBy is the orderByField value for the column's first field with show-order-by, as the html header links send it
            const showOrderBy = (field) => {
                const header = child(field, "header-field");
                return header ? attr(header, "show-order-by") : "";
            };
            const sortField = column.find((field) => showOrderBy(field) === "true" || showOrderBy(field) === "case-insensitive");
            const orderBy = sortField ? (showOrderBy(sortField) === "case-insensitive" ? "^" : "") + attr(sortField, "name") : "";
            const orderCode = orderBy ? `,orderBy:${jsString(orderBy)}` : "";
            // The client sizes columns from print-width as the text mode does, against the canvas it has.
            const sized = column.find((field) => attr(field, "print-width"));
            const widthCode = sized ? `,printWidth:${jsString(attr(sized, "print-width"))},printWidthType:${jsString(attr(sized, "print-width-type") || "characters")}` : "";
            const alignCode = column.some(numeric) ? `,align:"right"` : "";
            // the names of the fields the column's cell shows, in its parts' order: the keys of a canvas read's rows
            const names = column.filter((field) => attr(field, "hide") !== "true").map((field) => attr(field, "name")).filter((name) => name);
            const fieldsCode = names.length ? `,fields:${JSON.stringify(names)}` : "";
            return `$columns.push({title:[${parts.join(",")}].filter(($part) => $part).join(" / ")${orderCode}${widthCode}${alignCode}${fieldsCode}});\n`;
        }).join("");
        const rowKey = idField
            ? `String(sri.getNamedValuePlain(${jsString(idField)}, ${S.nodeLit(node, false)}) || $index)`
            : "String($index)";
        const pageName = (suffix) => (listKey ? jsString(listKey + suffix) : '""');
        const selectIdField = selection ? attr(selection, "id-field") : "";
        // row-selection condition: checked once a render; when it does not hold the list is written as one without row-selection
        const selectCondition = selection ? attr(selection, "condition") : "";
        const selectOn = selectCondition ? `const $selOn = ${S.heldCode(selectCondition, `row-selection of form-list ${formName}`)};
        ` : "";
        const ifSelOn = (code) => (selectCondition && code ? `if ($selOn) {\n${code}}\n` : code);
        // The list views (views, view-board, view-calendar, view-tree; style-guide §8.2), as qvt's m-list-views reads them: the
        // client draws the chosen one with its board, calendar or tree component, from the page's rows as props.
        const viewBoard = child(node, "view-board");
        const viewCalendar = child(node, "view-calendar");
        const viewTree = child(node, "view-tree");
        const namesOf = (text) => String(text || "").split(",").map((part) => part.trim()).filter((part) => part);
        const viewKinds = (() => {
            const known = { list: true, board: !!viewBoard, calendar: !!viewCalendar, tree: !!viewTree };
            const named = namesOf(attr(node, "views")).filter((kind, index, all) => known[kind] && all.indexOf(kind) === index);
            return named.length ? named : ["list"].concat(viewBoard ? ["board"] : [], viewCalendar ? ["calendar"] : [], viewTree ? ["tree"] : []);
        })();
        const hasViews = viewKinds.length > 1;
        const vb = (name) => (viewBoard ? attr(viewBoard, name) : "");
        const vc = (name) => (viewCalendar ? attr(viewCalendar, name) : "");
        const fieldByName = {};
        fields.forEach((field) => { fieldByName[attr(field, "name")] = field; });
        const firstShown = fields.find((field) => !S.listFieldHiddenWidget(field));
        const viewTitle = vb("card-title-field") || vc("title-field") || (firstShown ? attr(firstShown, "name") : "");
        const hiddenKeys = fields.filter((field) => { const dflt = child(field, "default-field"); return dflt && child(dflt, "hidden"); })
            .map((field) => attr(field, "name"));
        const viewKey = (el) => namesOf(el ? attr(el, "key-fields") : "")[0] || hiddenKeys[0] || selectIdField || viewTitle;
        // A field's text as the list shows it: its default widget's cell text, a link's text, else the plain value.
        const viewText = (name) => {
            const field = fieldByName[name];
            const dflt = field ? child(field, "default-field") : null;
            const widget = dflt ? dflt.children.find((kid) => kid.name && kid.name !== "hidden" && kid.name !== "ignored") : null;
            if (!widget) return `String(ec.context.get(${jsString(name)}) ?? "")`;
            if (widget.name === "link") return `String(${S.linkTextExpr(widget)} ?? "")`;
            return `String(${cellText(widget)} ?? "")`;
        };
        const boardFields = namesOf(vb("card-fields"));
        const calendarFields = namesOf(vc("card-fields"));
        // the board's default grouping: group-field, else the first of group-fields
        const boardGroup = vb("group-field") || namesOf(vb("group-fields"))[0] || "";
        // the tree (view-tree): the first field of each column shown, less the roll-ups, the first the outline's label; the roll-ups
        const vt = (name) => (viewTree ? attr(viewTree, name) : "");
        const treeRollups = namesOf(vt("rollup-fields"));
        const treeColumns = S.formListColumns(node).map((column) => column.find((field) => !S.listFieldHiddenWidget(field) && !treeRollups.includes(attr(field, "name"))))
            .filter((field) => field).map((field) => attr(field, "name"));
        const treeTitle = (name) => {
            const sub = fieldByName[name] ? S.headerSub(fieldByName[name]) : null;
            return sub ? `String(${S.localizedTitle(sub)} || "")` : jsString(name);
        };
        const viewRowCode = hasViews ? `{
                    const $value = ($name) => { const $v = ec.context.get($name); return $v == null ? "" : String($v); };
                    const $stamp = ($name) => { const $v = ec.context.get($name); return $v == null || $v === "" ? "" : String(ec.l10n.format($v, "yyyy-MM-dd HH:mm") || ""); };
                    const $v = {id:$value(${jsString(viewKey(viewBoard || viewCalendar || viewTree))}) || $rowKey,title:${viewText(viewTitle)},_open:$firstLink($cells)};
                    ${viewTree ? `$v.key = $value(${jsString(viewKey(viewTree))});
                    $v.parent = $value(${jsString(vt("parent-field"))});
                    $v.label = ${viewText(treeColumns[0] || viewTitle)};
                    $v.tc = [${treeColumns.slice(1).map(viewText).join(", ")}];
                    $v.tr = [${treeRollups.map(viewText).join(", ")}];
                    ` : ""}${viewBoard ? `$v.group = $value(${jsString(boardGroup)});
                    $v.groupText = ${viewText(boardGroup)};
                    ${boardFields.map((name, index) => `$v.f${index} = ${viewText(name)};
                    `).join("")}${vb("badge-field") ? `$v.badge = ${viewText(vb("badge-field"))};
                    ` : ""}` : ""}${viewCalendar ? `$v.start = $stamp(${jsString(vc("start-field"))});
                    $v.end = $stamp(${jsString(vc("end-field"))});
                    ${calendarFields.map((name, index) => `$v.c${index} = ${viewText(name)};
                    `).join("")}` : ""}$viewRows.push($v);
                }
            ` : "";
        const boardFieldNames = JSON.stringify(boardFields.map((name, index) => "f" + index).concat(vb("badge-field") ? ["badge"] : []));
        const viewsBlock = hasViews ? `try {
            let $formKey = String(sri.getScreenUrlInstance().path || "");
            for (const $root of ["/apps", "/qapps"]) {
                if ($formKey === $root || $formKey.startsWith($root + "/")) { $formKey = $formKey.slice($root.length); break; }
            }
            $formKey += "#" + ${jsString(formName)};
            const $prefKey = "LIST_VIEW_" + $formKey;
            const $kinds = ${JSON.stringify(viewKinds)};
            const $asked = String(ec.context.get("view") || "");
            const $kept = String(ec.user.getPreference($prefKey) || "");
            $list.views = {kinds:$kinds,key:$prefKey,chosen:$kinds.includes($asked) ? $asked : ($kinds.includes($kept) ? $kept : $kinds[0])};
            ${viewBoard ? `{
                const $group = ${jsString(boardGroup)};
                const $statusTypeId = ${jsString(vb("status-type-id"))};
                const $order = ${JSON.stringify(namesOf(vb("column-order")))};
                const $isStatus = !!$statusTypeId || /^statusId$|StatusId$/.test($group);
                let $groups = [];
                if ($isStatus || $order.length) {
                    const $values = $order.concat($viewRows.map(($row) => $row.group)).filter(($g, $i, $all) => $g && $all.indexOf($g) === $i);
                    const $out = ec.service.sync().name("org.moqui.impl.StatusServices.get#ListViewColumns").parameters({
                        values:$values,statusTypeId:$statusTypeId || null,status:$isStatus,statusFlowId:${jsString(vb("status-flow-id"))} || null
                    }).disableAuthz().call() || {};
                    $groups = Array.from($out.columns || []).map(($c) => ({value:String($c.value),label:String($c.label || $c.value),color:$c.color ? String($c.color) : ""}));
                }
                if (!$groups.length) {
                    for (const $row of $viewRows) {
                        if ($row.group && !$groups.some(($g) => $g.value === $row.group)) $groups.push({value:$row.group,label:$row.groupText || $row.group,color:""});
                    }
                }
                const $move = ${vb("transition") ? `${vb("move-condition") ? `!${S.heldCode(vb("move-condition"), `moves of the view-board of form-list ${formName}`)} ? null : ` : ""}sri.makeUrlByType(${jsString(vb("transition"))}, "transition", null, "false")` : "null"};
                $list.views.board = {groups:$groups,rows:$viewRows,groupId:"value",groupLabel:"label",groupColor:"color",groupOf:"group",id:"id",title:"title",
                    fields:${boardFieldNames},open:"{_open}",move:$move && !$move.getDisableLink() ? ($move.getUrlWithParams() || "") : "",
                    moveId:${jsString(viewKey(viewBoard))},moveGroup:${jsString((boardGroup === vb("group-field") && vb("parameter-name")) || boardGroup)}};
            }
            ` : ""}${viewCalendar ? `$list.views.calendar = {events:$viewRows.filter(($row) => $row.start),start:"start",end:"end",title:"title"};
            ` : ""}${viewTree ? `$list.views.tree = {rows:$viewRows,columns:[${treeColumns.map(treeTitle).join(", ")}],rollups:[${treeRollups.map(treeTitle).join(", ")}]};
            ` : ""}
        } catch ($err) {
            ${fail("views")}
        }
        ` : "";
        return `{
    const $info = sri.openFormList(${jsString(name)});
    try {
        ${setChoice}${formCode}const $columns = [];
        ${bySet(titles)}const $headerFields = [];
        ${headerFields(node)}const $rows = [];
        ${selectOn}${hasViews ? `const $viewRows = [];
        const $firstLink = ($cells) => {
            for (const $cell of $cells || []) {
                if (Array.isArray($cell)) { const $in = $firstLink($cell); if ($in) return $in; }
                else if ($cell && typeof $cell === "object" && $cell.t === "link" && $cell.url && !$cell.disabled) return String($cell.url);
            }
            return "";
        };
        ` : ""}for (let $index = 0; $index < $info.count; $index++) {
            sri.startFormListRow($info, $index);
            try {
                const $rowKey = ${rowKey};
                const $cells = [];
                ${bySet((columns) => columnCells(columns, list))}const $row = {id:$rowKey,cells:$cells};
                ${rowHiddenBlock}${selectIdField ? (selectCondition ? `if ($selOn) {
                    const $selectId = ec.context.get(${jsString(selectIdField)});
                    if ($selectId != null && $selectId !== "") $row.selectId = String($selectId);
                }
                ` : `const $selectId = ec.context.get(${jsString(selectIdField)});
                if ($selectId != null && $selectId !== "") $row.selectId = String($selectId);
                `) : ""}$rows.push($row);
            ${viewRowCode}} finally {
                sri.endFormListRow();
            }
        }
        const $indexValue = ${pageName("PageIndex")} ? ec.context.get(${pageName("PageIndex")}) : null;
        const $pageIndex = $indexValue == null || $indexValue === "" ? $whole(ec.context.get("pageIndex")) : $whole($indexValue);
        const $sizeValue = ${pageName("PageSize")} ? ec.context.get(${pageName("PageSize")}) : null;
        const $pageSize = $sizeValue == null || $sizeValue === "" ? $whole(ec.context.get("pageSize")) : $whole($sizeValue);
        const $total = ${pageName("Count")} ? ec.context.get(${pageName("Count")}) : null;
        const $count = $total == null || $total === "" ? $info.count : $whole($total);
        const $size = $pageSize > 0 ? $pageSize : $info.count;
        const $paging = {index:$pageIndex,size:$size,count:$count};
        for (const [$key, $suffix] of [["maxIndex", "PageMaxIndex"], ["rangeLow", "PageRangeLow"], ["rangeHigh", "PageRangeHigh"]]) {
            const $value = ${listKey ? `ec.context.get(${jsString(listKey)} + $suffix)` : "null"};
            if ($value != null && $value !== "") $paging[$key] = $whole($value);
        }
        const $list = {t:"formList",id:$uid(${jsString(id)}),name:${jsString(name)},domId:${domIdExpr(node)},columns:$columns,rows:$rows,paging:$paging,headerFields:$headerFields};
        ${selection ? `${selectCondition ? "if ($selOn) " : ""}$list.rowSelection = {idField:${jsString(selectIdField)},parameter:${jsString(attr(selection, "parameter") || selectIdField)}${attr(selection, "list-mode") ? `,listMode:${jsString(attr(selection, "list-mode"))}` : ""}};
        ` : ""}${listFormBlock}${savedFindsBlock}${selectColumnsBlock}${exportsBlock}${viewsBlock}$emit($list);
${actionDialogs ? "        " + ifSelOn(actionDialogs) : ""}    } finally {
        sri.safeCloseList($info);
    }
}
`;
    });

    const rawText = (node) => {
        if (attr(node, "location")) {
            return `sri.renderTextValue(${S.expandNoL10nExpr(attr(node, "location"))}, ${jsString(attr(node, "template") || "true")})`;
        }
        const inline = node.text || "";
        const template = attr(node, "template");
        return !template || template === "true" ? `sri.renderInlineText(${jsString(inline)})` : jsString(inline);
    };
    // A tui text is one node or an array of nodes. Text that is not JSON, or JSON of another shape, becomes an
    // error node naming the screen and the text, and the rest of the page is kept.
    const tuiText = (node) => {
        if (!attr(node, "location") && !(node.text || "").trim()) return "";
        const where = `String((sri.getActiveScreenDef() && sri.getActiveScreenDef().location) || "") + " " + ${jsString(attr(node, "location") || "inline")}`;
        return `{
    const $rawValue = ${rawText(node)};
    const $raw = String($rawValue == null ? "" : $rawValue);
    let $parsed;
    let $problem = "";
    try { $parsed = JSON.parse($raw); } catch ($parseError) { $problem = " is not JSON: " + $parseError.message; }
    if (!$problem && $parsed && typeof $parsed === "object" && !Array.isArray($parsed)) $parsed = [$parsed];
    if (!$problem && !(Array.isArray($parsed) && $parsed.every(($item) => $item && typeof $item === "object" && !Array.isArray($item) && typeof $item.t === "string"))) {
        $problem = " is not a node or an array of nodes";
    }
    // A component names its client widget, and its transitions are under the screen's path unless it gives a base.
    if (!$problem) {
        const $baseUrl = sri.getScreenUrlInstance();
        const $base = ($baseUrl && $baseUrl.path) || "/";
        const $components = ($items) => {
            for (const $item of $items) {
                if ($problem) return;
                if (!$item || typeof $item !== "object" || Array.isArray($item)) continue;
                if ($item.t === "component") {
                    if (typeof $item.name !== "string" || !$item.name.trim()) {
                        $problem = " has a component with no name";
                        return;
                    }
                    if (typeof $item.base !== "string" || !$item.base) $item.base = $base;
                }
                if (Array.isArray($item.c)) $components($item.c);
            }
        };
        $components($parsed);
    }
    if ($problem) {
        const $text = ${where} + $problem;
        ec.logger.warn("tui text in " + $text);
        $emit({t:"error",id:$uid(${jsString(idOf(node))}),text:$text});
    } else {
        $emit({t:"fragment",id:$uid(${jsString(idOf(node))}),c:$parsed});
    }
}
`;
    };
    // HTML as blocks and numbered links (sri.htmlText); text without markup is the same blocks.
    const htmlNode = (idExpr, htmlExpr) => `$emit(Object.assign({t:"html",id:$uid(${idExpr})}, JSON.parse(sri.htmlText(String(${htmlExpr} == null ? "" : ${htmlExpr})))));\n`;
    // An `any` text is written for the browser, so it is read as HTML.
    const anyText = (node) => {
        if (!attr(node, "location") && !(node.text || "").trim()) return "";
        return `{
    const $rawValue = ${rawText(node)};
    ${htmlNode(jsString(idOf(node)), "$rawValue")}}
`;
    };
    const textCode = (node) => {
        const type = attr(node, "type");
        if (typeNames(type).includes("tui")) return tuiText(node);
        if (!type || type === "any") return anyText(node);
        return `$emit({t:"placeholder",id:$uid(${jsString(idOf(node))}),text:"text",url:${screenUrlExpr}});\n`;
    };
    const menuItems = `const $menu = sri.getMenuItems() || [];
        const $items = [];
        for (let $mi = 0; $mi < $menu.length; $mi++) {
            const $item = $menu[$mi];
            $items.push({title:$item.title || "",url:$item.url || "",active:!!$item.inPath});
        }
`;

    // A status id list attribute: a comma list, or one ${} expression of a list; null when it is not given (qvt's idList).
    const idListExpr = (text) => {
        if (!text) return "null";
        const whole = /^\s*\$\{([\s\S]*)\}\s*$/.exec(text);
        if (whole) return `$ids(ec.resource.expression(${jsString(whole[1])}, ""))`;
        return `$ids(${exprNoL10n(text)})`;
    };
    // status-bar: the stages and transitions of org.moqui.impl.StatusServices.get#StatusBar, as qvt's statusBarData, and a
    // link per transition that posts the new status. Without a status, or when the service fails, the status's text alone.
    const statusBarCode = (node) => {
        const transition = attr(node, "transition");
        const title = attr(node, "title") ? `String(${S.expandExpr(attr(node, "title"))} || "")` : '""';
        const links = transition ? `if ($bar && !$bar.readOnly && $bar.transitions.length) {
        const $made = sri.makeUrlByType(${jsString(transition)}, "transition", ${S.nodeLit(node, true)}, "true");
        if ($made && !$made.getDisableLink()) {
            const $title = ${title};
            const $confirm = String(${exprNoL10n(attr(node, "confirm"))}) !== "false";
            for (const $tr of $bar.transitions) {
                const $to = $tr.toStatusId == null ? "" : String($tr.toStatusId);
                if (!$to) continue;
                const $label = $tr.label == null || $tr.label === "" ? $to : String($tr.label);
                const $link = {t:"link",id:$uid($barId + "." + $to),text:$label,url:$made.cloneUrlInstance().addParameter(${jsString(attr(node, "parameter-name") || "statusId")}, $to).getUrlWithParams() || "",method:"post",kind:"hidden-form-link",btnType:$tr.isNext ? "primary" : ($tr.isDanger ? "danger" : "")};
                if ($confirm) $link.confirm = $label + ($title ? ": " + $title : "") + "?";
                if ($writes($made)) $link.writes = true;
                $emit($link);
            }
        }
    }
    ` : "";
        return `{
    const $barId = $uid(${jsString(idOf(node))});
    const $rawStatus = ${exprNoL10n(attr(node, "status-id"))};
    const $statusId = $rawStatus == null ? "" : String($rawStatus).trim();
    let $bar = null;
    if ($statusId && $statusId !== "null") {
        try {
            const $readOnly = String(${exprNoL10n(attr(node, "read-only"))}) === "true";
            const $out = ec.service.sync().name("org.moqui.impl.StatusServices.get#StatusBar").parameters({statusId:$statusId,statusTypeId:${exprNoL10n(attr(node, "status-type-id"))} || null,statusFlowId:${exprNoL10n(attr(node, "status-flow-id"))} || null,stageStatusIds:${idListExpr(attr(node, "stages"))},allowedToStatusIds:$readOnly ? [] : ${idListExpr(attr(node, "allowed-status-ids"))}}).disableAuthz().call() || {};
            $bar = {readOnly:$readOnly,current:$out.current || null,stages:Array.from($out.stages || []),transitions:$readOnly ? [] : Array.from($out.transitions || [])};
        } catch ($err) {
            ec.logger.error(${jsString(`Leaving the stages of status-bar ${idOf(node)} out for status `)} + $statusId + ": " + $err);
        }
    }
    const $text = $bar && $bar.current && $bar.current.label != null && $bar.current.label !== "" ? String($bar.current.label) : String(${S.statusTextExpr(node)} || "");
    const $stages = $bar ? $bar.stages.map(($stage) => ({statusId:$stage.statusId == null ? "" : String($stage.statusId),label:$stage.label == null ? "" : String($stage.label),state:$stage.state == null ? "" : String($stage.state)})) : [];
    $openChildren({t:"statusBar",id:$barId,text:$text,stages:$stages});
    ${links}$closeChildren();
}
`;
    };
    // tree: the URL its items load from (the transition, else the actions transition and the tree's name), as qvt's m-tree-top.
    const treeCode = (node) => {
        const name = attr(node, "name") || attr(node, "id") || "tree";
        const transition = attr(node, "transition");
        const items = transition ? `($made.path || "")` : `($made.path || "") + ${jsString("/" + (attr(node, "name") || ""))}`;
        return `{
    const $made = sri.makeUrlByType(${jsString(transition || "actions")}, "transition", ${S.nodeLit(node, true)}, "true");
    const $tree = {t:"tree",id:$uid(${jsString(name)}),url:$made ? ${items} : ""};
    const $open = ${exprNoL10n(attr(node, "open-path"))};
    if ($open != null && $open !== "" && $open !== "null") $tree.openPath = String($open);
    const $params = $strMap($made ? $made.getParameterMap() : null, true);
    if (Object.keys($params).length) $tree.parameters = $params;
    $emit($tree);
}
`;
    };

    const pass = stmt((node, children) => children);
    // A box toolbar, a panel side, a split side: a region node in place, so the client keeps where its nodes go.
    const regionCode = (owner, kid, name, extra) => kid && kid.source
        ? `$openChildren({t:"region",id:$uid(${jsString(idOf(owner) + "." + name)}),name:${jsString(name)}${extra || ""}});\n${kid.source}$closeChildren();\n`
        : "";
    const sizeOf = (kid, name) => (kid && /^\d+$/.test(attr(kid, name)) ? `,size:${attr(kid, name)}` : "");
    const group = (type, extra) => stmt((node, children) =>
        `$openChildren({t:${jsString(type)},id:$uid(${jsString(idOf(node))})${extra ? extra(node) : ""}});\n${children}$closeChildren();\n`);

    const overrides = {
        "screen": pass,
        "widgets": pass,
        "fail-widgets": pass,
        "dynamic-form": pass,
        // The user's columns form is a nested unit, so it and the form's own code both sit in one fragment's "c".
        "form-list-user-columns": stmt((node, children) =>
            `$openRender({t:"fragment",id:$uid(${jsString((attr(node, "name") || "list") + ".columns")})});
if (!sri.renderUserColumnsForm(${jsString(attr(node, "name") || "list")})) {
    $w("[");
    $stack.push($comma);
    $comma = false;
    ${children}$w("]");
    $comma = $stack.pop();
}
$closeRender();
`),
        "box-header": pass,
        "box-toolbar": pass,
        "box-body": pass,
        "box-body-nopad": pass,
        "panel-header": pass,
        "panel-left": pass,
        "panel-center": pass,
        "panel-right": pass,
        "panel-footer": pass,
        "split-side": pass,
        "split-main": pass,
        "field": pass,
        "header-field": pass,
        "first-row-field": pass,
        "second-row-field": pass,
        "last-row-field": pass,
        "default-field": pass,
        "conditional-field": pass,
        "field-group": pass,
        "field-accordion": pass,
        "field-row": pass,
        "field-row-big": pass,
        "field-col-row": pass,
        "field-col": pass,
        "fields-not-referenced": pass,
        "subscreens-menu": pass,
        "container": group("container"),
        "container-box": stmt((node) => {
            const header = child(node, "box-header");
            const title = header ? attr(header, "title") : "";
            const body = node.children.filter((kid) => kid.name !== "box-toolbar").map((kid) => kid.source || "").join("");
            return `$openChildren({t:"box",id:$uid(${jsString(idOf(node))}),title:${titleOf(title)},type:${jsString(attr(node, "type"))},initial:${jsString(attr(node, "initial"))}});
${regionCode(node, child(node, "box-toolbar"), "toolbar")}${body}$closeChildren();
`;
        }),
        "container-row": group("row"),
        "row-col": group("col", (node) =>
            `,xs:${jsString(attr(node, "xs"))},sm:${jsString(attr(node, "sm"))},md:${jsString(attr(node, "md"))},lg:${jsString(attr(node, "lg"))}`),
        "container-panel": stmt((node) => {
            const regions = ["header", "left", "center", "right", "footer"].map((name) => {
                const kid = child(node, "panel-" + name);
                return regionCode(node, kid, name, name === "left" || name === "right" ? sizeOf(kid, "size") : "");
            }).join("");
            return `$openChildren({t:"panel",id:$uid(${jsString(idOf(node))})});\n${regions}$closeChildren();\n`;
        }),
        "container-dialog": stmt((node, children) => {
            const button = attr(node, "button-text") || "";
            const title = attr(node, "title") || button;
            return guarded(node, `$openChildren({t:"dialog",id:$uid(${jsString(idOf(node))}),title:${titleOf(title)},button:${titleOf(button)}});\n${children}$closeChildren();\n`);
        }),
        "container-split": stmt((node) => {
            const width = /^\d+$/.test(attr(node, "side-width")) ? `,sideWidth:${attr(node, "side-width")}` : "";
            const regions = regionCode(node, child(node, "split-side"), "side") + regionCode(node, child(node, "split-main"), "main");
            return `$openChildren({t:"split",id:$uid(${jsString(idOf(node))})${width}${attr(node, "select-parameter") ? `,selectParameter:${jsString(attr(node, "select-parameter"))}` : ""}});\n${regions}$closeChildren();\n`;
        }),
        "dashboard": group("dashboard"),
        "section": stmt((node) =>
            `$openRender({t:"section",id:$uid(${jsString(idOf(node))}),name:${jsString(attr(node, "name") || "")}});\nsri.renderSection(${jsString(attr(node, "name") || "")});\n$closeRender();\n`),
        "section-iterate": stmt((node) =>
            `$openRender({t:"section",id:$uid(${jsString(idOf(node))}),name:${jsString(attr(node, "name") || "")},iterate:true});\nsri.renderSection(${jsString(attr(node, "name") || "")});\n$closeRender();\n`),
        "section-include": stmt((node) =>
            `$openRender({t:"section",id:$uid(${jsString(idOf(node))}),name:${jsString(attr(node, "name") || "")}});\nsri.renderSectionInclude(${S.bareLit(node)});\n$closeRender();\n`),
        "subscreens-active": stmt((node) =>
            `$openRender({t:"active",id:$uid(${jsString(attr(node, "id") || "subscreens")})});\nsri.renderSubscreen();\n$closeRender();\n`),
        "subscreens-panel": stmt((node) => {
            const type = attr(node, "type") || "tab";
            const id = attr(node, "id") || "subscreens";
            // A wizard's caption for its last step, as qvt's m-subscreens-wizard finish-text.
            const finish = type === "wizard" && attr(node, "finish-text") ? `,finishText:String(${S.expandExpr(attr(node, "finish-text"))} || "")` : "";
            return `{
                ${menuItems}$openRender({t:"subscreens",id:$uid(${jsString(id)}),kind:${jsString(type)},items:$items${finish}});
                sri.renderSubscreen();
                $closeRender();
            }
`;
        }),
        "include-screen": stmt((node) =>
            `$openRender({t:"section",id:$uid(${jsString(attr(node, "location") || "include")}),name:"include"});\nsri.renderIncludeScreen(${jsString(attr(node, "location"))}, ${jsString(attr(node, "share-scope"))});\n$closeRender();\n`),
        "render-dynamic-form": stmt((node) => {
            const name = attr(node, "name") || "form";
            return `$openRender({t:"fragment",id:$uid(${jsString(name)})});\nsri.renderDynamicForm(${jsString(name)});\n$closeRender();\n`;
        }),
        "render-mode": stmt((node) => {
            const texts = childrenNamed(node, "text");
            const tui = texts.find((kid) => typeNames(attr(kid, "type")).includes("tui"));
            if (tui) return textCode(tui);
            const any = texts.find((kid) => !attr(kid, "type") || attr(kid, "type") === "any");
            if (any) return textCode(any);
            return `$emit({t:"placeholder",id:$uid(${jsString(idOf(node))}),text:"render-mode",url:${screenUrlExpr}});\n`;
        }),
        "text": stmt((node) => textCode(node)),
        "label": stmt((node) => guarded(node,
            `$emit({t:"text",id:$uid(${jsString(idOf(node))}),text:${S.expandExpr(attr(node, "text") || "")}${attr(node, "type") && attr(node, "type") !== "span" ? `,style:${jsString(attr(node, "type"))}` : ""}});\n`)),
        "link": stmt((node) => {
            if (fieldNodeOf(node) && formNodeOf(node)) return "";
            return linkStatement(node, inHeader(node) ? false : formNodeOf(node) && formNodeOf(node).name === "form-list");
        }),
        "form-single": formSingle,
        "form-list": formList,
        "number-card": stmt((node) => guarded(node,
            `$emit({t:"numberCard",id:$uid(${jsString(idOf(node))}),title:String(${S.expandExpr(attr(node, "title") || "")} || ""),value:String(${S.expandNoL10nExpr(attr(node, "value") || "")} || ""),url:String(${S.expandNoL10nExpr(attr(node, "url") || "")} || "")});\n`)),
        "dynamic-dialog": stmt((node) => guarded(node, `{
    const $made = sri.makeUrlByType(${jsString(attr(node, "transition") || "")}, "transition", ${S.nodeLit(node, true)}, "true");
    const $button = String(${S.expandExpr(attr(node, "button-text") || "")} || "");
    $emit({t:"dynamicDialog",id:$uid(${jsString(idOf(node))}),url:$made ? ($made.getUrlWithParams() || "") : "",button:$button,title:${attr(node, "title") ? `String(${S.expandExpr(attr(node, "title"))} || "") || $button` : "$button"}});
}
`)),
        "dynamic-container": stmt((node) => `{
    const $made = sri.makeUrlByType(${jsString(attr(node, "transition") || "")}, "transition", ${S.nodeLit(node, true)}, "true");
    $emit({t:"dynamicContainer",id:$uid(${jsString(idOf(node))}),url:$made ? ($made.addParameter("_dynamic_container_id", ${jsString(idOf(node))}).getUrlWithParams() || "") : ""});
}
`),
        "image": stmt((node) =>
            `$emit({t:"image",id:$uid(${jsString(idOf(node))}),alt:String(${S.expandExpr(attr(node, "alt") || "")} || ""),url:String(${S.expandNoL10nExpr(attr(node, "url") || "")} || "")});\n`),
        // a code-view as a pre block of an html node, its lines kept
        "code-view": stmt((node) => guarded(node, `{
    const $code = ((t) => (t && t !== "null" ? String(t).replace(/\\s+$/, "") : ""))(ec.resource.expandNoL10n(${jsString(attr(node, "text") || "")}));
    if ($code) ${htmlNode(jsString(idOf(node)), '"<pre>" + $code.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;") + "</pre>"')}}
`)),
        "markdown-view": stmt((node) => guarded(node,
            `$emit({t:"markdown",id:$uid(${jsString(idOf(node))}),text:String(${S.markdownTextExpr(node)} || "")});\n`)),
        "status-bar": stmt((node) => guarded(node, statusBarCode(node))),
        // The text when there is one, else the HTML, as the text modes read an html-view.
        "html-view": stmt((node) => guarded(node, `{
    const $text = ec.resource.expandNoL10n(${jsString(attr(node, "text") || "")});
    if ($text && $text !== "null") $emit({t:"text",id:$uid(${jsString(idOf(node))}),text:String($text).trim()});
    else {
        const $html = ec.resource.expandNoL10n(${jsString(attr(node, "html") || "")});
        ${htmlNode(jsString(idOf(node)), "($html === \"null\" ? \"\" : $html)")}    }
}
`)),
        // Its items are compiled (button-menu is not opaque here) and are the menu's children: links, dialogs and the like.
        "button-menu": stmt((node) => {
            const items = node.children.filter((kid) => kid.name !== "image" && kid.name !== "parameter").map((kid) => kid.source || "").join("");
            return guarded(node, `{
    let $text = ${S.linkTextExpr(node)};
    if ($text == null || $text === "null") $text = "";
    $openChildren({t:"buttonMenu",id:$uid(${jsString(idOf(node))}),text:String($text)});
}
${items}$closeChildren();
`);
        }),
        // as qvt: a menu-item writes nothing
        "menu-item": stmt(() => ""),
        "tree": stmt(treeCode),
        // The row-selection actions are written by the form-list after the list, so nothing here; their forms are compiled.
        "row-selection": stmt(() => ""),
        "action": pass,
        "include-content": stmt((node) =>
            `$emit({t:"placeholder",id:$uid(${jsString(idOf(node))}),text:"include-content",url:String(${S.expandNoL10nExpr(attr(node, "location") || "")} || "")});\n`)
    };

    // Elements a form, link or list macro reads for itself, and elements the terminal does not draw (context).
    const formInternals = [
        "check", "date-find", "date-period", "date-time", "display", "display-entity", "drop-down", "dynamic-options",
        "editable", "file", "hidden", "ignored", "lookup", "password", "radio", "range-find", "reset", "submit",
        "text-area", "text-find", "text-line", "entity-options", "list-options", "option", "depends-on", "field-layout",
        "field-ref", "form-list-column", "columns", "column", "auto-fields-entity", "auto-fields-service",
        "auto-widget-entity", "auto-widget-service", "exclude", "entity-find", "entity-find-one", "set",
        "hidden-parameters", "row-selection", "row-actions", "visible-when", "editable-load", "action", "first-row",
        "second-row", "last-row", "parameter", "dialog", "tree-node", "tree-sub-node", "view-board", "view-calendar", "view-tree",
        "context"
    ];
    // Drawn as a placeholder with the browser link until a later phase gives them a terminal view.
    const later = ["chatter", "record-files"];
    const macros = Object.assign({}, T.noops(formInternals), overrides);
    later.forEach((name) => {
        macros[name] = stmt((node) => placeholderEmit(idOf(node), node.name, screenUrlExpr));
    });
    const unnamed = S.allElements.filter((name) => !macros[name]);
    if (unnamed.length) throw new Error("ScreenTui.js names no macro for " + unnamed.join(", "));

    // button-menu hands its items to its own macro, and the form-list places the forms of its row-selection actions, so
    // their children are generated, as in qvt.
    const opaque = S.opaqueElements.filter((name) => !["button-menu", "row-selection", "action"].includes(name));

    return { macros, opaque, prelude, epilogue };
})
