# Pipelines

MR Moqui runs pipelines for the git repositories it hosts ([Git-Hosting](Git-Hosting.md)): a file in the repository
says what to run, a push or a merge request queues a run, each job runs in a container on another machine (or on a
registered runner), and each job's result is a commit status, which the branch rules already gate merges on. This
guide is for whoever sets it up and for an application that shows runs. No Java counterpart. The plan is
[Dogfood](../../plans/Dogfood.md) §8; the code is `crates/moqui-git/src/ci`.

Contents: [the file](#the-file) · [events](#events) · [jobs](#jobs) · [the built-in pipeline](#the-built-in-pipeline) ·
[statuses](#statuses) · [trust and approval](#trust-and-approval) · [secrets](#secrets) ·
[runners and drivers](#runners-and-drivers) · [images](#images) · [job tokens](#job-tokens) ·
[the runner REST](#the-runner-rest) · [moqui ci-run](#moqui-ci-run) · [records](#records) · [services](#services) ·
[the engine](#the-engine) · [limits and conf](#limits-and-conf) · [tests](#tests) · [not built](#not-built)

## Set it up

1. A machine with Docker or Podman that is **not the server**, reachable from it, and from which the server's URL is
   reachable ([runners and drivers](#runners-and-drivers)).
2. An `InstanceHost` for it and at least one `GitRunnerPool` (System data, or `store#RunnerPool`), with the images it
   allows. The engine queues nothing while there is no enabled pool, so a server that runs no CI writes no statuses.
3. The `moqui-build` image on that machine ([images](#images)).
4. `git-ci` `server-url` when runners reach the server by another URL than the public one, and `run-url` so statuses
   link to a page.
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
| `deploy` | A deployment to an environment ([not built](#not-built); the event and the records are there) | |

Pushes and tags are the framework's gitECA rule `FrameworkQueuePipelineRuns` (`Git.gitecas.xml`, `post-receive`, the
sources `push` and `write`; not a mirror's fetch or an import, which nobody with the update right pushed). Merge
requests are a service ECA rule on `queue#MergeRequestEvent` (`GitCi.secas.xml`), which every change of a merge
request goes through, a fork's, a pushed-for-review and a hidden one's included. Schedules are
`GitPipelineSchedule` rows, written whenever the default branch moves, so the schedule job reads no file.

An event delivered twice makes one run: a run of the same pipeline, commit, event, ref and merge request that has not
ended is not queued again.

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
  checkout. It is restored before the first step, and saved after the last when the job succeeded and nothing is
  stored under the key. Caches are per repository and apart for protected refs: a run for a protected ref reads only
  caches such a run wrote; any other run reads both.
- **`artifact`**: files uploaded when the steps have run, also of a failed job, as one tar.gz download with a size, a
  checksum and an expiry (`retention-days`).
- **`report`**: JUnit XML files. Each is a download, and each failed test becomes a row with its suite, name, file,
  line and message (`moqui test --junit` writes such files; the file and line come from the test case's attributes,
  else from the first `path:line` of the failure's text).
- **`service`**: a container beside the job (a database), reached by its name as a host name, removed with the job.
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
status's target is the run's page: what `git-ci` `run-url-service` answers
(`org.moqui.git.GitCiServices.get#PipelineRunUrl`), else the `run-url` template with `{repositoryId}`,
`{repositoryPath}`, `{pipelineRunId}`, `{runNum}` and `{jobRunId}`.

The engine writes these rows itself, as the system: `create#GitCommitStatus` is a user's, and an application may keep
the `ci/` contexts from users.

## Trust and approval

A run is **trusted** only when it is for a ref of the repository itself, pushed or asked for by an account with the
update right. A schedule's run is trusted: it is the repository's own branch by its own file. A run is **untrusted**
when it is for a fork's merge request or a pushed-for-review one, or was caused by an account without the update
right. An untrusted run:

- gets **no secrets**, whatever its jobs ask for;
- **reads caches and writes none**;
- runs only in a **pool that takes untrusted runs** (`GitRunnerPool.trusted` N), and a trusted run never runs there;
- **waits for approval** (`statusId` and `approvalState` `waiting`, statuses pending "Waiting for approval") until
  `approve#PipelineRun` is called by a user with the update right, or with authorization disabled. Approved, it is
  still untrusted.

Before any run is queued the engine asks the service `git-ci` `run-check-service` names, with authorization
disabled, as the LFS upload check is asked (`org.moqui.git.GitCiServices.check#PipelineRun` is its interface). It
answers `decision`: `allow` (an untrusted run may start with no approval), `wait` (hold a trusted run too, for a
quota) or `refuse` (an `error` run whose statuses carry the `message`; an error the service leaves refuses too). It
may lower `trusted` and never raise it, and may name the `ownerId` the run's secrets and usage count under and the
`runnerPoolId` its jobs use. With no service, or no decision, a trusted run is allowed and an untrusted one waits.

The engine never runs a step on the server.

## Secrets

A `GitCiSecret` is a repository's, or an owner's that the application names (a group, an account; the run's
`ownerId`). Its value is encrypted, written through `store#CiSecret` and never read back by a service. A job is given
a secret it asks for only when its run is trusted and the secret's scope is met:

| Scope | Given when |
| --- | --- |
| `protected` | The run's ref is named by a branch rule (a protected branch or tag). Never for a merge request's run. |
| `tags` | The run's ref is a protected tag. |
| `environment` | The job's `environment` is the secret's environment. |

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
| `trusted` | Y: trusted runs only. N: untrusted runs only. |
| `allowedImages` | Image patterns (`*`). A job whose image none matches is an error; an empty list allows none. |
| `maxConcurrent`, `maxMinutes` | Jobs at once, and the longest a job runs. |
| `cpuLimit`, `memoryMb`, `diskMb`, `pidsLimit` | The container's limits. `diskMb` needs a storage driver that takes it. |
| `egressRule` | The container's network mode: `bridge` (the default), `none`, or a network of the host whose rules limit what a job reaches. |
| `serverUrl` | The URL the pool's runners reach the server by, when it is not `git-ci` `server-url`. |

**Drivers.** An `InstanceHostType` names the services that start a run that ends, inspect it, remove it and clean up
(`runStartService`, `runInspectService`, `runRemoveService`, `runCleanupService`; the interfaces are
`start#InstanceRun`, `inspect#InstanceRun`, `remove#InstanceRun` and `cleanup#InstanceRuns` of
`org.moqui.dev.InstanceServices`). So a driver is a set of services and the engine does not change.

**The `docker` host type** speaks the Docker Engine API, which Podman serves on its socket too. An `InstanceHost`:

| `hostProtocol` | `hostAddress` | |
| --- | --- | --- |
| `unix` | `/run/podman/podman.sock`, `/var/run/docker.sock`, `/run/user/1000/podman/podman.sock` | A socket on the server's machine: for a try on one machine only, since a runner is not to be on the server's |
| `https` | a host name, with `adminPort` (2376) | TLS with `tlsCaCert`, `tlsClientCert` and `tlsClientKey` (PEM; the key encrypted) |
| `http` | a host name, with `adminPort` (2375) | No authentication: loopback or a private link only |

Docker: `dockerd --tlsverify --tlscacert=ca.pem --tlscert=server-cert.pem --tlskey=server-key.pem -H 0.0.0.0:2376`.
Podman: `podman system service --time=0 tcp://0.0.0.0:2376` behind a TLS proxy, or the socket forwarded over SSH;
rootless Podman is the better host for untrusted runs. The server's certificate must name the host as the server
reaches it.

A job's container is created with: no privilege, every capability dropped, `no-new-privileges`, no host path or
socket mounted, no port published, the user `git-ci` `container-user` (1000:1000; never root), a read-only root with
an anonymous volume at `container-workdir` (`/workspace`) and a tmpfs at `/tmp`, the pool's limits and network mode,
the name `moqui-ci-<jobRunId>` and the labels `moqui.ci.job`, `moqui.ci.run` and `moqui.ci.server`. Its environment is
the server's URL, the job token and the job run's id; the job's own variables and its secrets are asked for by the
runner with the token, and are never in the container's definition. An image the host does not have is pulled.
Service containers are started first on a network of the job's own. The image must have a directory at the working
directory that the user can write (the volume takes its owner), as `moqui-build` has.

**The `fake` host type** starts nothing, records what it was asked (`FakeHosts` in
`crates/moqui-git/src/ci/driver/fake.rs`) and answers as the host's `hostAddress` says: `success` and `hang` (a
container that runs until it is removed), `fail` (it does not start), `lost` (the host does not know it) and `exit`
(it exited with no result). It is for the engine's tests.

**Registered runners** are for what a container cannot be (macOS, Windows): a long-lived machine that asks for jobs.
`create#RunnerPoolToken` gives a pool of the `registered` kind a registration token (shown once, stored hashed). On
the machine:

```sh
MOQUI_CI_SERVER_URL=https://moqui.example.com MOQUI_CI_POOL_TOKEN=mqcp_... moqui ci-run register --name mac-mini-1
MOQUI_CI_SERVER_URL=https://moqui.example.com MOQUI_CI_RUNNER_TOKEN=mqcr_... moqui ci-run poll
```

`register` prints the runner token, once. `poll` asks for a job every few seconds; when its pool has room it is given
the oldest queued job of the pool with a job token, runs it in a directory of its own, and asks again. A runner runs
one job at a time, and nothing isolates a job from the machine: use such a pool for trusted runs.

## Images

The files are in `podman/` ([README](../../podman/README.md)). `podman/build.sh --build-image` builds `moqui-build`
(`Containerfile.build`) beside the server's image, on the same Debian base: the Rust toolchain `rust-toolchain.toml`
pins with clippy and rustfmt, a C toolchain, git, bash, curl, the libraries the embedded Postgres loads, the `moqui`
binary (the runner), and an MR Moqui tree at `/opt/moqui` for the built-in pipeline. `--chromium` and `--java` add
Chromium and a Java runtime. Node is not needed.

The engine builds no image. A pool lists the images it allows, and the host pulls one it does not have. A job's image
needs the runner command (`git-ci` `runner-command`, `moqui ci-run`), git and a shell; an image without the `moqui`
binary cannot run a job yet ([not built](#not-built)).

## Job tokens

When a job starts the engine makes a job token: 32 random bytes, stored only as their SHA-256 (`GitJobRun.tokenHash`),
good while that job run is `running` and until its time limit. It authorizes two things:

- **Reading the job's repository at the job's commit over git HTTP**: an upload-pack of that repository, in which one
  ref is advertised (`refs/heads/ci`, at the commit), every `want` must be that commit, and the fetch must be shallow
  with a depth of at most `git-ci` `max-fetch-depth`. A push, another repository, another commit, a deep fetch and
  plain HTTP are refused. No user is logged in.
- **The runner endpoints of that job run.**

It is cleared when the job ends, by its result, a timeout or a cancel, so it is dead from then on. It is no user's
and reaches no service, screen or other REST. `moqui ci-run` does not pass it to the steps' environment; a step runs
as the runner's user, though, and so could read it from the runner's process, which gives it nothing the job did not
have.

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
| `POST claim` | runner | A queued job of the runner's pool, `{jobRunId, token}`, or `{}` |

Every job endpoint checks that the token's job run is the one in the path (403 otherwise) and still running (401
after), and counts the job's requests (`runner-max-requests-per-minute`, 429 over it). A JSON body is at most
`max-log-batch-bytes`; an upload is limited by its kind (`max-artifact-bytes`, `max-report-bytes`,
`max-cache-bytes`), and a job uploads at most `max-artifacts` artifacts and reports. A cache from an untrusted run is
422. A refusal is its status with `{"error": ...}`. A token that is not valid counts as an authentication failure for
web protection.

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
the job's calls (cancelled or past its time), on which it stops the step at once. A job whose runner exits with no
result is ended by the server ([the engine](#the-engine)).

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
| `GitCiEnvironment` | `repositoryId` or `ownerId`, `name`, and for deployments `targetUrl`, `targetToken` (encrypted), `requiredPipeline`, `approvalRequired`, `lastCommitId`, `lastDeployDate` |
| `GitRunnerPool`, `GitRunner` | Pools and their registered runners |
| `GitRunnerUsage` | The time of one job run, written when it ends: `repositoryId`, `ownerId`, `runnerPoolId`, `trusted`, `periodKey` (yyyy-MM), `secondsUsed`, `minutesUsed` (started minutes) |
| `GitPipelineSchedule` | The schedule pipelines of a repository's default branch |
| `GitDeployment` | For deployments ([not built](#not-built)) |

Logs and uploads are content blobs ([Git-Hosting](Git-Hosting.md)): the rows refer to them, and `gc#ContentBlobs`
deletes a blob once no row does. `delete#GitRepository` does not know these rows; an application that deletes a
repository calls `delete#RepositoryRuns` first.

## Services

`org.moqui.git.GitCiServices` (`crates/moqui-git/src/ci/service.rs`). Reading a run follows viewing its repository. A
run of a hidden merge request's head (`hidden` Y) is read only with authorization disabled and `includeHidden`, or by
the user it was made for.

| Service | Needs | |
| --- | --- | --- |
| `find#PipelineRuns` | view | By `repositoryId`, with `refName`, `event`, `statusId`, `mergeRequestId`, `commitId`, `pipelineName`; paged. `runList`, `runCount` |
| `get#PipelineRun` | view | By `pipelineRunId`, or `repositoryId` and `runNum`. `run`, `jobList` in the order `needs` gives, `canUpdate` |
| `get#JobRun` | view | `job`, `stepList`, `failedTestList`, `artifactList` (artifacts and reports) |
| `get#JobLog` | view | See below |
| `get#RunArtifact` | view | An artifact or report and a piece of its content (`offset`, `maxBytes`; `bytes`, `nextOffset`, `done`). A cache is not found. |
| `run#Pipeline` | update | A pipeline with the manual event, for a branch or tag |
| `rerun#PipelineRun` | update | A new run for the same commit and event; one that was untrusted for where its commits came from stays so |
| `cancel#PipelineRun` | update, or the user who asked for the run | A run that has ended is an error |
| `retry#JobRun` | update | A new attempt of an ended job in its run; the old one is `superseded`, the jobs skipped for it wait again |
| `approve#PipelineRun` | update, or authorization disabled | A run that does not wait is an error |
| `find#CiSecrets`, `store#CiSecret`, `delete#CiSecret` | update for a repository's; authorization disabled for an owner's | Values are written and never read |
| `find#CiEnvironments` | view (a repository's) | Without the target's token |
| `store#CiEnvironment`, `delete#CiEnvironment` | as secrets | Delete is refused while a secret is scoped to it |
| `find#RunnerPools`, `store#RunnerPool`, `delete#RunnerPool`, `create#RunnerPoolToken` | an operator's (the caller's service authorization), or the application's | |
| `get#RunnerUsage` | view, or authorization disabled for an owner's | A month's `minutesUsed`, `secondsUsed`, `untrustedMinutesUsed`, `jobCount` |
| `delete#RepositoryRuns` | delete, or authorization disabled | Everything of a repository |

**A job's log.** While the job runs `get#JobLog` gives the chunks after `afterChunkSeq` (`chunkList`,
`nextChunkSeq`); once it has ended (`finished`) it gives the stored log from `offset` as `text` (`nextOffset`). A
page that tails passes back both `nextChunkSeq` and `nextOffset` each time, so it loses no line when the job ends
between two reads. With `watch` the caller is sent a notification on the topic `GitCiJobLog` (sub-topic the job
run's id; `{jobRunId, nextChunkSeq, finished}`) when the log grows, at most once a second, for three minutes after
each read, so a page reads when it is told and need not poll every second. The watchers are kept in the memory of
the server that took the read: on a cluster a page still polls, less often.

The engine's own services are the rules' and the jobs': `queue#PushRuns`, `queue#MergeRequestRuns`,
`queue#ScheduledRuns`, `start#QueuedJobRuns`, `end#OverdueJobRuns`, `expire#RunData`.

## The engine

- **Queueing** reads the file at the event's commit, asks the application's check, and writes the run, its jobs (each
  with its `jobSpec`, the planned job as JSON) and their pending statuses.
- **Starting.** `start_QueuedJobRuns_frequent` (and a pass soon after a run is queued) starts queued jobs oldest
  first, up to each pool's `maxConcurrent`: it picks the pool (the one the check named, else an enabled pool of the
  job's pool name that takes the run's trust, the owner's before a shared one), checks the image against
  `allowedImages`, makes the job token and calls the host type's start service. A job with no pool, an image that is
  not allowed, or a host that does not start it is an `error`.
- **Ending.** The runner's result ends a job: its steps left are closed, its log becomes one blob, its usage row is
  written, its status is written, its token is cleared and its container removed. Then the run moves on: waiting
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
| `max-artifact-bytes`, `max-report-bytes`, `max-cache-bytes`, `max-artifacts` | 1 GB, 16 MB, 2 GB, 100 | Uploads |
| `artifact-retention-days`, `max-artifact-retention-days`, `log-retention-days`, `cache-retention-days` | 30, 365, 90, 30 | |
| `lost-seconds`, `queue-timeout-minutes` | 180, 1440 | |
| `runner-max-requests-per-minute`, `runner-command`, `start-inline` | 1200, `moqui ci-run`, true | |
| `container-user`, `container-read-only-root`, `container-workdir`, `container-tmp-mb` | 1000:1000, true, `/workspace`, 1024 | A job's container |

## Tests

The engine is tested against the fake host type with a real `git` push and the runner in the test's process
(`crates/moqui-git/src/server/ci_tests.rs`), the file's reader, planning, JUnit and the runner's pieces beside their
code, and the container driver's requests against a small Docker API in the test, over TCP and a Unix socket
(`ci/driver/docker_tests.rs`). One job on a real Docker or Podman socket is opt-in:

```sh
MOQUI_CI_DOCKER_SOCKET=/var/run/docker.sock cargo test -p moqui-git --test ci_docker -- --nocapture
```

It needs an image with a shell (`MOQUI_CI_DOCKER_IMAGE`, `docker.io/library/busybox:latest` by default), starts one
container through the driver's services, inspects it, and removes it.

## Not built

- **Deployments** (Dogfood §8.6, DG10): the `deploy` event, `GitCiEnvironment` and `GitDeployment` are there so they
  need no schema change; nothing queues a deploy run, decides one or deploys.
- **A small static runner** for an image without the `moqui` binary.
- **Windows registered runners**: steps run with `sh -c`, and a step past its time is killed by process group on
  Unix only.
- **Egress rules** are the host's: the engine names a network (`egressRule`) and makes none.
- **Log tailing across a cluster** by notification (see `get#JobLog`).
- **A pool's limit on a cluster** is counted by the server that starts a job: one start pass runs at a time in a
  process, and a job is claimed by an update that names its state, which is not one statement on every database. Two
  servers starting at the same moment can put a pool a job over `maxConcurrent`, or start a second container for a
  job: only one token is the job's, so the other runner is refused and stops, and its container is removed with the
  job's.
- **Merge request runs of forks and of pushes for review** (Dogfood F4, F5): the engine reads `sourceRepositoryId`,
  `pushedForReview` and `hiddenDate` from the merge request and treats such a run as untrusted (and a hidden one's as
  hidden); they are tested by a run asked for as such, since those fields arrive with those features.
- Kubernetes, AWS and other providers' APIs as drivers (Dogfood D19).
