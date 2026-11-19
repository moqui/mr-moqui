# Pipelines

MR Moqui runs pipelines for the git repositories it hosts ([Git-Hosting](Git-Hosting.md)): a file in the repository
says what to run, a push or a merge request queues a run, each job runs in a container on another machine (or on a
registered runner), and each job's result is a commit status, which the branch rules already gate merges on. This
guide is for whoever sets it up and for an application that shows runs. No Java counterpart. The plan is
[Dogfood](../../plans/Dogfood.md) §8; the code is `crates/moqui-git/src/ci`.

Contents: [the file](#the-file) · [events](#events) · [jobs](#jobs) · [the built-in pipeline](#the-built-in-pipeline) ·
[statuses](#statuses) · [trust and approval](#trust-and-approval) · [secrets](#secrets) ·
[runners and drivers](#runners-and-drivers) · [images](#images) · [job tokens](#job-tokens) ·
[the runner REST](#the-runner-rest) · [moqui ci-run](#moqui-ci-run) · [deployments](#deployments) ·
[records](#records) · [services](#services) · [the engine](#the-engine) · [limits and conf](#limits-and-conf) ·
[tests](#tests) · [not built](#not-built)

## Set it up

1. A machine with Docker or Podman that is **not the server**, reachable from it, and from which the server's URL is
   reachable ([runners and drivers](#runners-and-drivers)).
2. An `InstanceHost` for it and at least one `GitRunnerPool` (System data, or `store#RunnerPool`), with the images it
   allows. The engine queues nothing while there is no enabled pool, so a server that runs no CI writes no statuses.
3. The `moqui-build` image on that machine ([images](#images)).
4. `git-ci` `server-url` when runners reach the server by another URL than the public one, and `run-url` so statuses
   link to a page. A container's runner sends its job token to that URL, so it must be `https` (or loopback, or the
   server has `git-server` `allow-http`): with neither `server-url` nor the pool's `serverUrl` the engine gives it the
   git server's public URL (`git-server` `public-url`, else from the webapp's host and port), and a job whose URL
   would be plain HTTP to another host is not started but ended as an `error` that names the conf to set.
5. A `ci.xml` in a repository, or a repository that is a Moqui component.

## The file

`ci.xml` at the repository's root, read at the commit a run is for. Its schema is `framework/xsd/git-ci-3.xsd`
(`moqui check` validates it, and the Dev AIDE's XML editing knows it).

```xml
<ci>
    <pipeline name="check" on="push merge-request" branches="main release/*">
        <env name="CARGO_BUILD_JOBS" value="4"/>
        <job name="framework" image="moqui-build" pool="linux" timeout-minutes="30">
            <cache key="cargo-${hash:Cargo.lock}" path="target"/>
            <step run="cargo lint"/>
            <step name="Tests" run="./moq test-framework"/>
            <artifact name="logs" path="runtime/log" retention-days="7"/>
            <report junit="test-results/*.xml"/>
        </job>
        <job name="package" image="moqui-build" pool="linux" needs="framework" environment="staging">
            <secret name="DEPLOY_KEY"/>
            <step run="./moq archive"/>
        </job>
    </pipeline>
    <pipeline name="nightly" on="schedule manual" cron="0 0 3 * * ?">
        <job name="acceptance" image="moqui-build" pool="linux" timeout-minutes="120">
            <step run="./moq accept-all --junit test-results"/>
            <report junit="test-results/*/*.xml"/>
        </job>
    </pipeline>
</ci>
```

The reader is strict. An element or attribute the schema does not have, a name that is not letters, digits, dot, dash
and underscore, a path that leaves the checkout, a job that needs a job that is not there (or needs itself through
others), or a file over the server's limits is an error with its line. A run of such a file is one `error` run with
no jobs and a status `ci/ci.xml` that says `ci.xml line 3: job j has no <step>`.

## Events

| `on` | When | The run's ref and commit |
| --- | --- | --- |
| `push` | A branch moved, by a push or by a write through Moqui (a merge, an edit on a page). `branches` narrows it. | The branch and its new head |
| `tag` | A tag was created. `tags` narrows it. | The tag and the commit it peels to |
| `merge-request` | A merge request was opened, reopened, made ready, or its head moved. `branches` is matched against its target. | The target branch, and the merge request's head |
| `schedule` | The pipeline's `cron` came due (six fields, seconds first, as a ServiceJob's). Read from the default branch. | The first branch `branches` names, else the default branch, and its head |
| `manual` | `run#Pipeline`, by a user with the update right | The branch or tag asked for |
| `deploy` | `request#GitDeployment`: the environment's `requiredPipeline` only ([deployments](#deployments)) | The branch asked for and its head |

Pushes and tags are the framework's gitECA rule `FrameworkQueuePipelineRuns` (`Git.gitecas.xml`, `post-receive`, the
sources `push` and `write`; not a mirror's fetch or an import, which nobody with the update right pushed). A merge
through `merge#MergeRequest` or `merge#GitRefs` is a write of the target branch, so its push pipelines run once for
the merge, as the user who merged. A push to `refs/for/<branch>` is no branch's and the head ref of a merge request
(`refs/merge-requests/<n>/head`) raises no event: neither queues a push pipeline.

Merge requests are a service ECA rule on `queue#MergeRequestEvent` (`GitCi.secas.xml`,
`GitCiQueueMergeRequestRuns`), which every change of a merge request goes through
([Git-Hosting](Git-Hosting.md#merge-requests)): one of a branch, one from a fork, one pushed for review
(`git push origin HEAD:refs/for/main`, a proposal of `commit#GitFiles`), and a hidden one, for which the service
sends no webhook and still runs. The rule calls the engine with authorization disabled, and does not run after an
event that was refused (a caller of `queue#MergeRequestEvent` without the update right). The run is in the merge
request's repository, the target, at the commit its `refs/merge-requests/<n>/head` has there, whatever the source:
the objects of a fork's branch were copied to the target when the merge request took them, so no run reads another
repository.

Schedules are `GitPipelineSchedule` rows, written whenever the default branch moves, so the schedule job reads no
file.

An event delivered twice makes one run: a run of the same pipeline, commit, event, ref and merge request that has not
ended is not queued again. `opened` for a head that has had its run queues none, ended or not: a merge request that
is shown (`set#MergeRequestHidden`) is told its webhooks as opened, and already has the run it got while hidden.
`rerun#PipelineRun` runs a head again.

`queue#PushRuns` and `queue#MergeRequestRuns` are the rules' and are refused without authorization disabled: the
user and the commit of a run are its event's, never a caller's.

**The commit of a run** is read as the system reads ([Reachability](Git-Hosting.md#reachability)): a commit a
branch, a tag or a merge request's head reaches, a hidden head too. In a fork network, and in a repository that has
had a hidden merge request, a commit no ref of the repository reaches any more (another repository's, a head its
merge request has left) has no pipeline, so a run, or a re-run of an old one, is no way to read one.

## Jobs

A job is steps run in order in one container, in a shallow checkout of the commit (`fetch-depth`, 1). A step is a
shell command (`sh -c`); one that fails ends the job unless it says `continue-on-error`. The jobs of a pipeline run in
parallel unless one `needs` another: it then starts when each needed job has succeeded and is `skipped` when one did
not.

- **`env`** on the pipeline and on a job. Every job also has `CI`, `MOQUI_CI`, `MOQUI_CI_COMMIT`, `MOQUI_CI_REF`,
  `MOQUI_CI_EVENT`, `MOQUI_CI_PIPELINE`, `MOQUI_CI_JOB`, `MOQUI_CI_RUN_NUM`, `MOQUI_CI_REPOSITORY` and
  `MOQUI_CI_MERGE_REQUEST`.
- **`matrix`**: the job runs once for each combination of its axes' values, each a job of its own.
  `${matrix.NAME}` is expanded in the image, env values, step commands, cache keys, artifact names and service
  images, and the job has `MATRIX_NAME`. A job that needs a matrix job waits for all of them.
- **`cache`**: a directory kept between runs by key. `${hash:FILE}` in the key is the SHA-256 of a file of the
  checkout. It is restored before the first step (at most `git-ci` `max-unpack-bytes`, 16 GiB, and a million entries;
  regular files, directories and
  links that stay in the workspace, resolved through the links already restored; past a limit it stops and the log says
  it was not restored), and saved after the last when the job succeeded and nothing is
  stored under the key. Caches are per repository and apart for protected refs: a run for a protected ref reads only
  caches such a run wrote, an untrusted run never reads them (a build with secrets may have filled one), and another
  trusted run reads both. A cache of a job that was given secrets (an environment whose `allowedRefs` names a branch
  no rule protects) is of the protected class too. A key with a `/` outside its `${hash:...}` parts, or longer than 200 characters once each
  hash is its 16 digits, is an error of the file.
- **`artifact`**: files uploaded when the steps have run, also of a failed job, as one tar.gz download with a size, a
  checksum and an expiry (`retention-days`).
- **`report`**: JUnit XML files. Each is a download, and each failed test becomes a row with its suite, name, file,
  line and message (`moqui test --junit` writes such files; the file and line come from the test case's attributes,
  else from the first `path:line` of the failure's text).
- **`service`**: a container beside the job (a database), reached by its name as a host name, removed with the job.
  At most `git-ci` `max-services` (4) a job; its image must be one the pool allows, as the job's is, and it is kept in
  as the job's container is ([runners and drivers](#runners-and-drivers)).
- **`secret`**: a secret the job asks for, as an environment variable ([secrets](#secrets)).

A path (`cache`, `artifact`, `report`) is inside the checkout; `*` in a segment is any run of characters of one name.

## The built-in pipeline

A repository whose root has a `component.xml` and no `ci.xml` gets one pipeline, `component`, on push and
merge-request, with one job, `test` (the status context `ci/component/test`). It runs the component's regression
suite as `./moq test <component> --junit --workers N` runs it locally, so a spec over the regression budget fails it
here as it does there ([Build-Test](../Build-Test.md#the-two-suites)).

The job's script (`crates/moqui-git/src/ci/builtin-component.sh`) needs an image with an MR Moqui tree at
`MOQUI_HOME` (`/opt/moqui`): `moq`, `addons.xml`, the runtime and a built `moqui` binary, with git and bash. The
`moqui-build` image is one. The script copies the tree beside the checkout when it cannot write it (a job's root is
read only), copies the checkout to `runtime/components/addon/<name>`, gets what the component depends on with
`./moq component-get <name>`, and runs the tests with their JUnit files as the job's report.

The framework version: a `component.xml` that names one (`<depends-on name="framework" version="4.2.0"/>`) is tested
only in an image of that version, and the job fails when they differ. `git-ci` `builtin-image` may carry
`{frameworkVersion}` (`registry.example.com/moqui-build:{frameworkVersion}`; `latest` for a component that names
none), so each component runs in the image of its framework. `builtin-pool` names the pool, `builtin-workers` the
workers, and `builtin-enabled="false"` turns it off.

## Statuses

Each job run writes a `GitCommitStatus` of the run's commit with the context `ci/<pipeline>/<job>`, or
`ci/<pipeline>/<job> (value, value)` for a matrix job: `pending` when it is queued (or waits for approval), then
`success`, `failure` (a step failed, or a job it needs did not succeed) or `error` (timed out, cancelled, no pool, a
refused run). A branch rule's `requiredStatusContexts` names the contexts a merge needs, as for outside CI. The
contexts a merge request's run writes are those its **head's** `ci.xml` defines, as the run reads the file at the
head: a merge request can drop or rename a job, and then its context is never written, so the merge waits, but it can
also write a context the target branch's pipeline does not have. Name in `requiredStatusContexts` the contexts the
target branch's pipeline defines (the engine does not read the target's file to decide which are required; the
branch rule's list is what decides). The
status's target is the run's page: what `git-ci` `run-url-service` answers
(`org.moqui.git.GitCiServices.get#PipelineRunUrl`), else (no service, no `url` from it for a repository whose page is
not the application's, or a service that failed) the `run-url` template with `{repositoryId}`, `{repositoryPath}`,
`{pipelineRunId}`, `{runNum}` and `{jobRunId}`.

The engine writes them through `create#GitCommitStatus`, as the system: with authorization disabled and
`includeHidden`, so the status of a hidden merge request's head is written where a user's is not, a service ECA rule
on `create#GitCommitStatus` runs for the engine's statuses too (an application's notice of a failed check), and an
application may keep the `ci/` contexts from users (`git-server` `reserved-status-contexts="ci/"`,
[Git-Hosting](Git-Hosting.md#commit-statuses)): without that a user with a push right can post a `ci/...` status
themselves. A description is plain text: `<` and `>` are left out of it (the run's `statusMessage` keeps a message
whole). Where a commit its ref has left is not read (above), its status is not written either (a head replaced
while its job ran); the job's own state stands.

## Trust and approval

A run is **trusted** only when it is for a ref of the repository itself, pushed or asked for by an account with the
update right. A schedule's run of the default branch or a protected one is trusted: it is the repository's own branch by
its own file; one of another branch, which an account without the update right may make, is trusted only as a push of
its head would be, by the account that brought the head (`GitCommitPush`; untrusted when none is known). A run is **untrusted**
when it is for a merge request from a fork (`GitMergeRequest.sourceRepositoryId`), one pushed for review
(`pushedForReview`) or one that stopped following its source (`sourceDetachedDate`), whoever opened it and whatever
their rights (the commits are not a branch's of the repository), or was caused by an account without the update
right. An untrusted run:

- gets **no secrets**, whatever its jobs ask for;
- **reads caches and writes none**;
- runs only in a **pool that takes untrusted runs** (`GitRunnerPool.trusted` N), and a trusted run never runs there;
- **waits for approval** (`statusId` and `approvalState` `waiting`, statuses pending "Waiting for approval") until
  `approve#PipelineRun` is called by a user with the update right, or with authorization disabled. Approved, it is
  still untrusted.

A run with a job whose `environment` has `approvalRequired` waits for approval when it is queued, whatever its trust
(its `statusMessage` names the environment). Such a run is approved only by a user with the delete right on the
repository (a maintainer) who did not trigger it (the pusher), and `approvedByUserId` records who did.

A run refused by the check (`approvalState` `refused`) or never approved is not run again: `retry#JobRun` refuses
it, and for any other ended run asks the check again (with the run's event, ref, commit and the caller), so a run
the check would now refuse, hold or trust less is not run again either. Two retries of one job at once make one
attempt (the old one is marked superseded first, by one conditional update). A cancel ends a job whatever a claim
moved it to meanwhile, and a start pass or a runner's claim skips the jobs of a run that ended; a job cancelled
between its claim and its container's start gets no container (the start reads the job again, and a container started
as it was cancelled is removed). A start pass runs outside its caller's transaction, so a rollback never queues again
a job whose container runs; a claim whose later writes fail gives its job back to the queue, its token never handed
out. The new attempt keeps the pool the old one
ran in (`runnerPoolId`).

Before any run is queued the engine asks the service `git-ci` `run-check-service` names, with authorization
disabled, as the LFS upload check is asked (`org.moqui.git.GitCiServices.check#PipelineRun` is its interface). It
answers `decision`: `allow` (an untrusted run may start with no approval), `wait` (hold a trusted run too, for a
quota), `refuse` (an `error` run whose statuses carry the `message`; an error the service leaves refuses too) or
`skip` (nothing at all: no run, no status, for a repository whose pipelines the application has turned off; it is
asked too, as pipeline `ci.xml` with no jobs, before the `error` run of a file that is not valid). It
may lower `trusted` and never raise it, and may name the `ownerId` the run's secrets and usage count under and the
`runnerPoolId` its jobs use. With no service, or no decision, a trusted run is allowed and an untrusted one waits.
**A server whose accounts sign themselves up sets `run-check-service`**: with none, any account's push to its own
repository is a trusted run (it has the update right there), so the application decides whether a public account's
own repository runs without approval, and names each run's owner.

A scheduled pipeline runs at most once in `git-ci` `schedule-min-minutes` (15), whatever its cron says.

The engine never runs a step on the server.

## Secrets

A `GitCiSecret` is a repository's, or an owner's that the application names (a group, an account; the run's
`ownerId`). Its value is encrypted, written through `store#CiSecret` and never read back by a service. The secrets
and environments of a repository are managed (`find#CiSecrets`, `store#`, `delete#CiSecret`, `store#`,
`delete#CiEnvironment`) by a user with the **delete** right on it (a maintainer), not the update right a developer
who pushes has, or with authorization disabled; an owner's with authorization disabled only. A secret moved to another
scope or environment takes a new value: its old one was given for the old scope. In the same way an environment whose
`allowedRefs` gains a pattern, or whose `approvalRequired` is turned off, has the values of its secrets cleared (they are
given again for the environment as it is now), and one whose `targetUrl` changes without a new `token` loses its
token. A job is given a secret it asks for
only when its run is trusted and the secret's scope is met:

| Scope | Given when |
| --- | --- |
| `protected` | The run's ref is protected: a branch rule that names it keeps direct pushes to a push group (`pushUserGroupId`) or to merge requests with at least one approval (`requireMergeRequest` and `requiredApprovals` 1 or more; with none, a Developer merges their own). A rule that only forbids a force push or a delete protects nothing. Never for a merge request's run. |
| `tags` | The run's ref is a protected tag. |
| `environment` | The job's `environment` is the secret's environment, and the environment allows the run: never a merge request's run; a run of a protected ref, or of a branch its `allowedRefs` names; with `approvalRequired`, only an approved run. |

A secret that is not given leaves its variable unset. Its value is masked (`***`) in the job's log as the lines are
stored, whole and line by line for a value of several lines; a value under four characters is not masked. A step can
still write a secret somewhere else (an artifact, a request): give secrets only to pipelines whose file is reviewed,
which is what the protected scope does.

## Runners and drivers

A runner is never the server, nor on its machine or its network. A pool (`GitRunnerPool`) says where its jobs run:

| Field | |
| --- | --- |
| `poolName` | What a job's `pool` names; `default` for a job that names none. Two pools may share a name, one trusted and one not. |
| `poolKind` | `container`: the engine starts a container on `instanceHostId`. `registered`: the pool's runners ask for jobs. |
| `ownerId` | Whose runs it takes; a shared pool when empty. A pool of `registered` runners has one on a server with `run-check-service` (one with tenants): a runner keeps its machine from one job to the next. A shared pool takes no job of an owner that has its own pool of the job's pool name and trust. |
| `trusted` | Y: trusted runs only. N: untrusted runs only. |
| `allowedImages` | Image patterns (`*`). A job whose image none matches is an error; an empty list allows none. |
| `maxConcurrent`, `maxMinutes` | Jobs at once, and the longest a job runs. |
| `cpuLimit`, `memoryMb`, `diskMb`, `pidsLimit` | The container's limits. `diskMb` needs a storage driver that takes it. |
| `egressRule` | The container's network mode: `bridge` (the default), `none`, or a network of the host whose rules limit what a job reaches. A network is named by letters, digits, dot, dash and underscore only, and is no engine keyword, so never `host`, `container:...`, Podman's `ns:<path>`, `slirp4netns`, `pasta` or `private`, with options or without (refused when the pool is stored, and `none` if a row has one). |
| `serverUrl` | The URL the pool's runners reach the server by, when it is not `git-ci` `server-url`. |

The pools are an operator's: `find#RunnerPools`, `store#`, `delete#RunnerPool`, `create#RunnerPoolToken` and
`revoke#Runner` need the `GIT_CI_ADMIN` permission (the `ADMIN` group has it) or authorization disabled. It is
a sealed permission (`user-facade` `authz`): only a holder grants it to a group.

**Drivers.** An `InstanceHostType` names the services that start a run that ends, inspect it, remove it and clean up
(`runStartService`, `runInspectService`, `runRemoveService`, `runCleanupService`; the interfaces are
`start#InstanceRun`, `inspect#InstanceRun`, `remove#InstanceRun` and `cleanup#InstanceRuns` of
`org.moqui.dev.InstanceServices`). So a driver is a set of services and the engine does not change.

**The `docker` host type** speaks the Docker Engine API, which Podman serves on its socket too. An `InstanceHost`:

| `hostProtocol` | `hostAddress` | |
| --- | --- | --- |
| `unix` | `/run/podman/podman.sock`, `/var/run/docker.sock`, `/run/user/1000/podman/podman.sock` | A socket on the server's machine: for a try on one machine only, since a runner is not to be on the server's |
| `https` | a host name, with `adminPort` (2376) | TLS with `tlsCaCert`, `tlsClientCert` and `tlsClientKey` (PEM; the key encrypted) |
| `http` | `localhost`, `127.0.0.1` (127.0.0.0/8), `::1` or `::ffff:127.0.0.1`, with `adminPort` (2375) | No authentication: this machine only. Plain http to any other host is refused before anything is sent (SEC-OUT-08); reach it over `https`, or forward its socket |

The hosted-instance services (`init#InstanceDocker` and the others) reach the same rows through `call#DockerApi`,
on the same client and rule, with `adminPort` 2375 when the row has none.

Docker: `dockerd --tlsverify --tlscacert=ca.pem --tlscert=server-cert.pem --tlskey=server-key.pem -H 0.0.0.0:2376`.
Podman: `podman system service --time=0 tcp://0.0.0.0:2376` behind a TLS proxy, or the socket forwarded over SSH;
rootless Podman is the better host for untrusted runs. The server's certificate must name the host as the server
reaches it.

A job's container is created with: no privilege, every capability dropped, `no-new-privileges`, no host path or
socket mounted, no port published, an init as process 1 that reaps what a step leaves (so orphans do not use up
`pidsLimit`), the user `git-ci` `container-user` (1000:1000; root in no form, `0`, `0:x` or `root:x`; `image` runs the
image's own user, and a container whose image's user is not a numeric uid other than 0 (none, root, or a name, which
the image's own `/etc/passwd` may make uid 0) is refused before it starts), a read-only
root with a sized tmpfs at `/tmp` (`container-tmp-mb`) and the working directory (`container-workdir`, `/workspace`)
on a tmpfs of `container-workspace-mb` (4096; it counts toward `memoryMb`), or, when the pool sets `diskMb`, on the
root filesystem `diskMb` sizes, which is then writable: the working directory is never unbounded. Then the pool's
limits and network mode, the name `moqui-ci-<server id>-<jobRunId>` and the labels `moqui.ci.job`, `moqui.ci.run`
and `moqui.ci.server`. Its environment is the server's URL, the job token and the job run's id; the job's own
variables and its secrets are asked for by the runner with the token, and are never in the container's definition.

An image the host does not have is pulled for a trusted pool, one pull at a time a host (others go on), for at most
five minutes; **an untrusted pool's images are never pulled on demand**: one not on the host is an error, so pull them
there first. The containers of a start pass are started after its claims, outside the pass's lock, on a thread of
the engine's own (not the git server's workers), so a slow host holds no other start.

Service containers are started first, kept in as the job's container is (its user, no capabilities, a read-only root
with a sized tmpfs at `/tmp`, an init, the pool's CPU, memory and swap, pids and disk limits): a service image runs as
that user and writes under `/tmp` (Postgres with `PGDATA` there). They are on a network of the job's own, named after
its container, which reaches out only when the pool's `egressRule` is `bridge`: for `none` or a network of the host
it is internal, and the job's container is put on the pool's network too, so the job's egress is the pool's and a
service has none. A network of that name already there is an error (another run's), never shared. A job's or a
service's image (the image is inspected before its container is made) gets a tmpfs of `container-tmp-mb`, owned by
the container's user, at each path its image declares a `VOLUME` (a database service keeps its data there, within the
memory limit), where the engine would otherwise make an anonymous
volume on the host's disk, outside the pool's limits. A path no tmpfs covers (not absolute), and a container the
engine gave a volume anyway, are refused before it starts.

The names carry this server's id (`git-ci` `server-id`, else the public URL's host), and a removal or a cleanup
touches only containers with this server's label, so two servers can share a host. The cleanup leaves a container
made after it read which jobs run (a job that just started). The image must have a directory at the working directory
that the user can write when `diskMb` is set, as `moqui-build` has.

**The `fake` host type** starts nothing, records what it was asked (`FakeHosts` in
`crates/moqui-git/src/ci/driver/fake.rs`) and answers as the host's `hostAddress` says: `success` and `hang` (a
container that runs until it is removed), `fail` (it does not start), `lost` (the host does not know it) and `exit`
(it exited with no result). It is for the engine's tests.

**Registered runners** are for what a container cannot be (macOS, Windows): a long-lived machine that asks for jobs.
`create#RunnerPoolToken` gives a pool of the `registered` kind a registration token (shown once, stored hashed). On
the machine:

```sh
MOQUI_CI_SERVER_URL=https://moqui.example.com MOQUI_CI_POOL_TOKEN=mqcp_... moqui ci-run register --name mac-mini-1 > ~/.moqui-ci-token
chmod 600 ~/.moqui-ci-token
MOQUI_CI_SERVER_URL=https://moqui.example.com moqui ci-run poll --token-file ~/.moqui-ci-token --pass-env CARGO_HOME
```

`register` prints the runner token, once. `poll` reads it from `--token-file` or the file
`MOQUI_CI_RUNNER_TOKEN_FILE` names; `MOQUI_CI_RUNNER_TOKEN` itself still works, with a warning, since a process's
starting environment can be read by any process of its user (`/proc/<pid>/environ`), a job's steps included. The
runner token takes any job of the pool and, through that job's token, its secrets, so it is kept out of the steps
(plans/Security-Review.md SEC-GIT-02): a registered runner's step does not inherit the machine's environment, only
`PATH`, `HOME`, `USER`, `LOGNAME`, `LANG`, `LANGUAGE`, `TZ`, `TMPDIR`, `SHELL`, `TERM`, the `LC_` variables and the names
`--pass-env NAME,...` gives, and then the job's own variables; no runner, pool or job token is passed on in any
case. A step that runs as the runner's user can still read the token file. On unix, `--step-user USER` runs the steps as
another user: the runner runs as root (its token file root's alone, mode 600), the checkout, its caches and the steps'
`home` beside it are handed to USER (`chown -R`) before the first step, each step runs as USER (`USER` and `LOGNAME`
are USER's), and after the last step the processes of USER that started during the job are killed. The reports,
artifacts and caches are then found and read as USER too, by a child of the same binary (`moqui ci-run collect`, within
30 minutes for all of them); `poll` checks at its start that USER can run it and refuses to start if not. **One step
user per runner process**: the kill before reading takes every process of USER that started during the job, so two
runners with one step user would end each other's steps; `poll` holds an exclusive lock on
`/run/moqui-ci-step-user-<uid>.lock` while it runs and refuses to start when another runner holds it. In any mode a report or artifact path is kept only while it resolves inside the workspace, a report that is a
link is refused, and a link under an artifact directory is stored as a link (plans/Security-Review.md SEC-GIT-81).
`poll` works in a directory of its own: `--work DIR` is made when missing, refused when it is a link, another user's,
or writable by a group or other users, and set to mode 700 (711 with `--step-user`, which the step user passes
through and cannot list); without `--work` it is a new `moqui-ci-<random>` in the temporary directory, made at the
start, so no other local user can have made it first. Each job's directory in it is `<work>/<jobRunId>` (an id, or the
job is not run), made new with the same mode, and its checkout and `home` are 700 until they are handed to the step
user; the runner's scratch files there are always made new, never through a name that is there (plans/Security-Review-Medium.md
SEC-GIT-98).
`poll` says at each claim whether it was started with `--step-user` (`stepUser` in the claim's body), and **a pool that takes untrusted runs
(`trusted` N) gives a job only to a runner that was**: one without it is given none, its claim's answer says why
(`notice`, which `poll` prints once), and the untrusted run waits, queued, for such a runner (plans/Security-Review-Low.md
SEC-GIT-19); for untrusted runs on machines whose runners cannot run steps as another user, use a pool of containers.
Without `--step-user`, keep a pool for one owner's trusted runs.

A runner token expires `git-ci` `runner-token-days` (90; 0 is never) after it was made; a runner registered before
tokens expired counts from its registration. A claim's answer says when to rotate (`rotateAfter`, halfway), and `poll`
then calls `POST /ci/runner/rotate`, which answers a new token and ends the old one at once, and writes the new token
to its token file (a new file, mode 600, renamed over the old one). A token given in the environment cannot be
rotated: `poll` says so, and the runner registers again once it expires. An expired token is a 401 that says so.
A pool's `allowedAddresses` (CIDRs, comma separated, by `store#RunnerPool`) are the addresses its runners register,
claim and rotate from; another address is a 403, noted as a failed sign-in, and an entry that is not an address or a
range is refused when the pool is stored. `GitRunner.lastAddress` is where a runner was last heard from. Revoke a
runner (`revoke#Runner`) whose machine was shared.

`poll` asks for a job every few seconds; when its pool has room it is given
the oldest queued job of the pool with a job token, runs it in a directory of its own, and asks again. A runner runs
one job at a time, and nothing but `--step-user` isolates a job from the machine: use such a pool for trusted runs,
of one owner, or for untrusted runs with runners started with `--step-user`. Each
step's process group is killed when the step ends (before the step's process is reaped, so a reused group id is
never hit), and after each job `poll` kills the processes of the runner's user that started since the job began, but
itself and what it runs under (never as root; `--keep-processes` leaves them), so what a job left running, in a group
or session of its own, does not see the next job, and a process started before it (a `tee` of the runner's output)
is left alone: run the runner as a user of its own.

A pool token registers runners for `git-ci` `pool-token-days` (7), or the `validDays` of `create#RunnerPoolToken`;
one with no expiry (made before tokens expired) registers nothing: make a new one. A new token replaces it, and with
`revokeRunners` the pool's runners are revoked too (a token that leaked). A runner is reserved for its job by one
conditional update (`GitRunner.currentJobRunId` empty), so it never gets two jobs at once; a claim that fails after
the reservation gives the runner back, and under the pool's lease a runner reserved for a job it is not running (a
claim that stopped between the two) is freed at its next claim; a pool with no `ownerId` on
a server with `run-check-service` gives its runners no job (as `store#RunnerPool` refuses it), and a runner's claim
takes the pool's lease only when a job is queued.
`revoke#Runner` revokes one runner (`GitRunner.statusId` `revoked`): its token is refused from then on, and the job it
runs ends as an error.

## Images

The files are in `podman/` ([README](../../podman/README.md)). `podman/build.sh --build-image` builds `moqui-build`
(`Containerfile.build`) beside the server's image, on the same Debian base: the Rust toolchain `rust-toolchain.toml`
pins with clippy and rustfmt, a C toolchain, git, bash, curl, the libraries the embedded Postgres loads, the `moqui`
binary (the runner), and an MR Moqui tree at `/opt/moqui` for the built-in pipeline. `--chromium` and `--java` add
Chromium and a Java runtime. Node is not needed.

The engine builds no image. A pool lists the images it allows (the job's and its services'), and the host of a
trusted pool pulls one it does not have; an untrusted pool's host must have it. A job's image
needs the runner command (`git-ci` `runner-command`, `moqui ci-run`), git and a shell; an image without the `moqui`
binary cannot run a job yet ([not built](#not-built)).

## Job tokens

When a job starts the engine makes a job token: 32 random bytes, stored only as their SHA-256 (`GitJobRun.tokenHash`),
good while that job run is `running` and until its time limit. It authorizes two things:

- **Reading the job's repository at the job's commit over git HTTP**: an upload-pack of that repository, in which one
  ref is advertised (`refs/heads/ci`, at the commit), every `want` must be that commit, and the fetch must be shallow
  with a depth of at most `git-ci` `max-fetch-depth`. A push, another repository, another commit, a deep fetch and
  plain HTTP are refused; the token of a job that ended is 410 Gone, which is no failed sign-in (a runner fetching
  after a cancel), as on the runner REST. No user is logged in. The repository is the run's: for a merge request from a fork, the
  target, never the fork. The commit must still be the repository's to give
  ([Reachability](Git-Hosting.md#reachability)): for a merge request's run, one the head ref of that merge request
  reaches (`refs/merge-requests/<n>/head`, a hidden one's too, which this token alone fetches); for any other run,
  one a ref the repository's readers read reaches. Once the ref has left the commit (a forced branch, an amended
  push for review, a released head) the fetch is refused wherever the store may hold what its readers were never to
  read: in a fork network, and in a repository that has had a hidden merge request. (In any other repository every
  commit was once a reader's, and the job still fetches its own.) A `have` is acknowledged for that commit only, so
  the token learns nothing of what else the store holds.
- **The runner endpoints of that job run.**

It is dead once the job ends, by its result, a timeout or a cancel: only the token of a `running` job within its
time is taken. Its hash stays on the row, so a runner's late call with it (after a cancel or a timeout, on whichever
cluster member) is answered **410 Gone**, which `moqui ci-run` takes as "the job is gone" (exit 3) and is no failed
sign-in for web protection; a token that is malformed or not this server's is 401. It is no user's
and reaches no service, screen or other REST. `moqui ci-run` does not pass it to the steps' environment; a step runs
as the runner's user, though, and so could read it from the runner's process, which gives it nothing the job did not
have. A registered runner's own token is another matter: see [Runners and drivers](#runners-and-drivers).

## The runner REST

At `/ci/runner/`, on the git pool as a clone is. It takes no session and no user credentials: every request has a
token in `Authorization: Bearer`, taken only over HTTPS, from loopback, or with `git-server` `allow-http`.

| Request | Token | |
| --- | --- | --- |
| `GET jobs/{jobRunId}/job` | job | The job: `spec` (steps, env, caches, artifacts, reports, services), `commitId`, `refName`, `cloneUrl`, `ciEnv`, `timeoutMinutes`, `trusted`, and for a trusted run `secrets` |
| `POST jobs/{jobRunId}/log` | job | `{stepSeq, text}`: whole lines, appended to the log |
| `POST jobs/{jobRunId}/steps/{seq}` | job | `{status: running, success, failure or skipped, exitCode}` |
| `PUT jobs/{jobRunId}/artifacts?kind=&name=&path=&key=&retentionDays=` | job | The body is the content: an `artifact`, a `report`, or a `cache` (with `key`), streamed to a content blob |
| `GET jobs/{jobRunId}/caches?key=` | job | The cache archive, or 404 |
| `POST jobs/{jobRunId}/result` | job | `{status: success, failure or error, exitCode, message}`. The job ends and the token is dead. |
| `POST register` | pool | `{name, platform}`: a new runner, `{runnerId, token}` |
| `POST claim` | runner | A queued job of the runner's pool, `{jobRunId, token}`, or `{}`; with `rotateAfter` (epoch milliseconds) when the token expires. The body says `{"stepUser": true}` for a runner started with `--step-user`; a pool that takes untrusted runs gives a job to no other, and answers it a `notice` |
| `POST rotate` | runner | A new runner token, `{token, expiresAt, rotateAfter}`; the one it was called with is dead at once |

Every job endpoint checks that the token's job run is the one in the path (403 otherwise) and still running (410
after it ended or passed its time; 401 for a token that is not a job token of this server), and counts the job's requests (`runner-max-requests-per-minute`, 429 over it). A JSON body is at most
`max-log-batch-bytes`; an upload is limited by its kind (`max-artifact-bytes`, `max-report-bytes`,
`max-cache-bytes`), and a job uploads at most `max-artifacts` artifacts, reports and caches (counted again once one is
stored, so uploads sent at once do not each pass it). What a repository, a run's owner and an untrusted run keep is
capped (`max-repository-upload-bytes`, 10 GiB; `max-owner-upload-bytes`, none; `max-untrusted-run-upload-bytes`, 1 GiB
over all its jobs; 0, no cap): an upload past it is
refused (`GitCiStorageQuota`), one that would pass it is cut there and refused the same way, and an upload is
counted again once stored, so of two at once the one that went past the cap is taken back. Artifacts stored before
they carried their run's owner are given it before an owner's cap is counted. A cache from an untrusted run is
422. A refusal is its status with `{"error": ...}`. A token that is not valid (401) counts as an authentication failure
for web protection; an ended job's (410) does not.

## REST

A run's resources for people and their tools are in the framework's [hosting REST](Git-Hosting.md#rest), under
`/rest/s1/moqui/git/{repositoryId}`, a run by its number in the repository. Each service checks the repository's rights
as without REST, and the writes ask the application that keeps the repository first (`git-server`
`rest-write-check-service`, `GitKeptByApplication`).

| Path under `/rest/s1/moqui/git/{repositoryId}` | Methods | Service |
|---|---|---|
| `pipelines` | GET, POST | `find#PipelineRuns`; `GitRestServices.run#Pipeline` (a pipeline with the manual event: `pipelineName`, `refName`) |
| `pipelines/definitions` | GET | `get#PipelineDefinitions` (`revision`) |
| `pipelines/{runNum}` | GET | `get#PipelineRun`: the run and its jobs |
| `pipelines/{runNum}/rerun`, `/cancel`, `/approve` | POST | `GitRestServices.rerun#PipelineRun`, `cancel#PipelineRun`, `approve#PipelineRun` |
| `jobs/{jobRunId}` | GET | `get#JobRun`: the steps, the failed tests with their files and lines, the artifacts |
| `jobs/{jobRunId}/log` | GET | `get#JobLog`: `afterChunkSeq` while it runs, `offset` once ended, as a page tails it |
| `jobs/{jobRunId}/retry` | POST | `GitRestServices.retry#JobRun`, of a job of this repository's runs |
| `artifacts/{artifactId}` | GET | `GitRestServices.get#RunArtifact`: a piece (`offset`, `maxBytes`) as `base64` |

This is not [the runner REST](#the-runner-rest), which a runner calls with its own token.

## moqui ci-run

`moqui ci-run` is the runner: a subcommand of the binary that opens no runtime, conf or database
(`moqui help ci-run`). With `MOQUI_CI_SERVER_URL`, `MOQUI_CI_TOKEN` and `MOQUI_CI_JOB_RUN_ID` in its environment, as
the engine starts it, it:

1. gets the job;
2. checks the commit out with the system `git` (`git init`, a shallow `fetch` of the commit with the token as a
   bearer header given in git's environment, `checkout --detach`), into `src` in the working directory;
3. restores each cache whose key is stored;
4. runs each step with `sh -c` in the checkout, with the job's variables and secrets, in a process group of its own,
   until it ends or its time limit; a step past its time is killed with what it started;
5. sends the log lines as they are written, in batches, at least once a second;
6. uploads the reports and the artifacts, also of a failed job, and for a trusted job that succeeded the caches that
   were not there;
7. posts the result.

Its exit code is 0 when the job succeeded, 1 when a step failed, 2 for an error and 3 when the server no longer takes
the job's calls (410, or 401 or 403: cancelled or past its time), on which it stops the step at once and tries no call
again; only a call that does not connect or is answered 5xx is tried again. A job whose runner exits with no
result is ended by the server ([the engine](#the-engine)).

## Deployments

A deployment puts the head of a branch on another instance, after the environment's pipeline passed for it and, when
the environment requires one, an approver approved it (Dogfood §8.6). The code is `crates/moqui-git/src/ci/deploy.rs`
and `attest.rs`; the target's side is Component-Deploy's ([the gate](../tools/Component-Deploy.md#the-gate)).

**The environment** (`store#CiEnvironment`, a maintainer's, as its secrets are) names:

| Field | |
| --- | --- |
| `targetUrl` | The target instance's base URL: `https`, or `http` to a loopback IP literal only (the instance and itself). Its address follows the webhooks' rules: a loopback, private or link-local one (written out or resolved) is refused unless `git-ci` `deploy-private-network="true"`. It is the attestation's `audience`, which the target compares with its own `public-urls`. `find#CiEnvironments` answers it to a maintainer only. |
| `token` (`targetToken`) | The secret the target issued: its `component-deploy` `attestation-secret`, at least 32 characters. Encrypted, never read back (`hasToken`), and never sent: it signs. |
| `requiredPipeline` | The pipeline that must pass for the commit. It must have the `deploy` event (`on="deploy"`, beside others if it likes). |
| `approvalRequired`, `approverGroupId` | Whether an approver approves once the pipeline passed, and who: the members of the group who view the repository, else a maintainer (the delete right). Never the requester. |
| `allowedRefs` | The branches that deploy besides protected ones (as for the environment's secrets). A deployment of a branch no branch rule protects waits for an approver whatever `approvalRequired` says: whoever pushes to it chose the commit and the pipeline file that tests it. |
| `cloneUrl` | The URL the target fetches from (https or ssh, with no user or password in it); the repository's public clone URL when empty. To a maintainer only, as `targetUrl`. The target's `allow` entry names this URL. |
| `dataTypes` | The data types the target loads of the component's data files, comma separated, signed in each attestation; empty for the target's defaults (those of Deploy and Update). |

**The steps.** `request#GitDeployment` (the update right; a repository the caller does not view is not found) takes a
branch (`refName`, the default branch when empty), protected or named by `allowedRefs`, and deploys its head
(`commitId`, when given, must be that head: Component-Deploy fetches a branch). It writes a `GitDeployment`
(`requested`, with `approvalRequired` Y when the environment requires approval or no branch rule protects the branch)
and queues `requiredPipeline` for the commit with the `deploy` event through the engine's queue, as the requester, so
the run check decides trust and approval as for any run (`waiting-pipeline`; `failed` at once when the commit has no
such pipeline, the run is refused, or pipelines are off here). A job of that pipeline in an environment that requires
approval holds the run for a maintainer first, as any such run; that approval is of the run, and does not count for
the deployment: `decide#GitDeployment` is the one approval step (as approving opens the lease in
[Vault-Instances](../../plans/Vault-Instances.md) §4.6). When the run ends:

- not `success`: the deployment is `failed` (`errorText` names the run and how it ended);
- `success`, with `approvalRequired` (the deployment's or the environment's): `waiting-approval` until
  `decide#GitDeployment` (`approve` or `refuse` with a `reason`); refused is `refused`;
- else, or approved: `deploying`, and the deploy step starts on a thread of its own once the transaction commits (a
  step left waiting is started again by `end#OverdueJobRuns`; one that began more than `deploy-timeout-seconds` and
  five minutes ago and never ended fails).

`cancel#GitDeployment` (the requester, or the update right) cancels one whose step has not begun, with its run.

**The deploy step**, claimed by one conditional update (`startedDate`), so one thread of one member calls the target:

1. reads `component.xml` at the commit. A repository with none, the framework's own, is **not deployable here**: a
   framework change ships as a new archive or image ([Run-Deploy](../Run-Deploy.md)), and the deployment fails saying
   so, nothing built;
2. checks the target URL and its token, and asks the outbound check (`git-server` `outbound-check-service`, kind
   `deploy`, the environment as its event; [Git-Hosting](Git-Hosting.md#outbound-check));
3. posts `{attestation, signature}` to `<targetUrl>/rest/s1/moqui/components/deploy` (`deploy#AttestedComponent`):
   no redirect followed, at most `deploy-timeout-seconds` (600), 1 MiB of answer read;
4. records the answer: `deployed` with `deployResult` (`status` deployed or up-to-date, `component`, `commitId`,
   `problems`), the environment's `lastCommitId`, `lastDeploymentId` and `lastDeployDate`; or `failed` with the
   target's errors in `errorText`, which `get#` and `find#GitDeployment(s)` answer to a maintainer (the delete right)
   only.

**The attestation** (version 2) is the JSON text of `{kind: "moqui-deploy-attestation", v: 2, audience, repositoryUrl,
refName, commitId, component, pipeline, pipelineRunId, runNum, runStatus: "success", environment, deploymentId,
issuedAt (milliseconds), source, dataTypes}`, where `audience` is the environment's `targetUrl` (scheme and host in
lower case, no default port or trailing slash) and `dataTypes` its data types, and the signature is `sha256=` and the
hex HMAC-SHA256, keyed with the token, of `moqui-deploy-attestation-v2\n` followed by that exact text. Version 1 is
not taken. The target verifies the signature over the text it was sent, then that it is no older than its
`max-age-seconds` (and not over a minute ahead of its clock), that `audience` is one of its own public URLs, and that
an `allow` entry names the component, URL and branch, then claims it (none replayed) and deploys that component at
that commit only, while the branch it fetches is still there ([the gate](../tools/Component-Deploy.md#the-gate)).

**What a passed run attests.** The pipeline that ran is read from `ci.xml` at the deployed commit, as for any run: a
person who may push to the branch chooses both the change and the tests that pass it. A protected branch takes a
change only by its rule (a merge request with its approvals, or its push group); a branch only `allowedRefs` names has
no such rule, which is why its deployment always waits for an approver.

**Events.** Each move writes the commit status `deploy/<environment>`: `pending` (requested, waiting for its pipeline
or approval, deploying), then `success` (deployed), `failure` (failed) or `error` (refused, cancelled), with the run's
page as its target. Whoever views the repository reads a status, so an ended one says only how it ended (`Deployed`,
`Deploy failed`, `Deploy refused`, `Deploy cancelled`); why is in `errorText`. Keep users from posting it as for
`ci/`: `git-server` `reserved-status-contexts="ci/,deploy/"`.
The requester is told each end, and the approver group when one is awaited or decided, on the notification topic
`GitDeployment` (sub-topic the repository; `{deploymentId, repositoryId, environment, commitId, statusId}`).

On moqui.org: a demo instance's environment for the protected main with no approval, which a rule or a person asks
for after each green merge; moqui.org's own with `approvalRequired` and its target set to `require-passed-run`, with an
`allow` entry for each component it takes.

## Records

In `moqui.resource` (`framework/entity/GitCiEntities.xml`). States are plain codes.

| Entity | |
| --- | --- |
| `GitPipelineRun` | A run: `repositoryId`, `runNum` (per repository), `pipelineName`, `definitionSource` (file, builtin), `event`, `refName`, `commitId`, `mergeRequestId`, `sourceRepositoryId`, `triggeredByUserId`, `ownerId`, `trusted`, `protectedRef`, `hidden`, `approvalState` (none, waiting, approved), `approvedByUserId`, `statusId` (waiting, queued, running, success, failure, error, cancelled), `statusMessage`, `environmentId`, `retryOfRunId`, dates |
| `GitJobRun` | A job of a run: `jobName`, `matrixKey`, `attempt`, `superseded`, `needs`, `poolName`, `runnerPoolId`, `runnerId`, `image`, `statusId` (waiting, queued, running, success, failure, error, cancelled, skipped), `statusMessage`, `exitCode`, `timeoutMinutes`, `jobSpec` (JSON), `instanceHostId`, `instanceRef`, `tokenHash`, `tokenExpireDate`, `logBlobId`, `logBytes`, `nextChunkSeq`, `minutesUsed`, `testCount`, `testFailedCount`, `commitStatusId`, dates |
| `GitJobStep` | A step: `stepSeq`, `stepName`, `command`, `statusId` (pending, running, success, failure, skipped), `exitCode`, `firstChunkSeq`, dates |
| `GitRunLogChunk` | Log lines of a running job: `chunkSeq`, `stepSeq`, `byteOffset`, `chunkText`. Removed when the job ends and the log becomes one content blob. |
| `GitRunArtifact` | An upload: `artifactKind` (artifact, report, cache), `name`, `path`, `blobId`, `sizeBytes`, `sha256`, `cacheKey`, `protectedRef`, `keep`, `expireDate`, `lastUsedDate` |
| `GitRunTestFailure` | A failed test: `suiteName`, `testName`, `filePath`, `lineNumber`, `message`, `durationMillis` |
| `GitCiSecret` | `repositoryId` or `ownerId`, `name`, `secretValue` (encrypted), `scope`, `environmentId`, `lastUsedDate` |
| `GitCiEnvironment` | `repositoryId` or `ownerId`, `name`, `allowedRefs`, and for deployments `targetUrl`, `targetToken` (encrypted), `cloneUrl`, `requiredPipeline`, `approvalRequired`, `approverGroupId`, `dataTypes`, `lastCommitId`, `lastDeploymentId`, `lastDeployDate` |
| `GitRunnerPool`, `GitRunner` | Pools and their registered runners |
| `GitRunnerUsage` | The time of one job run, written when it ends: `repositoryId`, `ownerId`, `runnerPoolId`, `trusted`, `periodKey` (yyyy-MM), `secondsUsed`, `minutesUsed` (started minutes) |
| `GitPipelineSchedule` | The schedule pipelines of a repository's default branch |
| `GitDeployment` | A deployment ([deployments](#deployments)): `repositoryId`, `environmentId`, `environmentName`, `commitId`, `refName`, `componentName`, `pipelineRunId`, `statusId` (requested, waiting-pipeline, waiting-approval, deploying, deployed, failed, refused, cancelled), `requestedByUserId`, `approvalRequired`, `approvedByUserId`, `approvedDate`, `refusedByUserId`, `deployResult`, `errorText` (a maintainer's only; never a secret), `createdDate`, `startedDate`, `finishedDate` |

Logs and uploads are content blobs ([Git-Hosting](Git-Hosting.md)): the rows refer to them, and `gc#ContentBlobs`
deletes a blob once no row does. `delete#GitRepository` removes a repository's rows of these entities with it, in
its transaction (and once more after it commits, for what a job left meanwhile): the jobs still running are
stopped, their containers removed and their runners freed, and their tokens go with their rows. No status is
written for them, the repository's go too. A fork's runs are its own and go with the fork; the runs of a merge
request from it are its target's and stay. `delete#RepositoryRuns` does the same for an application that clears a
repository's pipelines and keeps the repository.

## Services

`org.moqui.git.GitCiServices` (`crates/moqui-git/src/ci/service.rs`). Reading a run follows viewing its repository. A
run of a hidden merge request's head (`hidden` Y) is read only with authorization disabled and `includeHidden`, or by
the user it was made for (the author who pushed it): its log and uploads are that head's files. So are cancel,
retry, re-run and approve: a user with the update right does not see a hidden run to approve it, the application
does. A merge request's runs are hidden and shown with it: `set#MergeRequestHidden` sets `hidden` on each of them,
and a re-run takes it from the merge request as it is then. Showing one queues no run. Its jobs run as any untrusted
run's do once approved, and their token is the only thing that fetches the hidden head
([job tokens](#job-tokens)).

| Service | Needs | |
| --- | --- | --- |
| `find#PipelineRuns` | view | By `repositoryId`, with `refName`, `event`, `statusId`, `mergeRequestId`, `commitId`, `pipelineName`; paged. `runList`, `runCount` |
| `get#PipelineRun` | view | By `pipelineRunId`, or `repositoryId` and `runNum`. `run`, `jobList` in the order `needs` gives, `canUpdate` |
| `get#JobRun` | view | `job`, `stepList`, `failedTestList`, `artifactList` (artifacts and reports) |
| `get#JobLog` | view | See below |
| `get#RunArtifact` | view | An artifact or report and a piece of its content (`offset`, `maxBytes`; `bytes`, `nextOffset`, `done`). A cache is not found. Each artifact map carries its `location`, below. |
| `send#RunArtifact` | view | Send an artifact or report as the request's response, streamed from its content blob with `Range`, `ETag` and `If-Range` (`inline` shows a passive file: an image, a PDF, plain text, JSON): call it from a screen's download transition, and nothing is copied to a file. A job's HTML, SVG, XML or JavaScript is still an attachment unless `inlineActive` is set too; set that only for a run whose commits the application trusts, since the file is shown on the application's host. Shown inline, a file is sandboxed (`Content-Security-Policy: sandbox`, no script, an origin of its own) and never sniffed (`nosniff`). A cache is not found. |
| `set#RunArtifactKeep` | update, or authorization disabled | Keep an artifact or report past its expiry (`keep`: a release's, which the daily retention then leaves), or let it expire again; it still counts toward the storage caps. A cache is not found. |
| `get#PipelineDefinitions` | view | The pipelines a commit (`revision`: branch, tag, ref or commit; the default branch when empty) defines, as the engine reads them: `source` (`file`, `builtin`, `none`, `invalid` with `errorMessage`) and `pipelineList` (`pipelineName`, `events`, `manual`, `branches`, `tags`, `cron`, `jobNames`), so an application need not parse `ci.xml`. |
| `run#Pipeline` | update | A pipeline with the manual event, for a branch or tag |
| `rerun#PipelineRun` | update | A new run for the same commit and event; one that was untrusted for where its commits came from stays so |
| `cancel#PipelineRun` | update, or the user who asked for the run | A run that has ended is an error |
| `retry#JobRun` | update | A new attempt of an ended job in its run, in the pool the old one ran in; the old one is `superseded`, the jobs skipped for it wait again. Not of a refused or never approved run, nor one the run check would now refuse, hold or trust less |
| `approve#PipelineRun` | update, or authorization disabled; delete for a run with a job in an `approvalRequired` environment, never by who triggered it | A run that does not wait is an error |
| `find#CiSecrets`, `store#CiSecret`, `delete#CiSecret` | delete (a maintainer) for a repository's; authorization disabled for an owner's | Values are written and never read; a move to another scope or environment needs a new value |
| `find#CiEnvironments` | view (a repository's) | Without the target's token (`hasToken`) |
| `store#CiEnvironment`, `delete#CiEnvironment` | as secrets | `allowedRefs`, `approvalRequired`, and for deployments `targetUrl`, `token`, `cloneUrl`, `requiredPipeline`, `approverGroupId`; delete is refused while a secret is scoped to it |
| `request#GitDeployment` | update | `repositoryId`, `environment` (or `environmentId`), `refName`, `commitId` → `deploymentId`, `statusId`, `pipelineRunId`, `message` |
| `decide#GitDeployment` | an approver (the approver group, else delete), never the requester; or authorization disabled | `deploymentId`, `decision` (approve, refuse), `reason` → `statusId` |
| `cancel#GitDeployment` | update, or the requester | `deploymentId` → `statusId` |
| `find#GitDeployments`, `get#GitDeployment` | view | `repositoryId`, `environment`, `statusId`, `commitId`, paged → `deploymentList`, `deploymentCount`; `deploymentId` → `deployment`, `canDecide`, `canCancel` |
| `find#RunnerPools`, `store#RunnerPool`, `delete#RunnerPool`, `create#RunnerPoolToken`, `revoke#Runner` | `GIT_CI_ADMIN`, or authorization disabled | A token's `validDays` and `revokeRunners` |
| `get#RunnerUsage` | view, or authorization disabled for an owner's | A month's `minutesUsed`, `secondsUsed`, `untrustedMinutesUsed`, `jobCount`, and the `artifactBytes` and `cacheBytes` stored now, for an application's quota |
| `delete#RepositoryRuns` | delete, or authorization disabled | Everything the engine keeps of a repository, as `delete#GitRepository` removes it |

**A job's log.** While the job runs `get#JobLog` gives the chunks after `afterChunkSeq` (`chunkList`,
`nextChunkSeq`); once it has ended (`finished`) it gives the stored log from `offset` as `text` (`nextOffset`). A
page that tails passes back both `nextChunkSeq` and `nextOffset` each time, so it loses no line when the job ends
between two reads. With `watch` the caller is sent a notification on the topic `GitCiJobLog` (sub-topic the job
run's id; `{jobRunId, nextChunkSeq, finished}`) when the log grows, at most once a second, for three minutes after
each read, so a page reads when it is told and need not poll every second. The watchers are kept in the memory of
the server that took the read: on a cluster a page still polls, less often.

**An artifact's location.** Each artifact and report is also the read-only resource location
`ci-artifact://<artifactId>/<fileName>` (the `location` of an artifact map; the id alone finds it, the name gives the
type and the name it is sent under), read with `get#RunArtifact`'s check: the view right on the repository, a hidden
run's only with authorization disabled or as the user it was made for; a cache, one that is not there and one the
caller does not read are not found. So every transport that sends a file sends one, streamed from its content blob:
`ec.web.sendResourceResponse(location, inline)` in a screen (a user's file, so never sniffed, an attachment when its
type can run script, sandboxed when shown inline), and a PASS authority download over gRPC, whose authority service
answers the location (`download.location`) once it has checked its caller; the gRPC frontend opens a `ci-artifact://`
answer with authorization off (the service vouches for it, a hidden run's file too) and any other answer with the
call's own authorization, and sends it with the same headers (nosniff; an active type an attachment; inline sandboxed).

The engine's own services are the rules' and the jobs': `queue#PushRuns` and `queue#MergeRequestRuns` (with
authorization disabled only), `queue#ScheduledRuns`, `start#QueuedJobRuns`, `end#OverdueJobRuns` (which also starts a
deploy step left waiting), `expire#RunData`.

## The engine

- **Queueing** reads the file at the event's commit (as the system, a hidden merge request's head too), asks the
  application's check, and writes the run, its jobs (each with its `jobSpec`, the planned job as JSON) and their
  pending statuses.
- **Starting.** `start_QueuedJobRuns_frequent` (and a pass soon after a run is queued) starts queued jobs oldest
  first, up to each pool's `maxConcurrent`: it picks the pool (the one the check named, else an enabled pool of the
  job's pool name that takes the run's trust, the owner's before a shared one), checks the image against
  `allowedImages`, makes the job token and calls the host type's start service. A job with no pool, an image that is
  not allowed, a runner URL that would be plain HTTP ([set it up](#set-it-up)), or a host that does not start it is
  an `error`.
- **Claims are atomic across cluster members.** A job moves from one state to the next by one conditional `UPDATE`
  (`... WHERE jobRunId = ? AND statusId = ?`) whose row count says whether it applied, so of two members (or two
  threads) that start, claim, end or skip the same job only one does; the `statusId` audit log is written as an
  entity update writes it. A start's or a registered runner's claim is a statement of its own, committed at once,
  and is made under the pool's lease (`moqui.resource.GitLease` `ci-pool:<runnerPoolId>`), held while the pool's
  running jobs are counted against `maxConcurrent` and one is claimed: a member that counts next counts it, so a pool
  never goes over its limit. A start whose transaction rolls back after its claim gives the job back to the queue.
- **Ending.** The runner's result ends a job: its steps left are closed, its log becomes one blob, its usage row is
  written, its status is written, its token dies and its container is removed. Then the run moves on: waiting
  jobs whose needs all succeeded are queued, those whose need did not are skipped, and when every job has ended the
  run is `success`, else `error`, `failure` or `cancelled`.
- **Overdue and lost.** `end_OverdueJobRuns_frequent` ends as errors the running jobs past their time limit, the ones
  whose runner has not called for `lost-seconds` and whose container the host no longer has or has as exited, and
  the queued jobs no runner took in `queue-timeout-minutes`. It then removes from each host the containers labelled
  as this server's whose job run is not running.
- **Retention.** `expire_PipelineRunData_daily` removes artifacts, reports and caches past their expiry that are not
  kept (`keep`), the logs of jobs that ended `log-retention-days` ago, and log chunks an ended job left.
- **Schedules.** `queue_ScheduledPipelineRuns_frequent`.

Each job tolerates one failing piece: a job that cannot be started, checked or expired is logged and the rest go on.

## Behind a front or edge node, and in a pool

The runner REST (`/ci/runner/`) and the job's git fetch go through a front or edge node as any request does, in
every topology ([Git-Hosting](Git-Hosting.md#behind-a-front-or-edge-node-and-in-a-pool) has what is forwarded and how
the client's address arrives).

- **Streamed.** Artifact, cache and report uploads and the result post stream down the link to the local node, and
  a cache download streams back; the edge node waits for the answer's head as long as the body keeps moving (`<edge
  forward-idle-seconds>`) and then up to `forward-head-seconds` once it is all sent, so an upload answered once it is
  stored is not timed out. The edge node's `request-timeout` does not apply to a request it forwards; the local
  node's does, and `/ci/runner/` is exempt from it.
- **`server-url`.** The URL the runners reach the server by: the public URL on the edge node (empty uses it), or
  another name only when runners reach the local node some other way (a private address on the same network).
  Statuses and the `run-url` link name the public URL.
- **Refusing hosts.** A host whose root screen refuses the framework's routes still serves `/ci/runner/`, so a
  site's own host can be the runners' URL.
- **Web protection.** Runners share addresses (a NAT, a pool of machines), so a request to `/ci/runner/`, or a job's
  fetch under `/git/`, whose job, runner or pool token (`mqcj_`, `mqcr_`, `mqcp_` and 64 hex digits, as `Bearer` or
  git's Basic password) was found good a short time ago is counted by its token: by the engine's own limit per job and
  per runner (`runner-max-requests-per-minute`), and by no per-client rate limit, for 10 minutes after its last good
  request on that host. The answer that finds a token good carries a marker that the node serving it records, and an
  edge node in front of it records too as the answer passes (and removes), so this holds on the edge node and on the
  local node alike. Until then, and for a token that is made up, the request is counted by its address as any other;
  a well-formed token that is not known counts toward the `ci-token-failure` ban (100 in 10 minutes), not
  `auth-failure`'s, and a token of a job that ended (410) toward neither. The path is taken as the router routes it: a
  `%` escape or a `.` or `..` segment makes it no runner request. An address already banned is refused at connect,
  whatever it sends.
- **In a pool.** Any member answers any runner request: jobs, logs, artifacts and caches are rows and content in the
  shared database and content store. A job fetches its commit over git HTTP with its job token, from any member.

## Limits and conf

`<git-ci>` in the conf (`moqui-conf-3.xsd`; the defaults are in `MoquiDefaultConf.xml`).

| Attribute | Default | |
| --- | --- | --- |
| `enabled` | empty | `true`, `false`, or empty: on while there is an enabled pool |
| `server-url`, `server-id` | the public URL, its host | How runners reach the server; the label of this server's containers |
| `run-check-service`, `run-url-service`, `run-url` | none | The application's hooks |
| `builtin-enabled`, `builtin-image`, `builtin-pool`, `builtin-workers` | true, `moqui-build`, default, 2 | The built-in pipeline |
| `max-file-bytes`, `max-pipelines`, `max-jobs`, `max-matrix`, `max-steps` | 256 KB, 20, 50, 20, 50 | What a `ci.xml` may hold |
| `max-timeout-minutes`, `default-timeout-minutes`, `max-fetch-depth` | 360, 60, 50 | |
| `max-log-bytes`, `max-log-batch-bytes` | 16 MB, 1 MB | A job's log; one runner request |
| `max-artifact-bytes`, `max-report-bytes`, `max-cache-bytes`, `max-artifacts` | 1 GB, 16 MB, 2 GB, 100 | Uploads (`max-artifacts` counts caches too) |
| `max-unpack-bytes` | 16 GiB | What a cache unpacks to on a runner, sent with each job |
| `max-repository-upload-bytes`, `max-owner-upload-bytes`, `max-untrusted-run-upload-bytes` | 10737418240, 0, 1073741824 | What a repository, an owner and an untrusted run keep of uploads; 0: no cap |
| `max-services`, `schedule-min-minutes`, `pool-token-days` | 4, 15, 7 | Services of a job; the least time between a schedule's runs; how long a pool token registers |
| `runner-token-days` | 90 | How long a registered runner's token is good for; rotated halfway; 0 is no expiry |
| `artifact-retention-days`, `max-artifact-retention-days`, `log-retention-days`, `cache-retention-days` | 30, 365, 90, 30 | |
| `lost-seconds`, `queue-timeout-minutes` | 180, 1440 | |
| `runner-max-requests-per-minute`, `runner-command`, `start-inline` | 1200, `moqui ci-run`, true | |
| `runner-max-registrations-per-minute`, `max-runners-per-pool` | 10, 50 | Registrations of one pool in a minute (429 past it); active runners a pool holds (a registration past it is 409 until one is revoked), so a leaked pool token mints only so many runner tokens. The minute count is each cluster member's own (n members allow n times it); the runner cap is counted under the pool's lease across the cluster, 0 no cap |
| `container-user`, `container-read-only-root`, `container-workdir`, `container-tmp-mb`, `container-workspace-mb` | 1000:1000, true, `/workspace`, 1024, 4096 | A job's container and its services |
| `deploy-timeout-seconds`, `deploy-private-network` | 600, false | How long a deployment's target is given; whether a target may be at a loopback, private or link-local address |

## Tests

The engine is tested against the fake host type with a real `git` push and the runner in the test's process
(`crates/moqui-git/src/server/ci_tests.rs`, with the framework's gitECA rules and the service ECA rule of
`GitCi.secas.xml` loaded: a merge request of a branch, one from a real fork and real pushes to `refs/for/main`, a
hidden one on a server with `review-start-hidden`, a merge, and the delete of a repository), the file's reader, planning, JUnit and the runner's pieces beside their
code, and the container driver's requests against a small Docker API in the test, over TCP and a Unix socket
(`ci/driver/docker_tests.rs`). Deployments are tested the same way (`server/ci_deploy_tests.rs`), with the deploy step's
call over loopback to a target that verifies the attestation with `ci::attest::verify`, the code of the real target's
`deploy#AttestedComponent`, whose gate is tested in moqui-devkit (`deploy/git_tests.rs`, `gate_tests.rs` and `services_tests.rs`, `--features git`: the allow list, the audience, the claim under concurrency, an attested update of a `db` repository, a start and another member under the gate, and the service called with no user, as its REST resource calls it). One job on
a real Docker or Podman socket is opt-in:

```sh
MOQUI_CI_DOCKER_SOCKET=/var/run/docker.sock cargo test -p moqui-git --test ci_docker -- --nocapture
```

It needs an image with a shell (`MOQUI_CI_DOCKER_IMAGE`, `docker.io/library/busybox:latest` by default), starts one
container through the driver's services, inspects it, and removes it.

## Not built

- **Deployments**: an owner's environment (a repository's only); a private repository's fetch by the target (it
  fetches with no credential, so the clone URL must be one the target reads); a tag, or a commit that is not its
  branch's head; the REST of the hosting API for deployments (the services are there); building a framework archive
  or image, which a framework deployment only reports as not deployable here. Deploys to managed instances
  ([Vault-Instances](../../plans/Vault-Instances.md) §4.6) are to name an `InstanceEnvironment` from the environment;
  `targetUrl` and `targetToken` stay for a target not managed here, the token read in one place
  (`ci::deploy::target_token`) so it can become a vault item, and `decide#GitDeployment` stays the one approval step.
- **A small static runner** for an image without the `moqui` binary.
- **Windows registered runners**: steps run with `sh -c`, and a step past its time is killed by process group on
  Unix only.
- **Egress rules** are the host's: the engine names a network (`egressRule`) and makes none.
- **Log tailing across a cluster** by notification (see `get#JobLog`). The secrets a member masks in a job's log are
  kept in its memory for five minutes at most, so a job that ended on another member leaves nothing behind.
- Kubernetes, AWS and other providers' APIs as drivers (Dogfood D19).
