// Java's MoquiAuthFilter as a web script filter (docs/Web-Scripts.md): the request goes on only for a signed in user,
// and when the init-param "permission" names one, a user who has it. The sign in already happened as it does for a
// screen: the session cookie, Basic, api_key or login_key, or authUsername and authPassword. Nobody signed in is 401
// with a Basic challenge, a user without the permission 403. A conf for Java naming the class
// org.moqui.impl.webapp.MoquiAuthFilter gets this script.
({
    doFilter(request, response, chain, config) {
        if (!ec.user.userId) {
            response.setHeader("WWW-Authenticate", 'Basic realm="Moqui"')
            response.sendError(401, "Login required")
            return
        }
        const permission = config.getInitParameter("permission")
        if (permission && !ec.user.hasPermission(permission)) {
            response.sendError(403, `User ${ec.user.username} does not have permission ${permission}`)
            return
        }
        chain.doFilter(request, response)
    },
})
