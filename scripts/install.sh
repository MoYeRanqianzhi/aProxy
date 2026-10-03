#!/bin/sh
# aProxy 引导安装脚本（POSIX sh：Linux / macOS / Git Bash 等）。
#
# 职责 = bootstrap 首装：从 GitHub Releases 下载二进制与 skill 文档，落位到
# $APROXY_HOME（默认 ~/.aproxy）。已安装则不重复安装——装机后的升级一律
# `aproxy install` 自管（本脚本指路）。
#
# 用法：
#   curl -fsSL https://raw.githubusercontent.com/MoYeRanqianzhi/aProxy/main/scripts/install.sh | sh
#   curl -fsSL .../install.sh | sh -s -- --pre        # 允许安装预发布
#   或本地执行：sh scripts/install.sh [--pre] [v0.1.0]
# 选版规则（无显式 tag 时）：
#   1. 只认 v<数字> 开头的 tag（同仓库的 format-v* 是另一条发版线，必须排除）并跳过 draft；
#   2. 默认取最新的稳定版（非 prerelease，按版本号取最大）；
#   3. 仓库里一个 v* 稳定版都没有时（0.1.0 发布前）回退到最新的 v* 预发布并打印说明；
#   4. --pre（或 APROXY_PRE=1）：取最新创建的 v* release，预发布也可。
# 环境变量：
#   APROXY_HOME     目录根（默认 ~/.aproxy）
#   APROXY_NO_SKILLS=1   跳过 skill 文档
#   APROXY_PRE=1    等效 --pre
#   APROXY_DL_PROXY 下载代理（仅本次；与上游请求代理完全无关）
set -eu

REPO="MoYeRanqianzhi/aProxy"
# glibc 版本下限：Linux gnu 产物按此基线构建，低于它的系统（如 RHEL/CentOS 7 的 glibc 2.17）
# 改选静态链接的 musl 产物。必须与 .github/workflows/release.yml 中 gnu 构建的
# glibc 下限、npm/aproxy/bin/aproxy.js 的 MIN_GLIBC 保持一致。
MIN_GLIBC="2.28"
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

# ---- SHA256：按 sha256sum → shasum -a 256 → openssl dgst -sha256 择一 ----
# 工具缺失与「校验失败」是两种错误，分开报：缺工具时在下载之前就明确退出，
# 不会让用户把环境问题误读为文件被篡改。
pick_sha_tool() {
    if command -v sha256sum >/dev/null 2>&1; then SHA_TOOL="sha256sum"
    elif command -v shasum >/dev/null 2>&1; then SHA_TOOL="shasum"
    elif command -v openssl >/dev/null 2>&1; then SHA_TOOL="openssl"
    else
        echo "缺少 SHA256 校验工具：需要 sha256sum、shasum 或 openssl 之一（本脚本不跳过校验）" >&2
        exit 1
    fi
}

# 输出小写十六进制摘要；命令本身失败（不是不匹配）时返回非零
sha256_of() {
    case "$SHA_TOOL" in
        sha256sum) sha256sum "$1" | cut -d' ' -f1 ;;
        shasum) shasum -a 256 "$1" | cut -d' ' -f1 ;;
        # 输出形如 "SHA2-256(file)= <hex>"（OpenSSL 3）或 "SHA256(file)= <hex>"
        openssl) openssl dgst -sha256 "$1" | sed 's/.*= *//' ;;
    esac
}

# verify_sha256 <文件> <.sha256 文件>：不匹配或无法计算均报错并退出
verify_sha256() {
    expected=$(cut -d' ' -f1 "$2" | tr 'A-F' 'a-f')
    actual=$(sha256_of "$1") || { echo "无法计算 $1 的 SHA256（$SHA_TOOL 执行失败）" >&2; exit 1; }
    actual=$(printf '%s' "$actual" | tr 'A-F' 'a-f')
    if [ -z "$actual" ] || [ "$expected" != "$actual" ]; then
        echo "SHA256 校验失败: $1" >&2
        echo "  期望 $expected" >&2
        echo "  实际 $actual" >&2
        exit 1
    fi
}

