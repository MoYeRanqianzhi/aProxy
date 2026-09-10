#!/bin/bash
# P6 v2: 死亡事件风暴——3 实例同时被杀 → 批量 respawn；pid 从注册表 JSON 提取
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORTS="59984 59985 59986"
WDLOG=/root/probe/p6_watchdog.log
PASS=0; FAIL=0
say(){ echo "[$1] $2"; }
ok(){ PASS=$((PASS+1)); say PASS "$1"; }
bad(){ FAIL=$((FAIL+1)); say FAIL "$1"; }
reg_pid(){ grep -m1 '"pid"' $RUN/$1.pid 2>/dev/null | grep -o '[0-9]*' || echo ""; }
WD=""
declare -A OLD NEW
cleanup(){
  [ -n "${WD:-}" ] && kill -9 $WD 2>/dev/null
  for p in $PORTS; do
    [ -n "${OLD[$p]:-}" ] && kill -9 ${OLD[$p]} 2>/dev/null
    NP=$(reg_pid $p); [ -n "$NP" ] && [ "$NP" != "${OLD[$p]:-}" ] && kill -9 $NP 2>/dev/null
    rm -f /dev/shm/aproxy-heart-$p
  done
  rm -f $RUN/*
}
trap cleanup EXIT

mkdir -p $RUN
rm -f $RUN/* $WDLOG
for p in $PORTS; do
  cat > /root/probe/configs/storm$p.toml <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$p"
EOF
  env APROXY_RUN_DIR=$RUN $BIN --config /root/probe/configs/storm$p.toml --daemon-child >/dev/null 2>&1 &
  OLD[$p]=$!
done
READY=0
for i in $(seq 1 60); do
  READY=1
  for p in $PORTS; do
    [ -f $RUN/$p.pid ] && [ -S $RUN/$p.sock ] || READY=0
  done
  [ $READY = 1 ] && break
  sleep 0.2
done
# 注册表 pid 与进程一致性
for p in $PORTS; do
  RP=$(reg_pid $p)
  [ "$RP" = "${OLD[$p]}" ] || bad "端口 $p 注册表 pid($RP)≠进程(${OLD[$p]})"
done
[ $READY = 1 ] && ok "3 守护全部就绪且注册表一致" || { bad "守护未全部就绪"; exit 1; }

env APROXY_RUN_DIR=$RUN APROXY_WATCHDOG_SCAN_SECS=1 $BIN --daemon-watchdog > $WDLOG 2>&1 &
WD=$!
sleep 4
grep -qa "收养实例" $WDLOG && ok "看护者已收养（日志实证）" || bad "收养日志缺失"

t0=$(date +%s)
for p in $PORTS; do kill -9 ${OLD[$p]} 2>/dev/null; done
say INFO "3 守护已同时 SIGKILL，等待批量 respawn"

NEW_ALL=1
for i in $(seq 1 60); do
  NEW_ALL=1
  for p in $PORTS; do
    NP=$(reg_pid $p)
    [ -n "$NP" ] && [ "$NP" != "${OLD[$p]}" ] && [ -S $RUN/$p.sock ] || NEW_ALL=0
    NEW[$p]=$NP
  done
  [ $NEW_ALL = 1 ] && break
  sleep 1
done
elapsed=$(($(date +%s)-t0))
if [ $NEW_ALL = 1 ]; then
  ok "3 实例全部 respawn 为新进程（耗时 ${elapsed}s）"
  for p in $PORTS; do say INFO "端口 $p: ${OLD[$p]} → ${NEW[$p]}"; done
  N=$(grep -ca "实例已重拉并就绪" $WDLOG || echo 0)
  [ "$N" -ge 3 ] && ok "日志实证 3 次重拉就绪" || say INFO "重拉就绪日志 $N 条（人工复核）"
else
  for p in $PORTS; do
    NP=$(reg_pid $p)
    if [ -z "$NP" ] || [ "$NP" = "${OLD[$p]}" ]; then bad "端口 $p 未 respawn"; fi
  done
  say INFO "--- 看护者日志 ---"; cat $WDLOG
fi
kill -0 $WD 2>/dev/null && ok "看护者风暴后仍在任" || bad "看护者风暴后退出"
say INFO "P6 结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
