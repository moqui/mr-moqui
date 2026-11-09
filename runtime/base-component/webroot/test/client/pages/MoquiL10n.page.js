/* This software is in the public domain under CC0 1.0 Universal plus a Grant of Patent License. */

// MoquiL10n in the page. l10n is moqui.l10n from /js/MoquiL10n.js. The shared vectors are the repo file the
// server checks too; a classic script cannot await at the top, so the vectors test fetches them.
"use strict";

const l10n = moqui.l10n;

test("the shared vectors format the same way as the server", async () => {
    const response = await fetch("/files/crates/moqui-lib/testdata/format-vectors.json");
    const vectors = await response.json();
    assert.ok(vectors.length > 20);
    for (const vector of vectors) {
        const locale = vector.locale || "en_US";
        let got;
        if (vector.kind === "number") got = l10n.formatNumber(vector.value, vector.pattern, locale);
        else if (vector.kind === "currency") got = l10n.formatCurrencyAmount(vector.value, vector.symbol, vector.fractionDigits === undefined ? 2 : vector.fractionDigits, locale);
        else if (vector.kind === "timestamp") got = l10n.formatTimestamp(vector.value, vector.pattern, vector.tz || "UTC");
        else if (vector.kind === "date") got = l10n.formatDate(vector.value, vector.pattern);
        else if (vector.kind === "time") got = l10n.formatTime(vector.value, vector.pattern);
        else assert.fail("unknown vector kind " + vector.kind);
        assert.equal(got, vector.expect, JSON.stringify(vector));
    }
});

test("rounding is half even on the decimal text", () => {
    assert.equal(l10n.formatNumber("0.125", "0.00", "en_US"), "0.12");
    assert.equal(l10n.formatNumber("0.135", "0.00", "en_US"), "0.14");
    assert.equal(l10n.formatNumber("2.5", "0", "en_US"), "2");
    assert.equal(l10n.formatNumber(0.125, "0.00", "en_US"), "0.12");
    assert.equal(l10n.formatNumber("1e3", "#,##0", "en_US"), "1,000");
    assert.equal(l10n.formatNumber("123456789012345678901234567890.5", "#,##0.0", "en_US"), "123,456,789,012,345,678,901,234,567,890.5");
});

test("an exponent pattern and text that is not a number are errors", () => {
    assert.throws(() => l10n.formatNumber("1", "0.###E0", "en_US"));
    assert.throws(() => l10n.formatNumber("abc", "0", "en_US"));
    assert.throws(() => l10n.formatTimestamp("2024-03-05T14:07:09Z", "yyyy-MM-dd q", "UTC"));
});

test("format picks by the type of the value, and a string stays as it is", () => {
    l10n.setLocale("en-US");
    l10n.setTimeZone("America/Los_Angeles");
    assert.equal(l10n.format(null), "");
    assert.equal(l10n.format("as is", "0"), "as is");
    assert.equal(l10n.format(1234.125), "1,234.125");
    assert.equal(l10n.format("2024-03-05T14:07:09Z", null, "timestamp"), "2024-03-05 06:07");
    assert.equal(l10n.format("2024-03-05", "MM/dd/yyyy", "date"), "03/05/2024");
    assert.equal(l10n.format(new Date(Date.UTC(2024, 2, 5, 14, 7, 9)), "HH:mm"), "06:07");
    assert.equal(l10n.format(true), "true");
    // epoch millis with any date pattern is a time stamp; a number pattern keeps it a number
    const millis = Date.UTC(2024, 2, 5, 14, 7, 9);
    assert.equal(l10n.format(millis, "h:mm a"), "6:07 AM");
    assert.equal(l10n.format(millis, "EEE h:mm a"), "Tue 6:07 AM");
    assert.equal(l10n.format(millis, "HH:mm"), "06:07");
    assert.equal(l10n.format(1234.5, "#,##0.00"), "1,234.50");
    l10n.setTimeZone("UTC");
});

test("a date pattern is told from a number pattern by its letters outside quotes", () => {
    for (const pattern of ["h:mm a", "HH:mm", "EEE", "mm:ss", "yyyy-MM-dd", "a", "w", "z", "d MMM 'at' h"]) assert.ok(l10n.isDatePattern(pattern), pattern);
    for (const pattern of ["#,##0.00", "0", "0.###E0", "#,##0 'units'", "00000", "", "'hello'"]) assert.ok(!l10n.isDatePattern(pattern), pattern);
});

