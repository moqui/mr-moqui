// The navigation catalog of an app home (plans/archive/Tools-Fancy.md B7), used by the sections of template/HomeParts.xml.
//
// A home lists its entries once, in its actions, as `navCatalog`:
//
//     {path: "Security/UserAccount", group: "Administration"}          a screen in a group of icon links
//     {path: "Monitor", featured: true, text: "Traces, logs, errors",        a featured card, with up to three
//         links: [{path: "Monitor/Traces"}, {path: "Monitor/Logs"}]}         secondary links
//     {url: "//apps/Terminal", title: "Terminal UI", group: "Elsewhere"}     a link to something outside the menus
//
// - `path` is the screen's path below the app root (the screen whose default subscreen the home is). The entry is
//   dropped unless every screen on the path is in the menu of the one above it for this user, so a screen that a
//   `menu-include` or a conf override hides, or that the user may not view, has no tile. Its title and icon are the
//   menu item's; an entry's own `title` replaces a menu title that says too little out of its menu ("Messages"), and
//   its own `icon` the menu's (a subscreen with no image of its own has its parent's).
// - `url` in place of `path` is a link the menus do not know (another app, a screen out of every menu); it needs a
//   `title`, and is kept as it is: the link itself is left out for a user who may not follow it.
// - `featured: true` puts the entry in the featured cards and not in a group; `text` is the card's one line, and
//   `links` its secondary links (the first three that are shown), each `{path | url, title, parameters}`.
// - `group` is the title of the group of an entry that is not featured, by default its menu item's group (the
//   screen's `menu-group`, which the drawer shows too); the groups keep the order in which the catalog first names
//   them. `parameters` is added to the link as a query string.
// - `hidden: true` names the entry's screen and shows nothing for it: a screen the menu has that this home leaves out
//   for this user (a tool whose permission the user does not hold), so it is not in the last group either.
// - A menu item of the app root that no entry names (by the first name of a `path`, in an entry or in a card's
//   links) goes in the group its menu group names, when the catalog has that group, else in a last group,
//   `otherTitle`, so a screen another component mounts is never missing.
//
// build(catalog, menuOf, options) is the rule, with nothing of the screen in it, so a spec calls it with a catalog and
// menu items of its own: menuOf(parentPath) gives the menu items ({name, title, icon, group}) of the screen at parentPath
// ("" is the app root). options: homeName (the home's own menu item, never listed), linkPrefix (what a path is
// written after, "../" from a home), otherTitle. It returns {featured: [{path, title, text, icon, link, links:
// [{title, link}]}], groups: [{title, items: [{path, title, text, icon, link}]}]}.

function queryString(parameters) {
    const pairs = []
    for (const [name, value] of Object.entries(parameters || {})) {
        if (value === null || value === undefined || value === "") continue
        pairs.push(encodeURIComponent(name) + "=" + encodeURIComponent(String(value)))
    }
    return pairs.length ? "?" + pairs.join("&") : ""
}

function pathNames(path) {
    return String(path || "")
        .split("/")
        .filter(name => name)
}

