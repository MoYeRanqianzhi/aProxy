# 端口占用者：bind 后什么都不做，用于制造 respawn 必败（P2）
import socket
import sys

port = int(sys.argv[1])
s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port))
s.listen(5)
print(f"dummy holding {port}", flush=True)
try:
    while True:
        conn, _ = s.accept()
        conn.close()
except KeyboardInterrupt:
    pass
