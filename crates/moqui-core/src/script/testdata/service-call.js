(function() {
    if (true) {
        var call_service_result = ec.service.callXml("org.moqui.example.ExampleServices.do#Thing", 25, 30, ec.context, {"orderId": orderId});
        if (call_service_result != null) result = Object.assign({}, result || {}, call_service_result);
        if (ec.message.hasError()) return;
    }
    return;
})()
