// This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License.
//
// The entities a user opened last in Tools › Data › Grid (plans/archive/Tools-Fancy.md B8), newest first: the user preference
// `tools.recent`, a JSON list of at most twenty entity names with no name twice, as REPORT_RECENT is for dashboards.
// The grid records a name when it opens an entity (screen/Tools/Data/Grid.xml); the Tools home lists them.
//
// - put(names, name): the list with name first; the rule, with nothing of the user in it.
// - read(): the user's list; a preference that is not a list of names reads as empty.
// - record(name): put name first for the signed-in user. Nothing is written when it is first already, and a write
//   that fails is logged and is not the caller's error: the grid opens whether or not its entity is remembered.

const KEY = "tools.recent"
const LIMIT = 20

function put(names, name) {
    const first = String(name || "")
    const others = Array.from(names || [])
        .map(String)
        .filter(other => other && other !== first)
    return Array.from(new Set(first ? [first].concat(others) : others)).slice(0, LIMIT)
}

function read() {
    let names = []
    try {
        names = JSON.parse(ec.user.getPreference(KEY) || "[]")
    } catch (error) {
        names = []
    }
    return Array.isArray(names) ? names.filter(name => typeof name === "string" && name) : []
}

function record(name) {
    const first = String(name || "")
    if (!first || !ec.user.userId) return
    try {
        const names = read()
        if (names[0] === first) return
        ec.user.setPreference(KEY, JSON.stringify(put(names, first)))
    } catch (error) {
        ec.logger.warn("Tools recent entities: " + first + " was not recorded: " + String(error))
    }
}

;({KEY, LIMIT, put, read, record})
