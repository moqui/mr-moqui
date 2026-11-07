# Run and Deploy

This guide covers running MR Moqui on your machine and deploying it on a server, in a container or as a cluster. It
also covers connecting instances to each other: PASS servers to a BOSS server, and developers' instances to a team's
group server. The first sections give the steps for the common cases; the sections after them have the detail. Building
and the build packages are in [Build-Test](Build-Test.md).

- [Quick start](#quick-start): develop on your machine, run one production server, run in a container
- [Everyday commands](#everyday-commands)
- [Choose a topology](#choose-a-topology)
- [Production checklist](#production-checklist)
- Detail:
  - setup: [Runtime directory and conf](#runtime-directory-and-conf), [Command line](#command-line),
    [Data and the first start](#data-and-the-first-start), [Database and search](#database-and-search),
    [HTTPS, ports and proxies](#https-ports-and-proxies);
  - deployments: [One production server](#one-production-server), [Cluster](#cluster),
    [PASS servers with a BOSS server or cluster](#pass-servers-with-a-boss-server-or-cluster),
    [Dev instances with a dev group server](#dev-instances-with-a-dev-group-server),
    [A local server with an edge node](#a-local-server-with-an-edge-node), [An all-in-one node](#an-all-in-one-node);
  - reference: [Components](#components), [Moving from Moqui Java](#moving-from-moqui-java),
    [Directory layout](#directory-layout).

## Quick start

### Develop on your machine

You need Rust 1.99 or newer (`rustc --version`; quickjs-oxide declares 1.99, and `rust-toolchain.toml` pins 1.99.0),
git and bash, on Linux or macOS.

```sh
git clone https://github.com/moqui/mr-moqui.git
cd mr-moqui
./moq component-get apps   # optional: Marble ERP (mr-marble) and IronStone BOSS (mr-boss), with Mantle (mr-mantle)
./moq load                 # builds the Maximal package the first time, then loads every data type, demo included
./moq run                  # serves http://localhost:8080; ctrl-c stops it
```

Open `http://localhost:8080` and log in as `john.doe` with the password `moqui`.

- **The first build** downloads the embedded Postgres archive once (`scripts/pg-payload.sh strip`).
- **The dev conf** (`runtime/conf/MoquiDevConf.xml`) loads every data type into an empty database on the first start,
  so a plain `./moq run` gets the same data. `./moq load` is for loading again later.
- **To update**, `./moq git-pull` pulls every repository, and `./moq run` builds before it starts.

### Run one production server

On a build machine with the same OS and CPU as the server, build the Maximal package into an archive, leaving out the
local database and secrets:

```sh
./moq archive --release --no-data   # mr-moqui-YYYYMMDD.zip, which expands to mr-moqui-YYYYMMDD/
```

On the server:

1. Make the `moqui` user and unpack the archive at `/opt/moqui`. The archive directory is a runtime directory with the
   binary in it. The `systemd/` files here and in step 4 come from a checkout.

   ```sh
   sudo systemd-sysusers systemd/moqui.sysusers   # makes the moqui user
   unzip mr-moqui-20261029.zip
   sudo install -d -o moqui -g moqui /opt/moqui
   sudo cp -a mr-moqui-20261029/. /opt/moqui/ && sudo chown -R moqui:moqui /opt/moqui
   ```

2. Put the certificate for your host in `/opt/moqui/cert/`, for example certbot's `cert/erp.example.com/fullchain.pem`
   and `privkey.pem` ([certificates](#certificates)).
3. Write the host's settings to `/opt/moqui/.env`. The first start adds the generated secrets to the same file.
   `webapp_https_port` is the port clients use; leave it out when they reach the server on 443.

   ```sh
   sudo -u moqui tee /opt/moqui/.env > /dev/null <<'EOF'
   webapp_http_host=erp.example.com
   webapp_https_enabled=true
   webapp_https_port=8443
   EOF
   sudo chmod 600 /opt/moqui/.env
   ```

4. Install the unit and start it:

   ```sh
   sudo cp systemd/moqui.service /etc/systemd/system/
   sudo systemctl daemon-reload && sudo systemctl enable --now moqui
   ```

5. The first start creates the tables and loads the seed, seed-initial and install data. Open
   `https://erp.example.com:8443/Login` and create the first administrator with **Create Initial Admin Account**. The
   form is there only while nobody has an account.
6. Back up `/opt/moqui/.env` with the database. It holds `entity_ds_crypt_pass`, the key of the encrypted fields.

Upgrades, backups, binding port 443 and the rest are in [One production server](#one-production-server).

### Run in a container

```sh
podman/build.sh                                    # ./moq build --release, then the image localhost/moqui:latest
cp podman/env.example podman/.env                  # then change the passwords
systemctl --user start podman.socket
podman compose -f podman/moqui-compose.yml up -d   # one container, embedded Postgres
podman exec moqui-moqui-1 curl -fsS http://127.0.0.1:8080/health
```

The image runs the production conf, so the first start loads the seed, seed-initial and install data. Three more
stacks (Postgres; Postgres and OpenSearch; MySQL and OpenSearch), the data volume and Quadlet are in
[podman/README.md](../podman/README.md).

## Everyday commands

`./moq` runs the tasks that were Gradle tasks in Moqui Java. `./moq help <task>` describes one, and a task name matches
by the prefix of each part (`./moq r-p` is `run-production`).

| Task | `./moq` | Moqui Java |
|---|---|---|
| Run the dev server | `./moq run` | `./gradlew run` |
| Run on another port | `./moq run -p 8081` | `java -jar moqui.war port=8081` |
| Run with the production conf | `./moq run-production` | `./gradlew runProduction` |
| Set a property for one run | `./moq run -D entity_ds_db_conf=postgres` | `-Dentity_ds_db_conf=postgres` |
| Load every data type | `./moq load` | `./gradlew load` |
| Load some types | `./moq load seed seed-initial install` | `./gradlew load -Ptypes=seed,seed-initial,install` |
| Load one component's data | `./moq load seed components=mr-mantle` | `java -jar moqui.war load components=...` |
| Load one file | `./moq load -l runtime/component/mr-mantle/data/SomeData.xml` | `java -jar moqui.war load location=...` |
| Start over with an empty database | `./moq clean-db && ./moq load` | `./gradlew cleanDb load` |
| Load once, then save a snapshot | `./moq load-save` | `./gradlew loadSave` |
| Save a snapshot / restore it | `./moq save-db before-upgrade`, `./moq reload-save before-upgrade` | `./gradlew reloadSave` |
| Pull every repository | `./moq git-pull` | `./gradlew gitPullAll` |
| Status of every repository | `./moq git-status` | `./gradlew gitStatusAll` |
| Get a component and its dependencies | `./moq component-get mr-boss` | `./gradlew getComponent -Pcomponent=...` |
| Run a component's specs | `./moq test mr-mantle` | `./gradlew runtime:component:mantle-usl:test` |
| Run client specs against a server | `./moq test-client framework` | |
| Archive the runtime with the binary | `./moq archive --release` | `./gradlew addRuntime` |
| Remove the database, logs and old build output | `./moq clean-all` | `./gradlew cleanAll` |
| Show the binary's package and features | `target/release/moqui version` | |

## Choose a topology

| Topology | Use it for | Package | Database | Section |
|---|---|---|---|---|
| One dev instance | Developing on your machine | Maximal with the dev tools (the default) | Embedded Postgres | [Quick start](#develop-on-your-machine) |
| One production server | One business or one site | Maximal (`--release`) | Embedded Postgres, or an external server | [One production server](#one-production-server) |
| Cluster | More capacity, or a member that can fail | Enterprise (`--enterprise`) | One external Postgres for all members | [Cluster](#cluster) |
| PASS servers with a BOSS server or cluster | Public sites (forum, members) with no business data on the public machines | PASS: any server package, Lite is the smallest. BOSS: Maximal or Enterprise | Each PASS server its own; BOSS its own | [PASS](#pass-servers-with-a-boss-server-or-cluster) |
| Dev instances with a dev group server | A team: shared repositories, tasks and models | Laptops: Dev. Group server: Maximal or Enterprise | Each its own | [Dev group](#dev-instances-with-a-dev-group-server) |
| Public demo server | The apps for anyone to try with the `john.doe` login | Maximal (`--release`) | Embedded Postgres, restored on a schedule | [Public demo server](#public-demo-server) |
| A local server with an edge node | A server in the office, behind a firewall or a changing address, with a public presence: web access, mail, chat webhooks, public sites | Local: Maximal or Enterprise. Edge node: Maximal | Each its own; the edge node keeps only the link's queue and copies | [A local server with an edge node](#a-local-server-with-an-edge-node) |
| An all-in-one node | A public server, with its own users, apps and data, that is also the edge node of one local node (a desktop or office node), so no second cloud instance is needed | Server: Maximal. Local: Maximal or Enterprise | Each its own; the server also keeps the local node's queue and copies | [An all-in-one node](#an-all-in-one-node) |
| The desktop app | A node on a person's own computer, in a window with a tray, paired with an edge or all-in-one node | Maximal, in the app | Embedded Postgres in the app data directory | [moqui-desktop](../crates/moqui-desktop/README.md) |

## Production checklist

- **Package**: Maximal for embedded Postgres, or Enterprise for an external database. Dev and Debug carry the dev tools
  and are not for deployment ([Packaging](Build-Test.md#packages)).
- **Purpose**: `instance_purpose` is `production` everywhere except in the dev conf. An archive's `MoquiConf.xml` is
  the production conf.
- **Secrets**: the first start writes generated secrets to `runtime/.env` with mode 0600. Keep the file private and back
  it up. Without `entity_ds_crypt_pass` the encrypted fields cannot be read ([conf](#runtime-directory-and-conf)).
- **Data**: no demo data, so no `john.doe` with password `moqui` ([first start](#data-and-the-first-start)).
- **HTTPS**: a certificate in `runtime/cert`, or a proxy in front with `webapp_trusted_proxies` set
  ([HTTPS](#https-ports-and-proxies)).
- **Ports**: firewall 8080. `webapp_http_bind` sets the address of both HTTP and HTTPS, so `127.0.0.1` suits only a
  server behind a proxy on the same machine. The cluster listener (8444) starts on every server, on every address, so
  firewall it unless cluster members use it ([ports](#ports)).
- **Backups**: run the embedded database's backup job, or back up the external database
  ([backups](#backups)).
- **Listeners**: SSH, inbound SMTP, A2A, MCP and the LLM gateway are off by default. Turn on only the ones that are used.
- **CORS**: leave `webapp_allow_origins` empty. The dev conf sets `*`.
- **Lock-down**: the full list is in [Attack-Surface.md, Heavy lock-down](security/Attack-Surface.md#heavy-lock-down).

---

## Runtime directory and conf

The runtime directory holds everything that varies per instance: components, conf, certificates, the database, logs
and secrets ([runtime/README.md](../runtime/README.md)). The binary takes the first directory with a `base-component/`
in it, from this list:

1. `MOQUI_RUNTIME`;
2. `runtime` in the working directory;
3. `runtime` beside the binary;
4. the binary's own directory, which is how an unpacked archive runs as `<dir>/moqui` from anywhere.

Keep the runtime path short. The embedded Postgres socket, `<runtime>/db/embedded/.s.PGSQL.5432`, must be under 108
bytes, or the start fails with "postgres socket path is N bytes".

The conf is merged in this order:

1. `base-component/framework/conf/MoquiDefaultConf.xml`;
2. each component's `MoquiConf.xml`, in component order;
3. the runtime conf, on top.

The runtime conf is the first of these that is found:

1. the file `MOQUI_CONF` names, relative to the runtime directory or absolute (`./moq run -c FILE` and `moqui --conf
   FILE` set it);
2. `<runtime>/MoquiConf.xml`;
3. `<runtime>/conf/MoquiDevConf.xml`.

`./moq run` uses the dev conf and `./moq run-production` uses `conf/MoquiProductionConf.xml`. A running server takes
a changed conf file without a restart where what reads the setting allows it (System › Conf, `systemctl reload moqui`
or SIGHUP, and the file watcher on a dev instance), and lists the rest as waiting for a restart
([Component-Deploy](tools/Component-Deploy.md)). Conf merges add components but never remove one. To leave a component out, keep it out of `runtime/component`. The example dev
component is the exception: `example_disabled=true` leaves it out. The demo dev component is off until
`demo_disabled=false` or a conf names it ([Public demo server](#public-demo-server)).

### Properties

A `default-property` in a conf is a default only. A property is looked up in this order:

1. a `-D name=value` on the command line (`./moq -D` sets it as an environment variable);
2. the process environment;
3. `runtime/.env`;
4. the merged conf's `default-property`.

Use the underscore names (`entity_ds_db_conf`), not dotted ones. An empty value counts as unset, so it cannot clear a
conf's default; to clear one, override the `default-property` in the runtime conf.

`runtime/.env` holds `KEY=value` lines. Blank lines and `#` comments are ignored, a matching pair of quotes is
stripped, and `$` is not expanded. It is read after the conf merge, so it cannot select `MOQUI_CONF`. An archive made
with `--no-data` leaves it out, so it is the place for per-host settings that should survive an upgrade.

When a secret property still ends in `CHANGEME` at startup, a value is generated and written to `runtime/.env`. This
covers `entity_ds_crypt_pass`, `session_cookie_key`, `ssh_host_key`, `git_lfs_token_key`, `cluster_tls_cert`,
`cluster_tls_key` and `edge_link_key`. A new file gets mode 0600, and a `.env` that cannot be written fails the start.
`mail_dkim_key` (empty by default) is made there too, the first time a server needs it.

The properties most deployments set or should know. Their defaults are in `MoquiDefaultConf.xml`.

| Property | Default | What it does |
|---|---|---|
| `instance_purpose` | `production` (`dev` in the dev conf) | `production`, `dev` or `test`. Only `dev` loads the dev components (Dev AIDE, the example component) |
| `default_locale`, `default_time_zone`, `database_time_zone` | `en_US`, `UTC`, `UTC` | |
| `webapp_http_host` | empty: the request's host | The host in generated URLs |
| `webapp_http_bind` | empty: every address | The address the HTTP and HTTPS listeners bind; `127.0.0.1` keeps them on the machine |
| `webapp_http_port` | empty: 8080 | The HTTP listen port (`-p`) |
| `webapp_https_listen_port` | empty: 8443 | The HTTPS listen port (`--https-port`), or `off` |
| `webapp_https_port` | empty: 443 | The HTTPS port in generated URLs and redirects |
| `webapp_https_enabled` | `false` | `true` makes generated URLs https |
| `webapp_allow_origins` | empty (`*` in the dev conf) | Other origins allowed for CORS |
| `webapp_trusted_proxies` | empty | Address ranges of the proxies in front ([proxies](#behind-a-proxy-or-load-balancer)) |
| `webapp_client_ip_header` | empty | The header those proxies put the client address in |
| `webapp_status_ips` | `127.0.0.1` | Addresses, besides loopback, that `/health` answers |
| `scheduled_job_check_time` | `60` | Seconds between job checks; `0` runs no scheduled jobs |
| `moqui_chromium_path` | empty | The Chromium for PDF and the browser tests; empty looks in `runtime/chromium`, then for an installed Chromium, Google Chrome or Microsoft Edge (the PATH and the OS's install locations) |
| `ssh_port`, `ssh_host` | empty, `127.0.0.1` | The SSH server (SFTP, git, the apps in a terminal); off while `ssh_port` is empty (`2222` in the dev conf) |
| `rtc_media_port`, `rtc_media_host`, `rtc_public_address` | empty, `127.0.0.1`, empty | Call media ([WebRTC](interfaces/WebRTC.md)); off while `rtc_media_port` is empty; the public address is what browsers are told, for a server behind 1:1 NAT |
| `call_ring_seconds` | `45` | How long a call rings before it is missed |
| `chat_server_name`, `chat_to_device_retention_days` | `localhost`, `30` | The server name in the Matrix user IDs of browsers' crypto devices (set it before the first device: a change makes every device new), and how long a to-device message waits for a device that does not take it ([Chat keys](interfaces/Chat-Keys.md)) |
| `chat_store_key_secret` | generated | The platform secret each crypto device's browser-store key is derived under (HMAC of user, device and revision), so no table holds the keys. A new value makes every browser's store unreadable: each makes a new device and reads older messages from the key backup ([Chat keys](interfaces/Chat-Keys.md)) |
| `rtc_cascade_enabled` | `false` | Cascading media nodes ([WebRTC](interfaces/WebRTC.md#cascading-media-nodes)): a leg's media goes to the media node of its own socket, which relays to the call's node over both nodes' `rtc_media_port`; set it on every node |
| `call_captions_enabled`, `call_captions_provider`, `call_captions_store` | `false`, `llm`, `false` | Live captions in calls ([WebRTC, Captions](interfaces/WebRTC.md#captions)): off unless `true`; the provider is the model bound to the `transcribe` feature (`local`, speech-local, is not built yet); `true` keeps each caption as a `CallTranscript` |
| `call_captions_chunk_seconds`, `call_captions_queue`, `call_captions_workers`, `call_captions_timeout_seconds`, `call_captions_language` | `5`, `6`, `2`, `20`, empty | The longest chunk of speech sent, the chunks that wait (past this the oldest is dropped), how many of a call's go at once (all calls on a node together use at most half the service pool), how long one may take, and the language (empty lets the model tell) |
| `turn_port`, `turn_tls_port`, `turn_tls443_host`, `turn_public_host`, `turn_auth_secret`, `turn_external_urls`, ... | empty; the secret generated | The embedded TURN server, or coturn instead ([TURN](interfaces/TURN.md)) |
| `call_recording_enabled`, `call_recording_ack_required`, `call_recording_policy_revision` | `false`, `false`, `1` | Call recording by a call's host, with a notice to every leg, and whether each leg must acknowledge it ([Call recording](interfaces/Call-Recording.md)) |
| `call_recording_retention_days` | `90` | Recordings are deleted this long after they start; `0` keeps them |
| `call_recording_queue_frames`, `call_recording_segment_mb`, `call_recording_segment_minutes`, `call_recording_max_mb` | `4096`, `64`, `30`, `2048` | The recorder's queue, when a file is cut into the next segment, and where a recording stops |
| `call_recording_prompt_audio`, `call_captions_prompt_audio` | empty | WAV files a phone leg hears after the recording and captions tones ([SIP, Notices](interfaces/SIP.md#notices)) |
| `sip_port`, `sip_host`, `sip_tls_port`, `sip_tls_host`, `sip_rtp_ports`, `sip_rtp_host`, `sip_public_address`, `sip_tls_ca_file` | empty, `127.0.0.1`, empty, `sip_host`, `20000-20999`, `sip_host`, empty, the system bundle | Phone calls over SIP trunks ([SIP](interfaces/SIP.md)); off while both ports are empty, and only beside call media; the public address is what trunks are told in Contact and SDP |
| `channels_webhooks_enabled`, `channels_webhook_max_bytes` | `false`, `262144` | The chat bridges' routes a provider calls: `POST /channels/{chatBridgeId}/webhook` (its body at most this many bytes) and `GET /channels/{chatBridgeId}/media/{token}` ([SMS](interfaces/SMS.md)) |
| `channels_public_url`, `channels_media_key`, `channels_media_minutes` | empty, generated, `60` | The https base a provider reaches this server at, for the files of an MMS that goes out (none without it), the key their addresses are signed with, and how long one works |
| `sip_request_rate`, `sip_request_burst`, `sip_max_transactions`, `sip_max_connections`, `sip_max_connections_per_source` | `20`, `100`, `2000`, `500`, `100` | The SIP listener's flood caps: new requests a second per source and at once, the transactions past which new requests are dropped, TCP and TLS connections at once, and from one source ([SIP, Settings](interfaces/SIP.md#settings)) |
| `sip_registrar_enabled`, `sip_domain` | `false`, empty | The registrar for desk and soft phones, off even with the SIP listeners on; the realm and domain phones register to (`sip_public_address`, else `moqui`, when empty) ([SIP, Registrar](interfaces/SIP.md#registrar)) |
| `xmpp_domain`, `xmpp_rooms_domain`, `xmpp_upload_domain` | empty, `rooms.<domain>`, `upload.<domain>` | XMPP for local users ([XMPP](interfaces/XMPP.md)): off while `xmpp_domain` is empty; users are `user@<domain>`, rooms `room@rooms.<domain>` |
| `xmpp_port`, `xmpp_host`, `xmpp_tls_port`, `xmpp_tls_host`, `xmpp_websocket_enabled` | empty, `0.0.0.0`, empty, `xmpp_host`, `false` | The XMPP client listeners: STARTTLS (5222 by convention) and direct TLS (5223), each off while empty; `/xmpp-websocket` on the HTTP listener when `true` |
| `xmpp_upload_url`, `xmpp_upload_max_bytes`, `xmpp_upload_secret` | `https://<domain>` (with `webapp_https_port`), `20971520`, generated | The HTTPS base of upload URLs (`/xmpp-upload/`), the largest upload, and the secret the URLs of chat files sent to XMPP clients are derived with |
| `xmpp_resume_seconds`, `xmpp_fast_token_days` | `300`, `14` | How long a dropped session waits for stream resumption, and how long a FAST token lasts |
| `xmpp_upload_seconds` | `300` | The longest a PUT to an upload slot may take |
| `xmpp_turn_enabled`, `xmpp_turn_per_hour` | `false`, `30` | External services (XEP-0215): TURN credentials for XMPP apps' calls, never to an external user, at most so many a user an hour |
| `xmpp_scram_decoy_key` | generated | Derives the SCRAM salts of names that are no user's, so they stay the same across restarts and nodes |
| `xmpp_s2s_port` | empty | Server to server (5269, federation, C3b): not built, nothing listens on it yet |
| `call_forward_max_hops`, `call_forward_ring_seconds` | `5`, `30` | The most forwards one phone call takes (a user's rules, a ring group's fallback; 1 to 10), and how long a forward to an outside number rings before the call goes on (10 to 120) ([SIP, Call handling](interfaces/SIP.md#call-handling)) |
| `call_queue_tick_seconds`, `call_transfer_ring_seconds` | `5`, `30` | How often the SIP engine checks each call waiting in a queue (offers, the wait limit, the caller's place; 1 to 30), and how long a blind transfer's target rings before the caller goes to their voicemail (10 to 120) ([SIP, Queues](interfaces/SIP.md#queues), [Transfer and merge](interfaces/SIP.md#transfer-and-merge)) |
| `call_hold_music_location`, `call_queue_prompt_location` | empty | Hold music for calls on hold and in queues without their own (WAV or Ogg Opus, looped; empty: a tone pattern; use only audio you may use), and a folder of `0.wav` to `9.wav` that speaks a queue caller's place (empty: pips) ([SIP, Queues](interfaces/SIP.md#queues)) |
| `sip_registrar_min_expires`, `sip_registrar_max_expires`, `sip_registrar_default_expires`, `sip_registrar_nat_expires` | `60`, `3600`, `600`, `60` | A phone binding's expiry in seconds: its bounds, when the phone asks none, and behind a NAT without Outbound ([SIP, Registrar](interfaces/SIP.md#registrar)) |
| `voicemail_max_seconds`, `voicemail_trash_days` | `120`, `30` | The longest voicemail message (10 to 600), and how long a deleted one waits in the trash before it and its audio go (0 keeps them) ([Voicemail](interfaces/Voicemail.md)) |
| `voicemail_prompt_location` | empty | A folder (any resource location) with the recorded speech played after the voicemail tones: `greeting.wav`, `enter-box.wav`, `enter-pin.wav`, `pin-refused.wav`, `menu.wav`, `no-messages.wav`, `goodbye.wav`; empty plays tones only ([Voicemail, Prompts](interfaces/Voicemail.md#prompts)) |
| `voicemail_pin_min_digits` | `6` | The fewest digits a new voicemail PIN has (4 to 12); four fall to the box's limit of failed tries in weeks ([Voicemail](interfaces/Voicemail.md#phone-access)) |
| `voicemail_transcribe`, `voicemail_transcribe_language` | `false`, empty | `true` sends each voicemail to the speech provider (the model bound to `transcribe`) for its transcript, by the deployment's recording and transcription policy; the language as an ISO 639-1 code |
| `node_role` | empty | `edge` makes this an edge node, which takes links from paired local nodes on its HTTPS listener ([Edge node](#a-local-server-with-an-edge-node)); `all-in-one` makes a full server that also takes one local node's link, routing each item by its owner ([All-in-one node](#an-all-in-one-node)). Another value is logged and means a normal node |
| `edge_link_key` | generated | The key an edge node's links use; generated on every node, used only on an edge node. Local nodes pin it, so a new one means each must pair again |
| `edge_node_url`, `edge_node_key`, `edge_node_pin` | empty | On a local node, its edge node: the address, this node's private key (a secret) and the pin of the edge node's link key. Pairing writes them to `runtime/.env` and unpairing removes them; they are not generated |
| `edge_calls_enabled` | empty (on) | On a paired local node, whether new calls have their media on the edge node's media node; `false` keeps its media row as draining, so new calls use this node's own media nodes and calls already on the edge node go on. The wizard's Apply writes it and unpairing removes it ([Edge node](#a-local-server-with-an-edge-node)) |
| `edge_sip_enabled` | empty (on) | On a paired local node, whether its trunks and phones go through the edge node's SIP engine; `false` sends the replicated sets `voiceTrunks` and `voiceFallbacks` empty, so the edge node registers no trunk and answers no number from a fallback, and keeps its `SipNode` row but gives it no new call. The wizard's Apply writes it and unpairing removes it ([SIP](interfaces/SIP.md#behind-an-edge-node)) |
| `edge_ssh_tunnel_port`, `edge_ssh_tunnel_host`, `edge_ssh_tunnel_node` | empty | On an edge node, a port whose connections go to a local node's SSH server, the address it listens on (empty for every one) and the node (empty for the only one linked) |
| `edge_xmpp_port`, `edge_xmpp_tls_port`, `edge_xmpp_host`, `edge_xmpp_node` | empty | On an edge node, the ports whose connections go to a local node's XMPP server, STARTTLS (5222) and direct TLS (5223), the address they listen on (empty for every one) and the node (empty for the only one linked) |
| `edge_offline_page` | empty | On an edge node, the location of the page a terminate host shows while its node's link is down; empty for the built-in one |
| `edge_public_addresses` | empty | On an edge node, its public IPv4 and IPv6 addresses, comma-separated, which it offers its local nodes for their A and AAAA records; empty, and the wizard expects the addresses of its MX host name |
| `edge_mail_mta` | `moqui` in MoquiEdgeConf.xml, else empty | On an edge node, what delivers the local node's mail out: `moqui` (to each domain's MX), or empty or `postfix` (relayed to the MTA beside it, E5a) |
| `edge_mail_relay_host`, `edge_mail_relay_port` | `127.0.0.1`, `10025` | On an edge node with `edge_mail_mta=postfix`, the MTA beside it that mail the local node sends is relayed to (Postfix's relay port) |
| `edge_mail_bounce_domain` | empty | On an edge node, the domain of the envelope sender of mail it sends out, `bounce+<item>@` it, whose bounces the listener takes back to the message; empty sends from the message's own sender |
| `smtp_internet`, `mail_rspamd_url` | empty | `true` runs the SMTP listener in internet mode (port 25; `true` in MoquiEdgeConf.xml), with Rspamd's HTTP API for its spam verdict when the URL is set ([Edge node](#a-local-server-with-an-edge-node), Mail) |
| `mail_dkim_key`, `mail_dkim_selector`, `mail_dkim_domains` | generated, `moqui`, empty | The node's DKIM key (base64 PKCS#8, or PEM; RSA-2048 made into `runtime/.env` the first time a server needs it), its selector, and the domains signed for (empty: the hosted accounts' domains). `get#DkimRecord` gives the TXT record. A send by SMTP is signed only when the key is set or the send goes to this node's own listener |
| `mail_arc_trusted_sealers` | empty | Domains whose passing ARC seal overrides a DMARC reject or quarantine on port 25; empty for no ARC exception |
| `mail_delivery_retry_seconds`, `mail_delivery_days` | `300`, `5` | On an edge node with the MTA in moqui, the first wait before a delivery is tried again (doubled each time, to 4 hours), and when a message not delivered goes back as a bounce |
| `mail_mx_override`, `mail_dns_offline` | empty | Testing and staging only: `domain=host:port` pairs (`*` for any) used in place of the MX, and `true` for no DNS query |
| `moqui_log_level`, `moqui_log_dir` | `info`, `runtime://log` | The console level, and the directory of `moqui.log` and `moqui-error.log` |
| `search_backend` | `postgres` | `postgres` or `opensearch` ([search](#search)) |
| `example_disabled` | `false` | `true` leaves out the example component and its gallery on a dev instance |
| `demo_disabled` | `true` | `false` loads the demo lock-down on a dev instance; a demo server names the component in its conf instead |
| `llm_openai_url`, `llm_openai_api_key`, `llm_openai_model` | `https://api.openai.com`, empty, empty | The default model endpoint for Assist and the LLM services (the dev conf points it at `http://127.0.0.1:1234`) |

The database properties are in [Database and search](#database-and-search), and the cluster properties in
[Cluster](#cluster).

## Command line

`moqui` with no command starts the server. `moqui help` lists the commands, and `moqui help <command>` describes one.

| Command | What it does |
|---|---|
| `moqui [options]` | Start the server in the foreground |
| `moqui serve [options]` | Start it in the background. It returns once the server is listening and prints the address and process id; logs go only to the files. Stop it with SIGTERM |
| `moqui load [name=value ...]` | Load data files into the configured database, then exit |
| `moqui save-db <cluster-dir> <dump-file>` | `pg_dump` of an embedded cluster, usually `runtime/db/embedded` |
| `moqui load-save <cluster-dir> <dump-file>` | `pg_restore` into an embedded cluster; stop the server first |
| `moqui tui [--path P] [--user U]` | The apps in this terminal, on this runtime; `--server URL` reaches a running server instead ([TUI](ui/TUI.md)) |
| `moqui test`, `moqui test-client` | Component specs ([Testing](Build-Test.md#component-specs), [Client-Testing](dev/Client-Testing.md)) |
| `moqui lint`, `check`, `refs`, `types`, `lsp` | Checks and tools for developers |
| `moqui version` | The version, package and features of this binary |

Server options:

| Option | What it does |
|---|---|
| `-p N`, `--port N` | The HTTP port (`webapp_http_port`) |
| `--https-port N` | The HTTPS listen port (`webapp_https_listen_port`), or `off` |
| `-c FILE`, `--conf FILE` | The runtime conf, as `MOQUI_CONF` does |
| `--no-run-es` | Do not start the OpenSearch in `runtime/opensearch` (`opensearch_run=false`) |
| `-D name=value` | Set a property for this run; repeatable |

The Java forms `port=N` and `conf=FILE` also work.

`./moq run`, `run-production`, `load` and the other tasks that run the binary pass the arguments after their own
options to it, so `./moq run lint --screens` is `moqui lint --screens` with the dev conf. Their package flags (`--lite`,
`--debug`, `--release`, `--enterprise`) pick the binary, and no flag means the Maximal package (the same as `--release`). They build first unless
given `--nobuild`.

`moqui load` arguments are `name=value` pairs. `./moq load` also takes bare words as data types.

| Argument | What it does |
|---|---|
| `types=a,b` | Data types to load. Omitted: seed, seed-initial and install. Empty or `all`: every type |
| `components=a,b` | Load only these components' data files |
| `location=FILE` | Load only this file |
| `timeout=SECONDS` | Transaction timeout of each file (default 600; each file is one transaction) |
| `raw=true` | For an empty database: no entity ECA rules, audit log, foreign keys or data feed |
| `disableEntityEca`, `disableAuditLog`, `disableFkCreate`, `disableDataFeed` | Each of those alone (`=true`) |

## Data and the first start

When a server starts on a database with no `moqui.basic.Enumeration` rows, it loads the data types in
`entity_empty_db_load` (default `seed,seed-initial,install`; `all` in the dev conf). It then starts its factory again,
and the log has:

```
Found 0 Enumeration records, loading empty-db-load data types (seed,seed-initial,install)
Data loaded into empty DB, re-initializing ExecutionContextFactory
```

Every start checks the tables. `entity_add_missing_startup` (default `true`) creates missing tables and adds missing
columns and indexes. Columns are never dropped or changed.

While the only accounts are the seeded ones nobody logs in as (`_NA_` and the agents' chat users), the login page
shows **Create Initial Admin Account**. The account it makes is in the `ADMIN` group. Once any person has an account,
the form is gone and the service refuses the call.

| Type | What it holds |
|---|---|
| `seed` | What the code needs: enumerations, status flows, security groups, jobs |
| `seed-initial` | Starting rows a deployment may change |
| `install` | Setup for a new instance |
| `demo` | Demo data and users (`john.doe`); never in production |
| others | Components add their own, such as `test` |

To load more on a server that already has data, stop it and run `moqui load types=...` (or `./moq load-production
...`). On embedded Postgres a second process cannot open the data directory while the server holds it.

By default an entity's create-or-update is one statement that inserts or updates (`INSERT ... ON CONFLICT (pk) DO
UPDATE`, on MySQL `ON DUPLICATE KEY UPDATE`) where nothing depends on which it does: no create or update ECA rule, no
audit-logged, create-only or defaulted field, no optimistic lock, and the user may both create and update the entity.
Otherwise it is UPDATE first, then INSERT. `-D create-or-update=true` switches to Moqui Java's read-first form
(SELECT, then INSERT or UPDATE). Where the row is looked for first, the INSERT does nothing when another transaction
has created the row since (`ON CONFLICT DO NOTHING`, on MySQL `ON DUPLICATE KEY UPDATE`), and that row is then updated.
Either way, concurrent calls for one new key do not fail on it
([Parity-Diffs](../plans/Parity-Diffs.md#atomic-create-or-update-2026-10-30)).

## Database and search

### Embedded Postgres

Without `entity_ds_db_conf` (or with Moqui Java's `h2`), the server runs its own Postgres. Every package but Enterprise
embeds it.

- **Data**: `runtime/db/embedded`, with two databases: `moqui` for the entities and `moqui_content` for the stored
  files ([Content database](#content-database)).
- **Binaries**: unpacked from the moqui binary on the first start, into `runtime/install/postgres` or under
  `MOQUI_INSTALL`.
- **Connections**: on Unix there is no TCP port. The server connects through a socket in the data directory, so
  nothing else on the network reaches the database.

### External Postgres or MySQL

Set `entity_ds_db_conf` and the other `entity_ds_*` properties, in the environment, `.env` or the runtime conf. There
are examples in `runtime/conf/MoquiDevConf.xml`. Use `mysql8` for MySQL 8 and `mysql` for MariaDB (which has no
`LATERAL`) and MySQL 5.7. Both create `utf8mb4` tables, so any character (an emoji) can be stored; create the database
with `CHARACTER SET utf8mb4`. A database made before, with 3-byte `utf8` tables, needs a fresh load or each table
converted by hand (`ALTER TABLE ... CONVERT TO CHARACTER SET utf8mb4`). Loading every data type into each was checked
([MySQL-Issues](../plans/archive/MySQL-Issues.md)).

| Property | Example | What it does |
|---|---|---|
| `entity_ds_db_conf` | `postgres` | `postgres`, `mysql` or `mysql8`; empty is embedded Postgres |
| `entity_ds_host` | `db.internal` | Default `127.0.0.1` |
| `entity_ds_port` | | Empty is 5432 or 3306 |
| `entity_ds_database` | `moqui` | |
| `entity_ds_schema` | | Empty for the default schema |
| `entity_ds_user`, `entity_ds_password` | `moqui`, ... | |
| `entity_ds_sslmode` | | Empty is verified TLS. `verify-full` checks the authority and the name, `verify-ca` the authority only, `require` encrypts without checking, and `disable` turns TLS off (for a local dev server only) |
| `entity_ds_sslrootcert` | `/etc/moqui/db-ca.pem` | A PEM file of the authorities to trust instead of the public ones |
| `entity_ds_crypt_pass` | generated | The key of encrypted fields; it must move with the database |

The content datasource has its own `content_ds_*` properties ([Content database](#content-database)).

Each connection to an external database sets the datasource's transaction isolation level for its session: the
`inline-jdbc` `isolation-level`, else the database's `default-isolation-level`, `ReadCommitted` for Postgres and MySQL
in `MoquiDefaultConf.xml`, as Moqui Java does. `None` leaves the server's default (InnoDB's is `REPEATABLE READ`). The
code assumes READ COMMITTED: at REPEATABLE READ or SERIALIZABLE, Postgres fails an `ON CONFLICT` insert whose
conflicting row the transaction's snapshot does not see, and MySQL takes gap locks that two creates of one missing row
can deadlock on. On Postgres a level other than `ReadCommitted` is logged as a warning at startup; the embedded database
does not set it and runs at READ COMMITTED.

### Content database

The stored bytes of files are in a database of their own, apart from the busy business rows. This is the `content`
entity group:

- `DbResource`, `DbResourceFile` and `DbResourceFileHistory`, behind `dbresource://`;
- the content blobs (`ContentBlob*`), which hold every file over 64 KiB;
- the objects, refs and LFS objects of `db` git repositories (`GitObject`, `GitRef`, `GitLfsObject`).

Mail attachments, chat files, call recordings, voicemail, drives, wiki pages and other content files are all
`dbresource://` files, so they are all in it. The rows that point at a file keep only its location.

Only the resource implementations (`dbresource://`, the blob store, git) read and write these entities. Code everywhere
else uses `ec.resource`. No foreign key or view-entity crosses between the content database and the main one.

| `content_ds_db_conf` | Where the content database is |
|---|---|
| empty, embedded transactional database (default) | the `content_ds_database` database (`moqui_content`) in the embedded cluster |
| empty, external transactional database | the transactional database itself, with no second database |
| `postgres`, `mysql` or `mysql8` | `content_ds_database` on that server |

For a server of its own, set `content_ds_host`, `content_ds_port`, `content_ds_user`, `content_ds_password`,
`content_ds_sslmode` and `content_ds_sslrootcert`. Each one left empty takes its `entity_ds_` value. So setting only
`content_ds_db_conf=postgres` puts the content in a `moqui_content` database on the transactional server, which must
exist. `content_ds_schema` names a schema, as `entity_ds_schema` does.

One transaction spans both databases, as Moqui Java's transaction spans every datasource, but without two-phase commit:

- **Commit order**: the content database commits first, then the transactional one. A failure between the two commits
  leaves a file that no row names (a blob of it goes in the blob cleanup), never a row naming a file that is not
  there.
- **Rollback**: a rollback, or an error in either database, rolls back both.
- **The edge case**: a file deleted in a transaction whose transactional commit then fails stays deleted.

Each transaction can hold a connection to each database, so the content pool is the size of the transactional one (80).
Its 72 ordinary connections (8 are reserve) hold the request and service workers' (8 + 25) × 2, one agent worker and a
connection for each streamed download reading a window at once (the `webroot` webapp's `transfer-workers`, 4): 71. When
the content shares an external transactional database, that pool holds the same 71. The embedded cluster makes room for
both pools. Startup refuses a pool too small for the workers, and warns about one too small for the downloads' reads,
which then read fewer at once. A cluster of servers shares the content database as it shares the
transactional one, so it needs both external.

### Search

Search runs on Postgres by default: tables in the `search` schema of the entity database, or a dedicated embedded
cluster when the entity database is not Postgres. OpenSearch is optional:

- set `search_backend=opensearch`, and `elasticsearch_url` (default `http://127.0.0.1:9200`), plus
  `elasticsearch_user`, `elasticsearch_password` and `elasticsearch_index_prefix` as needed;
- `./moq opensearch-download` installs OpenSearch in `runtime/opensearch`;
- the server starts that install and stops it on shutdown when `elasticsearch_url` is on this host, nothing answers
  there, and there is no pid file. `opensearch_run=false` (or `--no-run-es`) turns that off;
- `./moq opensearch-start` and `opensearch-stop` run it as a daemon with a pid file.

### Snapshots

`./moq save-db [name]` writes the embedded database to `runtime/save/<name>/moqui.dump`, and the content database beside
it to `moqui_content.dump`. It also writes the OpenSearch data when there is any. `./moq reload-save [name]` empties the
database and restores that save. `./moq load-save -n <name> [types]` runs `clean-all`, loads, and saves. Stop the server
before a restore. These work on the embedded database only; an external database has its own tools.

### Backups

The job `backup_EmbeddedDatabase_daily` backs up the embedded clusters while the server runs. It is paused in the seed
data; set it running on System › Service Jobs, or with `embedded_db_backup_scheduled=true`, which each boot applies
(`false` pauses it). It runs daily at 02:00 and keeps 7 daily, 5 weekly and 12 monthly archives, and every yearly one,
in `runtime/db/backup`, or the directory in `embedded_db_backup_dir`.

- **Modes**: `backupMode=dump`, the default, writes a `pg_dump` file of each database: `moqui-<stamp>-d.dump` and
  `moqui_content-<stamp>-d.dump`, with the same stamp. `incremental` takes `pg_basebackup` fulls and block
  incrementals of the whole cluster, both databases in one copy.
- **Mirror**: archives can be copied to an SFTP server. Set `sendUrl`, the user and a password or key on the
  `EmbeddedDbBackup` system message remote.
- **Restore**: restore a dump with `moqui load-save` while the server is stopped. It also restores the content dump
  of the same name beside it (`moqui_content-<stamp>-d.dump` for `moqui-<stamp>-d.dump`). A physical backup is put
  together with `pg_combinebackup`.
- **Not included**: `.env` and the field-encryption key are not in the archive. Keep them with the backup.

The details are in [plans/archive/PostgresBackup.md](../plans/archive/PostgresBackup.md). The job does not back up an
external database.

## HTTPS, ports and proxies

### Ports

| Port | Listener | Default | Properties |
|---|---|---|---|
| 8080 | HTTP | On, every address | `webapp_http_port`, `webapp_http_bind` |
| 8443 | HTTPS, certificates from `runtime/cert`; HTTP/2 or HTTP/1.1 by ALPN, and gRPC over HTTP/2 when a gRPC service is on | On when there is a certificate (dev makes one) | `webapp_https_listen_port` (`off`), `webapp_http2` |
| 8444 | Cluster: TLS with the cluster certificate, client certificate required; gRPC on HTTP/2, or HTTP/1.1 | On in every serving process, every address | `cluster_https_port`, `cluster_bind_address`, `cluster_protocol` |
| `ssh_port` | SSH: SFTP, git, admin shell, the apps (`ssh -p 2222 john.doe@localhost` in dev) | Off; 2222 on 127.0.0.1 in the dev conf | `ssh_port`, `ssh_host` |
| `smtp_listen_port` | Inbound SMTP behind a mail gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)) | Off; Maximal and Enterprise only | `smtp_listen_port`, `smtp_listen_host` |
| `rtc_media_port` (UDP) | Call media: every browser leg of a call through the SFU ([WebRTC](interfaces/WebRTC.md)); with `rtc_cascade_enabled`, the relay links between the media nodes of a call too, so the nodes must reach each other's | Off; Maximal, Enterprise and Debug only | `rtc_media_port`, `rtc_media_host`, `rtc_public_address`, `rtc_cascade_enabled` |
| `rtc_media_tcp_port` (TCP) | ICE-TCP, for browsers that cannot reach the UDP media port ([TURN](interfaces/TURN.md)) | Off | `rtc_media_tcp_port` |
| `turn_port` (UDP and TCP), `turn_tls_port`, 443 on `turn_tls443_host`; `turn_relay_ports` (UDP) | The embedded TURN server and its relays ([TURN](interfaces/TURN.md)) | Off | `turn_port`, `turn_tls_port`, `turn_tls443_host`, `turn_host`, `turn_relay_ports` |
| `sip_port` (UDP and TCP), `sip_tls_port`; `sip_rtp_ports` (UDP) | SIP trunks, desk and soft phones (the registrar, with `sip_registrar_enabled`; `sip_tls_port` 5061 and `_sips._tcp` for remote phones), and the RTP of phone legs ([SIP](interfaces/SIP.md)) | Off; Maximal, Enterprise and Debug only | `sip_port`, `sip_host`, `sip_tls_port`, `sip_tls_host`, `sip_rtp_ports`, `sip_rtp_host`, `sip_public_address` |
| `xmpp_port` (5222), `xmpp_tls_port` (5223) | XMPP clients, STARTTLS and direct TLS with ALPN `xmpp-client` ([XMPP](interfaces/XMPP.md)); the WebSocket is on the HTTP listener (`/xmpp-websocket`) | Off; Maximal, Enterprise and Debug only | `xmpp_domain`, `xmpp_port`, `xmpp_host`, `xmpp_tls_port`, `xmpp_tls_host` |

XMPP clients find the server by DNS: `_xmpp-client._tcp.<domain>` (5222, STARTTLS) and `_xmpps-client._tcp.<domain>`
(5223, direct TLS, XEP-0368), both at the host's name; the certificates in `runtime/cert` must cover the domain,
`rooms.<domain>` and `upload.<domain>` (a wildcard of the domain does). `_xmpp-server._tcp` waits for federation (C3b).

`/health` answers `{"ready":true}` to loopback and to the addresses in `webapp_status_ips`, and 404 to everyone else.

### HTTP/2

The HTTPS listener offers HTTP/2 and HTTP/1.1 by ALPN, so browsers and other clients that speak HTTP/2 use it; the
plain HTTP listener is HTTP/1.1 only. `webapp_http2=false` keeps HTTPS to HTTP/1.1.

- **WebSockets** (`/notws`, `/callws`, `/termws`, `/xmpp-websocket`) open on an HTTP/2 connection by extended CONNECT
  (RFC 8441), or on a connection of their own over HTTP/1.1, as the client chooses.
- **gRPC.** When the OTLP/gRPC receiver or the PASS authority's gRPC is on, the HTTPS listener takes their calls over
  HTTP/2 too, so a client can use the HTTPS port instead of 4317; the gRPC listener stays.
- **Limits.** One HTTP/2 connection carries at most `webapp_http2_max_streams` (100) requests at once. One with no
  request for `webapp_header_timeout` (30 seconds) is closed with a GOAWAY, as an idle HTTP/1.1 connection is closed,
  and so is a client's connection when web protection refuses that client.
- **Slow readers.** A connection whose client takes nothing of a response for `webapp_write_timeout` (60 seconds) is
  closed, a WebSocket's included; a download that keeps moving is not. A WebSocket keeps its connection's place in
  `webapp_max_connections` and web protection's `max-per-client` until it closes.
- **Behind a proxy**, HTTP/2 is between the client and the proxy; the proxy may reach the server over HTTP/1.1 or, on
  the HTTPS port, over HTTP/2 (gRPC through a proxy needs HTTP/2 to the server).

### Certificates

Certificates are looked up by the requested domain in `runtime/cert` (git-ignored), in any of these layouts:

- nginx-proxy's `<domain>.crt` and `<domain>.key` (`dhparam.pem` files are ignored);
- certbot's or acme-companion's `<domain>/fullchain.pem` and `privkey.pem`;
- mkcert's `<domain>.pem` and `<domain>-key.pem`.

A subdomain falls back one label up only: first to `_wildcard.<parent>` (mkcert), then to `<parent>` (nginx-proxy).
After that, `default` serves any other name. A domain with no certificate gets a refused handshake. The files are read
again within seconds of a change, so a renewal needs no restart.

In dev (`instance_purpose=dev`), an empty directory gets a self-signed `localhost` certificate. Outside dev with no
certificate, the HTTPS listener does not start, or the start fails when the port was set explicitly.

### Listen ports and URL ports

`webapp_https_listen_port` is the port the server binds. `webapp_https_port` is the port clients see in generated URLs
and https redirects; empty is 443, as in Moqui Java. To make generated URLs use https, set `webapp_https_enabled=true`.

| Clients reach the server | Set |
|---|---|
| Directly on 8443 | `webapp_https_enabled=true`, `webapp_https_port=8443` |
| On 443, through a port redirect or a proxy | `webapp_https_enabled=true`; leave `webapp_https_port` empty |
| On 443, which the server binds itself | `webapp_https_enabled=true`, `webapp_https_listen_port=443`, and the capability to bind it (below) |

A firewall redirect from 443 to 8443 (nftables or iptables) keeps the server unprivileged. To bind 443 directly under
systemd, add a drop-in `/etc/systemd/system/moqui.service.d/https-443.conf`:

```ini
[Service]
AmbientCapabilities=CAP_NET_BIND_SERVICE
Environment=webapp_https_listen_port=443
```

### Behind a proxy or load balancer

The server terminates HTTPS itself, so a proxy is optional. A proxy or load balancer in front needs these settings:

- `webapp_trusted_proxies` lists the proxies' address ranges (`10.0.0.0/8`, `fd00::/8`, or single addresses). Only
  those peers are believed for `X-Forwarded-Proto` and the client address header. The client address is then the
  rightmost address that is not a trusted proxy.
- `webapp_client_ip_header` names the header: `X-Forwarded-For`, `X-Real-IP`, `CF-Connecting-IP` or
  `CloudFront-Viewer-Address`.
- Setting `webapp_trusted_proxies` also turns on the per-client web protection: rate limits, connection limits and
  bans by client address. Without a proxy, `webapp_web_protection=true` turns it on.
- Leave `webapp_https_port` empty when the proxy serves 443.

## One production server

### From an archive

`./moq archive` writes a zip, or with `--tgz` a gzipped tar. It expands to one directory, which is a runtime directory
with the binary in it.

- **What goes in**: the package's binary; `runtime/conf/MoquiProductionConf.xml` as `MoquiConf.xml` (`--conf FILE` puts
  in another); `README.txt`; and all of `runtime/` except `log/`, `tmp/`, `save/` and `install/`.
- **`--no-data`** leaves out the database, the OpenSearch data, `.env` and `cert/`, for a fresh instance or an upgrade.
  Without it the embedded database goes in as files, so stop the server first.
- **Platform**: the binary runs on the OS and CPU it was built for, so build on the server's platform.
- **By hand**: copy the binary into a runtime directory with a `MoquiConf.xml` in it, or set
  `MOQUI_CONF=conf/MoquiProductionConf.xml`. From a checkout, `./moq run-production` does the same.

`systemd/moqui.service` runs `/opt/moqui/moqui` as the `moqui` user.

- It is `Type=notify`: the server reports when the listener is bound and when it stops. An in-app restart runs the
  binary again in the same process and reports a reload, so the unit stays up.
- `/etc/moqui/environment` is optional, for variables that must exist before the process starts (`MOQUI_CONF`,
  `MOQUI_RUNTIME`). The server reads `/opt/moqui/.env` itself.
- For a database on the same machine, `systemd/drop-in/` has `After=` and `Wants=` drop-ins for Postgres and MySQL.

### Podman and Quadlet

`podman/build.sh` builds the Maximal package and the image `localhost/moqui:latest`, with whatever components are in
`runtime/component`. One volume at `/var/lib/moqui` holds the database, logs, certificates and generated secrets.
There are compose stacks for embedded Postgres, an external Postgres, Postgres with OpenSearch, and MySQL with
OpenSearch, and `podman/quadlet/` runs the single container as a systemd service. Everything is in
[podman/README.md](../podman/README.md).

### Upgrading

1. Back up. For embedded Postgres, the backup job's latest archive, or a dump taken with the server stopped:

   ```sh
   sudo systemctl stop moqui
   sudo -u moqui mkdir -p /opt/moqui/save
   sudo -u moqui /opt/moqui/moqui save-db /opt/moqui/db/embedded /opt/moqui/save/before-upgrade.dump
   ```

   For an external database, use its own backup.
2. On the build machine, run `./moq git-pull`, then `./moq archive --release --no-data`.
3. On the server, remove the old `moqui` binary, `base-component/` and `component/`, so files deleted upstream do not
   linger, and unpack the new archive over `/opt/moqui`:

   ```sh
   sudo rm -rf /opt/moqui/moqui /opt/moqui/base-component /opt/moqui/component
   sudo cp -a mr-moqui-20261105/. /opt/moqui/ && sudo chown -R moqui:moqui /opt/moqui
   ```

   `db/`, `.env`, `cert/` and `log/` are not in the archive, so they stay. `MoquiConf.xml` is replaced, which is why
   per-host settings belong in `.env`.
4. Start the server: `sudo systemctl start moqui`. The table check adds new tables and columns. New seed data is not
   loaded on its own. To load it, stop the server, run `sudo -u moqui /opt/moqui/moqui load types=seed`, and start it
   again. Leave out `seed-initial`, whose rows the deployment may have changed since.

A deploy marker for the dashboards can be posted from the upgrade script ([Monitoring](tools/Monitoring.md#deploy-markers-and-annotations)).

### Chat actors

The first upgrade past Comms C1 moves chat to actors ([Chat bridges](interfaces/Chat-Bridges.md#actors)): the room
members, mentions and poll votes are keyed by actor, not by user. The table check never changes a primary key, so a
one-time migration does it, `ChatServices.migrate#ChatActors`. It runs from `ChatBridgeSetupData.xml`, which a new
database loads on its own; an existing one loads it once.

1. **Maintenance window.** No server on the old code may write chat while it runs: stop every node of the cluster.
2. **Back up** the database ([Upgrading](#upgrading) step 1). On MySQL the table changes are not transactional, so the
   backup is the rollback.
3. Unpack the new build, then load the file with the server stopped (the table check runs first and adds the new
   columns and tables):

   ```sh
   sudo -u moqui /opt/moqui/moqui load location=runtime/base-component/framework/data/ChatBridgeSetupData.xml
   ```

   The log names each table rebuilt (`chat actors: moqui.chat.ChatRoomMember rebuilt with its new key, N of N rows`) and
   each problem. It gives every user the chat rows name an actor (deleted accounts' history too), gives each row its
   actor, rebuilds the three tables on their new keys with every row (all or nothing per table, the counts checked), and
   checks that every actor a row names exists and is its user's. A table with a row it could not give an actor is not
   rebuilt; the log says which, and the old shape keeps working for local users until it is fixed and loaded again.
   A row is given an actor only when its user's actor was made, so a user whose actor failed holds its table back. On
   MySQL a rebuild cut off after the new table was made leaves it empty beside its `*_PRE_C1` copy; a later load says so
   (`... is empty and ..._PRE_C1 has N rows`) and leaves the copy for you to restore.
4. Start the server. The chat search document gains the sender's actor and external name: reload
   `ChatSetupData.xml` (`types=seed-initial` rows, so `location=` it) to have them in the index.
5. **Rollback.** The old rows stay in `CHAT_ROOM_MEMBER_PRE_C1`, `CHAT_EVENT_MENTION_PRE_C1` and `CHAT_POLL_VOTE_PRE_C1`
   (on Postgres `chat_room_member_pre_c1` and so on). To go back to the old build, restore the backup; the copies are for
   checking and for a manual restore. Drop them once the new build has run a while.

Loading the file again does nothing.

### Optional services

| Service | How |
|---|---|
| PDF | Chromium: `./moq chromium-download` into `runtime/chromium`, or `moqui_chromium_path`. The server starts without it, and PDF is unavailable |
| OpenSearch | [Search](#search) |
| Outbound mail | SMTP through a `moqui.basic.email.EmailServer` record; every package has it |
| Inbound mail | IMAP polling, or the SMTP listener behind a gateway ([Mail-Inbound](interfaces/Mail-Inbound.md)); Maximal and Enterprise |
| SSH | `ssh_port`, with `ssh_host=0.0.0.0` to reach it from other machines ([TUI](ui/TUI.md), [Sync](interfaces/Sync.md), [Git-Hosting](interfaces/Git-Hosting.md)) |

## Public demo server

A server where anyone can try the apps with the `john.doe` / `moqui` login. It runs on the production purpose with
`conf/MoquiDemoConf.xml`, which loads the demo component (`runtime/dev-component/demo`) and loads all data, demo users
included, on the first start:

```sh
./moq load --release -c conf/MoquiDemoConf.xml
./moq save-db --release demo                    # the database to go back to
./moq run-production --release -c conf/MoquiDemoConf.xml
```

The component keeps the apps writable and takes away running code or SQL, bulk import and export, the file manager,
writes in Tools and System, SSH, WebDAV, MCP, A2A, the LLM gateway, mail to other hosts, and changes that would lock
visitors out of the shared login ([its README](../runtime/dev-component/demo/README.md)). Restore the saved database on
a schedule with `./moq reload-save --release demo`, with the server stopped. Do not run a demo as `instance_purpose=dev`:
the Dev AIDE loads there.

## A local server with an edge node

An edge node is a small server in the cloud in front of a local server, which dials out to it and keeps one link
open ([plan](../plans/Edge-Node.md)). The local server needs no open port, port forward or fixed address. Through the
edge node go: the local node's apps and its SSH server, mail (with the MTA in moqui on port 25, or Postfix and Rspamd
beside the edge node), chat providers' webhooks and MMS files, and mr-pass's public sites, which call the local node as
their PASS authority, the media of calls, with TURN, whose recordings come back to the local node, XMPP
clients through its tunnel ports, and phone calls (SIP trunks and outside phones), with a voicemail fallback while the
local node is away.

**Attach a local node in one sitting.**
1. Set up the edge node (below): its `.env` (`webapp_http_host`, the mail settings, `edge_public_addresses`), a WebPKI
   certificate, and a start with `conf/MoquiEdgeConf.xml`. In its System › Edge Clients, make a pairing code.
2. On the local node, in System › Edge Node, choose **Pair with an edge node**. The wizard's steps:
   1. **Edge node.** Its address and the pairing code.
   2. **Connect.** Check its name and link key fingerprint against Edge Clients. Next pairs and waits for the link.
   3. **Discover.** What the edge node offers, as a form with each area on or off and this node's choices filled in:
      the host names, passthrough or terminate (this node's `webapp_http_host` by default, unless it is `localhost` or
      has one label, and the names the edge node has for this node now); the mail domains (from the hosted and shared
      accounts), and whether the system's mail (off by default when `SYSTEM` has a server of its own) and the chosen
      accounts' mail goes out through the edge node; the chat bridges whose webhooks go to it; the PASS sites it
      serves, with their SSO callback; XMPP, this node's `xmpp_domain` through the edge node's tunnel ports (on by
      default when both are there; Next adds the domain to the passthrough names, as uploads and `/xmpp-websocket`
      ride on web access); calls, new calls' media on the edge node's media node with its TURN (on by default when
      it runs one, unless `edge_calls_enabled` is false here; office users in a call with each other then send media
      to the cloud and back, with no cascading to a media node of this node's own, and one local node per edge node
      takes calls); phones, this node's trunks and phones through the edge node's SIP engine (on by default when it
      runs one, unless `edge_sip_enabled` is false here), with the trunks and their numbers that go through it (from
      the replicated set `voiceTrunks`), its registrar for outside phones, which cannot register while this node is
      away, and how many numbers have a voicemail fallback, which follows no call-handling rules or schedules; a
      number with none is refused (480) then. A host name must be a public DNS name:
      not an IP address, `localhost` or a name with one label.
   4. **Apply.** The whole setup is checked first, and nothing changes when any of it is not right. The edge node's
      part goes first (host names, mail domains, bridges, sites, the XMPP domain): it takes only this node's names, and refuses a name
      another node holds and its own names (its public names, its SMTP host, its bounce domain, its sites' hosts, and
      their subdomains); a name of this node not listed is removed. When the edge node does not take the setup,
      nothing changes on this node. Then it writes this node's part: an `EmailServer` `EDGE` with `smtpHost` `edge:`,
      `SYSTEM`'s `smtpHost` set to `edge:` when chosen, and the chosen accounts sending through it;
      `channels_public_url` (the edge node's address); `pass_edge_client`, `pass_client_edge_sites` and
      `pass_client_edge_callback`; `edge_calls_enabled`, and the edge node's media row written again with it (draining
      while off), with a line each for its media address (public), the TURN secret its link handed this node, and the
      row (live and first for new calls); `edge_sip_enabled`, with a line each for the replication of `voiceTrunks`
      and `voiceFallbacks` (their cursor and the changes waiting; empty while off), the edge node's `SipNode` row,
      the realm phones made here get (the edge node's, and a failed line for phones made for another), and the
      numbers with and without a fallback. The host or server each mail row had is kept (`edgePriorSmtpHost`,
      `edgePriorServerId`), and **unpairing puts it back**; mail for an `edge:` server while this node is not paired is
      not queued, but fails as retryable and stays in the Outbox. Every replicated set is sent whole again. An area
      turned off is left as it is. Then it lists the DNS records to set, each looked up live: MX, SPF, the DKIM key and DMARC for
      each mail domain, A and AAAA for each host name, and with XMPP `_xmpp-client._tcp` and `_xmpps-client._tcp`
      for the XMPP domain, naming the edge node and its tunnel ports, and with calls A and AAAA for the host of its TURN
      URLs when it is a name (`turn_public_host` there), naming its addresses or its media address, and with phones
      `_sips._tcp`, `_sip._udp` and `_sip._tcp` for the edge node's `sip_domain`, naming the edge node and its SIP
      ports. Check again looks them up again. `_xmpp-server._tcp` (federation) is listed for later. A line says when this node's `xmpp_port` or
      `xmpp_tls_port`, which Chat apps shows, differ from the edge node's ports.
   5. **Test.** The link, mail in (the edge node takes mail for a hosted account's address), mail out (the system's
      and the accounts' servers are on `edge:`, and the edge node can send: its DKIM key, or its Postfix relay
      answering), the webhooks, PASS, and XMPP (a client connection through each tunnel port reaches this node's XMPP
      server, its TLS this node's own certificate, and the domain is one of its host names there), and calls (the edge
      node's media row is live and first for new calls, `make#IceServers` signs its TURN URLs with its secret, and that
      credential allocates on its TURN with a permission for its media address's IP), and phones (the edge node's
      `SipNode` row is live, its SIP engine loaded every enabled trunk and none failed, and its copy holds every
      number's fallback), each a pass, fail
      or not-checked line, with round trips on the queue and no outside mail or provider. It runs once per visit;
      Test again runs it again. Continue anyway goes on with a failure; Finish shows the status.
3. Set each chat provider's webhook URL to `<the edge node's address>/channels/<chatBridgeId>/webhook`.

The status screen shows what is set up (host names, mail, chat, PASS, XMPP, calls, phones), with Set up again, which opens the wizard at
Discover. Edge Clients shows what each node's setup sent. A page waits at most 3 s for the edge node's offer, and
says when it is not answering. The offer is read fresh each time; the area names in the edge node's Hello are read at
its start, so restart it after changing its mail, chat, PASS or XMPP settings. The sections below are what the wizard sets,
for an operator who sets a part by hand; a name the operator adds on Edge Clients is not checked against the edge
node's own names.

- **The edge node** runs `conf/MoquiEdgeConf.xml` (`node_role=edge`, web protection on) with the base components and
  mr-pass (`./moq component-get edge`). It needs the Maximal package, or Enterprise with an external database; both
  have the link:

  ```sh
  ./moq run-production --release -c conf/MoquiEdgeConf.xml
  ./moq archive --release --no-data --conf conf/MoquiEdgeConf.xml    # or package it for the host
  ```

  Local nodes dial its HTTPS listener, which tells their connections from browsers' by ALPN, so it needs a WebPKI
  certificate ([Certificates](#certificates)) and no port of its own.
  - **Port.** A local node dials port 443 unless the address has a port, and `webapp_https_listen_port` is 8443 by
    default. Listen on 443, or give the port in the address.
  - **No TLS-terminating proxy in front.** The link needs ALPN and client certificates end to end.
  - **Link key.** Pairing checks the web certificate. The link itself uses the edge node's own link key,
    `edge_link_key`. It is generated into `runtime/.env` on every node at the first start, but only an edge node uses
    it. A renewed web certificate does not touch the link, but a changed `edge_link_key` means every local node must
    pair again.
  - **Edge Clients.** Its operator makes a pairing code in System › Edge Clients, which also lists the paired nodes,
    pings them and revokes them. It shows the edge node's link key fingerprint, to compare with the one the local node
    shows when it pairs.
- **The local node** needs no role and no open port. In System › Edge Node, enter the edge node's address and the code,
  check its name and link key fingerprint, and pair. Pairing writes `edge_node_url`, `edge_node_key`, `edge_node_pin` and `edge_node_id` (the edge node's instance id) to
  `runtime/.env` and brings the link up; unpairing removes them.
- **Web access.** The wizard sets the local node's host names, or add each in System › Edge Clients, such as
  `erp.example.com`, and point its DNS at the edge node. In the office's DNS the same name can point at the local node. A name has a
  mode:
  - **Passthrough**, the default. The edge node reads the name the browser asks for, before any TLS, and sends the
    connection down the link with a PROXY header that names the client. The local node ends the TLS with its own
    certificate, so the edge node sees no content. The local node needs its HTTPS listener and a certificate for the
    name (DNS-01 with an outside tool, or a copied one). While the link is down the connection is refused.
  - **Terminate.** The edge node ends the TLS with its own certificate and forwards each request down the link, with
    the client's address. Its web protection runs first. While the link is down it shows an offline page (503), or the
    page at `edge_offline_page`.

  The apps, `/notws` and the other sockets, DAV and git over HTTPS work in both. The local node's logs and web
  protection see the client's address. If a proxy is in front of the edge node, name it in `webapp_trusted_proxies`;
  on an edge node a client-IP header with no trusted proxies is ignored.
- **Calls.** The edge node is the media node of the local node's calls ([WebRTC](interfaces/WebRTC.md#the-edge-node-as-media-node)):
  `conf/MoquiEdgeConf.xml` turns on its media socket (`rtc_media_port` 10000 on every address) and TURN (`turn_port`
  3478); set `rtc_public_address` in its `.env` and open UDP 10000, UDP and TCP 3478 and the relay range
  (`turn_relay_ports`). The local node needs no media port: it keeps the edge node's media row from the link, sends the
  calls' signaling over it, signs TURN credentials with the edge node's secret (told on the link,
  [TURN](interfaces/TURN.md#on-an-edge-node)) and stores the recordings the edge node queues
  ([Call-Recording](interfaces/Call-Recording.md)).
- **Phones (SIP).** Set `sip_port` (5060) and `sip_tls_port` (5061), `sip_host` 0.0.0.0 and `sip_public_address` in the
  edge node's `.env` (with `sip_registrar_enabled=true` for desk and soft phones outside the office), and open those
  ports and `sip_rtp_ports` (UDP 20000-20999). Set up trunks and numbers on the local node as usual (System, Voice):
  the edge node gets the trunks by replication and registers them, the local node keeps its SIP node's row from the
  link, routes each call there, and stores its voicemail, which the edge node records and queues. Phones' passwords
  stay on the local node, which checks each sign-in. While the local node is away a call is answered from its number's
  fallback: the user's mobile (when they ring it with their calls) or a ring group's outside number, then a message
  that arrives when the link is back; a number with neither gets 480, so set the trunk provider's failover number too.
  The fallbacks are written as the numbers, greetings and boxes change, and every minute while the link is up
  ([SIP](interfaces/SIP.md#behind-an-edge-node), [Voicemail](interfaces/Voicemail.md#while-the-local-node-is-away)).
- **SSH.** Set `edge_ssh_tunnel_port` on the edge node (and `edge_ssh_tunnel_host`, empty for every address). The edge
  node's web protection admits each connection, and it goes down the link to the local node's SSH server, when the
  local node runs one (`ssh_port`). The SSH server's logs and login checks see the client's address. With more than
  one linked node, `edge_ssh_tunnel_node` names the one.
- **XMPP.** Set `edge_xmpp_port` (5222) and `edge_xmpp_tls_port` (5223) on the edge node (and `edge_xmpp_host`), and
  `xmpp_domain` on the local node, not the edge node, which starts no XMPP server. The edge node's web protection
  admits each connection, and it goes down the link to the local node's XMPP server, which ends the TLS with its
  certificates for the domain, so channel binding (SCRAM-SHA-256-PLUS) works. The local node needs no XMPP port of
  its own for this; give any it has the same numbers, which Chat apps shows. `_xmpp-client._tcp` and
  `_xmpps-client._tcp` name the edge node in public DNS. Uploads and `/xmpp-websocket` go through web access, so the
  XMPP domain is one of the node's host names. While the link is down clients cannot connect, and retry
  ([XMPP](interfaces/XMPP.md#behind-an-edge-node)).
- **Work over the link** (the `<edge>` conf element, the same on both nodes; [plan](../plans/Edge-Node.md) §5.2). A
  service listed in `delegate` runs on the local node when called on the edge node, as the user `_EDGE_`, whose grants
  (the `EDGE_DELEGATE` artifact group) must cover it; it fails at once while the link is down. A `queue-kind` names the
  service that applies queued items of that kind; what waits for a local node that is away is kept
  `queue-retention-days` (14, the same on both nodes) up to `queue-max-megabytes`, then the oldest fails, logged; a
  payload is at most 1 MiB, and a larger body (up to 64 MiB, a mail message) crosses as the item's content. A
  `replicate` set is copied to the edge node, read-only there, and sent whole again when
  the edge node lost its copy. On a clustered local node only the member holding the link sends its outbox and
  replication. System › Edge Node shows the outbox and the sets, with Pause and Resync; Edge Clients shows each node's
  queue and sets. The edge node's database now holds the queue: unpause its backup job
  (`backup_EmbeddedDatabase_daily`).
- **PASS** ([plan](../plans/Edge-Node.md) §5.5). With `pass_backend_url=edge:` (set in MoquiEdgeConf.xml), the edge
  node's mr-pass calls its local node as the authority over gRPC down the link; the local node needs no public authority
  and no PASS client secret for it. On the local node, the edge node is the client `pass_edge_client` (default `edge`):
  set `pass_client_edge_sites` (the sites the edge node serves) and `pass_client_edge_callback` (its SSO callback). Set
  `pass_backend_web_url` on the edge node to the local node's web address (a host name with web access through the
  edge node) for the SSO front channel. While the link is down every PASS page fails as unavailable.
- **Chat and SMS** ([plan](../plans/Edge-Node.md) §5.4, [SMS](interfaces/SMS.md)). MoquiEdgeConf.xml sets
  `channels_webhooks_enabled=true`: the edge node checks each webhook's signature with the local node's replicated
  bridge and queues it for the local node. On the local node, `channels_public_url` is the edge node's address, so the
  files of an MMS are served by the edge node.
- **Mail** ([plan](../plans/Edge-Node.md) §5.4). The MTA in moqui is the default (`edge_mail_mta=moqui` in
  MoquiEdgeConf.xml; an empty `edge_mail_mta` keeps E5a's relay to Postfix, so an older edge node is not switched by
  an upgrade): the edge node takes port 25 itself and delivers to each domain's MX. Postfix and Rspamd beside it are
  the alternative, below.
  - **In.** The SMTP listener runs in internet mode (`smtp_internet=true`): set `smtp_listen_port=25`,
    `smtp_listen_host=0.0.0.0` (or `::`) and `smtp_hostname` (the MX name, also the authserv-id of its
    `Authentication-Results`) in `.env`. It offers STARTTLS with the web certificate, limits each address
    (`max-connections-per-ip`, `connections-per-minute`), checks SPF, DKIM, DMARC and ARC after the data, removes any
    `Authentication-Results` claiming its name, refuses a DMARC failure whose policy is reject (550) and files a
    quarantine one in Junk. With `mail_rspamd_url` (Rspamd's HTTP API, `http://127.0.0.1:11333`) Rspamd's verdict
    sets `X-Spam-Flag` and `X-Spam-Status` (any that came are removed); without it there is no spam check. Both fail
    open: Rspamd not reached, or a check that fails, is logged and the mail taken (as not spam); checks that do not
    finish in 45 seconds answer 451. DMARC is RFC 9989's (`t=y` lowers the policy; there is no `pct`), and a passing
    ARC chain sealed by one of `mail_arc_trusted_sealers` (a forwarder or list you trust) overrides a reject or
    quarantine; with none named there is no ARC exception. `RCPT TO`
    is answered from the local node's accounts, replicated to the edge node (`mailRecipients`), so an unknown address
    is refused while the local node is away too, and each message is queued for the local node, which delivers it as
    its own listener would, trusting the edge node's `Authentication-Results`.
  - **Out.** On the local node, give `SYSTEM` and the accounts an `EmailServer` with `smtpHost` `edge:`. A send is
    queued for the edge node, which DKIM-signs it (`mail_dkim_key`, generated into `.env` the first time; selector
    `mail_dkim_selector`, default `moqui`; for `mail_dkim_domains`, else the hosted accounts' domains) and delivers it
    to each recipient domain's MX: STARTTLS when offered, MTA-STS honored (with `mode: enforce`, only the policy's MX
    hosts, with a valid certificate), a 4xx tried again from 5 minutes (`mail_delivery_retry_seconds`) doubling to 4
    hours, for `mail_delivery_days` (5). Set `edge_mail_bounce_domain` (as `bounces.example.com`, with its MX at the
    edge node) and each message goes from `bounce+<item>@` that domain, so a remote server's later bounce names the
    message. A message no recipient got (refused at the MX, by a later bounce, or after `mail_delivery_days`) goes back
    to a mail client's Outbox with the replies as its send error, as does one the local node could not hand to the edge
    node within the queue's retention period; one some recipients got stays Sent, with the refused named in its send
    error. Mail to the local node's own accounts is queued back to it by the edge node, signed and with an
    `Authentication-Results`, not sent round through port 25.
  - **DNS to publish.** For each mail domain: MX at the edge node's name; SPF (`v=spf1 a:edge.example.com -all`, or
    its addresses); the DKIM key at `moqui._domainkey.<domain>` (the TXT value is
    `org.moqui.impl.EmailServices.get#DkimRecord`'s `record`, run on the edge node; it is longer than 255 characters,
    so enter it as several quoted strings of at most 255 each, which DNS joins); DMARC at `_dmarc.<domain>`
    (`v=DMARC1; p=quarantine; rua=mailto:...` to start); the bounce domain's MX at the edge node too. Reverse DNS: the
    edge node's address must resolve to its `smtp_hostname`, and that name back to the address (set it at the cloud
    provider). MTA-STS for the domains themselves (`_mta-sts` and `https://mta-sts.<domain>/.well-known/mta-sts.txt`)
    is optional. DANE is not checked yet.
  - **Postfix and Rspamd beside it** (E5a) do the internet side instead ([Mail-Inbound](interfaces/Mail-Inbound.md)
    has the gateway's rules). In `.env`: `edge_mail_mta=postfix`, `smtp_internet=false`, `smtp_listen_port=10024` and
    `smtp_allowed_addresses=127.0.0.1,::1`, and `protocol="lmtp"` on the `smtp-server` in the runtime's conf. Postfix
    hands mail for the local node's domains to that LMTP listener; set `smtp_authserv_id` on the local node to
    Rspamd's authserv-id, so only its `Authentication-Results` are kept. Mail out is relayed to Postfix on
    `edge_mail_relay_port` (10025), and Postfix signs it with DKIM through Rspamd and delivers it; while Postfix is not
    reached the message waits in the queue.

  ```
  # /etc/postfix/main.cf (the listener answers RCPT TO itself, so no reject_unverified_recipient)
  mynetworks = 127.0.0.0/8 [::1]/128
  relay_domains = example.com, bounces.example.com
  transport_maps = hash:/etc/postfix/transport    # example.com and bounces.example.com  lmtp:inet:127.0.0.1:10024
  smtpd_recipient_restrictions = permit_mynetworks, reject_unauth_destination
  smtpd_milters = inet:127.0.0.1:11332            # Rspamd: SPF, DKIM, DMARC and spam in; DKIM signing out
  non_smtpd_milters = inet:127.0.0.1:11332
  milter_default_action = accept
  # /etc/postfix/master.cf: the relay port the edge node sends to, for this host only
  127.0.0.1:10025 inet n - n - - smtpd
    -o mynetworks=127.0.0.0/8
    -o smtpd_client_restrictions=permit_mynetworks,reject
    -o smtpd_recipient_restrictions=permit_mynetworks,reject
  # /etc/rspamd/local.d/dkim_signing.conf: sign mail from this host for the domains
  selector = "moqui";
  path = "/var/lib/rspamd/dkim/$domain.$selector.key";
  sign_local = true;
  # /etc/rspamd/local.d/milter_headers.conf: Authentication-Results with the edge node's authserv-id
  use = ["authentication-results", "x-spam-status"];
  ```

## An all-in-one node

A full server, with its own users, apps, data, mail domains, chat bridges, calls and XMPP domain, can also be the edge
node of one local node, such as a desktop node ([plan](../plans/Edge-Node.md#10-all-in-one-node-e12) §10). The local
node pairs and runs the wizard as with a dedicated edge node (above), and sees no difference.

- **Conf.** Run `conf/MoquiAllInOneConf.xml` (`node_role=all-in-one`) with the server's full runtime, on the Maximal
  package:

  ```sh
  ./moq run-production --release -c conf/MoquiAllInOneConf.xml
  ```

  It needs what an edge node needs: a WebPKI certificate on the HTTPS listener, `edge_public_addresses`, and the edge
  node's mail, calls and SIP properties in `.env`. It takes the link and never dials an edge node itself.
- **Each item has one owner.** The server's own host names, mail accounts, chat bridges, numbers, calls and XMPP
  domain are served here as on a normal node; the local node's `EdgeHost` names, recipients, bridges, numbers and calls
  go over the link. A mail domain may be split by address.
- **Claims both sides make.** The server's own items win. The wizard's Configure refuses the local node's bridges,
  XMPP domain, SIP domain, numbers and a mail domain the server holds whole (its `edge_mail_bounce_domain`). A
  replicated record that claims a server's item is not applied: System › Edge Clients lists it under Replica conflicts,
  with Check again. The server's own setup refuses an address, bridge or number the local node holds. An address both
  sides hold gets 451 at `RCPT TO`, and a bridge both hold gets 409 at its webhook; each is logged.
- **XMPP.** The server's XMPP server keeps `xmpp_port` and `xmpp_tls_port`. Give the local node's domain tunnel ports
  of their own, such as `edge_xmpp_port=5242` and `edge_xmpp_tls_port=5243`, named in that domain's SRV records. A
  tunnel port the same as the server's own is off, with an error logged.
- **PASS.** The server's mr-pass keeps its local authority (`pass_backend_url` is not `edge:`). PASS sites for the
  local node are not served here yet.
- **Limits.** One paired node. Its load shares the server's CPU, bandwidth and media ports. The queue is in the
  server's database, so the server's backups cover it. Unpairing leaves the server's own services as they were.

## Cluster

Several members share one database and run behind a load balancer. Members find each other in the
`moqui.server.ClusterMember` table, with heartbeats on the database clock; there is no Hazelcast or Redis. Over gRPC on
TLS with client certificates they send each other entity cache clears, notifications, co-editing updates, session ends
and call signaling.
Sessions are rows in the shared database, so any member can serve any request.

```
                   clients
                      | https
               load balancer             webapp_trusted_proxies on each member
           +----------+----------+
       member 1   member 2   member 3    8444 between members only (cluster_base_url)
           +----------+----------+
              shared Postgres            entity_ds_db_conf=postgres
```

### Requirements

- **One external database.** Postgres, or MySQL with the default transport. The Enterprise package has no embedded
  Postgres, so a member cannot start on a database of its own by mistake. The content database shares it unless
  `content_ds_db_conf` names another ([Content database](#content-database)).
- **The same secrets on every member**: `session_cookie_key`, `cluster_tls_cert`, `cluster_tls_key` and
  `entity_ds_crypt_pass`. Startup fails when a live member has a different session key or certificate, with
  "session_cookie_key does not match a live cluster member" or "cluster_tls_cert does not match a live cluster member".
- **A distinct instance id**: each member keeps its own in `runtime/db/instance_id` (or `instance_id`). It is not in
  `.env`, so copying `.env` between members is safe.

### Steps

1. Start the first member once with the shared database settings, so it generates its secrets into `.env`.
2. Copy that `.env` to each other member before its first start.
3. Set `cluster_base_url=https://<this member's private address>:8444` on each member. Empty is
   `https://127.0.0.1:8444`, which logs an error once other members are live.
4. Open port 8444 between the members only.
5. If the load balancer does not keep a client on one member, set guaranteed session writes in the runtime conf:

   ```xml
   <webapp-list><webapp name="webroot"><session-config flush="guarantee"/></webapp></webapp-list>
   ```

6. Set `webapp_trusted_proxies` and `webapp_client_ip_header` for the load balancer, then start the other members.
7. Check the cluster with `SELECT instance_id, cluster_base_url, heartbeat FROM cluster_member;` on the shared database.

### Transports

`cluster_transport` sets how peers reach this member:

| Value | How |
|---|---|
| empty (default) | Its cluster port at `cluster_base_url` (8444): TLS with the cluster certificate, then gRPC or HTTP/1.1 (below) |
| `notify` | Postgres `LISTEN`/`NOTIFY` on the shared database, for members that cannot connect to each other; needs Postgres |
| `ssh` | Through the member's SSH server (`ssh_port`); needs the `ssh` feature |

On the cluster port TLS picks the protocol by ALPN. `cluster_protocol` sets what a member speaks there, as listener and
as sender:

| Value | How |
|---|---|
| empty (default), `grpc` | gRPC on HTTP/2 (`moqui.cluster.v1.Cluster`): each peer gets what was waiting for it as one call. A peer that does not choose HTTP/2 is posted to over HTTP/1.1 |
| `rest` | HTTP/1.1 only: `POST /cluster/{topic}`, one message each |

Members on different settings, or of different builds, still reach each other: the HTTP/1.1 POST is always there.

### What is shared and what is not

- **Shared**: scheduled jobs take a lock row, so each job runs on one member at a time. Content blobs, `db` git
  repositories, sessions and the LLM quotas are in the database.
- **Not shared**:
  - `local` and `remote` git repositories are on one member's disk. Use `db` repositories, or send `/git` and SSH to
    that member.
  - Files on disk are each member's own: the SFTP root (`runtime://sftp`), and any setting that names a directory,
    such as `pass_media_root`. Share the directory between the members.
  - The LLM gateway's requests-per-minute and the web protection's limits are counted on each member separately.

**Not built**: shared bans across members; a screen listing the members.

## PASS servers with a BOSS server or cluster

IronStone PASS (`mr-pass`) serves public sites: Forum, Account, Members and Sites. A PASS server can run without Mantle
or BOSS. It then calls an **authority** for every read and write: a BOSS server, or cluster, holding Mantle and the
business database. The public machines keep only sessions and their own conf.

```
 visitors --https--> PASS server forum.example.com --+
                     base components and mr-pass     |  https: /rest/s1/pass/v1/sites/*
                     its own small database          +------> authority boss.example.com
 visitors --https--> PASS server shop.example.com ---+        mr-boss and mr-mantle (mr-pass for local sites)
                                                               business database, jobs, mail, media
                                                               one server or a cluster
```

In local mode, mr-pass runs in the same runtime as Mantle and makes in-process calls; that is the default when
`pass_backend_url` is not set. Provisioning, mail, media, workers and imports are in mr-pass
[docs/Deployment.md](../runtime/component/mr-pass/docs/Deployment.md). This section covers the remote layout.

The remote calls go over REST, which every package has and which is Lite's default, or over gRPC where the package has
it (Maximal, Enterprise): direct to the authority's gRPC listener, or from an
[edge node](#a-local-server-with-an-edge-node) to its local node over the edge link (`pass_backend_url=edge:`).
`pass_backend_transport` chooses (`rest`, `grpc`, or empty for automatic); the settings are in Deployment.md. With an
edge node as the PASS server, the local node's Edge Node wizard writes the client properties for it.

### The authority

1. Run a BOSS server: `./moq component-get mr-boss`, which brings mr-mantle. It can be one server or a
   [cluster](#cluster). Use Maximal or Enterprise: forum thumbnails need `image`, and sign-in providers need `sso`.
   The site readiness check reports what is missing. The PASS jobs are in Mantle's data (`ForumSetupData.xml`) and run
   here. Add mr-pass only if this server also serves sites itself.
2. Register each PASS deployment with these properties, kept with the other secrets in `.env` or the environment:

   | Property | Value |
   |---|---|
   | `pass_client_<id>_secret` | The deployment's secret |
   | `pass_client_<id>_sites` | The site IDs it may serve, comma-separated |
   | `pass_client_<id>_callback` | The exact SSO callback, such as `https://forum.example.com/account/Sso` |

   For gRPC clients, set `pass_authority_grpc_enabled=true`: the gRPC listener (`pass_authority_grpc_port`, 4317) then
   takes PASS calls, with the client's ID and secret in metadata and TLS from the HTTPS listener's certificate. With no
   certificate in `cert/` the listener falls back to plain h2c, and then takes PASS calls only on a loopback address
   (an error is logged otherwise), unless `pass_authority_grpc_insecure=true` says to send the secrets in clear. The
   HTTPS listener takes the same calls over [HTTP/2](#http2), so a PASS server's `pass_backend_grpc_url` can name the
   HTTPS port (`grpcs://boss.example.com:443`) and no other port needs to be open.
3. Create the sites and bind their hosts and mounts with `mantle.public.SiteServices.create#Site` and `store#Mounts`
   (Deployment.md, Provisioning).
4. When web protection is on (`webapp_trusted_proxies` set, or `webapp_web_protection=true`), exempt the PASS servers,
   or all their visitors count as one client:

   ```xml
   <webapp-list><webapp name="webroot"><web-protection>
       <client-rule name="pass-servers" action="exempt" cidr="10.0.2.0/24"/>
   </web-protection></webapp></webapp-list>
   ```

### A PASS server

1. Make a checkout that has no Mantle. mr-pass declares no `depends-on`, and a conf cannot remove a component that is
   in `runtime/component`:

   ```sh
   git clone https://github.com/moqui/mr-moqui.git pass && cd pass
   ./moq component-get mr-pass
   ```

2. Build it with any server package. PASS calls the authority for images and sign-in, so Lite, the smallest, is
   enough: `./moq archive --lite --no-data`.
3. Configure it in `.env` and `MoquiConf.xml`. Set `pass_backend_url` to the authority's final https origin, so that
   no call depends on a redirect.

   ```sh
   pass_backend_url=https://boss.example.com
   pass_client_id=forum
   pass_client_secret=...
   ```

   ```xml
   <webapp-list><webapp name="webroot">
       <public-site host="forum.example.com" site-id="FORUM_SITE" screen-location="component://mr-pass/screen/Pass.xml"
               resolver-service="pass.RouteServices.resolve#Route">
           <mount path="/forum" screen="Forum"/>
           <mount path="/account" screen="Account"/>
           <mount path="/members" screen="Members"/>
           <mount path="/sites" screen="Sites"/>
       </public-site>
   </webapp></webapp-list>
   ```

   The hosts and mounts here must match the ones bound at the authority.
4. Embedded Postgres is enough. The PASS server stores framework tables and sessions, and the sessions hold each
   visitor's delegation from the authority.
5. Put the certificate for the public host in `cert/`. Behind a proxy, set `webapp_trusted_proxies` and
   `webapp_client_ip_header`. Each call to the authority carries the visitor's address, and the authority's sign-in rate
   limits count by it.
6. Check:
   - the site answers at its host;
   - logged in as an operator on the authority, `https://boss.example.com/rest/s1/pass/v1/sites/<siteId>/readiness`
     reports the media, mail, SSO, host and mount checks;
   - Deployment.md, Reproducing the remote check, runs the two-runtime test.

### Scaling

- **More PASS servers**: the authority takes any number of client IDs. A client ID has one SSO callback, so give each
  hostname that offers SSO its own client ID.
- **More PASS nodes for one hostname**: each PASS server can have its own database, or several nodes of one server can
  share a Postgres as their own [cluster](#cluster). That cluster is separate from the authority's.
- **The authority as a cluster**: delegations, SSO handoffs and rate-limit counts are rows, so any member can check
  them. The second factor and the SSO front channel use the authority's web session, so set `flush="guarantee"` or keep
  a client on one member. `pass_media_root` and `pass_import_dir` must name storage that every member can read.
- **Latency**: each public page makes at least two calls to the authority, and nothing is cached. Keep the PASS servers
  close to it.

**Not built**:
- a readiness screen: the check is the REST route and `check#Readiness` only, and it does not look at the PASS server's
  own conf;
- a cache of public pages;
- realtime push to a remote PASS server (the stream polls);
- more than one authority for one deployment, which is by design;
- the Commerce, Blog and other PASS apps;
- SAML sign-in.

## Dev instances with a dev group server

Each developer runs a dev instance on their own machine. The team shares a **group server**, a production instance
with IronStone BOSS. It hosts the components' repositories, the Projects work items, and the LLM gateway with the
team's model keys. Developers who do not run a server, often business users who extend screens and entities, work on a
**shared dev-aide server**. That is a separate dev instance that uses the group server as a laptop does.

```
 laptop      ./moq run, dev, 127.0.0.1 ---+   git over https or ssh, /llm, /rest/s1/mantle/my/tasks
 laptop      ./moq run, dev, 127.0.0.1 ---+--------> group server team.example.com, production purpose
 shared dev-aide server, dev, https ------+          mr-boss, mr-mantle: Projects, git hosting, LLM gateway
   dev_aide_shared=true                              one server or a cluster
```

The group server runs production purpose because it holds the team's real data. A dev instance loads Dev AIDE, which
gives its users code execution as the server's OS user, and the dev conf turns off the tarpits and allows every
origin.

### The group server

1. Run BOSS: `./moq component-get mr-boss` (which brings mr-mantle), on Maximal, or on Enterprise as a
   [cluster](#cluster). Set it up as in [One production server](#one-production-server).
2. **Git hosting** ([Git-Hosting](interfaces/Git-Hosting.md)):
   - HTTPS clones work as soon as HTTPS does: `https://team.example.com/git/<repositoryId>.git`.
   - For SSH, set `ssh_port` (such as 2222, since 22 is usually the system's sshd), `ssh_host=0.0.0.0`, and
     `ssh_public_port` for the clone URLs shown.
   - Grant `SSH_GIT` to the developers' group. Seed data grants it to ADMIN only.
   - Create repositories from a project's Repositories tab, or on the Git Repositories screen (System, Resource
     Finder). On a cluster, make them `db` repositories.
   - Commit trailers such as `Task: PROJ-12` and branch names link commits to Projects work items through Mantle's
     gitECA rules.
3. **LLM gateway** ([Gateway](ai/Gateway.md)):
   - set `llm_gateway_enabled=true` and the aliases developers call;
   - grant `LlmGatewayUse` to the developers' group;
   - for each developer's key, add an `LlmApiClient` row with the `gateway` scope (System › LLM › Gateway). Its
     `allowedAliases`, `dailyTokens` and `dailyCost` limit that key.
4. Optionally, turn on MCP (`mcp_enabled=true`) for coding agents. `/mcp/setup` gives a client config to paste
   ([MCP](ai/MCP.md)).

### Each dev instance

1. **Credential.** On the group server, make an access token: Projects › Code › Git access, or Commons › Settings ›
   Account. It is shown once. It is a login key, and it serves as:
   - the password for git over HTTPS;
   - the dev-aide task source's token;
   - the gateway key, once it has an `LlmApiClient` row.

   For git over SSH, add your public key on the same Git access screen instead.
2. **Repositories.** Put the team's repository and components in your `myaddons.xml`, which is not in git:

   ```xml
   <addons>
       <repository name="team">
           <location type="git" url="https://team.example.com/git/${name}.git"/>
       </repository>
       <component name="acme-site" repository="team"/>
   </addons>
   ```

   Then run `./moq component-get acme-site`. The repository ids on the group server must be the component names. Keep
   the token in a git credential helper (`git config --global credential.helper store`, or the OS keychain). With
   `default-repository="team"` on `<addons>`, every component comes from the group server.
3. **Models.** Point the default model endpoint at the gateway, in `runtime/.env` or with `-D`. The URL has no `/v1`,
   since the endpoint adds `/v1/chat/completions`.

   ```sh
   llm_openai_url=https://team.example.com/llm
   llm_openai_api_key=<access token>
   llm_openai_model=<alias>
   ```

4. **Tasks.** In Dev AIDE (`/qapps/dev`) › Tasks, add a source of kind `moqui-rest` with the base URL
   `https://team.example.com` and the token. It lists your work items from `/rest/s1/mantle/my/tasks`. A task app with
   the group server's base URL (Dev AIDE › Settings) opens them there. Commits from the Workspace carry the current
   task's trailer.
5. **Binding.** Set `webapp_http_bind=127.0.0.1`. Dev AIDE gives its users code execution, so a laptop's server should
   not be reachable from the network.

### The shared dev-aide server

1. Run a dev instance (`./moq run`, or `MOQUI_CONF=conf/MoquiDevConf.xml`) with the components the users extend, and
   set `dev_aide_shared=true`.
2. Give it a certificate, leave `webapp_http_bind` empty, and do not publish 8080. Set `webapp_allow_origins` to the
   server's own origin, such as `https://dev.example.com`, in place of the dev conf's `*`. An empty value would not
   replace it.
3. Put its users in `DEV_AIDE_EXTENDERS`, who may write site components and publish them, or `DEV_AIDE_USERS`, who may
   do everything.
4. Network git (fetch, pull, push, clone) is off on a shared server until an admin turns it on in Dev AIDE › Settings,
   with a deploy token or per-user tokens from the group server.
5. Commits carry the logged-in user as author. A save checks the file on disk by its hash, so a save over another
   user's change offers Compare instead of overwriting it.

### What is not shared

A shared dev database is not supported. Every serving process joins the cluster of its database, so dev instances
with their own generated secrets refuse to start on one. If they did start, one developer's entity changes would alter
everyone's schema, and any instance could run the team's jobs. Each dev instance keeps its own embedded Postgres. To
hand a database to someone else, send a `./moq save-db` save and the matching `entity_ds_crypt_pass`.

**Not built**:
- Moqui is not an identity provider, so a dev instance cannot log in through the group server. Point both at one
  external OIDC provider, or keep separate accounts.
- Pushing a hosted repository to another server; a `remote` repository is a read-only mirror.
- OAuth or OIDC tokens for MCP and A2A; they take login keys and app passwords.

## Components

Components live in `runtime/component`, and dev-only ones in `runtime/dev-component`. Each is its own git repository.
`addons.xml` lists the known components and repositories. A `myaddons.xml` beside it, which is not in git, adds or
overrides `repository`, `component` and `component-set` elements. It can set `default="a,b"` (what `component-get` gets
with no names) and `default-repository` on its root.

| Task | What it does |
|---|---|
| `./moq component-get <name...>` | Get components and the components they depend on (`depends-on` in `component.xml`); a `component-set` name gets the components of the set (`apps`: mr-marble and mr-boss) |
| `./moq component-depends` | Get missing dependencies of what is in `runtime/component` |
| `./moq component-create <name>` | Make a component from the component template, `runtime/dev-component/template`: an entity, services, a REST API, screens, data and specs; `--rest` or `--screens` for one part |
| `./moq component-zip [name...]` | Zip components |
| `./moq git <args>`, `git-status`, `git-pull` | Run git in the root repository and every component repository |

The get tasks take `-t git|current|release|binary` and `-r <repository>`. `--dev` puts the named components in
`runtime/dev-component`. A component with a `Cargo.toml` is linked into the binary on the next build
([Native-Components](dev/Native-Components.md)). A new component loads at the next start. System › Components can also
load it on a running server, with its data, when it has no Rust code
([Component-Deploy](tools/Component-Deploy.md#loading-a-component)).

## Moving from Moqui Java

| Moqui Java | MR Moqui |
|---|---|
| `java -jar moqui.war` | `moqui`, or `./moq run` from a checkout |
| `java -jar moqui.war load types=...` | `moqui load types=...`, or `./moq load ...` |
| `./gradlew addRuntime`, moqui-plus-runtime.war | `./moq archive`: a directory with the binary, not a WAR |
| `MoquiInit.properties`, `-Dmoqui.runtime`, `-Dmoqui.conf` | `MOQUI_RUNTIME`, `MOQUI_CONF`, or `--conf` |
| H2 by default | Embedded Postgres by default; `entity_ds_db_conf=h2` also means embedded |
| JDBC driver jars in `runtime/lib` | Built in: Postgres in every package, MySQL in Maximal and Enterprise |
| Tomcat, Elastic Beanstalk with a WAR | One process: the binary serves HTTP and HTTPS itself |
| Docker Compose with nginx-proxy | Podman stacks, and no proxy container ([podman/README.md](../podman/README.md)) |
| OpenSearch or ElasticSearch for Mantle and the apps | Optional; search is on Postgres by default. `no-run-es` still works |
| Hazelcast for a cluster | The shared database and HTTPS between members |
| `JAVA_TOOL_OPTIONS` heap sizing | No JVM, so no heap to size |

Component and data changes for a Java deployment are in [Moqui-Java-Migration.md](dev/Moqui-Java-Migration.md).

## Directory layout

```
mr-moqui/
  moq                 the task script (bash 3.2 or newer)
  addons.xml          known components and repositories; myaddons.xml (not in git) adds to it
  crates/             the Rust workspace: moqui (the binary), moqui-host, moqui-core, moqui-lib, moqui-llm, moqui-db,
                      moqui-tui, moqui-browser
  runtime/            the runtime directory (runtime/README.md)
    base-component/   framework, tools, webroot
    component/        components from ./moq component-get (not in git)
    dev-component/    components that load only on a dev instance: dev-aide, example, demo (off until a conf names
                      it); template for component-create
    conf/             MoquiDevConf.xml, MoquiProductionConf.xml, MoquiDemoConf.xml, MoquiEdgeConf.xml
    cert/ db/ log/ .env   per-instance files (not in git)
  podman/             the image, compose stacks and Quadlet units
  systemd/            the unit, sysusers file and drop-ins
  scripts/            pg-payload.sh, check-packages.sh, wasm-build.sh
  docs/               reference guides
  plans/              designs, status and reviews
  target/             build output: target/release/moqui, target/lite/moqui, ...
```
