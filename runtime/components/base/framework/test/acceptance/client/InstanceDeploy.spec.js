// Deploys to managed instances on a real Podman host (plans/Vault-Instances.md §4.6, §6 M3; docs/tools/Component-Deploy.md,
// "A deploy over the link"): an acceptance test, since it needs a manager with a Podman host whose MR Moqui instances
// dial its manager link, an image to deploy, and a component repository on the manager with a passed pipeline run. The
// regression suite covers the rules with the fake engine and a fake link (crates/moqui-instance/src/tests/deploy.rs):
// the gate, the lease's items, one member at a time drained, and rollback refused across a schema change.
//
// The target is the manager's URL, signed in as an INSTANCE_ADMIN of the environment's approver group:
//
//   moqui test-client framework --acceptance --spec InstanceDeploy https://manager.example.com \
//     --param user=ops.admin --param password=... \
//     --param podmanHost=PodmanHost --param environment=staging \
//     --param nextImage=MoquiNext --param commit=<a commit of the component's repository its pipeline passed> \
//     --param component=site --param repositoryUrl=https://manager.example.com/git/site.git --param repositoryId=site
//
// The environment holds machine access (System › Vaults) to the items its deploys open, so no lease waits for an
// approver: its required pipeline's run for the commit must have passed. nextImage is an InstanceImage of the mr-moqui
// kind, another version than the one provisioned (MoquiLocal). Without podmanHost every feature skips, so the
// framework's acceptance run on a machine with no Podman passes over it.

const instances = params.instancesPath ?? "/qapps/system/Instances"
const tag = params.tag ?? `dep${Date.now() % 100000}`
const state = {}

spec({
    setupSpec() {
        if (!params.podmanHost) skip("no Podman host: --param podmanHost=<InstanceHost id> (and environment, nextImage, commit)")
        login(params.user ?? "john.doe", params.password ?? "moqui")
    },
})

// an instance's row, as the entity REST answers it to an INSTANCE_ADMIN
const instance = (id) => http.get(`/rest/e1/moqui.server.instance.AppInstance/${id}`, { accept: "json" }).json()
// its actions, newest first
const actions = (id) =>
    http
        .get("/rest/e1/moqui.server.instance.InstanceAction", {
            accept: "json",
            params: { appInstanceId: id, orderByField: "-startDate", pageSize: 20 },
        })
        .json()

const post = (path, form) => http.post(`${instances}/${path}`, { form })

feature("Postgres and MR Moqui are provisioned on the Podman host, and the instance pairs and reports unsealed", {
    when() {
        const created = (form) => {
            post("InstanceList/createAppInstance", form)
            return waitUntil(() =>
                http
                    .get("/rest/e1/moqui.server.instance.AppInstance", {
                        accept: "json",
                        params: { instanceName: form.instanceName },
                    })
                    .json()[0]?.appInstanceId,
            )
        }
        state.pg = created({
            instanceName: `pg-${tag}`,
            instanceImageId: "Postgres18",
            environmentId: params.environment,
            instanceHostId: params.podmanHost,
        })
        state.app = created({
            instanceName: `app-${tag}`,
            instanceImageId: params.firstImage ?? "MoquiLocal",
            environmentId: params.environment,
            instanceHostId: params.podmanHost,
            databaseInstanceId: state.pg,
        })
        post("InstanceDetail/requestInstanceAction", { appInstanceId: state.app, actionEnumId: "IatProvision" })
        const up = waitUntil(
            () => {
                const row = instance(state.app)
                return row.sealState == "unsealed" && row.linkState == "up" && row.observedState == "running" ? row : null
            },
            { timeout: 600, interval: 5000 },
        )
        state.firstDigest = up.imageDigest
        return { up, pg: instance(state.pg) }
    },
    then({ up, pg }) {
        pg.observedState == "running"
        up.observedState == "running"
        up.sealState == "unsealed"
        !!up.imageDigest
    },
})

feature("a component's commit is deployed over the link, gated by its passed run, as an action with its versions", {
    when() {
        if (!state.app) skip("the instances were not provisioned")
        post("InstanceDetail/requestDeploy", {
            appInstanceId: state.app,
            componentName: params.component ?? "site",
            repositoryUrl: params.repositoryUrl,
            refName: params.branch ?? "main",
            commitId: params.commit,
            repositoryId: params.repositoryId ?? "",
        })
        const done = waitUntil(
            () => actions(state.app).find((row) => row.actionEnumId == "IatDeploy" && row.statusEnumId != "IasRunning"),
            { timeout: 900, interval: 5000 },
        )
        state.componentAction = done.instanceActionId
        return { done, row: instance(state.app), backups: actions(state.pg) }
    },
    then({ done, row, backups }) {
        done.statusEnumId == "IasDone"
        done.componentCommitsAfter.includes(params.commit)
        row.componentCommits.includes(params.commit)
        // its database was backed up before the change
        backups.some((action) => action.actionEnumId == "IatBackup" && action.statusEnumId == "IasDone")
    },
})

feature("a new image replaces the container, keeping its volume and its seal, and comes back unsealed", {
    when() {
        if (!state.componentAction) skip("the component was not deployed")
        post("InstanceDetail/requestDeploy", {
            appInstanceId: state.app,
            instanceImageId: params.nextImage,
            commitId: params.imageCommit ?? params.commit,
        })
        const done = waitUntil(
            () =>
                actions(state.app).find(
                    (row) =>
                        row.actionEnumId == "IatDeploy" &&
                        row.instanceActionId != state.componentAction &&
                        row.statusEnumId != "IasRunning",
                ),
            { timeout: 900, interval: 5000 },
        )
        state.imageAction = done.instanceActionId
        return { done, row: instance(state.app) }
    },
    then({ done, row }) {
        done.statusEnumId == "IasDone"
        done.versionBefore == state.firstDigest
        row.imageDigest != state.firstDigest
        row.sealState == "unsealed"
        // the data volume, with its seal and database settings, stayed: the component deployed before is still there
        row.componentCommits.includes(params.commit)
    },
})

feature("rolling the image deploy back returns to the recorded digest, unless its schema changed", {
    when() {
        if (!state.imageAction) skip("the image was not deployed")
        const before = actions(state.app).find((row) => row.instanceActionId == state.imageAction)
        const res = post("InstanceDetail/rollbackDeploy", { appInstanceId: state.app, rollbackOfActionId: state.imageAction })
        const schemaChanged = before.schemaHashBefore && before.schemaHashBefore != before.schemaHashAfter
        const done = schemaChanged
            ? null
            : waitUntil(
                  () => actions(state.app).find((row) => row.actionEnumId == "IatRollback" && row.statusEnumId != "IasRunning"),
                  { timeout: 900, interval: 5000 },
              )
        return { res, done, schemaChanged, row: instance(state.app) }
    },
    then({ done, schemaChanged, row }) {
        // a schema change is not rolled back; else the digest recorded before the deploy is the instance's again
        schemaChanged || done.statusEnumId == "IasDone"
        schemaChanged || row.imageDigest == state.firstDigest
        row.sealState == "unsealed"
    },
})

feature("the instances are removed with their volumes", {
    when() {
        if (!state.app) skip("nothing was provisioned")
        for (const id of [state.app, state.pg]) {
            post("InstanceDetail/requestInstanceAction", { appInstanceId: id, actionEnumId: "IatRemove", removeVolumes: "true" })
        }
        return waitUntil(() => [instance(state.app), instance(state.pg)].every((row) => !row.instanceUuid), {
            timeout: 300,
            interval: 5000,
        })
    },
    then(removed) {
        removed
    },
})
