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

// One function per concrete xml-actions element. The Rust walker calls
// macros[name](node, children). Node is {name, attrs, text, children}.
// Each child already has its rendered source. Statements use bare names.
//
// This file is a function of the support object from TemplateSupport.js,
// which describes the macro kinds (stmt, expr, chain, macro) and the js tag.
// A macro computes its pieces first, then returns one template. Multi-line
// statements use js so the template reads like the generated code; its
// nesting is by indentation of the template, not by spaces in the output.
(function (T) {
    "use strict";

    const { attr, has, attrOrNull, jsString, child, childrenNamed, sourcesOf, bodyOf, joinPresent, raw, unraw, js } = T;
    const { stmt, expr, chain, macro, noops } = T;

    const expandExpr = (text) => `ec.resource.expand(${jsString(text)})`;

    const literalOrExpand = (text) => {
        const value = text == null ? "" : String(text);
        if (value.indexOf("$") >= 0 && value.charAt(value.length - 1) !== "$") return expandExpr(value);
        return jsString(value);
    };

    const valueExpr = (node) => {
        let base;
        if (has(node, "from")) base = attr(node, "from");
        else if (has(node, "value")) base = expandExpr(attr(node, "value"));
        else base = "null";
        if (has(node, "default-value")) {
            const fallback = expandExpr(attr(node, "default-value"));
            base = `((${base} == null || ${base} === "") ? ${fallback} : (${base}))`;
        }
        const type = attr(node, "type");
        if (type === "NewList") return "[]";
        if (type === "NewMap") return "{}";
        if (type) return `basicConvert(${base}, ${jsString(type)})`;
        return base;
    };

    const fieldExpr = (node) => {
        if (has(node, "from")) return attr(node, "from");
        if (has(node, "value")) return expandExpr(attr(node, "value"));
        return attr(node, "field-name");
    };

    // The `ignore` attribute is an expression: the condition is skipped when it is true (`ignore="!term"`).
    const ignoreExpr = (node) => has(node, "ignore") ? `!!(${attr(node, "ignore")})` : "false";

    const econditionFactory = (node) => {
        const fromExpr = has(node, "from") ? attr(node, "from") : fieldExpr(node);
        const value = has(node, "value") ? jsString(attr(node, "value")) : "null";
        const toField = has(node, "to-field-name") ? jsString(attr(node, "to-field-name")) : "null";
        return `ec.entity.conditionFactory.makeActionConditionDirect(${jsString(attr(node, "field-name"))}, ` +
            `${jsString(attr(node, "operator") || "equals")}, ${fromExpr}, ${value}, ${toField}, ` +
            `${attr(node, "ignore-case") === "true"}, ${attr(node, "ignore-if-empty") === "true"}, ` +
            `${attr(node, "or-null") === "true"}, ${ignoreExpr(node)})`;
    };

    const econditionSuffix = (node) => {
        const operator = attr(node, "operator") || "equals";
        const simple = operator === "equals" && attr(node, "ignore-case") !== "true" &&
            attr(node, "ignore-if-empty") !== "true" && attr(node, "or-null") !== "true" &&
            !has(node, "to-field-name") && !has(node, "ignore");
        if (simple) return `.condition(${jsString(attr(node, "field-name"))}, ${fieldExpr(node)})`;
        return `.condition(${econditionFactory(node)})`;
    };

    // The arguments shared by makeConditionDate, filterByDate and filterMapListByDate.
    const dateFilterArgs = (node) =>
        `${jsString(attr(node, "from-field-name") || "fromDate")}, ${jsString(attr(node, "thru-field-name") || "thruDate")}, ` +
        `${attrOrNull(node, "valid-date")}, ${attr(node, "ignore-if-empty") === "true"}`;

    const dateFilterExpr = (node) =>
        `ec.entity.conditionFactory.makeConditionDate(${dateFilterArgs(node)}, ${ignoreExpr(node)})`;

    const conditionObjectExpr = (node) => {
        const parts = node.children.map((kid) => {
            if (kid.name === "econdition") return econditionFactory(kid);
            if (kid.name === "date-filter") return dateFilterExpr(kid);
            if (kid.name === "econdition-object") return attr(kid, "field") || "null";
            // a nested econditions is a condition here, not the `.condition(...)` it is directly under a find
            if (kid.name === "econditions") return conditionObjectExpr(kid);
            return kid.source || "null";
        });
        return `ec.entity.conditionFactory.makeCondition([${parts.join(", ")}], ${jsString(attr(node, "combine") || "and")})`;
    };

    // A limit or offset: as written when it is a number, or expanded to text when it has ${}, as Java's
    // `ec.resource.expand(...) as Integer`; the host find reads the integer, and empty or non-numeric text is no limit.
    const countExpr = (text) => text.indexOf("$") >= 0 && text.charAt(text.length - 1) !== "$" ? expandExpr(text) : text;

    // ec.entity.find(...) through the options that entity-find-one and the find-by-condition elements share.
    const findHead = (node, distinct) => joinPresent([
        // An entity name with ${} is expanded, as the Groovy string of the Java template was.
        `ec.entity.find(${literalOrExpand(attr(node, "entity-name"))})`,
        has(node, "cache") && `.useCache(${attr(node, "cache")})`,
        has(node, "for-update") && `.forUpdate(${attr(node, "for-update")})`,
        distinct && has(node, "distinct") && `.distinct(${attr(node, "distinct")})`,
        has(node, "use-clone") && `.useClone(${attr(node, "use-clone")})`
    ]);

    const findCondition = (kid, cached) => {
        switch (kid.name) {
        case "date-filter":
            return cached ? "" : (kid.source || `.condition(${dateFilterExpr(kid)})`);
        case "econdition":
            return kid.source || econditionSuffix(kid);
        case "econditions":
            return kid.source || `.condition(${conditionObjectExpr(kid)})`;
        case "econdition-object":
            return attr(kid, "field") ? `.condition(${attr(kid, "field")})` : "";
        case "having-econditions":
            return kid.source || "";
        default:
            return "";
        }
    };

    const findPrefix = (node) => {
        const cached = attr(node, "cache") === "true";
        return joinPresent([
            findHead(node, true),
            has(node, "offset") && `.offset(${countExpr(attr(node, "offset"))})`,
            has(node, "limit") && `.limit(${countExpr(attr(node, "limit"))})`,
            childrenNamed(node, "select-field").map((kid) => `.selectField(${literalOrExpand(attr(kid, "field-name"))})`).join(""),
            // The fields a form-list shows, set by the screen on the copy of its entity-find (not a schema attribute).
            has(node, "form-select-fields") && `.selectFormFields(${jsString(attr(node, "form-select-fields"))})`,
            childrenNamed(node, "order-by").map((kid) => `.orderBy(${literalOrExpand(attr(kid, "field-name"))})`).join(""),
            node.children.map((kid) => findCondition(kid, cached)).join("")
        ]);
    };

    const searchFormLines = (listName, node) => {
        const input = has(node, "input-fields-map") ? attr(node, "input-fields-map") : "context";
        let defs = "null";
        const defaults = child(node, "default-parameters");
        if (defaults) {
            const attrs = defaults.attrs || {};
            const parts = Object.keys(attrs).map((key) => `${jsString(key)}: ${expandExpr(String(attrs[key]))}`);
            defs = `{${parts.join(", ")}}`;
        }
        const paginate = has(node, "paginate") ? attr(node, "paginate") : "true";
        // A find method gives the next find, so the variable takes it.
        const requireLine = has(node, "require-parameters")
            ? `${listName}_xafind = ${listName}_xafind.requireSearchFormParameters(${expandExpr(attr(node, "require-parameters"))} === "true");`
            : "";
        return js`
            if (true) {
                ${requireLine}
                ${listName}_xafind = ${listName}_xafind.searchFormMap(${input}, ${defs}, ${jsString(attr(node, "skip-fields"))}, ${jsString(attr(node, "default-order-by"))}, ${paginate});
            }
        `;
    };

    const conditionExpr = (node) => {
        const parts = [];
        if (has(node, "condition") && attr(node, "condition") !== "") parts.push(attr(node, "condition"));
        const cond = child(node, "condition");
        if (cond) {
            const inner = sourcesOf(cond.children);
            if (inner) parts.push(inner);
        }
        if (!parts.length) return "true";
        return parts.join(" && ");
    };

    const blockBody = (node) => {
        const skipped = ["condition", "then", "else", "else-if"];
        const direct = node.children.filter((kid) => !skipped.includes(kid.name));
        return bodyOf(direct) + childrenNamed(node, "then").map((kid) => bodyOf(kid.children)).join("");
    };

    const compareCall = (node) => {
        const op = has(node, "operator") ? jsString(attr(node, "operator")) : `"equals"`;
        const value = has(node, "value") ? literalOrExpand(attr(node, "value")) : "null";
        const toField = attrOrNull(node, "to-field");
        const format = has(node, "format") ? jsString(attr(node, "format")) : "null";
        const type = has(node, "type") ? jsString(attr(node, "type")) : `"Object"`;
        return `compare(${attr(node, "field")}, ${op}, ${value}, ${toField}, ${format}, ${type})`;
    };

    const messageCall = (error, isPublic, type, text) => {
        const expanded = expandExpr(text);
        if (error) return `ec.message.addError(${expanded});`;
        if (isPublic) return `ec.message.addPublic(${expanded}, ${jsString(type || "info")});`;
        return `ec.message.addMessage(${expanded}, ${jsString(type || "info")});`;
    };

    // Join the sources of an and/or/not group, or true when it is empty.
    const joinSources = (node, separator) => {
        const parts = node.children.map((kid) => kid.source || "true");
        return parts.length ? parts.join(separator) : "true";
    };

    // The loop over a list or map. The list is read from the context once. Each bare read of a list converts it.
    const iterateSource = (node) => {
        const list = attr(node, "list");
        const entry = attr(node, "entry");
        const key = attr(node, "key");
        const body = bodyOf(node.children);
        const local = `_${entry}_list`;
        let setup;
        let limit;
        let assign;
        if (key) {
            limit = `_${entry}Entries.length`;
            setup = `var _${entry}Entries = (typeof ${local}.entries === "function") ? Array.from(${local}.entries()) : Object.entries(${local});`;
            assign = js`
                var _${entry}Entry = _${entry}Entries[${entry}_index];
                ${key} = _${entry}Entry[0];
                ${entry} = _${entry}Entry[1];
            `;
        } else {
            limit = `${entry}_len`;
            setup = `var ${entry}_len = (typeof ${local}.length === "number") ? ${local}.length : 0;`;
            assign = `${entry} = (typeof ${local}.get === "function") ? ${local}.get(${entry}_index) : ${local}[${entry}_index];`;
        }
        return js`
            if (${list} != null) {
                var ${local} = ${list};
                var ${entry}_index = 0;
                ${setup}
                for (; ${entry}_index < ${limit}; ${entry}_index = ${entry}_index + 1) {
                    ${assign}
                    ${entry}_has_next = (${entry}_index + 1) < ${limit};
                    ${body}
                }
            }
        `;
    };

    const serviceCallChain = (node, isAsync) => {
        const inMap = attr(node, "in-map");
        const tx = attr(node, "transaction");
        const syncOnly = !isAsync;
        // multi="parameter" (the transition shortcut's default) is a multi call when the request says _isMulti, as a multi form-list posts.
        // A name with ${} is expanded, as the Groovy string of the Java template was.
        return joinPresent([
            `ec.service.${isAsync ? "async()" : "sync()"}.name(${literalOrExpand(attr(node, "name"))})`,
            syncOnly && attr(node, "disable-authz") === "true" && ".disableAuthz()",
            syncOnly && attr(node, "multi") === "true" && ".multi(true)",
            syncOnly && attr(node, "multi") === "parameter" && ".multi(!!(ec.web && ec.web.parameters && ec.web.parameters._isMulti === 'true'))",
            syncOnly && has(node, "transaction") && joinPresent([
                tx === "ignore" && ".ignoreTransaction(true)",
                (tx === "force-new" || tx === "force-cache") && ".requireNewTransaction(true)",
                has(node, "transaction-timeout") && `.transactionTimeout(${attr(node, "transaction-timeout")})`,
                (tx === "cache" || tx === "force-cache") ? ".useTransactionCache(true)" : ".useTransactionCache(false)"
            ]),
            // The whole context is ec.context, which the host reads from the stack instead of a copy made in script.
            (inMap === "true" || inMap === "context") && ".parameters(ec.context)",
            has(node, "in-map") && inMap !== "true" && inMap !== "context" && inMap !== "false" && inMap !== "" && `.parameters(${inMap})`,
            childrenNamed(node, "field-map")
                .map((kid) => `.parameter(${jsString(attr(kid, "field-name"))}, ${fieldExpr(kid)})`).join(""),
            ".call()"
        ]);
    };

    // A statement that calls a method on the value-field entity value.
    const valueCall = (call) => stmt((node) => `${attr(node, "value-field")}.${call};`);

    return {
        // Elements a parent macro reads directly render nothing themselves.
        ...noops([
            "condition", "then", "else", "else-if", "search-form-inputs", "default-parameters", "select-field",
            "order-by", "limit-range", "limit-view", "use-iterator", "field-map"
        ]),
        "actions": stmt((node) => unraw(js`
            (function() {
                ${bodyOf(node.children)}
                return;
            })()
        `)),
        "service-call": stmt((node) => {
            const name = attr(node, "name");
            const isAsync = has(node, "async") && attr(node, "async") !== "false";
            const call = serviceCallChain(node, isAsync);
            const outMap = attr(node, "out-map");
            let callLines;
            if (outMap && !isAsync) {
                // A context name read is a copy, so the merged map is assigned back.
                const add = !has(node, "out-map-add-to-existing") || attr(node, "out-map-add-to-existing") === "true";
                // `context` is the stack itself (the default out-map of a transition's service-call): each result goes on it by name, as
                // Java's context.putAll. A name the script host owns (ec, sri) is not replaced.
                const assign = outMap === "context"
                    ? js`
                        if (call_service_result != null) for (const mq_key of Object.keys(call_service_result))
                            if (!Object.prototype.hasOwnProperty.call(globalThis, mq_key)) globalThis[mq_key] = call_service_result[mq_key];
                    `
                    : add
                    ? `if (call_service_result != null) ${outMap} = Object.assign({}, ${outMap} || {}, call_service_result);`
                    : `${outMap} = call_service_result;`;
                callLines = js`
                    var call_service_result = ${call};
                    ${assign}
                `;
            } else callLines = `${call};`;
            // web-send-json-response="resultList" sends that out-parameter; "true" sends the result map; "false" (the default) sends nothing.
            const jsonAttr = attr(node, "web-send-json-response");
            const jsonResponse = jsonAttr === "false" ? "" : jsonAttr;
            if (jsonResponse && !isAsync && !(outMap && !isAsync)) {
                callLines = `var call_service_result = ${call};`;
            }
            const sendJson = !jsonResponse || isAsync
                ? ""
                : jsonResponse === "true"
                    ? "ec.web.sendJsonResponse(call_service_result);"
                    : `ec.web.sendJsonResponse(call_service_result == null ? null : call_service_result[${jsString(jsonResponse)}]);`;
            const errorLines = attr(node, "ignore-error") === "true"
                ? js`
                    if (ec.message.hasError()) {
                        ec.logger.warn(${jsString(`Ignoring error running service ${name}: `)} + ec.message.getErrorsString());
                        ec.message.clearErrors();
                    }
                `
                : "if (ec.message.hasError()) return;";
            // The JSON goes out before the error check, as in Java, so a failed call answers with its errors.
            return js`
                if (true) {
                    ${callLines}
                    ${sendJson}
                    ${errorLines}
                }
            `;
        }),
        "script": stmt((node) => {
            const location = has(node, "location") && attr(node, "location") !== ""
                ? `ec.resource.script(${jsString(attr(node, "location"))});`
                : "";
            return js`
                ${location}
                // begin inline script
                ${raw(node.text || "")}
                // end inline script
            `;
        }),
        "set": stmt((node) => {
            const field = attr(node, "field");
            const value = valueExpr(node);
            if (attr(node, "set-if-empty") === "false") {
                return js`
                    var _temp_internal = ${value};
                    if (_temp_internal != null && _temp_internal !== "") ${field} = _temp_internal;
                `;
            }
            return `${field} = ${value};`;
        }),
        "order-map-list": stmt((node) => {
            const fields = childrenNamed(node, "order-by").map((ob) => literalOrExpand(attr(ob, "field-name")));
            const list = attr(node, "list");
            // an entity list sorts only as a copy (see its prototype), so sort an array copy and set the name to it;
            // a plain array is sorted in place as Groovy does
            return js`
                if (${list} != null && !Array.isArray(${list})) {
                    var _listToOrder = Array.from(${list});
                    orderMapList(_listToOrder, [${fields.join(", ")}]);
                    ${list} = _listToOrder;
                } else orderMapList(${list}, [${fields.join(", ")}]);
            `;
        }),
        "filter-map-list": stmt((node) => {
            const list = attr(node, "list");
            // an entity list has no slice or splice, so the filter runs on an array copy. A context name holds a copy of
            // an array of entity values, so the name is set after the filter.
            const toList = `var _listToFilter = Array.from(${list});`;
            const assign = `${has(node, "to-list") ? attr(node, "to-list") : list} = _listToFilter;`;
            const maps = childrenNamed(node, "field-map");
            const fieldFilter = maps.length
                ? `filterMapList(_listToFilter, {${maps.map((kid) => `${jsString(attr(kid, "field-name"))}: ${fieldExpr(kid)}`).join(", ")}});`
                : "";
            const dateFilters = childrenNamed(node, "date-filter")
                .map((kid) => `filterMapListByDate(_listToFilter, ${dateFilterArgs(kid)});`).join("\n");
            return js`
                if (${list} != null) {
                    ${toList}
                    ${fieldFilter}
                    ${dateFilters}
                    ${assign}
                }
            `;
        }),
        "date-filter": chain((node) => `.condition(${dateFilterExpr(node)})`),
        "entity-data": stmt(() => `throw new Error("entity-data is not implemented");`),
        "entity-find-one": stmt((node) => {
            const maps = childrenNamed(node, "field-map");
            let autoCondition = "";
            if (has(node, "auto-field-map")) {
                const auto = attr(node, "auto-field-map");
                if (auto === "true") autoCondition = "context";
                else if (auto !== "false" && auto !== "") autoCondition = auto;
            } else if (!maps.length) autoCondition = "context";
            const find = joinPresent([
                findHead(node, false),
                autoCondition && `.condition(${autoCondition})`,
                maps.map((kid) => `.condition(${jsString(attr(kid, "field-name"))}, ${fieldExpr(kid)})`).join(""),
                childrenNamed(node, "select-field").map((kid) => `.selectField(${literalOrExpand(attr(kid, "field-name"))})`).join("")
            ]);
            return `${attr(node, "value-field")} = ${find}.one();`;
        }),
        "entity-find": stmt((node) => {
            const list = attr(node, "list");
            const cached = attr(node, "cache") === "true";
            const sfi = child(node, "search-form-inputs");
            const limitRange = child(node, "limit-range");
            const limitView = child(node, "limit-view");
            let run;
            if (limitRange && !cached) {
                run = `${list}_xafind.offset(${attr(limitRange, "start")}).limit(${attr(limitRange, "size")}).list()`;
            } else if (limitView && !cached) {
                const index = attr(limitView, "view-index");
                const size = attr(limitView, "view-size");
                run = `${list}_xafind.offset((${index} - 1) * ${size}).limit(${size}).list()`;
            } else run = `${list}_xafind.list()`;
            // A cached list cannot filter by date in the query, so it filters the result.
            const dateFilters = cached
                ? childrenNamed(node, "date-filter")
                    .map((kid) => `${list} = ${list}.filterByDate(${dateFilterArgs(kid)});`).join("\n")
                : "";
            // A paginated search sets the list's Count, PageIndex, PageSize, PageMaxIndex, PageRangeLow and PageRangeHigh, which the
            // form-list pagination reads (Java's XmlActions entity-find). The count of all rows runs only when the page is full.
            const paginate = sfi && attr(sfi, "paginate") !== "false" && !limitRange && !limitView;
            const pageLines = paginate ? js`
                var ${list}_xalimit = ${list}_xafind.getLimit();
                var ${list}_xaoffset = ${list}_xafind.getOffset() || 0;
                var ${list}_xacount = ${list}_xalimit == null ? null : ${list}_xafind.copy();
            ` : "";
            const pageAfter = paginate ? js`
                if (${list}_xalimit == null) {
                    ${list}Count = ${list}.length;
                    ${list}PageIndex = 0;
                    ${list}PageSize = ${list}Count > 20 ? ${list}Count : 20;
                } else {
                    ${list}PageSize = ${list}_xalimit;
                    ${list}PageIndex = ${list}_xalimit > 0 ? Math.floor(${list}_xaoffset / ${list}_xalimit) : 0;
                    ${list}Count = ${list}.length < ${list}PageSize ? ${list}.length + ${list}PageIndex * ${list}PageSize : ${list}_xacount.count();
                }
                ${list}PageMaxIndex = ${list}PageSize > 0 && ${list}Count > 0 ? Math.trunc((${list}Count - 1) / ${list}PageSize) : 0;
                ${list}PageRangeLow = ${list}PageIndex * ${list}PageSize + 1;
                ${list}PageRangeHigh = Math.min(${list}PageIndex * ${list}PageSize + ${list}PageSize, ${list}Count);
            ` : "";
            // The local temp holds the find; the list itself is a context name.
            return js`
                var ${list}_xafind = ${findPrefix(node)};
                ${sfi ? searchFormLines(list, sfi) : ""}
                ${pageLines}
                ${list} = ${run};
                ${dateFilters}
                ${pageAfter}
            `;
        }),
        "econditions": chain((node) => `.condition(${conditionObjectExpr(node)})`),
        "having-econditions": chain((node) => node.children.map((kid) => {
            if (kid.name === "econdition") return `.havingCondition(${econditionFactory(kid)})`;
            if (kid.name === "date-filter") return `.havingCondition(${dateFilterExpr(kid)})`;
            if (kid.name === "econditions") return `.havingCondition(${conditionObjectExpr(kid)})`;
            if (kid.name === "econdition-object" && attr(kid, "field")) return `.havingCondition(${attr(kid, "field")})`;
            return "";
        }).join("")),
        "econdition": chain((node) => econditionSuffix(node)),
        "econdition-object": chain((node) => {
            const field = attr(node, "field");
            return field ? `.condition(${field})` : "";
        }),
        "entity-find-count": stmt((node) => {
            const find = findPrefix(node);
            const sfi = child(node, "search-form-inputs");
            if (!sfi) return `${attr(node, "count-field")} = ${find}.count();`;
            return js`
                var _count_xafind = ${find};
                ${searchFormLines("_count", sfi)}
                ${attr(node, "count-field")} = _count_xafind.count();
            `;
        }),
        "entity-find-related-one": stmt((node) => {
            const value = attr(node, "value-field");
            return `${attr(node, "to-value-field")} = (${value} == null) ? null : ${value}.findRelatedOne(${jsString(attr(node, "relationship-name"))}, ${attrOrNull(node, "cache")}, ${attrOrNull(node, "for-update")});`;
        }),
        "entity-find-related": stmt((node) => {
            const value = attr(node, "value-field");
            return `${attr(node, "list")} = (${value} == null) ? null : ${value}.findRelated(${jsString(attr(node, "relationship-name"))}, ${attrOrNull(node, "map")}, ${attrOrNull(node, "order-by-list")}, ${attrOrNull(node, "cache")}, ${attrOrNull(node, "for-update")});`;
        }),
        "entity-make-value": stmt((node) => {
            const field = attr(node, "value-field");
            const setFields = has(node, "map") && attr(node, "map") !== ""
                ? `${field}.setFields(${attr(node, "map")}, true, null, null);`
                : "";
            return js`
                ${field} = ec.entity.makeValue(${literalOrExpand(attr(node, "entity-name"))});
                ${setFields}
            `;
        }),
        "entity-create": stmt((node) =>
            `${attr(node, "value-field")}.${attr(node, "or-update") === "true" ? "createOrUpdate()" : "create()"};`),
        "entity-update": valueCall("update()"),
        "entity-delete": valueCall("delete()"),
        "entity-delete-related": stmt((node) =>
            `${attr(node, "value-field")}.deleteRelated(${jsString(attr(node, "relationship-name"))});`),
        "entity-delete-by-condition": stmt((node) => `${findPrefix(node)}.deleteAll();`),
        "entity-set": stmt((node) => {
            const map = has(node, "map") ? attr(node, "map") : "context";
            const prefix = has(node, "prefix") && attr(node, "prefix") !== "" ? attr(node, "prefix") : "null";
            const include = attr(node, "include");
            const flag = include === "pk" ? "true" : (include === "nonpk" ? "false" : "null");
            return `${attr(node, "value-field")}.setFields(${map}, ${attr(node, "set-if-empty") === "true"}, ${prefix}, ${flag});`;
        }),
        "entity-sequenced-id-primary": valueCall("setSequencedIdPrimary()"),
        "entity-sequenced-id-secondary": valueCall("setSequencedIdSecondary()"),
        "break": stmt(() => "break;"),
        "continue": stmt(() => "continue;"),
        "iterate": stmt(iterateSource),
        "message": stmt((node) =>
            messageCall(attr(node, "error") === "true", attr(node, "public") === "true", attr(node, "type"), node.text || "")),
        "check-errors": stmt(() => "if (ec.message.hasError()) return;"),
        "return": stmt((node) => {
            const message = has(node, "message") && attr(node, "message") !== ""
                ? messageCall(attr(node, "error") === "true", attr(node, "public") === "true", attr(node, "type"), attr(node, "message"))
                : "";
            return js`
                ${message}
                return;
            `;
        }),
        "assert": stmt((node) => {
            const title = attr(node, "title");
            const prefix = title ? `[${title}] ` : "";
            return node.children.map((kid) => {
                const src = kid.source || "true";
                return `if (!(${src})) ec.message.addError(${expandExpr(`${prefix}Assert failed: ${unraw(src)}`)});`;
            }).join("\n");
        }),
        "if": stmt((node) => {
            const elseIfs = childrenNamed(node, "else-if").map((kid) => js`
                } else if (${conditionExpr(kid)}) {
                    ${blockBody(kid)}
            `).join("\n");
            const elses = childrenNamed(node, "else");
            const elseBlock = elses.length ? js`
                } else {
                    ${bodyOf(elses[0].children)}
            ` : "";
            return js`
                if (${conditionExpr(node)}) {
                    ${blockBody(node)}
                ${elseIfs}
                ${elseBlock}
                }
            `;
        }),
        "while": stmt((node) => js`
            while (${conditionExpr(node)}) {
                ${blockBody(node)}
            }
        `),
        "or": expr((node) => `(${joinSources(node, " || ")})`),
        "and": expr((node) => `(${joinSources(node, " && ")})`),
        "not": expr((node) => `!(${node.children.length ? (node.children[0].source || "true") : "true"})`),
        // A statement when it has a body or an else, otherwise the call as an expression.
        "compare": macro((node) => {
            const call = compareCall(node);
            const elseNode = child(node, "else");
            const body = bodyOf(node.children.filter((kid) => kid.name !== "else"));
            if (!body && !elseNode) return call;
            const elseBlock = elseNode ? js`
                } else {
                    ${bodyOf(elseNode.children)}
            ` : "";
            return js`
                if (${call}) {
                    ${body}
                ${elseBlock}
                }
            `;
        }),
        // A statement when it has children, otherwise the text as an expression.
        "expression": macro((node) => {
            const text = node.text || "";
            if (!node.children.length) return raw(text);
            return js`
                if (${text || "true"}) {
                    ${bodyOf(node.children)}
                }
            `;
        }),
        "log": stmt((node) => {
            const level = has(node, "level") && attr(node, "level") !== "" ? attr(node, "level") : "info";
            return `ec.logger.log(${jsString(level)}, ${expandExpr(attr(node, "message"))}, null);`;
        })
    };
})
