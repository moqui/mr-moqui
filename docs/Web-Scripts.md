# Web scripts: servlets, filters, listeners and lifecycle actions

A webapp in the conf (`webapp-list/webapp`, the `webroot` one) can add its own request handling in JavaScript:

- a **servlet** answers the requests on its URL patterns;
- a **filter** runs before the servlet on its patterns, and may stop the request or change the response;
- a **listener** hears the server start and stop, sessions begin and end, and requests begin and end;
- the **lifecycle actions** are XML actions run at fixed points of a request, a sign in and the server's life.

The model is Java's: Moqui Java registered `servlet`, `filter` and `listener` classes with the servlet container. Here
each is a script whose value is a **handler object** with the Servlet API's method names, so a Java servlet or filter
ports by moving its methods. Scripts run as service scripts do, with `ec` and the same tools. The objects a script gets
cover the common cases of the Servlet API and will grow.

Code: `crates/moqui-core/src/web/servlet.rs` (conf, mapping, filter chain, listeners, lifecycle actions),
`crates/moqui-core/src/script/runtime/web_host.rs` (`request`, `response`, the handler call),
`crates/moqui-core/src/web/upstream.rs` (proxied bodies).

## Conf

```xml
<webapp name="webroot">
    <filter name="MyAuth" location="component://mycomp/script/MyAuthFilter.js" priority="3">
        <init-param name="permission" value="MY_REMOTE"/>
        <url-pattern>/my/*</url-pattern>
    </filter>
    <listener location="component://mycomp/script/AuditListener.js"/>
    <servlet name="MyApi" location="component://mycomp/script/MyApiServlet.js" max-body="20m">
        <init-param name="greeting" value="${my_greeting ?: 'hello'}"/>
        <url-pattern>/my/*</url-pattern>
    </servlet>
</webapp>
```

- **Locations.** `location` is the script. The file is read again when it changes, as service scripts are, so there
  is no restart in development.
- **Init params.** `init-param` values expand `${}` from the properties, as the rest of the conf does.
- **`class` instead of `location`.** `class` names one of the Java classes the server has natively:

  | Class | What it is here |
  |---|---|
  | `org.moqui.impl.webapp.MoquiServlet` | the screens: token routes, then the screen at the path |
  | `org.moqui.impl.webapp.MoquiFopServlet` | the screen at the path info as a PDF |
  | `org.moqui.impl.webapp.MoquiAuthFilter` | `component://webroot/script/AuthFilter.js` |
  | `org.moqui.impl.webapp.ElasticRequestLogFilter` | the server's request log; `enabled="false"` turns it off |
  | `org.moqui.impl.webapp.MoquiSessionListener` | the server's session expiry |

  Any other class is logged at startup and left out, so a conf written for Java starts.
- **When the conf declares no servlet.** The webapp then gets `MoquiServlet` on `/*` and `MoquiFopServlet` on `/fop/*`.
- **Unsupported attributes.** `async-supported` and `load-on-startup` are accepted and not read. `dispatcher` may only
  be `REQUEST`: a filter runs once, on the request from the client.
- **Startup checks.** Each script and action block is checked once at startup. A missing file or actions that do not
  compile are logged then, and the element fails again, logged, when it runs; the rest of the webapp works.

## URL patterns

Servlets map as the Servlet spec (12.2) maps them. For a request path, the first of these wins:

1. **An exact pattern** (`/status`).
   - `servletPath` is the path.
   - `pathInfo` is null.
2. **The longest path prefix** (`/my/*`).
   - `servletPath` is the prefix.
   - `pathInfo` is the rest, or null when the path is just the prefix.
   - `/*` is a prefix of everything, which is why `MoquiServlet` is the fallback.
3. **An extension** (`*.csv`).
4. **The default** (`/`).

No match is a 404. Filters run on every pattern that covers the path, in `priority` order (default 5, lower first),
then in the order declared.

## The handler object

The file's value is its last expression: an object literal in parentheses, as a script file returns its last
expression. Names declared before it stay local to the file.

```js
// component://mycomp/script/MyApiServlet.js
const greet = (name, config) => `${config.getInitParameter("greeting")}, ${name}`

;({
    doGet(request, response, config) {
        response.setContentType("application/json")
        response.write(JSON.stringify({ message: greet(request.getParameter("name") ?? "you", config) }))
    },
    doPost(request, response) {
        const body = JSON.parse(request.bodyText())
        ec.service.sync().name("mycomp.MyServices.store#Thing").parameters(body).call()
        if (ec.message.hasError()) { response.sendError(400, ec.message.errorsString); return }
        response.setStatus(201)
    },
})
```

