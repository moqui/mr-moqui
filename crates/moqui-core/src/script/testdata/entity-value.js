(function() {
    row = ec.entity.makeValue("moqui.test.TestEntity");
    row.setFields(seedMap, true, null, null);
    row.create();
    row.update();
    row.delete();
    var _xa_was_disabled = ec.artifactExecution.disableAuthz();
    try {
        row.update();
    } finally {
        if (!_xa_was_disabled) ec.artifactExecution.enableAuthz();
    }
    return;
})()
