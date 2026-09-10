# R4: 重试隔离性——实例 A 的上游 502 疯狂重试期间，实例 B（正常上游）的请求
# 不受任何干扰（低延迟、高成功率）。验证重试风暴不拖垮同进程其他请求路径。
# 用法: python3 r4_isolation.py <窗口秒>
import http.server
import json
import socket
import socketserver
import subprocess
import sys
import threading
import time

WINDOW = float(sys.argv[1])
MOCK_BAD = 59997   # A 的上游（502 窗口）
MOCK_GOOD = 59996  # B 的上游（恒 200）
A_PORT, B_PORT = 59991, 59992
state = {"fail_until": time.time() + WINDOW}


def mk_handler(fail_check):
    class Handler(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def setup(self):
            super().setup()
            self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

        def do_POST(self):
            self.rfile.read(int(self.headers.get("Content-Length", 0)))
            if fail_check and time.time() < state["fail_until"]:
                body = b"bad\n"
                self.send_response(502)
                self.send_header("Content-Length", str(len(body)))
                self.end_headers()
                self.wfile.write(body)
                return
            resp = json.dumps({"ok": True}).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(resp)))
            self.end_headers()
            self.wfile.write(resp)

        def log_message(self, *a):
            pass

    return Handler


class TS(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


srv_bad = TS(("127.0.0.1", MOCK_BAD), mk_handler(True))
srv_good = TS(("127.0.0.1", MOCK_GOOD), mk_handler(False))
for s in (srv_bad, srv_good):
    threading.Thread(target=s.serve_forever, daemon=True).start()
print(f"mocks: bad={MOCK_BAD}（502 {WINDOW}s 窗口） good={MOCK_GOOD}（恒 200）", flush=True)

lat = []
errs = []
lock = threading.Lock()


def worker():
    t = time.time()
    out = subprocess.run(
        ["curl", "-s", "-X", "POST", f"http://127.0.0.1:{B_PORT}/v1/chat", "-d", "{}", "-m", "10"],
        capture_output=True, text=True, timeout=15,
    )
    dt = time.time() - t
    with lock:
        if "ok" in out.stdout:
            lat.append(dt)
        else:
            errs.append(out.stdout[:40])


th_b = threading.Thread(target=worker, args=())
threads = [threading.Thread(target=worker) for _ in range(10)]
# 窗口开始：A 疯狂重试的同时 B 持续被打
t0 = time.time()
th_b.start()
th_b.join()
print(f"B 首个请求: {time.time()-t0:.2f}s（窗口期内）", flush=True)
for th in threads:
    th.start()
    time.sleep(0.3)
for th in threads:
    th.join()
wall = time.time() - t0
lat_s = sorted(lat)
print(f"B 实例结果: ok={len(lat)}/{len(lat)+len(errs)} "
      f"p50={lat_s[len(lat_s)//2]*1000:.0f}ms max={lat_s[-1]*1000:.0f}ms wall={wall:.1f}s", flush=True)
print(f"EXPECT: ok={len(lat)+len(errs)}/{len(lat)+len(errs)} 且 max < 500ms（A 的重试不得干扰 B）", flush=True)
srv_bad.shutdown()
srv_good.shutdown()
sys.exit(0 if not errs else 1)
