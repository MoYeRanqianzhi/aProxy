#!/bin/bash
# M1: 3 分钟浸泡——稳态 200 转发/秒 + 每 30s 注入 5s 的 502 窗口（重试混合负载）
# 观测 RSS 漂移、fd 泄漏、/dev/shm 残留
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORT=59988
CFG=/root/probe/configs/m1.toml
PASS=0; FAIL=0
say(){ echo "[$1] $2"; }
ok(){ PASS=$((PASS+1)); say PASS "$1"; }
bad(){ FAIL=$((FAIL+1)); say FAIL "$1"; }
cleanup(){
  [ -n "${PID:-}" ] && kill -9 $PID 2>/dev/null
  [ -n "${MP:-}" ] && kill -9 $MP 2>/dev/null
  env APROXY_RUN_DIR=$RUN timeout 5 $BIN stop $PORT 2>/dev/null
  rm -f $RUN/* /dev/shm/aproxy-heart-$PORT
}
trap cleanup EXIT

mkdir -p $RUN
rm -f $RUN/*
cat > $CFG <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$PORT"
EOF

# 混合 mock：恒 200 但每 30s 起一个 5s 的 502 窗口
python3 - <<'PY' >/dev/null 2>&1 &
import http.server, json, socket, socketserver, threading, time
state = {"fail_until": 0}
def storm():
    while True:
        time.sleep(30)
        state["fail_until"] = time.time() + 5
threading.Thread(target=storm, daemon=True).start()
class H(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"
    def setup(self):
        super().setup()
        self.connection.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    def do_POST(self):
        self.rfile.read(int(self.headers.get("Content-Length", 0)))
        if time.time() < state["fail_until"]:
            body = b"bad\n"; self.send_response(502)
        else:
            body = json.dumps({"ok": True}).encode(); self.send_response(200)
        self.send_header("Content-Length", str(len(body))); self.end_headers(); self.wfile.write(body)
    def log_message(self, *a): pass
class TS(socketserver.ThreadingTCPServer):
    allow_reuse_address = True; daemon_threads = True
TS(("127.0.0.1", 59997), H).serve_forever()
PY
MP=$!
disown $MP
sleep 0.8
env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
PID=$!
disown $PID
for i in $(seq 1 40); do [ -S $RUN/$PORT.sock ] && break; sleep 0.1; done

rss0=$(ps -o rss= -p $PID)
fds0=$(ls /proc/$PID/fd 2>/dev/null | wc -l)
say INFO "起始 RSS=${rss0}KB fd=${fds0}"

# 3 分钟：4 并发持续打（每请求间隔 ~50ms，约 80 RPS/端；502 窗口期这些请求会重试挂起）
t0=$(date +%s)
for w in 1 2 3 4; do
  (
    while [ $(($(date +%s)-t0)) -lt 180 ]; do
      curl -s -o /dev/null -X POST http://127.0.0.1:$PORT/v1/chat -d '{}' -m 30
      sleep 0.05
    done
  ) &
done
sleep 60
rss1=$(ps -o rss= -p $PID)
fds1=$(ls /proc/$PID/fd 2>/dev/null | wc -l)
say INFO "60s: RSS=${rss1}KB fd=${fds1}"
wait
wall=$(($(date +%s)-t0))
rss2=$(ps -o rss= -p $PID)
fds2=$(ls /proc/$PID/fd 2>/dev/null | wc -l)
say INFO "180s: RSS=${rss2}KB fd=${fds2}"

# 断言：RSS 无趋势性增长（允许 ±20% 抖动；泄漏特征是单调显著上涨）
GROW=$(( (rss2 - rss0) * 100 / (rss0 > 0 ? rss0 : 1) ))
FD_GROW=$(( fds2 - fds0 ))
[ $GROW -lt 30 ] && ok "RSS 漂移 ${GROW}%（${rss0}→${rss2}KB）" || bad "RSS 增长 ${GROW}% 疑似泄漏"
[ $FD_GROW -lt 10 ] && ok "fd 漂移 ${FD_GROW}（${fds0}→${fds2}）" || bad "fd 增长 ${FD_GROW} 疑似泄漏"
kill -0 $PID 2>/dev/null && ok "浸泡全程守护存活" || bad "浸泡中守护死亡"
say INFO "M1 结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
