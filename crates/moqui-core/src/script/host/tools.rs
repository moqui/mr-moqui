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

//! The facade calls the tools screens and services make that are not part of the Server host methods: cache admin,
//! the entity and service model, screen statistics, the server status, the SQL runner, and the data writer and
//! loader builders. The functions are the hidden functions of the tools shim ([`SHIM`]) and return plain data;
//! `tools_js!` puts the Java-shaped methods on it (`getEntityDefinition(name).getPrettyName(...)`,
//! `workerPool.getActiveCount()`, `makeDataWriter().file(...)`).
//!
//! A method does not survive the context stack, which holds data, so each object also has its getters as data
//! (`prettyName`, `activeCount`), which is what a screen reads once it has `set` the object in a field.

use moqui_lib::node::MNode;
use moqui_lib::object::{HostObject, Value};

use super::{entity_facade, factory, method, with_ec, Call, HostError, Method, Ret, Shim};

macro_rules! tools_js {
    () => {
        r#"(function (ec, cu, su, wu, host) {
    // ---- collections ----
    // `context` is the stack; the page values are set on it by bare name, as the generated screen code reads them.
    // A list that is already a page, `{page}AlreadyPaginated` or a `{page}Count` from a paginated find (which sets the
    // range of the whole result), is the page as it is, as `paginateParameters` marks it and the screen's paginate does.
    cu.paginateList = function (listName, pageListName, context) {
        pageListName = pageListName || listName;
        const theList = Array.from(context[listName] || []);
        if (context[pageListName + "AlreadyPaginated"] != null || context[pageListName + "Count"] != null) {
            if (pageListName !== listName) globalThis[pageListName] = theList;
            return;
        }
        const values = Object.assign({}, context);
        if (values[pageListName + "PageRangeLow"] == null || values[pageListName + "PageRangeHigh"] == null) {
            cu.paginateParameters(theList.length, pageListName, values);
            for (const suffix of ["Count", "PageIndex", "PageSize", "PageMaxIndex", "PageRangeLow", "PageRangeHigh"])
                globalThis[pageListName + suffix] = values[pageListName + suffix];
        }
        const low = values[pageListName + "PageRangeLow"], high = values[pageListName + "PageRangeHigh"];
        globalThis[pageListName] = theList.slice(Math.max(low - 1, 0), high);
    };
    cu.orderMapList = function (list, fields) {
        orderMapList(list, fields);
        return list;
    };

    // ---- wrappers: a tools object is data with a `__wrap` name; reading it from the stack adds its methods back ----
    const wrappers = {};
    const wrap = (name, data) => { if (data == null) return null; data.__wrap = name; return wrappers[name](data); };
    globalThis.__moquiWrap = (name, data) => (wrappers[name] ? wrappers[name](data) : data);

    // An element written as MNode.write_string writes it, for `toString()` of an element below a parsed root.
    const escapeAttr = (value) => String(value).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");
    const writeXml = (node, level) => {
        const pad = "    ".repeat(level);
        let out = pad + "<" + node.name;
        for (const [key, value] of Object.entries(node.attributes)) out += " " + key + '="' + escapeAttr(value) + '"';
        const kids = node.childNodes;
        if (node.text != null || kids.length) {
            out += ">";
            if (node.text != null) out += "<![CDATA[" + String(node.text).split("]]>").join("]]]]><![CDATA[>") + "]]>";
            for (const kid of kids) out += "\n" + writeXml(kid, level + 1);
            if (kids.length > 1) out += "\n" + pad;
            return out + "</" + node.name + ">";
        }
        return out + "/>";
    };
    // the parts of MNode the screens call: `{name, attributes, text, children, xml}`. `host.parseNode` sends `xml` for the
    // root only; an element below it (`parsed`) is written when its `toString()` is first called.
    wrappers.Node = function (node) {
        node.attributes = node.attributes || {};
        // the child data is `childNodes`, since `children(name)` is the method
        if (Array.isArray(node.children)) node.childNodes = node.children;
        const parsed = node.xml != null || node.parsed === true;
        const all = (node.childNodes || []).map((kid) => {
            if (kid.__wrap) return kid;
            if (parsed) kid.parsed = true;
            return wrap("Node", kid);
        });
        node.childNodes = all;
        node.children = (name) => all.filter((kid) => name == null || kid.name === name);
        node.attribute = (name) => node.attributes[name] ?? null;
        node.first = (name) => all.find((kid) => name == null || kid.name === name) || null;
        node.hasChild = (name) => all.some((kid) => kid.name === name);
        node.toString = () => {
            if (node.xml == null && node.parsed === true) node.xml = writeXml(node, 0);
            return node.xml || "";
        };
        return node;
    };
    const makeNode = (data) => wrap("Node", data);
    // a rule or tree that a screen shows with `toString()`
    wrappers.Text = (data) => Object.assign(data, { toString: () => data.text || "" });
    const makeText = (text) => wrap("Text", { text: text });

    // ---- cache ----
    ec.cache.cacheExists = host.cacheExists;
    ec.cache.clearAllCaches = host.clearAllCaches;
    ec.cache.getAllCachesInfo = host.getAllCachesInfo;
    ec.cache.makeElementInfoList = host.makeElementInfoList;
    ec.cache.getLocalCache = ec.cache.getCache;
    ec.cache.getDistributedCache = ec.cache.getCache;

    // ---- entity ----
    ec.entity.isEntityDefined = host.isEntityDefined;
    ec.entity.clearEntityDefinitionFromCache = host.clearEntityDefinitionFromCache;
    ec.entity.getValueListFromPlainMap = host.valuesFromPlainMap;
    ec.entity.getAllEntityNames = host.getAllEntityNames;
    ec.entity.getAllEntitiesInfo = host.getAllEntitiesInfo;
    ec.entity.getAllEntityInfo = host.getAllEntityInfo;
    ec.entity.getDatasourceGroupNames = host.getDatasourceGroupNames;
    // The table check of the named entities (of the entity groups `groups` when given), whatever the groups'
    // add-missing settings say, after forgetting what is known of the tables: `{ddl: [the statements run], errors,
    // failed: [{entityName, error}]}`, a name that fails skipped. Tools' Check Tables.
    ec.entity.checkTables = host.checkTables;
    ec.entity.getEecaRuleCount = host.getEecaRuleCount;
    // statement stats of this instance when entity-facade@query-stats is on (moqui_entity::entity::query_stats)
    ec.entity.getQueryStatsList = host.getQueryStatsList;
    ec.entity.clearQueryStats = host.clearQueryStats;
    ec.entity.runSqlStatement = host.runSqlStatement;
    // another database, named by a map of `entity_ds_db_conf`, `entity_ds_host`, `entity_ds_port`, `entity_ds_database`,
    // `entity_ds_user` and `entity_ds_password`, as Java's `getConfConnection`
    ec.entity.testConfConnection = host.testConfConnection;
    ec.entity.runSqlCountConf = host.runSqlCountConf;
    ec.entity.runSqlUpdateConf = host.runSqlUpdateConf;
    ec.entity.eecaRulesByEntityName = {
        get: (entityName) => host.eecaRuleTexts(entityName).map((text) => ({ eecaNode: makeText(text) }))
    };
    // `RelationshipInfo`, with the calls of the auto screen menu
    wrappers.Relationship = (rel) => Object.assign(rel, {
        riRelatedEntityName: () => rel.relatedEntityName,
        riPrettyName: () => rel.prettyName,
        getTargetParameterMap: (values) => {
            const target = {};
            for (const [field, related] of Object.entries(rel.keyMap || {})) {
                const value = values == null ? null : (typeof values.get === "function" ? values.get(field) : values[field]);
                if (value != null && value !== "") target[related] = value;
            }
            return target;
        }
    });
    wrappers.EntityDefinition = function (ed) {
        const fieldByName = {};
        for (const field of ed.entityInfo.allFieldInfoArray) fieldByName[field.name] = field;
        const relationships = ed.relationshipsInfo;
        Object.assign(ed, {
            getFullEntityName: () => ed.fullEntityName,
            getPkFieldNames: () => ed.pkFieldNames.slice(),
            getAllFieldNames: () => ed.allFieldNames.slice(),
            getFieldNames: (pks, nonPks) => ed.entityInfo.allFieldInfoArray.filter((f) => (pks && f.isPk) || (nonPks && !f.isPk)).map((f) => f.name),
            isField: (name) => fieldByName[name] != null,
            isPkField: (name) => fieldByName[name] != null && fieldByName[name].isPk,
            getFieldInfo: (name) => fieldByName[name] || null,
            getColumnName: (name) => fieldByName[name] ? fieldByName[name].columnName : null,
            getPrettyName: (title, baseName) => host.prettyName(ed.entityName, title || null, baseName || null),
            getPrimaryKeys: (values) => {
                const pks = {};
                for (const name of ed.pkFieldNames) {
                    const value = values == null ? null : (typeof values.get === "function" ? values.get(name) : values[name]);
                    if (value != null && value !== "") pks[name] = value;
                }
                return pks;
            },
            getRelationshipsInfo: (dependentsOnly) => relationships.filter((rel) => !dependentsOnly || rel.dependent),
            getRelationshipInfo: (name) => relationships.find((rel) => rel.relationshipName === name || rel.shortAlias === name ||
                (rel.title == null && rel.relatedEntityName.endsWith("." + name))) || null,
            getMasterDefinitionMap: () => Object.fromEntries(ed.masterNames.map((name) => [name, { name: name }])),
            getEntityNode: () => ed.entityNode,
            getDependentsTree: () => Object.assign(makeText(ed.dependentsText), { allDescendants: ed.allDescendants })
        });
        return ed;
    };
    ec.entity.getEntityDefinition = function (entityName) {
        const info = host.entityDefinitionInfo(entityName);
        if (info == null) return null;
        for (const field of info.fields) field.fieldNode = makeNode({ name: "field", attributes: { name: field.name, type: field.type } });
        for (const rel of info.relationships) wrap("Relationship", rel);
        return wrap("EntityDefinition", {
            fullEntityName: info.fullEntityName, entityName: info.entityName, isViewEntity: info.isViewEntity,
            tableName: info.tableName, groupName: info.groupName, pkFieldNames: info.pkFieldNames,
            allFieldNames: info.allFieldNames, relationshipsInfo: info.relationships, masterNames: info.masterNames,
            entityNode: makeNode({ name: "entity", attributes: info.entityAttributes }),
            dependentsText: info.dependentsText, allDescendants: info.allDescendants,
            prettyName: host.prettyName(info.entityName, null, null),
            entityInfo: {
                internalEntityName: info.entityName, shortAlias: info.shortAlias, fullTableName: info.tableName,
                needsEncrypt: info.needsEncrypt, needsAuditLog: info.needsAuditLog, createOnly: info.createOnly,
                allFieldInfoArray: info.fields, allFieldInfoList: info.fields
            }
        });
    };
    // The model loads the reverse relationships with the definitions, so there is nothing to create.
    ec.entity.createAllAutoReverseManyRelationships = () => {};
    // `getAllEntityRelatedFields`: the fields of an entity and of each related entity, with whether a data view has them
    ec.entity.getAllEntityRelatedFields = function (entityName, orderByField, dbViewEntityName) {
        const ed = entityName ? ec.entity.getEntityDefinition(entityName) : null;
        if (ed == null) return [];
        const alias = (entityAlias, fieldName) => dbViewEntityName ? ec.entity.find("moqui.entity.view.DbViewEntityAlias")
            .condition({dbViewEntityName: dbViewEntityName, entityAlias: entityAlias, fieldName: fieldName}).one() : null;
        const out = [];
        for (const field of ed.entityInfo.allFieldInfoArray) {
            const aliasVal = alias("MASTER", field.name);
            out.push({entityName: ed.fullEntityName, fieldName: field.name, type: field.type, cardinality: "one",
                inDbView: aliasVal != null, functionName: aliasVal?.functionName ?? null});
        }
        for (const rel of ed.getRelationshipsInfo(false)) {
            const red = ec.entity.getEntityDefinition(rel.relatedEntityName);
            if (red == null) continue;
            const member = dbViewEntityName ? ec.entity.find("moqui.entity.view.DbViewEntityMember")
                .condition({dbViewEntityName: dbViewEntityName, entityName: red.fullEntityName}).list()[0] : null;
            for (const field of red.entityInfo.allFieldInfoArray) {
                const aliasVal = member ? alias(member.entityAlias, field.name) : null;
                out.push({entityName: rel.relatedEntityName, fieldName: field.name, type: field.type, cardinality: rel.type,
                    title: rel.title, inDbView: aliasVal != null, functionName: aliasVal?.functionName ?? null});
            }
        }
        if (orderByField) orderMapList(out, [orderByField]);
        return out;
    };
    ec.entity.makeDataWriter = function () {
        const ops = [];
        const w = {};
        const op = (name) => (value) => { ops.push([name, value]); return w; };
        for (const name of ["fileType", "entityName", "skipEntityName", "dependentLevels", "master", "prefix",
                "filterMap", "fromDate", "thruDate", "isoDateTime", "tableColumnNames"]) w[name] = op(name);
        w.entityNames = (names) => { for (const name of Array.from(names || [])) ops.push(["entityName", name]); return w; };
        w.skipEntityNames = (names) => { for (const name of Array.from(names || [])) ops.push(["skipEntityName", name]); return w; };
        w.allEntities = () => { ops.push(["allEntities", true]); return w; };
        w.dependentRecords = (flag) => { ops.push(["dependentLevels", flag ? 2 : 0]); return w; };
        w.orderBy = (fields) => { for (const field of Array.from(fields || [])) ops.push(["orderBy", field]); return w; };
        w.file = (filename) => host.writeData(ops, "file", filename, null).count;
        w.directory = (path) => host.writeData(ops, "directory", path, null).count;
        w.zipFile = (within, zipFilename) => host.writeData(ops, "zipFile", within, zipFilename).count;
        w.zipDirectory = (within, zipFilename) => {
            if (typeof zipFilename !== "string") throw new Error("zipDirectory to a stream is not available; write a zip file and send it as a resource");
            return host.writeData(ops, "zipDirectory", within, zipFilename).count;
        };
        // Java's writer(Writer): the text comes back, and the caller sends it.
        w.text = () => host.writeData(ops, "text", null, null).text;
        return w;
    };
    ec.entity.makeDataLoader = function () {
        const options = { dataTypes: [], componentNames: [] };
        const l = {};
        for (const name of ["xmlText", "csvText", "jsonText", "dummyFks", "useTryInsert", "onlyCreate",
                "disableEntityEca", "disableAuditLog", "disableFkCreate", "disableDataFeed", "transactionTimeout",
                "messageNoActionFiles", "restrictSensitiveEntities", "csvEntityName", "csvDelimiter", "csvCommentStart",
                "csvQuoteChar"])
            l[name] = (value) => { options[name] = value; return l; };
        // Java adds each location to a list; defaultValues adds to the map.
        options.locations = [];
        l.location = (location) => { if (location) options.locations.push(String(location)); return l; };
        l.locationList = (locations) => { options.locations.push(...Array.from(locations || []).map(String)); return l; };
        l.defaultValues = (values) => { options.defaultValues = Object.assign(options.defaultValues || {}, values || {}); return l; };
        // the fields of a CSV text with no header line
        l.csvFieldNames = (names) => { options.csvFieldNames = Array.from(names || []); return l; };
        l.dataTypes = (types) => { options.dataTypes.push(...Array.from(types || [])); return l; };
        l.componentNameList = (names) => { options.componentNames.push(...Array.from(names || [])); return l; };
        l.load = (messages) => {
            const result = host.loadData(options);
            if (Array.isArray(messages)) messages.push(...result.messages);
            return result.records;
        };
        // Java's check(): compare the data with the database and write nothing. With no argument the messages of
        // what did not match come back; check(messages) adds them to the list and returns the fields checked.
        l.check = (messages) => {
            const result = host.loadData(Object.assign({}, options, { check: true }));
            if (Array.isArray(messages)) { messages.push(...result.messages); return result.fieldsChecked; }
            return result.messages;
        };
        // Java's checkInfo(): a row for each record not found and each field that did not match. With no argument the
        // rows come back and the messages are info messages; checkInfo(diffInfoList, messages) adds to the lists and
        // returns the fields checked.
        l.checkInfo = (diffInfoList, messages) => {
            const result = host.loadData(Object.assign({}, options, { check: true, checkInfo: true }));
            if (Array.isArray(diffInfoList)) {
                diffInfoList.push(...result.diffInfo);
                if (Array.isArray(messages)) messages.push(...result.messages);
                return result.fieldsChecked;
            }
            for (const message of result.messages) ec.message.addMessage(message, "info");
            return result.diffInfo;
        };
        // Java's list(): the values of the records the data has, nested records after the record they are in, with
        // nothing written and no service called.
        l.list = () => host.loadData(Object.assign({}, options, { list: true })).values;
        return l;
    };

    // Java's `EntityDataDocument`: the documents as JSON to a writer, a new file or a directory of files.
    ec.entity.getEntityDataDocument = function () {
        const write = (mode, path, ids, condition, from, thru, pretty) => host.writeDataDocuments(mode,
            path == null ? "" : String(path), Array.from(ids || []).map(String), condition == null ? null : condition,
            from == null ? null : from, thru == null ? null : thru, pretty === true || String(pretty) === "true");
        return {
            getDataDocuments: (id, condition, from, thru) => ec.entity.getDataDocuments(id, condition, from, thru),
            writeDocumentsToWriter: (writer, ids, condition, from, thru, pretty) => {
                const result = write("text", null, ids, condition, from, thru, pretty);
                writer.write(result.text);
                return result.count;
            },
            writeDocumentsToFile: (path, ids, condition, from, thru, pretty) =>
                write("file", path, ids, condition, from, thru, pretty).count,
            writeDocumentsToDirectory: (path, ids, condition, from, thru, pretty) =>
                write("dir", path, ids, condition, from, thru, pretty).count,
        };
    };

    // Java's `ServiceCallSpecial`: a service run asynchronously when the transaction in place commits or rolls back.
    ec.service.special = function () {
        let name = null;
        const parameters = {};
        const special = {
            name: (a, b, c) => {
                name = c !== undefined ? `${a}.${b}#${c}` : b !== undefined ? `${a}#${b}` : String(a);
                return special;
            },
            parameters: (map) => { Object.assign(parameters, map || {}); return special; },
            parameter: (key, value) => { parameters[key] = value; return special; },
            registerOnCommit: () => host.registerServiceOnEnd(name, parameters, true),
            registerOnRollback: () => host.registerServiceOnEnd(name, parameters, false),
        };
        return special;
    };

    // ---- service ----
    ec.service.getKnownServiceNames = host.getKnownServiceNames;
    Object.defineProperty(ec.service, "knownServiceNames", { get: host.getKnownServiceNames });
    ec.service.getAllServiceInfo = host.getAllServiceInfo;
    ec.service.getSecaRuleCount = host.getSecaRuleCount;
    ec.service.secaRulesByServiceName = {
        get: (serviceName) => { const texts = host.secaRuleTexts(serviceName); return texts.length ? texts.map(makeText) : null; }
    };
    wrappers.ServiceDefinition = (info) => Object.assign(info, {
        getInParameter: (name) => info.inParameterInfoMap[name]?.parameterNode ?? null,
        convertValidateCleanParameters: (parameters) => host.convertValidateCleanParameters(info.serviceName, parameters)
    });
    ec.service.getServiceDefinition = function (serviceName) {
        const info = host.serviceDefinitionInfo(serviceName);
        if (info == null) return null;
        info.serviceNode = makeNode(host.parseNode(info.serviceXml));
        for (const param of [...Object.values(info.inParameterInfoMap), ...Object.values(info.outParameterInfoMap)]) {
            param.parameterNode = makeNode(host.parseNode(param.parameterXml));
            delete param.parameterXml;
        }
        return wrap("ServiceDefinition", info);
    };
    wrappers.JobRunner = (info) => Object.assign(info, {
        getLastExecuteTime: () => info.lastExecuteTime, getExecuteCount: () => info.executeCount,
        getTotalJobsRun: () => info.totalJobsRun, getLastJobsActive: () => info.lastJobsActive,
        getLastJobsPaused: () => info.lastJobsPaused
    });
    ec.service.getJobRunner = () => wrap("JobRunner", host.jobRunnerInfo());
    // One pool runs service calls and jobs in this port, so the worker pool and the job pool are the same.
    wrappers.WorkerPool = (info) => Object.assign(info, {
        getActiveCount: () => info.activeCount, getPoolSize: () => info.poolSize, getMaximumPoolSize: () => info.maximumPoolSize,
        getCorePoolSize: () => info.poolSize,
        queue: { size: () => info.queueSize, remainingCapacity: () => info.remainingCapacity }
    });
    const poolInfo = () => {
        const info = host.workerPoolInfo();
        return wrap("WorkerPool", {
            activeCount: info.active, poolSize: info.workers, maximumPoolSize: info.workers, queueSize: info.queued,
            remainingCapacity: Math.max(info.queueCap - info.queued, 0)
        });
    };
    Object.defineProperty(ec.service, "jobWorkerPool", { get: poolInfo });

    // ---- screen ----
    ec.screen.getAllRootScreenLocations = host.getAllRootScreenLocations;
    ec.screen.getTransitionInfo = host.transitionInfo;
    wrappers.ScreenInfo = (info) => Object.assign(info, { getIndentedName: () => info.indentedName });
    ec.screen.getScreenInfoList = function (rootLocation, levels) {
        return host.getScreenInfoList(rootLocation, levels).map((info) =>
            wrap("ScreenInfo", Object.assign(info, { indentedName: "- ".repeat(info.level) + " " + info.name })));
    };

    // ---- resources: the parts of ResourceReference the tools screens and services call ----
    // A version of a `dbresource://` or `git://` file: `{versionName, previousVersionName, versionDate, userId, note}`,
    // with Java's `Version` methods; `location` is kept for `getText`, `getPreviousVersion` and `getNextVersions`. `note`
    // is what was written with the version: the commit message on git, `versionNote` on the database tree.
    wrappers.ResourceVersion = (version) => Object.assign(version, {
        getVersionName: () => version.versionName, getPreviousVersionName: () => version.previousVersionName,
        getVersionDate: () => version.versionDate, getUserId: () => version.userId, getNote: () => version.note,
        getText: () => host.resourceVersionText(version.location, String(version.versionName)),
        getPreviousVersion: () => version.previousVersionName == null ? null
            : makeVersion(host.resourceVersion(version.location, "named", String(version.previousVersionName)), version.location),
        getNextVersions: () => host.resourceNextVersions(version.location, String(version.versionName))
            .map((next) => makeVersion(next, version.location)),
        getMap: () => ({ versionName: version.versionName, previousVersionName: version.previousVersionName,
            versionDate: version.versionDate, userId: version.userId, note: version.note })
    });
    const makeVersion = (version, location) => {
        if (version == null) return null;
        Object.defineProperty(version, "location", { value: location, enumerable: false });
        return wrap("ResourceVersion", version);
    };
    // `childOfLocation` is the directory `findChildFile` found the file under, kept as data for `getActualChildPath`.
    const childOf = (childRef, directoryRef) => { if (childRef != null) childRef.childOfLocation = directoryRef.location; return childRef; };
    const joinPath = (location, path) => location.replace(/\/$/, "") + (path.charAt(0) === "/" ? "" : "/") + path;
    wrappers.Resource = function (ref) {
        // size, lastModified and children are read the first time a script asks for them
        if (!("size" in ref)) {
            let stat = ref.exists ? null : {};
            const statOf = () => stat ??= (host.resourceStat(ref.location) || {});
            Object.defineProperty(ref, "size", { get: () => statOf().size ?? null, enumerable: true, configurable: true });
            Object.defineProperty(ref, "lastModified", { get: () => statOf().lastModified ?? null, enumerable: true,
                configurable: true });
        }
        if (ref.directory && !("children" in ref)) {
            let children = null;
            Object.defineProperty(ref, "children", { get: () => children ??= host.resourceEntries(ref.location),
                enumerable: false, configurable: true });
        }
        Object.assign(ref, {
            getLocation: () => ref.location, getFileName: () => ref.fileName, getExists: () => ref.exists,
            isDirectory: () => ref.directory, isFile: () => ref.file, getSize: () => ref.size,
            isText: () => ref.text, isBinary: () => ref.binary, getContentType: () => ref.contentType,
            getLastModified: () => ref.lastModified, length: () => ref.size || 0, getName: () => ref.fileName,
            getDirectoryEntries: () => host.resourceEntries(ref.location).map((child) => wrap("Resource", child)),
            // null for a file that is not there, as Java's (the stream is null)
            getInputStream: () => ref.openStream(),
            // a reference of `location#versionName` reads that version, as getLocationText does
            getText: function (versionName) {
                if (versionName) return host.resourceVersionText(ref.location, String(versionName));
                if (ref.versionName && host.resourceSupportsVersion(ref.location))
                    return host.resourceVersionText(ref.location, String(ref.versionName));
                const info = host.resourceInfo(ref.location);
                return info != null && info.exists && info.file ? host.resourceText(ref.location) : null;
            },
            // a note (optional) is kept with the version the write makes: the commit message on git, versionNote on the
            // database tree (docs/data/Revision-Management.md, Version notes)
            putText: (text, note) => note == null ? host.resourcePutText(ref.location, String(text))
                : ec.resource.putContent(ref.location, String(text), note),
            putBytes: (bytes, note) => note == null ? host.resourcePutBytes(ref.location, bytes)
                : ec.resource.putContent(ref.location, bytes, note),
            // Java's `putStream(fileItem.getInputStream())` and `putStream(other.openStream())`
            putUpload: (fileItem) => fileItem.write(ref.location),
            // `putStream(stream)`: the stream of `fileItem.getInputStream()` or `openStream()`, or the bytes of a Uint8Array.
            putStream: function (stream, note) {
                if (stream == null) throw new Error("putStream needs a stream");
                if (note != null) ec.resource.putContent(ref.location, stream, note);
                else if (typeof stream.write === "function") stream.write(ref.location);
                else host.resourcePutBytes(ref.location, stream);
            },
            // `openStream()` and `getInputStream()`: the file's bytes as a stream; null for a file that is not there. A
            // reference of `location#versionName` streams that version (null when it is not there)
            openStream: function () {
                if (ref.versionName && host.resourceSupportsVersion(ref.location))
                    return host.resourceVersionStream(ref.location, String(ref.versionName));
                const info = host.resourceInfo(ref.location);
                return info != null && info.exists && info.file ? host.resourceStream(ref.location) : null;
            },
            copyFrom: (source) => host.resourceCopy(typeof source === "string" ? source : source.location, ref.location),
            delete: () => host.resourceDelete(ref.location),
            // nothing to move is not an error, as in Java (a page often has no matching directory)
            move: function (newLocation) {
                const info = host.resourceInfo(ref.location);
                if (info != null && info.exists) host.resourceMove(ref.location, String(newLocation));
            },
            supportsVersion: () => host.resourceSupportsVersion(ref.location),
            getVersion: (versionName) => makeVersion(host.resourceVersion(ref.location, "named", String(versionName)), ref.location),
            getCurrentVersion: () => makeVersion(host.resourceVersion(ref.location, "current"), ref.location),
            getRootVersion: () => makeVersion(host.resourceVersion(ref.location, "root"), ref.location),
            getVersionHistory: () => host.resourceVersionHistory(ref.location).map((version) => makeVersion(version, ref.location)),
            getNextVersions: (versionName) => host.resourceNextVersions(ref.location, String(versionName))
                .map((version) => makeVersion(version, ref.location)),
            // the line diff between two versions (an empty name is the current text): {fromName, toName, unifiedDiff,
            // html, stats: {insertions, deletions, hunks}, hunks}
            getVersionDiff: (versionNameLhs, versionNameRhs) => host.resourceVersionDiff(ref.location,
                versionNameLhs == null ? "" : String(versionNameLhs), versionNameRhs == null ? "" : String(versionNameRhs)),
            // the directory named as this file without its extensions, or this directory
            findMatchingDirectory: function () {
                if (ref.directory) return ref;
                let dirLoc = ref.location, directoryRef = ref;
                while (!(directoryRef.exists && directoryRef.directory) && dirLoc.lastIndexOf(".") > 0) {
                    dirLoc = dirLoc.substring(0, dirLoc.lastIndexOf("."));
                    directoryRef = makeRef(dirLoc);
                }
                return directoryRef;
            },
            getChild: (childName) => childName ? makeRef(joinPath(ref.findMatchingDirectory().location, String(childName))) : null,
            getChildren: function () {
                const directoryRef = ref.findMatchingDirectory();
                if (directoryRef == null || !directoryRef.exists) return null;
                return directoryRef.getDirectoryEntries().filter((child) => child.file);
            },
            findChildFile: function (relativePath) {
                if (!relativePath) return ref;
                relativePath = String(relativePath);
                const directoryRef = ref.findMatchingDirectory();
                let childRef = null;
                if (directoryRef.exists) {
                    const theFile = makeRef(joinPath(directoryRef.location, relativePath));
                    if (theFile.exists && theFile.file) childRef = theFile;
                    if (childRef == null) {
                        // not at the literal path: search the directory tree for the file name
                        const lastSlash = relativePath.lastIndexOf("/");
                        const directoryPath = lastSlash > 0 ? relativePath.substring(0, lastSlash) : "";
                        const childFilename = lastSlash >= 0 ? relativePath.substring(lastSlash + 1) : relativePath;
                        childRef = findFileUnder(directoryRef.findChildDirectory(directoryPath), childFilename, new Set());
                    }
                    if (childRef != null) return childOf(childRef, directoryRef);
                    // still nothing: the literal path, which does not exist
                    return childOf(makeRef(directoryRef.location + "/" + relativePath), directoryRef);
                }
                let newDirectoryLoc = ref.location;
                const lastSlash = newDirectoryLoc.lastIndexOf("/");
                const dot = newDirectoryLoc.indexOf(".", lastSlash);
                if (dot >= 0) newDirectoryLoc = newDirectoryLoc.substring(0, dot);
                return makeRef(newDirectoryLoc + "/" + relativePath);
            },
            findChildDirectory: function (relativePath) {
                if (!relativePath) return ref;
                relativePath = String(relativePath);
                let childDirectoryRef = ref.file ? ref.findMatchingDirectory() : ref;
                for (const name of relativePath.split("/")) {
                    childDirectoryRef = findDirectoryUnder(childDirectoryRef, name, new Set());
                    if (childDirectoryRef == null) break;
                }
                if (childDirectoryRef != null) return childDirectoryRef;
                let newDirectoryLoc = ref.location;
                if (ref.file) {
                    const lastSlash = newDirectoryLoc.lastIndexOf("/");
                    const dot = newDirectoryLoc.indexOf(".", lastSlash);
                    if (dot >= 0) newDirectoryLoc = newDirectoryLoc.substring(0, dot);
                }
                return makeRef(newDirectoryLoc + "/" + relativePath);
            },
            getActualChildPath: function () {
                const parentLocation = ref.childOfLocation;
                if (parentLocation == null || !ref.location.startsWith(parentLocation)) return null;
                const childPath = ref.location.substring(parentLocation.length);
                return childPath.startsWith("/") ? childPath.substring(1) : childPath;
            },
            // `{path, name, location}` of every file under this one (through matching directories) into the flat list,
            // and as a tree with `childResourceList` into the other; either may be null
            walkChildTree: function (allChildFileFlatList, childResourceList) {
                if (ref.file) walkFileTree(ref, "", allChildFileFlatList, childResourceList);
                if (ref.directory) for (const childRef of ref.getDirectoryEntries())
                    walkFileTree(childRef, "", allChildFileFlatList, childResourceList);
            }
        });
        Object.defineProperty(ref, "currentVersion", { get: ref.getCurrentVersion, configurable: true });
        Object.defineProperty(ref, "directoryEntries", { get: ref.getDirectoryEntries, configurable: true });
        return ref;
    };
    // a directory with this name under `directoryRef`, directly or anywhere below it
    const findDirectoryUnder = function (directoryRef, childDirName, visited) {
        if (directoryRef == null || !directoryRef.exists) return null;
        if (!childDirName) return directoryRef;
        const directRef = makeRef(joinPath(directoryRef.location, childDirName));
        if (directRef != null && directRef.exists) return directRef;
        visited.add(directoryRef.location);
        for (const childRef of directoryRef.getDirectoryEntries()) {
            if (!childRef.directory || visited.has(childRef.location)) continue;
            if (childRef.fileName === childDirName || childRef.fileName.includes(childDirName + ".")) return childRef;
            const subRef = findDirectoryUnder(childRef, childDirName, visited);
            if (subRef != null) return subRef;
        }
        return null;
    };
    // a file with this name, or this name and an extension, under `directoryRef`, files before subdirectories
    const findFileUnder = function (directoryRef, childFilename, visited) {
        if (directoryRef == null || !directoryRef.exists) return null;
        const exactRef = directoryRef.getChild(childFilename);
        if (exactRef != null && exactRef.file && exactRef.exists) return exactRef;
        visited.add(directoryRef.location);
        const entries = directoryRef.getDirectoryEntries();
        for (const childRef of entries)
            if (childRef.file && (childRef.fileName === childFilename || childRef.fileName.startsWith(childFilename + "."))) return childRef;
        for (const childRef of entries) {
            if (!childRef.directory || visited.has(childRef.location)) continue;
            const subRef = findFileUnder(childRef, childFilename, visited);
            if (subRef != null) return subRef;
        }
        return null;
    };
    const walkFileTree = function (fileRef, pathFromRoot, allChildFileFlatList, childResourceList) {
        if (!fileRef.file) return;
        const childPathBase = pathFromRoot ? pathFromRoot + "/" : "";
        let curFileName = fileRef.fileName;
        if (curFileName.includes(".")) curFileName = curFileName.substring(0, curFileName.lastIndexOf("."));
        const curPath = childPathBase + curFileName;
        const curChildResourceList = [];
        if (allChildFileFlatList != null) allChildFileFlatList.push({ path: curPath, name: curFileName, location: fileRef.location });
        if (childResourceList != null) childResourceList.push({ path: curPath, name: curFileName, location: fileRef.location,
            childResourceList: curChildResourceList });
        const matchingDirRef = fileRef.findMatchingDirectory();
        if (!matchingDirRef.exists || !matchingDirRef.directory) return;
        const childPath = childPathBase + matchingDirRef.fileName;
        for (const childRef of matchingDirRef.getDirectoryEntries())
            walkFileTree(childRef, childPath, allChildFileFlatList, curChildResourceList);
    };
    // `location#versionName` (a past version, as get#ResourceInfo of FileServices gives it): the reference of the file,
    // with versionName, which getText() and openStream() read
    const makeRef = (location) => {
        const info = host.resourceInfo(location);
        const hash = location.indexOf('#');
        if (info != null && hash > 0 && hash + 1 < location.length) info.versionName = location.substring(hash + 1);
        return wrap("Resource", info);
    };
    // `ResourceReference.isTextFilename(filename)` (static): the file name's content type is text
    ec.resource.isTextFilename = (filename) => host.isTextFilename(filename == null ? "" : String(filename));
    globalThis.ResourceReference = { isTextFilename: ec.resource.isTextFilename };
    ec.resource.getLocationReference = (location) => makeRef(String(location));
    // Java's getUriReference takes a URI; a location is one here
    ec.resource.getUriReference = (uri) => makeRef(String(uri));
    // Java's getLocationStream: the file as a stream, null for a file that is not there
    ec.resource.getLocationStream = (location) => makeRef(String(location)).openStream();
    // the line diff between two texts, as getVersionDiff returns it: {fromName, toName, unifiedDiff, html, stats, hunks}
    ec.resource.textDiff = (oldText, newText, fromName, toName) => host.textDiff(oldText == null ? "" : String(oldText),
        newText == null ? "" : String(newText), fromName == null ? "old" : String(fromName),
        toName == null ? "new" : String(toName));
    // CommonMark as HTML, unsanitized: a caller showing it to others cleans it
    ec.resource.markdownHtml = (text) => host.markdownHtml(text == null ? "" : String(text));
    // digests and archives read as streams (archive_host.rs): checksums of exports, operator-supplied import archives
    ec.resource.digest = (location, algorithm) => host.resourceDigest(String(location), String(algorithm || "sha256"));
    ec.resource.digestText = (text, algorithm) => host.textDigest(String(text ?? ""), String(algorithm || "sha256"));
    ec.resource.gunzip = (source, target, maxBytes) => host.resourceGunzip(String(source), String(target),
        maxBytes == null ? 4 * 1024 * 1024 * 1024 : Number(maxBytes));
    ec.resource.extractArchive = (source, target, limits) => host.resourceExtractArchive(String(source), String(target), limits || {});
    ec.resource.writeTarGz = (target, entries) => host.resourceWriteTarGz(String(target), Array.from(entries || []));
    ec.resource.readLines = (location, offset, maxLines, maxBytes) => host.resourceReadLines(String(location), Number(offset || 0),
        Number(maxLines || 1000), Number(maxBytes || 16 * 1024 * 1024));
    // a second argument that is a charset name (a Rust-port extension; Java's is the cache flag) decodes the bytes in it
    ec.resource.getLocationText = (location, cacheOrCharset) => typeof cacheOrCharset === "string" && cacheOrCharset.trim()
        ? host.resourceTextIn(String(location), cacheOrCharset) : host.resourceText(String(location));
    // the registry token of a Docker image (InstanceImage.authTokenCmd): the command's first output line, no shell
    ec.resource.runTokenCommand = (command) => host.runTokenCommand(String(command));

    // ---- forms: the field names a form has, for the DB form response services ----
    // A DbForm is read as of a date (now by default), with its drafts when includeDrafts (db_form.rs).
    ec.screen.getFormNode = function (location, asOfDate, includeDrafts) {
        location = String(location);
        if (location.startsWith("DbForm#")) {
            return makeNode(host.parseNode(host.dbFormXml(location.substring(7), asOfDate == null ? null : asOfDate,
                !!includeDrafts)));
        }
        return makeNode(host.parseNode(host.formXml(location)));
    };
    // ---- REST client: `ec.service.rest()`, the parts of Java's RestClient scripts call ----
    const restQuery = (parameters) => parameters.filter(([, value]) => value != null)
        .map(([name, value]) => encodeURIComponent(name) + "=" + encodeURIComponent(String(value))).join("&");
    function RestResponse(client, sent) {
        this.statusCode = sent.statusCode;
        this.reasonPhrase = sent.reasonPhrase;
        this.client = client;
        this.body = sent.text;
        this.bytesValue = sent.bytes;
        // a body `send()` left on the server, read by the first text() or bytes(), or passed on by response.forward
        this.bodyId = sent.bodyId ?? null;
        this.contentLength = sent.length ?? null;
        // the headers by lower case name, each with the list of its values, and as sent in order
        this.headerMap = {};
        this.headerPairs = sent.headers || [];
        for (const [name, value] of this.headerPairs) (this.headerMap[name.toLowerCase()] ||= []).push(value);
    }
    RestResponse.prototype.loadBody = function () {
        if (this.bodyId == null) return;
        const read = host.restReadBody(this.bodyId);
        this.bodyId = null;
        this.body = read.text;
        this.bytesValue = read.bytes;
    };
    // the body as received, a Uint8Array: use it for a body that is not text, such as a label image
    RestResponse.prototype.bytes = function () { this.loadBody(); return this.bytesValue; };
    RestResponse.prototype.headerList = function (name) { return (this.headerMap[String(name).toLowerCase()] || []).slice(); };
    RestResponse.prototype.headerFirst = function (name) { return (this.headerMap[String(name).toLowerCase()] || [null])[0]; };
    RestResponse.prototype.getContentType = function () { return this.headerFirst("content-type"); };
    RestResponse.prototype.getHeaders = function () { return this.headerMap; };
    RestResponse.prototype.getStatusCode = function () { return this.statusCode; };
    RestResponse.prototype.getReasonPhrase = function () { return this.reasonPhrase; };
    RestResponse.prototype.text = function () { this.loadBody(); return this.body; };
    RestResponse.prototype.jsonObject = function () {
        this.loadBody();
        try { return JSON.parse(this.body); }
        catch (e) { throw new Error("Error parsing JSON response from request to " + this.client.uriString + ": " + e.message); }
    };
    function RestUriBuilder(client) {
        this.client = client; this.protocolText = "http"; this.hostText = null; this.portNumber = 80; this.pathText = "";
        this.parameterList = [];
    }
    RestUriBuilder.prototype.protocol = function (protocol) {
        if (!protocol) throw new Error("Empty protocol not allowed");
        this.protocolText = String(protocol); return this;
    };
    RestUriBuilder.prototype.host = function (host) {
        if (!host) throw new Error("Empty host not allowed");
        this.hostText = String(host); return this;
    };
    RestUriBuilder.prototype.port = function (port) {
        port = Number(port);
        if (!(port > 0)) throw new Error("Invalid port " + port);
        this.portNumber = port; return this;
    };
    RestUriBuilder.prototype.path = function (pathEl) {
        if (pathEl == null || pathEl === "") return this;
        pathEl = String(pathEl);
        this.pathText += (pathEl.startsWith("/") ? "" : "/") + pathEl;
        if (this.pathText.endsWith("/")) this.pathText = this.pathText.substring(0, this.pathText.length - 1);
        return this;
    };
    RestUriBuilder.prototype.parameter = function (name, value) { this.parameterList.push([String(name), value]); return this; };
    RestUriBuilder.prototype.parameters = function (map) {
        for (const [name, value] of Object.entries(map || {})) this.parameterList.push([name, value]);
        return this;
    };
    RestUriBuilder.prototype.build = function () {
        if (!this.hostText) throw new Error("No host specified, call the host() method before build()");
        const query = restQuery(this.parameterList);
        return this.client.uri(this.protocolText + "://" + this.hostText + ":" + this.portNumber + (this.pathText || "/") +
            (query ? "?" + query : ""));
    };
    function RestClient() {
        this.methodName = "GET"; this.uriString = null; this.headerList = []; this.bodyText = null;
        this.bodyParameterList = []; this.contentTypeText = "application/json"; this.timeoutSeconds = 30;
        this.bodyBinary = null; this.bodyMultipart = null; this.bodyFromRequest = false; this.followRedirectsFlag = true; this.denyInternalFlag = false;
    }
    RestClient.prototype.method = function (method) { this.methodName = String(method).toUpperCase(); return this; };
    RestClient.prototype.uri = function (uri) {
        if (arguments.length === 0) return new RestUriBuilder(this);
        this.uriString = String(uri); return this;
    };
    RestClient.prototype.getUri = function () { return this.uriString; };
    RestClient.prototype.getUriString = function () { return this.uriString; };
    RestClient.prototype.addHeader = function (name, value) { this.headerList.push([String(name), String(value)]); return this; };
    RestClient.prototype.addHeaders = function (map) {
        for (const [name, value] of Object.entries(map || {})) this.addHeader(name, value);
        return this;
    };
    RestClient.prototype.contentType = function (contentType) { this.contentTypeText = String(contentType); return this; };
    RestClient.prototype.text = function (bodyText) { this.bodyText = bodyText == null ? null : String(bodyText); return this; };
    RestClient.prototype.jsonObject = function (body) {
        if (body == null) { this.bodyText = null; return this; }
        return this.text(typeof body === "string" ? body : JSON.stringify(body));
    };
    // As Java's (Jetty `request.param`), body parameters go in the query string.
    RestClient.prototype.addBodyParameter = function (name, value) { this.bodyParameterList.push([String(name), value]); return this; };
    RestClient.prototype.addBodyParameters = function (map) {
        for (const [name, value] of Object.entries(map || {})) this.bodyParameterList.push([name, value]);
        return this;
    };
    RestClient.prototype.timeout = function (seconds) { this.timeoutSeconds = Number(seconds); return this; };
    // A binary body, a Uint8Array, sent as it is; the Content-Type is the caller's header.
    RestClient.prototype.bodyBytes = function (bytes) { this.bodyBinary = bytes == null ? null : bytes; this.bodyText = null; return this; };
    // Rebuild multipart from server-owned uploads and allow-listed text fields. No browser/session headers are copied.
    RestClient.prototype.multipart = function (parts) { this.bodyMultipart = parts; this.bodyFromRequest = false; this.bodyText = null; this.bodyBinary = null; return this; };
    // The body of the web request being handled, sent as it came without passing through the script (a proxy).
    RestClient.prototype.streamFrom = function () { this.bodyFromRequest = true; this.bodyText = null; this.bodyBinary = null; return this; };
    // false passes a redirect back as the 3xx response instead of following it; only send() reads it.
    RestClient.prototype.followRedirects = function (follow) { this.followRedirectsFlag = follow !== false; return this; };
    // Refuse a host that is or resolves to an internal address, redirects included: for a URL from outside, such as a
    // mail's link. Only call() reads it; send() refuses it rather than going out without the check.
    RestClient.prototype.denyInternal = function () { this.denyInternalFlag = true; return this; };
    // As call(), with the headers only as given (no Content-Type or Accept added) and the body of the response left on
    // the server until text() or bytes() reads it, or response.forward(up) sends it on to the client as it comes.
    RestClient.prototype.send = function () {
        let uri = this.uriString;
        if (!uri) throw new Error("No URI set in RestClient");
        if (this.denyInternalFlag) throw new Error("RestClient send() does not support denyInternal(), use call()");
        const query = restQuery(this.bodyParameterList);
        if (query) uri += (uri.includes("?") ? "&" : "?") + query;
        const sent = host.restOpen({ method: this.methodName, uri: uri, headers: this.headerList.slice(), body: this.bodyText,
            bodyBytes: this.bodyBinary, multipart: this.bodyMultipart, bodyFromRequest: this.bodyFromRequest, followRedirects: this.followRedirectsFlag,
            timeoutSeconds: this.timeoutSeconds });
        return new RestResponse(this, sent);
    };
    RestClient.prototype.call = function () {
        if (this.bodyMultipart) return this.send();
        let uri = this.uriString;
        if (!uri) throw new Error("No URI set in RestClient");
        const query = restQuery(this.bodyParameterList);
        if (query) uri += (uri.includes("?") ? "&" : "?") + query;
        const headers = this.headerList.slice();
        const hasHeader = (name) => headers.some(([key]) => key.toLowerCase() === name);
        if (this.bodyText && !hasHeader("content-type")) headers.push(["Content-Type", this.contentTypeText]);
        if (!hasHeader("accept")) headers.push(["Accept", this.contentTypeText]);
        const sent = host.restSend({ method: this.methodName, uri: uri, headers: headers, body: this.bodyText,
            timeoutSeconds: this.timeoutSeconds, denyInternal: this.denyInternalFlag });
        return new RestResponse(this, sent);
    };
    ec.service.rest = () => new RestClient();
    ec.service.isLastRunBeforeLastSchedule = host.isLastRunBeforeLastSchedule;
    ec.service.nextCronExecution = host.nextCronExecution;
    // (expression, locale, shortFormat): English text only, from the cron parser
    ec.service.getCronDescription = (expression) => host.cronDescription(String(expression));
    ec.service.restApi = { getFreshRootResources: host.restRootResources, getRootResources: host.restRootResources,
        resolveService: host.resolveRestService };
    ec.entity.getAllEntityNamesWithMaster = host.getAllEntityNamesWithMaster;

    // `ScreenUrlInfo.isPermitted(ec, null, AUTHZA_ALL)` as a check of one artifact: the screen, the type, the action.
    ec.artifactExecution = { isPermitted: host.isPermitted, getFindFiltersForUser: host.getFindFiltersForUser,
        // Java's: disable returns whether entity ECA was already disabled, and the caller that found it so leaves it so
        disableEntityEca: host.disableEntityEca, enableEntityEca: host.enableEntityEca,
        entityEcaDisabled: host.entityEcaDisabled, getStackNameString: host.stackNameString,
        // Java's: disable returns whether authorization was already disabled; it stays so until enable
        disableAuthz: host.disableAuthz, enableAuthz: host.enableAuthz,
        // body() as the current user, with authorization on whatever disabled it (a ServiceJob, an async call): the
        // screen at location checked for all actions and pushed under it, as a request to it would be
        runAsUser: (location, body) => {
            let result;
            host.runAsUser(String(location), body, () => { result = body(); });
            return result;
        },
        // body() with no user and authorization off, sharing the transaction; only from a service's own code
        runAsSystem: (body) => {
            let result;
            host.runAsSystem(body, () => { result = body(); });
            return result;
        },
        // whether authorization is disabled now, changing nothing (disableAuthz to ask would hold a guard of its own)
        authzDisabled: host.authzDisabled,
        // Java's: disable returns whether tarpit counting was already disabled; it stays so until enable
        disableTarpit: host.disableTarpit, enableTarpit: host.enableTarpit,
        // Java's stack: each frame as {name, typeEnumId, actionEnumId}, top first. No history is kept here: what ran
        // is in the telemetry traces (docs/tools/Monitoring.md).
        getStack: host.artifactStack, getStackArray: host.artifactStack,
        peek: () => host.artifactStack()[0] ?? null,
        // Java's push(name, type, action, requiresAuthz): the frame is checked as any is, and throws when the user may
        // not; the caller pops it. The type and action are Java's enum ids (AT_OTHER, AUTHZA_VIEW) or the short forms.
        push: (name, type, action, requiresAuthz) => host.artifactPush(String(name), type == null ? null : String(type),
            action == null ? null : String(action), requiresAuthz !== false),
        pop: host.artifactPop,
        // Java's: what runs under the top frame is authorized for an anonymous user, for every action or for view
        setAnonymousAuthorizedAll: () => host.artifactAnonymous(true),
        setAnonymousAuthorizedView: () => host.artifactAnonymous(false) };

    // `Locale.getAvailableLocales()` with English display names. Java lists every JVM locale; the port has no locale
    // database, so this is a fixed list of the common ones.
    const localeNames = { ar: "Arabic", cs: "Czech", da: "Danish", de: "German", de_AT: "German (Austria)",
        de_CH: "German (Switzerland)", de_DE: "German (Germany)", el: "Greek", en: "English", en_AU: "English (Australia)",
        en_CA: "English (Canada)", en_GB: "English (United Kingdom)", en_IN: "English (India)", en_US: "English (United States)",
        es: "Spanish", es_ES: "Spanish (Spain)", es_MX: "Spanish (Mexico)", fi: "Finnish", fr: "French", fr_CA: "French (Canada)",
        fr_FR: "French (France)", he: "Hebrew", hi: "Hindi", hu: "Hungarian", it: "Italian", ja: "Japanese", ko: "Korean",
        nl: "Dutch", no: "Norwegian", pl: "Polish", pt: "Portuguese", pt_BR: "Portuguese (Brazil)", ro: "Romanian",
        ru: "Russian", sv: "Swedish", th: "Thai", tr: "Turkish", uk: "Ukrainian", vi: "Vietnamese", zh: "Chinese",
        zh_CN: "Chinese (China)", zh_TW: "Chinese (Taiwan)" };
    ec.l10n.getAvailableTimeZones = host.getAvailableTimeZones;
    // `formatFieldString(entityName, fieldName, text)`: Java converts the text to the field's type and formats it with
    // the user's locale; the audit log keeps the text, so it is shown as stored here.
    ec.entity.formatFieldString = (entityName, fieldName, text) => text == null ? "" : String(text);
    ec.l10n.getAvailableLocales = () => Object.entries(localeNames).map(([locale, name]) => ({ locale: locale, name: name }));

    // ---- the execution context's shared map: one per execution context, whatever the context stack holds ----
    // A proxy over the host's map, so `sharedMap[key]`, `sharedMap[key] = value`, `delete sharedMap[key]`, `key in
    // sharedMap`, and `Object.keys(sharedMap)` work. A value is copied in and out, so change an object in it by
    // assigning the key again.
    const sharedMap = new Proxy({}, {
        get: (target, key) => typeof key === "symbol" ? undefined : host.sharedGet(key),
        set: (target, key, value) => { if (typeof key !== "symbol") host.sharedSet(key, value); return true; },
        deleteProperty: (target, key) => { if (typeof key !== "symbol") host.sharedDelete(key); return true; },
        has: (target, key) => typeof key !== "symbol" && host.sharedHas(key),
        ownKeys: () => host.sharedKeys(),
        getOwnPropertyDescriptor: (target, key) => typeof key !== "symbol" && host.sharedHas(key)
            ? { value: host.sharedGet(key), enumerable: true, configurable: true, writable: true } : undefined
    });
    // on the stack's methods (`ec.context` is the stack itself, where an assignment would set a name)
    const contextMethods = globalThis.__moquiContextMethods;
    if (contextMethods) {
        Object.defineProperty(contextMethods, "sharedMap", { get: () => sharedMap, enumerable: false, configurable: true });
        contextMethods.getSharedMap = () => sharedMap;
    }

    // ---- user and messages ----
    // `getPeriodRange(baseName, inputs)`: from `{base}_period`, `_poffset`, and `_pdate`, else `_from` and `_thru`.
    // `getPeriodRange(period, poffset, pdate)`, Java's other form: the range itself, from a date (epoch millis or
    // yyyy-MM-dd) or today.
    ec.user.getPeriodRange = function (baseName, inputs, pdate) {
        if (arguments.length >= 3 || (inputs != null && typeof inputs !== "object"))
            return host.periodRange(String(baseName), String(inputs ?? ""), String(pdate ?? ""));
        inputs = inputs || {};
        const period = inputs[baseName + "_period"];
        if (period) return host.periodRange(String(period), String(inputs[baseName + "_poffset"] ?? ""), String(inputs[baseName + "_pdate"] ?? ""));
        return [inputs[baseName + "_from"] || null, inputs[baseName + "_thru"] || null];
    };

    // ---- factory and web ----
    ec.factory.getStatusMap = host.getStatusMap;
    ec.factory.getComponentInfoList = host.getComponentInfoList;
    // Java's getComponentBaseLocations: each component's location by its name, in load order
    ec.factory.getComponentBaseLocations = () => {
        const out = {};
        for (const info of host.getComponentInfoList()) out[info.name] = info.location;
        return out;
    };
    // the factory outlives every script it runs
    ec.factory.isDestroyed = () => false;
    // Java's factory-level facades: the same objects as ec's here, as there is one context per thread of work
    for (const name of ["entity", "service", "l10n", "resource", "cache", "screen", "logger"]) {
        Object.defineProperty(ec.factory, name, { get: () => ec[name], enumerable: false, configurable: true });
    }
    // What the server reported when it became ready (moqui_core::startup): the listeners, integrations and tools.
    ec.factory.getStartupReport = host.getStartupReport;
    ec.factory.getRuntimePath = host.getRuntimePath;
    // This server's instance id, as its telemetry and the database audit name it (Tools' Database › Queries).
    ec.factory.getInstanceId = host.getInstanceId;
    Object.defineProperty(ec.factory, "workerPool", { get: poolInfo });
    Object.defineProperty(ec.factory, "versionMap", { get: () => ({ moqui: ec.factory.moquiVersion }) });
    ec.factory.getThreadList = host.getThreadList;
    ec.factory.waitWorkerPoolEmpty = host.waitWorkerPoolEmpty;
    ec.web.getRequestDetails = host.getRequestDetails;
    Object.defineProperty(ec.web, "secureRequestParameters", { get: host.secureRequestParameters });
    su.camelCaseToPretty = host.camelCaseToPretty;
    su.prettyToCamelCase = host.prettyToCamelCase;
    // `new URI(null, null, filename, null).toASCIIString()`: a path with the characters a URI does not allow escaped
    su.encodeAsciiFilename = (filename) => encodeURI(String(filename ?? ""));
    // Java's `masker(mask, placeholder)`: a `MaskFormatter` whose value holds no literal characters, null for no mask
    su.masker = (mask, placeholder) => (mask == null || String(mask) === "") ? null
        : wrap("MaskFormatter", { mask: String(mask), placeholder: placeholder == null ? "" : String(placeholder) });
    // `valueToString(value)`: each mask character takes the next value character (`#` digit, `U` and `L` a letter made
    // upper or lower case, `A` letter or digit, `?` letter, `H` hex digit, `*` any), a literal is written as it is
    // (`'` quotes the next one), and a value that runs out is filled with the placeholder. An invalid character throws.
    wrappers.MaskFormatter = (data) => Object.assign(data, {
        valueToString: (value) => {
            const text = value == null ? "" : String(value), mask = data.mask, placeholder = data.placeholder;
            const checks = { '#': /\p{Nd}/u, U: /\p{L}/u, L: /\p{L}/u, A: /[\p{L}\p{Nd}]/u, "?": /\p{L}/u, H: /[0-9a-fA-F]/, "*": /[^]/ };
            let out = "", index = 0;
            for (let i = 0; i < mask.length; i++) {
                const kind = mask[i];
                if (kind === "'") { if (++i < mask.length) out += mask[i]; continue; }
                if (!checks[kind]) { out += kind; continue; }
                if (index >= text.length) {
                    out += placeholder.length > 1 ? (placeholder[index] ?? " ") : (placeholder || " ");
                } else {
                    const ch = text[index];
                    if (!checks[kind].test(ch)) throw new Error(`Invalid character: ${ch}`);
                    out += kind === "U" ? ch.toUpperCase() : kind === "L" ? ch.toLowerCase() : ch;
                }
                index++;
            }
            return out;
        },
        toString: () => data.mask,
    });
    // Java's `getYearList(years)`: this year and the ones after it, as text
    su.getYearList = (years) => {
        const start = new Date().getFullYear();
        return Array.from({ length: Math.max(Number(years) || 0, 0) }, (_, i) => String(start + i));
    };
    // Entities the generic tools (auto screens, data edit, data import) do not take from a request entity name
    wu.isRestrictedGenericEntity = host.isRestrictedGenericEntity;
})"#
    };
}

