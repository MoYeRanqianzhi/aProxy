#!/usr/bin/env bash
# npm 渠道组包与发布。
#
# 结构（esbuild/rollup 同款多平台包模式）：主包 @meowo/aproxy 只含
# JS 转发器；5 个平台子包各携带一份对应平台二进制，经 optionalDependencies
# 的 os/cpu/libc 字段由 npm 自动按平台装配——全程 registry 内分发，无二次下载。
#
# 用法：
#   CI 模式（build 矩阵 artifact 已合并到 ./artifacts/）：
#     bash npm/build-and-publish.sh
#   本地模式（从 GitHub Release 拉资产组包，首发/补发用）：
#     RELEASE_TAG=v0.1.0-alpha.7 bash npm/build-and-publish.sh [--dry-run]
#
# 认证：设置 NPM_TOKEN 环境变量走 token（trusted publishing 绑定前的首发期）；
# 不设置时依赖 setup-node 的 OIDC（npmjs.com 绑定 Trusted Publisher 后）。
# 本地首发另需 npm adduser 或在 ~/.npmrc 配好 token。
#
# 包名/仓库/版本约定：trusted publishing 校验 package.json 的 repository
# 与 OIDC claims 精确匹配（大小写敏感）——改仓库名时必须同步此处模板。
#
# --tag latest：npm 11 对 prerelease 版本强制显式 tag。0.1.x 全程预发布、
# 无 stable，latest 指向最新 alpha 语义正确（`npm i -g @meowo/aproxy`
# 直接可装）；首个 stable（0.2.0）发布后 latest 自然指向 stable，无需改动。

set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$(pwd)

DRY_RUN=""
if [ "${1:-}" = "--dry-run" ]; then
  DRY_RUN="--dry-run"
fi

# 版本来源：CI 取 tag（v 前缀剥离），本地取 RELEASE_TAG
VERSION="${GITHUB_REF_NAME:-}"
if [ -n "${RELEASE_TAG:-}" ]; then
  VERSION="${RELEASE_TAG#v}"
elif [ -z "$VERSION" ] || [ "$VERSION" = "master" ] || [ "$VERSION" = "main" ]; then
  echo "错误：CI 外运行必须设 RELEASE_TAG（如 v0.1.0-alpha.7）" >&2
  exit 1
fi
VERSION="${VERSION#v}"

REPO_URL="https://github.com/MoYeRanqianzhi/aProxy"
PKG_SCOPE="@meowo/aproxy"

# 子包后缀 : 资产文件名 : os : cpu
# npm 分发固定取 baseline 变体（-v3 是 GitHub 资产的安装优化，npm 链不提供）
MAPPINGS=(
  "windows-x64:aproxy-x86_64-pc-windows-msvc.exe:win32:x64"
  "windows-ia32:aproxy-i686-pc-windows-msvc.exe:win32:ia32"
  "windows-arm64:aproxy-aarch64-pc-windows-msvc.exe:win32:arm64"
  "linux-x64:aproxy-x86_64-unknown-linux-gnu:linux:x64"
  "linux-x64-musl:aproxy-x86_64-unknown-linux-musl:linux:x64"
  "linux-arm64:aproxy-aarch64-unknown-linux-gnu:linux:arm64"
  "linux-arm64-musl:aproxy-aarch64-unknown-linux-musl:linux:arm64"
  "darwin-arm64:aproxy-aarch64-apple-darwin:darwin:arm64"
  "darwin-x64:aproxy-x86_64-apple-darwin:darwin:x64"
)

# 资产就位：CI 从合并的 artifact 目录取；本地从 GitHub Release 下载
ASSET_DIR=""
if [ -n "${RELEASE_TAG:-}" ]; then
  echo "== 下载 Release 资产（${RELEASE_TAG}）"
  ASSET_DIR=".npm-assets"
  rm -rf "$ASSET_DIR" && mkdir -p "$ASSET_DIR"
  gh release download "$RELEASE_TAG" -R MoYeRanqianzhi/aProxy -p "aproxy-*" -D "$ASSET_DIR"
else
  ASSET_DIR="artifacts"
fi

