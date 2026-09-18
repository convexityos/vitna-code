# Drives vitna-coded over its named pipe or socket, the way the window does,
# to put real turns, sessions and receipts on disk for a capture.
#
#   python scripts/drive-daemon.py <workspace>              the demo set
#   python scripts/drive-daemon.py <workspace> "prompt" ... these, in one new session
#   python scripts/drive-daemon.py --list                   what the daemon holds
#
# The frame is vitna-protocol's: a 4-byte big-endian length, then a JSON
# envelope whose `payload` is the request's own JSON as a byte array. Every
# turn asks for the sku "stub-messages-api", for use with
# scripts/stub-messages-api.py, so the receipts it produces name the stub
# rather than a model that never served them. Windows and Unix endpoints both.
import json
import os
import struct
import sys
import time

T = "type.vitna.ai/vitna.protocol.v1."


def endpoint():
    # The same resolution as crates/protocol/src/endpoint.rs.
    if os.name == "nt":
        user = "".join(c if c.isascii() and c.isalnum() else "-" for c in os.environ.get("USERNAME", ""))
        bs = chr(92)  # a backslash, built rather than typed
        return bs + bs + "." + bs + "pipe" + bs + "vitna-" + user
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    base = os.path.join(runtime, "vitna") if runtime else os.path.join(os.environ["HOME"], ".vitna")
    return os.path.join(base, "vitna.sock")


class Client:
    def __init__(self):
        if os.name == "nt":
            self.f = open(endpoint(), "r+b", buffering=0)
            self.read, self.write = self.f.read, self.f.write
        else:
            import socket
            s = socket.socket(socket.AF_UNIX)
            s.connect(endpoint())
            self.read = lambda n: s.recv(n, socket.MSG_WAITALL)
            self.write = s.sendall
        self.seq = 0

    def call(self, name, payload):
        self.seq += 1
        env = {"protocol_version_major": 1, "protocol_version_minor": 0, "schema_version": 1,
               "type_url": T + name, "session_id": "", "run_id": "", "sequence": self.seq,
               "idempotency_key": "%s-%d" % (name, self.seq),
               "payload": list(json.dumps(payload).encode())}
        data = json.dumps(env).encode()
        self.write(struct.pack(">I", len(data)) + data)
        n = struct.unpack(">I", self.read(4))[0]
        reply = json.loads(self.read(n))
        body = json.loads(bytes(reply["payload"]))
        if reply["type_url"].endswith(".Error"):
            raise RuntimeError(body["message"])
        return body

    def turn(self, session, prompt):
        r = self.call("SubmitTurn", {"session_id": session, "prompt": prompt, "provider": "anthropic",
                                     "model_sku": "stub-messages-api", "auto_approve": True,
                                     "verification_command": None})
        print(" ", r["run_id"], r["completion_state"], r["files_modified"])
        time.sleep(1.2)  # run ids are millisecond stamps; keep them apart


def main():
    c = Client()
    if sys.argv[1:] == ["--list"]:
        print(json.dumps(c.call("ListSessions", {}), indent=1))
        print(json.dumps(c.call("ListTurns", {}), indent=1))
        return
    ws, prompts = sys.argv[1], sys.argv[2:]
    print("health", c.call("Health", {}))
    if prompts:
        s = c.call("CreateSession", {"workspace_root": ws})["session_id"]
        for p in prompts:
            c.turn(s, p)
        return
    s1 = c.call("CreateSession", {"workspace_root": ws})["session_id"]
    c.turn(s1, "Draft release notes for the run list and the ListTurns call")
    c.turn(s1, "Explain what the verifier checks in a receipt")
    s2 = c.call("CreateSession", {"workspace_root": ws})["session_id"]
    c.turn(s2, "Sketch a search palette: its sections, filters and keys")
    c.call("CreateSession", {"workspace_root": ws})  # one with no turn, which reads "New session"


if __name__ == "__main__":
    main()