/// The tools shim: the tools methods across `ec` and the helper objects (`tools_js!`), then `ec.web.request` and
/// `ec.web.response` (`web.rs`), `ec.service.passAuthority` (`pass.rs`) and the drive calls on `ec.resource`
/// (`files.rs`), each over the hidden functions of all five files.
pub(super) static SHIM: Shim = Shim {
    js: concat!(
        "(function (global, hidden, helpers) {\n    const ec = global.ec;\n    (",
        tools_js!(),
        ")(ec, helpers.CollectionUtilities, helpers.StringUtilities, helpers.WebUtilities, hidden);\n    (",
        super::web::web_js!(),
        ")(ec, hidden);\n    (",
        super::pass::pass_js!(),
        ")(ec, hidden);\n    (",
        super::files::files_js!(),
        ")(ec, hidden);\n    (",
        super::context::context_js!(),
        ")(ec, hidden, global);\n})"
    ),
    hidden: &[
        METHODS,
        super::web::METHODS,
        super::archive::METHODS,
        super::pass::METHODS,
        super::files::METHODS,
        super::context::METHODS,
    ],
};

/// The tools functions of the shim.
static METHODS: &[Method] = &[
    method!("tools", "cacheExists", [Text], cache_exists),
    method!("tools", "clearAllCaches", [], clear_all_caches),
    method!("tools", "getAllCachesInfo", [OptValue, OptValue], get_all_caches_info),
    method!("tools", "makeElementInfoList", [Text, OptValue], make_element_info_list),
    method!("tools", "isEntityDefined", [OptValue], is_entity_defined),
    method!(
        "tools",
        "clearEntityDefinitionFromCache",
        [OptValue],
        clear_entity_definition_from_cache
    ),
    method!("tools", "getAllEntityNames", [OptValue], get_all_entity_names),
    method!(
        "tools",
        "getAllEntitiesInfo",
        [OptValue, OptValue, OptValue, OptValue],
        get_all_entities_info
    ),
    method!("tools", "getAllEntityInfo", [OptValue, OptValue], get_all_entity_info),
    method!("tools", "entityDefinitionInfo", [OptValue], entity_definition_info),
    method!(
        "tools",
        "prettyName",
        [Text, OptValue, OptValue],
        None,
        leaf,
        pretty_name
    ),
    method!("tools", "getDatasourceGroupNames", [], get_datasource_group_names),
    method!("tools", "checkTables", [Texts, OptTexts], check_tables),
    method!("tools", "getEecaRuleCount", [], get_eeca_rule_count),
    method!(
        "tools",
        "getQueryStatsList",
        [OptValue, OptValue, OptValue],
        get_query_stats_list
    ),
    method!("tools", "clearQueryStats", [], clear_query_stats),
    method!("tools", "eecaRuleTexts", [Text], eeca_rule_texts),
    method!("tools", "runSqlStatement", [Text, Text, OptValue], run_sql_statement),
    method!("tools", "writeData", [Value, Text, OptValue, OptValue], write_data),
    method!("tools", "loadData", [Value], load_data),
    method!(
        "tools",
        "writeDataDocuments",
        [Text, Text, Texts, Value, OptValue, OptValue, OptValue],
        Release,
        write_data_documents
    ),
    method!(
        "tools",
        "registerServiceOnEnd",
        [Text, Value, Bool],
        register_service_on_end
    ),
    method!("tools", "getKnownServiceNames", [], get_known_service_names),
    method!("tools", "getAllServiceInfo", [OptValue], get_all_service_info),
    method!("tools", "getSecaRuleCount", [], get_seca_rule_count),
    method!("tools", "secaRuleTexts", [Text], seca_rule_texts),
    method!("tools", "serviceDefinitionInfo", [OptValue], service_definition_info),
    method!(
        "tools",
        "convertValidateCleanParameters",
        [Text, Value],
        convert_validate_clean_parameters
    ),
    method!("tools", "parseNode", [Text], None, leaf, parse_node),
    method!("tools", "jobRunnerInfo", [], job_runner_info),
    method!("tools", "workerPoolInfo", [], worker_pool_info),
    method!("tools", "getAllRootScreenLocations", [], get_all_root_screen_locations),
    method!("tools", "getScreenInfoList", [Text, OptValue], get_screen_info_list),
    method!("tools", "getStatusMap", [OptValue], get_status_map),
    method!("tools", "getComponentInfoList", [], get_component_info_list),
    method!("tools", "getStartupReport", [], get_startup_report),
    method!("tools", "getRuntimePath", [], get_runtime_path),
    method!("tools", "getInstanceId", [], get_instance_id),
    method!(
        "tools",
        "runAsUser",
        [Text, Callback, Callback],
        None,
        leaf,
        run_as_user
    ),
    method!("tools", "runAsSystem", [Callback, Callback], None, leaf, run_as_system),
    method!("tools", "getThreadList", [], get_thread_list),
    method!("tools", "waitWorkerPoolEmpty", [OptNumber], wait_worker_pool_empty),
    method!("tools", "getRequestDetails", [], get_request_details),
    method!("tools", "secureRequestParameters", [], secure_request_parameters),
    method!(
        "tools",
        "camelCaseToPretty",
        [OptValue],
        None,
        leaf,
        camel_case_to_pretty
    ),
    method!(
        "tools",
        "isRestrictedGenericEntity",
        [OptValue],
        is_restricted_generic_entity
    ),
    method!(
        "tools",
        "getAvailableTimeZones",
        [],
        None,
        leaf,
        get_available_time_zones
    ),
    method!("tools", "resourceInfo", [Text], resource_info),
    method!("tools", "resourceStat", [Text], resource_stat),
    method!("tools", "resourceVersionStream", [Text, Text], resource_version_stream),
    method!("tools", "restRootResources", [], rest_root_resources),
    method!("tools", "resolveRestService", [Text, Text], resolve_rest_service),
    method!("tools", "transitionInfo", [Text, Text, Text, Text], transition_info),
    method!("tools", "valuesFromPlainMap", [Value, Text], values_from_plain_map),
    method!("tools", "isPermitted", [Text, OptText, OptText], is_permitted),
    method!("tools", "disableEntityEca", [], disable_entity_eca),
    method!("tools", "enableEntityEca", [], enable_entity_eca),
    method!("tools", "artifactStack", [], None, leaf, artifact_stack),
    method!("tools", "artifactPush", [Text, OptValue, OptValue, OptValue], artifact_push),
    method!("tools", "artifactPop", [], artifact_pop),
    method!("tools", "artifactAnonymous", [Value], None, leaf, artifact_anonymous),
    method!("tools", "disableAuthz", [], disable_authz),
    method!("tools", "enableAuthz", [], enable_authz),
    method!("tools", "authzDisabled", [], authz_disabled),
    method!("tools", "disableTarpit", [], disable_tarpit),
    method!("tools", "enableTarpit", [], enable_tarpit),
    method!("tools", "entityEcaDisabled", [], entity_eca_disabled),
    method!("tools", "stackNameString", [], stack_name_string),
    method!("tools", "sharedGet", [Text], shared_get),
    method!("tools", "sharedSet", [Text, Value], shared_set),
    method!("tools", "sharedDelete", [Text], shared_delete),
    method!("tools", "sharedHas", [Text], shared_has),
    method!("tools", "sharedKeys", [], shared_keys),
    method!("tools", "getFindFiltersForUser", [Text], get_find_filters_for_user),
    method!(
        "tools",
        "getAllEntityNamesWithMaster",
        [],
        get_all_entity_names_with_master
    ),
    method!("tools", "periodRange", [Text, Text, Text], period_range),
    method!("tools", "resourceText", [Text], resource_text),
    method!("tools", "resourceTextIn", [Text, Text], resource_text_in),
    method!("tools", "resourcePutText", [Text, Text], resource_put_text),
    method!("tools", "resourceStream", [Text], resource_stream),
    method!("tools", "isTextFilename", [Text], None, leaf, is_text_filename),
    method!("tools", "resourcePutBytes", [Text, Value], resource_put_bytes),
    method!("tools", "resourceDelete", [Text], resource_delete),
    method!("tools", "resourceEntries", [Text], resource_entries),
    method!("tools", "resourceMove", [Text, Text], resource_move),
    method!("tools", "resourceCopy", [Text, Text], resource_copy),
    method!("tools", "resourceSupportsVersion", [Text], resource_supports_version),
    method!("tools", "resourceVersion", [Text, Text, OptText], resource_version),
    method!("tools", "resourceVersionHistory", [Text], resource_version_history),
    method!("tools", "resourceNextVersions", [Text, Text], resource_next_versions),
    method!("tools", "resourceVersionText", [Text, Text], resource_version_text),
    method!(
        "tools",
        "resourceVersionDiff",
        [Text, Text, Text],
        resource_version_diff
    ),
    method!("tools", "textDiff", [Text, Text, Text, Text], None, leaf, text_diff),
    method!("tools", "markdownHtml", [Text], None, leaf, markdown_html),
    method!("tools", "restSend", [Value], rest_send),
    method!("tools", "runTokenCommand", [Text], run_token_command),
    method!("tools", "testConfConnection", [Value], test_conf_connection),
    method!("tools", "runSqlCountConf", [Text, Text, Value], run_sql_count_conf),
    method!("tools", "runSqlUpdateConf", [Text, Value], run_sql_update_conf),
    method!("tools", "formXml", [Text], form_xml),
    method!("tools", "dbFormXml", [Text, Value, Bool], db_form_xml),
    method!(
        "tools",
        "isLastRunBeforeLastSchedule",
        [Text, OptValue, OptValue, OptValue],
        is_last_run_before_last_schedule
    ),
    method!("tools", "nextCronExecution", [Text, OptValue], next_cron_execution),
    method!("tools", "cronDescription", [Text], None, leaf, cron_description),
    method!(
        "tools",
        "prettyToCamelCase",
        [OptValue, OptValue],
        None,
        leaf,
        pretty_to_camel_case
    ),
];

