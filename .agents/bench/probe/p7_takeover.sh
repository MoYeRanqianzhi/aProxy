#!/bin/bash
# P7: claim 接管与双看护者窗口——SIGSTOP 假死前任 → 接管者接管（unix 不杀前任）
# → SIGCONT 恢复前任 → 观察双看护者共存与 claim 覆写竞争（审查隐患 D5 实证）
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
# B2：claim 过期 → old_alive（恒 true + starttime 匹配）→ terminate_verified(unix no-op) → 接管
env APROXY_RUN_DIR=$RUN APROXY_WATCHDOG_SCAN_SECS=1 $BIN --daemon-watchdog > $LOGB 2>&1 &
WDB=$!
B_OK=0
for i in $(seq 1 20); do [ "$(claim_pid)" = "$WDB" ] && { B_OK=1; break; }; sleep 0.5; done
[ $B_OK = 1 ] && ok "看护者 B2 已接管 claim (pid=$WDB)" || { say INFO "claim=$(cat $RUN/watchdog.claim 2>/dev/null)"; bad "B2 未接管（claim 心跳过期判定失效？）"; }
grep -qa "前任看护者仍在但心跳过期" $LOGB && ok "日志实证: B2 检出假死前任并尝试终止（unix no-op 未杀）" || say INFO "B2 未检出假死前任（时序）"
grep -qa "已有在任的看护者" $LOGB && bad "B2 让位退出（接管未发生）"

# 恢复 A（SIGCONT）——观察夺权后行为
kill -CONT $WDA
say INFO "SIGCONT 恢复 A，观察 10s：夺权检测 / claim 双写竞争"
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
# 无夺权检测 → A 醒来继续跑（双看护者）；claim 双写 → 出现 ≥2 个 pid
if [ "$ALIVE_A" = 1 ] && [ "$ALIVE_B" = 1 ]; then
  ok "双看护者共存实证（A 恢复后不退出，unix 不杀假死前任 + 无运行期夺权检测）"
else
  ok "单看护者收敛（A 恢复后自退出或被杀）——需查日志归因"
fi
[ $(echo $PID_SEEN | wc -w) -ge 2 ] && ok "claim 覆写竞争实证（claim pid 在 $PID_SEEN 间翻转）" \
  || say INFO "claim pid 未翻转（$PID_SEEN——refresh_claim 覆写竞争未观测到，人工复核）"
say INFO "--- A 日志 ---"; cat $LOGA
say INFO "--- B 日志 ---"; cat $LOGB
say INFO "P7 结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
