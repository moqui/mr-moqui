(function() {
    found_xafind = ec.entity.find("moqui.test.TestEntity").orderBy("testId").condition("testId", seedId);
    found = found_xafind.list();
    oneRow = ec.entity.find("moqui.test.TestEntity").condition("testId", seedId).one();
    anyRow = ec.entity.find("moqui.test.TestEntity").disableAuthz().condition("testId", seedId).one();
    anyCount = ec.entity.find("moqui.test.TestEntity").disableAuthz().count();
    return;
})()