fn list(rows: Vec<moqui_lib::object::ValueMap>) -> Ret {
    Ret::Value(Value::List(rows.into_iter().map(Value::Map).collect()))
}

fn texts(texts: Vec<String>) -> Ret {
    texts.into()
}

/// The plain text of an optional argument, empty for none (runtime/convert.rs `text_or_empty`).
fn text_of(value: Option<&Value>) -> String {
    match value {
        None | Some(Value::Null) => String::new(),
        Some(value) => moqui_lib::object::to_plain_string(value),
    }
}

/// The truthiness of an optional argument, false for none.
fn flag_of(value: Option<&Value>) -> bool {
    value.is_some_and(super::truthy)
}

/// A count argument: a finite number not below zero, else `default`.
fn count_of(value: Option<&Value>, default: usize) -> usize {
    #[allow(clippy::cast_precision_loss)]
    let number = match value {
        Some(Value::I32(number)) => f64::from(*number),
        Some(Value::I64(number)) => *number as f64,
        Some(Value::F64(number)) => *number,
        _ => return default,
    };
    if !number.is_finite() || number < 0.0 {
        return default;
    }
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // Checked finite and not negative.
    let count = number as usize;
    count
}

/// A timestamp argument as epoch millis: none for null or none, or for a value that is not a moment.
fn millis_of(value: Option<&Value>) -> Option<i64> {
    let value = value.filter(|value| !value.is_null())?;
    match crate::l10n::basic_convert(value, "Timestamp").value {
        Value::Timestamp(stamp) => Some(stamp.as_millisecond()),
        _ => None,
    }
}

