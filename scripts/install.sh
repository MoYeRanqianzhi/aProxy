#!/bin/sh
# aProxy 引导安装脚本（POSIX sh：Linux / macOS / Git Bash 等）。
#
# 职责 = bootstrap 首装：从 GitHub Releases 下载二进制与 skill 文档，落位到
# $APROXY_HOME（默认 ~/.aproxy）。已安装则不重复安装——装机后的升级一律
# `aproxy install` 自管（本脚本指路）。
#
# 用法：
#   curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/master/scripts/install.sh | sh
#   或本地执行：sh scripts/install.sh [v0.1.0-alpha.7]
# 环境变量：
#   APROXY_HOME     目录根（默认 ~/.aproxy）
#   APROXY_NO_SKILLS=1   跳过 skill 文档
#   APROXY_DL_PROXY 下载代理（仅本次；与上游请求代理完全无关）
set -eu

REPO="MoYeRanqianzhi/aProxy"
APROXY_HOME="${APROXY_HOME:-$HOME/.aproxy}"
BIN_DIR="$APROXY_HOME/bin"
SKILLS_DIR="$APROXY_HOME/skills"
TMP_DIR="$APROXY_HOME/staging/bootstrap"
UA="aproxy-install-script"

# ---- 下载函数：优先 curl，回退 wget ----
fetch() {
    url="$1"; out="$2"
    if command -v curl >/dev/null 2>&1; then
        if [ -n "${APROXY_DL_PROXY:-}" ]; then
            curl -fsSL --proxy "$APROXY_DL_PROXY" -A "$UA" -o "$out" "$url"
        else
            curl -fsSL -A "$UA" -o "$out" "$url"
        fi
    elif command -v wget >/dev/null 2>&1; then
        if [ -n "${APROXY_DL_PROXY:-}" ]; then
            wget -q --proxy="$APROXY_DL_PROXY" -U "$UA" -O "$out" "$url"
        else
            wget -q -U "$UA" -O "$out" "$url"
        fi
    else
        echo "需要 curl 或 wget 之一" >&2
        exit 1
    fi
}

# ---- 最新 tag：releases/latest 在仅有 prerelease 时 404（alpha 时代整线
# 都是 prerelease）——用列表接口取第一个（GitHub 按创建时间倒序）----
latest_tag() {
    fetch "https://api.github.com/repos/$REPO/releases?per_page=1" - \
        | grep '"tag_name"' | head -1 | sed 's/.*"tag_name": *"\([^"]*\)".*/\1/'
}

# ---- 已安装检测：装机后的升级归 install 管，本脚本只做首装 ----
if [ -f "$BIN_DIR/aproxy" ]; then
    echo "aProxy 已安装于 $BIN_DIR/aproxy，本脚本不做覆盖。"
    echo "升级请使用: aproxy install"
    exit 0
fi

# ---- 解析版本与平台资产 ----
TAG="${1:-$(latest_tag)}"
[ -n "$TAG" ] || { echo "无法解析最新 release（仓库无 release 或网络不可达）" >&2; exit 1; }
echo "安装 aProxy $TAG"

OS="$(uname -s)"; ARCH="$(uname -m)"
case "$OS" in
    Linux) platform="unknown-linux-gnu" ;;
    Darwin) platform="apple-darwin" ;;
    *) echo "不支持的平台: $OS" >&2; exit 1 ;;
esac
case "$ARCH" in
    x86_64|amd64) rust_arch="x86_64" ;;
    aarch64|arm64) rust_arch="aarch64" ;;
    *) echo "不支持的架构: $ARCH" >&2; exit 1 ;;
esac
# 本脚本不探测 AVX2（保守拉 baseline）；指令集变体选择由 `aproxy install` 做
ASSET="aproxy-${rust_arch}-${platform}"
BASE="https://github.com/$REPO/releases/download/$TAG"

mkdir -p "$BIN_DIR" "$SKILLS_DIR" "$TMP_DIR"

# ---- 下载二进制 + 校验 + 落位 ----
echo "下载二进制 $ASSET ..."
fetch "$BASE/$ASSET" "$TMP_DIR/aproxy"
fetch "$BASE/$ASSET.sha256" "$TMP_DIR/aproxy.sha256"
expected=$(cut -d' ' -f1 "$TMP_DIR/aproxy.sha256")
actual=$(sha256sum "$TMP_DIR/aproxy" | cut -d' ' -f1)
[ "$expected" = "$actual" ] || {
    echo "SHA256 校验失败:" >&2
    echo "  期望 $expected" >&2
    echo "  实际 $actual" >&2
    exit 1
}
chmod 755 "$TMP_DIR/aproxy"
mv "$TMP_DIR/aproxy" "$BIN_DIR/aproxy"

# ---- skill 文档（非强制：失败不影响安装）----
if [ "${APROXY_NO_SKILLS:-}" != "1" ]; then
    if fetch "$BASE/aproxy-skills.zip" "$TMP_DIR/aproxy-skills.zip" 2>/dev/null \
        && fetch "$BASE/aproxy-skills.zip.sha256" "$TMP_DIR/aproxy-skills.zip.sha256" 2>/dev/null; then
        expected=$(cut -d' ' -f1 "$TMP_DIR/aproxy-skills.zip.sha256")
        actual=$(sha256sum "$TMP_DIR/aproxy-skills.zip" | cut -d' ' -f1)
        if [ "$expected" = "$actual" ]; then
            # unzip 缺失时降级 python（macOS 旧版/精简容器无 unzip）
            if command -v unzip >/dev/null 2>&1; then
                unzip -oq "$TMP_DIR/aproxy-skills.zip" -d "$SKILLS_DIR"
            else
                python3 -c "import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" \
                    "$TMP_DIR/aproxy-skills.zip" "$SKILLS_DIR"
            fi
            echo "skill 文档已就位 $SKILLS_DIR（按所用 agent 的方式链接/复制到其 skills 目录）"
        else
            echo "skill 文档 SHA256 校验失败，跳过（不影响安装）"
        fi
    else
        echo "skill 文档下载失败，跳过（不影响安装，可稍后用 aproxy install 重试）"
    fi
fi

# ---- 清理与提示 ----
rm -rf "$TMP_DIR"

echo ""
echo "aProxy 已安装: $BIN_DIR/aproxy"
case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        echo "注意: $BIN_DIR 不在 PATH 中——加入后即可直接使用 aproxy 命令："
        echo "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.profile && source ~/.profile"
        ;;
esac
echo "启动: aproxy   （或完整路径 \"$BIN_DIR/aproxy\"）"
echo "状态: aproxy status    升级: aproxy install"
