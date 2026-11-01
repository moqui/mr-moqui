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

use super::*;

/// RFC 7677 section 3's example: user, pencil, its salt, 4096 iterations, its nonces, and the final messages.
#[test]
fn the_rfc_7677_example_signs_in() {
    let salt = STANDARD.decode("W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
    let verifier = derive("pencil", &salt, 4096);
    let credentials = Credentials {
        salt,
        iterations: 4096,
        verifiers: vec![derive("other", b"x", 4096), verifier],
    };
    let first = client_first(b"n,,n=user,r=rOprNGfwEbeRWgbNEkqO", false, false).unwrap();
    let (mut exchange, _) = ServerExchange::start(first, Some(credentials), None, b"k");
    // the RFC's server nonce, so the proof is the RFC's
    exchange.nonce = "rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0".to_string();
    exchange.server_first =
        "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096".to_string();
    let (index, last) = exchange
        .finish(b"c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ=")
        .unwrap();
    assert_eq!(index, 1, "the second password matched");
    assert_eq!(last, b"v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=");
}

#[test]
fn plus_binds_to_the_exporter_and_a_downgrade_is_refused() {
    let salt = b"0123456789abcdef".to_vec();
    let credentials = Credentials {
        salt: salt.clone(),
        iterations: 4096,
        verifiers: vec![derive("pw", &salt, 4096)],
    };
    let exporter = vec![7_u8; 32];
    let (mut client, first) = ClientExchange::new("jane", "pw", Some(exporter.clone()));
    let parsed = client_first(&first, true, true).unwrap();
    let (server, server_first) = ServerExchange::start(parsed, Some(credentials.clone()), Some(exporter), b"k");
    let last = client.respond(&server_first).unwrap();
    let (_, server_final) = server.finish(&last).unwrap();
    assert!(client.verify(&server_final));

    // the same client against another TLS session's exporter: refused
    let (mut client, first) = ClientExchange::new("jane", "pw", Some(vec![7_u8; 32]));
    let parsed = client_first(&first, true, true).unwrap();
    let (server, server_first) = ServerExchange::start(parsed, Some(credentials.clone()), Some(vec![8_u8; 32]), b"k");
    let last = client.respond(&server_first).unwrap();
    assert_eq!(server.finish(&last).err(), Some(Refused::Binding));

    // y where the server binds is a downgrade; n is fine for the plain mechanism; p for the plain one is refused
    assert_eq!(
        client_first(b"y,,n=jane,r=abc", false, true).err(),
        Some(Refused::Binding)
    );
    assert!(client_first(b"y,,n=jane,r=abc", false, false).is_ok());
    assert!(client_first(b"n,,n=jane,r=abc", false, true).is_ok());
    assert_eq!(
        client_first(b"p=tls-exporter,,n=jane,r=abc", false, true).err(),
        Some(Refused::Binding)
    );
    assert_eq!(
        client_first(b"p=tls-unique,,n=jane,r=abc", true, true).err(),
        Some(Refused::Binding)
    );
    assert_eq!(
        client_first(b"n,,n=jane,r=abc,m=x", false, true).err(),
        Some(Refused::Malformed)
    );

    // a wrong password, and a user the server does not know, fail at the end alike
    let (mut client, first) = ClientExchange::new("jane", "wrong", None);
    let (server, server_first) = ServerExchange::start(
        client_first(&first, false, true).unwrap(),
        Some(credentials),
        None,
        b"k",
    );
    let last = client.respond(&server_first).unwrap();
    assert_eq!(server.finish(&last).err(), Some(Refused::NotAuthorized));
    let (mut client, first) = ClientExchange::new("nobody", "pw", None);
    let (server, server_first) = ServerExchange::start(client_first(&first, false, true).unwrap(), None, None, b"k");
    let (again, _) = ServerExchange::start(client_first(&first, false, true).unwrap(), None, None, b"k");
    assert_eq!(
        server.server_first.split(",s=").nth(1),
        again.server_first.split(",s=").nth(1),
        "a stable decoy salt"
    );
    let last = client.respond(&server_first).unwrap();
    assert_eq!(server.finish(&last).err(), Some(Refused::NotAuthorized));
}
