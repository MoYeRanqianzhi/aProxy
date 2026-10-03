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
# dist-tag：预发布（版本含 -alpha/-beta/-rc）发 next，稳定版发 latest，规则
# 见下方 npm_dist_tag()。首个稳定版发布后，`npm i -g @meowo/aproxy` 只会装到
# 稳定版；想用预发布须显式 `npm i -g @meowo/aproxy@next`。
#
# 可重跑：每个包发布前先查询该版本是否已在 registry 上，已存在就跳过（见
# npm_version_exists()）。CI 的 publish job 中途失败后可以直接 re-run，不会在
# 第一个已发布的包处报 403 退出；查询本身失败则硬失败，不会误当「未发布」。

set -euo pipefail

# npm dist-tag 选择。与 .github/workflows/release.yml、release-format.yml 中
# softprops/action-gh-release 的 prerelease 表达式是同一条规则：tag 含
# -alpha/-beta/-rc 即预发布（GitHub 标 prerelease ↔ npm 发 next），否则为
# 稳定版（GitHub 可成为 Latest ↔ npm 发 latest）。改动任一处必须同步其余两处。
# GitHub 表达式的 contains() 不区分大小写，这里先转小写再匹配，保持一致。
# 无论哪种都显式传 --tag：npm 11 对预发布版本不给 --tag 会直接拒发。
npm_dist_tag() {
  case "$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" in
    *-alpha* | *-beta* | *-rc*) echo next ;;
    *) echo latest ;;
  esac
}

# 判断 <包>@<精确版本> 是否已在 registry 上：已存在返回 0，未发布返回 1。
# npm view 的三种结果：版本存在时输出该版本号；包存在但没有这个版本时输出为空、
# 退出码 0；包本身从未发布时报 E404、退出码非 0。其余失败（网络、认证、
# registry 故障）直接终止脚本——把查询失败当成「未发布」继续 publish，会在
# 已发布的版本上撞 403，或掩盖真正的问题。
# --prefer-online：跳过本地缓存的 packument，避免刚发布的版本被旧缓存判为不存在。
npm_version_exists() {
  local pkg="$1" ver="$2" out errfile
  errfile=$(mktemp)
  if out=$(npm view "${pkg}@${ver}" version --prefer-online 2>"$errfile"); then
    rm -f "$errfile"
    [ "$out" = "$ver" ] && return 0
    [ -z "$out" ] && return 1
    echo "错误：npm view ${pkg}@${ver} 返回了意外的输出：${out}" >&2
    exit 1
  fi
  if grep -q 'E404' "$errfile"; then
    rm -f "$errfile"
    return 1
  fi
  echo "错误：查询 ${pkg}@${ver} 是否已发布时失败（不是 404，不能当作未发布继续）：" >&2
  cat "$errfile" >&2
  rm -f "$errfile"
  exit 1
}

cd "$(dirname "$0")/.."
ROOT=$(pwd)

DRY_RUN=""
if [ "${1:-}" = "--dry-run" ]; then
  DRY_RUN="--dry-run"
fi
# 组名参数化：空 = aproxy 主组（默认）；"format" = aproxy-format 双包组
# （官方示例 format 二进制，独立发版链 release-format.yml）。$1 是组名时
# --dry-run 移到 $2。
GROUP="${1:-}"
if [ "$GROUP" = "--dry-run" ]; then GROUP=""; fi
GROUP_ARG="${1:-}"
shift_arg=0
case "$GROUP_ARG" in
  --dry-run) DRY_RUN="--dry-run"; shift_arg=1 ;;
  format) shift_arg=1 ;;
esac
if [ $shift_arg -eq 1 ] && [ "${2:-}" = "--dry-run" ]; then DRY_RUN="--dry-run"; fi

if [ "$GROUP_ARG" = "format" ]; then
  GROUP="format"
fi

# 版本来源：CI 取 tag（组前缀剥离），本地取 RELEASE_TAG。format 组的 tag
# 是 format-v0.1.0（版本独立于 aproxy alpha 线）。
TAG_PREFIX="v"
if [ "$GROUP" = "format" ]; then
  TAG_PREFIX="format-v"
