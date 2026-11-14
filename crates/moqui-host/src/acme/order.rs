/*
 * This software is in the public domain under CC0 1.0 Universal plus a
 * Grant of Patent License.
 *
 * To the extent possible under law, the author(s) have dedicated all
 * copyright and related and neighboring rights to this software to the
 * public domain worldwide. This software is distributed without any
 * warranty.
 *
 * You should have received a copy of the CC0 Public Domain Dedication
 * along with this software (see the LICENSE.md file). If not, see
 * <http://creativecommons.org/publicdomain/zero/1.0/>.
 */

//! An order for one name set up here (RFC 8555, on `instant-acme`): the account, the order, its TLS-ALPN-01 challenge,
//! the finalize with this node's own key and request, and the certificate stored and written. One member of a cluster
//! orders a name at a time, by its lock row; in this process one order per name runs at a time.
//!
//! The account's key is P-256, derived from `acme_account_key` (a secret in `runtime/.env`, generated at the first
//! start): the same key is the same account at each authority, created there at the first order (`newAccount` returns
//! the existing one after that). The certificate's own key is a new P-256 key for each order.
//!
//! The network parts run on the edge runtime ([`crate::edge::block_on`]); the rows are written between them on the
//! calling thread, so no blocking database call runs on a runtime thread.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use instant_acme::{
    Account, AuthorizationStatus, ChallengeType, Identifier, NewOrder, Order, OrderStatus, RetryPolicy,
};
use moqui_core::context::ExecutionContextFactory;
use moqui_lib::error::Error;
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer, PrivatePkcs8KeyDer};
use sha2::{Digest, Sha256};

use super::store::{self, Issued, NameRow, Outcome};
use super::Acme;

/// The job that renews the names set up here (framework/data/MoquiSetupData.xml).
pub(crate) const RENEWAL_JOB: &str = "renew_AcmeCertificates_daily";
/// How long a challenge stays open: the authority validates within seconds of being told it is ready.
const CHALLENGE_OPEN: Duration = Duration::from_secs(10 * 60);
/// A name that failed is not tried again by the job for this long.
const RETRY_AFTER_MILLIS: i64 = 6 * 60 * 60 * 1000;
/// With no issue date, a certificate is renewed this long before it runs out.
const RENEW_BEFORE_MILLIS: i64 = 30 * 24 * 60 * 60 * 1000;
const DAY_MILLIS: i64 = 24 * 60 * 60 * 1000;
/// The most the account and the order may take at the authority, and then its validation and the certificate: well
/// under the order lock's lease (15 minutes).
const START_LIMIT: Duration = Duration::from_secs(60);
const FINISH_LIMIT: Duration = Duration::from_secs(180);

/// The PKCS#8 head of a P-256 private key with no public key (RFC 5208, RFC 5915): the 32-byte scalar follows.
const P256_PKCS8_HEAD: [u8; 35] = [
    0x30, 0x41, 0x02, 0x01, 0x00, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
    0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x04, 0x27, 0x30, 0x25, 0x02, 0x01, 0x01, 0x04, 0x20,
];

/// The account key from `seed` (the base64 of `acme_account_key`): a P-256 key whose scalar is a SHA-256 of the seed,
/// hashed again in the rare case it is not a valid scalar. The same seed is always the same key.
pub(crate) fn account_key(seed: &str) -> Result<(instant_acme::Key, PrivateKeyDer<'static>), Error> {
    let seed = STANDARD
        .decode(seed.trim())
        .map_err(|_| Error::new("acme_account_key is not base64"))?;
    if seed.len() < 16 {
        return Err(Error::new(
            "acme_account_key is too short: it is the base64 of 32 random bytes",
        ));
    }
    let mut scalar: [u8; 32] = Sha256::new()
        .chain_update(b"moqui acme account key")
        .chain_update(&seed)
        .finalize()
        .into();
    for _ in 0..16 {
        let mut der = P256_PKCS8_HEAD.to_vec();
        der.extend_from_slice(&scalar);
        if let Ok(key) = instant_acme::Key::from_pkcs8_der(PrivatePkcs8KeyDer::from(der.as_slice())) {
            return Ok((key, PrivateKeyDer::Pkcs8(PrivatePkcs8KeyDer::from(der))));
        }
        scalar = Sha256::digest(scalar).into();
    }
    Err(Error::new("acme_account_key gives no P-256 key"))
}

