// The decimal evaluator of payroll formulas (org.moqui.impl.ExpressionServices, IronStone People design PE-09;
// crates/moqui-lib/src/decimal_expr.rs, whose cargo tests hold the grammar's cases): exact decimals, and an unknown name,
// function or anything outside the grammar refused in the answer, never as a service error.
// Run: ./moq test framework --spec DecimalExpression
const SERVICES = "org.moqui.impl.ExpressionServices"

// the service's answer and the error messages it left (none for a refused expression)
function invoke(verb, parameters) {
    const result =
        ec.service
            .sync()
            .name(SERVICES + "." + verb + "#DecimalExpression")
            .parameters(parameters)
            .call() || {}
    const error = ec.message.hasError() ? ec.message.getErrorsString() : ""
    ec.message.clearErrors()
    return Object.assign({}, result, {error})
}
const evaluate = (expression, inputs, inputNames) => invoke("evaluate", {expression, inputs, inputNames})
const validate = (expression, inputNames) => invoke("validate", {expression, inputNames})

spec({
    // the services need a signed-in user, as any caller has, and read nothing but their parameters
    setupSpec() {
        ec.user.internalLoginUser("john.doe")
    },
    cleanupSpec() {
        ec.user.logoutUser()
    }
})

feature("a formula computes in exact decimals over its named inputs", {
    when() {
        return {
            sum: evaluate("0.1 + 0.2", {}),
            annual: evaluate("salary * 12", {salary: toDecimal("1000.10")}),
            overtime: evaluate("round(if(hours > 40, (hours - 40) * rate * 1.5, 0), 2)", {hours: 45, rate: "20.333"}),
            capped: evaluate("min(gross * 0.1, cap)", {gross: toDecimal("5000"), cap: toDecimal("450.00")})
        }
    },
    then({sum, annual, overtime, capped}) {
        sum.valid && sum.error == ""
        sum.result.eq(toDecimal("0.3"))
        String(annual.result) == "12001.20"
        String(overtime.result) == "152.50"
        capped.result.eq(toDecimal("450"))
        overtime.names.join(",") == "hours,rate"
    }
})

feature("validate lists what a formula reads and refuses a name outside its declared inputs", {
    when() {
        return {
            ok: validate("max(base, floor) * factor", ["base", "floor", "factor"]),
            unknown: validate("base * bonus", ["base"])
        }
    },
    then({ok, unknown}) {
        ok.valid && ok.error == ""
        ok.names.join(",") == "base,factor,floor"
        ok.functions.join(",") == "max"
        !unknown.valid && unknown.errorCode == "UNKNOWN_NAME" && unknown.errorPosition == 8
        unknown.error == ""
    }
})

feature("anything outside the grammar is refused with its code and leaves no error", {
    when() {
        const codes = {}
        for (const expression of ["ec(1)", "eval(1)", "ec.user", "'text'", "a = 1", "a; b", "1 >", "1 > 0"])
            codes[expression] = validate(expression)
        return {
            codes,
            errors: Object.values(codes)
                .map(answer => answer.error)
                .join("")
        }
    },
    then({codes, errors}) {
        codes["ec(1)"].errorCode == "UNKNOWN_FUNCTION" && codes["eval(1)"].errorCode == "UNKNOWN_FUNCTION"
        codes["ec.user"].errorCode == "SYNTAX_ERROR" && codes["ec.user"].errorPosition == 3
        codes["'text'"].errorCode == "SYNTAX_ERROR"
        codes["a = 1"].errorCode == "SYNTAX_ERROR" && codes["a; b"].errorCode == "SYNTAX_ERROR"
        codes["1 >"].errorCode == "SYNTAX_ERROR"
        codes["1 > 0"].errorCode == "TYPE_MISMATCH"
        Object.values(codes).every(answer => answer.valid === false && answer.errorMessage)
        errors == ""
        !ec.message.hasError()
    }
})

feature("a missing input, a bad input and a division by zero are refused in the answer", {
    when() {
        return {
            missing: evaluate("a * b + c", {a: 1, c: null}),
            bad: evaluate("a + 1", {a: "ten"}),
            zero: evaluate("a / b", {a: 1, b: 0}),
            empty: evaluate("", {})
        }
    },
    then({missing, bad, zero, empty}) {
        !missing.valid && missing.errorCode == "MISSING_INPUT" && missing.result == null
        missing.missingNames.join(",") == "b,c"
        bad.errorCode == "INPUT_INVALID" && bad.errorMessage.includes("a")
        zero.errorCode == "DIVIDE_BY_ZERO"
        empty.errorCode == "EXPRESSION_REQUIRED"
        missing.error == "" && bad.error == "" && zero.error == "" && empty.error == ""
    }
})

feature("a parameter of the wrong shape is refused in the answer, not as an error", {
    when() {
        return {
            names: evaluate("a + 1", {a: 1}, ["a", 2]),
            text: invoke("validate", {expression: {a: 1}}),
            negative: evaluate("-a", {a: toDecimal("0.00")})
        }
    },
    then({names, text, negative}) {
        !names.valid && names.errorCode == "INPUT_INVALID" && names.error == ""
        !text.valid && text.errorCode == "INPUT_INVALID" && text.error == ""
        String(negative.result) == "0.00"
    }
})
