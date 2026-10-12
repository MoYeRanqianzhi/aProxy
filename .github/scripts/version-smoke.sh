#!/usr/bin/env bash
# 对构建出的 aproxy 跑 `--version` 冒烟（永久兼容契约 C5，格式见 src/lib.rs 的 VERSION_LINE）：
# 退出码 0；首行是 `aproxy v<版本> (<变体>; <目标三元组>[; x86-64-v3])`，版本等于 Cargo.toml
# 里 aproxy 的版本，变体等于期望的那一种，目标三元组等于本机 rustc 的 host（CI 不交叉编译）。
#
# 带界面的 exe 在 Windows 上缺 Common-Controls v6 清单时于 main 之前就以 0xC0000139 退出，
# Linux 上缺 WebKitGTK 时 ld.so 在 main 之前就报错——这一步就是为挡住这类「连 --version 都
# 起不来」的构建（RT-05）。
#
# 用法：version-smoke.sh <二进制> <desktop|headless>
set -euo pipefail

bin=$1
want_variant=$2

out=$("$bin" --version)
line=${out%%$'\n'*}
echo "$line"

want_version=$(cargo pkgid -p aproxy | sed 's/.*[#@]//')
want_target=$(rustc -vV | sed -n 's/^host: //p')

re='^aproxy v([^ ]+) \(([^;)]+); ([^;)]+)(; x86-64-v3)?\)$'
if [[ ! $line =~ $re ]]; then
  echo "::error::--version 的首行不是 C5 的格式：$line"
  exit 1
fi
version=${BASH_REMATCH[1]}
variant=${BASH_REMATCH[2]}
target=${BASH_REMATCH[3]}

status=0
if [[ $version != "$want_version" ]]; then
  echo "::error::版本段是 $version，应为 Cargo.toml 里的 $want_version"
  status=1
fi
if [[ $variant != "$want_variant" ]]; then
  echo "::error::变体段是 $variant，应为 $want_variant"
  status=1
fi
if [[ $target != "$want_target" ]]; then
  echo "::error::目标段是 $target，应为 $want_target"
  status=1
fi
exit $status
