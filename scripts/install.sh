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
#   curl -fsSL .../install.sh | sh -s -- --variant headless   # 指定变体
#   或本地执行：sh scripts/install.sh [--pre] [--variant desktop|headless] [<tag>]
#   <tag> 是 Releases 页上的版本号，形如 vX.Y.Z 或 vX.Y.Z-alpha.N；写了就不查 release 列表
# 变体：带桌面界面的 desktop（`aproxy desktop` 打开窗口）与无界面的 headless，代理与命令行
# 完全相同。没指定时 Windows 与 macOS 装 desktop；Linux 先看本机能不能跑带界面的版本（它动态
# 链接 GTK 与 WebKitGTK，缺库或库太旧时连 --version 都起不来）：musl、glibc 低于
# DESKTOP_MIN_GLIBC、找不到 libwebkit2gtk-4.1 的装 headless 并说明原因；下载后带界面的版本
# 自检跑不起来（例如 WebKitGTK 低于 2.40）也改装 headless。指定了 desktop 而本机跑不了就报错退出。
# 选版规则（无显式 tag 时）：
#   1. 只认符合项目版本号文法的 tag：vX.Y.Z 或 vX.Y.Z-(alpha|beta|rc).N（同仓库的
#      format-v* 是另一条发版线；v0.1.0-alpha.12t3 这类历史测试 tag 按 semver 会排在
#      alpha.17 之上，必须排除），并跳过 draft；
#   2. 默认取最新的稳定版（非 prerelease，按版本号取最大）；
#   3. 仓库里一个 v* 稳定版都没有时回退到版本号最大的 v* 预发布并打印说明（v0.1.0 起仓库里
#      已有稳定版，这一条只是兜底）；
#   4. --pre（或 APROXY_PRE=1）：取版本号最大的 v* release，预发布也可（与 `aproxy install
#      --pre` 同一规则：按版本号而非创建时间，补丁版晚于预发布发布时也不会装到更低版本）。
# 环境变量：
#   APROXY_HOME     目录根（默认 ~/.aproxy；空或只含空白视为未设，否则必须是绝对路径）
#   APROXY_NO_SKILLS=1   跳过 skill 文档
#   APROXY_PRE=1    等效 --pre
#   APROXY_VARIANT  等效 --variant（desktop 或 headless）
#   APROXY_DL_PROXY 下载代理（仅本次；与上游请求代理完全无关）
set -eu

REPO="MoYeRanqianzhi/aProxy"
# glibc 版本下限：Linux gnu 产物按此基线构建，低于它的系统（如 RHEL/CentOS 7 的 glibc 2.17）
# 改选静态链接的 musl 产物。必须与 .github/workflows/release.yml 中 gnu 构建的
# glibc 下限、npm/aproxy/bin/aproxy.js 的 MIN_GLIBC 保持一致。
MIN_GLIBC="2.28"
# 带界面的 Linux gnu 产物能运行的最低 glibc。它链接构建系统里的 GTK 与 WebKitGTK，下限由构建
# 系统决定；必须与 release.yml 的 DESKTOP_GLIBC_FLOOR、npm/aproxy/bin/aproxy.js 的
# DESKTOP_MIN_GLIBC 保持一致。只用于没指定变体时替用户挑一个：本脚本每次都从 main 现取，
# 跟得上最新的构建；aproxy 自己的安装器不预先猜，只看下载下来的二进制能不能跑。
DESKTOP_MIN_GLIBC="2.34"
UA="aproxy-install-script"

