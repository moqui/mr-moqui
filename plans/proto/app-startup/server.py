# A test server for run.js: each variant's page after DOC_MS (TLS, session check, render), its data after DATA_MS,
# and the shell libraries from site/libs (run.sh copies them).
import http.server, socketserver, time, json, os, sys
DOC_MS, DATA_MS = 400, 300
SITE = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'site')
DOC_CACHE = {'a': 'no-store', 'b': 'public, max-age=0', 'd': 'public, max-age=0', 'e': 'public, max-age=0', 'f': 'public, max-age=31536000, immutable'}
class H(http.server.BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def send(self, b, ctype, cache):
        if isinstance(b, str): b = b.encode()
        try:
            self.send_response(200); self.send_header('Content-Type', ctype); self.send_header('Content-Length', str(len(b)))
            self.send_header('Cache-Control', cache); self.end_headers(); self.wfile.write(b)
        except (BrokenPipeError, ConnectionResetError): pass
    def do_GET(self):
        p = self.path.split('?')[0]
        if p.startswith('/libs/'):
            ct = 'text/css' if p.endswith('.css') else 'text/javascript'
            return self.send(open(SITE + p, 'rb').read(), ct, 'public, max-age=31536000, immutable')
        parts = p.split('/'); v = parts[1] if len(parts) > 2 else ''
        if v not in DOC_CACHE: return self.send_error(404)
        rest = p[len(v) + 2:]
        if rest == '':
            time.sleep(DOC_MS / 1000)
            return self.send(open(f'{SITE}/{v}.html').read(), 'text/html', DOC_CACHE[v])
        if rest == 'data':
            time.sleep(DATA_MS / 1000)
            return self.send(json.dumps({'items': [f'Story {i}: a headline of normal length for a list row' for i in range(40)]}), 'application/json', 'no-store')
        if rest in ('app.js', 'sw.js'):
            return self.send(open(f'{SITE}/{rest}').read(), 'text/javascript', 'no-cache' if rest == 'sw.js' else 'public, max-age=31536000, immutable')
        self.send_error(404)
class S(socketserver.ThreadingMixIn, http.server.HTTPServer): daemon_threads = True
S(('127.0.0.1', int(sys.argv[1])), H).serve_forever()
