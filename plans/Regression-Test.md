# Regression test: `moqui test-regression`

Nov 22, 2026 · P1 and P2 are built (`crates/moqui-host/src/test_regression/`). P3 to P6 are open.

This plan replaces the separate Java regression harness (Python and Node, its own repository) with a test
inside MR Moqui. The runner is Rust. The specs are JS, in the format of `moqui test` and `moqui test-client`.
There is no Python, npm or Docker.

The test starts two engines, runs the same specs on both, and compares what each one did. It has two uses:

| Use | Command | Pass means |
|---|---|---|
| Rust against Rust | `./moq test-regression rust:master rust:.` | Fully equal. Any difference is a regression. |
| Java against Rust | `./moq test-regression java:master rust:.` | Equal, except for the listed known differences. |

Related: [Parity-Mapping](Parity-Mapping.md), [Parity-Diffs](Parity-Diffs.md),
[ClientTestHarness](archive/ClientTestHarness.md).

## 1. Decisions

- **D1. One command, two targets.** `moqui test-regression <baseline> <candidate> [component...]`. The first target
  is the reference. The second is the one under test.
- **D2. A target is one of three forms.**
  - `rust:<ref>`: an MR Moqui git ref. `rust:.` is the working tree.
  - `java:<ref>`: a Moqui Java git ref. The checkout path comes from `--java-repo` or `MOQUI_JAVA_REPO`.
  - `java:.` is the Java checkout as it is: its built `moqui.war` and its `runtime/`.
- **D3. Rust runner, JS specs.** The runner is a module `test_regression` in `moqui-host`, next to `test_client`.
  It reuses that module's HTTP client, sessions, Chromium driver, spec runner and console report.
- **D4. Specs are regression specs, not new assertions.** A spec says what to do. The runner does it on both
  engines and compares the observations. A spec holds no expected values.
- **D5. Screens need functional equivalence only.** The test compares fields, labels, options, submitted
  values, HTTP statuses and the rows a screen writes. It does not compare pixels or raw HTML bytes.
  Screenshots are saved as evidence only.
- **D6. Database rows are compared after each case.** Rows are keyed by primary key. Columns that are
  volatile by design are masked from one list per component (§5).
- **D7. Both engines get the same time.** The runner writes the data clock file and gives its path to each
  engine (`MOQUI_TEST_DATA_CLOCK_FILE`). It starts at 2026-01-01T00:00:00Z and moves 1 second forward after each
  case. mr-moqui reads it. A Java build that does not read it uses the real clock, so its time columns differ.
- **D8. Approved differences are a file.** `test/regression/approved-differences.json` in the component. Only a Java
  target reads it. A run fails on a difference that is not listed. A listed difference that no longer occurs
  is reported, so the list gets shorter.
- **D9. PostgreSQL comes from the embedded server.** One embedded PostgreSQL holds two databases, one per
  engine. The cluster listens on 127.0.0.1 so that Moqui Java connects over JDBC. `MOQUI_TEST_JDBC_JAR`
  names the PostgreSQL driver jar when the Java runtime has none.
- **D10. Builds are cached by commit SHA.** A ref is resolved to a SHA first. A Rust build goes to
  `target/test-regression/rust-<sha12>/`, with one shared cargo target directory. `rust:.` uses the current
  binary. A Java build goes to the Java checkout's `target/test-regression/java-<sha12>/`.
- **D11. Secrets stay random.** Password salts and session tokens are not made predictable. The compare
  masks them. The field-encryption key is one fixed test key, so ciphertext is equal on both sides.
- **D12. Opt-in in `accept-all`.** Rust against Rust runs in `./moq accept-all`. Java against Rust runs there
  only with `MOQUI_TEST_JAVA_REGRESSION=1`, as `MOQUI_TEST_MYSQL` does for MySQL.

## 2. Parts

| Part | File | Work |
|---|---|---|
| Command | `test_regression/mod.rs` | Parse targets and options. Run both targets. Compare. Exit 0 or 1. |
| Targets | `test_regression/target.rs` | Resolve a ref to a SHA. Build it once in a git worktree, or reuse the build. |
| Engine | `test_regression/engine.rs` | Copy the runtime and conf. Start the engine on a free port with the data clock. Wait for ready. Stop it. |
| Database | `test_regression/db.rs` | The embedded PostgreSQL cluster, one database per target. Snapshots and row diffs. |
| Recorder | `test_regression/record.rs`, `host.rs`, `prelude.js` | The spec host: `app` on top of the test-client host. After each case: the row changes, then the clock moves. |
| Compare | `test_regression/compare.rs` | Mask the volatile and approved data. Compare boot rows, answers and row changes. `report.json` and the console lines. |

