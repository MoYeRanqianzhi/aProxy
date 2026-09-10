#!/bin/bash
# 压力实测主编排：release 构建 → 逐场景执行 → 汇总
# 场景：P1 挂死处决 / P2 give_up+P3 zombie / P6 死亡风暴 / S 生命周期 / R1 重试风暴 /
#       R2 长挂重试 / R4 隔离性 / M1 浸泡
set -u
cd /root/aproxy-unixfix
export PATH=$HOME/.cargo/bin:$PATH
PROBE=/root/probe
mkdir -p $PROBE/configs $PROBE/run

echo "=========== release 构建 ==========="
cargo build --release 2>&1 | tail -2
cp target/release/aproxy $PROBE/aproxy

declare -A RESULTS
run(){
  local name=$1 script=$2
  echo ""
  echo "=========== $name ==========="
  bash $script 2>&1 | tee /root/probe/$name.log
  local rc=${PIPESTATUS[0]}
  RESULTS[$name]=$rc
}

run "P1 挂死处决"     /root/probe/p1_hang_kill.sh
run "P2 give_up+P3 zombie" /root/probe/p2_giveup.sh
run "P6 死亡风暴"     /root/probe/p6_storm.sh
run "S 生命周期"      /root/probe/s_cycle.sh
run "R1 重试风暴"     /root/probe/r1_storm.sh
run "R2 长挂重试"     /root/probe/r2_stall.sh
run "R3 拒绝连接重试" /root/probe/r3_refused.sh
run "R4 隔离性"       /root/probe/r4_iso.sh
run "M1 浸泡"         /root/probe/m1_soak.sh

echo ""
echo "=================== 汇总 ==================="
TOTAL_FAIL=0
for k in "P1 挂死处决" "P2 give_up+P3 zombie" "P6 死亡风暴" "S 生命周期" "R1 重试风暴" "R2 长挂重试" "R3 拒绝连接重试" "R4 隔离性" "M1 浸泡"; do
  rc=${RESULTS[$k]}
  [ "$rc" = 0 ] && echo "  PASS  $k" || { echo "  FAIL($rc) $k"; TOTAL_FAIL=$((TOTAL_FAIL+1)); }
done
# 遗留资源检查
echo "--- /dev/shm 心跳残留 ---"
ls /dev/shm/aproxy-heart-* 2>/dev/null || echo "  (无残留)"
echo "--- 崩溃轮次后的 zombie（应为 0，看护者均已退出收割交给 init）---"
ps -eo stat,comm | awk '$1 ~ /Z/ && $2 ~ /aproxy/' | wc -l
echo "TOTAL_FAIL=$TOTAL_FAIL"
exit $TOTAL_FAIL
