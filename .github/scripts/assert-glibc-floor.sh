#!/usr/bin/env bash
# 断言 Linux gnu 产物引用的 GLIBC_ 符号版本都不高于约定的下限，打印实际的最高需求。
#
# 动态加载器启动时逐条核对 ELF 的 .gnu.version_r（verneed）版本需求，缺任何一条都直接拒绝启动——
# 哪怕引用它的符号是弱符号（alpha.17 实证：std 对 pidfd_spawnp 的弱引用仍带出非弱的 GLIBC_2.39
# 需求）。objdump -p 输出的 "Version References" 就是 verneed。非数字的 GLIBC_ 需求（如链接器打开
# DT_RELR 时出现的 GLIBC_ABI_DT_RELR，要求 glibc 2.36+）同样判失败。下限必须与安装脚本、npm
# 启动器、安装器探测本机 glibc 时用的数一致，否则它们会把产物装到跑不起来的系统上。
#
# 用法：assert-glibc-floor.sh <二进制> <glibc 下限，如 2.28>
set -euo pipefail

bin=$1
floor=$2

names=$(objdump -p "$bin" | grep -oE 'GLIBC_[A-Za-z0-9_.]+' | sort -u || true)
if [ -z "$names" ]; then
  echo "::error::$bin 未引用任何 GLIBC_ 符号版本，不像 glibc 动态链接产物"
  exit 1
fi
echo "$bin 的 glibc 版本需求：$(tr '\n' ' ' <<<"$names")"
odd=$(printf '%s\n' "$names" | grep -vE '^GLIBC_[0-9]+(\.[0-9]+)*$' || true)
if [ -n "$odd" ]; then
  echo "::error::$bin 含非数字的 glibc 版本需求：$(tr '\n' ' ' <<<"$odd")"
  exit 1
fi
max=$(printf '%s\n' "$names" | sed 's/^GLIBC_//' | sort -V | tail -n 1)
top=$(printf '%s\n%s\n' "$max" "$floor" | sort -V | tail -n 1)
if [ "$top" != "$floor" ]; then
  echo "::error::$bin 需要 GLIBC_$max，高于约定下限 GLIBC_$floor"
  exit 1
fi
echo "最高 GLIBC_$max <= 下限 GLIBC_$floor：通过"
