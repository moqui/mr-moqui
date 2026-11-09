(function() {
    statusId = orderStatus;
    label = ec.resource.expand("Hello ${name}");
    var _temp_internal = orderStatus;
    if (!__isEmpty(_temp_internal)) code = _temp_internal;
    var _temp_default = orderStatus;
    fallback = (__isGroovyFalse(_temp_default) ? ec.resource.expand("Open") : _temp_default);
    return;
})()
