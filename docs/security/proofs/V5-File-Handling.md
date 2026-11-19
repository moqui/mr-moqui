# V5 File Handling

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V5 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V5.1 File Handling Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-5.1.1 | 2 | Documented upload types, sizes and safe serving | Application | none; the framework's own uploads: sizes in Attack-Surface-Files and -Web, any type, served by the inert-type and sandbox rule (map) | PASS: to write: black-box |

## V5.2 File Upload and Content

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-5.2.1 | 1 | Upload size bounded to what server handles | Gap | SEC-RTE-82 (a multipart body holds up to the 2 GiB `max-upload` in memory); limits: `webapp-list/webapp[webroot]/web-protection/requests@max-upload`, `@max-body`, `@max-spool`, DAV `max-file` | - |
| v5.0.0-5.2.2 | 1 | Upload extension and content match expected type | Application | `web/multipart.rs` `is_executable` under `webapp-list/webapp[webroot]@upload-executable-allow` false; image services decode by bytes; no extension-to-content check | - |
| v5.0.0-5.2.3 | 2 | Archive size and entry limits before expanding | Gap | SEC-DAT-91 (loader holds nested zips and whole documents unbounded); `script/host/archive.rs` `resource_extract_archive` stops at 4 GiB / 1M entries while expanding | PASS: to write: in-process |

## V5.3 File Storage

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-5.3.1 | 1 | Stored uploads never run as server code | Gap | SEC-SCR-81 (a wiki page saved with a template extension runs as server script; fix merged 2026-11-18, not yet recorded); the cut at `#` (SEC-PFS-82's cause) is fixed: `context/resource.rs` `refuse_cut_write` refuses a write at a location holding `#` | - |
| v5.0.0-5.3.2 | 1 | File paths built from trusted data, not names | Gap | `print#ResourceDocument` reads only a `dbresource://` or `content://` location (SEC-RTE-83 fixed); `context/resource.rs` `runtime_ref` joins `..`; met in the drive, DAV, SFTP and elFinder checks and `multipart.rs` `base_name` | - |

## V5.4 File Download

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-5.4.1 | 2 | Download name set by server in Content-Disposition | Framework | `web/mod.rs` `attachment_disposition`, `send_resource_response` (the given name or the location's last segment) | V5FileHandling "v5.0.0-5.4.2: a quote in a requested file name stays inside one Content-Disposition"; PASS: to write: black-box |
| v5.0.0-5.4.2 | 2 | Served file names encoded against header injection | Framework | `web/mod.rs` `attachment_disposition`: `"`, `\`, `;` and controls replaced, `filename*=utf-8''` percent-encoded; mail attachments not read | V5FileHandling "v5.0.0-5.4.2: a quote in a requested file name stays inside one Content-Disposition"; PASS: to write: black-box |
| v5.0.0-5.4.3 | 2 | Untrusted files scanned for malware | Gap | no scan hook for drives, DAV, SFTP or `upload#PublicShare` (map) | - |