## 3. Spec format

Specs are in `test/regression/*.spec.js` of a component. They use `spec`, `feature`, `where` and `expect` as
the client specs do, with every test-client global (`session`, `browser`, and the others). The runner adds `app`:

```js
feature("convert #amount from #uomId to #toUomId", {
    where: [
        { amount: "2", uomId: "REGRESSION_M", toUomId: "REGRESSION_CM" },
        { amount: "200", uomId: "REGRESSION_CM", toUomId: "REGRESSION_M" },
    ],
    expect({ amount, uomId, toUomId }) {
        app.service("org.moqui.impl.BasicServices.convert#Uom", { uomId, toUomId, amount }).ok
    },
})
```

- `app.service(name, parameters)` runs a service through Tools > Service > Run (its `runJson` transition), as
  john.doe after a login through the login form. Both engines have that screen, so one route serves both.
  The answer is `{ ok, status, body }`. The runner records it.
- `app.record(name, value)` records a value the spec read some other way.
- Each feature, or each `where` row, is one case. The runner takes the row changes after each case.

An `expect` condition only says that the engine answered. A spec holds no expected values: the baseline's run
is the expectation.

## 4. What is compared

| Kind | Compared | Not compared |
|---|---|---|
| Service call | Outputs, error messages, rows written | Elapsed time |
| HTTP request | Method, status, redirect target, JSON body after masking | Header order, session cookies |
| Screen | Form names, field names, labels, option lists, values, visible messages, link targets | Pixels, HTML bytes, CSS, asset version keys |
| Submit | The posted form values, then the rows written | Token values |
| Startup | Schema, row counts, rows of seed tables | Tables of §5 |

Requests are matched by method, URL and occurrence. They are not matched by arrival order.

## 5. Volatile data

One file per component, `test/regression/volatile.json`, used by every compare:

- `columns`: masked in every table.
- `tables`: skipped.
- `tableColumns`: masked in one table.

The file has a `description`. A column goes on the list only when two runs of one engine give different
values for it. Timestamps are compared as given: the data clock makes them equal.

## 6. Phases

| Phase | Content | Done when |
|---|---|---|
| **P1** | Command, launcher and Rust adapter. Startup compare. Database compare. Service steps. Port the 16 service cases as specs. | `rust:. rust:.` is equal in 3 runs of 3. A seeded bug in one side is found. |
| **P2** | Java adapter. Java on the embedded PostgreSQL. Approved-differences file. | `java:<pinned> rust:.` passes the 16 cases with a reviewed list. |
| **P3** | Screen model and functional compare. Port the Data Document Fields cases. | The 8 cases pass on both pairs. A changed label fails. A pixel-only change passes. |
| **P4** | Application specs: Mantle and SimpleScreens on Java, MR Mantle and MR SimpleScreens on Rust. | A first set of order and party specs runs on both pairs. |
| **P5** | `./moq test-regression`, `accept-all`, docs page `docs/dev/Parity-Test.md`. | D12 works. Help text is in `./moq help`. |
| **P6** | Archive the old harness repository. | The Rust test covers every case the harness passed. |

P1 needs no Java, no JDK and no browser. It is about half of the Rust code (estimate: 2,000 lines in all).

## 7. What moves from the old harness

Only knowledge moves. No code is ported line by line.

- The 16 service cases and their inputs.
- The volatile column list and the reasons.
- The approved differences, as the first approved-differences file.
- The data clock proof: one spec that reads a stamp back and checks it equals the supplied time.
- The Data Document Fields operation plan.

## 8. Risks

- **Java on embedded PostgreSQL (D9).** If JDBC cannot connect, Java gets a database URL option, and the
  user supplies a PostgreSQL. Decide in P2.
- **Old refs.** A ref without the data clock runs on the real clock, so its time columns differ.
- **Rust background writes.** The artifact index writes after ready and races the clock. P1 skips its table
  (§5). A later fix can finish the index before ready.
- **Java build time.** A first build of a ref takes minutes and needs JDK 21. The SHA cache makes it a
  one-time cost.
- **Loader transaction boundaries differ** (Java per file, Rust per record). The test compares state after
  a step, not each transaction, so this does not matter here.

