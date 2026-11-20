// System › Instances (framework/service/org/moqui/dev/InstanceServices.xml, crates/moqui-instance): managed instances
// start containers and make databases on the hosts their rows name, so every InstanceServices service asks the sealed
// INSTANCE_ADMIN, which an ADMIN outside ADMIN_ADV does not have; the configuration rows that hold a command, a script or
// a service name are not written through the generic tools, nor an instance's environment or secret bindings
// (plans/Security-Review.md SEC-TLS-02, plans/Vault-Instances.md §4.1).
// Run: ./moq test tools --spec HostedInstances
const ADMIN_ONLY = "SPEC_HI_ADMIN"
const HOLDER = "SPEC_HI_HOLDER"
const make = (entityName, fields) => {
    if (ec.entity.find(entityName).condition(fields).count() > 0) return
    const row = ec.entity.makeValue(entityName)
    row.setFields(fields, true, null, null)
    row.create()
}
// a System › Instances transition as the user, as its button posts it: the services run under the screen's authorization
function as(userId, transition, parameters) {
    ec.user.internalLoginUser(userId.toLowerCase().replace(/_/g, "."))
    const res = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml")
        .render("Instances/InstanceList/" + transition, parameters, "post")
    const error = res.errorMessages.join(" ")
    ec.message.clearErrors()
    ec.user.logoutUser()
    return { error }
}

spec({
    setupSpec() {
        ec.artifactExecution.disableAuthz()
        for (const userId of [ADMIN_ONLY, HOLDER]) {
            make("moqui.security.UserAccount", { userId, username: userId.toLowerCase().replace(/_/g, "."), userFullName: "Spec " + userId })
            make("moqui.security.UserGroupMember", { userGroupId: "ADMIN", userId, fromDate: ec.user.nowTimestamp })
        }
        make("moqui.security.UserGroup", { userGroupId: "SPEC_HI_HOLDERS", description: "Spec hosted instance holders" })
        make("moqui.security.UserGroupPermission", { userGroupId: "SPEC_HI_HOLDERS", userPermissionId: "INSTANCE_ADMIN", fromDate: ec.user.nowTimestamp })
        make("moqui.security.UserGroupMember", { userGroupId: "SPEC_HI_HOLDERS", userId: HOLDER, fromDate: ec.user.nowTimestamp })
        ec.artifactExecution.enableAuthz()
    },
})

feature("an ADMIN without INSTANCE_ADMIN is refused by the instance services; a holder passes the check", {
    when() {
        const provision = { appInstanceId: "SPEC_HI_NONE", actionEnumId: "IatProvision" }
        const refused = as(ADMIN_ONLY, "requestInstanceAction", provision)
        const created = as(ADMIN_ONLY, "createAppInstance", { instanceImageId: "Postgres18", instanceHostId: "LocalDocker",
            environmentId: "SPEC_HI_ENV", instanceName: "spec-hi" })
        const held = as(HOLDER, "requestInstanceAction", provision)
        return { refused, created, held }
    },
    then({ refused, created, held }) {
        refused.error.includes("INSTANCE_ADMIN")
        created.error.includes("INSTANCE_ADMIN")
        !held.error.includes("INSTANCE_ADMIN")
        held.error.includes("No AppInstance SPEC_HI_NONE")
    },
})

// an instance made from its kind: its page names each secret by its binding, never a value, and previews what a
// provision makes and the lease it needs (rendered once, in html: the screen has no mode-specific template)
feature("an instance's page shows its secret bindings and its provision preview", {
    when() {
        ec.artifactExecution.disableAuthz()
        make("moqui.server.instance.InstanceEnvironment", { environmentId: "SPEC_HI_ENV", vaultId: "SPEC_HI_VAULT" })
        ec.artifactExecution.enableAuthz()
        const created = as(HOLDER, "createAppInstance", { instanceImageId: "Postgres18", instanceHostId: "LocalDocker",
            environmentId: "SPEC_HI_ENV", instanceName: "spec-hi-pg", hostPort: "25499" })
        ec.user.internalLoginUser("spec.hi.holder")
        const res = ec.screen.makeTest().rootScreen("component://tools/screen/System.xml").renderMode("html")
            .render("Instances/InstanceDetail", { appInstanceId: "spec-hi-pg" }, null)
        const out = { created: created.error, errors: res.errorMessages.join(" "), output: String(res.output || "") }
        ec.message.clearErrors()
        ec.user.logoutUser()
        return out
    },
    then({ created, errors, output }) {
        created == ""
        errors == ""
        output.includes("AppInstance:spec-hi-pg:admin")
        output.includes("/run/secrets/postgres-password")
        output.includes("POSTGRES_PASSWORD_FILE")
        output.includes("It opens no vault item")
        output.includes("Deposited into vault SPEC_HI_VAULT")
    },
})

