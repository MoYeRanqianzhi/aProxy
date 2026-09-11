#!/bin/bash
# 502 重试路径完全自包含验证：mock 窗口 → 经 aproxy → 期望「阻塞在重试直至恢复」
# 用法: retry_502.sh <aproxoy端口> [max_backoff配置描述]
PORT=${1:-59999}
echo "=== 目标实例端口 $PORT"
cd /root/aproxy-fix
nohup python3 /tmp/bench/mock_flaky3.py > /tmp/bench/flaky-e2e2.log 2>&1 &
MP=$!
sleep 1.0
T0=$(date +%s.%N)
curl -s -X POST http://127.0.0.1:$PORT/v1/chat -d '{}' -m 60 \
  -w "HTTP:%{http_code} time=%{time_total}s\n"
T1=$(date +%s.%N)
kill $MP 2>/dev/null
echo "等待耗时: $(echo "$T1 - $T0" | bc)s（预期 ≥6s: 窗口剩余 + 重试间隔）"