/// Order `host`'s certificate now, on this thread: the name must be set up. Its row says the outcome, and a failure
/// is the error too.
pub(crate) fn run(factory: &Arc<ExecutionContextFactory>, host: &str) -> Result<(), Error> {
    let acme = Acme::of(factory);
    let Some(_ordering) = acme.begin_order(host) else {
        return Err(Error::new(format!("{host} is being ordered already")));
    };
    let Some(name) = store::name(factory, host)? else {
        return Err(Error::new(format!("{host} is not set up with ACME")));
    };
    let instance = moqui_core::drain::instance_id(factory);
    if !store::claim_lock(factory, host, &instance)? {
        return Err(Error::new(format!("another member of this cluster is ordering {host}")));
    }
    store::record(factory, host, &Outcome::Started)?;
    acme.want();
    let result = issue(factory, &name);
    let recorded = match &result {
        Ok(issued) => {
            tracing::info!(
                "ACME: {host} has a certificate from {} until {}",
                issued.issuer,
                jiff::Timestamp::from_millisecond(issued.not_after).map_or_else(|_| String::new(), |at| at.to_string())
            );
            store::record(
                factory,
                host,
                &Outcome::Issued {
                    issued: issued.issued,
                    not_after: issued.not_after,
                },
            )
        }
        Err(err) => {
            tracing::error!("ACME: ordering {host}'s certificate failed: {err}");
            store::record(factory, host, &Outcome::Failed(err.message()))
        }
    };
    if let Err(err) = store::release_lock(factory, host, &instance) {
        tracing::error!("ACME: the order lock of {host} could not be released: {err}");
    }
    recorded?;
    result.map(|_| ())
}

/// [`run`] on a thread of its own, for the screen: its row shows the outcome.
pub(crate) fn start(factory: &Arc<ExecutionContextFactory>, host: &str) -> Result<(), Error> {
    let factory = Arc::clone(factory);
    let host = host.to_string();
    std::thread::Builder::new()
        .name("moqui-acme".to_string())
        .spawn(move || {
            // a failed order is logged and recorded in run; this says why one did not start
            if let Err(err) = run(&factory, &host) {
                tracing::info!("ACME: the order of {host} ended: {err}");
            }
        })
        .map(|_| ())
        .map_err(|err| Error::new(format!("could not start the order: {err}")))
}

fn seed() -> Result<String, Error> {
    let seed = moqui_lib::env::get_prop_or_env("acme_account_key");
    if seed.trim().is_empty() || seed.ends_with("CHANGEME") {
        return Err(Error::new(
            "acme_account_key is not set; it is generated into runtime/.env at start",
        ));
    }
    Ok(seed)
}

/// `acme_directory_ca`: the root (a PEM file, absolute or under the runtime) an authority's HTTPS is checked against
/// in place of the operating system's, for a company's own authority; empty for the system's roots.
fn directory_ca(factory: &ExecutionContextFactory) -> Option<std::path::PathBuf> {
    let file = moqui_lib::env::get_prop_or_env("acme_directory_ca");
    let file = file.trim();
    if file.is_empty() {
        return None;
    }
    let path = std::path::Path::new(file);
    Some(if path.is_absolute() {
        path.to_path_buf()
    } else {
        factory.runtime_dir().join(path)
    })
}

/// The account's HTTPS client: the system's roots, or the one root `ca` names.
fn builder(ca: Option<&std::path::Path>) -> Result<instant_acme::AccountBuilder, Error> {
    match ca {
        None => Account::builder().map_err(acme_error("the HTTPS client")),
        Some(ca) => Account::builder_with_root(ca).map_err(acme_error(&format!("acme_directory_ca {}", ca.display()))),
    }
}

fn acme_error(what: &str) -> impl Fn(instant_acme::Error) -> Error + '_ {
    move |err| Error::new(format!("{what}: {err}"))
}