// ---- cache ----

fn cache_info_row(info: &moqui_lib::cache::CacheInfo) -> moqui_lib::object::ValueMap {
    let number = |value: u64| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    let mut row = indexmap::IndexMap::default();
    row.insert("name".to_string(), Value::string(&info.name));
    row.insert("expireTimeIdle".to_string(), Value::I64(info.expire_idle_ms / 1000));
    row.insert("expireTimeLive".to_string(), Value::I64(info.expire_live_ms / 1000));
    row.insert("maxElements".to_string(), number(info.max_entries as u64));
    row.insert("evictionStrategy".to_string(), Value::string("LRU"));
    row.insert("size".to_string(), number(info.size as u64));
    row.insert("getCount".to_string(), number(info.stats.gets));
    row.insert("putCount".to_string(), number(info.stats.puts));
    row.insert("hitCount".to_string(), number(info.stats.hits));
    row.insert("missCountTotal".to_string(), number(info.stats.misses));
    row.insert("evictionCount".to_string(), number(info.stats.evictions));
    row.insert("removeCount".to_string(), number(info.stats.removals));
    row.insert("expireCount".to_string(), number(info.stats.expires));
    row
}

/// Every cache the admin screen lists: the conf caches, the screen caches, and the entity list caches.
fn all_cache_infos() -> Result<Vec<moqui_lib::cache::CacheInfo>, HostError> {
    let factory = factory()?;
    let mut infos = factory
        .cache()
        .infos()
        .map_err(|err| HostError::message(err.message()))?;
    infos.extend(crate::screen::ScreenFacade::of(&factory).cache_infos());
    if let Some(facade) = factory.entity() {
        infos.extend(crate::entity::cache::list_cache_infos(&facade));
    }
    Ok(infos)
}

