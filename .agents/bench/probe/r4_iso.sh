#!/bin/bash
# R4 v2: 起两个 aproxy 实例（A=59991→bad mock 59997、B=59992→good mock 59996）→ 隔离性验证
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
mkdir -p $RUN
rm -f $RUN/* /dev/shm/aproxy-heart-5999{1,2}
cat > /root/probe/configs/r4a.toml <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:59991"
EOF
cat > /root/probe/configs/r4b.toml <<EOF
base_url = "http://127.0.0.1:59996"
listen_addr = "127.0.0.1:59992"
EOF
env APROXY_RUN_DIR=$RUN $BIN --config /root/probe/configs/r4a.toml --daemon-child >/dev/null 2>&1 &
PA=$!
disown $PA
env APROXY_RUN_DIR=$RUN $BIN --config /root/probe/configs/r4b.toml --daemon-child >/dev/null 2>&1 &
PB=$!
disown $PB
READY=0
for i in $(seq 1 60); do
  [ -S $RUN/59991.sock ] && [ -S $RUN/59992.sock ] && { READY=1; break; }
  sleep 0.1
done
if [ $READY = 0 ]; then echo "FAIL: R4 实例未就绪"; kill -9 $PA $PB 2>/dev/null; exit 1; fi
python3 /root/probe/r4_isolation.py 8
RC=$?
env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop 59991 >/dev/null 2>&1
env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop 59992 >/dev/null 2>&1
kill -9 $PA $PB 2>/dev/null
rm -f $RUN/* /dev/shm/aproxy-heart-5999{1,2}
exit $RC
