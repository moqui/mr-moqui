/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */
// The rows the acceptance spec InstanceDeploy (test/acceptance/client/InstanceDeploy.spec.js, plans/archive/Vault-Instances.md
// §4.6, M3) needs on its manager, made on the runtime's own database before the manager is started on it:
//
//   moqui test framework --acceptance --spec InstanceDeployFixtures --db existing
//
// - deploy.maker and deploy.approver, each with a password and an authenticator (TOTP) factor of a fixed secret, so their
//   REST sign-ins are strong ones, which a personal vault key needs, run after run: the spec makes each code from the
//   secret as an authenticator app does (a code works once, and a sign-in takes the next step's). The maker is in ADMIN
//   as well, for the entity REST, and DEPLOY_TEST_ROWS lets the group view the instance, action and run rows there,
//   which no artifact group of the framework has (System's screens read them). Both in
//   DEPLOY_TEST_ADMIN (INSTANCE_ADMIN and VAULT_ADMIN, sealed permissions, granted here with authorization off as a data
//   load grants them), which is the environment's approver group too;
// - the environment deploytest: its operational vault deploytest-vault (the spec makes the vault, with that id, in its
//   members' clients), a required pipeline release, a single-person deploy, an automatic unseal by transit;
// - the component repository mr-home (https://github.com/moqui/mr-home, a public one with a component and no code: a
//   managed instance is a production one, which deploys only from a public host, SEC-TLS-08) with a CI environment that
//   deploys to deploytest, and a passed run of release for its head, MR_HOME_COMMIT; no GitRepository row, which the
//   run and the CI environment name without a foreign key (a remote one would be fetched on a schedule). That run stands in for a CI build:
//   the gate itself is crates/moqui-instance's tests/deploy.rs. When mr-home's master moves on, change MR_HOME_COMMIT;
// - the images MoquiDeployBase, localhost/moqui:deploytest, the MoquiLocal image without mr-home (podman/build.sh puts
//   every add-on component in, and a component the image has is not deployed over it), and MoquiNext,
//   localhost/moqui:next, a second build of that one, its source the same passed run, so its digest differs and its
//   schema does not. Make them before the spec:
//     printf 'FROM localhost/moqui:latest\nRUN rm -rf /opt/moqui/components/addon/mr-home\n' | podman build -t localhost/moqui:deploytest -
//     printf 'FROM localhost/moqui:deploytest\nLABEL org.moqui.test=instance-deploy\n' | podman build -t localhost/moqui:next -
//
// Not demo data: no production load reaches it.

const MR_HOME_URL = "https://github.com/moqui/mr-home.git"
const MR_HOME_COMMIT = "d3c054c2b4fa3da83ce009df0ec3fb14dca7e82f"
const GROUP = "DEPLOY_TEST_ADMIN"
// username, password, factor id, the TOTP secret (base32 of 20 bytes; InstanceDeploy.spec.js holds the same)
const USERS = [
    ["deploy.maker", "DeployMaker1!!", "DEPLOY_TEST_MAKER", "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ"],
    ["deploy.approver", "DeployApprover1!!", "DEPLOY_TEST_APPROVER", "MFRGGZDFMZTWQ2LKNNWG23TPOBYXE43U"],
]

const one = (entity, fields) => ec.entity.find(entity).condition(fields).one()
function sys(run) {
    const wasDisabled = ec.artifactExecution.disableAuthz()
    try {
        return run()
    } finally {
        if (!wasDisabled) ec.artifactExecution.enableAuthz()
    }
}
// a row with these fields, made or brought back to them
function put(entity, fields) {
    const row = ec.entity.makeValue(entity)
    row.setFields(fields, true, null, null)
    row.createOrUpdate()
}

