"""Exercise URL installs using a local HTTP server; no external services."""
import hashlib
import http.server
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import threading

binary = str(Path(sys.argv[1]).resolve())
payload = Path(__file__).resolve().parents[1].joinpath('dist/hello-1.0.0.dpk').read_bytes()
plain = io.BytesIO()
with tarfile.open(fileobj=plain, mode='w:gz') as archive:
    data = b'hello'
    entry = tarfile.TarInfo('data.txt')
    entry.size = len(data)
    archive.addfile(entry, io.BytesIO(data))

widget = io.BytesIO()
widget_manifest = json.dumps({
    'id': 'org.smoke.widget', 'name': 'Smoke Widget', 'description': 'smoke',
    'version': '1.0.0', 'apiVersion': 1, 'kind': 'ui', 'system': False,
    'entry': 'org.smoke.widget.js', 'order': 100, 'settingsVersion': 1,
    'enabledByDefault': True, 'settings': [],
}).encode()
with tarfile.open(fileobj=widget, mode='w:gz') as archive:
    for name, data in [('extension/extension.json', widget_manifest),
                       ('extension/dist/org.smoke.widget.js', b'export default {};\n')]:
        entry = tarfile.TarInfo(name)
        entry.size = len(data)
        archive.addfile(entry, io.BytesIO(data))


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def do_GET(self):
        if self.path.startswith('/redirect.dpk'):
            self.send_response(302)
            self.send_header('Location', '/app.dpk?token=private')
            self.end_headers()
            return
        if self.path.startswith('/missing.dpk'):
            self.send_error(404)
            return
        data = (b'broken' if self.path == '/broken.dpk'
                else plain.getvalue() if self.path == '/plain.tar.gz'
                else widget.getvalue() if self.path == '/widget.dpkx'
                else payload)
        self.send_response(200)
        self.send_header('Content-Length', str(len(data)))
        self.end_headers()
        self.wfile.write(data)


server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
threading.Thread(target=server.serve_forever, daemon=True).start()
try:
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory) / 'store'
        base = f'http://127.0.0.1:{server.server_port}'
        environment = dict(os.environ, XDG_DATA_HOME=str(Path(directory) / 'xdg'))

        def run(*args, ok=True):
            result = subprocess.run([binary, '--root', str(root), *args],
                                    capture_output=True, text=True, encoding='utf-8', env=environment)
            assert (result.returncode == 0) == ok, (args, result.stdout, result.stderr)
            return result

        run('install', base + '/redirect.dpk', '--sha256', hashlib.sha256(payload).hexdigest())
        run('verify')
        before = (root / 'registry.json').read_bytes()
        for url, extra in [('/app.dpk', ['--sha256', '0' * 64]), ('/missing.dpk?token=private', []), ('/broken.dpk', []), ('/index.html', [])]:
            result = run('install', base + url, *extra, ok=False)
            assert 'token=private' not in result.stderr
            assert (root / 'registry.json').read_bytes() == before
        run('install', base + '/plain.tar.gz')
        packages = json.loads(run('list', '--json').stdout)['packages']
        assert 'plain' in packages
        run('install', base + '/widget.dpkx')
        extensions = json.loads(run('list', '--json').stdout)['extensions']
        assert 'org.smoke.widget' in extensions
        run('verify')
        run('install', 'ftp://example.com/app.dpk', ok=False)
    print('URL install smoke passed: redirect, query, checksum, HTTP errors, corrupt archive, tar.gz name inference, .dpkx extension URL, unchanged registry on failure')
finally:
    server.shutdown()
    server.server_close()
