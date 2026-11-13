// runtime/components/base/tools/test/load/ServiceCall.load.js: what the Service Load Runner screen did
plan({
    title: "Service call",
    description: "One service called again and again in this server",
    uses: ["service"],
    params: {
        serviceName: { default: "org.moqui.impl.BasicServices.echo#Data", description: "The service to call" },
        parameters: { default: '{"textIn1": "load"}', description: "Its parameters, as JSON" },
    },
})

const parameters = JSON.parse(params.parameters)     // once per user: the file is evaluated in each user's engine

scenario("calls", {
    users: 10, rampUp: 5, duration: 60, think: 100,
    run(user, iteration) {
        service.call(params.serviceName, { ...parameters, index: user * 1000000 + iteration })
    },
})
