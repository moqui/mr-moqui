// How a wiki page path and a page file name name a page (org.moqui.resource.WikiServices; plans/Security-Review-Medium.md
// SEC-BOS-85), one rule for the record lookups, the file lookup and the children list, so the record whose access rules
// are asked and the file that is read are always the same page's:
//   normalPath(path)           the path with no empty segment (a doubled, leading or trailing slash names the same page),
//                              each segment in NFC, as update#WikiPage saves a name
//   pageNameOf(fileName)       the page a file is: its name less the longest page type it ends with (get#WikiPageTypes:
//                              Name.md.fancy is page Name), else less one extension (a file of another type), a dot first
//                              kept (.profile is page .profile); namer() gives it with the types looked up once
//   pageOfFile(spaceId, path, exceptWikiPageId)
//                              the WikiPage record (other than exceptWikiPageId) of the page at a file's page path: the
//                              one at that path, else one whose path is the same ignoring case (a case-insensitive file
//                              system finds Restricted.md for restricted), else null
// CC0 plus patent grant.
function normalPath(path) {
    return String(path == null ? "" : path)
        .split("/")
        .filter(part => part !== "")
        .map(part => part.normalize("NFC"))
        .join("/")
}
function pageTypes() {
    const types = ec.service.sync().name("org.moqui.resource.WikiServices.get#WikiPageTypes").call() || {}
    return Array.from(types.plainTypeList || [])
        .concat(Array.from(types.scriptTypeList || []))
        .concat(["markdown"])
        .sort((left, right) => right.length - left.length)
}
function pageNameOf(fileName, types = pageTypes()) {
    const name = String(fileName == null ? "" : fileName)
    const lower = name.toLowerCase()
    const type = types.find(type => name.length > type.length + 1 && lower.endsWith("." + type))
    if (type) return name.substring(0, name.length - type.length - 1)
    const dot = name.lastIndexOf(".")
    return dot > 0 ? name.substring(0, dot) : name
}
// pageNameOf with the types looked up once, for a loop over many file names
function namer() {
    const types = pageTypes()
    return fileName => pageNameOf(fileName, types)
}
function pageOfFile(wikiSpaceId, filePagePath, exceptWikiPageId) {
    if (!wikiSpaceId || filePagePath == null) return null
    const path = normalPath(filePagePath)
    const cf = ec.entity.conditionFactory
    const find = () => {
        const found = ec.entity.find("moqui.resource.wiki.WikiPage").condition("wikiSpaceId", wikiSpaceId)
        return exceptWikiPageId ? found.condition("wikiPageId", "not-equals", exceptWikiPageId) : found
    }
    const exact = Array.from(find().condition("pagePath", path || null).list())[0]
    if (exact) return exact
    if (!path) return null
    return Array.from(find().condition(cf.makeCondition("pagePath", "equals", path, false, true)).orderBy("wikiPageId").list())[0] || null
}

;({normalPath, pageNameOf, namer, pageOfFile})