| Kind | Methods |
|---|---|
| Servlet | `service(request, response, config)`, or one per method: `doGet`, `doPost`, `doPut`, `doDelete`, `doPatch`, `doHead` (else `doGet`), `doOptions`. With neither, the request is 405 |
| Filter | `doFilter(request, response, chain, config)`. `chain.doFilter()` runs the next filter, or the servlet, and returns when it is done, so the filter can work on the response after. A filter that does not call it ends the request with what it wrote |
| Listener | any of `contextInitialized`, `contextDestroyed`, `sessionCreated`, `sessionDestroyed`, `requestInitialized`, `requestDestroyed`, each taking `(event, config)`. `event.type` is the method's name; the session events have `event.sessionId` |

**Config.** Each method also gets `config`, and `this.config` is set to it before the call:

- `getInitParameter(name)` and `getInitParameterNames()`
- `name`, the element's name
- `location`, the script's location

**State and scope.** Do not keep state in the object between requests. Each worker thread evaluates the file on its own,
so state there is not shared and not kept. Use `ec.cache` for state that outlives a request, and
`context.sharedMap` within one execution context.

**Running servlets and filters.** These run on a request worker, inside a context frame of their own. By then the user
of the request is signed in:

- the session cookie;
- Basic auth;
- `api_key` or `login_key`;
- `authUsername` and `authPassword`.

`ec.user` is that user. CORS preflights (`OPTIONS`) are answered before any servlet. A request from an origin CORS does
not allow is refused with 401. The request's own site is never refused: its `Host`, the `X-Forwarded-Host` of a
believed proxy (`client-ip-header`, `trusted-proxies`), or the conf webapp's `http-host` or `https-host`.

**Running listeners.** Session and context events run with an execution context of their own and no request. Request
events run around the request's servlet.

