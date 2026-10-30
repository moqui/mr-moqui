// Generic location exclusions narrow a drive search before traversal and result limits.
const driveId = "TEST_SEARCH_EXCLUSION",
    root = "dbresource://test-resource-search-exclusion"
function search(extra = {}) {
    return ec.service
        .sync()
        .name("org.moqui.impl.FileServices.find#Resources")
        .parameters(Object.assign({driveId, text: "AcctSearchProbe", limit: 2}, extra))
        .disableAuthz()
        .throwOnError(true)
        .call()
}
spec({
    setupSpec() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        const fixtureDrive = ec.entity.makeValue("moqui.resource.ResourceDrive")
        fixtureDrive.set("driveId", driveId)
        fixtureDrive.setFields(
            {
                driveId,
                driveName: "Search exclusion fixture",
                driveKindEnumId: "RdkPersonal",
                ownerUserId: ec.user.userId,
                rootLocation: root
            },
            false,
            null,
            false
        )
        fixtureDrive.createOrUpdate()
        for (const path of [
            "public/AcctSearchProbe-one.txt",
            "public/AcctSearchProbe-two.txt",
            ...Array.from({length: 12}, (_, index) => "private/AcctSearchProbe-" + index + ".txt")
        ])
            ec.resource.getLocationReference(root + "/" + path).putText("Search fixture " + path)
    },
    setup() {
        login("john.doe")
        ec.artifactExecution.disableAuthz()
    }
})
feature("excluding a subtree preserves public matches beyond private results and the requested limit", {
    when() {
        return search({excludeLocationPrefixList: [root + "/private"]})
    },
    then(result) {
        result.resultList.length == 2
        result.resultList.every(row => row.path.startsWith("public/"))
        new Set(result.resultList.map(row => row.path)).size == 2
    }
})
feature("callers without exclusions retain the existing drive search", {
    when() {
        return search({limit: 30})
    },
    then(result) {
        result.resultList.length == 14
        result.resultList.some(row => row.path.startsWith("private/"))
        result.resultList.some(row => row.path.startsWith("public/"))
    }
})
feature("canonical database resource aliases and an excluded whole drive apply before search", {
    when() {
        return {
            alias: search({excludeLocationPrefixList: ["dbresource:/test-resource-search-exclusion//private/"]}),
            all: search({excludeLocationPrefixList: [root]})
        }
    },
    then({alias, all}) {
        alias.resultList.length == 2
        alias.resultList.every(row => row.path.startsWith("public/"))
        all.resultList == []
    }
})

feature("indexed exclusions run in the search query before its result limit", {
    when() {
        assert(ec.search.isAvailable(), "The framework search fixture requires the test search facade")
        ec.search.indexDocuments(
            [
                ...["one", "two"].map(name => ({
                    path: "public/AcctSearchProbe-" + name + ".txt",
                    modifiedDate: "2026-01-01T12:00:00Z"
                })),
                ...Array.from({length: 12}, (_, index) => ({
                    path: "private/AcctSearchProbe-" + index + ".txt",
                    modifiedDate: "2026-02-01T12:00:00Z"
                }))
            ].map(row =>
                Object.assign(
                    {
                        _type: "MoquiDriveFile",
                        _id: driveId + ":" + row.path,
                        driveId,
                        name: row.path.split("/").pop(),
                        isDirectory: "N",
                        fileType: "text"
                    },
                    row
                )
            )
        )
        return {all: search(), allowed: search({excludeLocationPrefixList: [root + "/private"]})}
    },
    then({all, allowed}) {
        all.usedIndex == true
        all.resultList.length == 2
        all.resultList.every(row => row.path.startsWith("private/"))
        allowed.usedIndex == true
        allowed.resultList.length == 2
        allowed.resultList.every(row => row.path.startsWith("public/"))
    }
})