feature("the generic tools do not write an InstanceImage, whose authTokenCmd runs on the host", {
    when() {
        login("john.doe")
        const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Browse/AutoEdit/AutoEditMaster/update",
            { aen: "moqui.server.instance.InstanceImage", instanceImageId: "SPEC_HI_IMAGE", authTokenCmd: "touch /tmp/spec-hi" }, "post")
        ec.message.clearErrors()
        const written = ec.entity.find("moqui.server.instance.InstanceImage").condition("instanceImageId", "SPEC_HI_IMAGE").disableAuthz().count()
        return { errors: res.errorMessages.join(" "), written }
    },
    then({ errors, written }) {
        errors.includes("not available through this tool")
        written == 0
    },
})

// Service Run calls the service a request names: an entity-auto service of an entity no generic write takes is refused
// there too, as Service REST refuses it, and other services still run (plans/Security-Review-Medium.md SEC-TLS-85)
feature("Service Run does not call the entity-auto services of a hosted-instance row or a service-written row", {
    when() {
        const run = (parameters) => {
            ec.user.internalLoginUser("spec.hi.admin")
            ec.message.clearAll()
            const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Service/ServiceRun/run", parameters, "post")
            const out = { errors: res.errorMessages.join(" ") + " " + ec.message.errors.join(" ") }
            ec.message.clearAll()
            ec.user.logoutUser()
            return out
        }
        // a host type names the services a hosted instance runs
        const hostType = run({ serviceName: "create#moqui.server.instance.InstanceHostType", hostTypeId: "SPEC_HI_HOST",
            startService: "org.moqui.impl.SystemMessageServices.send#AllProducedSystemMessages" })
        const ingest = run({ serviceName: "create#moqui.monitor.IngestKey", keyId: "SPEC_HI_KEY", source: "spec" })
        // an instance's secret binding decides which vault item its provision opens
        const binding = run({ serviceName: "create#moqui.server.instance.AppInstanceSecret", appInstanceId: "SPEC_HI_AI",
            secretName: "admin", bindingKey: "DatabaseHost:other:admin" })
        const allowed = run({ serviceName: "create#moqui.basic.EnumerationType", enumTypeId: "SPEC_HI_RUN", description: "Spec Service Run" })
        ec.artifactExecution.disableAuthz()
        const count = (entityName, field, value) => ec.entity.find(entityName).condition(field, value).count()
        const written = { hostType: count("moqui.server.instance.InstanceHostType", "hostTypeId", "SPEC_HI_HOST"),
            ingest: count("moqui.monitor.IngestKey", "keyId", "SPEC_HI_KEY"),
            binding: count("moqui.server.instance.AppInstanceSecret", "appInstanceId", "SPEC_HI_AI"),
            type: count("moqui.basic.EnumerationType", "enumTypeId", "SPEC_HI_RUN") }
        ec.entity.find("moqui.basic.EnumerationType").condition("enumTypeId", "SPEC_HI_RUN").deleteAll()
        ec.entity.find("moqui.server.instance.InstanceHostType").condition("hostTypeId", "SPEC_HI_HOST").deleteAll()
        ec.entity.find("moqui.monitor.IngestKey").condition("keyId", "SPEC_HI_KEY").deleteAll()
        ec.artifactExecution.enableAuthz()
        return { hostType, ingest, binding, allowed, written }
    },
    then({ hostType, ingest, binding, allowed, written }) {
        written.hostType == 0
        written.ingest == 0
        written.binding == 0
        binding.errors.includes("written only by its own services")
        hostType.errors.includes("written only by its own services")
        ingest.errors.includes("written only by its own services")
        allowed.errors.trim() == ""
        written.type == 1
    },
})