# ---- aProxy 主目录（永久兼容契约 C1，与 aproxy 二进制同一规则）----
# 规则不一致，脚本装进去的目录就不是二进制认的 home：
# - APROXY_HOME 为空或只含空白视为未设，用 $HOME/.aproxy。$HOME 也为空时二进制改查系统的
#   用户数据库，脚本做不到同样的事，只能请用户设 APROXY_HOME。
# - 否则原样使用（不 trim），必须是绝对路径，不是就报错退出、什么都不装：相对路径会随工作
#   目录变成不同的目录。绝对路径以 / 开头；Windows 侧（Git Bash/MSYS/Cygwin）装的是 Windows
#   二进制，它还认 C:\…、C:/… 与 \\…——从 Windows 环境继承来的 APROXY_HOME 就是这种写法。
# - 空白按 [:space:] 判断。二进制认的是 Unicode 空白：全角空格之类的值在这里不算空白，会在
#   绝对路径检查处被拒——拒绝安装，不会装错地方。
# 调用前 OS 已经确定。
resolve_home() {
    case "${APROXY_HOME:-}" in
        *[![:space:]]*) ;;
        *)
            [ -n "${HOME:-}" ] || {
                echo "HOME 为空，确定不了默认的 aProxy 目录；请把 APROXY_HOME 设为绝对路径后重试。什么都没有安装。" >&2
                exit 1
            }
            APROXY_HOME="$HOME/.aproxy"
            return
            ;;
    esac
    case "$APROXY_HOME" in
        /*) return ;;
    esac
    case "$OS" in
        MINGW*|MSYS*|CYGWIN*)
            case "$APROXY_HOME" in
                [A-Za-z]:/*|[A-Za-z]:\\*|\\\\*) return ;;
            esac
            ;;
    esac
    echo "APROXY_HOME 必须是绝对路径，现在是 \"$APROXY_HOME\"：相对路径会随工作目录变成不同的目录。改成绝对路径，或者删掉这个环境变量（默认 \$HOME/.aproxy）。什么都没有安装。" >&2
    exit 1
}

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

# max_version：stdin 为 tag 列表，输出版本号最大者。只接受项目版本号文法
# vX.Y.Z 或 vX.Y.Z-(alpha|beta|rc).N，其余行忽略。比较键逐段比：主.次.补 → 预发布
# 档位（alpha 0 / beta 1 / rc 2 / 正式版 3，正式版高于同号的任何预发布）→ 预发布序号。
# 用 awk 逐段数值比较而不是 sort -V，避免依赖 sort 的 GNU 扩展（旧 BSD/busybox 不一定有）。
max_version() {
    awk '
        /^v[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|rc)\.[0-9]+)?$/ {
            n = split(substr($0, 2), p, /[.-]/)
            k[1] = p[1] + 0; k[2] = p[2] + 0; k[3] = p[3] + 0
            if (n == 3) { k[4] = 3; k[5] = 0 }
            else { k[4] = (p[4] == "alpha") ? 0 : ((p[4] == "beta") ? 1 : 2); k[5] = p[5] + 0 }
            better = !seen
            if (seen) for (i = 1; i <= 5; i++) if (k[i] != b[i]) { better = (k[i] > b[i]); break }
            if (better) { for (i = 1; i <= 5; i++) b[i] = k[i]; tag = $0; seen = 1 }
        }
        END { if (seen) print tag }
    '
}

# resolve_tag：设置全局 TAG；want_pre=1 时取版本号最大的 v* release（含预发布）
resolve_tag() {
    list="$TMP_DIR/releases.json"
    fetch "https://api.github.com/repos/$REPO/releases?per_page=100" "$list" \
        || { echo "无法获取 release 列表（网络不可达或 GitHub API 限流）" >&2; exit 1; }
    parsed=$(parse_releases < "$list") \
        || { echo "无法解析 GitHub release 列表（响应格式与预期不符）；可以写明要装的 tag，跳过列表查询：sh install.sh <tag>（经 curl 管道时是 sh -s -- <tag>），tag 取 Releases 页上的版本号，形如 vX.Y.Z 或 vX.Y.Z-alpha.N" >&2; exit 1; }
    [ -n "$parsed" ] || { echo "release 列表为空或响应格式与预期不符（可能是 API 限流页）；可以写明要装的 tag，跳过列表查询：sh install.sh <tag>（经 curl 管道时是 sh -s -- <tag>），tag 取 Releases 页上的版本号，形如 vX.Y.Z 或 vX.Y.Z-alpha.N" >&2; exit 1; }
    # 只留符合项目版本号文法且非 draft 的条目（排除 format-v* 与历史测试 tag）
    cands=$(printf '%s\n' "$parsed" | awk '$1 ~ /^v[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|rc)\.[0-9]+)?$/ && $2 == "false" { print }')
    [ -n "$cands" ] || { echo "仓库里没有可安装的 v* release" >&2; exit 1; }

    if [ "$want_pre" = "1" ]; then
        TAG=$(printf '%s\n' "$cands" | cut -d' ' -f1 | max_version)
        return
    fi
    # 稳定版：prerelease 标志为 false 且 tag 没有预发布后缀，两个信号都满足才算
    # （任一处标成预发布都不当稳定版装给默认用户）
    TAG=$(printf '%s\n' "$cands" | awk '$3 == "false" && $1 !~ /-/ { print $1 }' | max_version)
    if [ -z "$TAG" ]; then
        TAG=$(printf '%s\n' "$cands" | cut -d' ' -f1 | max_version)
        echo "说明: 仓库暂无 v* 稳定版，回退到版本号最大的预发布 $TAG（正式版发布后默认装稳定版）"
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

# 本机 glibc 的「主.次」版本（如 2.35）；musl 系统或读不出时输出空
glibc_version() {
    # musl 的 ldd 把版本信息打到 stderr 且退出码非零，所以合并输出并忽略退出码
    ldd_out=$(ldd --version 2>&1 || true)
    case "$ldd_out" in
        *musl*) return ;;
    esac
    v=""
    if command -v getconf >/dev/null 2>&1; then
        v=$(getconf GNU_LIBC_VERSION 2>/dev/null | sed -n 's/^glibc \([0-9][0-9]*\.[0-9][0-9]*\).*/\1/p')
    fi
    if [ -z "$v" ]; then
        # ldd 首行形如 "ldd (Debian GLIBC 2.36-9+deb12u14) 2.36" / "ldd (GNU libc) 2.35"
        v=$(printf '%s\n' "$ldd_out" | head -n 1 | grep -o '[0-9][0-9]*\.[0-9][0-9]*' | tail -n 1 || true)
    fi
    printf '%s\n' "$v"
}