test("currency uses the symbol and fraction digits of the unit, and no symbol when asked", () => {
    l10n.setLocale("en_US");
    assert.equal(l10n.formatCurrency("1234.125", "USD"), "$1,234.12");
    assert.equal(l10n.formatCurrencyNoSymbol("1234.125", "USD"), "1,234.12");
    assert.equal(l10n.formatCurrency(1234, "JPY"), "¥1,234");
    assert.equal(l10n.formatCurrency("", "USD"), "");
    assert.equal(l10n.formatCurrency("5", "ZZZ"), "ZZZ5.00");
    assert.equal(l10n.formatCurrency("5", "USD", 3), "$5.000");
    l10n.setCurrencies({ ZZZ: { symbol: "z", fractionDigits: 1 } });
    assert.equal(l10n.formatCurrency("5", "ZZZ"), "z5.0");
    l10n.setCurrencies({});
    l10n.setLocale("de_DE");
    // The euro amount uses a non-breaking space before the symbol, same as the Node file.
    assert.equal(l10n.formatCurrency("1234.5", "EUR"), "1.234,50\u00a0€");
    l10n.setLocale("en_US");
});

test("localize reads the catalog of the locale, drops a ## suffix, and returns the text when there is no entry", async () => {
    l10n.setCatalog({});
    assert.equal(l10n.localize("Save"), "Save");
    const loaded = await l10n.loadCatalog("fr_FR", (locale) => Promise.resolve({ locale, messages: { Save: "Enregistrer", "Thing##a": "Chose A" } }));
    assert.equal(loaded.Save, "Enregistrer");
    assert.equal(l10n.localize("Save"), "Enregistrer");
    assert.equal(l10n.localize("Thing##a"), "Chose A");
    assert.equal(l10n.localize("Thing##b"), "Thing");
    assert.equal(l10n.localize("Cancel##x ${a##b}"), "Cancel##x ${a##b}");
    assert.equal(l10n.localize(""), "");
    l10n.setCatalog({});
});

test("formatZoned and the calendar day follow the user's language and zone, through daylight saving and an unknown zone", () => {
    const priorLocale = l10n.locale, priorZone = l10n.timeZone;
    const plain = (text) => String(text).replace(/[  ]/g, " ");
    try {
        l10n.setLocale("de_DE");
        l10n.setTimeZone("Europe/Berlin");
        const at = Date.parse("2026-11-08T22:30:00Z");
        assert.equal(l10n.formatZoned(at, { hour: "numeric", minute: "2-digit" }), "23:30");
        assert.ok(l10n.formatZoned(at, { dateStyle: "full" }).startsWith("Sonntag, 8. November 2026"));
        l10n.setLocale("en_US");
        assert.equal(plain(l10n.formatZoned(at, { hour: "numeric", minute: "2-digit" })), "11:30 PM");
        assert.equal(l10n.dayKey(at), "2026-11-08");
        l10n.setTimeZone("Asia/Tokyo");
        assert.equal(l10n.dayKey(at), "2026-11-09", "the same instant is the next day in Tokyo");
        // 2026-11-01 in New York has 25 hours: its last hour is still that day, one day after October 31.
        l10n.setTimeZone("America/New_York");
        assert.equal(l10n.dayNumber(Date.parse("2026-11-02T04:30:00Z")) - l10n.dayNumber(Date.parse("2026-10-31T16:00:00Z")), 1);
        // 2026-03-08 has 23 hours: its first and last minutes are one calendar day.
        assert.equal(l10n.dayNumber(Date.parse("2026-03-09T03:59:00Z")), l10n.dayNumber(Date.parse("2026-03-08T05:00:00Z")));
        l10n.setTimeZone("Not/AZone");
        const browser = new Date(at).toLocaleTimeString("en-US", { hour: "numeric", minute: "2-digit" });
        assert.equal(plain(l10n.formatZoned(at, { hour: "numeric", minute: "2-digit" })), plain(browser), "an unknown zone is the browser's");
    } finally {
        l10n.setLocale(priorLocale);
        l10n.setTimeZone(priorZone);
    }
});