fn cache_exists(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    Ok(Ret::from(all_cache_infos()?.iter().any(|info| info.name == name)))
}

/// `clearAllCaches`: the conf caches, the screen definitions, and the entity lists.
fn clear_all_caches(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    factory
        .cache()
        .clear_all()
        .map_err(|err| HostError::message(err.message()))?;
    crate::screen::ScreenFacade::of(&factory).clear();
    if let Some(facade) = factory.entity() {
        crate::entity::cache::clear_all_list_caches(&facade).map_err(HostError::Lib)?;
    }
    Ok(Ret::Undefined)
}

fn get_all_caches_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let order_by = call.value(0).cloned();
    let filter = call.value(1).cloned();
    let filter = text_of(filter.as_ref());
    let mut rows: Vec<_> = all_cache_infos()?
        .iter()
        .filter(|info| crate::entity::info::name_matches(&info.name, &filter))
        .map(cache_info_row)
        .collect();
    order_maps(&mut rows, &text_of(order_by.as_ref()));
    Ok(list(rows))
}

/// Order maps by one field, `-` descending, numbers by value and the rest as text.
fn order_maps(rows: &mut [moqui_lib::object::ValueMap], order_by: &str) {
    let descending = order_by.starts_with('-');
    let field = order_by.trim_start_matches(['-', '+', '^']);
    if field.is_empty() {
        return;
    }
    rows.sort_by(|left, right| {
        let ordering = match (left.get(field), right.get(field)) {
            (Some(Value::I64(left)), Some(Value::I64(right))) => left.cmp(right),
            (left, right) => {
                let text = |value: Option<&Value>| value.map(moqui_lib::object::to_plain_string).unwrap_or_default();
                text(left).cmp(&text(right))
            }
        };
        if descending {
            ordering.reverse()
        } else {
            ordering
        }
    });
}

fn make_element_info_list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let order_by = call.value(1).cloned();
    let factory = factory()?;
    let found = factory
        .cache()
        .get(&name)
        .map_err(|err| HostError::message(err.message()))?;
    let stamp = |millis: i64| jiff::Timestamp::from_millisecond(millis).map_or(Value::Null, Value::Timestamp);
    let mut rows: Vec<_> = found
        .map(|cache| cache.entry_infos())
        .unwrap_or_default()
        .into_iter()
        .map(|entry| {
            let mut row = indexmap::IndexMap::default();
            row.insert("key".to_string(), Value::string(entry.key));
            row.insert("value".to_string(), entry.value);
            row.insert(
                "hitCount".to_string(),
                Value::I64(i64::try_from(entry.hit_count).unwrap_or(i64::MAX)),
            );
            row.insert("creationTime".to_string(), stamp(entry.created));
            if entry.updated > 0 {
                row.insert("lastUpdateTime".to_string(), stamp(entry.updated));
            }
            if entry.accessed > 0 {
                row.insert("lastAccessTime".to_string(), stamp(entry.accessed));
            }
            row
        })
        .collect();
    order_maps(&mut rows, &text_of(order_by.as_ref()));
    Ok(list(rows))
}

// ---- entity ----

fn is_entity_defined(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.value(0).cloned();
    let facade = entity_facade()?;
    Ok(Ret::from(crate::entity::info::is_entity_defined(
        &facade,
        &text_of(name.as_ref()),
    )))
}

/// `clearEntityDefinitionFromCache`, which the data view edit screen calls after it writes a view's rows: the
/// definitions built from rows are read again on their next use. Java drops the one named; this drops them all.
fn clear_entity_definition_from_cache(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    entity_facade()?.clear_derived_definitions();
    Ok(Ret::Undefined)
}

fn get_all_entity_names(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let filter = call.value(0).cloned();
    let facade = entity_facade()?;
    Ok(texts(crate::entity::info::all_entity_names(
        &facade,
        &text_of(filter.as_ref()),
    )))
}

fn get_all_entities_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let order_by = call.value(0).cloned();
    let filter = call.value(1).cloned();
    let master_only = call.value(2).cloned();
    let exclude_views = call.value(3).cloned();
    let facade = entity_facade()?;
    let rows = crate::entity::info::all_entities_info(
        &facade,
        &text_of(order_by.as_ref()),
        &text_of(filter.as_ref()),
        flag_of(master_only.as_ref()),
        flag_of(exclude_views.as_ref()),
    );
    Ok(list(rows))
}

fn get_all_entity_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let levels = call.value(0).cloned();
    let exclude_views = call.value(1).cloned();
    let facade = entity_facade()?;
    let rows =
        crate::entity::info::all_entity_info(&facade, count_of(levels.as_ref(), 2), flag_of(exclude_views.as_ref()));
    Ok(list(rows))
}

fn entity_definition_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.value(0).cloned();
    let facade = entity_facade()?;
    match crate::entity::info::definition_info(&facade, &text_of(name.as_ref())) {
        Some(info) => Ok(Ret::Value(Value::Map(info))),
        None => Ok(Ret::Value(Value::Null)),
    }
}

fn pretty_name(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let entity_name = call.text_or_empty(0);
    let title = call.value(1).cloned();
    let base_name = call.value(2).cloned();
    let title = text_of(title.as_ref());
    let base = text_of(base_name.as_ref());
    Ok(Ret::from(crate::entity::info::pretty_name(
        &entity_name,
        Some(title.as_str()).filter(|text| !text.is_empty()),
        Some(base.as_str()).filter(|text| !text.is_empty()),
    )))
}

fn get_datasource_group_names(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let facade = entity_facade()?;
    Ok(texts(crate::entity::info::datasource_group_names(&facade)))
}

fn get_eeca_rule_count(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let facade = entity_facade()?;
    Ok(Ret::from(
        i32::try_from(crate::entity::load_feed::eca_rule_count(&facade)).unwrap_or(i32::MAX),
    ))
}

/// `ec.entity.getQueryStatsList(orderBy, entityFilter, sqlFilter)`: the statement stats of this instance, as Java's.
fn get_query_stats_list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let order_by = call.value(0).cloned();
    let entity_filter = call.value(1).cloned();
    let sql_filter = call.value(2).cloned();
    let rows = crate::entity::query_stats::list(
        &text_of(order_by.as_ref()),
        &text_of(entity_filter.as_ref()),
        &text_of(sql_filter.as_ref()),
    );
    Ok(list(rows))
}

/// `ec.entity.clearQueryStats()`: through `org.moqui.dev.DatabaseServices.reset#QueryStats` (moqui-tools), which
/// needs `DATABASE_ADMIN` and writes the audit row; a refusal is a message error, as the service adds it.
fn clear_query_stats(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::service::call::sync_with(
        "org.moqui.dev.DatabaseServices.reset#QueryStats",
        &std::collections::BTreeMap::new(),
        &crate::service::call::CallOptions::default(),
    )
    .map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::Undefined)
}

/// The EECA rules of an entity, each as the XML of its rule.
fn eeca_rule_texts(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let entity_name = call.text_or_empty(0);
    let facade = entity_facade()?;
    let rules: Vec<String> = crate::entity::load_feed::eca_rules(&facade, &entity_name)
        .into_iter()
        .map(|rule| {
            let on = |flag: bool, name: &str| {
                if flag {
                    format!(" {name}=\"true\"")
                } else {
                    String::new()
                }
            };
            format!(
                "<eeca id=\"{}\" entity=\"{}\"{}{}{}{}>{}{}</eeca>",
                rule.id,
                rule.entity_name,
                on(rule.on_create, "on-create"),
                on(rule.on_update, "on-update"),
                on(rule.on_delete, "on-delete"),
                on(rule.run_before, "run-before"),
                rule.condition,
                rule.actions
            )
        })
        .collect();
    Ok(texts(rules))
}

/// Check the tables of `names` (view entities by their members), first forgetting what is known of the tables, as
/// Java's `forceCheckExistingTables`: `{ddl, errors, failed}`. With `groups`, only the entities of those entity groups
/// are checked. The names are checked together; when that fails (a view with a member that is not defined), each is
/// checked on its own, and a name that fails is logged and listed in `failed` (`{entityName, error}`) while the rest
/// go on.
fn check_tables(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let names = call.texts(0).unwrap_or_default().to_vec();
    let groups = call.texts(1);
    let facade = entity_facade()?;
    let default_group = facade.default_group().to_string();
    let mut failed: Vec<(String, String)> = Vec::new();
    let names: Vec<String> = match groups {
        None => names,
        Some(groups) => names
            .into_iter()
            .filter(|name| {
                match facade.with_entity(name, |entity, _, _| {
                    Ok(entity.group.clone().unwrap_or_else(|| default_group.clone()))
                }) {
                    Ok(group) => groups.contains(&group),
                    Err(err) => {
                        failed.push((name.clone(), err.message().to_string()));
                        false
                    }
                }
            })
            .collect(),
    };
    facade.forget_tables();
    let mut ddl = Vec::new();
    let mut errors = 0usize;
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    match facade.check_tables_ddl(&refs) {
        Ok(check) => {
            ddl = check.ddl;
            errors = check.errors;
        }
        Err(err) => {
            tracing::warn!(
                "Checking the tables together failed, checking each entity: {}",
                err.message()
            );
            for name in &names {
                match facade.check_tables_ddl(&[name.as_str()]) {
                    Ok(check) => {
                        ddl.extend(check.ddl);
                        errors += check.errors;
                    }
                    Err(err) => {
                        tracing::error!("Table check of entity {name} failed: {}", err.message());
                        failed.push((name.clone(), err.message().to_string()));
                    }
                }
            }
        }
    }
    let mut map = indexmap::IndexMap::default();
    map.insert(
        "ddl".to_string(),
        Value::List(ddl.into_iter().map(Value::String).collect()),
    );
    map.insert(
        "errors".to_string(),
        Value::I64(i64::try_from(errors).unwrap_or(i64::MAX)),
    );
    let failed = failed
        .into_iter()
        .map(|(name, error)| {
            let mut row = indexmap::IndexMap::default();
            row.insert("entityName".to_string(), Value::String(name));
            row.insert("error".to_string(), Value::String(error));
            Value::Map(row)
        })
        .collect();
    map.insert("failed".to_string(), Value::List(failed));
    Ok(Ret::Value(Value::Map(map)))
}

/// The SQL runner's statement: `{columns, records, limitReached, rowsAffected}`, values formatted as text.
fn run_sql_statement(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let group = call.text_or_empty(0);
    let sql = call.text_or_empty(1);
    let limit = call.value(2).cloned();
    let facade = entity_facade()?;
    let result =
        crate::entity::info::run_sql(&facade, &group, &sql, count_of(limit.as_ref(), 500)).map_err(HostError::Lib)?;
    let mut map = indexmap::IndexMap::default();
    map.insert(
        "columns".to_string(),
        Value::List(result.columns.into_iter().map(Value::String).collect()),
    );
    let records = result
        .records
        .into_iter()
        .map(|row| {
            Value::List(
                row.into_iter()
                    .map(|value| match value {
                        Value::Bytes(bytes) => Value::string(format!("({} bytes)", bytes.len())),
                        other => other,
                    })
                    .collect(),
            )
        })
        .collect();
    map.insert("records".to_string(), Value::List(records));
    map.insert("limitReached".to_string(), Value::Bool(result.limit_reached));
    map.insert(
        "rowsAffected".to_string(),
        result.rows_affected.map_or(Value::Null, |count| {
            Value::I64(i64::try_from(count).unwrap_or(i64::MAX))
        }),
    );
    Ok(Ret::Value(Value::Map(map)))
}

/// The data writer: replay the builder's calls, then write in `mode`. Returns `{count, text}`.
fn write_data(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let ops = call.value_or_null(0);
    let mode = call.text_or_empty(1);
    let first = call.value(2).cloned();
    let second = call.value(3).cloned();
    let facade = entity_facade()?;
    let mut writer = crate::entity::EntityDataWriter::new(&facade);
    let Value::List(ops) = ops else {
        return Err(HostError::message("data writer calls are a list"));
    };
    for op in ops {
        let Value::List(pair) = op else { continue };
        let name = pair.first().map(moqui_lib::object::to_plain_string).unwrap_or_default();
        let value = pair.get(1).cloned().unwrap_or(Value::Null);
        let text = moqui_lib::object::to_plain_string(&value);
        let flag = matches!(value, Value::Bool(true)) || text == "true";
        match name.as_str() {
            "fileType" => {
                writer.file_type(crate::entity::DataFileType::parse(&text).map_err(HostError::Lib)?);
            }
            "entityName" => {
                writer.entity_name(&text);
            }
            "skipEntityName" => {
                writer.skip_entity_name(&text);
            }
            "allEntities" => {
                writer.all_entities();
            }
            "dependentLevels" => {
                writer.dependent_levels(text.parse().unwrap_or(0));
            }
            "master" => {
                writer.master(&text);
            }
            "prefix" => {
                writer.prefix(&text);
            }
            "filterMap" => {
                if let Value::Map(map) = value {
                    writer.filter_map(map.into_iter().collect());
                }
            }
            "orderBy" => {
                writer.order_by(&text);
            }
            "fromDate" => {
                writer.from_date(value);
            }
            "thruDate" => {
                writer.thru_date(value);
            }
            "isoDateTime" => {
                writer.iso_date_time(flag);
            }
            "tableColumnNames" => {
                writer.table_column_names(flag);
            }
            _ => {}
        }
    }
    let first = text_of(first.as_ref());
    let second = text_of(second.as_ref());
    let mut out = indexmap::IndexMap::default();
    let lib = |result: Result<u64, moqui_lib::error::Error>| result.map_err(HostError::Lib);
    let count = match mode.as_str() {
        "file" => lib(writer.file(&first))?,
        "directory" => lib(writer.directory(&first))?,
        "zipFile" => lib(writer.zip_file(&first, &second))?,
        "zipDirectory" => lib(writer.zip_directory(&first, &second))?,
        _ => {
            let (text, count) = writer.text().map_err(HostError::Lib)?;
            out.insert("text".to_string(), Value::String(text));
            count
        }
    };
    out.insert(
        "count".to_string(),
        Value::I64(i64::try_from(count).unwrap_or(i64::MAX)),
    );
    Ok(Ret::Value(Value::Map(out)))
}