# ---- 平台子包：二进制 + package.json（os/cpu/libc 元数据由 npm 消费）----
for m in "${MAPPINGS[@]}"; do
  IFS=: read -r suffix asset os cpu <<<"$m"
  dir="npm/${PKG_SCOPE}-${suffix}"
  mkdir -p "$dir/bin"
  # 包内 bin 名：win32 平台带 .exe（wrapper 按平台查找 aproxy[.exe]）
  bin_out="aproxy"
  [ "$os" = "win32" ] && bin_out="aproxy.exe"
  cp "$ASSET_DIR/$asset" "$dir/bin/$bin_out"
  chmod +x "$dir/bin/$bin_out" 2>/dev/null || true

  # libc 字段：musl 包声明（npm 据此在 Alpine 等 musl 环境选包）
  libc_json=""
  case "$suffix" in *-musl) libc_json=$',\n  "libc": ["musl"]' ;; esac

  cat >"$dir/package.json" <<EOF
{
  "name": "${PKG_SCOPE}-${suffix}",
  "version": "${VERSION}",
  "description": "The aProxy binary for ${suffix} (local API proxy with infinite retries)",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+${REPO_URL}.git"
  },
  "os": ["${os}"],
  "cpu": ["${cpu}"]${libc_json},
  "files": ["bin/"]
}
EOF
  echo "   组包 ${dir}"
done

# ---- 主包：转发器 + README + optionalDependencies 全平台清单 ----
main_dir="npm/${PKG_SCOPE}"
mkdir -p "$main_dir/bin"
cp npm/aproxy/bin/aproxy.js "$main_dir/bin/aproxy.js"
cp npm/aproxy/README.md "$main_dir/README.md"

# skill 支线：主包附带 skill zip（install 的 npm 通道从这里提取；内容与
# GH release 的 aproxy-skills.zip 同源同形——条目自带 aproxy-cli/ 顶层前缀，
# 消费侧 install_skill_dir 按此形态解包）
mkdir -p "$main_dir/skills"
(
  cd .claude/skills
  # zip 输出走仓库根绝对路径——相对路径会按 cd 后的 cwd 解析（alpha.10
  # 首跑曾因 "../../" 指到 .claude/skills/npm/ 下而创建失败）
  zip -qr "$ROOT/$main_dir/skills/aproxy-cli.zip" aproxy-cli -x '*.zip'
)

opt_deps=""
for m in "${MAPPINGS[@]}"; do
  IFS=: read -r suffix _ <<<"$m"
  [ -n "$opt_deps" ] && opt_deps+=","$'\n'
  opt_deps+="    \"${PKG_SCOPE}-${suffix}\": \"${VERSION}\""
done

cat >"$main_dir/package.json" <<EOF
{
  "name": "${PKG_SCOPE}",
  "version": "${VERSION}",
  "description": "Local API proxy with infinite retries for agent workloads",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+${REPO_URL}.git"
  },
  "keywords": ["proxy", "retry", "api", "agent", "llm", "claude"],
  "bin": {
    "aproxy": "bin/aproxy.js"
  },
  "files": ["bin/", "README.md", "skills/"],
  "engines": {
    "node": ">=18"
  },
  "optionalDependencies": {
${opt_deps}
  }
}
EOF
echo "   组包 ${main_dir}"

# ---- 发布：平台包全部就位后才发主包（optionalDependencies 指向的版本须已存在）----
if [ -n "${NPM_TOKEN:-}" ]; then
  echo "== 使用 NPM_TOKEN 认证（trusted publishing 绑定前的首发路径）"
  printf '//registry.npmjs.org/:_authToken=%s\n' "$NPM_TOKEN" >>"$HOME/.npmrc"
fi

for m in "${MAPPINGS[@]}"; do
  IFS=: read -r suffix _ <<<"$m"
  echo "== npm publish ${PKG_SCOPE}-${suffix}@${VERSION} ${DRY_RUN}"
  (cd "npm/${PKG_SCOPE}-${suffix}" && npm publish --access public --tag latest $DRY_RUN)
done

echo "== npm publish ${PKG_SCOPE}@${VERSION} ${DRY_RUN}"
(cd "$main_dir" && npm publish --access public --tag latest $DRY_RUN)

echo "== npm 渠道发布完成"
