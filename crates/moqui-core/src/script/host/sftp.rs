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

//! `SftpClient` for scripts: the Groovy `new org.moqui.sftp.SftpClient(host, username)` is `new SftpClient(host,
//! username)`, a global like `EdiHandler`. The builder calls (`password`, `publicKeyPkcs8`, `preserveAttributes`,
//! `connect`) and `put`/`rm` return the client, so calls chain as in Java. Bytes are `Uint8Array`s. `openStream` and
//! `putStream` return small stream objects, since a script has no Java streams. The Java-object overloads
//! (`publicKey(PrivateKey, PublicKey)`, `publicKey(KeyProvider...)`, `getInternalClient`) have no script form and throw.
//! A client the script did not close is closed when it is collected, with an error in the log, as Java's `finalize`.
//! The constructor is a shim over a hidden `make`, whose object is an [`SFTP_CLIENT`].

use std::sync::{Arc, Mutex};

use moqui_lib::object::{HostObject, Value};

use super::{lock_state, method, with_ec, Call, HostClass, HostError, Ret, Shim};
use crate::sftp::{SftpClient, SftpKind};

const SFTP_CLIENT_JS: &str = r#"(function (global, hidden) {
    function SftpClient(host, username, port) {
        this.client = hidden.make(host == null ? "" : String(host), username == null ? "" : String(username),
            port == null ? null : Number(port));
    }
    ["password", "publicKeyPkcs8", "preserveAttributes", "hostKeyFingerprint", "connect"].forEach(function (name) {
        SftpClient.prototype[name] = function () { this.client[name].apply(this.client, arguments); return this; };
    });
    ["lsFiles", "ls", "getBinary", "getText", "moveFile", "renameFile", "rename", "close", "isConnected"]
        .forEach(function (name) {
            SftpClient.prototype[name] = function () { return this.client[name].apply(this.client, arguments); };
        });
    SftpClient.prototype.put = function (path, filename, data, charset, createDir) {
        if (data != null && typeof data === "object" && !(data instanceof Uint8Array) && !(data instanceof ArrayBuffer)
                && typeof data.getLocation === "function") data = { resourceLocation: String(data.getLocation()) };
        this.client.put(path, filename, data, charset == null ? null : String(charset), !!createDir);
        return this;
    };
    SftpClient.prototype.rm = function (path) { this.client.rm(path); return this; };
    SftpClient.prototype.openStream = function (path) {
        var client = this.client, handle = client.openStream(path);
        return {
            read: function (max) { return client.readChunk(handle, max == null ? 65536 : max); },
            readAllBytes: function () { return client.readAll(handle); },
            readText: function (charset) { return client.readAllText(handle, charset == null ? null : String(charset)); },
            close: function () { client.closeStream(handle); }
        };
    };
    SftpClient.prototype.putStream = function (path) {
        var client = this.client, handle = client.putStream(path);
        return {
            write: function (data, charset) { client.writeChunk(handle, data, charset == null ? null : String(charset)); },
            close: function () { client.closeStream(handle); }
        };
    };
    var noScriptForm = function (name) {
        return function () {
            throw new Error("SftpClient." + name + " takes Java key objects, which a script does not have; use publicKeyPkcs8(privateKeyPem, publicKey)");
        };
    };
    SftpClient.prototype.publicKey = noScriptForm("publicKey");
    SftpClient.prototype.getInternalClient = noScriptForm("getInternalClient");
    SftpClient.SFTP_PORT = 115;
    SftpClient.SSH_PORT = 22;
    global.SftpClient = SftpClient;
})"#;

pub(super) static SHIM: Shim = Shim {
    js: SFTP_CLIENT_JS,
    hidden: &[&[method!("SftpClient", "make", [Text, Text, OptValue], make)]],
};

