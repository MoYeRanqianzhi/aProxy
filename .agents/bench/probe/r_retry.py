# R1/R3: 重试风暴——N 并发客户端经 aproxy 打请求，上游 502 窗口/拒绝连接 → 全部重试至恢复 200
# 用法: python3 r_retry.py <窗口模式:502|refused> <窗口秒> <并发数>
import http.server
import json
import socket
import socketserver
import subprocess
import sys
import threading
import time

MODE = sys.argv[1]      # 502 = 502 响应窗口；refused = 关闭监听窗口
WINDOW = float(sys.argv[2])
NCONC = int(sys.argv[3])
PORT = 59997            # mock 端口（aprox­y 的 base_url）
state = {"up": True, "fail_until": time.time() + WINDOW}


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if MODE == "502" and time.time() < state["fail_until"]:
            body = b"upstream overloaded\n"
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


class TS(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


srv = TS(("127.0.0.1", PORT), Handler)

# 502 模式：mock 常驻，窗口期返回 502
# refused 模式：mock 延迟 WINDOW 秒才启动——窗口期端口无人监听（OS 拒绝），
# 到点启动即恢复（避免运行期关闭 socket 导致 selector 持续报错无法重开）
if MODE == "refused":

    def late_start():
        time.sleep(WINDOW)
        threading.Thread(target=srv.serve_forever, daemon=True).start()
        print(f"mock 上线（{WINDOW}s 拒绝期结束）", flush=True)

    threading.Thread(target=late_start, daemon=True).start()
else:
    threading.Thread(target=srv.serve_forever, daemon=True).start()
print(f"mock {MODE} window {WINDOW}s on {PORT}", flush=True)

results = []
lock = threading.Lock()
t0 = time.time()


def worker(i):
    t = time.time()
    out = subprocess.run(
        ["curl", "-s", "-X", "POST", "http://127.0.0.1:59990/v1/chat", "-d", "{}", "-m", "120"],
        capture_output=True, text=True, timeout=125,
    )
    dt = time.time() - t
    with lock:
        results.append((i, out.stdout.strip()[:60], round(dt, 2)))


threads = [threading.Thread(target=worker, args=(i,)) for i in range(NCONC)]
for th in threads:
    th.start()
for th in threads:
    th.join()

wall = time.time() - t0
ok = sum(1 for _, s, _ in results if '"ok"' in s or '"ok": true' in s)
ok = sum(1 for _, s, _ in results if "ok" in s)
lat = sorted(d for _, _, d in results)
print(f"clients={NCONC} ok={ok}/{NCONC} wall={wall:.1f}s "
      f"p50={lat[len(lat)//2]:.1f}s max={lat[-1]:.1f}s "
      f"(窗口 {WINDOW}s → 客户端时长应 ≥ {WINDOW:.0f}s 且全部成功)", flush=True)
print(f"EXPECT: ok={NCONC}/{NCONC}（无限重试保证最终成功）", flush=True)
time.sleep(0.5)
srv.shutdown()
sys.exit(0 if ok == NCONC else 1)
