# 挂死 mock：所有 POST 恒 502，永不恢复（R2 长挂重试用）
import http.server
import socket

PORT = 59997


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)

    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        body = b"upstream overloaded\n"
        self.send_response(502)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


class TS(http.server.ThreadingHTTPServer):
    allow_reuse_address = True
    daemon_threads = True


TS(("127.0.0.1", PORT), Handler).serve_forever()
