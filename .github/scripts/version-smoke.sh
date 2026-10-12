#!/usr/bin/env bash
# 对构建出的 aproxy 跑 `--version` 冒烟（永久兼容契约 C5，格式见 src/lib.rs 的 VERSION_LINE）：
# 首行必须正好是 `aproxy v<版本> (<变体>; <目标三元组>[; x86-64-v3])`，版本等于 Cargo.toml
# 里 aproxy 的版本（发布时已对齐成 tag 版本），变体、目标三元组、指令集档等于期望的取值。
#
# 两层检查：
# - 二进制里必须嵌着这一行（VERSION_LINE 是一个字符串字面量，原样留在只读数据段里）。交叉编译、
#   在构建机上跑不了的产物只做这一层；它挡住的是暂存时把产物与名字对错（带界面与无界面、
#   baseline 与 x86-64-v3 混在一起）。
# - 能在本机跑的，再真的执行 `--version`：退出码 0，首行与期望逐字相同。带界面的 exe 在 Windows
#   上若导入了 Common-Controls v6 专有的函数而又没带上 v6 清单，于 main 之前就以 0xC0000139 退出；
#   Linux 上缺 WebKitGTK 时 ld.so 在 main 之前就报错——这一层挡住这类「连 --version 都起不来」
#   的构建（RT-05）。
#
# 用法：version-smoke.sh <二进制> <desktop|headless> [<目标三元组> [<baseline|x86-64-v3>]]
# 目标三元组缺省为本机 rustc 的 host，指令集档缺省为 baseline（CI 的普通构建都是这样）。
set -euo pipefail

bin=$1
want_variant=$2
host=$(rustc -vV | sed -n 's/^host: //p')
want_target=${3:-$host}
want_cpu=${4:-baseline}

want_version=$(cargo pkgid -p aproxy | sed 's/.*[#@]//')
case $want_cpu in
  baseline) cpu_part="" ;;
  x86-64-v3) cpu_part="; x86-64-v3" ;;
  *) echo "::error::未知的指令集档 $want_cpu（baseline 或 x86-64-v3）"; exit 1 ;;
esac
want_line="aproxy v$want_version ($want_variant; $want_target$cpu_part)"
echo "期望：$want_line"

# 二进制按字节找，不受本机字符集影响
if ! LC_ALL=C grep -q -a -F -e "$want_line" "$bin"; then
  echo "::error::$bin 里没有嵌着 \`$want_line\`：产物与名字对错了，或版本没有对齐"
  exit 1
fi

# 32 位 Windows 程序在 64 位 Windows 上照样能跑，静态链接的 musl 程序在同一架构的 glibc 系统上
# 照样能跑；其余目标不同于 host 的都是交叉编译
runnable=false
if [[ $want_target == "$host" ]]; then
  runnable=true
elif [[ $host == x86_64-pc-windows-msvc && $want_target == i686-pc-windows-msvc ]]; then
  runnable=true
elif [[ $host == *-unknown-linux-gnu && $want_target == "${host%-gnu}-musl" ]]; then
  runnable=true
fi
if [[ $runnable != true ]]; then
  echo "$want_target 是交叉编译的产物，在 $host 上跑不了：只核对了嵌着的版本行"
  exit 0
fi

out=$("$bin" --version)
line=${out%%$'\n'*}
echo "实际：$line"
if [[ $line != "$want_line" ]]; then
  echo "::error::--version 的首行与期望不同"
  exit 1
fi
