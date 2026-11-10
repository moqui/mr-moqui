(function() {
    found_xafind = ec.entity.find("moqui.test.TestEntity").orderBy("testId").condition("testId", seedId);
    found = found_xafind.list();
    oneRow = ec.entity.findOneXml("moqui.test.TestEntity", 0, null, {"testId": seedId}, null);
    anyRow = ec.entity.findOneXml("moqui.test.TestEntity", 64, null, {"testId": seedId}, null);
    anyCount = ec.entity.find("moqui.test.TestEntity").disableAuthz().count();
    return;
})()
