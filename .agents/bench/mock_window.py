import http.server
import json
import socket
import socketserver
import subprocess
import threading
import time

# 窗口期 mock：进程内启动，前 5 秒 502，之后 200
WINDOW = 5
PORT = 59997

state = {"fail_until": time.time() + WINDOW}


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if time.time() < state["fail_until"]:
            body = b"upstream overloaded\n"
            self.send_response(502)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
        else:
            resp = json.dumps({"ok": True, "after": WINDOW}).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(resp)))
            self.end_headers()
            self.wfile.write(resp)

    def log_message(self, *a):
        pass


class TS(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


srv = TS(("127.0.0.1", PORT), Handler)
threading.Thread(target=srv.serve_forever, daemon=True).start()
print(f"mock listening {PORT}, 502 window {WINDOW}s", flush=True)

# 窗口期内经 aproxy 发请求
time.sleep(1.0)
t0 = time.time()
out = subprocess.run(
    ["curl", "-s", "-X", "POST", "http://127.0.0.1:59999/v1/chat", "-d", "{}", "-m", "40"],
    capture_output=True, text=True, timeout=45,
)
t1 = time.time()
print(f"status={out.stdout[:120]!r} elapsed={t1 - t0:.1f}s", flush=True)
print("EXPECT: elapsed >= 4s（窗口剩余+重试）且最终 200", flush=True)
time.sleep(1)
srv.shutdown()
