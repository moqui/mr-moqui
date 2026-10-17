# Embedded Postgres backup

The job `backup_EmbeddedDatabase_daily` takes a live backup of each embedded Postgres cluster this process runs, rotates the local archives, and can mirror them to SFTP. The schedule is the ServiceJob cron (`0 0 2 * * ?` in the seed). The job is paused. Set `paused` to `N` to run it.

The service is `org.moqui.impl.ServerServices.backup#EmbeddedDatabase`. `moqui-core` holds the retention and mirror decisions and the service. `moqui-host` runs `pg_dump` and `pg_basebackup` and registers that runner when a postmaster starts. `moqui-core` does not spawn Postgres tools. With no embedded cluster the service returns `skipped=Y` and does not raise `ServiceJobError`.

`moqui save-db` and `moqui load-save` are the same custom-format dump and restore, run by hand against one cluster directory. This job is that dump on a schedule, plus rotation, plus the physical option below. The cluster layout is [EmbeddedPostgres.md](EmbeddedPostgres.md). The retention counts follow `docker/postgres_backup.sh` in the Java tree: 7 daily, 5 weekly, 12 monthly, and yearly with no limit.

## Three live mechanisms

`backupMode=dump` is the default. `pg_dump --format=custom` takes an MVCC snapshot (`ACCESS SHARE`) of the `moqui` database and writes one compressed file. Moqui stays up. Nothing about the postmaster configuration changes. Restore is `moqui load-save`.

