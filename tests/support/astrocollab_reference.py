"""Start the separately checked-out public reference server for host tests."""
import json
import sys
import threading

sys.path.insert(0, sys.argv[1])
from reference.server import serve

server = serve(port=0)
threading.Thread(target=server.serve_forever, daemon=True).start()
print(json.dumps({"url": "http://127.0.0.1:%s/" % server.server_address[1],
                  "code": server.tools.issue_pairing_code("Director host test")}), flush=True)
sys.stdin.read()
server.shutdown()
server.server_close()
