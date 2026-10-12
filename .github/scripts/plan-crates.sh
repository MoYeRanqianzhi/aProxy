#!/usr/bin/env bash
# 列出这次要发布到 crates.io 的 crate（两条发版线的发布与演练 job 共用）。
#
# 逐个查询 crates.io API 的 /api/v1/crates/<名>/<版本>，版本取工作区里的（发布前已对齐成 tag
# 版本）。200 = 该版本已存在，跳过；404 = 未发布（含 crate 本身不存在），列入待发布；其他状态码
# 或网络失败一律硬失败——只有「已存在」这一种情况可以跳过，不用 || 吞掉错误。于是发布 job 失败
# 后可以单独 re-run，已发布的不会重发。
#
# 参数按依赖顺序排列，被依赖的在前（aproxy 依赖 aproxy-envelope 与 aproxy-desktop 的 registry
# 版本）。结果以 `args=-p <名> …`（全部已发布时为空）写进 $GITHUB_OUTPUT，直接接在
# `cargo publish` 后面。
#
# 用法：plan-crates.sh <crate>...
set -euo pipefail

meta=$(cargo metadata --no-deps --locked --format-version 1)
args=""
for c in "$@"; do
  v=$(printf '%s' "$meta" | python3 -c 'import json, sys; n = sys.argv[1]; print(next(p["version"] for p in json.load(sys.stdin)["packages"] if p["name"] == n))' "$c")
  code=$(curl -sS --retry 3 -o crates-io-resp.json -w '%{http_code}' \
    -A "aProxy-release (https://github.com/MoYeRanqianzhi/aProxy)" \
    "https://crates.io/api/v1/crates/$c/$v") || {
    echo "::error::查询 crates.io 上的 $c@$v 失败（网络错误）"
    exit 1
  }
  case "$code" in
    200) echo "$c@$v 已在 crates.io，跳过" ;;
    404) echo "$c@$v 尚未发布，列入待发布"; args="$args -p $c" ;;
    *)
      echo "::error::查询 crates.io 上的 $c@$v 返回 HTTP $code"
      cat crates-io-resp.json
      exit 1
      ;;
  esac
done
rm -f crates-io-resp.json
echo "args=$args" >> "$GITHUB_OUTPUT"
[ -n "$args" ] || echo "全部 crate 的当前版本都已在 crates.io，无需发布"
