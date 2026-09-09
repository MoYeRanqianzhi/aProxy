#!/bin/bash
# P2: respawn 连败退避 → 达 max_restarts(5) → give_up 写 startup.log → 看护者闲置自灭
# P3: 顺带统计 respawn 失败留下的 zombie 堆积（看护者不收割子进程的实机数据）
set -u
BIN=/root/probe/aproxy
RUN=/root/probe/run
PORT=59983
DUMMY_PORT=59982
CFG=/root/probe/configs/p2.toml
CFG_BAD=/root/probe/configs/p2bad.toml
PASS=0; FAIL=0
say(){ echo "[$1] $2"; }
ok(){ PASS=$((PASS+1)); say PASS "$1"; }
bad(){ FAIL=$((FAIL+1)); say FAIL "$1"; }
cleanup(){
  [ -n "${WD:-}" ] && kill -9 $WD 2>/dev/null
  [ -n "${D0:-}" ] && kill -9 $D0 2>/dev/null
  [ -n "${DP:-}" ] && kill -9 $DP 2>/dev/null
  rm -f $RUN/* /dev/shm/aproxy-heart-$PORT
}
trap cleanup EXIT

mkdir -p $RUN /root/probe/configs
rm -f $RUN/* /dev/shm/aproxy-heart-$PORT
cat > $CFG <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$PORT"
EOF

# 1. 正常启动 → .restore 自动产生 → kill -9 制造崩溃残留
env APROXY_RUN_DIR=$RUN $BIN --config $CFG --daemon-child >/dev/null 2>&1 &
D0=$!
for i in $(seq 1 50); do
  [ -f $RUN/$PORT.restore ] && break
  sleep 0.2
done
[ -f $RUN/$PORT.restore ] && ok ".restore 已产生" || { bad ".restore 未产生"; exit 1; }
kill -9 $D0; wait $D0 2>/dev/null
sleep 1

# 2. 让 respawn 必败：.restore 的 args 里 --config 指向监听端口被 dummy 占用的配置
python3 /root/probe/dummy_port.py $DUMMY_PORT >/dev/null 2>&1 &
DP=$!
sleep 0.5
cat > $CFG_BAD <<EOF
base_url = "http://127.0.0.1:59997"
listen_addr = "127.0.0.1:$DUMMY_PORT"
EOF
# .restore 记录的 args 用初始配置路径：把初始配置文件内容换成坏端口
cp $CFG_BAD $CFG
say INFO "配置已换成被占用端口 $DUMMY_PORT，respawn 将持续失败"

# 3. 启动看护者，观测退避→give_up 全程（预计 ~60s：立即+1+2+4+8s 退避 + 每次 spawn/就绪尝试）
# startup.log 是追加日志（历史测试也会写）——以基线行数起算，只认本次新增
BASE=$(wc -l < ~/.aproxy/logs/startup.log 2>/dev/null || echo 0)
env APROXY_RUN_DIR=$RUN APROXY_WATCHDOG_SCAN_SECS=1 $BIN --daemon-watchdog >/dev/null 2>&1 &
WD=$!
t0=$(date +%s)
GAVE_UP=""
Z_COUNTS=""
for i in $(seq 1 90); do
  sleep 1
  Z=$(ps -eo stat,comm | awk '$1 ~ /Z/ && $2 ~ /aproxy/' | wc -l)
  Z_COUNTS="$Z_COUNTS ${Z}"
  NEW=$(tail -n +$((BASE + 1)) ~/.aproxy/logs/startup.log 2>/dev/null | grep -a "放弃自动重拉" | tail -1)
  [ -n "$NEW" ] && [ -z "$GAVE_UP" ] && GAVE_UP="$NEW"
  [ -n "$GAVE_UP" ] && break
done
elapsed=$(($(date +%s)-t0))
say INFO "zombie 数采样:$Z_COUNTS"
[ -n "$GAVE_UP" ] && ok "give_up 已宣告（${elapsed}s）: $GAVE_UP" || bad "90s 内未 give_up（重试语义不符）"
# give_up 后 .restore 应保留（人工兜底）
[ -f $RUN/$PORT.restore ] && ok ".restore 保留（人工兜底可用）" || bad ".restore 被误删"
# P3 断言：crashloop 期间 zombie 有积累（看护者不 wait 子进程的固有行为）
LASTZ=$(echo $Z_COUNTS | awk '{print $NF}')
say INFO "最终 zombie 数=$LASTZ（预期 >0：每次 respawn 失败留一个，看护者退出后由 init 收割）"
# 看护者闲置自灭（give_up 摘除全部 watch → empty_since → idle_exit...scan=1 时 idle_exit 默认 300s 太久，不强等）
say INFO "P2/P3 结果: PASS=$PASS FAIL=$FAIL"
exit $FAIL
