(function() {
    found_xafind = ec.entity.find("moqui.test.TestEntity").orderBy("testId").condition("testId", seedId);
    found = found_xafind.list();
    oneRow = ec.entity.find("moqui.test.TestEntity").condition("testId", seedId).one();
    return;
})()
