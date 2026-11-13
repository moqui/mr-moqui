// The access and grant review as one list for the screen, its print and its CSV (System › Access › AccessReview;
// plans/archive/Admin-Screens-Down.md, first IronStone Admin's): view(found) makes the rows of
// org.moqui.admin.AccessServices.get#GrantReview's answer, for each group reviewed its permission grants, its app gates
// with their row rules and its members, and a row for each group with what the review leaves out of it (sealed grants,
// counted). Every member is named, unless the caller names only some: memberIds is the users an app that scopes members
// may name (those of the organizations a user administers), the others counted per group (outsideCount) and never named,
// with outsideNote what a group's note says of them after their count.
const dateText = value => (value == null || Number(value) <= 0 ? "" : ec.l10n.format(Number(value), "yyyy-MM-dd HH:mm"))

function view(found, memberIds, outsideNote) {
    const named = memberIds ? new Set(Array.from(memberIds)) : null
    const members = Array.from(found.memberList || [])
    const reviewRows = []
    const groupRows = []
    for (const group of Array.from(found.groupList || [])) {
        const base = {userGroupId: group.userGroupId, groupDescription: group.groupDescription}
        for (const entry of Array.from(found.entryList || []).filter(row => row.userGroupId === group.userGroupId))
            reviewRows.push(
                Object.assign({}, base, {
                    entryText: entry.entryType === "appGate" ? "App gate" : "Permission",
                    name: entry.id,
                    detail: entry.label && entry.label !== entry.id ? entry.label : "",
                    kindLabel: entry.kindLabel || "",
                    fromText: dateText(entry.fromDate),
                    thruText: dateText(entry.thruDate),
                    scope: entry.scope || ""
                })
            )
        const groupMembers = members.filter(row => row.userGroupId === group.userGroupId)
        for (const member of groupMembers.filter(row => !named || named.has(row.userId)))
            reviewRows.push(
                Object.assign({}, base, {
                    entryText: "Member",
                    name: member.userFullName || member.username,
                    detail: member.username || "",
                    kindLabel: "",
                    fromText: dateText(member.fromDate),
                    thruText: dateText(member.thruDate),
                    scope: ""
                })
            )
        const outsideCount = named ? groupMembers.filter(row => !named.has(row.userId)).length : 0
        groupRows.push(
            Object.assign({}, base, {
                appName: group.appName || "",
                privileged: !!group.privileged,
                memberCount: group.memberCount,
                outsideCount,
                sealedOmitted: group.sealedOmitted || 0,
                noteText: [
                    group.privileged ? "Privileged group" : "",
                    group.sealedOmitted ? group.sealedOmitted + " sealed grant(s) not listed" : "",
                    outsideCount ? outsideCount + (outsideNote || " member(s) not listed") : ""
                ]
                    .filter(Boolean)
                    .join("; ")
            })
        )
    }
    return {
        reviewRows,
        groupRows,
        asOfText: dateText(found.asOfDate),
        groups: groupRows.map(row => row.userGroupId).join(","),
        sealedOmittedCount: found.sealedOmittedCount || 0,
        outsideCount: groupRows.reduce((sum, row) => sum + row.outsideCount, 0)
    }
}

;({view})
