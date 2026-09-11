#!/bin/bash
# 无限重试端到端一次性测试：启动 502 窗口 mock → 经 aproxy 打请求 → 断言重试至恢复
cd /root/aproxy-fix
python3 /tmp/bench/mock_flaky3.py > /tmp/bench/flaky-once.log 2>&1 &
MP=$!
sleep 1.2
echo "t0=$(date +%H:%M:%S)"
curl -s -X POST http://127.0.0.1:59999/v1/chat -d '{}' -m 60 \
  -w "HTTP:%{http_code} time=%{time_total}s\n"
echo "t1=$(date +%H:%M:%S)"
kill $MP 2>/dev/null
echo "--- 59999 日志（本次窗口）:"
tail -20 ~/.aproxy/logs/59999.log | grep -aE "16:1[7-9]|16:[2-5]" | tail -12
