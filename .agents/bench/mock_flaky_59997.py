import http.server, json, socketserver, socket, threading, time

# 前 8 秒所有 POST 返回 502，之后恢复 200——测 aproxy 的无限重试
state = {"fail_until": time.time() + 8}


def recover():
    time.sleep(8)
    state["fail_until"] = 0


threading.Thread(target=recover, daemon=True).start()


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
            resp = json.dumps({"ok": True, "after": 8}).encode()
            self.send_response(200)
            self.send_header("Content-Length", str(len(resp)))
            self.end_headers()
            self.wfile.write(resp)

    def log_message(self, *a):
        pass


class TS(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


TS(("127.0.0.1", 59997), Handler).serve_forever()