detect_linux_libc() {
    case "$(ldd --version 2>&1 || true)" in
        *musl*) echo musl; return ;;
    esac
    glibc_ver=$(glibc_version)
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

# ---- 变体：本机能不能跑带界面的版本 ----
# 找得到 WebKitGTK 4.1 的运行库（libwebkit2gtk-4.1.so.0）就成功返回。先问动态加载器的缓存：
# ldconfig 在不少发行版的普通用户 PATH 之外（/sbin），所以连同常见路径都试；都没有就看常见的库目录。
# 版本够不够（2.40+）这里查不了，下载后 --version 自检会挡住太旧的。
find_webkit() {
    for ldc in ldconfig /sbin/ldconfig /usr/sbin/ldconfig; do
        if command -v "$ldc" >/dev/null 2>&1; then
            "$ldc" -p 2>/dev/null | grep -q 'libwebkit2gtk-4\.1\.so\.0'
            return
        fi
    done
    for f in /usr/lib/libwebkit2gtk-4.1.so.0 /usr/lib64/libwebkit2gtk-4.1.so.0 /usr/lib/*-linux-gnu/libwebkit2gtk-4.1.so.0; do
        [ -e "$f" ] && return 0
    done
    return 1
}

# 带界面的版本在本机跑不了时成功返回，原因写进 $blocker。只看 Linux（Windows、macOS 上它随处能跑，
# 缺 WebView2 只影响 aproxy desktop 本身）。调用前 OS 与 libc 已经确定。
desktop_blocker() {
    [ "$OS" = Linux ] || return 1
    if [ "$libc" = musl ]; then
        blocker="这台 Linux 用的是 musl 或 glibc 低于 $MIN_GLIBC，只有无界面的版本能用"
        return 0
    fi
    glibc_now=$(glibc_version)
    if [ -n "$glibc_now" ] && version_lt "$glibc_now" "$DESKTOP_MIN_GLIBC"; then
        blocker="本机 glibc $glibc_now 低于带界面版本要求的 $DESKTOP_MIN_GLIBC"
        return 0
    fi
    if ! find_webkit; then
        blocker="没有找到 WebKitGTK 4.1（libwebkit2gtk-4.1.so.0；Debian / Ubuntu 上装 libwebkit2gtk-4.1-0）"
        return 0
    fi
    return 1
}

# asset_for <desktop|headless> <目标三元组去掉架构的部分，如 unknown-linux-gnu>：发布资产名
asset_for() {
    prefix="aproxy-"
    [ "$1" = headless ] && prefix="aproxy-headless-"
    echo "${prefix}${rust_arch}-$2${EXE_SUFFIX}"
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
    VARIANT="${APROXY_VARIANT:-}"
    TAG=""
    while [ $# -gt 0 ]; do
        case "$1" in
            --pre) want_pre=1 ;;
            --variant)
                # 放在最后又没给值：先报错，不然下面的 shift 在 dash 里报「can't shift」、
                # 在 bash 的 sh 模式下静默以 1 退出，用户看不出原因
                [ $# -ge 2 ] || { echo "--variant 需要一个值（desktop 或 headless）" >&2; exit 1; }
                shift; VARIANT="$1" ;;
            --variant=*) VARIANT="${1#--variant=}" ;;
            -*) echo "未知参数: $1（可用: --pre、--variant desktop|headless、tag）" >&2; exit 1 ;;
            *) TAG="$1" ;;
        esac
        shift
    done
    case "$VARIANT" in
        ""|desktop|headless) ;;
        *) echo "未知变体: $VARIANT（可用: desktop、headless）" >&2; exit 1 ;;
    esac
    # tag 会拼进下载 URL，限定字符集
    case "$TAG" in
        *[!A-Za-z0-9._-]*) echo "非法的 tag: $TAG" >&2; exit 1 ;;
    esac

    # ---- 平台解析（先于已安装检测：Windows 落位 aproxy.exe，检测名随平台）----
    OS="$(uname -s)"; ARCH="$(uname -m)"
    # EXE_SUFFIX：Windows 侧（含 Git Bash/MSYS）二进制带 .exe。入口兜底脚本
    # （aproxy.bat）不在这里写：本脚本只做首装，`aproxy install` 每次交换二进制之前
    # 都会自己写好它——只有交换期间 aproxy.exe 才可能短暂缺席
    EXE_SUFFIX=""
    libc=""
    case "$OS" in
        Linux) platform="unknown-linux-gnu"; libc="$(detect_linux_libc)" ;;
        Darwin) platform="apple-darwin" ;;
        MINGW*|MSYS*|CYGWIN*) platform="pc-windows-msvc"; EXE_SUFFIX=".exe" ;;
        *) echo "不支持的平台: $OS" >&2; exit 1 ;;
    esac
    case "$ARCH" in
        x86_64|amd64) rust_arch="x86_64" ;;
        aarch64|arm64) rust_arch="aarch64" ;;
        *) echo "不支持的架构: $ARCH" >&2; exit 1 ;;
    esac
    [ "$libc" = "musl" ] && platform="unknown-linux-musl"
    BIN_NAME="aproxy${EXE_SUFFIX}"

    # ---- 变体（先于下载：要装哪个文件取决于它）----
    # 没指定变体时按本机情况挑：看起来跑不了带界面的版本就装 headless 并说明原因。
    # 指定了 desktop 就照装，跑不跑得起来交给下载后的 --version 自检（它会打出动态加载器的
    # 报错）：这里的探测可能误判（库装在动态加载器缓存之外、只靠 LD_LIBRARY_PATH 等），
    # 用户写明了就不能拦。只有 musl 例外：它没有带界面的产物，下载只会 404。
    # variant_auto=1：没指定变体、选了 desktop，下载后跑不起来时可以改装 headless；
    # headless_reason：最后装的是 headless 而不是用户指定的，向用户说明原因
    variant_auto=0
    headless_reason=""
    blocker=""
    if [ -z "$VARIANT" ]; then
        if desktop_blocker; then
            VARIANT=headless
            headless_reason="$blocker"
        else
            VARIANT=desktop
            variant_auto=1
        fi
    elif [ "$VARIANT" = desktop ] && [ "$libc" = musl ]; then
        echo "这台 Linux 用的是 musl 或 glibc 低于 $MIN_GLIBC，带桌面界面的版本只有 glibc 的构建；改装无界面版本：--variant headless" >&2
        exit 1
    fi

    resolve_home
    BIN_DIR="$APROXY_HOME/bin"
    SKILLS_DIR="$APROXY_HOME/skills"
    TMP_DIR="$APROXY_HOME/install/bootstrap"

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
    [ -n "$headless_reason" ] && echo "安装无界面的 headless 版本：$headless_reason"

    # 本脚本不探测 AVX2（保守拉 baseline）；指令集变体选择由 `aproxy install` 做
    BASE="https://github.com/$REPO/releases/download/$TAG"
    # 暂存文件带平台后缀：Windows 侧自证需要 .exe 才能直接执行
    STAGED="$TMP_DIR/aproxy${EXE_SUFFIX}"

    # ---- 下载二进制 + 校验 + 自证 + 落位 ----
    ASSET=$(asset_for "$VARIANT" "$platform")
    download_binary "$ASSET"
    if ! self_test "$STAGED" && [ "$variant_auto" = 1 ]; then
        # 没指定变体时，带界面的版本跑不起来（WebKitGTK 低于 2.40、缺别的界面库）就改装无界面的
        why=$("$STAGED" --version </dev/null 2>&1 | head -n 1 || true)
        echo "带桌面界面的版本在本机起不来（${why:-原因未知}），改装无界面的 headless 版本 ..." >&2
        VARIANT=headless
        headless_reason="带界面的版本在本机起不来（${why:-原因未知}）"
        ASSET=$(asset_for headless "$platform")
        download_binary "$ASSET"
    fi
    if ! self_test "$STAGED"; then
        if [ "$libc" = "gnu" ] && [ "$VARIANT" = headless ]; then
            # 版本探测可能误判（如 glibc 满足下限但缺别的系统库）：静态 musl 产物兜底
            echo "gnu 产物在本机无法运行，改用 musl 产物重试 ..." >&2
            ASSET=$(asset_for headless unknown-linux-musl)
            download_binary "$ASSET"
        fi
        if ! self_test "$STAGED"; then
            echo "安装失败: 下载的二进制（$ASSET）在本机无法运行（--version 自检未通过）。未做任何落位。" >&2
            # 暂存文件随后会被 EXIT trap 删除：趁它还在，把系统给出的原因打出来
            "$STAGED" --version </dev/null 2>&1 | head -n 5 >&2 || true
            # 走到这里的 desktop 都是用户写明的（没写明的上面已改装 headless）；写明时不按 glibc
            # 拦（见变体选择），所以 glibc 太旧也会落到这里，那时只提 WebKitGTK 会误导
            if [ "$VARIANT" = desktop ] && [ "$OS" = Linux ]; then
                glibc_now=$(glibc_version)
                if [ -n "$glibc_now" ] && version_lt "$glibc_now" "$DESKTOP_MIN_GLIBC"; then
                    echo "本机 glibc $glibc_now 太旧：带桌面界面的版本要 glibc $DESKTOP_MIN_GLIBC 或更新，还要 WebKitGTK 2.40 或更新；不需要窗口就改装无界面版本：--variant headless" >&2
                else
                    echo "带桌面界面的版本需要 WebKitGTK 2.40 或更新（Debian / Ubuntu：libwebkit2gtk-4.1-0）；不需要窗口就改装无界面版本：--variant headless" >&2
                fi
            fi
            exit 1
        fi
    fi
    # 自证通过才落位：bin 目录里永远不会出现跑不起来的二进制
    mv "$STAGED" "$BIN_DIR/$BIN_NAME"

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
    echo "aProxy 已安装: $BIN_DIR/$BIN_NAME（$VARIANT）"
    if [ -n "$headless_reason" ]; then
        echo "装的是无界面的 headless 版本（$headless_reason）：代理与命令行完全相同，只是没有 aproxy desktop 窗口。"
        echo "  装好所需的系统库之后，可用 aproxy install --variant desktop 换成带界面的版本。"
    fi
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