**Errors.** A servlet or filter that throws answers that request 500 (or the status of the error's kind), and the
error is logged. A listener that throws is logged, and the other listeners still run.

## `request` and `response`

`ec.web.request` and `ec.web.response` are the same objects that a handler gets. Any script running in a web request
can use them, a screen's transition included. Outside a web request both are null. `ec.web.getRequest()` and
`ec.web.getResponse()` return them too.

### `request`

`request` is a subset of `HttpServletRequest`. Each field is also a getter: `getMethod()`, `getRequestURI()` and so
on, with `isSecure()` for `secure`.

| Member | What it is |
|---|---|
| `method`, `requestURI`, `queryString` | as sent; the query is still encoded |
| `contextPath`, `servletPath`, `pathInfo` | `""`, then the servlet's match |
| `scheme`, `serverName`, `serverPort`, `secure` | from the request and its `Host` |
| `remoteAddr` | the client address behind `client-ip-header` |
| `contentType`, `characterEncoding`, `contentLength` | the body's |
| `getRequestURL()` | scheme, host, port when not the default, path |
| `getHeader(name)`, `getHeaders(name)`, `getHeaderNames()`, `headerPairs` | header names in lower case |
| `getCookies()` | `[{name, value}]` |
| `getParameter(name)`, `getParameterValues(name)`, `getParameterMap()`, `getParameterNames()` | the query, form and JSON body parameters, as `ec.web.requestParameters` |
| `getAttribute(name)`, `setAttribute(name, value)`, `removeAttribute(name)`, `getAttributeNames()` | request attributes, which a filter can leave for the servlet |
| `getSession()` | `{id, getAttribute, setAttribute, removeAttribute, getAttributeNames}` over `ec.web.sessionAttributes`, or null |
| `bodyText()`, `bodyBytes()` | the raw body as text (UTF-8) or a `Uint8Array`; empty for multipart, whose files are `ec.web.multiPartParameters` |

### `response`

`response` is a subset of `HttpServletResponse`.

| Member | What it does |
|---|---|
| `setStatus(code)`, `getStatus()` | the status, until the body is written |
| `setHeader(name, value)`, `addHeader(name, value)`, `getHeader(name)`, `getHeaders(name)`, `getHeaderNames()`, `containsHeader(name)` | response headers; `setHeader` with null removes |
| `setContentType(type)`, `getContentType()`, `setCharacterEncoding(charset)` | the `Content-Type` header |
| `write(text or Uint8Array)`, `getWriter()`, `getOutputStream()` (also `writer`, `outputStream`) | appends to the body; the writer has `write`, `print`, `println` |
| `sendError(code, message)` | an error page, JSON or text by the request's `Accept`, as `ec.web.sendError` |
| `sendRedirect(url)` | 302 to `url` |
| `isCommitted()` | whether a body or another answer was sent |
| `forward(restResponse, {skipHeaders})` | an `ec.service.rest()` response as this response (below) |

The conf's `screen-render` headers, and CORS for an allowed origin, are added to every response, a script servlet's
included. A header name must be a token and a value has no line break; anything else throws.

## Calling out and passing a body through

`ec.service.rest()` is the REST client of service scripts. `call()` reads the whole answer into the script. Four methods
are for passing bodies through:

| Method | What it does |
|---|---|
| `.streamFrom(request)` | sends the web request's body as it came, without copying it into the script |
| `.bodyBytes(uint8Array)` | sends a binary body |
| `.followRedirects(false)` | hands a 3xx back instead of following it (only `send()` reads it) |
| `.send()` | sends the headers only as given, with no `Content-Type` or `Accept` added, and returns as soon as the answer's headers are in |

The body of a `send()` answer stays on the server:

- `text()`, `bytes()` and `jsonObject()` read it, up to 32 MiB.
- `response.forward(up)` sends it on to the client as it arrives, with no limit and without it ever being a script
  value. A script's heap is 64 MiB, so a large body must go this way.
- A body that is not read or forwarded is dropped when the outermost script ends.

`forward` copies the status and the headers. It leaves out the hop-by-hop headers, `Content-Length` (the body is sent
in chunks) and the names in `skipHeaders`.

## The shipped proxies

`webroot/script/ProxyServlet.js` is a transparent proxy, like Jetty's `ProxyServlet.Transparent`:

- **Path and query.** The path after `prefix`, and the query, go to `proxyTo`. An empty `prefix` uses the servlet path.
- **Method, headers and body.** These go with the request, except for the hop-by-hop headers, `Host`, `Authorization`
  and `Cookie`. The client's Moqui sign in never reaches the upstream.
- **Upstream sign in.** The `user` and `password` init params are sent as Basic auth.
- **`requireElasticApi`.** With `"true"`, the servlet answers 404 unless the search backend is OpenSearch.
- **`timeout`.** The whole exchange in seconds; 120 by default.

`webroot/script/AuthFilter.js` is `MoquiAuthFilter`:

- No user is 401 with a Basic challenge.
- A user without the `permission` init param is 403.

`MoquiDefaultConf.xml` puts them together as:

| Path | Servlet | Upstream | Permission |
|---|---|---|---|
| `/search/*` | `SearchProxy` | the search store at `elasticsearch_url`, signed in as `elasticsearch_user`, bodies up to 100 MB for a bulk load | `SearchRemote` |
| `/dashboard/*` | `DashboardProxy` | OpenSearch Dashboards at `kibana_host` and `kibana_port`, off when `kibana_host` is empty | `DashboardRemote` |

For `/dashboard`, set `server.basePath: "/dashboard"` in `opensearch_dashboards.yml`.

The proxies run on the request pool, as every servlet does, and a Dashboards page makes many requests at once. If its
requests queue, raise `request-pool-max` on `tools`.

## Lifecycle actions

Each is an `<actions>` block in the webapp element, run as XML actions are, with the same tools as services. Each runs
in a context frame of its own.

| Element | When | On error |
|---|---|---|
| `before-request` | each web request, once the user is signed in and before the filters and servlet; the request parameters are in the context | the request answers 500, since the actions may be a check |
| `after-request` | each web request however it ended, before the session work ends; the parameters are in the context | logged |
| `after-login` | a sign in during a web request (password, Basic, key, `ec.user.loginUser`) | logged |
| `before-logout` | a sign out during a web request, while the user is still there | logged |
| `first-hit-in-visit` | a web request that starts a visit (Java defines it and never runs it) | logged |
| `after-startup` | the server is up, after the listeners' `contextInitialized` | logged |
| `before-shutdown` | the server is stopping, before the listeners' `contextDestroyed`, with the services and database still up | logged |

`ec.web.request` and `ec.web.response` are there for the request ones, so an action can set a header:

```xml
<before-request><actions>
    <script>ec.web.response.setHeader('X-Served-By', ec.factory.moquiVersion)</script>
</actions></before-request>
```

Differences from Java are in [Parity-Diffs](../plans/Parity-Diffs.md#screen-and-web-review-2026-10-20) (P9-02, P9-03).