# ---- 选版 ----
# GitHub 在同一仓库里同时承载两条发版线：v*（aproxy 主线）与 format-v*
# （aproxy-format），/releases/latest 可能指向 format-v*；列表接口首项也不可靠
# （按创建时间排，并列时次序偶然）。因此拉一整页（上限 100）自己过滤。
#
# 解析假设（无 jq，纯 POSIX 工具）：release 列表的每个对象里，tag_name、draft、
# prerelease 三个键按此顺序出现，且嵌套对象（author/assets/reactions）没有同名键；
# release 正文里的引号在 JSON 中被转义为 \"，不会被 `"key":` 模式命中。
# 任一假设不成立（键缺失/次序错）时 awk 以非零退出，脚本明确报错而不是猜一个 tag。
#
# parse_releases 输入 JSON，输出每行 "<tag> <draft> <prerelease>"，保持列表顺序。
# 用 ERE（grep -E）写交替：BRE 里的 \| 是 GNU 扩展，musl/busybox 这类严格 POSIX
# 的正则实现不认——而 Alpine 等 musl 系统恰恰是 musl 回退要服务的对象。
parse_releases() {
    grep -oE '"(tag_name|draft|prerelease)": *[^,}]*' | tr -d '" \r' | awk -F: '
        $1 == "tag_name"   { if (state != 0) bad = 1; tag = $2; state = 1; next }
        $1 == "draft"      { if (state != 1) bad = 1; draft = $2; state = 2; next }
        $1 == "prerelease" { if (state != 2) bad = 1; print tag, draft, $2; state = 0; next }
        END { if (bad || state != 0) exit 3 }
    '
}

# max_stable：stdin 为 tag 列表，输出版本号最大者（仅接受 vX.Y.Z）。
# 用 awk 数值比较而不是 sort -V，避免依赖 sort 的 GNU 扩展（旧 BSD/busybox 不一定有）。
max_stable() {
    awk '
        /^v[0-9]+\.[0-9]+\.[0-9]+$/ {
            split(substr($0, 2), p, ".")
            key = p[1] * 1000000000000 + p[2] * 1000000 + p[3]
            if (!seen || key > best) { best = key; tag = $0; seen = 1 }
        }
        END { if (seen) print tag }
    '
}

# resolve_tag：设置全局 TAG；want_pre=1 时取最新创建的 v* release（含预发布）
resolve_tag() {
    list="$TMP_DIR/releases.json"
    fetch "https://api.github.com/repos/$REPO/releases?per_page=100" "$list" \
        || { echo "无法获取 release 列表（网络不可达或 GitHub API 限流）" >&2; exit 1; }
    parsed=$(parse_releases < "$list") \
        || { echo "无法解析 GitHub release 列表（响应格式与预期不符）；请改用显式 tag：sh install.sh v0.1.0" >&2; exit 1; }
    [ -n "$parsed" ] || { echo "release 列表为空或响应格式与预期不符（可能是 API 限流页）；请改用显式 tag：sh install.sh v0.1.0" >&2; exit 1; }
    # 只留 v<数字> 开头且非 draft 的条目（排除 format-v*）
    cands=$(printf '%s\n' "$parsed" | awk '$1 ~ /^v[0-9]/ && $2 == "false" { print }')
    [ -n "$cands" ] || { echo "仓库里没有可安装的 v* release" >&2; exit 1; }

    if [ "$want_pre" = "1" ]; then
        TAG=$(printf '%s\n' "$cands" | head -n 1 | cut -d' ' -f1)
        return
    fi
    TAG=$(printf '%s\n' "$cands" | awk '$3 == "false" { print $1 }' | max_stable)
    if [ -z "$TAG" ]; then
        TAG=$(printf '%s\n' "$cands" | awk '$3 == "true" { print $1 }' | head -n 1)
        [ -n "$TAG" ] || { echo "仓库里没有可安装的 v* release" >&2; exit 1; }
        echo "说明: 仓库暂无 v* 稳定版，回退到最新预发布 $TAG（正式版发布后默认装稳定版）"
    fi
}

# ---- Linux libc 探测：输出 gnu 或 musl ----
# 静态链接的 musl 产物在任何 Linux 上都能跑，gnu 产物只在 glibc >= MIN_GLIBC 的系统上
# 能跑。判定顺序：ldd 自报 musl → 读 glibc 版本比较 → 其余未知情形先试 gnu
# （之后的 --version 自证兜底，失败仍会回退 musl）。
version_lt() {
    awk -v a="$1" -v b="$2" 'BEGIN {
        split(a, x, "."); split(b, y, ".")
        exit !((x[1] + 0 < y[1] + 0) || (x[1] + 0 == y[1] + 0 && x[2] + 0 < y[2] + 0))
    }'
}

