#!/bin/bash
# R3 v2: 起 aproxy 实例（59990 → mock 59997）→ 上游拒绝连接 8s（mock 延迟上线）→ 重试至恢复
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORT=59990
mkdir -p $RUN
rm -f $RUN/* /dev/shm/aproxy-heart-$PORT
cat > /root/probe/configs/r3.toml <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$PORT"
EOF
env APROXY_RUN_DIR=$RUN $BIN --config /root/probe/configs/r3.toml --daemon-child >/dev/null 2>&1 &
PID=$!
disown $PID
for i in $(seq 1 50); do [ -S $RUN/$PORT.sock ] && break; sleep 0.1; done
if [ ! -S $RUN/$PORT.sock ]; then echo "FAIL: R3 aproxy 实例未就绪"; kill -9 $PID 2>/dev/null; exit 1; fi
python3 /root/probe/r_retry.py refused 8 10
RC=$?
env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop $PORT >/dev/null 2>&1
kill -9 $PID 2>/dev/null
rm -f $RUN/* /dev/shm/aproxy-heart-$PORT
exit $RC
