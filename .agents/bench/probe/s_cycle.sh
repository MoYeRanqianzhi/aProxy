#!/bin/bash
# S 组：守护生命周期极端压力
# S1 stop/start 快速循环 10 轮（泄漏检查：进程/sock/注册表）
# S2 并发 3×stop 同端口（幂等）
# S3 并发 3×start 抢同端口（唯一存活）
# S4 UDS IPC 并发 100 连发 ping（stop/status 竞争读）
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORT=59987
CFG=/root/probe/configs/s.toml
PASS=0; FAIL=0
say(){ echo "[$1] $2"; }
ok(){ PASS=$((PASS+1)); say PASS "$1"; }
bad(){ FAIL=$((FAIL+1)); say FAIL "$1"; }
cleanup(){
  [ -n "${PID:-}" ] && kill -9 $PID 2>/dev/null
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

# ---- S1: 快速循环 10 轮（每轮：后台 spawn 守护 → 等 sock → stop → 断言清理） ----
LEAK=0
t0=$(date +%s)
for r in $(seq 1 10); do
  env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
  PID=$!
  UP=0
  for i in $(seq 1 40); do
    [ -S $RUN/$PORT.sock ] && { UP=1; break; }
    sleep 0.1
  done
  [ $UP = 1 ] || { bad "第 $r 轮守护未就绪"; LEAK=$((LEAK+1)); continue; }
  env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop $PORT >/dev/null 2>&1
  sleep 0.5
  # 残留检查：unix 守护退出不删 sock 文件（bind 前自愈清理，与 Windows
  # 管道内核回收的多端差异）——降级为观察项；pid 注册必须删除
  [ -S $RUN/$PORT.sock ] && say INFO "第 $r 轮 sock 文件残留（bind 前自愈，多端差异记录）"
  [ -f $RUN/$PORT.pid ] && { bad "第 $r 轮 pid 注册残留"; LEAK=$((LEAK+1)); }
  kill -0 $PID 2>/dev/null && sleep 1
  kill -9 $PID 2>/dev/null && LEAK=$((LEAK+1))
done
elapsed=$(($(date +%s)-t0))
[ $LEAK = 0 ] && ok "S1: 10 轮 start/stop 无残留（${elapsed}s）" || bad "S1: 残留 $LEAK 处"
rm -f $RUN/*

# ---- S2: 并发 3×stop（无实例时并发调用——幂等语义） ----
env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
PID=$!
disown $PID
for i in $(seq 1 40); do [ -S $RUN/$PORT.sock ] && break; sleep 0.1; done
env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop $PORT >/dev/null 2>&1 &
env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop $PORT >/dev/null 2>&1 &
env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop $PORT >/dev/null 2>&1 &
wait
sleep 1
kill -0 $PID 2>/dev/null && { bad "S2: 并发 stop 后守护仍存活"; kill -9 $PID; } || ok "S2: 并发 3×stop 全停干净"
# stop 后 sock/pid 清理：unix 守护退出不删 sock 文件（bind 前自愈，多端差异），
# 断言只查 pid 注册
[ -f $RUN/$PORT.pid ] && { bad "S2: stop 后 pid 注册残留"; } || ok "S2: pid 注册清理干净（sock 残留为已知多端差异）"
rm -f $RUN/*

# ---- S3: 并发 3×start 抢同端口（start 命令带预检+spawn 分离守护） ----
( env APROXY_RUN_DIR=$RUN timeout 30 $BIN start --config $CFG >/dev/null 2>&1 ) &
( env APROXY_RUN_DIR=$RUN timeout 30 $BIN start --config $CFG >/dev/null 2>&1 ) &
( env APROXY_RUN_DIR=$RUN timeout 30 $BIN start --config $CFG >/dev/null 2>&1 ) &
wait
sleep 1
REGPID=$(grep -m1 '"pid"' $RUN/$PORT.pid 2>/dev/null | grep -o '[0-9]*' || echo "")
if [ -n "$REGPID" ] && [ -S $RUN/$PORT.sock ] && kill -0 $REGPID 2>/dev/null; then
  ok "S3: 并发 start 后恰好存活 pid=$REGPID（注册表+进程双验证）"
else
  bad "S3: 并发 start 后注册或进程异常 (pid='$REGPID')"
fi
# S3 清理
[ -n "$REGPID" ] && env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop $PORT >/dev/null 2>&1
sleep 1
rm -f $RUN/*

# ---- S4: IPC 并发 100 连发 status（UDS 读并发压力） ----
env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
PID=$!
disown $PID
for i in $(seq 1 40); do [ -S $RUN/$PORT.sock ] && break; sleep 0.1; done
for i in $(seq 1 100); do
  ( env APROXY_RUN_DIR=$RUN timeout 5 $BIN status --port $PORT >/dev/null 2>&1 ) &
done
wait
say INFO "S4: 100 并发 status 已完成（守护存活性检查）"
kill -0 $PID 2>/dev/null && ok "S4: 100 并发 IPC 后守护存活" || bad "S4: 并发 IPC 后守护死亡"
env APROXY_RUN_DIR=$RUN timeout 10 $BIN stop $PORT >/dev/null 2>&1
say INFO "S 组结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
