const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

function renderDialog(label) {
    const factory = vm.runInNewContext(fs.readFileSync(path.join(__dirname, 'ScreenHtml.js'), 'utf8'));
    // Only runtime rendering is exercised; macro callbacks are registered, not compiled.
    const module = factory({ stmt: fn => fn, noops: () => ({}) }, {
        anyMacros: () => ({}), allElements: [], opaqueElements: []
    });
    let html = '';
    const context = {
        sri: { write: text => { html += text; } },
        ec: { resource: { expand: text => text }, context: { get: () => null } }
    };
    vm.runInNewContext(module.runtime, context);
    context.$mqHtml.containerDialog({ attrs: { id: 'CreateSalesOrderDialog', 'button-text': label } });
    return html;
}

test('dialog trigger has an explicit accessible name independent of icon font content', () => {
    const button = renderDialog('Create Sales Order').match(/<button[^>]*>/)[0];
    assert.match(button, /aria-label="Create Sales Order"/);
    assert.match(button, /data-dialog="CreateSalesOrderDialog"/);
});

test('dialog accessible name is HTML-escaped', () => {
    const button = renderDialog('Create "Sales" & Orders').match(/<button[^>]*>/)[0];
    assert.match(button, /aria-label="Create &quot;Sales&quot; &amp; Orders"/);
});