/// The client a constructed `SftpClient` wraps.
pub(super) static SFTP_CLIENT: HostClass = HostClass {
    name: "host:SftpClient",
    methods: &[
        method!("SftpClient", "password", [OptValue], password),
        method!("SftpClient", "publicKeyPkcs8", [Value, OptValue], public_key_pkcs8),
        method!("SftpClient", "preserveAttributes", [Bool], preserve_attributes),
        method!("SftpClient", "hostKeyFingerprint", [Text], host_key_fingerprint),
        method!("SftpClient", "connect", [], connect),
        method!("SftpClient", "isConnected", [], is_connected),
        method!("SftpClient", "lsFiles", [Text], ls_files),
        method!("SftpClient", "ls", [Text], ls),
        method!("SftpClient", "getBinary", [Text], get_binary),
        method!("SftpClient", "getText", [Text, OptValue], get_text),
        method!("SftpClient", "put", [Text, Text, Value, OptValue, OptBool], put),
        method!("SftpClient", "moveFile", [Text, Text, OptBool], move_file),
        method!("SftpClient", "renameFile", [Text, Text], rename_file),
        method!("SftpClient", "rename", [Text, Text], rename),
        method!("SftpClient", "rm", [Text], rm),
        method!("SftpClient", "openStream", [Text], open_stream),
        method!("SftpClient", "putStream", [Text], put_stream),
        method!("SftpClient", "readChunk", [Value, Value], read_chunk),
        method!("SftpClient", "readAll", [Value], read_all),
        method!("SftpClient", "readAllText", [Value, OptValue], read_all_text),
        method!("SftpClient", "writeChunk", [Value, Value, OptValue], write_chunk),
        method!("SftpClient", "closeStream", [Value], close_stream),
        method!("SftpClient", "close", [], close),
    ],
    getters: &[],
    hidden: &[],
    shim_js: None,
    shim_members: &[],
};

type State = Mutex<SftpClient>;

fn make(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let host = call.text_or_empty(0);
    let username = call.text_or_empty(1);
    let port = match number_of(call.value(2)) {
        Some(number) if (1.0..=65535.0).contains(&number) =>
        {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            Some(number as u16)
        }
        _ => None,
    };
    let client = SftpClient::new(&host, &username, port).map_err(HostError::Lib)?;
    let state: State = Mutex::new(client);
    Ok(Ret::Object(HostObject::new(state), &SFTP_CLIENT))
}

/// The number a value is, as `JsValue::as_number` reads it: none for anything but a number.
fn number_of(value: Option<&Value>) -> Option<f64> {
    #[allow(clippy::cast_precision_loss)]
    match value? {
        Value::I32(number) => Some(f64::from(*number)),
        Value::I64(number) => Some(*number as f64),
        Value::F64(number) => Some(*number),
        _ => None,
    }
}

/// `body` on this client.
fn with_client<T>(
    call: &Call<'_>,
    body: impl FnOnce(&mut SftpClient) -> Result<T, moqui_lib::error::Error>,
) -> Result<T, HostError> {
    let state: Arc<State> = call.this::<State>()?;
    let mut client = lock_state(&state, "SftpClient")?;
    body(&mut client).map_err(HostError::Lib)
}

fn done<T>(result: Result<T, HostError>) -> Result<Ret, HostError> {
    result.map(|_| Ret::Undefined)
}

fn password(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let password = call.text_or_empty(0);
    done(with_client(call, |client| {
        client.password(&password);
        Ok(())
    }))
}

fn public_key_pkcs8(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let private_key = call.text_or_empty(0);
    let public_key = call.text_or_empty(1);
    done(with_client(call, |client| {
        client.public_key_pkcs8(&private_key, Some(public_key.as_str()));
        Ok(())
    }))
}

fn preserve_attributes(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let preserve = call.bool(0).unwrap_or(false);
    done(with_client(call, |client| {
        client.preserve_attributes(preserve);
        Ok(())
    }))
}

fn host_key_fingerprint(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let fingerprint = call.text_or_empty(0);
    done(with_client(call, |client| {
        client.host_key_fingerprint(&fingerprint);
        Ok(())
    }))
}

fn connect(call: &mut Call<'_>) -> Result<Ret, HostError> {
    done(with_client(call, |client| client.connect().map(|_| ())))
}

fn is_connected(call: &mut Call<'_>) -> Result<Ret, HostError> {
    with_client(call, |client| Ok(client.is_connected())).map(Ret::from)
}

fn ls_files(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    let names = with_client(call, |client| client.ls_files(&path))?;
    Ok(Value::List(names.into_iter().map(Value::String).collect()).into())
}