function build(catalog, menuOf, options) {
    const settings = Object.assign({homeName: "Home", linkPrefix: "../", otherTitle: "More"}, options || {})
    const menus = new Map()
    const menu = parentPath => {
        if (!menus.has(parentPath)) menus.set(parentPath, Array.from(menuOf(parentPath) || []))
        return menus.get(parentPath)
    }
    // the menu item of the screen at `path`, or null when a screen on the way is not in the menu above it
    const menuItem = path => {
        const names = pathNames(path)
        let found = null
        for (let index = 0; index < names.length; index++) {
            found = menu(names.slice(0, index).join("/")).find(item => item.name === names[index]) || null
            if (!found) return null
        }
        return found
    }
    const named = new Set()
    // what an entry or a card's link shows, or null when it is dropped
    const resolve = entry => {
        if (!entry) return null
        const query = queryString(entry.parameters)
        if (entry.url) {
            if (!entry.title) return null
            return {path: null, title: String(entry.title), icon: entry.icon || "", link: String(entry.url) + query}
        }
        const names = pathNames(entry.path)
        if (!names.length) return null
        named.add(names[0])
        const item = menuItem(entry.path)
        if (!item) return null
        return {
            path: names.join("/"),
            title: String(entry.title || item.title || item.name),
            icon: entry.icon || item.icon || "",
            link: settings.linkPrefix + names.join("/") + query,
            menuGroup: item.group || ""
        }
    }

    const featured = []
    const groups = []
    const groupOf = title => {
        let group = groups.find(candidate => candidate.title === title)
        if (!group) {
            group = {title, items: []}
            groups.push(group)
        }
        return group
    }
    for (const entry of Array.from(catalog || [])) {
        const shown = resolve(entry)
        // a card's links are named whether or not the card is shown, so a hidden card's screens do not come back below
        const links = Array.from((entry && entry.links) || [])
            .map(resolve)
            .filter(link => link)
            .slice(0, 3)
        if (!shown || entry.hidden) continue
        shown.text = entry.text ? String(entry.text) : ""
        if (entry.featured) {
            delete shown.menuGroup
            shown.links = links.map(link => ({title: link.title, link: link.link}))
            featured.push(shown)
        } else {
            const group = String(entry.group || shown.menuGroup || settings.otherTitle)
            delete shown.menuGroup
            groupOf(group).items.push(shown)
        }
    }

    const unnamed = menu("").filter(item => item.name !== settings.homeName && !named.has(item.name))
    if (unnamed.length) {
        // the last group, also when the catalog put entries of its own under the same title
        let other = groups.find(group => group.title === settings.otherTitle)
        if (other) groups.splice(groups.indexOf(other), 1)
        else other = {title: settings.otherTitle, items: []}
        for (const item of unnamed) {
            // with a menu group (menu-group) that the catalog has, it joins that group; else the last one
            const own = item.group ? groups.find(group => group.title === item.group) : null
            ;(own || other).items.push({
                path: item.name,
                title: String(item.title || item.name),
                text: "",
                icon: item.icon || "",
                link: settings.linkPrefix + item.name
            })
        }
        if (other.items.length) groups.push(other)
    }
    return {featured, groups: groups.filter(group => group.items.length)}
}

// The catalog of the home being rendered: the menus are read with sri.getMenuItems, below the screen the home is a
// subscreen of. One parent's menu failing is logged and read as empty, so its entries are dropped and the rest stay.
function forScreen(sri, ec, catalog, options) {
    const fullPath = Array.from(sri.getScreenUrlInfo().fullPathNameList || [])
    const homeName = (options && options.homeName) || fullPath[fullPath.length - 1] || "Home"
    const rootPath = fullPath.slice(0, Math.max(fullPath.lastIndexOf(homeName), 0))
    const menuOf = parentPath => {
        try {
            const names = rootPath.concat(pathNames(parentPath))
            return Array.from(sri.getMenuItems("/" + names.join("/")) || [])
                .filter(item => !item.disabled && !item.parameterMissing)
                .map(item => ({
                    name: String(item.name),
                    title: String(ec.resource.expand(item.title, "") || item.name),
                    icon: item.image && item.imageType === "icon" ? String(item.image) : "",
                    group: item.group ? String(item.group) : ""
                }))
        } catch (error) {
            ec.logger.error("Home navigation: the menu of " + (parentPath || "the app root") + " failed: " + String(error))
            return []
        }
    }
    return build(catalog, menuOf, Object.assign({}, options || {}, {homeName}))
}

// Whether the Dev AIDE app is mounted beside this one (a dev instance): an item named dev in the menu of the apps
// root, the first name of the path. A menu that cannot be read is logged and read as not mounted.
function devMounted(sri, ec) {
    try {
        const fullPath = Array.from(sri.getScreenUrlInfo().fullPathNameList || [])
        if (!fullPath.length) return false
        return Array.from(sri.getMenuItems("/" + fullPath[0]) || []).some(item => String(item.name) === "dev")
    } catch (error) {
        ec.logger.error("Home navigation: the apps menu failed: " + String(error))
        return false
    }
}

;({build, forScreen, devMounted})
