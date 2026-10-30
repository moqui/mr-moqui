// Authored TypeScript against the declarations generated for the fixtures. `tsc --noEmit` passes only when every
// line without a marker type-checks and every `@ts-expect-error` line is an error.

export {};

// ---- entity values, lists and finds ----

const invoice = ec.entity.find("typegen.test.Invoice").condition("invoiceId", "100").one();
if (invoice != null) {
    const amount: Decimal | null = invoice.get("amount");
    const when: Timestamp | null = invoice.get("invoiceDate");
    const status: string | null = invoice.get("statusId");
    invoice.set("amount", "10.00");
    invoice.set("lineCount", 3);
    // Fields are properties too.
    const byDot: Decimal | null = invoice.amount;
    const byIndex: string | null = invoice["statusId"];
    invoice.statusId = "Paid";
    invoice.lineCount = 4;
    // @ts-expect-error not a field of the entity
    void invoice.noSuchField;
    // @ts-expect-error a number field does not take a boolean by property
    invoice.lineCount = true;
    void [byDot, byIndex];
    // @ts-expect-error not a field of the entity
    invoice.get("noSuchField");
    // @ts-expect-error a decimal field does not take a boolean
    invoice.set("amount", true);
    const line = invoice.findRelated("Line#typegen.test.InvoiceItem").get(0);
    const quantity: Decimal | null | undefined = line?.get("quantity");
    const statusValue = invoice.findRelatedOne("status");
    const description: string | null | undefined = statusValue?.get("description");
    // @ts-expect-error a titled relationship is not found by the related name
    invoice.findRelated("InvoiceItem");
    if (amount != null && quantity != null) {
        const total: Decimal = amount.multiply(quantity).add(amount);
        // @ts-expect-error decimals have no operators
        const wrong = amount + quantity;
        // @ts-expect-error decimals have no operators
        const product = amount * quantity;
        const text = `${total}`;
        void [wrong, product, text];
    }
    void [when, status, description];
}

// A short alias and a unique entity name resolve to the same shape.
const byAlias: EntityValue<"inv"> | null = ec.entity.find("inv").one();
const viaShort = ec.entity.find("Invoice").list();
for (const row of viaShort) {
    const id: string | null = row.get("invoiceId");
    void id;
}
const ids: (string | null)[] = viaShort.map((row) => row.get("invoiceId"));
// A name that is not loaded is a plain map.
const loose = ec.entity.makeValue("some.other.Entity");
const anything: unknown = loose.get("whatever");
void [byAlias, ids, anything];

// ---- services ----

const sent = ec.service.sync().name("typegen.services.send#Invoice").parameters({ invoiceId: "100", total: "5.00" }).call();
const sentFlag: boolean | null | undefined = sent.sent;
// @ts-expect-error invoiceId is required
ec.service.sync().name("typegen.services.send#Invoice").parameters({ note: "x" });
// @ts-expect-error not a parameter of the service
ec.service.sync().name("typegen.services.send#Invoice").parameters({ invoiceId: "1", bogus: 1 });
// validate="false" passes other keys through.
ec.service.sync().name("typegen.services.echo#Anything").parameters({ text: "a", extra: 1 });
// The implicit entity-auto verbs: update and delete need the primary key.
const created = ec.service.sync().name("create#typegen.test.InvoiceItem").parameters({ invoiceId: "1", quantity: 2 }).call();
const seq: string | null | undefined = created.invoiceItemSeqId;
ec.service.sync().name("update#typegen.test.InvoiceItem").parameters({ invoiceId: "1", invoiceItemSeqId: "01", description: "d" });
// @ts-expect-error update needs every primary key field
ec.service.sync().name("update#typegen.test.InvoiceItem").parameters({ invoiceId: "1" });
ec.service.sync().name("store#Invoice").parameters({ invoiceId: "1", Status: { statusId: "S" } });
// A name nothing defines takes a plain map.
ec.service.async().name("some.Unknown.do#Thing").parameters({ any: "thing" }).call();
void [sentFlag, seq];

// ---- screens: the server context and the server-static package ----

const screen = context as MoquiScreenContexts["component://typegen/screen/Invoices.xml"];
const items: EntityList<"typegen.test.InvoiceItem"> | undefined = screen.itemList;
const other: unknown = screen.somethingAScriptSet;
void [items, other];

type Props = MoquiStaticScope<MoquiScreenPackages["component://typegen/screen/Invoices.xml"]>;
declare const props: Props;
const total: string | null | undefined = props.invoice?.amount;
const stamp: Timestamp | null | undefined = props.invoice?.invoiceDate;
const options: MoquiOption[] | undefined = props.mqExtra_EditInvoice?.mqOpt_EditInvoice_statusId_0_0;
const shown: string = props.ec.l10n.format(total, "#,##0.00");
// @ts-expect-error the form-list keys are not in the screen package
const rows: EntityList<"typegen.test.Invoice"> = props.invoiceList;
// @ts-expect-error the browser ec has no entity facade
props.ec.entity;
void [stamp, options, shown, rows];

declare const page: MoquiFormListPackages["component://typegen/screen/Invoices.xml#Invoices"];
const first = page.invoiceList[0];
const display: string | undefined = first?.statusId_display;
const rowAmount: string | null | undefined = first?.amount;
const count: number | undefined = page.invoiceListCount;
void [display, rowAmount, count];

// ---- the unwrapped projection strips credentials at any depth ----

type Account = MoquiStripped<{ username: string; currentPassword: string; nested: { api_key: string; keep: number } }>;
declare const account: Account;
const keep: number = account.nested.keep;
// @ts-expect-error a credential name is stripped
account.currentPassword;
// @ts-expect-error at any depth
account.nested.api_key;
void keep;