fi
VERSION="${GITHUB_REF_NAME:-}"
if [ -n "${RELEASE_TAG:-}" ]; then
  VERSION="${RELEASE_TAG#${TAG_PREFIX}}"
elif [ -n "$VERSION" ]; then
  VERSION="${VERSION#${TAG_PREFIX}}"
fi
if [ -z "$VERSION" ] || [ "$VERSION" = "master" ] || [ "$VERSION" = "main" ]; then
  echo "错误：CI 外运行必须设 RELEASE_TAG（如 ${TAG_PREFIX}0.1.0）" >&2
  exit 1
fi

REPO_URL="https://github.com/MoYeRanqianzhi/aProxy"
PKG_SCOPE="@meowo/aproxy"
ASSET_PREFIX="aproxy-"
BIN_NAME="aproxy"
PKG_DESC="Local API proxy with infinite retries for agent workloads"
SUB_DESC="The aProxy binary for SUB_SUFFIX (local API proxy with infinite retries)"
WRAPPER_SRC="npm/aproxy/bin/aproxy.js"
if [ "$GROUP" = "format" ]; then
  PKG_SCOPE="@meowo/aproxy-format"
  ASSET_PREFIX="aproxy-format-"
  BIN_NAME="aproxy-format"
  PKG_DESC="Official example format program for aProxy (protocol conversion, key rotation, multi-channel aggregation)"
  SUB_DESC="The aProxy format binary for SUB_SUFFIX (protocol conversion, key rotation, multi-channel aggregation)"
fi

# 子包后缀 : 资产文件名 : os : cpu
# npm 分发固定取 baseline 变体（-v3 是 GitHub 资产的安装优化，npm 链不提供）
MAPPINGS=(
  "windows-x64:${ASSET_PREFIX}x86_64-pc-windows-msvc.exe:win32:x64"
  "windows-ia32:${ASSET_PREFIX}i686-pc-windows-msvc.exe:win32:ia32"
  "windows-arm64:${ASSET_PREFIX}aarch64-pc-windows-msvc.exe:win32:arm64"
  "linux-x64:${ASSET_PREFIX}x86_64-unknown-linux-gnu:linux:x64"
  "linux-x64-musl:${ASSET_PREFIX}x86_64-unknown-linux-musl:linux:x64"
  "linux-arm64:${ASSET_PREFIX}aarch64-unknown-linux-gnu:linux:arm64"
  "linux-arm64-musl:${ASSET_PREFIX}aarch64-unknown-linux-musl:linux:arm64"
  "darwin-arm64:${ASSET_PREFIX}aarch64-apple-darwin:darwin:arm64"
  "darwin-x64:${ASSET_PREFIX}x86_64-apple-darwin:darwin:x64"
)

# 资产就位：CI 从合并的 artifact 目录取；本地从 GitHub Release 下载
ASSET_DIR=""
if [ -n "${RELEASE_TAG:-}" ]; then
  echo "== 下载 Release 资产（${RELEASE_TAG}）"
  ASSET_DIR=".npm-assets"
  rm -rf "$ASSET_DIR" && mkdir -p "$ASSET_DIR"
  gh release download "$RELEASE_TAG" -R MoYeRanqianzhi/aProxy -p "${ASSET_PREFIX}*" -D "$ASSET_DIR"
else
  ASSET_DIR="artifacts"
fi

