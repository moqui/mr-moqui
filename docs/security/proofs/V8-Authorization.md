# V8 Authorization

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V8 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V8.1 Authorization Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-8.1.1 | 1 | Documented rules for function and data access | Application | tool: `ArtifactGroup`, `ArtifactAuthz` and `EntityFilterSet` rows, read back by `AccessServices.get#Capabilities`, `get#ScopeRules`, `get#GrantReview` (Attack-Surface-Admin.md "Access"); the framework's own: Attack-Surface-Web.md seed grants, -Admin.md "Privileged groups and sealed permissions" | - (document) |
| v5.0.0-8.1.2 | 2 | Documented rules for field read and write | Application | no field-level authorization record; tools: service `in-parameters`/`out-parameters`, entity `restricted` and `encrypt`, password and identity fields kept out of REST (`moqui-entity/src/entity/sensitive.rs`) | - |

## V8.2 General Authorization Design

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-8.2.1 | 1 | Functions only for explicitly permitted callers | Gap | deny by default: no allow record refuses (`user/authz.rs` `is_permitted`; `artifact-execution@authz-enabled` true for every type); screens and services need a user unless marked; open: SEC-TLS-85 (Service Run reaches the refused entity-auto services), SEC-AI-95 (eval runs any service with authorization off) | - |
| v5.0.0-8.2.2 | 1 | Records only for callers permitted that record | Gap | tools: row filters (`user/filter.rs` `apply_entity_filters`, `EntityFilterSet`) and checks in services; open: SEC-RTE-81 (High; posted `_isMulti` replaces a pinned id), SEC-TLS-82 (High), SEC-TLS-84, SEC-DAT-81 | - |
| v5.0.0-8.2.3 | 2 | Fields only for callers permitted that field | Gap | REST drops password fields, and a non-`ADMIN`'s identity fields (`entity/sensitive.rs`, `service/rest.rs`); open: SEC-ADM-81 (users PATCH writes email, `disabled`, `ipAllowed` past the guard), SEC-TLS-81 (High; Data Load writes any field), SEC-TLS-84, SEC-DAT-92 | PassAuthority (mr-mantle) "an operation runs its schema service and a field the schema does not list is the envelope's 422" |

## V8.3 Operation Level Authorization

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-8.3.1 | 1 | Authorization decided on the server, not the client | Framework | every screen, transition, service, REST path and entity is checked in the server on the artifact stack (`user/authz.rs` `authorize`, `is_permitted`); the browser holds no rule | FW: SecurityScreens "A01 a VIEW-only user cannot run a service over HTTP"; PASS: PassAuthority (mr-mantle) "a delegation needs a registered client and a good credential, and a write needs a session or a delegation" |

## V8.4 Other Authorization Considerations

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-8.4.1 | 2 | One tenant cannot act on another tenant | Application | the framework knows no organization; tool: `EntityFilterSet` filters on the user's or active organization (`user/filter.rs`); open in the tool: SEC-DAT-88, SEC-TLS-94 | - |