/// The data loader: `{location | xmlText | csvText | jsonText, dataTypes, componentNames, ...}` to `{records,
/// messages}`. With data types or components and no location it loads the conf data, as `ec.entity.makeDataLoader()
/// .dataTypes(...).load()` does. CSV and JSON text go through a file in the runtime's `tmp` directory.
fn load_data(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let options = call.value_or_null(0);
    let facade = entity_facade()?;
    let Value::Map(options) = options else {
        return Err(HostError::message("the data loader takes an options map"));
    };
    let text = |name: &str| {
        options
            .get(name)
            .map(moqui_lib::object::to_plain_string)
            .unwrap_or_default()
    };
    let flag = |name: &str| matches!(options.get(name), Some(Value::Bool(true))) || text(name) == "true";
    let list = |name: &str| match options.get(name) {
        Some(Value::List(items)) => items.iter().map(moqui_lib::object::to_plain_string).collect(),
        _ => Vec::new(),
    };
    let mut locations: Vec<String> = list("locations");
    // An older caller passes one `location`.
    if !text("location").is_empty() {
        locations.push(text("location"));
    }
    let load_options = crate::entity::load_feed::LoadOptions {
        data_types: list("dataTypes"),
        components: list("componentNames"),
        only_create: flag("onlyCreate"),
        // `useTryInsert` is not read: a store is one upsert wherever that fits.
        dummy_fks: flag("dummyFks"),
        disable_fk_create: flag("disableFkCreate"),
        timeout: text("transactionTimeout")
            .parse::<u64>()
            .ok()
            .map(std::time::Duration::from_secs),
        restrict_sensitive_entities: flag("restrictSensitiveEntities"),
        csv_entity_name: Some(text("csvEntityName")).filter(|name| !name.is_empty()),
        csv_field_names: list("csvFieldNames"),
        csv_delimiter: text("csvDelimiter").chars().next(),
        csv_comment_start: text("csvCommentStart").chars().next(),
        csv_quote_char: text("csvQuoteChar").chars().next(),
        // Java's `messageNoActionFiles` is true unless the script sets it false.
        hide_no_action_files: text("messageNoActionFiles") == "false",
        default_values: match options.get("defaultValues") {
            Some(Value::Map(entries)) => entries
                .iter()
                .filter(|(_, value)| !value.is_null())
                .map(|(name, value)| (name.clone(), moqui_lib::object::to_plain_string(value)))
                .collect(),
            _ => Vec::new(),
        },
    };
    let before = facade.flags();
    facade.set_disable_entity_eca(flag("disableEntityEca") || before.disable_entity_eca);
    facade.set_disable_audit_log(flag("disableAuditLog") || before.disable_audit_log);
    facade.set_disable_data_feed(flag("disableDataFeed") || before.disable_data_feed);
    let factory = factory()?;
    let check = flag("check");
    let listing = flag("list");
    let mut fields_checked = 0_u64;
    let mut diff_info = Vec::new();
    let mut values = Vec::new();
    let loaded = if listing {
        let source = || load_source(&facade, &factory, &text, &locations, &load_options);
        crate::entity::load_feed::list_with(source).map(|((records, _), result)| {
            values = result.values;
            (records, Vec::new())
        })
    } else if check {
        // `check()`: the messages are what did not match, not what was loaded. `checkInfo()`: what did not match
        // is in the diff rows.
        let source = || load_source(&facade, &factory, &text, &locations, &load_options);
        let checked = if flag("checkInfo") {
            crate::entity::load_feed::check_info_with(source)
        } else {
            crate::entity::load_feed::check_with(source)
        };
        checked.map(|((records, _), result)| {
            fields_checked = result.fields_checked;
            diff_info = result.diff_info;
            (records, result.messages)
        })
    } else {
        load_source(&facade, &factory, &text, &locations, &load_options)
    };
    facade.set_disable_entity_eca(before.disable_entity_eca);
    facade.set_disable_audit_log(before.disable_audit_log);
    facade.set_disable_data_feed(before.disable_data_feed);
    let (records, messages) = loaded.map_err(HostError::Lib)?;
    let mut out = indexmap::IndexMap::default();
    out.insert(
        "records".to_string(),
        Value::I64(i64::try_from(records).unwrap_or(i64::MAX)),
    );
    out.insert(
        "messages".to_string(),
        Value::List(messages.into_iter().map(Value::String).collect()),
    );
    out.insert(
        "fieldsChecked".to_string(),
        Value::I64(i64::try_from(fields_checked).unwrap_or(i64::MAX)),
    );
    out.insert(
        "diffInfo".to_string(),
        Value::List(diff_info.into_iter().map(Value::Map).collect()),
    );
    if listing {
        out.insert(
            "values".to_string(),
            Value::Object(HostObject::new(crate::entity::EntityList::new(values))),
        );
    }
    Ok(Ret::Value(Value::Map(out)))
}

fn load_source(
    facade: &crate::entity::EntityFacade,
    factory: &crate::context::ExecutionContextFactory,
    text: &dyn Fn(&str) -> String,
    locations: &[String],
    options: &crate::entity::load_feed::LoadOptions,
) -> Result<(u64, Vec<String>), moqui_lib::error::Error> {
    use crate::entity::load_feed::{
        component_data_locations, conf_data_locations, load_entity_data, load_entity_text_file, load_entity_xml_text,
        location_in_components,
    };
    // As Java's `internalRun`: the XML, CSV and JSON text, each in its own transaction, then each location. With
    // none of those, the conf data and the components' data of the data types or components.
    let mut records = 0;
    let mut messages = Vec::new();
    let xml = text("xmlText");
    if !xml.is_empty() {
        records += load_entity_xml_text(facade, "xmlText", &xml, options)?.records;
    }
    for (name, extension) in [("csvText", "csv"), ("jsonText", "json")] {
        let body = text(name);
        if body.is_empty() {
            continue;
        }
        let dir = factory.runtime_dir().join("tmp");
        std::fs::create_dir_all(&dir).map_err(|err| {
            moqui_lib::error::Error::new(format!("could not create {}", dir.display())).with_source(err)
        })?;
        let path = dir.join(format!("data-load-{}.{extension}", crate::entity::uuid_v4()?));
        std::fs::write(&path, body).map_err(|err| {
            moqui_lib::error::Error::new(format!("could not write {}", path.display())).with_source(err)
        })?;
        let result = load_entity_text_file(facade, name, &path, options);
        let _ = std::fs::remove_file(&path);
        records += result?.records;
    }
    let given = !xml.is_empty() || !text("csvText").is_empty() || !text("jsonText").is_empty() || !locations.is_empty();
    let locations = if given {
        locations.to_vec()
    } else if options.data_types.is_empty() && options.components.is_empty() {
        Vec::new()
    } else {
        // Only the locations gathered here are narrowed to the components, as Java's `componentNameList`.
        let mut gathered = conf_data_locations(facade)?;
        let seed = options.data_types.is_empty() || options.data_types.iter().any(|kind| kind == "seed");
        for component in factory.components().iter() {
            if component.location != factory.framework_component_dir() {
                gathered.extend(component_data_locations(&component.name, &component.location, seed));
            }
        }
        gathered.retain(|location| location_in_components(location, &options.components));
        gathered
    };
    for location in locations {
        // As Java's `loadAllData`, a location that fails is logged and the load goes on with the next one.
        let result = match load_entity_data(facade, &location, options) {
            Ok(result) => result,
            Err(err) => {
                let text = crate::entity::load_feed::error_chain(&err);
                tracing::error!("Skipping to next file after error loading {location}: {text}");
                messages.push(format!("Error loading {location}: {text}"));
                continue;
            }
        };
        if result.records > 0 || given {
            messages.push(format!("Loaded {} records from {location}", result.records));
        }
        if result.failed_files > 0 {
            messages.push(format!("{} files failed to load from {location}", result.failed_files));
        }
        records += result.records;
    }
    Ok((records, messages))
}

// ---- service ----

fn get_known_service_names(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(texts(crate::service::info::known_service_names()))
}

fn get_all_service_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let levels = call.value(0).cloned();
    let levels = count_of(levels.as_ref(), 2);
    let mut counts: std::collections::BTreeMap<String, i64> = std::collections::BTreeMap::new();
    for name in crate::service::info::known_service_names() {
        *counts
            .entry(crate::entity::info::name_prefix(&name, levels))
            .or_default() += 1;
    }
    let rows = counts
        .into_iter()
        .map(|(name, services)| {
            let mut row = indexmap::IndexMap::default();
            row.insert("name".to_string(), Value::String(name));
            row.insert("services".to_string(), Value::I64(services));
            row
        })
        .collect();
    Ok(list(rows))
}

fn get_seca_rule_count(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(
        i32::try_from(crate::service::info::seca_rule_count()).unwrap_or(i32::MAX),
    ))
}

fn seca_rule_texts(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let service = call.text_or_empty(0);
    Ok(texts(crate::service::info::seca_rule_texts(&service)))
}

fn service_definition_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.value(0).cloned();
    match crate::service::info::service_definition_info(&text_of(name.as_ref())) {
        Some(info) => Ok(Ret::Value(Value::Map(info))),
        None => Ok(Ret::Value(Value::Null)),
    }
}

/// Java's `ServiceDefinition.convertValidateCleanParameters`: the in-parameters converted, checked and cleaned as a call
/// would before running the service, which does not run. A check that fails is a validation error in `ec.message` and
/// the parameters come back as given.
fn convert_validate_clean_parameters(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let parameters = call.value_or_null(1);
    let parameters: std::collections::BTreeMap<String, Value> = match parameters {
        Value::Map(map) => map.into_iter().collect(),
        _ => std::collections::BTreeMap::new(),
    };
    match crate::service::validate(&name, &parameters) {
        Ok(cleaned) => Ok(Ret::Value(Value::Map(cleaned.into_iter().collect()))),
        Err(err) if err.kind() == crate::context::ErrorKind::Validation => {
            Ok(Ret::Value(Value::Map(parameters.into_iter().collect())))
        }
        Err(err) => Err(HostError::Lib(moqui_lib::error::Error::new(err.message()))),
    }
}

/// An XML element as `{name, attributes, text, children}`, for the node wrapper of the screens. [`parse_node`] adds the
/// root's `xml`; the wrapper writes an element below it only when its `toString()` is called, rather than the XML of
/// every element being written here, each one again inside each of its ancestors.
fn node_value(node: &MNode) -> Value {
    let mut map = indexmap::IndexMap::default();
    map.insert("name".to_string(), Value::string(node.name()));
    map.insert(
        "attributes".to_string(),
        Value::Map(
            node.attributes()
                .into_iter()
                .map(|(name, value)| (name, Value::String(value)))
                .collect(),
        ),
    );
    map.insert("text".to_string(), node.text().map_or(Value::Null, Value::String));
    map.insert(
        "children".to_string(),
        Value::List(node.children().iter().map(node_value).collect()),
    );
    Value::Map(map)
}

fn parse_node(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let xml = call.text_or_empty(0);
    match MNode::parse_text("node", &xml) {
        Ok(node) => {
            let mut value = node_value(&node);
            if let Value::Map(map) = &mut value {
                map.insert("xml".to_string(), Value::string(node.write_string()));
            }
            Ok(Ret::Value(value))
        }
        Err(_) => Ok(Ret::Value(Value::Null)),
    }
}

fn job_runner_info(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    let Some(stats) = factory.service_pools().runner_stats() else {
        return Ok(Ret::Value(Value::Null));
    };
    let number = |value: u64| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    let mut map = indexmap::IndexMap::default();
    map.insert(
        "lastExecuteTime".to_string(),
        jiff::Timestamp::from_millisecond(stats.last_execute_millis).map_or(Value::Null, Value::Timestamp),
    );
    map.insert("executeCount".to_string(), number(stats.execute_count));
    map.insert("totalJobsRun".to_string(), number(stats.total_jobs_run));
    map.insert("lastJobsActive".to_string(), number(stats.last_jobs_active as u64));
    map.insert("lastJobsPaused".to_string(), number(stats.last_jobs_paused as u64));
    Ok(Ret::Value(Value::Map(map)))
}

fn worker_pool_info(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    let status = factory.service_pools().status();
    let number = |value: usize| Value::I64(i64::try_from(value).unwrap_or(i64::MAX));
    let mut map = indexmap::IndexMap::default();
    map.insert("workers".to_string(), number(status.workers));
    map.insert("active".to_string(), number(status.active));
    map.insert("queued".to_string(), number(status.queued));
    map.insert("queueCap".to_string(), number(status.queue_cap));
    Ok(Ret::Value(Value::Map(map)))
}

// ---- screen, factory, web ----

fn get_all_root_screen_locations(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    Ok(texts(crate::screen::info::all_root_screen_locations(&factory)))
}

fn get_screen_info_list(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let root_location = call.text_or_empty(0);
    let levels = call.value(1).cloned();
    let factory = factory()?;
    let rows = crate::screen::info::screen_info_list(&factory, &root_location, count_of(levels.as_ref(), 2))
        .map_err(|err| HostError::message(err.message()))?;
    Ok(list(rows))
}

fn get_status_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let include_sensitive = call.value(0).cloned();
    let factory = factory()?;
    let map = crate::context::status::status_map(&factory, flag_of(include_sensitive.as_ref()));
    Ok(Ret::Value(Value::Map(map)))
}

/// `ec.factory.getStartupReport()`: [`crate::startup::script_value`].
fn get_startup_report(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    Ok(Ret::Value(Value::Map(crate::startup::script_value(&factory))))
}

/// `ec.factory.getInstanceId()`: this server's instance id ([`crate::drain::instance_id`]).
fn get_instance_id(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    Ok(Ret::from(crate::drain::instance_id(&factory)))
}

fn get_component_info_list(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let factory = factory()?;
    Ok(list(crate::context::status::component_info_list(&factory)))
}

fn get_runtime_path(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(factory()?.runtime_dir().display().to_string()))
}

/// `ec.factory.waitWorkerPoolEmpty(retryLimit)`: wait up to `retryLimit` (default 50) times 100 ms for async service
/// work to finish. Returns whether the pool is empty.
fn wait_worker_pool_empty(call: &mut Call<'_>) -> Result<Ret, HostError> {
    // QuickJS's `Opt<u32>`: a number, refused outside the range of a u32, a fraction truncated
    let retry_limit = match call.number(0) {
        None => 50,
        Some(number) if number < 0.0 => {
            return Err(HostError::Type(
                "Error converting from js 'f64' into type 'u32': Underflow".to_string(),
            ))
        }
        Some(number) if number > f64::from(u32::MAX) => {
            return Err(HostError::Type(
                "Error converting from js 'f64' into type 'u32': Overflow".to_string(),
            ))
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Some(number) => number as u32,
    };
    Ok(factory()?.wait_worker_pool_empty(retry_limit).into())
}

fn get_thread_list(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(list(crate::context::status::thread_list()))
}

fn secure_request_parameters(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = crate::web::secure_parameters().into_iter().collect();
    Ok(Ret::Value(Value::Map(map)))
}

fn get_request_details(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(crate::web::request_details()))
}

// ---- resources ----

fn resource_ref(location: &str) -> Result<crate::context::ResourceRef, HostError> {
    let found = with_ec(|ec| ec.factory().resource().get(location))?;
    found.map_err(HostError::Context)
}

/// `{location, fileName, exists, directory, file, contentType, text, binary}` of a resource. Its size, last-modified
/// time and children are read when a script asks for them (`resourceStat`, `resourceEntries`): a reference made to ask
/// whether a file is there reads no file and lists no directory.
fn resource_info_value(resource: &crate::context::ResourceRef) -> Value {
    info_value(
        resource.location(),
        resource.exists(),
        resource.is_directory(),
        resource.is_file(),
    )
}

fn info_value(location: &str, exists: bool, directory: bool, file: bool) -> Value {
    let file_name = location.rsplit('/').next().unwrap_or_default().to_string();
    let mut map = indexmap::IndexMap::default();
    map.insert("location".to_string(), Value::string(location));
    map.insert("fileName".to_string(), Value::string(&file_name));
    map.insert("exists".to_string(), Value::Bool(exists));
    map.insert("directory".to_string(), Value::Bool(directory));
    map.insert("file".to_string(), Value::Bool(file));
    // `ResourceReference.getContentType`, `isText`, `isBinary`: from the file name's extension.
    let content_type = moqui_lib::resource::content_type_for_name(&file_name);
    map.insert(
        "contentType".to_string(),
        content_type.map_or(Value::Null, Value::string),
    );
    map.insert(
        "text".to_string(),
        Value::Bool(content_type.is_some_and(moqui_lib::resource::is_text_content_type)),
    );
    map.insert(
        "binary".to_string(),
        Value::Bool(content_type.is_some_and(moqui_lib::resource::is_binary_content_type)),
    );
    Value::Map(map)
}

/// `{size, lastModified}` of a resource (each null when it has none), or null for a location no scheme resolves.
fn resource_stat(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    match with_ec(|ec| ec.factory().resource().get(&location))? {
        Ok(resource) => {
            let mut map = indexmap::IndexMap::default();
            map.insert(
                "size".to_string(),
                resource
                    .size()
                    .map_or(Value::Null, |size| Value::I64(i64::try_from(size).unwrap_or(i64::MAX))),
            );
            map.insert(
                "lastModified".to_string(),
                resource.last_modified().map_or(Value::Null, Value::I64),
            );
            Ok(Ret::Value(Value::Map(map)))
        }
        Err(_) => Ok(Ret::Value(Value::Null)),
    }
}

/// [`resource_info_value`] of a location, or null for a location no scheme
/// resolves. A `dbresource://` location with no row is a reference that does not exist, as Java's.
fn resource_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    match with_ec(|ec| ec.factory().resource().get(&location))? {
        Ok(resource) => Ok(Ret::Value(resource_info_value(&resource))),
        Err(_) if location.starts_with("dbresource:") => Ok(Ret::Value(missing_info_value(&location))),
        Err(_) => Ok(Ret::Value(Value::Null)),
    }
}

fn missing_info_value(location: &str) -> Value {
    info_value(location, false, false, false)
}

/// The text at a location; a `#versionName` suffix reads that version (`ec.resource.getLocationText`).
fn resource_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    Ok(Ret::from(with_resources(|resources| {
        resources.location_text(&location)
    })?))
}

/// The text at a location decoded from its bytes in a charset (`ec.resource.getLocationText(location, charset)`).
fn resource_text_in(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let charset = call.text_or_empty(1);
    let bytes = resource_ref(&location)?.read_bytes().map_err(HostError::Context)?;
    Ok(Ret::from(
        super::decode_text(&bytes, Some(&charset)).map_err(HostError::Context)?,
    ))
}

/// `putText`: a `dbresource://` row, or a file for the file-backed schemes.
fn resource_put_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let text = call.text_or_empty(1);
    put_resource_bytes(&location, text.as_bytes())
}

/// The bytes of one version as a stream (`openStream` of a `location#versionName` reference), or null when that version
/// is not there.
fn resource_version_stream(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let version_name = call.text_or_empty(1);
    let bytes = with_resources(|resources| resources.read_version(&location, &version_name))?;
    match bytes {
        Some(bytes) => {
            let source = std::sync::Arc::new(crate::resource::source::MemorySource::new(bytes));
            Ok(Ret::Value(Value::Object(HostObject::new(super::StreamBytes {
                source,
            }))))
        }
        None => Ok(Ret::Value(Value::Null)),
    }
}

