// Regression spec (moqui test-regression, plans/Regression-Test.md): two units of length, a conversion between them,
// conversions both ways, a changed factor, and the deletes. The runner compares each answer, such as convertedAmount,
// and each row that changed with the other engine's run. The features run in order.

const METRE = { uomId: "REGRESSION_M" }
const CENTIMETRE = { uomId: "REGRESSION_CM" }
const CONVERSION = { uomConversionId: "REGRESSION_CONV" }

// The service ran; what it answered is for the comparison.
const ran = (res) => res.ok

feature("create the unit #uomId", {
    where: [
        { uomId: METRE.uomId, description: "Regression metres" },
        { uomId: CENTIMETRE.uomId, description: "Regression centimetres" },
    ],
    expect({ uomId, description }) {
        ran(app.service("create#moqui.basic.Uom", { uomId, uomTypeEnumId: "UT_LENGTH_MEASURE", description }))
    },
})

feature("create a conversion of 100 from metres to centimetres", {
    expect() {
        ran(app.service("create#moqui.basic.UomConversion", { ...CONVERSION, uomId: METRE.uomId,
            toUomId: CENTIMETRE.uomId, conversionFactor: "100", fromDate: "2020-01-01 00:00:00" }))
    },
})

feature("convert #amount from #uomId to #toUomId", {
    where: [
        { amount: "2", uomId: METRE.uomId, toUomId: CENTIMETRE.uomId },
        { amount: "200", uomId: CENTIMETRE.uomId, toUomId: METRE.uomId },
    ],
    expect({ amount, uomId, toUomId }) {
        app.service("org.moqui.impl.BasicServices.convert#Uom",
            { uomId, toUomId, amount, effectiveDate: "2026-01-01 00:00:00" }).ok
    },
})

feature("change the conversion factor to 200", {
    expect() {
        ran(app.service("org.moqui.impl.BasicServices.update#UomConversion", { ...CONVERSION, conversionFactor: "200" }))
    },
})

feature("convert 2 metres with the new factor", {
    expect() {
        app.service("org.moqui.impl.BasicServices.convert#Uom",
            { ...METRE, toUomId: CENTIMETRE.uomId, amount: "2", effectiveDate: "2026-01-01 00:00:00" }).ok
    },
})

feature("delete the conversion", {
    expect() { ran(app.service("delete#moqui.basic.UomConversion", CONVERSION)) },
})

feature("delete the unit #uomId", {
    where: [CENTIMETRE, METRE],
    expect({ uomId }) { ran(app.service("delete#moqui.basic.Uom", { uomId })) },
})