/// The steps of one order.
fn issue(factory: &Arc<ExecutionContextFactory>, name: &NameRow) -> Result<Issued, Error> {
    let host = name.host.clone();
    let account_key = account_key(&seed()?)?;
    let directory = name.directory_url.clone();
    let contact = name.contact_email.trim().to_string();
    let ca = directory_ca(factory);
    // the account, the order and its challenges' digests
    let (account, order, digests) = network("the account and the order", START_LIMIT, {
        let host = host.clone();
        async move {
            let (account, _) = builder(ca.as_deref())?
                .create_from_key(account_key, directory.clone())
                .await
                .map_err(acme_error(&format!("the account at {directory}")))?;
            if !contact.is_empty() {
                let mailto = format!("mailto:{contact}");
                if let Err(err) = account.update_contacts(&[mailto.as_str()]).await {
                    tracing::warn!("ACME: the account's contact could not be set to {contact}: {err}");
                }
            }
            let identifiers = [Identifier::Dns(host.clone())];
            let mut order = account
                .new_order(&NewOrder::new(&identifiers))
                .await
                .map_err(acme_error("the order"))?;
            let mut digests = Vec::new();
            let mut authorizations = order.authorizations();
            while let Some(authorization) = authorizations.next().await {
                let mut authorization = authorization.map_err(acme_error("the authorization"))?;
                match authorization.status {
                    AuthorizationStatus::Pending => {}
                    AuthorizationStatus::Valid => continue,
                    other => return Err(Error::new(format!("the authorization is {other:?}"))),
                }
                let challenge = authorization.challenge(ChallengeType::TlsAlpn01).ok_or_else(|| {
                    Error::new("the authority offers no tls-alpn-01 challenge, the only kind this node answers")
                })?;
                let mut digest = [0_u8; 32];
                digest.copy_from_slice(challenge.key_authorization().digest().as_ref());
                digests.push(digest);
            }
            Ok::<_, Error>((account, order, digests))
        }
    })?;
    for digest in &digests {
        store::put_challenge(factory, &host, digest, CHALLENGE_OPEN)?;
    }
    // every member reads the challenge rows again before the authority looks
    std::thread::sleep(super::SEEN_BY_EVERY_MEMBER);
    let finished = finish(account, order, &host);
    for digest in &digests {
        if let Err(err) = store::close_challenge(factory, &host, digest) {
            tracing::error!("ACME: the challenge of {host} could not be closed: {err}");
        }
    }
    let (chain_pem, key_pem) = finished?;
    let leaf = CertificateDer::pem_slice_iter(chain_pem.as_bytes())
        .next()
        .and_then(Result::ok)
        .ok_or_else(|| Error::new("the authority sent no certificate"))?;
    let not_after = moqui_lib::certificate::not_after(&leaf)
        .ok_or_else(|| Error::new("the certificate's expiry could not be read"))?
        .saturating_mul(1000);
    let issued = Issued {
        chain_pem,
        key_pem,
        issuer: moqui_lib::certificate::issuer(&leaf).unwrap_or_default(),
        directory_url: name.directory_url.clone(),
        issued: moqui_core::user::now_millis(),
        not_after,
    };
    store::save_certificate(factory, &host, &issued)?;
    Ok(issued)
}

/// Tell the authority the challenges are ready, wait for the order, then finalize it with a new key: the chain and
/// the key, PEM.
fn finish(account: Account, mut order: Order, host: &str) -> Result<(String, String), Error> {
    let host = host.to_string();
    network("the validation and the certificate", FINISH_LIMIT, async move {
        let _account = account;
        let mut authorizations = order.authorizations();
        while let Some(authorization) = authorizations.next().await {
            let mut authorization = authorization.map_err(acme_error("the authorization"))?;
            if authorization.status != AuthorizationStatus::Pending {
                continue;
            }
            if let Some(mut challenge) = authorization.challenge(ChallengeType::TlsAlpn01) {
                challenge.set_ready().await.map_err(acme_error("the challenge"))?;
            }
        }
        let waiting = RetryPolicy::new()
            .initial_delay(Duration::from_millis(500))
            .backoff(2.0)
            .timeout(Duration::from_secs(120));
        let status = order.poll_ready(&waiting).await.map_err(acme_error("the validation"))?;
        if status != OrderStatus::Ready {
            return Err(Error::new(why_invalid(&mut order, &host).await));
        }
        let key = rcgen::KeyPair::generate().map_err(|err| Error::new(format!("the certificate's key: {err}")))?;
        let request = rcgen::CertificateParams::new(vec![host.clone()])
            .and_then(|params| params.serialize_request(&key))
            .map_err(|err| Error::new(format!("the certificate request: {err}")))?;
        order
            .finalize_csr(request.der())
            .await
            .map_err(acme_error("the finalize"))?;
        let chain = order
            .poll_certificate(&waiting)
            .await
            .map_err(acme_error("the certificate"))?;
        Ok((chain, crate::tls::pem("PRIVATE KEY", &key.serialize_der())))
    })
}