/// The file as a stream (`openStream`, `getInputStream`), read when the script reads or writes it.
fn resource_stream(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let opened = with_ec(|ec| ec.factory().resource().open(&location))?
        .map_err(|err| HostError::message(err.message()))?
        .ok_or_else(|| HostError::message(format!("no file at {location}")))?;
    Ok(Ret::Value(Value::Object(HostObject::new(super::StreamBytes {
        source: opened.source,
    }))))
}

/// `ResourceReference.isTextFilename`: the content type of the file name is a text type.
fn is_text_filename(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let filename = call.text_or_empty(0);
    Ok(moqui_lib::resource::content_type_for_name(&filename)
        .is_some_and(moqui_lib::resource::is_text_content_type)
        .into())
}

/// `putBytes`: a `Uint8Array` or `ArrayBuffer`, written as `putText` writes.
fn resource_put_bytes(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let Some(Value::Bytes(bytes)) = call.value(1) else {
        return Err(HostError::message("putBytes needs a Uint8Array or an ArrayBuffer"));
    };
    put_resource_bytes(&location, bytes)
}

fn put_resource_bytes(location: &str, bytes: &[u8]) -> Result<Ret, HostError> {
    let written = with_ec(|ec| ec.factory().resource().write_bytes(location, bytes))?;
    written.map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::Undefined)
}

/// `delete` of a file, or an empty directory, of a `dbresource://` or file-backed location. `false` when nothing is
/// there.
fn resource_delete(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let deleted = with_ec(|ec| ec.factory().resource().delete(&location))?;
    Ok(Ret::from(deleted.map_err(|err| HostError::message(err.message()))?))
}

/// `getDirectoryEntries`: the files and directories directly under a directory, each as [`resource_info`] gives it,
/// from the listing (no child is looked up again). A location the facade cannot list (such as `classpath`) has its child files only.
fn resource_entries(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let factory = factory()?;
    let resources = factory.resource();
    let entries = match resources.list(&location) {
        Ok(entries) => entries
            .iter()
            .map(|entry| info_value(&entry.location, true, !entry.is_file, entry.is_file))
            .collect(),
        Err(_) => match resources.get(&location) {
            Ok(resource) if resource.is_directory() => resource
                .children()
                .unwrap_or_default()
                .iter()
                .map(resource_info_value)
                .collect(),
            _ => Vec::new(),
        },
    };
    Ok(Ret::Value(Value::List(entries)))
}

/// `move`: a file or directory to a new location of the same kind of storage.
fn resource_move(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let new_location = call.text_or_empty(1);
    let moved = with_ec(|ec| ec.factory().resource().rename(&location, &new_location))?;
    Ok(Ret::from(moved.map_err(|err| HostError::message(err.message()))?))
}

/// Copy the bytes of one file to another location, for what a Groovy script did with `openStream` and `putStream`.
fn resource_copy(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let from = call.text_or_empty(0);
    let to = call.text_or_empty(1);
    let bytes = resource_ref(&from)?
        .read_bytes()
        .map_err(|err| HostError::message(err.message()))?;
    let written = with_ec(|ec| ec.factory().resource().write_bytes(&to, &bytes))?;
    Ok(Ret::from(written.map_err(|err| HostError::message(err.message()))?))
}

/// `supportsVersion`: `dbresource://` files keep versions, as in Java, and so does a provider scheme that says so
/// (`git://`).
fn resource_supports_version(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    Ok(Ret::from(with_ec(|ec| {
        ec.factory().resource().supports_version(&location)
    })?))
}

fn version_value(version: crate::context::ResourceVersion) -> Value {
    let mut map = indexmap::IndexMap::default();
    map.insert("versionName".to_string(), Value::string(version.version_name));
    map.insert(
        "previousVersionName".to_string(),
        version.previous_version_name.map_or(Value::Null, Value::string),
    );
    map.insert("versionDate".to_string(), version.version_date);
    map.insert("userId".to_string(), version.user_id.map_or(Value::Null, Value::string));
    map.insert("note".to_string(), version.note.map_or(Value::Null, Value::string));
    Value::Map(map)
}

/// A resource facade call from a host function: a facade error is a script exception.
fn with_resources<T>(
    body: impl FnOnce(&crate::context::ResourceFacade) -> Result<T, crate::context::Error>,
) -> Result<T, HostError> {
    with_ec(|ec| body(ec.factory().resource()))?.map_err(HostError::Context)
}

/// `getCurrentVersion`, `getRootVersion` and `getVersion(name)`: `{versionName, previousVersionName, versionDate,
/// userId}`, or null.
fn resource_version(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let which = call.text_or_empty(1);
    let name = call.text(2).map(str::to_string);
    let found = with_resources(|resources| match which.as_str() {
        "current" => resources.current_version(&location),
        "root" => resources.root_version(&location),
        _ => resources.version(&location, name.as_deref().unwrap_or_default()),
    })?;
    Ok(Ret::Value(found.map_or(Value::Null, version_value)))
}

/// `getVersionHistory`: newest first.
fn resource_version_history(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let versions = with_resources(|resources| resources.version_history(&location))?;
    Ok(Ret::Value(Value::List(
        versions.into_iter().map(version_value).collect(),
    )))
}

/// `getNextVersions(versionName)`: the versions made from that one.
fn resource_next_versions(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let version_name = call.text_or_empty(1);
    let versions = with_resources(|resources| resources.next_versions(&location, &version_name))?;
    Ok(Ret::Value(Value::List(
        versions.into_iter().map(version_value).collect(),
    )))
}

/// `getText(versionName)`: the text of one version, or null when there is no such version.
fn resource_version_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let version_name = call.text_or_empty(1);
    match with_resources(|resources| resources.version_text(&location, &version_name))? {
        Some(text) => Ok(Ret::Value(Value::string(text))),
        None => Ok(Ret::Value(Value::Null)),
    }
}

/// `getVersionDiff(versionNameLhs, versionNameRhs)`: the line diff from one version to the other (an empty name is
/// the current text) as `{fromName, toName, unifiedDiff, html, stats, hunks}`. A version that is not there throws.
fn resource_version_diff(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let lhs = call.text_or_empty(1);
    let rhs = call.text_or_empty(2);
    let diff = with_resources(|resources| resources.version_diff(&location, &lhs, &rhs))?;
    Ok(Ret::Value(diff.to_value()))
}

/// `ec.resource.textDiff(oldText, newText, fromName, toName)`: the line diff between two texts, in the same shape as
/// `getVersionDiff`, for callers that keep their revisions outside resource versioning.
fn text_diff(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let old = call.text_or_empty(0);
    let new = call.text_or_empty(1);
    let from_name = call.text_or_empty(2);
    let to_name = call.text_or_empty(3);
    Ok(Ret::Value(
        crate::resource::textdiff::diff(&old, &new, &from_name, &to_name).to_value(),
    ))
}

/// `ec.resource.markdownHtml(text)`: CommonMark (with GitHub tables) as HTML, the `.md` template renderer for text a
/// script holds, such as imported posts. Raw HTML passes through, so a caller showing the result to others sanitizes it.
fn markdown_html(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.text_or_empty(0);
    Ok(Ret::from(moqui_lib::markdown::to_html(&text)))
}

// ---- SQL on a database named by a conf map (InstanceServices) ----

/// The `entity_ds_*` entries of a conf map, as Java's `getConfConnection` reads them.
fn conf_db_of(conf: &Value) -> Result<crate::driver::ConfDb, HostError> {
    let Value::Map(map) = conf else {
        return Err(HostError::message("the database conf is not a map"));
    };
    let text = |name: &str| match map.get(name) {
        Some(Value::Null) | None => String::new(),
        Some(value) => moqui_lib::object::to_plain_string(value),
    };
    Ok(crate::driver::ConfDb {
        db_type: text("entity_ds_db_conf"),
        host: text("entity_ds_host"),
        port: text("entity_ds_port"),
        database: text("entity_ds_database"),
        user: text("entity_ds_user"),
        password: text("entity_ds_password"),
        ssl_mode: text("entity_ds_sslmode"),
        ssl_root_cert: text("entity_ds_sslrootcert"),
    })
}

fn conf_connection(conf: &Value) -> Result<Box<dyn crate::driver::DbConnection>, HostError> {
    let conf = conf_db_of(conf)?;
    crate::driver::conf_driver(&conf)
        .and_then(|driver| driver.connect())
        .map_err(HostError::Lib)
}

/// `testConfConnection(conf)`: whether a connection opens. A failure is logged, as the Java scripts did.
fn test_conf_connection(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let conf = call.value_or_null(0);
    let conf = conf_db_of(&conf)?;
    match crate::driver::conf_driver(&conf).and_then(|driver| driver.connect()) {
        Ok(_) => Ok(true.into()),
        Err(err) => {
            tracing::warn!(host = %conf.host, database = %conf.database, "Test connection failed: {}", err.message());
            Ok(false.into())
        }
    }
}

/// `runSqlCountConf(table, where, conf)`: `SELECT COUNT(*) FROM table WHERE where` on that database.
fn run_sql_count_conf(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let table = call.text_or_empty(0);
    let where_text = call.text_or_empty(1);
    let conf = call.value_or_null(2);
    let mut conn = conf_connection(&conf)?;
    let sql = if where_text.trim().is_empty() {
        format!("SELECT COUNT(*) AS row_count FROM {table}")
    } else {
        format!("SELECT COUNT(*) AS row_count FROM {table} WHERE {where_text}")
    };
    let rows = conn.query_once(&sql, &[]).map_err(HostError::Lib)?;
    let count = rows
        .first()
        .and_then(|row| row.columns.first())
        .map(|(_, value)| moqui_lib::object::to_plain_string(value))
        .and_then(|text| text.parse::<f64>().ok())
        .unwrap_or(0.0);
    Ok(Ret::from(count))
}

/// `runSqlUpdateConf(sql, conf)`: run one statement outside a transaction (`CREATE DATABASE` cannot run inside one)
/// and return the rows it changed.
fn run_sql_update_conf(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let sql = call.text_or_empty(0);
    let conf = call.value_or_null(1);
    let mut conn = conf_connection(&conf)?;
    let rows = conn.execute(&sql, &[]).map_err(HostError::Lib)?;
    #[allow(clippy::cast_precision_loss)]
    Ok(Ret::from(rows as f64))
}

// ---- registry token command (init#InstanceDocker) ----

/// How long a token command may run.
const TOKEN_COMMAND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// `runTokenCommand(cmd)`: run a configured command, split on whitespace with no shell, and return the first line of
/// its output, trimmed. A command that fails, or runs past the timeout, throws with its error output.
fn run_token_command(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let command = call.text_or_empty(0);
    Ok(Ret::from(run_command_first_line(&command).map_err(HostError::Lib)?))
}

fn run_command_first_line(command: &str) -> Result<String, moqui_lib::error::Error> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};
    let mut words = command.split_whitespace();
    let Some(program) = words.next() else {
        return Err(moqui_lib::error::Error::new("the token command is empty"));
    };
    crate::entity::fence::check_effect(crate::entity::fence::Effect::Process, program)?;
    let mut child = Command::new(program)
        .args(words)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|err| moqui_lib::error::Error::new(format!("could not run token command {program}: {err}")))?;
    // Read both pipes on their own threads, so a child that writes much is not blocked on a full pipe.
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();
    let out = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stdout.as_mut() {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });
    let err = std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(pipe) = stderr.as_mut() {
            let _ = pipe.read_to_string(&mut text);
        }
        text
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() >= TOKEN_COMMAND_TIMEOUT => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(moqui_lib::error::Error::new(format!(
                    "token command {program} did not finish in {} seconds",
                    TOKEN_COMMAND_TIMEOUT.as_secs()
                )));
            }
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(wait_err) => {
                return Err(moqui_lib::error::Error::new(format!(
                    "token command {program}: {wait_err}"
                )))
            }
        }
    };
    let output = out.join().unwrap_or_default();
    let errors = err.join().unwrap_or_default();
    if !status.success() {
        return Err(moqui_lib::error::Error::new(format!(
            "token command {program} failed ({status}): {}",
            errors.trim()
        )));
    }
    Ok(output.lines().next().unwrap_or_default().trim().to_string())
}

// ---- REST client ----

/// Send one request of `ec.service.rest()`: `{method, uri, headers: [[name, value]], body, timeoutSeconds,
/// denyInternal}`. Returns `{statusCode, reasonPhrase, text}`. Denied in sim mode, as a `remote-rest` call is.
fn rest_send(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let request = call.value_or_null(0);
    let Value::Map(request) = request else {
        return Err(HostError::message("rest request is not an object"));
    };
    let text = |name: &str| match request.get(name) {
        Some(Value::Null) | None => None,
        Some(value) => Some(moqui_lib::object::to_plain_string(value)),
    };
    let method_name = text("method").unwrap_or_else(|| "GET".to_string()).to_ascii_uppercase();
    let method = crate::outbound::Method::parse(&method_name)
        .ok_or_else(|| HostError::message(format!("HTTP method {method_name} is not supported")))?;
    let url = text("uri")
        .filter(|url| !url.is_empty())
        .ok_or_else(|| HostError::message("no URI specified for REST call"))?;
    let mut headers = Vec::new();
    if let Some(Value::List(pairs)) = request.get("headers") {
        for pair in pairs {
            if let Value::List(pair) = pair {
                if let [name, value] = pair.as_slice() {
                    headers.push((
                        moqui_lib::object::to_plain_string(name),
                        moqui_lib::object::to_plain_string(value),
                    ));
                }
            }
        }
    }
    let timeout = request
        .get("timeoutSeconds")
        .and_then(|value| moqui_lib::object::to_plain_string(value).parse::<f64>().ok())
        .filter(|seconds| *seconds > 0.0)
        .map(std::time::Duration::from_secs_f64);
    let sim = with_ec(|ec| ec.factory().entity())?.is_some_and(|facade| facade.sim_active());
    if sim {
        return Err(HostError::message(format!(
            "HTTP {method} {} denied because sim mode is active",
            crate::outbound::redacted(&url)
        )));
    }
    if let Err(refused) = crate::entity::fence::check_effect(
        crate::entity::fence::Effect::RemoteCall,
        &format!("HTTP {method} {}", crate::outbound::redacted(&url)),
    ) {
        return Err(HostError::message(refused.message()));
    }
    let response = crate::outbound::send(crate::outbound::Request {
        method,
        url: url.clone(),
        headers,
        body: text("body").filter(|body| !body.is_empty()),
        timeout,
        deny_internal: matches!(request.get("denyInternal"), Some(Value::Bool(true))),
    })
    .map_err(|err| {
        HostError::message(format!(
            "Error calling HTTP request to {}: {}",
            crate::outbound::redacted(&url),
            err.message()
        ))
    })?;
    let mut map = indexmap::IndexMap::default();
    map.insert("statusCode".to_string(), Value::I64(i64::from(response.status)));
    map.insert(
        "reasonPhrase".to_string(),
        Value::string(crate::outbound::reason_phrase(response.status)),
    );
    // A sender that kept no bytes sent text: the text is the bytes.
    let bytes = if response.bytes.is_empty() {
        response.body.as_bytes().to_vec()
    } else {
        response.bytes
    };
    map.insert("text".to_string(), Value::string(response.body));
    map.insert("bytes".to_string(), Value::Bytes(bytes));
    map.insert(
        "headers".to_string(),
        Value::List(
            response
                .headers
                .iter()
                .map(|(name, value)| Value::List(vec![Value::string(name), Value::string(value)]))
                .collect(),
        ),
    );
    Ok(Ret::Value(Value::Map(map)))
}

/// The XML of the form named after `#` in a screen location: `component://.../Screen.xml#FormName`.
fn form_xml(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let (screen, form) = location.split_once('#').unwrap_or((location.as_str(), ""));
    let factory = factory()?;
    let screens = crate::screen::ScreenFacade::of(&factory);
    let def = screens
        .screen_def(&factory, screen)
        .map_err(|err| HostError::message(err.message()))?;
    let found = crate::screen::form::assemble(&screens, &factory, &def, form)
        .map_err(|err| HostError::message(err.message()))?;
    // The assembled form's XML as it was written, not parsed and written again.
    Ok(Ret::from(found.xml.to_string()))
}

/// The XML of the form a `DbForm` makes as of a date (null for now), with its drafts when asked.
fn db_form_xml(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let form_id = call.text_or_empty(0);
    let as_of = call.value_or_null(1);
    let drafts = call.bool(2).unwrap_or_default();
    let date = if as_of.is_null() { None } else { Some(as_of) };
    let as_of = crate::screen::db_form::AsOf::of(date.as_ref(), Some(&Value::Bool(drafts)));
    Ok(Ret::from(
        crate::screen::db_form::db_form_xml(&form_id, as_of).map_err(|err| HostError::message(err.message()))?,
    ))
}

/// `ScheduledJobRunner.isLastRunBeforeLastSchedule`: no last run, or the last run is before the latest time the cron
/// expression came due.
fn is_last_run_before_last_schedule(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cron = call.text_or_empty(0);
    let last_run = call.value(1).cloned();
    let now = call.value(3).cloned();
    // Java's `(cron, lastRun, description, nowTimestamp)`: the last two are optional here
    Ok(Ret::from(
        crate::service::info::is_last_run_before_last_schedule(
            &cron,
            millis_of(last_run.as_ref()),
            millis_of(now.as_ref()),
        )
        .map_err(|err| HostError::message(err.message()))?,
    ))
}

/// `ec.service.getCronDescription(cron)`.
fn cron_description(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cron = call.text_or_empty(0);
    Ok(Ret::from(
        crate::service::info::cron_description(&cron).map_err(|err| HostError::message(err.message()))?,
    ))
}