/// Maps of `name, path, isFile, size, lastAccess, lastModified`, as Java's `ls`.
fn ls(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    let entries = with_client(call, |client| client.ls(&path))?;
    let list = entries
        .into_iter()
        .map(|(full, entry)| {
            let mut map = indexmap::IndexMap::default();
            let attributes = entry.attributes;
            map.insert("name".to_string(), Value::String(entry.name));
            map.insert("path".to_string(), Value::String(full));
            map.insert("isFile".to_string(), Value::Bool(attributes.kind == SftpKind::File));
            map.insert(
                "size".to_string(),
                Value::I64(attributes.size.and_then(|size| i64::try_from(size).ok()).unwrap_or(0)),
            );
            map.insert(
                "lastAccess".to_string(),
                Value::I64(i64::from(attributes.access_time.unwrap_or(0))),
            );
            map.insert(
                "lastModified".to_string(),
                Value::I64(i64::from(attributes.modified_time.unwrap_or(0))),
            );
            Value::Map(map)
        })
        .collect();
    Ok(Value::List(list).into())
}

fn get_binary(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    with_client(call, |client| client.get_binary(&path)).map(|bytes| Value::Bytes(bytes).into())
}

fn get_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    let charset = call.text_or_empty(1);
    with_client(call, |client| client.get_text(&path, &charset)).map(Ret::from)
}

/// Text in a charset, or bytes (a `Uint8Array`, an `ArrayBuffer`, an uploaded file, or a resource location).
fn put(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    let filename = call.text_or_empty(1);
    let create_dir = call.bool(4).unwrap_or(false);
    if let Some(Value::String(text)) = call.value(2) {
        let text = text.clone();
        let charset = call.text_or_empty(3);
        return done(with_client(call, |client| {
            client
                .put_text(&path, &filename, &text, &charset, create_dir)
                .map(|_| ())
        }));
    }
    let bytes = data_bytes(call.value_or_null(2))?;
    done(with_client(call, |client| {
        if create_dir {
            let parent = path.trim_end_matches('/').rsplit_once('/').map_or("", |(dir, _)| dir);
            if !parent.is_empty() {
                client.mkdirs(parent)?;
            }
        }
        client.put_bytes(&path, &filename, &bytes).map(|_| ())
    }))
}

fn move_file(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let file_path = call.text_or_empty(0);
    let dest_dir = call.text_or_empty(1);
    let create_dir = call.bool(2).unwrap_or(true);
    with_client(call, |client| client.move_file(&file_path, &dest_dir, create_dir)).map(Ret::from)
}

fn rename_file(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let file_path = call.text_or_empty(0);
    let new_name = call.text_or_empty(1);
    with_client(call, |client| client.rename_file(&file_path, &new_name)).map(Ret::from)
}

fn rename(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let from = call.text_or_empty(0);
    let to = call.text_or_empty(1);
    with_client(call, |client| client.rename(&from, &to)).map(Ret::from)
}

fn rm(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    done(with_client(call, |client| client.rm(&path)))
}

#[allow(clippy::cast_precision_loss)]
fn handle_value(handle: u64) -> Ret {
    Ret::Value(Value::F64(handle as f64))
}

fn open_stream(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    with_client(call, |client| client.open_stream(&path)).map(handle_value)
}

fn put_stream(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let path = call.text_or_empty(0);
    with_client(call, |client| client.put_stream(&path)).map(handle_value)
}

fn read_chunk(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let handle = stream_handle(call);
    let max = stream_size(number_of(call.value(1)).unwrap_or(f64::NAN));
    with_client(call, |client| client.read_chunk(handle, max)).map(|bytes| Value::Bytes(bytes).into())
}

fn read_rest(call: &Call<'_>) -> Result<Vec<u8>, HostError> {
    let handle = stream_handle(call);
    with_client(call, |client| {
        let mut out = Vec::new();
        loop {
            let chunk = client.read_chunk(handle, 1 << 16)?;
            if chunk.is_empty() {
                return Ok(out);
            }
            out.extend_from_slice(&chunk);
        }
    })
}

fn read_all(call: &mut Call<'_>) -> Result<Ret, HostError> {
    read_rest(call).map(|bytes| Value::Bytes(bytes).into())
}

fn read_all_text(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let charset = call.text_or_empty(1);
    let encoding = crate::sftp::encoding_for(&charset).map_err(HostError::Lib)?;
    let bytes = read_rest(call)?;
    Ok(crate::sftp::decode(encoding, &bytes).into())
}

fn write_chunk(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let handle = stream_handle(call);
    let bytes = match call.value_or_null(1) {
        Value::String(text) => {
            let charset = call.text_or_empty(2);
            let encoding = crate::sftp::encoding_for(&charset).map_err(HostError::Lib)?;
            crate::sftp::encode(encoding, &text)
        }
        other => data_bytes(other)?,
    };
    done(with_client(call, |client| client.write_chunk(handle, &bytes)))
}

