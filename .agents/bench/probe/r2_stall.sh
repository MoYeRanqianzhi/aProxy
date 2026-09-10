#!/bin/bash
# R2: 长挂重试——上游恒 502（永不恢复），客户端单请求 -m 90s 挂着不断流；
# 观测 aproxy 日志重试持续、进程存活、客户端连接保持（超时退出码 28）
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORT=59989
CFG=/root/probe/configs/r2.toml
LOG=/root/.aproxy/logs/$PORT.log
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
: > $LOG
cat > $CFG <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$PORT"
EOF

python3 /root/probe/mock_stall.py >/dev/null 2>&1 &
MP=$!
sleep 0.8
env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
PID=$!
for i in $(seq 1 40); do [ -S $RUN/$PORT.sock ] && break; sleep 0.1; done

# 90s 长请求（上游恒 502 → aproxy 无限重试，客户端保持连接）
t0=$(date +%s)
curl -s -o /dev/null -X POST http://127.0.0.1:$PORT/v1/chat -d '{}' -m 90
CURL_RC=$?
wall=$(($(date +%s)-t0))
[ $CURL_RC = 28 ] && ok "客户端 90s 超时退出（rc=28，连接保持到超时）" || \
  { say INFO "curl rc=$CURL_RC wall=${wall}s"; bad "客户端 rc 异常（预期 28）"; }
[ $wall -ge 88 ] && ok "请求全程保持 ${wall}s（未提前断流）" || bad "请求提前断流（仅 ${wall}s）"
kill -0 $PID 2>/dev/null && ok "重试 90s 后守护存活" || bad "守护在重试风暴中死亡"
RETR=$(grep -ac "重试\|retry\|502" $LOG 2>/dev/null || echo 0)
say INFO "aprox­y 日志重试相关条目数=$RETR"
[ "$RETR" -gt 50 ] && ok "重试持续记录（$RETR 条）" || say INFO "重试条目 $RETR（视日志粒度人工复核）"
# 内存粗查
RSS=$(ps -o rss= -p $PID 2>/dev/null || echo 0)
say INFO "重试 90s 后守护 RSS=${RSS}KB"
say INFO "R2 结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