/// `ec.service.nextCronExecution(cron, after)`: epoch millis of the next time the Quartz cron expression is due after
/// the moment (epoch millis, a timestamp, or null for now), or null when it never is.
fn next_cron_execution(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let cron = call.text_or_empty(0);
    let after = call.value(1).cloned();
    let after = match after.as_ref().filter(|value| !value.is_null()) {
        Some(value) => match crate::l10n::basic_convert(value, "Timestamp").value {
            Value::Timestamp(stamp) => stamp.as_millisecond(),
            _ => return Err(HostError::message("nextCronExecution needs a moment to start after")),
        },
        None => jiff::Timestamp::now().as_millisecond(),
    };
    match crate::service::info::next_cron_execution(&cron, after).map_err(|err| HostError::message(err.message()))? {
        Some(millis) => Ok(Ret::Value(Value::I64(millis))),
        None => Ok(Ret::Value(Value::Null)),
    }
}

/// `ec.artifactExecution.disableEntityEca()`: entity ECA rules do not run for this execution context's writes until
/// `enableEntityEca()`. Returns whether they were already disabled.
fn disable_entity_eca(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(
        crate::context::ExecutionContext::disable_entity_eca,
    )?))
}

fn enable_entity_eca(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(crate::context::ExecutionContext::enable_entity_eca)?))
}

/// `ec.artifactExecution.runAsUser(location, body)`: `body()` with authorization required again (a ServiceJob or an
/// async call runs with it off), and the screen `location` checked for all actions as the current user and pushed, so
/// what `body` calls is authorized as from that screen. A user the screen does not authorize is refused, and `body` does
/// not run. The script's own disableAuthz state is set aside while `body` runs and put back after, whatever `body` did
/// with `disableAuthz` and `enableAuthz`, so nothing outside the call changes. All of that is undone here, after the
/// call back returns, whether `body` returned, threw or was stopped by the deadline (which no `finally` sees). The shim
/// passes `body` (refused when it is not a function) and a function that calls it and keeps its result, as for
/// `runAsSystem`.
fn run_as_user(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let location = call.text_or_empty(0);
    let outer_script_authz = with_ec(crate::context::ExecutionContext::take_script_authz)?;
    let required = with_ec(crate::context::ExecutionContext::require_authz)?;
    // The check may run nested scripts (an authorization service, the entity ECA rules of a recorded failure), so it
    // runs with the engine lent; the call back after it is this leaf's own.
    let mut pushed = None;
    let lent = call.ops.lend(&mut || {
        pushed = Some(with_ec(|ec| {
            ec.push_checked(
                "AT_XML_SCREEN",
                &location,
                "all",
                crate::context::Authz::Required,
                crate::context::Tarpit::Skip,
                None,
            )
        }));
    });
    let refused = match (lent, pushed) {
        (Err(err), _) => Some(err),
        (Ok(()), Some(Ok(Ok(())))) => None,
        (Ok(()), Some(Ok(Err(err)))) => Some(HostError::message(err.message())),
        (Ok(()), Some(Err(err))) => Some(err),
        (Ok(()), None) => Some(HostError::message("the authorization of runAsUser did not run")),
    };
    if let Some(refused) = refused {
        restore_run_as_user(outer_script_authz, required);
        return Err(refused);
    }
    // the frame is popped and the guards put back however the call back ends: a return, a throw, the deadline, or a
    // panic that unwinds through it
    let _restore = RunAsUserRestore {
        location: location.clone(),
        guards: Some((outer_script_authz, required)),
    };
    // the second function argument: the one that calls body and keeps its result
    call.ops.call_back(1, Vec::new()).map(|_| Ret::Undefined)
}

/// Pops `runAsUser`'s frame and puts back what it set aside, when dropped.
struct RunAsUserRestore {
    location: String,
    guards: Option<(Option<crate::context::AuthzGuard>, crate::context::AuthzRequired)>,
}

impl Drop for RunAsUserRestore {
    fn drop(&mut self) {
        let popped = crate::context::ExecutionContext::with_current(crate::context::ExecutionContext::pop);
        if let Some((outer_script_authz, required)) = self.guards.take() {
            restore_run_as_user(outer_script_authz, required);
        }
        let location = &self.location;
        match popped {
            Some(Err(err)) => tracing::warn!("Pop after runAsUser of {location}: {}", err.message()),
            None => tracing::warn!("No execution context to pop after runAsUser of {location}"),
            Some(Ok(())) => {}
        }
    }
}

/// The guards go in the reverse of their making: what body left of the script's state, then the requirement, and then
/// the caller's own state comes back.
fn restore_run_as_user(
    outer_script_authz: Option<crate::context::AuthzGuard>,
    required: crate::context::AuthzRequired,
) {
    crate::context::ExecutionContext::with_current(|ec| drop(ec.take_script_authz()));
    drop(required);
    crate::context::ExecutionContext::with_current(|ec| ec.put_script_authz(outer_script_authz));
}

/// `ec.artifactExecution.runAsSystem(body)`: `body()` with no user and authorization off, in a fresh execution context
/// that shares the caller's transaction ([`crate::user::as_system`]); the caller's user, authorization and artifact
/// stack come back when `body` returns or throws. Only a service's own code may call it
/// ([`crate::user::system_scope_allowed`]): a screen's widget expressions, actions and transitions, a REST resource and
/// a web script are refused (docs/security/Attack-Surface.md, Commerce F72). The shim passes `body` (refused when it is
/// not a function) and a function that calls it and keeps its result, so the result is the script's own value.
fn run_as_system(call: &mut Call<'_>) -> Result<Ret, HostError> {
    crate::user::system_scope_allowed().map_err(|err| HostError::message(err.message()))?;
    match crate::user::as_system(|| call.ops.call_back(1, Vec::new())) {
        Ok(result) => result.map(|_| Ret::Undefined),
        Err(err) => Err(HostError::message(err.message())),
    }
}

/// `ec.artifactExecution.disableAuthz()`: authorization is not checked for this execution context until
/// `enableAuthz()`. Returns whether it was already disabled, as Java.
fn disable_authz(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(
        crate::context::ExecutionContext::script_disable_authz,
    )?))
}

fn enable_authz(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(
        crate::context::ExecutionContext::script_enable_authz,
    )?))
}

/// `ec.artifactExecution.disableTarpit()`: no hit of this execution context is counted against a tarpit until
/// `enableTarpit()`. Returns whether it was already disabled, as Java.
fn disable_tarpit(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(crate::context::ExecutionContext::disable_tarpit)?))
}

fn enable_tarpit(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(crate::context::ExecutionContext::enable_tarpit)?))
}

/// `ec.artifactExecution.authzDisabled()`: whether authorization is disabled for this execution context now, by a
/// script or by Rust code (a call with `disableAuthz()`, a job), without changing it.
fn authz_disabled(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(crate::context::ExecutionContext::authz_disabled)?))
}

fn shared_get(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let found = with_ec(|ec| ec.shared_map().get(&key).cloned())?;
    match found {
        Some(value) => Ok(Ret::Value(value)),
        None => Ok(Ret::Undefined),
    }
}

fn shared_set(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    let value = call.value_or_null(1);
    let host = value;
    Ok(Ret::from(with_ec(|ec| {
        ec.shared_map().insert(key, host);
    })?))
}

fn shared_delete(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    Ok(Ret::from(with_ec(|ec| {
        ec.shared_map().shift_remove(&key);
    })?))
}

fn shared_has(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let key = call.text_or_empty(0);
    Ok(Ret::from(with_ec(|ec| ec.shared_map().contains_key(&key))?))
}

fn shared_keys(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(|ec| {
        ec.shared_map().keys().cloned().collect::<Vec<String>>()
    })?))
}

fn register_service_on_end(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let parameters = call.value_or_null(1);
    let on_commit = call.bool(2).unwrap_or_default();
    let parameters = match parameters {
        Value::Map(map) => map.into_iter().collect(),
        _ => std::collections::BTreeMap::new(),
    };
    Ok(Ret::from(
        crate::service::call::register_on_end(&name, &parameters, on_commit)
            .map_err(|err| HostError::Lib(moqui_lib::error::Error::new(err.message())))?,
    ))
}

fn stack_name_string(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(
        crate::context::ExecutionContext::artifact_stack_names,
    )?))
}

/// `ec.artifactExecution.getStack()`: each frame as `{name, typeEnumId, actionEnumId}`, top first.
fn artifact_stack(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let frames = with_ec(crate::context::ExecutionContext::artifact_stack_frames)?;
    let rows = frames
        .into_iter()
        .map(|(name, artifact_type, action)| {
            let mut row = moqui_lib::object::ValueMap::default();
            row.insert("name".to_string(), Value::String(name));
            row.insert("typeEnumId".to_string(), Value::String(artifact_type));
            row.insert("actionEnumId".to_string(), Value::String(action));
            row
        })
        .collect();
    Ok(list(rows))
}

/// `ec.artifactExecution.push(name, type, action, requiresAuthz)`: a frame checked as a push checks any, an error
/// when the user is not authorized. The type defaults to `AT_OTHER` and the action to `AUTHZA_VIEW`.
fn artifact_push(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let artifact_type = call.opt_text(1).filter(|text| !text.is_empty());
    let action = call.opt_text(2).filter(|text| !text.is_empty());
    let artifact_type = artifact_type.unwrap_or_else(|| "AT_OTHER".to_string());
    let action = action.unwrap_or_else(|| "AUTHZA_VIEW".to_string());
    let requires_authz = call.value(3).is_none_or(super::truthy);
    let pushed = with_ec(|ec| {
        ec.push_checked(
            &artifact_type,
            &name,
            &action,
            crate::context::Authz::required_if(requires_authz),
            crate::context::Tarpit::Count,
            None,
        )
    })?;
    pushed.map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::Undefined)
}

fn artifact_pop(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    with_ec(crate::context::ExecutionContext::pop)?.map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::Undefined)
}

/// `setAnonymousAuthorizedAll()` (true) and `setAnonymousAuthorizedView()`: the grant on the top frame.
fn artifact_anonymous(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let access = if call.value(0).is_some_and(super::truthy) {
        crate::context::AnonymousAccess::All
    } else {
        crate::context::AnonymousAccess::View
    };
    with_ec(|ec| ec.grant_anonymous(access, None))?;
    Ok(Ret::Undefined)
}

fn entity_eca_disabled(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(Ret::from(with_ec(
        crate::context::ExecutionContext::entity_eca_disabled,
    )?))
}

/// Whether the user may do `action` on the artifact, checked as a push would check it, without pushing it.
fn is_permitted(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.text_or_empty(0);
    let artifact_type = call.text(1).map(str::to_string);
    let action = call.text(2).map(str::to_string);
    let artifact_type = artifact_type.unwrap_or_else(|| "AT_XML_SCREEN".to_string());
    let action = action.unwrap_or_else(|| "view".to_string());
    Ok(Ret::from(
        crate::user::authorize(&artifact_type, &name, &action).is_ok(),
    ))
}

/// `getFindFiltersForUser(entityName)`: the `EntityFilter` records that apply to a find on the entity now, or null
/// when none do, as Java returns (an empty list would be true in a script condition).
fn get_find_filters_for_user(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let entity_name = call.text_or_empty(0);
    let filters = crate::user::find_filters_for_user(&entity_name).map_err(|err| HostError::message(err.message()))?;
    if filters.is_empty() {
        return Ok(Ret::Value(Value::Null));
    }
    let rows = filters
        .iter()
        .map(|filter| {
            filter
                .fields()
                .iter()
                .map(|(name, value)| (name.to_string(), value.clone()))
                .collect()
        })
        .collect();
    Ok(list(rows))
}

fn resolve_rest_service(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    let method = call.text_or_empty(1);
    let value = crate::service::resolve_rest_service(&path, &method).map_err(HostError::Context)?;
    Ok(Ret::Value(value))
}

fn transition_info(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let root = call.text_or_empty(0);
    let location = call.text_or_empty(1);
    let name = call.text_or_empty(2);
    let method = call.text_or_empty(3);
    let factory = factory()?;
    let value =
        crate::screen::info::transition_info(&factory, &root, &location, &name, &method).map_err(HostError::Context)?;
    Ok(Ret::Value(value))
}

fn values_from_plain_map(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let map = call.value_or_null(0);
    let entity = call.text_or_empty(1);
    let Value::Map(map) = map else {
        return Err(HostError::message("Entity graph must be a map"));
    };
    let values =
        crate::entity::query_rest::values_from_plain_map(&entity_facade()?, &map, &entity).map_err(HostError::Lib)?;
    Ok(Ret::Value(Value::Object(HostObject::new(
        crate::entity::EntityList::new(values),
    ))))
}

fn rest_root_resources(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let roots = crate::service::root_resource_infos().map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::Value(Value::List(roots)))
}

/// `getAllEntityNamesWithMaster`: the entities that have a master definition, sorted.
fn get_all_entity_names_with_master(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let facade = entity_facade()?;
    let mut names: Vec<String> = facade.with_model(|model| {
        model
            .entities()
            .iter()
            .filter(|entity| !entity.masters.is_empty())
            .map(|entity| entity.full_name.clone())
            .collect()
    });
    names.sort();
    Ok(texts(names))
}

fn period_range(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let period = call.text_or_empty(0);
    let offset = call.text_or_empty(1);
    let basis = call.text_or_empty(2);
    let (from, thru) =
        crate::user::period_range(&period, &offset, &basis).map_err(|err| HostError::message(err.message()))?;
    Ok(Ret::Value(Value::List(vec![from, thru])))
}

/// `TimeZone.getAvailableIDs()`: the zone names of the time zone database, sorted.
fn get_available_time_zones(_call: &mut Call<'_>) -> Result<Ret, HostError> {
    let mut names: Vec<String> = jiff::tz::db().available().map(|name| name.to_string()).collect();
    names.sort();
    Ok(texts(names))
}

fn is_restricted_generic_entity(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let name = call.value(0).cloned();
    let name = text_of(name.as_ref());
    // a view over a restricted entity is restricted too
    Ok(Ret::from(match with_ec(|ec| ec.factory().entity())? {
        Some(facade) => crate::entity::sensitive::is_restricted_entity_or_view(&facade, &name),
        None => crate::entity::sensitive::is_restricted_generic_entity(&name),
    }))
}

fn camel_case_to_pretty(call: &mut Call<'_>) -> Result<Ret, HostError> {
    Ok(moqui_lib::string::camel_case_to_pretty(&text_of(call.value(0))).into())
}

fn pretty_to_camel_case(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let text = call.value(0).cloned();
    let first_upper = call.value(1).cloned();
    Ok(Ret::from(moqui_lib::string::pretty_to_camel_case(
        &text_of(text.as_ref()),
        flag_of(first_upper.as_ref()),
    )))
}

/// The host side of `ec.entity.getEntityDataDocument()`'s writers: `mode` `text` gives `{text, count}` for
/// `writeDocumentsToWriter`, `file` and `dir` write under `path` (`writeDocumentsToFile`, `writeDocumentsToDirectory`)
/// and give `{count}`. After the condition come `fromUpdateStamp`, `thruUpdateStamp` and `prettyPrint`.
fn write_data_documents(call: &mut Call<'_>) -> Result<Ret, HostError> {
    use moqui_db::sql::Condition;
    let mode = call.text_or_empty(0);
    let path = call.text_or_empty(1);
    let ids = call.texts(2).unwrap_or_default().to_vec();
    let pretty = matches!(call.value(6), Some(Value::Bool(true)));
    let facade = entity_facade()?;
    let condition = match call.value_or_null(3) {
        Value::Null => None,
        Value::Object(object) if object.downcast::<Condition>().is_some() => object
            .downcast::<Condition>()
            .map(|condition| Condition::clone(&condition)),
        // a map of alias to value: an AND of equalities
        Value::Map(map) => {
            let mut equals: Vec<Condition> = map
                .into_iter()
                .map(|(field, value)| Condition::equals(field.as_str(), value))
                .collect();
            match equals.len() {
                0 => None,
                1 => equals.pop(),
                _ => Some(Condition::and(equals)),
            }
        }
        _ => return Err(HostError::message("the document writers take a condition or a map")),
    };
    let stamp = |value: Option<&Value>| value.filter(|value| !value.is_null()).and_then(timestamp_at);
    let (from, thru) = (stamp(call.value(4)), stamp(call.value(5)));
    let mut out = indexmap::IndexMap::default();
    let count = match mode.as_str() {
        "text" => {
            let (text, count) = crate::entity::document::documents_json(&facade, &ids, condition, from, thru, pretty)
                .map_err(HostError::Lib)?;
            out.insert("text".to_string(), Value::String(text));
            count
        }
        "file" => crate::entity::document::write_documents_to_file(
            &facade,
            std::path::Path::new(&path),
            &ids,
            condition,
            from,
            thru,
            pretty,
        )
        .map_err(HostError::Lib)?,
        "dir" => crate::entity::document::write_documents_to_directory(
            &facade,
            std::path::Path::new(&path),
            &ids,
            condition,
            from,
            thru,
            pretty,
        )
        .map_err(HostError::Lib)?,
        other => return Err(HostError::message(format!("unknown document write mode {other}"))),
    };
    out.insert(
        "count".to_string(),
        Value::I64(i64::try_from(count).unwrap_or(i64::MAX)),
    );
    Ok(Ret::Value(Value::Map(out)))
}

fn timestamp_at(value: &Value) -> Option<jiff::Timestamp> {
    crate::search::postgres::compile::parse_date_bound(value)
}