// Service Run as a generic tool: an account's password and an existing account's identity are not set through its
// entity-auto services, as the generic tools' protectUserAccounts refuses them; other fields still are
feature("Service Run sets no account's password and changes no existing account's identity", {
    when() {
        ec.artifactExecution.disableAuthz()
        make("moqui.security.UserAccount", { userId: "SPEC_HI_TARGET", username: "spec.hi.target", userFullName: "Spec Target",
            emailAddress: "target@spec.invalid", currentPassword: "spec-original-hash", passwordHashType: "SHA-256" })
        ec.artifactExecution.enableAuthz()
        const run = (parameters) => {
            ec.user.internalLoginUser("spec.hi.admin")
            ec.message.clearAll()
            const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Service/ServiceRun/run",
                Object.assign({ serviceName: "update#moqui.security.UserAccount", userId: "SPEC_HI_TARGET" }, parameters), "post")
            const out = res.errorMessages.join(" ") + " " + ec.message.errors.join(" ")
            ec.message.clearAll()
            ec.user.logoutUser()
            return out
        }
        const password = run({ currentPassword: "spec-forged-hash", passwordHashType: "NONE" })
        const email = run({ emailAddress: "taken@spec.invalid" })
        const name = run({ userFullName: "Spec Renamed" })
        ec.artifactExecution.disableAuthz()
        const stored = ec.entity.find("moqui.security.UserAccount").condition("userId", "SPEC_HI_TARGET").useCache(false).one()
        const out = { password, email, name, currentPassword: stored.currentPassword, emailAddress: stored.emailAddress,
            userFullName: stored.userFullName }
        ec.entity.find("moqui.security.UserAccount").condition("userId", "SPEC_HI_TARGET").deleteAll()
        ec.artifactExecution.enableAuthz()
        return out
    },
    then({ password, email, name, currentPassword, emailAddress, userFullName }) {
        password.includes("is not taken by a generic call")
        email.includes("is changed through the account services")
        name.trim() == ""
        currentPassword == "spec-original-hash"
        emailAddress == "target@spec.invalid"
        userFullName == "Spec Renamed"
    },
})

// A ServiceJob's run calls the service its row names with authorization off, so it is a generic call: an entity-auto
// service a generic write refuses does not run; the generic tools write no job row; and a seed job still runs
feature("a service job's entity-auto run of a refused entity is refused, no generic tool writes a job, and a seed job runs", {
    when() {
        ec.artifactExecution.disableAuthz()
        make("moqui.service.job.ServiceJob", { jobName: "SPEC_HI_JOB", serviceName: "create#moqui.server.instance.InstanceHostType", paused: "N" })
        make("moqui.service.job.ServiceJobParameter", { jobName: "SPEC_HI_JOB", parameterName: "hostTypeId", parameterValue: "SPEC_HI_JOB_HOST" })
        ec.artifactExecution.enableAuthz()
        // an administrator's run now, as System › Operations' runJobNow starts it (whose screen reads the job)
        login("john.doe")
        ec.artifactExecution.disableAuthz()
        const refusedRun = ec.service.job("SPEC_HI_JOB").run()
        const seedRun = ec.service.job("clean_ChatChanges_daily").run()
        // each run on a worker, until both have ended (at most about 20 seconds)
        const runOf = (jobRunId) => ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobRunId", jobRunId).useCache(false).one()
        const ended = (jobRunId) => runOf(jobRunId)?.endTime != null
        const until = Date.now() + 20000
        while (Date.now() < until && !(ended(refusedRun) && ended(seedRun))) ec.factory.waitWorkerPoolEmpty(1)
        ec.artifactExecution.enableAuthz()
        const res = ec.screen.makeTest().rootScreen("component://tools/screen/Tools.xml").render("Data/Browse/AutoEdit/AutoEditMaster/update",
            { aen: "moqui.service.job.ServiceJob", jobName: "SPEC_HI_JOB", serviceName: "create#moqui.basic.EnumerationType" }, "post")
        ec.message.clearErrors()
        ec.artifactExecution.disableAuthz()
        const out = { browse: res.errorMessages.join(" "), refused: runOf(refusedRun), seed: runOf(seedRun),
            hosts: ec.entity.find("moqui.server.instance.InstanceHostType").condition("hostTypeId", "SPEC_HI_JOB_HOST").count(),
            serviceName: ec.entity.find("moqui.service.job.ServiceJob").condition("jobName", "SPEC_HI_JOB").useCache(false).one().serviceName }
        out.refused = { hasError: out.refused?.hasError, errors: String(out.refused?.errors || "") }
        out.seed = { hasError: out.seed?.hasError, ended: out.seed?.endTime != null, errors: String(out.seed?.errors || "") }
        ec.entity.find("moqui.service.job.ServiceJobParameter").condition("jobName", "SPEC_HI_JOB").deleteAll()
        ec.entity.find("moqui.service.job.ServiceJobRun").condition("jobName", "SPEC_HI_JOB").deleteAll()
        ec.entity.find("moqui.service.job.ServiceJob").condition("jobName", "SPEC_HI_JOB").deleteAll()
        ec.artifactExecution.enableAuthz()
        return out
    },
    then({ browse, refused, seed, hosts, serviceName }) {
        hosts == 0
        refused.hasError == "Y"
        refused.errors.includes("written only by its own services")
        seed.ended == true
        seed.errors == ""
        seed.hasError != "Y"
        browse.includes("not available through this tool")
        serviceName == "create#moqui.server.instance.InstanceHostType"
    },
})
