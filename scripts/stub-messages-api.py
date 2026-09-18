# A local stand-in for the Anthropic Messages API, so real turns can run
# through vitna-coded on a machine with no key.
#
#   python scripts/stub-messages-api.py [port]        (default 8765)
#
# Then start the daemon against it, with a placeholder key:
#   ANTHROPIC_BASE_URL=http://127.0.0.1:8765/v1/messages
#   ANTHROPIC_API_KEY=local-stub-not-a-key
#
# It answers from a script keyed on the prompt: a prompt naming "release
# notes" or "search palette" gets write_file calls for files under
# .vitna/demo/ (gitignored) and then a closing line; a prompt naming "keep
# looking" gets a list_dir call on every round and never an answer, so the
# daemon's loop meets its round cap; anything else gets a text answer. It
# reports no token usage, since it has none to report.
#
# Everything that runs through it is a test double for the model, not a
# model. Drive it with scripts/drive-daemon.py, which asks for the sku
# "stub-messages-api" so no receipt claims a real model served the turn, and
# restart the daemon without these variables afterwards, so a turn somebody
# types in the window cannot produce a receipt naming a model that did not run.
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 8765

PLANS = [
    ("release notes", [(".vitna/demo/release-notes.md",
        "# Release notes\n\n- Recent runs on the start screen, read off the receipts.\n- Sessions named by their first prompt, from the daemon's log.\n")]),
    ("search palette", [
        (".vitna/demo/search-palette.md", "# Search\n\nRuns, sessions and actions, filtered as you type.\n"),
        (".vitna/demo/search-keys.md", "# Keys\n\nCtrl+K opens, arrows move, Enter opens, Esc closes.\n")]),
]


def first_user_text(msgs):
    for m in msgs:
        if m.get("role") == "user":
            c = m.get("content")
            if isinstance(c, str):
                return c
            if isinstance(c, list):
                return " ".join(b.get("text", "") for b in c if isinstance(b, dict))
    return ""


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("content-length", 0))
        body = json.loads(self.rfile.read(n) or b"{}")
        msgs = body.get("messages", [])
        last = msgs[-1].get("content") if msgs else None
        answered = isinstance(last, list) and any(
            isinstance(b, dict) and b.get("type") == "tool_result" for b in last)
        prompt = first_user_text(msgs).lower()
        files = next((f for key, f in PLANS if key in prompt), [])
        if "keep looking" in prompt:
            reply = {"content": [{"type": "tool_use", "id": "toolu_stub_look_%d" % len(msgs),
                                  "name": "list_dir", "input": {"path": "."}}],
                     "stop_reason": "tool_use"}
        elif files and not answered:
            blocks = [{"type": "tool_use", "id": "toolu_stub_%d" % i, "name": "write_file",
                       "input": {"path": p, "content": c}} for i, (p, c) in enumerate(files)]
            reply = {"content": blocks, "stop_reason": "tool_use"}
        elif files:
            n = len(files)
            reply = {"content": [{"type": "text", "text": "Wrote %d file%s under .vitna/demo." % (n, "" if n == 1 else "s")}],
                     "stop_reason": "end_turn"}
        else:
            reply = {"content": [{"type": "text", "text": "The verifier checks the schema version, the isolation label, the completion state, and that every changed file names a before and an after hash. The signature is checked only when a public key is supplied."}],
                     "stop_reason": "end_turn"}
        reply.update({"id": "msg_local_stub", "type": "message", "role": "assistant",
                      "model": body.get("model", "")})
        data = json.dumps(reply).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        sys.stderr.write("stub: " + (fmt % args) + "\n")


if __name__ == "__main__":
    HTTPServer(("127.0.0.1", PORT), Handler).serve_forever()
