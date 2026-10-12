#!/usr/bin/env bash
# 主线 GitHub Release 的说明正文（Markdown，输出到标准输出）：下载与更新链、两个变体怎样选、基础
# 信息，以及从各资产的 .sha256 汇总的 SHA256 表。变更记录由 GitHub 的 generate_release_notes
# 生成，叠在这段正文之后。发布与手动演练用同一份，演练时把它写进 job 摘要供检查。
#
# 用法：release-body.sh <资产目录> <tag，如 v0.2.0>
set -euo pipefail

dir=$1
tag=$2

cat <<EOF
## 下载与更新链

\`aproxy install\` 自动按默认链条尝试：**github → npm → cargo-binstall → cargo**，并保持已装的变体。

每个平台都有两个版本：带桌面界面的 **desktop**（\`aproxy desktop\` 打开窗口）与无界面的 **headless**，代理与命令行完全相同；Linux musl 只有 headless。Linux 上带界面的版本要 WebKitGTK 2.40+（Debian / Ubuntu 的 \`libwebkit2gtk-4.1-0\`）与较新的 glibc，缺了连 \`--version\` 都起不来——服务器选 headless，一键安装脚本与 \`aproxy install\` 会自动判断。

- **GitHub（本页 assets）**：带界面 \`aproxy-<target>[-v3]\`，无界面 \`aproxy-headless-<target>[-v3]\`；\`-v3\` 需要 AVX2（约 2013 年后的主流 x86_64 CPU），不确定就选无后缀的 baseline 版本
- **npm**：\`npm install -g @meowo/aproxy\`（带界面）或 \`npm install -g @meowo/aproxy-headless\`（无界面）；两个包都提供 \`aproxy\` 命令，只装其中一个
- **cargo-binstall**：\`cargo binstall aproxy\`（带界面的预编译产物）
- **cargo（源码编译）**：\`cargo install aproxy --locked --root ~/.aproxy\`（带界面，Linux 先装 \`libwebkit2gtk-4.1-dev\`）；无界面加 \`--no-default-features\`
- **发布清单**：\`aproxy-manifest.json\` 列出每个二进制的目标、变体、指令集档与 SHA256，\`aproxy install --force\` 按它挑选资产
- **skill 文档**：\`aproxy-skills.zip\`（总包，含全部 skill：aproxy-cli / aproxy-format）/\`aproxy-skill-<名>.zip\`（单包）——install 自动随版本更新 skill，通常无需手动下载

安装/升级推荐直接用 install 指令：\`aproxy install\`（自动选指令集档与变体，并滚动重启实例）。

## 基础信息

- 版本：\`$tag\`
- 平台产物：windows（x64/x86/arm64）/ linux（x64+arm64，gnu+musl）/ macOS（arm64+x64），各有 desktop 与 headless 两个变体（musl 只有 headless）；x86_64 的 Windows 与 Linux gnu 提供 baseline 与 AVX2（-v3）两种指令集档
- 变更记录：见下方 Automatic release notes

## SHA256

| 文件 | SHA256 |
|---|---|
EOF
for f in "$dir"/*.sha256; do
  h=$(cut -d' ' -f1 "$f")
  a=$(cut -d' ' -f2- "$f" | sed 's/^ *//')
  echo "| \`$a\` | \`$h\` |"
done
