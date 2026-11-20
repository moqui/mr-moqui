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

//! A credential's source (plans/Vault-Instances.md §4.7, step 1 and the check of step 3): where a rotation sets a new
//! value, and signs in with it to check it.
//!
//! - **A database user's password** on its database dependency, set by the leased admin login: on a database instance
//!   from a one-shot client container on its network (as `create#Database` is made, [`crate::database`]), on a database
//!   host over verified TLS. Postgres takes a SCRAM verifier (`ALTER ROLE ... PASSWORD`), so the password is in no
//!   statement or server log; MySQL `ALTER USER ... IDENTIFIED BY`. The check signs in as the user with the new value.
//! - **A database instance's admin password**, set signed in as the admin with the value it replaces, from a one-shot on
//!   its network; the check signs in as the admin with the new one.
//! - **An OpenSearch instance's admin password**, set through its security API (`PUT
//!   _plugins/_security/api/account`, the current and the new password) by a one-shot of its own image on its network,
//!   signed in as admin with the value it replaces (a netrc file, never the command); the check reads `authinfo` with
//!   the new one. The image's demo configuration serves a self-signed certificate, so the one-shot does not verify it:
//!   the call stays on the environment's own network, which is the limit Run-Deploy states.
//!
//! Putting the old value back is the same set, from the new value to the old one.

use moqui_lib::error::Error as LibError;
use serde_json::json;
use zeroize::Zeroizing;

use crate::common::fail;
use crate::database::{self, AdminLogin, OneShot, Server, Whose};
use crate::plan::{DatabaseUse, Reach};
use crate::provision::{leased_admin, leased_login, run_one_shot};

/// Where a credential's value is set and checked.
#[derive(Debug, Clone)]
pub enum Source {
    /// A database user's own password, on its database dependency, by its admin login (a lease's item).
    DatabaseUser {
        database: Box<DatabaseUse>,
        admin_binding: String,
    },
    /// A database instance's admin password, signed in as the admin.
    DatabaseAdmin(Network),
    /// An OpenSearch instance's admin password, through its security API.
    SearchAdmin(Network),
}

/// A server on an environment's network, reached by a one-shot of its own image on its host.
#[derive(Debug, Clone)]
pub struct Network {
    /// The `InstanceHost` row of its host.
    pub host: moqui_core::entity::EntityValue,
    pub network: String,
    pub image: String,
    /// Its container's name on the network.
    pub server: String,
    pub port: String,
    /// Its kind's database name (postgres, mysql); empty for OpenSearch.
    pub conf_name: String,
    /// The instance the one-shot is for (its name's prefix).
    pub instance_id: String,
}

/// The login a rotated item holds: its user name (none for an OpenSearch admin, whose user is `admin`) with `password`.
fn login(user: &str, password: &str) -> AdminLogin {
    AdminLogin {
        user: user.to_string(),
        password: Zeroizing::new(password.to_string()),
    }
}

/// A one-shot that must end 0: its `moqui:` lines, or an error naming what failed.
fn shot(network: &Network, shot: OneShot, what: &str) -> Result<Vec<String>, LibError> {
    let (exit, lines) = run_one_shot(
        &network.host,
        &network.network,
        &network.image,
        &network.instance_id,
        shot,
    )?;
    if exit != 0 {
        return Err(fail(format!(
            "{what} on {} failed (exit {exit}): {}",
            network.server,
            lines.join("; ")
        )));
    }
    Ok(lines)
}

/// Sets the credential's value at its source from `from` (the value it holds now, which an admin signs in with) to
/// `to`. `user` is the item's user name. The report's lines.
pub fn set(source: &Source, user: &str, from: &str, to: &str) -> Result<Vec<String>, LibError> {
    match source {
        Source::DatabaseUser {
            database,
            admin_binding,
        } => {
            let admin = leased_admin(database, admin_binding)?;
            match &database.reach {
                Reach::Network {
                    host,
                    network,
                    image,
                    server,
                    port,
                } => shot(
                    &Network {
                        host: host.clone(),
                        network: network.clone(),
                        image: image.clone(),
                        server: server.clone(),
                        port: port.clone(),
                        conf_name: database.conf_name.clone(),
                        instance_id: server.clone(),
                    },
                    database::one_shot_set_password(
                        Server {
                            conf_name: &database.conf_name,
                            host: server,
                            port,
                        },
                        &admin,
                        &database.user,
                        to,
                        Whose::User,
                    )?,
                    &format!("Setting the password of {}", database.user),
                ),
                Reach::Direct { host, port } => {
                    database::set_password_direct(
                        &database::Direct {
                            conf_name: &database.conf_name,
                            host,
                            port,
                            admin: &admin,
                        },
                        &database.user,
                        to,
                        Whose::User,
                    )?;
                    Ok(vec![format!(
                        "set the password of {} on {}",
                        database.user, database.on
                    )])
                }
            }
        }
        Source::DatabaseAdmin(network) => shot(
            network,
            database::one_shot_set_password(server_of(network), &login(user, from), user, to, Whose::Admin)?,
            &format!("Setting the password of {user}"),
        ),
        Source::SearchAdmin(network) => shot(
            network,
            search_account(network, from, to)?,
            "Changing the admin password",
        ),
    }
}