# ---- 平台子包：二进制 + package.json（os/cpu/libc 元数据由 npm 消费）----
for m in "${MAPPINGS[@]}"; do
  IFS=: read -r suffix asset os cpu <<<"$m"
  dir="npm/${PKG_SCOPE}-${suffix}"
  mkdir -p "$dir/bin"
  # 包内 bin 名：win32 平台带 .exe（wrapper 按平台查找 <BIN>[.exe]）
  bin_out="$BIN_NAME"
  [ "$os" = "win32" ] && bin_out="${BIN_NAME}.exe"
  cp "$ASSET_DIR/$asset" "$dir/bin/$bin_out"
  chmod +x "$dir/bin/$bin_out" 2>/dev/null || true

  # libc 字段：musl 包声明（npm 据此在 Alpine 等 musl 环境选包）
  libc_json=""
  case "$suffix" in *-musl) libc_json=$',\n  "libc": ["musl"]' ;; esac

  cat >"$dir/package.json" <<EOF
{
  "name": "${PKG_SCOPE}-${suffix}",
  "version": "${VERSION}",
  "description": "${SUB_DESC/SUB_SUFFIX/${suffix}}",
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
# wrapper 是包名自检测的通用脚本（@meowo/aproxy* 两组通用），format 组
# 复制同款内容为 <BIN>.js
cp "$WRAPPER_SRC" "$main_dir/bin/${BIN_NAME}.js"
if [ -f "npm/aproxy/README.md" ]; then
  cp "npm/aproxy/README.md" "$main_dir/README.md" 2>/dev/null || true
fi

# skill 支线：仅 aproxy 主组（format 组不携带 skill——install 的 npm 通道
# 只在主包提取）。打**一个多 skill 总包** aproxy-skills.zip（与 GH release
# 总包同源同形——条目自带各 skill 顶层目录前缀，消费侧 install_skill_dir
# 泛化后按顶层目录逐个落位）。
if [ "$GROUP" != "format" ]; then
  mkdir -p "$main_dir/skills"
  (
    cd .claude/skills
    names=""
    for d in */; do
      names="$names $(basename "$d")"
    done
    # zip 输出走仓库根绝对路径——相对路径会按 cd 后的 cwd 解析（alpha.10
    # 首跑曾因 "../../" 指到 .claude/skills/npm/ 下而创建失败）
    # shellcheck disable=SC2086
    zip -qr "$ROOT/$main_dir/skills/aproxy-skills.zip" $names -x '*.zip'
  )
fi

opt_deps=""
for m in "${MAPPINGS[@]}"; do
  IFS=: read -r suffix _ <<<"$m"
  [ -n "$opt_deps" ] && opt_deps+=","$'\n'
  opt_deps+="    \"${PKG_SCOPE}-${suffix}\": \"${VERSION}\""
done

# format 组主包不带 README/skills（description 已表意；skill 只随 aproxy 主组）
files_json='"bin/", "README.md", "skills/"'
if [ "$GROUP" = "format" ]; then
  files_json='"bin/"'
fi

cat >"$main_dir/package.json" <<EOF
{
  "name": "${PKG_SCOPE}",
  "version": "${VERSION}",
  "description": "${PKG_DESC}",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "git+${REPO_URL}.git"
  },
  "keywords": ["proxy", "retry", "api", "agent", "llm", "claude"],
  "bin": {
    "${BIN_NAME}": "bin/${BIN_NAME}.js"
  },
  "files": [${files_json}],
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

DIST_TAG=$(npm_dist_tag "$VERSION")
echo "== 版本 ${VERSION} → npm dist-tag ${DIST_TAG}"

# $1 = 包目录，$2 = 包名。已存在的版本跳过（re-run 时的预期情况），dry-run 下同样
# 先查询，输出与真实发布时一致的跳过/发布判断。
publish_pkg() {
  if npm_version_exists "$2" "$VERSION"; then
    echo "== 跳过 $2@${VERSION}：registry 上已存在该版本"
    return 0
  fi
  echo "== npm publish $2@${VERSION} --tag ${DIST_TAG} ${DRY_RUN}"
  (cd "$1" && npm publish --access public --tag "$DIST_TAG" $DRY_RUN)
}

for m in "${MAPPINGS[@]}"; do
  IFS=: read -r suffix _ <<<"$m"
  publish_pkg "npm/${PKG_SCOPE}-${suffix}" "${PKG_SCOPE}-${suffix}"
done

publish_pkg "$main_dir" "$PKG_SCOPE"

echo "== npm 渠道发布完成（${PKG_SCOPE} 组）"