`backupMode=incremental` uses `pg_basebackup -X stream`. The first archive in a generation, and every `fullEvery` archive after that (default 7, and the full counts as one), is a full physical copy of the whole cluster. The runs between those are block incrementals (`pg_basebackup --incremental` against the prior full's `backup_manifest`, Postgres 17+). A full is checked with `pg_verifybackup` before it is published. The checkpoint is the server's default spread checkpoint.

A block incremental needs `summarize_wal=on`. That setting is applied on the entity cluster and on a dedicated search cluster the next time this process starts the postmaster. The sim overlay does not get it. `archive_mode` stays off: a failed `archive_command` fills `pg_wal` until the postmaster panics. When `SHOW summarize_wal` is still off, or the prior manifest or WAL summaries are missing, the job takes a full `pg_basebackup` and still succeeds. A `pg_hba.conf` refusal and a timeout do not fall back that way, because a full would fail the same way. The job reports a `pg_hba` refusal and leaves `pg_hba.conf` unchanged.

The job backs up a postmaster this process started. It does not dump a remote server and it does not run `pg_basebackup` against a server it does not administer.

Filesystem snapshots, and external tools such as pgBackRest, Barman, and wal-g, are outside this job. Point-in-time recovery through `archive_mode` is the case the panic above rules out.

## What is in the archive

The entity cluster (`runtime/db/embedded`, database `moqui`) is included. When search uses that database, the `search` schema is in the same dump. When search has its own embedded cluster (`runtime/db/search`), that cluster is a second archive with the prefix `search-`. Both external, or a build without the embedded feature, skips the run.

The sim overlay is ephemeral and is left out. `crypt-pass`, `runtime/.env`, and other files outside the data directory are left out. Restoring a dump without the field-encryption secret leaves encrypted values unreadable. That secret is a deployment concern, kept with the rest of the runtime configuration.

## Retention

Every run publishes one archive as a daily. The file is written under a `.partial` name and renamed into place, so a killed job does not look complete. Leftover partials are removed at the start of the next run. Files are mode `0600` and directories `0700`.

Names are `moqui-<UTC stamp YYYYMMDDTHHMMSSZ>-d.dump` for a dump, with `-w`, `-m`, and `-y` after promotion. A physical backup is a directory of the same stem, without `.dump`. A search cluster uses the prefix `search-`. A second backup in the same second uses the next free second.

Before anything is deleted, completed periods are promoted, in the server's time zone (the same zone the cron expression uses). A period is completed when it is before the period of the backup just taken. The current week, month, and year stay where they are. For each completed ISO week, the newest daily of that week is renamed to a weekly. The same pass then promotes the newest weekly of each completed calendar month to a monthly, so a daily can climb to monthly in one run. Completed years promote from monthly to yearly. The rename is one step from the name on disk to the final name.

The counts then drop the oldest of each tier. `dailyCount` defaults to 7, `weeklyCount` to 5, `monthlyCount` to 12. `0` keeps none of that tier. `yearlyCount` is unset in the seed, and an empty value keeps every yearly. These are counts of archives, not lengths of time. With the default nightly cron, 7 dailies is 7 nights. An hourly cron with `dailyCount=7` keeps the last 7 runs.

A kept incremental keeps its parent full and the incrementals between them. Those parents do not take a slot in the tier they sit in. A sidecar `moqui-backup.txt` inside the directory records `kind` and `parent`. When a parent is renamed, the child's sidecar is updated. The sidecar is written after `pg_verifybackup`, so the check sees the tree the tool wrote.

The new archive is published before anything is pruned. One rename or delete that fails is logged and the rest continue.

`backupDir` empty means `runtime/db/backup`, a sibling of `embedded` and `search`. A directory inside a data directory, or one that contains a data directory, is refused before any tool runs.

## Parameters

| Parameter | Default | Meaning |
|---|---|---|
| `dailyCount` | 7 | Dailies to keep. `0` keeps none. |
| `weeklyCount` | 5 | Weeklies to keep. |
| `monthlyCount` | 12 | Monthlies to keep. |
| `yearlyCount` | empty | Empty keeps every yearly. |
| `backupMode` | `dump` | `dump` or `incremental`. |
| `backupDir` | empty | Empty is `runtime/db/backup`. |
| `fullEvery` | 7 | Archives in one physical generation, including the full, before the next full. Dumps do not count. |
| `timeoutMinutes` | 120 | Kills the tool's process group. The job's `transactionTimeout` of 7200 is recorded and does not bound the work: the service is `transaction="ignore"`. |
| `systemMessageRemoteId` | `EmbeddedDbBackup` | The `SystemMessageRemote` for the mirror. |

Out parameters: `archivePath` (the `moqui` archive when both clusters ran, otherwise the one that ran), `backupKind` (`dump`, `full`, or `incremental`), `bytesWritten` (the sum), `promoted`, `archivesRemoved`, `mirrored` (`Y`, or `N` when the mirror is off or did not finish), `skipped`.

A cluster that fails is named in the log and the other clusters still run. The run still succeeds locally for the clusters that published. `ServiceJobError` fires when any cluster or the mirror reports an error. A prefix that failed is not pruned, and its archives already on disk stay in the set the mirror keeps.

## SFTP mirror

`systemMessageRemoteId` names a `SystemMessageRemote`. The seed row `EmbeddedDbBackup` has no `sendUrl`, username, or password, so the mirror is off and `mirrored` is `N`. Set `sendUrl` (host or host:port; the port defaults to 22), `username`, and a password or a private key plus its public key. An empty `sshHostKeyFingerprint` trusts the host key on first use into `runtime/conf/ssh_known_hosts` and refuses a key that changes. `remoteAttributes=N` skips permission bits.

The remote directory is `SystemMessageType.sendPath` of the remote's type. The seed type uses `moqui-backup`, a path relative to the login, with no substitution. Entity and search archives share that directory; the prefix distinguishes them.

The mirror renames a remote file when a local promotion left the old name there, uploads retained locals the remote lacks (including incremental parents), then deletes remote names that match this job's archive pattern and are no longer in the local keep set. Other files stay. Partials are not uploaded. One file that fails is logged and the loop continues. A later run uploads anything still missing. When the mirror was on and did not finish, `mirrored` is `N` and the job records the error. The local archive is already in place.

## Restore

Stop Moqui before `moqui load-save` of a dump onto a cluster it is using. `pg_restore --clean` against the live pool is the case that stop avoids.

A physical backup is restored with the cluster stopped. `pg_combinebackup -o restored` takes the full and each incremental up to the one you want, in order. Replace the data directory with that result, then start Moqui. There is no separate restore command.

## Tests

`crates/moqui-core/src/backup.rs` tests the ladder and the mirror plan in UTC. `crates/moqui-host/src/db/backup.rs` starts an embedded cluster, dumps it, restores the row, and when `pg_combinebackup` is in the theseus install runs a full and an incremental. `crates/moqui-host/tests/it/backup.rs` runs the service and the SFTP mirror against the built-in SSH server.