spec({
    setupSpec() {
        sys(() => {
            put("moqui.security.UserGroup", { userGroupId: GROUP, description: "InstanceDeploy: instance and vault admins" })
            for (const permission of ["INSTANCE_ADMIN", "VAULT_ADMIN"]) {
                if (one("moqui.security.UserGroupPermission", { userGroupId: GROUP, userPermissionId: permission }) == null) {
                    put("moqui.security.UserGroupPermission", { userGroupId: GROUP, userPermissionId: permission, fromDate: ec.user.nowTimestamp })
                }
            }
            for (const [username, password, factorId, secret] of USERS) {
                const existing = one("moqui.security.UserAccount", { username })
                const userId = existing != null ? existing.userId : call("org.moqui.impl.UserServices.create#UserAccount",
                    { username, newPassword: password, newPasswordVerify: password, userFullName: username }).userId
                const account = one("moqui.security.UserAccount", { userId })
                account.setFields({ currentPassword: ec.user.hashPassword(password), passwordHashType: "argon2", passwordSalt: "", passwordBase64: "N",
                    requirePasswordChange: "N", disabled: "N", successiveFailedLogins: 0, disabledDateTime: null }, true, null, null)
                account.update()
                put("moqui.security.UserAuthcFactor", { factorId, userId, factorTypeEnumId: "UafTotp", factorOption: secret,
                    fromDate: ec.user.nowTimestamp, thruDate: null, needsValidation: "N" })
                // the maker reads the rows over entity REST, which ADMIN reaches; the rows themselves are in no artifact
                // group (System's screens read them), so DEPLOY_TEST_ROWS below lets this group view them
                for (const group of username == "deploy.maker" ? [GROUP, "ADMIN"] : [GROUP]) {
                    if (one("moqui.security.UserGroupMember", { userId, userGroupId: group }) == null) {
                        put("moqui.security.UserGroupMember", { userId, userGroupId: group, fromDate: ec.user.nowTimestamp })
                    }
                }
            }
            put("moqui.security.ArtifactGroup", { artifactGroupId: "DEPLOY_TEST_ROWS", description: "InstanceDeploy: the rows it reads" })
            for (const entity of ["moqui.server.instance.AppInstance", "moqui.server.instance.InstanceAction", "moqui.resource.GitPipelineRun"]) {
                put("moqui.security.ArtifactGroupMember", { artifactGroupId: "DEPLOY_TEST_ROWS", artifactName: entity, artifactTypeEnumId: "AT_ENTITY",
                    nameIsPattern: "N", inheritAuthz: "N" })
            }
            put("moqui.security.ArtifactAuthz", { artifactAuthzId: "DEPLOY_TEST_ROWS_VIEW", userGroupId: GROUP, artifactGroupId: "DEPLOY_TEST_ROWS",
                authzTypeEnumId: "AUTHZT_ALLOW", authzActionEnumId: "AUTHZA_VIEW" })
            put("moqui.server.instance.InstanceEnvironment", { environmentId: "deploytest", environmentName: "InstanceDeploy acceptance",
                environmentTypeEnumId: "IetDemo", vaultId: "deploytest-vault", approverUserGroupId: GROUP, requiredPipeline: "release",
                singlePersonDeploy: "Y", machineAccess: "N", unsealPolicyEnumId: "IupAutomatic", unsealMethod: "transit" })
            put("moqui.resource.GitCiEnvironment", { environmentId: "ci-deploytest", repositoryId: "mr-home", name: "deploytest",
                instanceEnvironmentId: "deploytest", cloneUrl: MR_HOME_URL, requiredPipeline: "release" })
            put("moqui.resource.GitPipelineRun", { pipelineRunId: "deploytest-release", repositoryId: "mr-home", pipelineName: "release",
                event: "push", refName: "refs/heads/master", commitId: MR_HOME_COMMIT, trusted: "Y", protectedRef: "Y", statusId: "success",
                createdDate: ec.user.nowTimestamp, finishedDate: ec.user.nowTimestamp })
            put("moqui.server.instance.InstanceImage", { instanceImageId: "MoquiDeployBase", instanceKindId: "mr-moqui",
                imageName: "localhost/moqui:deploytest" })
            put("moqui.server.instance.InstanceImage", { instanceImageId: "MoquiNext", instanceKindId: "mr-moqui", imageName: "localhost/moqui:next",
                sourceRepositoryId: "mr-home", sourceRevision: MR_HOME_COMMIT })
        })
    },
})

feature("the InstanceDeploy rows are there, and its users sign in with a password and a code", {
    when() {
        return sys(() => ({
            users: USERS.map(([username, , factorId]) => ({ account: one("moqui.security.UserAccount", { username }), factor: one("moqui.security.UserAuthcFactor", { factorId }) })),
            environment: one("moqui.server.instance.InstanceEnvironment", { environmentId: "deploytest" }),
            run: one("moqui.resource.GitPipelineRun", { pipelineRunId: "deploytest-release" }),
        }))
    },
    then({ users, environment, run }) {
        users.every((user) => user.account != null && user.account.disabled == "N" && user.factor != null && user.factor.thruDate == null)
        environment.vaultId == "deploytest-vault"
        run.statusId == "success"
    },
})
