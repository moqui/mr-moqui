// Change 6: Localized field-name keys retain Java's text-medium capacity through entity-auto services and storage.
feature('localized field names longer than 63 characters are stored without truncation and can be updated', {
    expect() {
        login('john.doe');
        const entity = 'moqui.basic.LocalizedEntityField';
        const prefix = 'fieldNameBeyondTheFormerSixtyThreeCharacterLimit_' + 'x'.repeat(40);
        const keys = ['A', 'B'].map(suffix => ({entityName: 'moqui.test.TestEntity', fieldName: prefix + suffix,
            pkValue: 'LongLocalizedFieldKey', locale: 'fr'}));
        assert.ok(keys[0].fieldName.length > 63);
        const read = key => ec.entity.find(entity).condition(key).useCache(false).one();
        const wasDisabled = ec.artifactExecution.disableAuthz();
        try {
            for (let i = 0; i < keys.length; i++) {
                call('create#' + entity, Object.assign({}, keys[i], {localized: 'value-' + i}));
                assert.equal(ec.message.hasError(), false, ec.message.getErrorsString());
                const stored = read(keys[i]);
                assert.equal(stored.fieldName, keys[i].fieldName);
                assert.equal(stored.localized, 'value-' + i);
            }
            call('update#' + entity, Object.assign({}, keys[0], {localized: 'updated'}));
            assert.equal(ec.message.hasError(), false, ec.message.getErrorsString());
            assert.equal(read(keys[0]).localized, 'updated');
            assert.equal(read(keys[1]).localized, 'value-1');
        } finally {
            try {
                for (const key of keys) {
                    const row = read(key);
                    if (row) row.delete();
                }
            } finally {
                if (!wasDisabled) ec.artifactExecution.enableAuthz();
            }
        }
    }
});
