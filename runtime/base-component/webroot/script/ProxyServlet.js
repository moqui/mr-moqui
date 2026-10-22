// A transparent proxy as a web script servlet (docs/Web-Scripts.md), as Jetty's ProxyServlet.Transparent: the path
// after the prefix, and the query, go to proxyTo with the request's method, headers and body, and the answer comes
// back as it is. Neither body passes through the script: the request's is sent from the server and the answer's is
// streamed to the client as it comes. Init params:
//   proxyTo            the upstream base URL, such as http://127.0.0.1:9200; with no host the servlet answers 404
//   prefix             stripped from the path before it is sent; empty for the servlet path
//   user, password     Basic sign in to the upstream; the client's own Authorization and cookies are never sent on
//   requireElasticApi  "true": 404 unless the search backend is OpenSearch (the /search proxy)
//   timeout            seconds for the whole exchange, 120 by default
const SKIP = new Set(["connection", "keep-alive", "proxy-authenticate", "proxy-authorization", "proxy-connection", "te",
    "trailer", "transfer-encoding", "upgrade", "host", "authorization", "cookie", "content-length"])

;({
    service(request, response, config) {
        const proxyTo = String(config.getInitParameter("proxyTo") || "").trim().replace(/\/+$/, "")
        if (!/^https?:\/\/[^\/:?#]/i.test(proxyTo)) {
            response.sendError(404, `${config.name} has no upstream to proxy to`)
            return
        }
        if (config.getInitParameter("requireElasticApi") === "true" && !ec.search.hasElasticApi()) {
            response.sendError(404, "The search backend is not OpenSearch, so there is nothing to proxy")
            return
        }
        const prefix = String(config.getInitParameter("prefix") || request.servletPath || "")
        let path = request.requestURI
        if (prefix && (path === prefix || path.startsWith(prefix + "/"))) path = path.substring(prefix.length)
        if (!path) path = "/"
        const query = request.queryString
        const upstream = ec.service.rest().method(request.method).uri(proxyTo + path + (query ? "?" + query : ""))
            .followRedirects(false).timeout(Number(config.getInitParameter("timeout") || 120))
        for (const [name, value] of request.headerPairs) if (!SKIP.has(name)) upstream.addHeader(name, value)
        const user = String(config.getInitParameter("user") || "").trim()
        if (user) {
            const password = config.getInitParameter("password") || ""
            upstream.addHeader("Authorization", "Basic " + StringUtilities.base64Encode(`${user}:${password}`))
        }
        response.forward(upstream.streamFrom(request).send())
    },
})
