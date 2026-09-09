#!/bin/bash
# P1 v2: SIGSTOP 挂死守护 → 看护者心跳过期 + IPC 无响应 → SIGKILL 处决 → respawn 新 pid
# 关键修正：注册表文件（<port>.pid）内容是 InstanceInfo JSON——pid 须从 JSON 提取；
# 看护者 stdout 存档，以日志行实证挂死判定→处决→respawn 全链路
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORT=59981
CFG=/root/probe/configs/p1.toml
WDLOG=/root/probe/p1_watchdog.log
PASS=0; FAIL=0
say(){ echo "[$1] $2"; }
ok(){ PASS=$((PASS+1)); say PASS "$1"; }
bad(){ FAIL=$((FAIL+1)); say FAIL "$1"; }
reg_pid(){ grep -m1 '"pid"' $RUN/$1.pid 2>/dev/null | grep -o '[0-9]*' || echo ""; }
cleanup(){
  [ -n "${WD:-}" ] && kill -9 $WD 2>/dev/null
  [ -n "${D0:-}" ] && kill -CONT $D0 2>/dev/null; kill -9 $D0 2>/dev/null
  [ -n "${D1:-}" ] && kill -9 $D1 2>/dev/null
  rm -f $RUN/* /dev/shm/aproxy-heart-$PORT
}
trap cleanup EXIT

mkdir -p $RUN /root/probe/configs
rm -f $RUN/* /dev/shm/aproxy-heart-$PORT $WDLOG
cat > $CFG <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$PORT"
EOF

env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
D0=$!
READY=0
for i in $(seq 1 50); do
  if [ -f $RUN/$PORT.pid ] && [ -S $RUN/$PORT.sock ] && \
     timeout 2 bash -c "</dev/tcp/127.0.0.1/$PORT" 2>/dev/null; then
    READY=1; break
  fi
  sleep 0.2
done
REG0=$(reg_pid $PORT)
[ $READY = 1 ] && [ "$REG0" = "$D0" ] && ok "守护启动且注册表 pid 与进程一致 (pid=$D0)" \
  || { say INFO "READY=$READY REG0='$REG0'"; bad "守护未就绪或注册表 pid 不符"; exit 1; }

env APROXY_RUN_DIR=$RUN APROXY_WATCHDOG_SCAN_SECS=1 $BIN --daemon-watchdog > $WDLOG 2>&1 &
WD=$!
sleep 4
grep -qa "收养实例" $WDLOG && ok "看护者已收养实例（日志实证）" || bad "收养日志缺失"
[ -f $RUN/watchdog.claim ] && ok "看护者接管 claim" || bad "claim 未出现"

kill -STOP $D0
say INFO "SIGSTOP 已挂起 pid=$D0；等心跳过期(≤30s)+ping 3s+SIGKILL+respawn"
t0=$(date +%s)
NEWPID=""
for i in $(seq 1 90); do
  sleep 1
  NP=$(reg_pid $PORT)
  if [ -n "$NP" ] && [ "$NP" != "$D0" ]; then NEWPID=$NP; break; fi
done
elapsed=$(($(date +%s)-t0))
D1=$NEWPID
[ -n "$NEWPID" ] && ok "respawn 新 pid=$NEWPID（耗时 ${elapsed}s）" || { say INFO "$(cat $WDLOG)"; bad "90s 内未 respawn（挂死处决失效）"; exit 1; }

sleep 1
if [ -S $RUN/$PORT.sock ] && timeout 2 bash -c "</dev/tcp/127.0.0.1/$PORT" 2>/dev/null; then
  ok "新实例 socket+TCP 就绪"
else
  bad "新实例 socket 或 TCP 不通"
fi
# 旧守护已被处决（SIGKILL 胜过 SIGSTOP——处决后才可能死亡）
if kill -0 $D0 2>/dev/null; then bad "旧守护仍存活"; kill -9 $D0; else ok "旧守护已被处决"; fi
if kill -0 $WD 2>/dev/null; then ok "看护者仍在任"; else bad "看护者异常退出"; fi
# 日志链路实证：挂死判定 → 终止 → 重拉 → 就绪
grep -qa "判定挂死" $WDLOG && ok "日志实证: 心跳过期+IPC 无响应→判定挂死" || bad "日志缺『判定挂死』"
grep -qa "实例已重拉并就绪" $WDLOG && ok "日志实证: respawn 完成并重新收养" || bad "日志缺『实例已重拉并就绪』"
say INFO "--- 看护者日志全文 ---"
cat $WDLOG
say INFO "P1 结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
