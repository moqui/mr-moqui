// A load script that makes a request at its top level, which the runner refuses (test/client/Load.spec.js).
const page = http.get("/Login")

scenario("never", { iterations: 1, run() {} })