/// Run `future` on the edge runtime for at most `limit`: the authority's HTTPS client has no timeout of its own.
fn network<T: Send + 'static>(
    what: &str,
    limit: Duration,
    future: impl std::future::Future<Output = Result<T, Error>> + Send + 'static,
) -> Result<T, Error> {
    crate::edge::block_on(async move { tokio::time::timeout(limit, future).await })?.unwrap_or_else(|_| {
        Err(Error::new(format!(
            "{what}: no answer from the authority in {} seconds",
            limit.as_secs()
        )))
    })
}

/// What the authority said about an invalid order: its challenge's error, as far as it says.
async fn why_invalid(order: &mut Order, host: &str) -> String {
    let mut authorizations = order.authorizations();
    while let Some(Ok(authorization)) = authorizations.next().await {
        for challenge in &authorization.challenges {
            if let Some(error) = &challenge.error {
                return format!("the authority could not validate {host}: {error}");
            }
        }
    }
    format!("the authority could not validate {host}")
}

/// Whether a name's order is running now: its row says so and a member holds its order lock (`locked`). One whose lock
/// lease ran out was cut off by a stop, and counts as failed.
pub(crate) fn ordering_now(name: &NameRow, locked: &HashSet<String>) -> bool {
    name.status == store::ORDERING && locked.contains(&name.host)
}

/// Whether a name's last order failed, or was cut off.
pub(crate) fn failed_now(name: &NameRow, locked: &HashSet<String>) -> bool {
    name.status == store::FAILED || (name.status == store::ORDERING && !locked.contains(&name.host))
}

/// The names the job renews now, given each one's row: those renewing whose certificate is missing or near its end (a
/// third of its life left, or [`RENEW_BEFORE_MILLIS`] with no issue date, at least a day), not ordering, and not
/// failed in the last [`RETRY_AFTER_MILLIS`].
pub(crate) fn due(names: &[NameRow], locked: &HashSet<String>, now: i64) -> Vec<String> {
    names
        .iter()
        .filter(|name| name.renewing && !ordering_now(name, locked))
        .filter(|name| !(failed_now(name, locked) && name.last_attempt.is_some_and(|at| now - at < RETRY_AFTER_MILLIS)))
        .filter(|name| match name.not_after {
            None => true,
            Some(not_after) => {
                let before = match name.issued {
                    Some(issued) if issued < not_after => ((not_after - issued) / 3).max(DAY_MILLIS),
                    _ => RENEW_BEFORE_MILLIS,
                };
                not_after - now <= before
            }
        })
        .map(|name| name.host.clone())
        .collect()
}

/// The job: renew each name [`due`] now, one after another; one that fails is logged and the rest go on. The renewed
/// and failed counts.
pub(crate) fn renew_due(factory: &Arc<ExecutionContextFactory>) -> Result<(usize, usize), Error> {
    let names = store::names(factory)?;
    let locked = store::live_locks(factory)?;
    let (mut renewed, mut failed) = (0, 0);
    for host in due(&names, &locked, moqui_core::user::now_millis()) {
        match run(factory, &host) {
            Ok(()) => renewed += 1,
            Err(err) => {
                failed += 1;
                tracing::error!("ACME renewal of {host} failed: {err}");
            }
        }
    }
    Ok((renewed, failed))
}
