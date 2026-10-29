# The framework's client specs

HTTP and WebSocket proofs of the framework's security, run from outside the server by `moqui test-client`
([plans/archive/ClientTestHarness.md](../../../../../plans/archive/ClientTestHarness.md) 7). They port Moqui Java's pytest suite
(`framework/test/*.py` in the Java checkout, with `SECURITY_TESTS.md` and `SecurityTestSupport.groovy`); the catalog
below lists each of its tests.

```sh
moqui test-client framework --serve                  # starts the server suite.js declares, runs, stops it
./moq run                                            # or a server of your own, in one terminal,
./moq test-client framework                          # and the specs against http://localhost:8080
moqui test-client framework --spec SecurityAuthn,SecurityCors --server https://test.example.com --insecure
```

`--serve` starts `moqui test framework --spec ClientFixtures --db snapshot --serve` under
`conf/MoquiLoadTestConf.xml` (`suite.js`): the production conf with the screen, transition and service tarpits off, so
that `LoadBasic` is not answered 429. Against another server:

- **The conf.** `SecurityCors` needs a server that allows no foreign origin: the production or load test conf. On the dev conf
  (`webapp_allow_origins=*`) its `setupSpec` fails with "this server allows every origin (the dev conf)", as the Python
  suite's `require_production_conf` fails it, and the foreign-origin proof of `SecurityWebSocket` skips. The other
  specs pass on any conf.
- **The fixture users.** `../ClientFixtures.spec.js` makes the `sec.*` accounts the proofs log in as, with
  Java's passwords, as `SecurityTestSupport.ensureUsers` did. They are not demo data. `sec.ip.v4`, `sec.ip.loop`
  and `sec.mfa.totp` are rows only: a password login from here is not how those proofs start. For a server
  `--serve` did not start, run it on that server's database:
  `moqui test framework --spec ClientFixtures --db existing`. Without them the features that need them skip,
  naming the command.
- **Users.** The other proofs log in as `john.doe`/`moqui` (`--param user=`, `--param password=`). None gives it a
  wrong password, since an account locks on its fourth failed login, and none logs it out over REST, which
  would end its other sessions (`UserAccount.hasLoggedOut`).

Each feature's name starts with its OWASP id, and a comment above it names the Python tests it ports, so
`grep -n test_<name> *.spec.js` finds the proof.

[Java-Port-Catalog.md](Java-Port-Catalog.md) maps each of Java's 143 HTTP tests to its proof here (125 ported, 18 not
applicable) and lists the deliberate differences from Java (also in
[Parity-Diffs](../../../../../plans/Parity-Diffs.md)).
