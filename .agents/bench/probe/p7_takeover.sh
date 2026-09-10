#!/bin/bash
# P7: claim 接管与假死前任处决——SIGSTOP 假死前任 → claim 心跳过期 →
# 接管者验证身份后 SIGKILL 前任并接管（S2 修复后 terminate_verified 已实装）
# → SIGCONT 确认前任不可恢复、claim 单一 pid 无覆写竞争
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORT=59980
CFG=/root/probe/configs/p7.toml
LOGA=/root/probe/p7_wd_a.log
LOGB=/root/probe/p7_wd_b.log
PASS=0; FAIL=0
say(){ echo "[$1] $2"; }
ok(){ PASS=$((PASS+1)); say PASS "$1"; }
bad(){ FAIL=$((FAIL+1)); say FAIL "$1"; }
claim_pid(){ grep -oE '"pid": ?[0-9]+' $RUN/watchdog.claim 2>/dev/null | grep -oE '[0-9]+' || echo ""; }
cleanup(){
  [ -n "${WDA:-}" ] && kill -CONT $WDA 2>/dev/null; kill -9 $WDA 2>/dev/null
  [ -n "${WDB:-}" ] && kill -9 $WDB 2>/dev/null
  [ -n "${WDB1:-}" ] && kill -9 $WDB1 2>/dev/null
  [ -n "${D0:-}" ] && kill -9 $D0 2>/dev/null
  rm -f $RUN/* /dev/shm/aproxy-heart-$PORT
}
trap cleanup EXIT

mkdir -p $RUN
rm -f $RUN/* $LOGA $LOGB
cat > $CFG <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$PORT"
EOF
env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
D0=$!
for i in $(seq 1 50); do [ -S $RUN/$PORT.sock ] && break; sleep 0.1; done

# 看护者 A 在任
env APROXY_RUN_DIR=$RUN APROXY_WATCHDOG_SCAN_SECS=1 $BIN --daemon-watchdog > $LOGA 2>&1 &
WDA=$!
for i in $(seq 1 20); do [ "$(claim_pid)" = "$WDA" ] && break; sleep 0.3; done
[ "$(claim_pid)" = "$WDA" ] && ok "看护者 A 在任 (pid=$WDA)" || { bad "A 未接管 claim"; exit 1; }

# 假死 A（SIGSTOP——心跳停写，进程活着）
kill -STOP $WDA
say INFO "看护者 A 已 SIGSTOP（假死）；先验证 claim 新鲜期 B 让位（唯一性），再等 90s 过期后接管"
# B1：claim 新鲜（<90s）→ 应让位退出（正确语义）
env APROXY_RUN_DIR=$RUN APROXY_WATCHDOG_SCAN_SECS=1 $BIN --daemon-watchdog > $LOGB 2>&1 &
WDB1=$!
sleep 3
if grep -qa "已有在任的看护者" $LOGB && ! kill -0 $WDB1 2>/dev/null; then
  ok "B1 让位退出（claim 新鲜期唯一性正确）"
else
  bad "B1 未按唯一性让位"
fi
# 等 claim 心跳过期（3×watchdog_heartbeat_secs=90s）
say INFO "等待 claim 心跳过期（90s）……"
sleep 95
# B2：claim 过期 → 身份验证（is_aproxy_process + starttime）通过 → SIGKILL 前任 → 接管
env APROXY_RUN_DIR=$RUN APROXY_WATCHDOG_SCAN_SECS=1 $BIN --daemon-watchdog > $LOGB 2>&1 &
WDB=$!
B_OK=0
for i in $(seq 1 20); do [ "$(claim_pid)" = "$WDB" ] && { B_OK=1; break; }; sleep 0.5; done
[ $B_OK = 1 ] && ok "看护者 B2 已接管 claim (pid=$WDB)" || { say INFO "claim=$(cat $RUN/watchdog.claim 2>/dev/null)"; bad "B2 未接管（claim 心跳过期判定失效？）"; }
grep -qa "前任看护者仍在但心跳过期" $LOGB && ok "日志实证: B2 检出假死前任并处决后接管" || say INFO "B2 未检出假死前任（时序）"
grep -qa "已有在任的看护者" $LOGB && bad "B2 让位退出（接管未发生）"

# 恢复 A（SIGCONT）——修复后语义：接管者已 SIGKILL 前任，SIGCONT 打在
# 不存在的 pid 上（no-op），前任不可复活；claim 全程只有接管者一个 pid
kill -CONT $WDA 2>/dev/null
say INFO "SIGCONT 恢复 A（预期打在已死 pid 上无效果），观察 10s"
PID_SEEN=""
for i in $(seq 1 20); do
  sleep 0.5
  CP=$(claim_pid)
  case "$PID_SEEN" in *"$CP "*) ;; *) PID_SEEN="$PID_SEEN$CP ";; esac
done
kill -0 $WDA 2>/dev/null && ALIVE_A=1 || ALIVE_A=0
kill -0 $WDB 2>/dev/null && ALIVE_B=1 || ALIVE_B=0
say INFO "10s 内 claim 出现过的 pid: $PID_SEEN"
say INFO "存活: A=$ALIVE_A B=$ALIVE_B"
[ "$ALIVE_A" = 1 ] && bad "前任看护者在接管后仍存活（terminate_verified 未生效或被 SIGCONT 复活）" \
  || ok "接管即杀前任（SIGCONT 不可恢复）"
[ "$ALIVE_B" = 1 ] && ok "接管者持续在任" || bad "接管者意外退出"
[ "$PID_SEEN" = "$WDB " ] && ok "claim 单一 pid 无覆写竞争" \
  || bad "claim 出现多个 pid: $PID_SEEN（refresh_claim 归属校验失效？）"
say INFO "--- A 日志 ---"; cat $LOGA
say INFO "--- B 日志 ---"; cat $LOGB
say INFO "P7 结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
