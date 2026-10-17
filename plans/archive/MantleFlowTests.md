# Mantle USL flow tests

> Archived 2026-10-16: a status snapshot of the mantle-usl specs on that date. The live summary (specs, coverage, the services never called) is [Remaining-Work, Mantle conversion follow-up](../Remaining-Work.md#mantle-conversion-follow-up); the run commands below still apply.

The ten Spock specs of mantle-usl (`runtime/component/mr-mantle-usl/src/test/groovy`, kept as the oracle) are ported to JavaScript specs in `runtime/component/mr-mantle-usl/test/`, run by `moqui test` (design: [TestHarness](TestHarness.md)). The earlier Rust port (`mantle_flows.rs`, `mantle_support`) is deleted.

```sh
moqui test mantle-usl                                  # every spec, fresh database
moqui test mantle-usl --db snapshot                    # load once, restore later
moqui test mantle-usl --spec OrderToCashBasicFlow      # one spec and its dependsOn closure
```

The component name is `mantle-usl` (from `component.xml`), not the directory name.

## Status (2026-10-16)

344 features pass in 43 specs, none ignored (about 30s with the scratch database loaded; the 8 flow specs account for 84 of them, and the whole suite with coverage reports 751 of 762 services called).

| Spec | Features |
|---|---|
| AccountingActivities | 6 |
| AssetReservationMultipleThreads | 1 |
| OrderProcureToPayBasicFlow | 23 |
| OrderToCashBasicFlow | 42 |
| OrderToCashTime | 1 |
| ReturnToResponseBasicFlow | 12 |
| WorkPlanToCashBasicFlow | 11 |
| RestApiTests | 1 summaries + 11 `where` rows |

`OrderTenantAccess` is not ported (not in the Spock suite; multi-tenant is deprecated).

## Specs without a Spock original (2026-10-16)

Written to run services that no flow reaches, and run after the flows. Each was written against the service text, not an oracle, so a failing check is as likely a wrong expectation as a bug; the notes in each file say where a service is an upstream stub. `moqui test mantle-usl --coverage` lists the services no spec called, per file; `--coverage-report file` writes the call counts.

| Spec | Services |
|---|---|
| HumanResServices | Employment, EmploymentSalary, RateAmount, payroll time periods, positions |
| PayrollServices | process#TimePeriodPay, employment pay, invoices, payments, reopen |
| ContactAndFacilityServices | party and facility contact info, FacilityServices |
| PartyServices | find, convert, roles, classifications, settings, user accounts, badge scans |
| BankFileServices | OFX and BAI 2 import, match, reconcile, unreconcile |
| BankOutputFiles | NACHA, Positive Pay (six formats) |
| FinancialReports | income statement, balance sheet, cash flow, ratios, ledger info, period series |
| ProductCatalogServices | features, variants, categories, content, clone, store slugs and options |
| BudgetServices | budgets, sub-period items, details, expense reports |
| TaxStatementServices | 1099-MISC statements, payer and recipient tax info |
| InvoicePaymentMisc | invoice and payment edits, credit memo, reverse, cancel, void, refund |
| WorkProjectServices | work efforts, projects, milestones, tasks, comments, time entries, events |
| LedgerMaintenance | GL accounts, transactions, journals, period close and reopen, reposting |
| OrderLifecycle | order edits, status changes, clone, merge, recurring orders, content, propose |
| FinancialAccountServices | deposits, withdrawals, transfers, authorizations, disbursement, replenishment |
| CrmMiscServices | contacts, accounts, requests, messages, equipment, purchase orders from a request |
| PaymentMethodServices | credit card and bank account methods, file completion, content |
| AssetReservationMisc | assets, reservations, issuance, physical inventory, party assignments, moves |
| ShipmentReturnMisc | packages, scans, shipment info, content, returns, cancel, resell |
| ManufacturingServices | production runs, consume and produce, estimates |
| PayrollAdjustments | tax, deduction and garnishment adjustments, workers comp rates, W-2 data, wage and tax records, EFW2 |
| InvoiceOrderApply | invoices applied to orders and shipments, invoice item details |
| ShipmentLoads | picklists, bin numbers, status from shipments, complete and close, bulk creation |
| PartyTimePeriods | period groups, previous and next periods, names, delete |
| TimeImports | time summary and detail imports, bulk time entry work efforts |
| PaymentGateway | authorize, release, capture, refund, the test gateway services, cancel |
| SalesReports | sales order and invoice summaries, top products, universal search |
| Promotions | promotion codes, buy-get and flat discounts |
| ShipmentPick | pickMove into containers, route segments, complete pack, receive a whole shipment |
| OrderBulk | store#Order create and revise, bulk status change, generated test orders |
| PartyAccounts | party and person search, update#Account, user account person, relationship settings |
| ReportLookups | lookup by id, filtered search, feature interactions, units, projected inventory |
| CarrierServices | order shipping rates, label markup, label request and refund dispatch, insurance, address validation, label printing |
| Subscriptions | subscriptions started by approving an order of a digital product |
| ContentRecords | payment, agreement and party content records, with and without an uploaded file |

Specs that create their own organization (`init#PartyAccountingConfiguration` with `sourcePartyId: "DefaultSettings"`) get a full set of fiscal periods and accounts. A spec that freezes time with `ec.user.setEffectiveTime` cannot give one party the same role twice in the same millisecond (primary key).

## Known differences from the Groovy checks

- `totalFieldsChecked` counting is dropped; `checkData` returns only errors.
- `passwordHashType`: this host hashes with argon2, the Groovy expects `SHA-256`.
- A date-only `fromDate` parses as UTC midnight, the Groovy expected US Central midnight.
- REST JSON is written `"key": value`, Jackson wrote `"key" : value`.
- `calculate#FixedAssetDepreciation` and the "sell Depreciated Asset Gain" check depend on the process time zone being behind UTC, as the Groovy does.