/// Signs in at the source with `value`, the credential's new value: Ok when the source takes it.
pub fn check(source: &Source, user: &str, value: &str) -> Result<Vec<String>, LibError> {
    match source {
        Source::DatabaseUser { database, .. } => match &database.reach {
            Reach::Network {
                host,
                network,
                image,
                server,
                port,
            } => shot(
                &Network {
                    host: host.clone(),
                    network: network.clone(),
                    image: image.clone(),
                    server: server.clone(),
                    port: port.clone(),
                    conf_name: database.conf_name.clone(),
                    instance_id: server.clone(),
                },
                database::one_shot_login(
                    Server {
                        conf_name: &database.conf_name,
                        host: server,
                        port,
                    },
                    &login(&database.user, value),
                    Some(&database.database),
                )?,
                &format!("Signing in as {}", database.user),
            ),
            Reach::Direct { host, port } => {
                database::login_direct(
                    &database.conf_name,
                    host,
                    port,
                    &login(&database.user, value),
                    &database.database,
                )?;
                Ok(vec![format!("{} signed in on {}", database.user, database.on)])
            }
        },
        Source::DatabaseAdmin(network) => shot(
            network,
            database::one_shot_login(server_of(network), &login(user, value), None)?,
            &format!("Signing in as {user}"),
        ),
        Source::SearchAdmin(network) => shot(network, search_check(network, value)?, "Signing in as admin"),
    }
}

/// Whether the old value can go back where the new one goes, checked before anything changes: a MySQL password with
/// a quote or a backslash, or an OpenSearch one a netrc file cannot hold, would not go back (and is changed by hand).
pub fn check_put_back(source: &Source, user: &str, old: &str) -> Result<(), LibError> {
    let refused = |err: LibError| {
        fail(format!(
            "the value it holds now could not be put back if the rotation failed, so nothing was changed: {} (change \
             it by hand)",
            err.message()
        ))
    };
    match source {
        Source::DatabaseUser { database, .. } => {
            database::password_statements(&database.conf_name, &database.user, old, Whose::User).map_err(refused)?;
        }
        Source::DatabaseAdmin(network) => {
            database::password_statements(&network.conf_name, user, old, Whose::Admin).map_err(refused)?;
        }
        Source::SearchAdmin(_) => {
            netrc_password(old).map_err(refused)?;
        }
    }
    Ok(())
}

/// The admin login of a database instance's own item, refused unless it names that server (§4.3: a login goes only
/// where its item says).
pub fn own_admin(network: &Network, binding: &str) -> Result<AdminLogin, LibError> {
    leased_login(binding, &network.server, &network.port)
}

fn server_of(network: &Network) -> Server<'_> {
    Server {
        conf_name: &network.conf_name,
        host: &network.server,
        port: &network.port,
    }
}

/// A password a netrc file holds: printable, with no space, quote or backslash (a value the manager made always is).
fn netrc_password(password: &str) -> Result<&str, LibError> {
    if password.is_empty()
        || password
            .chars()
            .any(|ch| ch.is_whitespace() || ch.is_control() || "'\"\\".contains(ch))
    {
        return Err(fail(
            "The OpenSearch admin password holds a space, a quote or a backslash, which its one-shot does not take: \
             change it by hand",
        ));
    }
    Ok(password)
}

fn netrc(network: &Network, password: &str) -> Result<(String, Zeroizing<Vec<u8>>), LibError> {
    let text = Zeroizing::new(format!(
        "machine {} login admin password {}\n",
        network.server,
        netrc_password(password)?
    ));
    Ok(("netrc".to_string(), Zeroizing::new(text.as_bytes().to_vec())))
}

/// The shell line that calls the search server's API with the netrc file: `code` holds the HTTP status.
fn curl(network: &Network, method: &str, path: &str, body: bool) -> Result<String, LibError> {
    let server = database::plain_word(&network.server, "server")?;
    let port = network
        .port
        .parse::<u16>()
        .map_err(|_| fail(format!("{} is not a port", network.port)))?;
    let data = if body {
        " -H 'Content-Type: application/json' --data-binary @/run/secrets/account.json"
    } else {
        ""
    };
    Ok(format!(
        "code=$(curl -s -o /dev/null -w '%{{http_code}}' --insecure --netrc-file /run/secrets/netrc -X {method}{data} \
         https://{server}:{port}{path}) || {{ echo 'moqui: the search server took no connection'; exit 4; }}\n"
    ))
}

/// The one-shot that changes the admin password of an OpenSearch instance from `from` to `to`.
fn search_account(network: &Network, from: &str, to: &str) -> Result<OneShot, LibError> {
    let body = Zeroizing::new(json!({ "current_password": from, "password": to }).to_string());
    let script = format!(
        "set -u\n{}\
         if [ \"$code\" = 401 ]; then echo 'moqui: the admin login was refused'; exit 5; fi\n\
         if [ \"$code\" = 403 ]; then echo 'moqui: the security plugin refused the change (403): a reserved admin is \
         changed with its own tools (securityadmin), by hand'; exit 6; fi\n\
         if [ \"$code\" != 200 ]; then echo \"moqui: changing the admin password failed ($code)\"; exit 6; fi\n\
         echo 'moqui: set the password of admin'\n",
        curl(network, "PUT", "/_plugins/_security/api/account", true)?
    );
    Ok(database::one_shot(
        script,
        vec![
            netrc(network, from)?,
            ("account.json".to_string(), Zeroizing::new(body.as_bytes().to_vec())),
        ],
    ))
}

/// The one-shot that signs in to an OpenSearch instance as admin with `value`.
fn search_check(network: &Network, value: &str) -> Result<OneShot, LibError> {
    let script = format!(
        "set -u\n{}\
         if [ \"$code\" != 200 ]; then echo \"moqui: the admin login was refused ($code)\"; exit 5; fi\n\
         echo 'moqui: admin signed in'\n",
        curl(network, "GET", "/_plugins/_security/authinfo", false)?
    );
    Ok(database::one_shot(script, vec![netrc(network, value)?]))
}
