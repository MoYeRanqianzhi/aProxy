import http.server
import json
import socket
import socketserver
import subprocess
import threading
import time

# 直接用 reqwest 同款的 rust 客户端发请求验证 502 路径——
# 绕过 aproxy：如果 rust 客户端直连也挂，问题在 python mock 与 hyper 的
# HTTP/1.1 兼容性；如果直连成功，问题在 aproxy 转发层。
PORT = 59996

state = {"fail_until": time.time() + 5}


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if time.time() < state["fail_until"]:
            self.send_response(502)
            self.send_header("Content-Length", "21")
            self.end_headers()
            self.wfile.write(b"upstream overloaded\n")
        else:
            resp = json.dumps({"ok": True}).encode()
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
print(f"mock listening {PORT}", flush=True)

time.sleep(0.5)
# 直连（不经 aproxy）的窗口期请求
out = subprocess.run(
    ["curl", "-s", "-X", "POST", f"http://127.0.0.1:{PORT}/v1/chat", "-d", "{}", "-m", "10",
     "-w", "\nHTTP:%{http_code}"],
    capture_output=True, text=True, timeout=15,
)
print(f"direct window: {out.stdout!r}", flush=True)
time.sleep(6)
out = subprocess.run(
    ["curl", "-s", "-X", "POST", f"http://127.0.0.1:{PORT}/v1/chat", "-d", "{}", "-m", "10",
     "-w", "\nHTTP:%{http_code}"],
    capture_output=True, text=True, timeout=15,
)
print(f"direct after: {out.stdout!r}", flush=True)
srv.shutdown()
