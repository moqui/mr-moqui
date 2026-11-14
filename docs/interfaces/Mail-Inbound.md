# Inbound mail for hosted and shared accounts (the SMTP listener)

A **hosted** account (`EmailAccount` kind `EakHosted`, an address at a domain the business hosts) and a **shared** account
(`EakShared`, such as orders@ or support@, with members) get their mail from the SMTP listener: the last hop behind an
**edge gateway** that does the internet side. The gateway (Postfix with Rspamd, or another MTA) takes the mail from the
world with TLS, checks SPF, DKIM and DMARC, filters spam and viruses, queues and retries, and hands each message to
mr-moqui over SMTP or LMTP, on a private link or with STARTTLS. The listener stores it in the account's Inbox as the IMAP
sync stores fetched mail and runs what new mail runs. External accounts (Gmail, Microsoft 365, any IMAP or POP server)
keep using the sync. As of Oct 21, 2026.

The code is `crates/moqui-host/src/smtp.rs` (the server) and `crates/moqui-mail/src/delivery.rs` (routing and
delivery). It is the moqui-host feature `inbound-smtp`, in the Debug, Maximal (`--release`) and Enterprise packages
([Packaging](../Build-Test.md#packages)); the Dev and Lite packages leave it out, and add it with:

```
./moq build -- --features inbound-smtp
```

It adds no crate: the server is a small RFC 5321 (and RFC 2033 LMTP) server on Tokio, for the one client it serves, with
rustls for STARTTLS.

## What the deployment configures

The listener starts with the server when `smtp_listen_port` is set. The settings are the attributes of the
`smtp-server` element in `MoquiDefaultConf.xml`; the deployment ones are also properties (`smtp_listen_port` and so on,
in the environment or `runtime/.env`), and the others are changed in a runtime or component conf, such as
`<smtp-server max-message-size="52428800"/>`.

| Property or attribute | Default | |
| --- | --- | --- |
| `smtp_listen_port` | empty (off) | The port the gateway delivers to, such as `2525`. Use a port above 1024: the gateway, not mr-moqui, has port 25. |
| `smtp_listen_host` | `127.0.0.1` | The address to listen on: loopback for a gateway on the same host, the WireGuard or private address for a remote one. |
| `smtp_allowed_addresses` | empty | The gateway's addresses, IPs and CIDRs separated by commas (`127.0.0.1`, `10.8.0.1`, `fd00::/8`). Every other address gets `554` at connect. **Empty does not start the listener**, so it never takes mail from everyone. |
| `smtp-server` `proxy-protocol` | `false` | Each connection starts with a PROXY v1 or v2 header (a TCP proxy such as HAProxy in front of mr-moqui). The address the header names must be in `smtp_allowed_addresses` too, so list both the proxy's and the gateway's addresses. |
| `smtp_hostname` | `webapp_http_host` | The name in the greeting and in the `Received` field. |
| `smtp_authserv_id` | empty | The authserv-id the gateway writes in its `Authentication-Results` (Rspamd: the `authserv_id` of its milter headers, usually the gateway's host name). Only those fields are kept on the message; any other `Authentication-Results` (such as one a sender forged) is dropped. Empty keeps them all as they came, and a warning says so at startup. |
| `smtp-server` `spam-headers` | `X-Spam-Flag,X-Spam-Status,X-Spam` | The gateway's spam verdict fields. A message where one of them starts with `yes` or `true` (`X-Spam-Flag: YES`, `X-Spam-Status: Yes, score=7.2`) goes to the account's Junk folder. |
| `smtp-server` `plus-separator` | `+` | The plus-addressing characters: `sales+news@example.com` goes to `sales@example.com`. Several characters are each a separator (`+-`); `none` turns it off. |
| `smtp-server` `max-message-size` | `26214400` (25 MiB) | Bytes, advertised as `SIZE`. A larger message is refused with `552` (at `MAIL FROM` when its `SIZE` says so, else after its data). A message is held in memory while it is stored. |
| `smtp-server` `max-recipients` | `100` | Recipients of one message; the next gets `452` and the gateway sends to it later. |
| `smtp-server` `max-connections` | `20` | Connections at once; the next gets `421`. |
| `smtp-server` `command-timeout` | `300` | Seconds for each command line and each chunk of data. |
| `smtp-server` `session-timeout` | `1800` | Seconds for a whole connection. |
| `smtp-server` `protocol` | `smtp` | `lmtp` for LMTP (RFC 2033): the gateway says `LHLO`, and after the data the listener answers once for each recipient, so a store that fails for one account is retried for that recipient only (see below). |
| `smtp-server` `starttls` | `false` | Offer STARTTLS (RFC 3207). The certificate is the `smtp_tls_cert_file` pair, else the one in `runtime/cert` for the name the gateway asks for (SNI), else for `smtp_hostname`, else `default` (the layouts of the HTTPS listener: `<domain>.crt` and `.key`, certbot's `<domain>/fullchain.pem` and `privkey.pem`, mkcert's; renewals picked up without a restart). **With no certificate the listener does not start.** |
| `smtp-server` `require-tls` | `false` | Refuse `MAIL` before STARTTLS with `530 5.7.0`, so mail never crosses the link in the clear. Needs `starttls`. |
| `smtp_tls_cert_file`, `smtp_tls_key_file` | empty | The certificate (with its chain) and private key PEM files, absolute or under the runtime directory (`runtime://` works too); both or neither. Read again when they change. |
| `smtp-server` `chunking` | `true` | CHUNKING (RFC 3030): `BDAT n [LAST]`, the chunk taken as it is (no dot-stuffing). |
| `smtp-server` `dsn` | `false` | DSN (RFC 3461): `RET` and `ENVID` on `MAIL`, `NOTIFY` and `ORCPT` on `RCPT`, checked (`501` when they do not parse) and kept. See [DSN](#dsn). |

The accounts are made as usual (`create#EmailAccount` with `accountKindEnumId` `EakHosted` or `EakShared`, the BOSS
MailAccounts wizard). An account gets the listener's mail when it is hosted or shared, not removed, and has **no incoming
server** (no `storeHost` on its `EmailServer`): a shared account the sync feeds from an IMAP mailbox is left to the sync,
or it would get each message twice. Its outgoing server, if it has one, is how its replies and automatic replies go out,
usually back through the gateway.

## DNS and the gateway

- **MX records point at the gateway**, never at mr-moqui: `example.com. MX 10 mx.example.com.` with `mx.example.com` the
  gateway's public address. SPF, DKIM (the gateway signs outbound mail) and DMARC records are the gateway's.
- The gateway relays the hosted domains to mr-moqui. Postfix, with mr-moqui on the same host:

  ```
  relay_domains = example.com
  transport_maps = hash:/etc/postfix/transport      # example.com  smtp:[127.0.0.1]:2525
  smtpd_recipient_restrictions = permit_mynetworks, reject_unauth_destination, reject_unverified_recipient
  ```

  `reject_unverified_recipient` asks mr-moqui (`RCPT TO`) before it accepts mail from the internet, so mail to an address
  no account has is refused at the edge and nothing bounces later (no backscatter). The listener answers `550 5.1.1` for
  such an address.
- **The gateway must remove** `Authentication-Results` fields with its own authserv-id that came from outside (RFC 8601
  section 5; the `remove` setting of Rspamd's `authentication-results` milter header), and the spam verdict fields that came
  from upstream. The listener trusts what the gateway writes and verifies nothing again.
- A remote gateway (a small VPS for a site with port 25 blocked or no fixed address) forwards over WireGuard or another
  private link: set `smtp_listen_host` to mr-moqui's address on that link and `smtp_allowed_addresses` to the gateway's.
  Over a link that is not private, turn on `starttls` and `require-tls` and have the gateway require TLS (below).

### LMTP

With SMTP, a message to two accounts gets one answer: when the store fails for one of them the listener answers `451` and
the gateway sends the message to both again (the account that has it sees the same Message-ID and does not store it
twice, but a message with no Message-ID would be). LMTP answers each recipient: `250` for an account that stored it (or
had it), `451` for one whose store failed, which the gateway retries for that recipient only, and `550 5.1.1` for an
account removed since `RCPT TO`. A whole-message answer (too large `552`, not mail `554`, no worker free `451`) is given
once for each recipient.

```
# conf: <smtp-server protocol="lmtp"/>
# main.cf
transport_maps = hash:/etc/postfix/transport        # example.com  lmtp:inet:127.0.0.1:2525
```

Postfix's `lmtp` client uses its own `lmtp_` parameters (`lmtp_tls_security_level` and so on). `reject_unverified_recipient`
works the same, through the transport.

### TLS between the gateway and mr-moqui

```
# conf: <smtp-server starttls="true" require-tls="true"/>, with smtp_tls_cert_file and smtp_tls_key_file, or
# moqui.example.com.crt and .key in runtime/cert

# main.cf, LMTP: the lmtp_ parameters only touch this link
lmtp_tls_security_level = secure
lmtp_tls_secure_cert_match = moqui.example.com
lmtp_tls_CAfile = /etc/postfix/moqui-ca.pem       # only for a certificate no public CA signed

# main.cf, SMTP: smtp_tls_security_level is also the level for the internet, so raise it for mr-moqui only
smtp_tls_policy_maps = hash:/etc/postfix/tls_policy
# /etc/postfix/tls_policy:   [10.8.0.2]:2525   secure match=moqui.example.com
```

`encrypt` instead of `secure` requires TLS without checking the certificate (a self-signed one with no CA file).
STARTTLS that cannot finish is never skipped silently: with `require-tls` the listener refuses mail until it is done,
and with no certificate it does not start. Bytes the gateway pipelines after `STARTTLS` (before the handshake) end the
session with `554`, and after the handshake the session starts over (`EHLO` or `LHLO` again, STARTTLS no longer offered).

## What happens to a message

At `RCPT TO` the address is looked up without regard to case: an account's address, then one of its `EmailIdentity`
addresses, then the same without the plus tag. An address two accounts claim at the same step goes to the account made
first (the lowest id), with a warning naming both, so an identity a user adds later cannot read another account's mail.

After `DATA` the message is stored once in each account it is for, however many of the account's addresses were
recipients:

1. Trace fields go on top: `Return-Path` (the envelope sender; any the message had is removed), a `Delivered-To` for each
   recipient address of the account, and `Received`. Only the gateway's `Authentication-Results` stay.
2. The `EmailMessage` is stored in the Inbox (Junk for the gateway's spam), unread, with its files as DbResource files
   under `dbresource://email/<account>/<message>/` and its place in a thread, in a transaction of its own. An app's
   `EmailMessage` ECA files it on the records by the account's filing policy (Mantle's `file#ReceivedEmail`: every party
   match for hosted and shared accounts; mail in Junk is not filed).
3. For Inbox mail: the email ECAs (`*.emecas.xml`), the account's rules (`run#EmailRules`), the snooze wake
   (`wake#SnoozedThreads`) and the automatic reply (`send#OutOfOfficeReplies`). Then the counts of the account's users.
   Each of these logs its own failure, and the message stays stored.

The listener answers `250` when every account has it. When a store fails (the database is down, the account has no Inbox
folder) or no request worker is free, it answers `451` and the gateway tries again later; an account that stored the
message the first time sees the same Message-ID and does not store it again. A message with no header fields is refused
with `554`. In LMTP each recipient gets its own answer ([LMTP](#lmtp)).

The `Received` field names the protocol (RFC 3848, RFC 6531): `ESMTP`, `ESMTPS` after STARTTLS, `LMTP`, `LMTPS`, and
`UTF8SMTP`, `UTF8LMTPS` and so on for `SMTPUTF8` mail.

### DSN

With `dsn="true"` the listener advertises `DSN` and takes the parameters the gateway passes on from the sender: `RET`
(`FULL` or `HDRS`) and `ENVID` (xtext) on `MAIL`, `NOTIFY` (`NEVER`, or `SUCCESS`, `FAILURE`, `DELAY`) and `ORCPT`
(`rfc822;` and the xtext address) on `RCPT`. Bad syntax is `501`. A recipient with an `ORCPT` gets an
`Original-Recipient` field on its copy. The recipients whose `NOTIFY` has `SUCCESS` and whose account stored the message
get one "delivered" report (`multipart/report`, RFC 3464, with the message's header) to the envelope sender, from the
null reverse path, through the outgoing server of the first such account (usually the gateway); an account with no
outgoing server, or a send that fails, is logged and the mail stays delivered. The gateway's resend after a `451` tells
nothing again. Failures need no DSN from mr-moqui: it refuses at the time and the gateway reports them. With `dsn` off
the gateway handles `NOTIFY=SUCCESS` itself (Postfix sends a "relayed" notice).

Each session is logged when it ends (the client and its EHLO name, the messages, recipients accepted and refused, how it
ended), and each message with its sender, recipients, size, the accounts and message ids it was stored as, the spam
verdict and the authentication results in short (`spf=pass dkim=pass dmarc=pass`). Message bodies are never logged.

## The edge node as the gateway

A local server behind a firewall gets its mail through its edge node ([Run-Deploy](../Run-Deploy.md#a-local-server-with-an-edge-node),
[plan](../../plans/archive/Edge-Node.md) §5.4). The local node then runs no listener of its own for the internet: the edge node
takes the mail and queues each message for it over the link, and the local node delivers it as this listener would.
`RCPT TO` is answered on the edge node from the local node's accounts, replicated to it, so an unknown address is refused
while the local node is away too.

- **The MTA in moqui** (E5b, the default on an edge node: `edge_mail_mta=moqui` in
  [`conf/edge/MoquiEdgeConf.xml`](../../runtime/conf/edge/MoquiEdgeConf.xml)). The edge node takes port 25 itself and
  delivers to each domain's MX. An empty `edge_mail_mta` keeps E5a's relay to Postfix, so an older edge node is not
  switched by an upgrade.
- **Postfix and Rspamd beside the edge node** (E5a). The gateway above runs on the edge node's host and hands mail to
  the edge node's listener by LMTP on loopback (`edge_mail_mta=postfix`, `protocol="lmtp"`). Postfix signs the local
  node's mail out with DKIM through Rspamd and delivers it ([below](#postfix-and-rspamd-beside-the-edge-node)).

The local node's System › Edge Node wizard sets the mail side up: the `edge:` server for `SYSTEM` and the chosen
accounts, and the DNS records (MX, SPF, the DKIM key and DMARC), each checked live. Discover leaves the system's mail
off when `SYSTEM` has a server of its own. Apply keeps the host or server each mail row had (`edgePriorSmtpHost`,
`edgePriorServerId`), and unpairing puts it back. Test checks mail in (the edge node takes mail for a hosted
account's address) and mail out (the servers are on `edge:`, and the edge node can send: its DKIM key, or its Postfix
relay answering).

### Mail in

The SMTP listener runs in internet mode (`smtp_internet=true`): set `smtp_listen_port=25`, `smtp_listen_host=0.0.0.0`
(or `::`) and `smtp_hostname` (the MX name, also the authserv-id of its `Authentication-Results`) in the edge node's
`.env`. It offers STARTTLS with the web certificate, limits each address (`max-connections-per-ip`,
`connections-per-minute`), checks SPF, DKIM, DMARC and ARC after the data, removes any `Authentication-Results`
claiming its name, refuses a DMARC failure whose policy is reject (550) and files a quarantine one in Junk.

- **Spam.** With `mail_rspamd_url` (Rspamd's HTTP API, `http://127.0.0.1:11333`) Rspamd's verdict sets `X-Spam-Flag`
  and `X-Spam-Status` (any that came are removed); without it there is no spam check. Both fail open: Rspamd not
  reached, or a check that fails, is logged and the mail taken (as not spam); checks that do not finish in 45 seconds
  answer 451.
- **DMARC** is RFC 9989's (`t=y` lowers the policy; there is no `pct`), and a passing ARC chain sealed by one of
  `mail_arc_trusted_sealers` (a forwarder or list you trust) overrides a reject or quarantine; with none named there is
  no ARC exception.
- **Recipients.** `RCPT TO` is answered from the local node's accounts, replicated to the edge node
  (`mailRecipients`), and each message is queued for the local node, which delivers it as its own listener would,
  trusting the edge node's `Authentication-Results`.
- **An edge cluster** ([Edge-Node2](../../plans/Edge-Node2.md) §4.6, E18). Every member listens, with an MX record each
  (or one name with an address per member). The copies of the accounts are shared rows; each member builds its
  recipients from them, again when a batch is applied on any member (the one that applied it tells the others on the
  cluster topic `edge-state`). A message taken by any member is one queue item, delivered once to the local node; with
  one member stopped, the others take the mail, and mail out goes on, as any member claims a delivery row.

### Mail out

On the local node, give `SYSTEM` and the accounts an `EmailServer` with `smtpHost` `edge:`. A send is queued for the
edge node, which DKIM-signs it (`mail_dkim_key`, generated into `.env` the first time; selector `mail_dkim_selector`,
default `moqui`; for `mail_dkim_domains`, else the hosted accounts' domains) and delivers it to each recipient domain's
MX: STARTTLS when offered, MTA-STS honored (with `mode: enforce`, only the policy's MX hosts, with a valid
certificate), a 4xx tried again from 5 minutes (`mail_delivery_retry_seconds`) doubling to 4 hours, for
`mail_delivery_days` (5).

- **Bounces.** Set `edge_mail_bounce_domain` (as `bounces.example.com`, with its MX at the edge node) and each message
  goes from `bounce+<item>@` that domain, so a remote server's later bounce names the message.
- **Failures.** A message no recipient got (refused at the MX, by a later bounce, or after `mail_delivery_days`) goes
  back to a mail client's Outbox with the replies as its send error, as does one the local node could not hand to the
  edge node within the queue's retention period; one some recipients got stays Sent, with the refused named in its
  send error. Mail for an `edge:` server while the local node is not paired is not queued, but fails as retryable and
  stays in the Outbox.
- **Local recipients.** Mail to the local node's own accounts is queued back to it by the edge node, signed and with an
  `Authentication-Results`, not sent round through port 25.

### DNS to publish

For each mail domain: MX at the edge node's name; SPF (`v=spf1 a:edge.example.com -all`, or its addresses); the DKIM
key at `moqui._domainkey.<domain>` (the TXT value is `org.moqui.mail.EmailServices.get#DkimRecord`'s `record`, run on
the edge node; it is longer than 255 characters, so enter it as several quoted strings of at most 255 each, which DNS
joins); DMARC at `_dmarc.<domain>` (`v=DMARC1; p=quarantine; rua=mailto:...` to start); the bounce domain's MX at the
edge node too. Reverse DNS: the edge node's address must resolve to its `smtp_hostname`, and that name back to the
address (set it at the cloud provider). MTA-STS for the domains themselves (`_mta-sts` and
`https://mta-sts.<domain>/.well-known/mta-sts.txt`) is optional. DANE is not checked yet.

### Postfix and Rspamd beside the edge node

They do the internet side instead, with the gateway's rules above. In the edge node's `.env`: `edge_mail_mta=postfix`,
`smtp_internet=false`, `smtp_listen_port=10024` and `smtp_allowed_addresses=127.0.0.1,::1`, and `protocol="lmtp"` on
the `smtp-server` in the runtime's conf. Postfix hands mail for the local node's domains to that LMTP listener; set
`smtp_authserv_id` on the local node to Rspamd's authserv-id, so only its `Authentication-Results` are kept. Mail out is
relayed to Postfix on `edge_mail_relay_port` (10025), and Postfix signs it with DKIM through Rspamd and delivers it;
while Postfix is not reached the message waits in the queue.

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

### On an all-in-one node

On an all-in-one node ([Run-Deploy](../Run-Deploy.md#an-all-in-one-node), a full server that is also the edge node,
[`conf/all-in-one/MoquiAllInOneConf.xml`](../../runtime/conf/all-in-one/MoquiAllInOneConf.xml)) the listener answers
`RCPT TO` from the server's own accounts first, then the local node's replicated ones, and a message with recipients
on both sides is delivered once to each. An address both sides hold gets 451, logged.

### Several local nodes on one edge node

With several local nodes on one edge node ([Run-Deploy](../Run-Deploy.md#several-local-nodes-on-one-edge-node),
[`conf/multi-domain/`](../../runtime/conf/multi-domain/README.md)) each has its own mail domains, and the MX of each
is the edge node. `RCPT TO` goes to the local node whose replicated accounts have the address, the most specific match
first (`joe+x@` over `joe@`); a domain may be split between local nodes by address. An address two local nodes both
have gets 451, logged, until one lets it go, and a replicated account that claims an address another local node holds
is kept as a replica conflict (System › Edge Clients › Domains). Mail out is DKIM-signed with the edge node's one key,
published in each domain, and only for the sending node's own domains. Delivery gives each local node with mail due an
even share of the sessions, and the queue's size cap holds for each local node alone.

## What it does not do

No `AUTH` (the allow-list is the gateway's identity), no client certificates, no relaying: it takes mail only for its own
accounts and only from the gateway. No `ETRN`, no `BINARYMIME`, and no failure or delay DSNs of its own. Out of internet mode no SPF,
DKIM or DMARC checks, no spam filter of its own, no queue: those are the gateway's. A `postmaster` mailbox is an account like any
other (the gateway answers for its own). `VRFY` answers `252`.
