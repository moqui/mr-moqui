# Data improvement plan (Not fully verified)
- Replace `seed`, `seed-initial`, `install` and `demo` load types with explicit ownership and sensitivity.
- Seed: code-owned, persisted/queryable, protected from ordinary database edits.
- Configuration: editable settings; secrets: deployment-provisioned references, never exported values.
- Transactions: business-owned records; PII requires consistent fake replacements or exclusion.
- Reuse Moqui entity XML, component ordering, loader/writer and existing secret-resolution rules.
- Dev export: filter sensitive fields before serialization; cover identifiers, nested payloads, history and attachments.
- Preserve scoped relationships and business invariants; substitute dev endpoints/secrets and exclude sessions/backlogs.
- Code import: `git diff` last applied revision against incoming code, then parse typed records by entity/primary key.
- Three-way compare previous code, incoming code and DB; update only when incoming changed and DB matches previous.
- Unchanged/already-applied records: no write; operator changes: conflict; additions/deletions require absence/match and relationship checks.
- Compare/write atomically; advance the applied baseline only after commit; redact conflicts and support idempotent retries.
- Persist a protected seed flag; only the internal importer holding a scoped seed semaphore may mutate seed or its flag.
- Amend architecture/load gates for merge/seed guards, secret/SSO plans for references, and search/messaging plans for safe rebuild/reset.
- Deliver classification → merge/guards → dev export/import; test conflicts, concurrency, bypasses, leaks and no production traffic.

Note (Nov 25, 2026): the Dev AIDE has the three-way part for one data file ([design-dev-aide.md](../archive/design-dev-aide.md) §12):
Load the changed records compares each record by entity and primary key in the base (the text its last load committed,
a `DevDataLoadBaseline` row, else the last commit's), the file and the database; writes only where the database still
has the base's values, shows a conflict with Keep database or Take file, deletes a removed record only by choice, writes
in one transaction and moves the baseline after the commit, so a retry finds its writes applied; a conflict hides
password and encrypted fields. The rest of this proposal (ownership and sensitivity, seed protection, dev export,
importing code across repositories) is not built.
