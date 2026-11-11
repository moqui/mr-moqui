// The navigation catalog of the app homes (plans/Tools-Fancy.md B7): the rule of template/HomeCatalog.js, which the
// sections of template/HomeParts.xml show. Rule tests, no screen is rendered: build() is called with a catalog and the
// menu items of a made-up app, as sri.getMenuItems would give them for a user.
// Run: ./moq test tools --spec HomeCatalog
const MODULE = "component://tools/template/HomeCatalog.js"
// the app's menu for this user: Printers is not in it (hidden by a conf, or not the user's to view), Voice is mounted
// by another component
const MENUS = {
    "": [
        {name: "Home", title: "Home", icon: "sym_o_home"},
        {name: "Cache", title: "Caches", icon: "sym_o_archive"},
        {name: "Security", title: "Security", icon: "sym_o_security"},
        {name: "Voice", title: "Voice", icon: "sym_o_call"}
    ],
    Security: [{name: "UserAccount", title: "Users", icon: "sym_o_security"}]
}
const CATALOG = [
    {
        path: "Security",
        featured: true,
        title: "Users and access",
        text: "Accounts and groups",
        links: [{path: "Security/UserAccount"}, {path: "Security/UserGroup"}]
    },
    {path: "Printers", featured: true, text: "Not in the menu"},
    {path: "Cache", group: "Server"},
    {path: "Printers", group: "Server"},
    {path: "Print/Jobs", group: "Server"},
    {path: "Security/UserAccount", group: "Security", icon: "sym_o_person", parameters: {disabled: "N"}},
    {path: "Security/ArtifactGroup", group: "Access"},
    {url: "//apps/Terminal", title: "Terminal UI", group: "Elsewhere"}
]
function build(catalog, options) {
    return ec.resource.script(MODULE).build(catalog, path => MENUS[path] || [], options)
}
function titles(items) {
    return items.map(item => item.title).join(", ")
}

feature("an entry whose screen is not in the menu gives no tile", {
    when() {
        const nav = build(CATALOG)
        const group = title => nav.groups.find(candidate => candidate.title === title) || {items: []}
        return {nav, server: group("Server"), security: group("Security"), card: nav.featured[0] || {links: []}}
    },
    then({nav, server, security, card}) {
        // the hidden screen has no card and no link; neither has a screen below a parent that is not in the menu
        titles(nav.featured) == "Users and access"
        titles(server.items) == "Caches"
        // a screen its parent's menu does not show is dropped, and a group left with nothing is not returned
        !nav.groups.some(group => group.title === "Access")
        // the title and icon are the menu item's unless the entry has its own; the link is below the app root
        card.icon == "sym_o_security"
        card.link == "../Security"
        card.text == "Accounts and groups"
        server.items[0].icon == "sym_o_archive"
        security.items[0].title == "Users"
        security.items[0].icon == "sym_o_person"
        security.items[0].link == "../Security/UserAccount?disabled=N"
        // a card's links are checked as entries are: the hidden one is left out, the other takes the menu's title
        card.links.length == 1
        card.links[0].title == "Users"
        card.links[0].link == "../Security/UserAccount"
    }
})

feature("a menu item no entry names is in the last group, and a link outside the menus is kept", {
    when() {
        const nav = build(CATALOG, {otherTitle: "Other"})
        const last = nav.groups[nav.groups.length - 1]
        const elsewhere = nav.groups.find(group => group.title === "Elsewhere") || {items: []}
        // every menu item named: no last group
        const complete = build(CATALOG.concat([{path: "Voice", group: "Server"}]), {otherTitle: "Other"})
        // a hidden entry names its screen and shows nothing: no card, and not in the last group
        const hidden = build(CATALOG.concat([{path: "Voice", featured: true, hidden: true}]), {otherTitle: "Other"})
        return {nav, last, elsewhere, complete, hidden}
    },
    then({nav, last, elsewhere, complete, hidden}) {
        // Voice, mounted by another component, is not missing; the home itself and the named screens are not repeated
        last.title == "Other"
        titles(last.items) == "Voice"
        last.items[0].link == "../Voice"
        last.items[0].icon == "sym_o_call"
        // the groups keep the catalog's order, the last group after them
        nav.groups.map(group => group.title).join(", ") == "Server, Security, Elsewhere, Other"
        elsewhere.items[0].link == "//apps/Terminal"
        elsewhere.items[0].title == "Terminal UI"
        !complete.groups.some(group => group.title === "Other")
        titles(complete.groups[0].items) == "Caches, Voice"
        titles(hidden.featured) == "Users and access"
        !hidden.groups.some(group => group.items.some(item => item.path === "Voice"))
    }
})