fn close_stream(call: &mut Call<'_>) -> Result<Ret, HostError> {
    let handle = stream_handle(call);
    done(with_client(call, |client| client.close_handle(handle)))
}

fn close(call: &mut Call<'_>) -> Result<Ret, HostError> {
    done(with_client(call, |client| {
        client.close();
        Ok(())
    }))
}

/// The stream handle of the first argument.
#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn stream_handle(call: &Call<'_>) -> u64 {
    number_of(call.value(0)).unwrap_or(f64::NAN).max(0.0) as u64
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn stream_size(max: f64) -> usize {
    if max.is_finite() && max >= 1.0 {
        (max as usize).min(1 << 20)
    } else {
        1 << 16
    }
}

/// Bytes from a `Uint8Array`, an `ArrayBuffer`, an uploaded file, or a resource location.
fn data_bytes(data: Value) -> Result<Vec<u8>, HostError> {
    match data {
        Value::Bytes(bytes) => return Ok(bytes),
        Value::Object(object) => {
            if let Some(upload) = object.downcast::<crate::web::multipart::UploadedFile>() {
                return upload.read_bytes().map_err(|err| HostError::message(err.message()));
            }
        }
        Value::Map(map) => {
            if let Some(location) = map.get("resourceLocation") {
                let location = moqui_lib::object::to_plain_string(location);
                let found = with_ec(|ec| ec.factory().resource().get(&location))?;
                return found
                    .and_then(|resource| resource.read_bytes())
                    .map_err(|err| HostError::message(err.message()));
            }
        }
        _ => {}
    }
    Err(HostError::message(
        "SftpClient.put takes text, a Uint8Array, an uploaded file, or a resource reference",
    ))
}

#[cfg(test)]
mod tests {
    use crate::script::eval_value;
    use crate::sftp::tests::memory;
    use moqui_lib::object::{to_plain_string, Value};

    #[test]
    fn a_script_uses_sftp_client_as_groovy_did() {
        let context = crate::context::ExecutionContextFactory::test_context();
        let _entered = context.enter();
        let connector = memory(context.factory());
        let script = r#"(function () {
            var out = [];
            var c = new SftpClient("files.example.com", "joe");
            try { c.lsFiles("/in"); } catch (e) { out.push(String(e.message)); }
            try { c.connect(); } catch (e) { out.push(String(e.message)); }
            c.password("pw").preserveAttributes(false).connect();
            c.put("/in", "a.txt", "café", "ISO-8859-1").put("/in/b.bin", "x", new Uint8Array([1, 2, 3]));
            c.put("/new/dir/c.txt", "c", "see", null, true);
            out.push(c.lsFiles("/in").join(","));
            var listed = c.ls("/in");
            out.push(listed[0].name + ":" + listed[0].path + ":" + listed[0].isFile + ":" + listed[0].size);
            out.push(c.getText("/in/a.txt", "ISO-8859-1"));
            out.push(c.getBinary("/in/b.bin").length);
            var w = c.putStream("/in/s.txt"); w.write("one "); w.write(new Uint8Array([116, 119, 111])); w.close();
            var r = c.openStream("/in/s.txt"); out.push(r.readText()); r.close();
            out.push(c.moveFile("/in/a.txt", "/in/done"));
            out.push(c.renameFile("/in/s.txt", "t.txt"));
            c.rm("/in/t.txt");
            try { c.publicKey(null, null); } catch (e) { out.push(String(e.message).includes("publicKeyPkcs8")); }
            c.close();
            try { c.getText("/in/b.bin"); } catch (e) { out.push(String(e.message)); }
            out.push(SftpClient.SFTP_PORT);
            return out;
        })()"#;
        let result = eval_value(script);
        let Value::List(items) = result.unwrap() else {
            panic!("not a list");
        };
        let texts: Vec<String> = items.iter().map(to_plain_string).collect();
        assert_eq!(
            texts,
            [
                "SFTP Client not connected",
                "No password or KeyProvider specified, cannot connect to joe@files.example.com:22",
                "a.txt,b.bin",
                "a.txt:/in/a.txt:true:4",
                "caf\u{e9}",
                "3",
                "one two",
                "/in/done/a.txt",
                "/in/t.txt",
                "true",
                "SFTP Client not connected",
                "115",
            ]
        );
        assert!(connector.disk.lock().unwrap().files.contains_key("/new/dir/c.txt"));
    }
}
