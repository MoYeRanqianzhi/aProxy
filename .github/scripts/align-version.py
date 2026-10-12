"""把要发布的版本号写进 Cargo.toml 与 Cargo.lock（release.yml 里每个构建、打包、发布 job 都先跑它）。

产物的 `--version` 自报的就是 Cargo.toml 里的版本，安装器下载后核对「自报 == 要装的版本」，
crates.io 也按 .crate 里的 manifest 登记版本——所以构建与发布之前必须先对齐。漏掉对齐时，tag
v…t1 发出去的产物仍自报旧版本，按版本安装的全部渠道自证失败，crates.io 也会把旧版本号永久占掉
（alpha.10、alpha.11 就是这么被占的）。

版本从哪来：
- tag 推送（GITHUB_EVENT_NAME=push）：tag 去掉开头的 v；
- 手动演练（workflow_dispatch）：输入的 version（环境变量 DRY_RUN_VERSION），留空就用 Cargo.toml
  里现有的版本。

改动的地方（aproxy 与 aproxy-desktop 同步发版，见 aproxy-desktop/Cargo.toml 开头；aproxy-envelope
在 aproxy 0.2.0 正式版之前也随主线同号发预发布版，见 aproxy-envelope/Cargo.toml 开头）：
- 根 Cargo.toml 的 [package] version，以及它对 aproxy-desktop、aproxy-envelope 的版本要求（`=` 精确钉住）；
- aproxy-desktop/Cargo.toml 与 aproxy-envelope/Cargo.toml 的 version；
- aproxy-format/Cargo.toml 对 aproxy-envelope 的版本要求（普通的 caret 要求，与 envelope 同号）；
- Cargo.lock 里 aproxy、aproxy-desktop、aproxy-envelope 三个条目的 version。
aproxy-format 自己的版本属于它独立的发版线（release-format workflow），不动。

最后把版本写进 $GITHUB_ENV 的 RELEASE_VERSION，同一 job 后面的步骤直接用。

读写一律显式 UTF-8（Windows runner 默认按 cp1252 解码会崩）、newline='' 原样保留行尾，正则里的
\\r?\\n 兼容 CRLF；不用 sed，BSD 与 GNU 的 -i 写法不同。cargo publish 要配 --allow-dirty：工作区
是有意改成这个版本的。
"""

import os
import re
import sys


def read(path):
    with open(path, encoding="utf-8", newline="") as f:
        return f.read()


def write(path, text):
    with open(path, "w", encoding="utf-8", newline="") as f:
        f.write(text)


def substitute(text, pattern, replacement, what, flags=0):
    """替换恰好一处；找不到或不止一处都说明文件的样子变了，直接失败，不猜。"""
    text, n = re.subn(pattern, replacement, text, flags=flags)
    if n != 1:
        sys.exit(f"align-version: {what} 应恰好出现一次，实际 {n} 次")
    return text


def main():
    root = read("Cargo.toml")
    current = re.search(r'^version = "([^"]*)"', root, flags=re.M)
    if current is None:
        sys.exit("align-version: 根 Cargo.toml 里没有 [package] version 行")

    if os.environ.get("GITHUB_EVENT_NAME") == "push":
        ref = os.environ.get("GITHUB_REF_NAME", "")
        if not ref.startswith("v") or len(ref) < 2:
            sys.exit(f"align-version: tag 推送触发，但 GITHUB_REF_NAME 不是 v<版本>：{ref!r}")
        version = ref[1:]
    else:
        version = os.environ.get("DRY_RUN_VERSION", "").strip() or current.group(1)
    # 版本号会拼进下面的替换文本与文件名，只放行 semver 用得到的字符
    if not re.fullmatch(r"[0-9A-Za-z.+-]+", version):
        sys.exit(f"align-version: 版本号含非法字符：{version!r}")

    # 根 Cargo.toml：[workspace] 段没有 version 行，第一条 ^version = 就是 [package] 的
    root = substitute(root, r'^version = "[^"]*"', f'version = "{version}"', "根 Cargo.toml 的 [package] version", re.M)
    for name in ("aproxy-desktop", "aproxy-envelope"):
        root = substitute(
            root,
            rf'^({name} = \{{ version = )"[^"]*"',
            rf'\g<1>"={version}"',
            f"根 Cargo.toml 对 {name} 的依赖行",
            re.M,
        )
    write("Cargo.toml", root)

    for member in ("aproxy-desktop", "aproxy-envelope"):
        path = f"{member}/Cargo.toml"
        text = substitute(read(path), r'^version = "[^"]*"', f'version = "{version}"', f"{path} 的 version", re.M)
        write(path, text)

    # aproxy-format 不钉死：caret 要求与 envelope 同号，工作区解析得了，将来的正式版也满足
    fmt = read("aproxy-format/Cargo.toml")
    fmt = substitute(
        fmt,
        r'^(aproxy-envelope = \{ version = )"[^"]*"',
        rf'\g<1>"{version}"',
        "aproxy-format/Cargo.toml 对 aproxy-envelope 的依赖行",
        re.M,
    )
    write("aproxy-format/Cargo.toml", fmt)

    lock = read("Cargo.lock")
    for name in ("aproxy", "aproxy-desktop", "aproxy-envelope"):
        lock = substitute(
            lock,
            rf'(name = "{name}"\r?\nversion = )"[^"]*"',
            rf'\g<1>"{version}"',
            f"Cargo.lock 里 {name} 的版本行",
        )
    write("Cargo.lock", lock)

    env_file = os.environ.get("GITHUB_ENV")
    if env_file:
        with open(env_file, "a", encoding="utf-8") as f:
            f.write(f"RELEASE_VERSION={version}\n")
    print(f"version -> {version}")


main()
