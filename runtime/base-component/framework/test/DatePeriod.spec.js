// Date-period value text for the HTML and QVT runtimes. Moved from template/screen/DatePeriod.test.js; no database.
spec({ database: false });

function readModule(file) {
    return runInContext(ec.resource.getLocationText("component://framework/template/screen/" + file), {});
}

// The ScreenSupport helpers a runtime takes as source (parseOrder); the macro tables are not compiled here.
function support() {
    return { anyMacros: () => ({}), allElements: [], opaqueElements: [], parseOrder: readModule('ScreenSupport.js')({}).parseOrder };
}

function datePeriodSummary(mode, values, expectedRangeArgs) {
    const factory = readModule(`Screen${mode}.js`);
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, support());
    const context = { ec: {
        context: { get: key => values[key] ?? null },
        l10n: {
            localize: text => ({ 'from##period': 'from', 'to##period': 'to', Week: 'Woche' })[text] || text,
            format: (value, format) => {
                assert.equal(format, 'yyyy-MM-dd');
                return new Date(value).toISOString().slice(0, 10);
            }
        },
        user: { getPeriodRange: (...args) => {
            assert.deepEqual(args, expectedRangeArgs);
            return [Date.UTC(2026, 6, 19), Date.UTC(2026, 9, 18)];
        } }
    } };
    runInContext(module.runtime, context);
    return context[`$mq${mode}`].valueText({ name: 'date-period', parent: {
        parent: { attrs: { name: 'entryDate' } }
    } });
}

function datePeriodJavaDescription(mode) {
    assert.equal(datePeriodSummary(mode, { entryDate_period: '30D', entryDate_poffset: '-3' }, ['30d', '-3', '']),
        '-3 30 Days (2026-07-19 to 2026-10-17)');
}

function datePeriodOffsetWords(mode) {
    const periods = { day: 'Day', '7d': '7 Days', '30d': '30 Days', week: 'Woche', weeks: 'Weeks',
        month: 'Month', months: 'Months', quarter: 'Quarter', year: 'Year', '7r': '+/-7d', '30r': '+/-30d' };
    for (const [period, label] of Object.entries(periods)) {
        for (const [offset, prefix] of [['0', 'This'], ['-1', 'Last'], ['1', 'Next'], ['2', '2']]) {
            assert.equal(datePeriodSummary(mode, { entryDate_period: period, entryDate_poffset: offset, entryDate_pdate: '2026-10-17' },
                [period, offset, '2026-10-17']), `${prefix} ${label} from 2026-10-17 (2026-07-19 to 2026-10-17)`);
        }
    }
    assert.match(datePeriodSummary(mode, { entryDate_period: 'month' }, ['month', '0', '']), /^This Month /);
}

function datePeriodAbsent(mode) {
    assert.equal(datePeriodSummary(mode, {}), '');
    assert.equal(datePeriodSummary(mode, { entryDate_from: Date.UTC(2026, 0, 1), entryDate_thru: Date.UTC(2026, 0, 31) }),
        'From 2026-01-01 to 2026-01-31');
}

feature("Html: date-period summary uses Java's description and inclusive end date", { expect() { datePeriodJavaDescription("Html"); } });
feature("Qvt: date-period summary uses Java's description and inclusive end date", { expect() { datePeriodJavaDescription("Qvt"); } });
feature("Html: offset words, period labels, basis date and localization", { expect() { datePeriodOffsetWords("Html"); } });
feature("Qvt: offset words, period labels, basis date and localization", { expect() { datePeriodOffsetWords("Qvt"); } });
feature("Html: absent period falls through to explicit date bounds, not null/undefined text", { expect() { datePeriodAbsent("Html"); } });
feature("Qvt: absent period falls through to explicit date bounds, not null/undefined text", { expect() { datePeriodAbsent("Qvt"); } });
