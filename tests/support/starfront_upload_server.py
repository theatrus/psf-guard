"""Isolated upload fixture using a separately checked-out Starfront server."""
import json
import os
from pathlib import Path
import secrets
import socket
import sys
import threading
import time

root = Path(sys.argv[2]).resolve()
root.mkdir(parents=True, exist_ok=True)
os.environ["STARFRONT_DATA"] = str(root / "data")
os.environ["STARFRONT_FILES"] = str(root / "files")
sys.path.insert(0, sys.argv[1])

from server import app
import uvicorn

owner = "upload-fixture-user"
agent = "000000000001"
project = "000000000010"
token = secrets.token_urlsafe(32)
token_file = root / "telescope-token"
fd = os.open(token_file, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
with os.fdopen(fd, "w") as handle:
    handle.write(token)
app.store.upsert_user(owner, secrets.token_urlsafe(32), "Upload fixture", "", [])
app.store.add_agent(agent, token, "Upload fixture telescope", owner_id=owner)
app.store.add_project(project, "Upload fixture M31", "Fixture", {"collectFiles": True}, owner_id=owner)

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
server = uvicorn.Server(uvicorn.Config(app.app, log_level="error", access_log=False))
thread = threading.Thread(target=server.run, kwargs={"sockets": [sock]}, daemon=True)
thread.start()
for _ in range(200):
    if server.started:
        break
    if not thread.is_alive():
        raise RuntimeError("Starfront fixture failed to start")
    time.sleep(0.05)
else:
    raise RuntimeError("Starfront fixture startup timed out")
print(json.dumps({"url": f"http://127.0.0.1:{sock.getsockname()[1]}/",
                  "agent": agent, "project": project, "owner": owner,
                  "token_file": str(token_file)}), flush=True)
sys.stdin.read()
server.should_exit = True
thread.join(timeout=10)
sock.close()
