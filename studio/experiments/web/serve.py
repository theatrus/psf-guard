"""Serves the spike directory and stores the results the web baseline posts.

Run from the spike directory:  python3 web/serve.py [port] [results-dir]
"""

import http.server
import pathlib
import re
import sys

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765
RESULTS = pathlib.Path(sys.argv[2] if len(sys.argv) > 2 else "web/results")
ROOT = pathlib.Path(__file__).resolve().parent.parent


class Handler(http.server.SimpleHTTPRequestHandler):
    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    def do_POST(self):
        name = re.sub(r"[^a-z0-9-]", "", self.path.rsplit("/", 1)[-1].lower()) or "result"
        body = self.rfile.read(int(self.headers.get("Content-Length", "0")))
        RESULTS.mkdir(parents=True, exist_ok=True)
        (RESULTS / f"{name}.json").write_bytes(body)
        self.send_response(204)
        self.end_headers()


http.server.ThreadingHTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
