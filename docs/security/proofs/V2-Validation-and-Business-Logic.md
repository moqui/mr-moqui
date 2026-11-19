# V2 Validation and Business Logic

The framework's status for each OWASP ASVS 5.0.0 requirement of chapter V2 at levels 1 and 2. The topic is in our words;
the requirement's own text is in [ASVS 5.0.0](https://github.com/OWASP/ASVS/tree/v5.0.0/5.0/en). How to read this
table, and what a status claims, is in the [index](README.md).

## V2.1 Validation and Business Logic Documentation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-2.1.1 | 1 | Documented format rules for each input item | Application | service XML `parameter` type, `required` and checks (`matches`, `number-range`, `text-email`, `credit-card`, `time-range`) are the declared rules; `service/validate.rs` `apply_check` | - |
| v5.0.0-2.1.2 | 2 | Documented rules for consistency across related fields | Application | no declarative cross-field rule; service actions or SECA `pre-validate` | - |
| v5.0.0-2.1.3 | 2 | Documented business limits, per user and overall | Application | `org.moqui.public.AccountServices.record#Attempt` windows and `ArtifactTarpit` rows are the builder's to document; web protection limits in Attack-Surface-Web (map) | PASS: to write: black-box |

## V2.2 Input Validation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-2.2.1 | 1 | Input checked against declared allowed forms | Application | `service/validate.rs` `clean`: type, `required`, declared checks; `allow-html` none refuses `<`; undeclared parameters dropped (`validate_input`) | PASS: mr-mantle `pass/PassAuthority` "an operation runs its schema service and a field the schema does not list is the envelope's 422" |
| v5.0.0-2.2.2 | 1 | Validation enforced on the server, not client | Framework | `service/validate.rs` `clean` runs in every service call before its body; client checks not relied on | V6Authentication "v5.0.0-6.2.1: a new password shorter than the minimum length is refused"; PASS: mr-mantle `pass/PassAuthority` "an operation runs its schema service and a field the schema does not list ..." |
| v5.0.0-2.2.3 | 2 | Related fields checked to fit together | Application | SECA `pre-validate`, service actions; entity-auto status flow check (`service/call.rs` `check_status`) | - |

## V2.3 Business Logic Security

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-2.3.1 | 1 | Multi-step flows run in order, no skips | Application | `StatusFlowTransition` on entity-auto update (`service/call.rs` `check_status`); sign-in holds the second-factor step (`moquiPreAuthcUsername`) | PASS: mr-mantle `order/ShopCheckout` "an order changed after its submit is not placed; its approved payment is kept for staff" |
| v5.0.0-2.3.2 | 2 | Business limits enforced as documented | Application | `AccountServices.record#Attempt`, `ArtifactTarpit` | - |
| v5.0.0-2.3.3 | 2 | Business operations commit whole or roll back | Framework | `service/define.rs` default `TransactionAttribute::UseOrBegin`; a failed service rolls back (`service/call.rs`) | `a_body_failure_is_recorded_rolled_back_and_returns_empty_results` (`service/call_tests.rs`); PASS: mr-mantle `order/ShopCheckout` "a placement that fails after the provider's success keeps the payment and attempt ..." |
| v5.0.0-2.3.4 | 2 | Locks keep scarce things from being sold twice | Application | `EntityFind.forUpdate` (`entity/find.rs` `for_update`), optimistic lock (`entity/value.rs`) | PASS: mr-mantle `work/EventRegistrations` "six requests at once for the last two seats: ... never more seats than the offer has" |

## V2.4 Anti-automation

| Requirement | L | Topic | Status | Evidence | Proof |
| --- | --- | --- | --- | --- | --- |
| v5.0.0-2.4.1 | 2 | Rate limits against automated excessive calls | Configuration | `webapp-list/webapp[webroot]/web-protection@enabled` (or `webapp-list/webapp[webroot]@trusted-proxies`) turns on per-client rate limits and bans (`moqui-host/src/guard.rs`); on by default: global limits, `user-facade/login@max-failures`, LLM tarpits (`AgentTypeData.xml`); open SEC-WEB-81, SEC-WEB-83 | - |