detect_linux_libc() {
    # musl 的 ldd 把版本信息打到 stderr 且退出码非零，所以合并输出并忽略退出码
    ldd_out=$(ldd --version 2>&1 || true)
    case "$ldd_out" in
        *musl*) echo musl; return ;;
    esac
    glibc_ver=""
    if command -v getconf >/dev/null 2>&1; then
        glibc_ver=$(getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/^glibc \([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p')
    fi
    if [ -z "$glibc_ver" ]; then
        # ldd 首行形如 "ldd (Debian GLIBC 2.36-9+deb12u14) 2.36" / "ldd (GNU libc) 2.35"
        glibc_ver=$(printf '%s\n' "$ldd_out" | head -n 1 | grep -o '[0-9][0-9]*\.[0-9][0-9]*' | tail -n 1 || true)
    fi
    if [ -n "$glibc_ver" ]; then
        if version_lt "$glibc_ver" "$MIN_GLIBC"; then echo musl; else echo gnu; fi
        return
    fi
    # ldd/getconf 都读不到：有 musl 动态加载器且没有 glibc 痕迹时按 musl 处理
    for f in /lib/ld-musl-*; do
        [ -e "$f" ] && { echo musl; return; }
    done
    echo gnu
}

# self_test <二进制>：--version 能正常退出才算可运行。
# </dev/null：curl | sh 时脚本本身占着 stdin，子进程不得读走它。
self_test() {
    "$1" --version </dev/null >/dev/null 2>&1
}

# download_binary <asset>：下载并校验到 $STAGED
download_binary() {
    echo "下载二进制 $1 ..."
    fetch "$BASE/$1" "$STAGED"
    fetch "$BASE/$1.sha256" "$TMP_DIR/aproxy.sha256"
    verify_sha256 "$STAGED" "$TMP_DIR/aproxy.sha256"
    chmod 755 "$STAGED"
}

main() {
    want_pre="${APROXY_PRE:-0}"
    TAG=""
    for arg in "$@"; do
        case "$arg" in
            --pre) want_pre=1 ;;
            -*) echo "未知参数: $arg（可用: --pre、tag）" >&2; exit 1 ;;
            *) TAG="$arg" ;;
        esac
    done
    # tag 会拼进下载 URL，限定字符集
    case "$TAG" in
        *[!A-Za-z0-9._-]*) echo "非法的 tag: $TAG" >&2; exit 1 ;;
    esac

    # ---- 平台解析（先于已安装检测：Windows 落位 aproxy.exe，检测名随平台）----
    OS="$(uname -s)"; ARCH="$(uname -m)"
    # EXE_SUFFIX/BAT_FALLBACK：Windows 侧（含 Git Bash/MSYS）二进制带 .exe 且需要
    # aproxy.bat fallback 入口（PATHEXT 机制，见 install 计划 swapping 专节）
    EXE_SUFFIX=""
    BAT_FALLBACK=0
    libc=""
    case "$OS" in
        Linux) platform="unknown-linux-gnu"; libc="$(detect_linux_libc)" ;;
        Darwin) platform="apple-darwin" ;;
        MINGW*|MSYS*|CYGWIN*) platform="pc-windows-msvc"; EXE_SUFFIX=".exe"; BAT_FALLBACK=1 ;;
        *) echo "不支持的平台: $OS" >&2; exit 1 ;;
    esac
    case "$ARCH" in
        x86_64|amd64) rust_arch="x86_64" ;;
        aarch64|arm64) rust_arch="aarch64" ;;
        *) echo "不支持的架构: $ARCH" >&2; exit 1 ;;
    esac
    [ "$libc" = "musl" ] && platform="unknown-linux-musl"
    BIN_NAME="aproxy${EXE_SUFFIX}"

    # ---- 已安装检测：装机后的升级归 install 管，本脚本只做首装 ----
    if [ -f "$BIN_DIR/$BIN_NAME" ]; then
        echo "aProxy 已安装于 $BIN_DIR/$BIN_NAME，本脚本不做覆盖。"
        echo "升级请使用: aproxy install"
        exit 0
    fi

    pick_sha_tool
    mkdir -p "$BIN_DIR" "$SKILLS_DIR" "$TMP_DIR"
    # 任何出口（含校验失败/自证失败）都清掉暂存目录，不留残骸
    trap 'rm -rf "$TMP_DIR"' EXIT

    # ---- 解析版本 ----
    [ -n "$TAG" ] || resolve_tag
    echo "安装 aProxy $TAG"
    [ "$libc" = "musl" ] && echo "检测到 musl 或 glibc < $MIN_GLIBC 的 Linux，选用静态链接的 musl 产物"

    # 本脚本不探测 AVX2（保守拉 baseline）；指令集变体选择由 `aproxy install` 做
    BASE="https://github.com/$REPO/releases/download/$TAG"
    # 暂存文件带平台后缀：Windows 侧自证需要 .exe 才能直接执行
    STAGED="$TMP_DIR/aproxy${EXE_SUFFIX}"

    # ---- 下载二进制 + 校验 + 自证 + 落位 ----
    ASSET="aproxy-${rust_arch}-${platform}${EXE_SUFFIX}"
    download_binary "$ASSET"
    if ! self_test "$STAGED"; then
        if [ "$libc" = "gnu" ]; then
            # 版本探测可能误判（如 glibc 满足下限但缺别的系统库）：静态 musl 产物兜底
            echo "gnu 产物在本机无法运行，改用 musl 产物重试 ..." >&2
            ASSET="aproxy-${rust_arch}-unknown-linux-musl"
            download_binary "$ASSET"
        fi
        if ! self_test "$STAGED"; then
            echo "安装失败: 下载的二进制（$ASSET）在本机无法运行（--version 自检未通过）。未做任何落位。" >&2
            # 暂存文件随后会被 EXIT trap 删除：趁它还在，把系统给出的原因打出来
            "$STAGED" --version </dev/null 2>&1 | head -n 5 >&2 || true
            exit 1
        fi
    fi
    # 自证通过才落位：bin 目录里永远不会出现跑不起来的二进制
    mv "$STAGED" "$BIN_DIR/$BIN_NAME"

    # ---- Windows：fallback 入口脚本（exe 缺席时重定向旧二进制）----
    if [ "$BAT_FALLBACK" = "1" ]; then
        printf '@echo off\r\nif exist "%%~dp0aproxy.exe" (\r\n  "%%~dp0aproxy.exe" %%*\r\n) else if exist "%%~dp0aproxy.old.exe" (\r\n  "%%~dp0aproxy.old.exe" %%*\r\n)\r\n' \
            > "$BIN_DIR/aproxy.bat"
    fi

    # ---- skill 文档（非强制：失败不影响安装）----
    if [ "${APROXY_NO_SKILLS:-}" != "1" ]; then
        if fetch "$BASE/aproxy-skills.zip" "$TMP_DIR/aproxy-skills.zip" 2>/dev/null \
            && fetch "$BASE/aproxy-skills.zip.sha256" "$TMP_DIR/aproxy-skills.zip.sha256" 2>/dev/null; then
            expected=$(cut -d' ' -f1 "$TMP_DIR/aproxy-skills.zip.sha256" | tr 'A-F' 'a-f')
            actual=$(sha256_of "$TMP_DIR/aproxy-skills.zip" | tr 'A-F' 'a-f') || actual=""
            if [ -n "$actual" ] && [ "$expected" = "$actual" ]; then
                # unzip 缺失时降级 python（macOS 旧版/精简容器无 unzip）；两者都没有
                # 或解压失败只跳过 skill，不能因 set -e 中止已经成功的二进制安装
                if command -v unzip >/dev/null 2>&1; then
                    unzip -oq "$TMP_DIR/aproxy-skills.zip" -d "$SKILLS_DIR" </dev/null
                elif command -v python3 >/dev/null 2>&1; then
                    python3 -c "import zipfile,sys; zipfile.ZipFile(sys.argv[1]).extractall(sys.argv[2])" \
                        "$TMP_DIR/aproxy-skills.zip" "$SKILLS_DIR" </dev/null
                else
                    false
                fi \
                    && echo "skill 文档已就位 $SKILLS_DIR（按所用 agent 的方式链接/复制到其 skills 目录）" \
                    || echo "skill 文档解压失败（需要 unzip 或 python3），跳过（不影响安装）"
            else
                echo "skill 文档 SHA256 校验失败，跳过（不影响安装）"
            fi
        else
            echo "skill 文档下载失败，跳过（不影响安装，可稍后用 aproxy install 重试）"
        fi
    fi

    # ---- 提示 ----
    echo ""
    echo "aProxy 已安装: $BIN_DIR/$BIN_NAME"
    case ":$PATH:" in
        *":$BIN_DIR:"*) ;;
        *)
            echo "注意: $BIN_DIR 不在 PATH 中——加入后即可直接使用 aproxy 命令："
            echo "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.profile && source ~/.profile"
            ;;
    esac
    echo "下一步: 先配置上游（新装机没有 base_url，直接启动会失败），再启动："
    echo "  aproxy config --baseurl <上游地址> --api-key <密钥>"
    echo "  aproxy        （或完整路径 \"$BIN_DIR/$BIN_NAME\"）"
    echo "状态: aproxy status    升级: aproxy install"
}

# 主体包在函数里并在最后一行才调用：curl | sh 下载被截断时，不完整的函数定义
# 是语法错误，不会执行半截脚本
main "$@"
