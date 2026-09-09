#!/bin/bash
# 补跑编排：v2 中失效/卡住的场景用修正版重跑（R1/R3/R4 修正版 + S 修正版 + P7 新场景）
export PATH=$HOME/.cargo/bin:$PATH
PROBE=/root/probe
declare -A RESULTS
run(){
  local key=$1 script=$2
  echo ""
  echo "=========== $key ==========="
  bash $script 2>&1 | tee /root/probe/fix_${key}.out
  RESULTS[$key]=${PIPESTATUS[0]}
}
run "R1" /root/probe/r1_storm.sh
run "R3" /root/probe/r3_refused.sh
run "R4" /root/probe/r4_iso.sh
run "S"  /root/probe/s_cycle.sh
run "M1" /root/probe/m1_soak.sh
run "P7" /root/probe/p7_takeover.sh
echo ""
echo "=================== 补跑汇总 ==================="
TOTAL_FAIL=0
for k in R1 R3 R4 S M1 P7; do
  rc=${RESULTS[$k]}
  [ "$rc" = 0 ] && echo "  PASS  $k" || { echo "  FAIL($rc) $k"; TOTAL_FAIL=$((TOTAL_FAIL+1)); }
done
echo "FIX_TOTAL_FAIL=$TOTAL_FAIL"