## 9. Decided questions

1. The name is `test-regression`.
2. The framework's regression specs are in the `framework` component's `test/regression/`.
3. The Java checkout is given with `--java-repo` or `MOQUI_JAVA_REPO`.

## 10. Proof that the port works

A phase is not done until its proof exists as files the user can open. A summary is not proof.

### 10.1 Same verdicts as the old harness

Run the old harness and the new test on the same two builds, pinned by SHA and by artifact hash.

| Check | Pass means |
|---|---|
| Per case verdict | Each ported case has the same pass or fail result in both tools. |
| Per case rows | The rows the new test reports as written equal the rows in the old harness captures. |
| Per case outputs | The service outputs equal the old captures. |
| Differences | Each difference the old harness found is found by the new test, or is in §5 with a reason. |

The result is one table, old against new, one line per case, with the path of both evidence files.

### 10.2 It finds real, known regressions

Fixes on master that removed a real Java difference each have a commit. For each one:

1. Run `java:<pinned> rust:<commit before the fix>`. The test must fail at that case.
2. Run `java:<pinned> rust:<commit of the fix>`. That case must pass.

This uses real history, not invented faults. The first set is the merged fixes that a framework spec covers.

### 10.3 It finds seeded faults

A set of small patches, each applied to a scratch worktree and run as `rust:. rust:<patched>`.

| Seeded change | Must |
|---|---|
| A service output value changes | Fail |
| A written column value changes | Fail |
| One more row is written | Fail |
| One row is not written | Fail |
| A field label changes | Fail |
| An HTTP status changes | Fail |
| A CSS color changes | Pass |
| An asset version key changes | Pass |

A test that never fails proves nothing. The "must pass" lines prove it is not too strict.

### 10.4 It is repeatable

`rust:. rust:.` runs 3 times. All 3 results are equal, and each reports zero differences.

### 10.5 What each run saves

`runtime/tmp/test-regression/<runId>/`:

- `report.json`: both targets with their commits, the rule files read, the boot verdict, and each case's verdict
  with its differences.
- `baseline/recording.json`, `candidate/recording.json`: each case's calls, answers and row changes, and the
  row counts after boot.
- `baseline/engine.log`, `candidate/engine.log`: both engine logs.

### 10.6 Proof per phase

| Phase | Proof needed |
|---|---|
| P1 | §10.3 (row and output lines), §10.4 |
| P2 | §10.1 for the 16 cases, §10.2 |
| P3 | §10.1 for the 8 screen cases, §10.3 (label, status, CSS lines) |
| P4 | §10.4 on the application specs |
| P6 | §10.1 complete. Only then is the old harness archived. |

## 11. How to run it

```sh
./moq test-regression rust:master rust:. framework              # two MR Moqui refs
./moq test-regression rust:. rust:. framework                   # repeatability: must be equal
./moq test-regression java:. rust:. framework --java-repo ../moqui-framework
./moq test-regression java:. rust:. framework --spec Uom        # one spec
```

Options: `--spec Name[,Name]`, `--feature text`, `--java-repo path`, `--out dir`, `--keep` (keep the runtimes and
the database cluster in `--out`), `--start-timeout seconds`, `--log-level level`. See `moqui help test-regression`.

A Java target needs a JDK 21 or newer: set `JAVA_HOME`, else the runner uses `java` on the `PATH`. The runner writes
a copy of the target's `MoquiDevConf.xml` with HTTPS off and no gRPC listener, so the engines serve plain HTTP on a
free port.

## 12. The test files

All are JS, in a component's `test/regression/` directory. First set, in the `framework` component:

| File | Phase | Content |
|---|---|---|
| (every run) | P1 | The boot compare: the rows of every table after an empty-database start. |
| `Enumeration.spec.js` | P1 | 7 cases: create type, create, duplicate refused, update, store, delete, delete type. |
| `Uom.spec.js` | P1 | 9 cases: create two units, create a conversion, convert forward and reverse, update, delete all. |
| `DataDocFields.spec.js` | P3 | 8 screen cases: field and condition create, update, delete and cancel, in both render modes. |
| `volatile.json` | P1 | The columns and tables that differ from one run to the next. |
| `approved-differences.json` | P2 | The reviewed Java differences. Not read for Rust against Rust. |

`Enumeration.spec.js` and `Uom.spec.js` are the 16 service cases of the old harness. P4 adds specs in the
application components' own `test/regression/` directories.
