"""从 Git 索引的普通文件生成可复现的 skill 总包；发布之前用安装器的规则核对它。"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile
import zipfile


ROOT = Path(__file__).resolve().parents[2]
PREFIX = b".claude/skills/"
ASSET = "aproxy-skills.zip"


def git(*args):
    """读取索引指向的对象，而不是经过 autocrlf、过滤器或本地编辑的工作副本。"""
    return subprocess.check_output(["git", *args], cwd=ROOT)


def build(version, target):
    """固定条目排序、Unix 权限和时间；不压缩让结果也不依赖构建机的 zlib 版本。"""
    contents = {}
    for line in git("ls-files", "--stage", "-z", "--", PREFIX.decode()).split(b"\0"):
        if not line:
            continue
        meta, path = line.split(b"\t", 1)
        mode, oid, stage = meta.split()
        if mode not in (b"100644", b"100755") or stage != b"0":
            raise ValueError("skill 输入不是已合并的普通文件")
        key = path[len(PREFIX):].decode("utf-8")
        contents[key] = git("cat-file", "blob", oid.decode("ascii"))
    files = {key: {"size": len(body), "sha256": hashlib.sha256(body).hexdigest()}
             for key, body in contents.items()}
    contents["manifest.json"] = json.dumps({"schema": 1, "version": version, "files": files},
                                           sort_keys=True, separators=(",", ":")).encode()
    with zipfile.ZipFile(target, "w") as archive:
        for key, body in sorted(contents.items()):
            entry = zipfile.ZipInfo(key, (1980, 1, 1, 0, 0, 0))
            entry.create_system = 3
            entry.external_attr = 0o100644 << 16
            archive.writestr(entry, body)


def main():
    """在输出卷的临时目录完成核验后再公开文件，失败不覆盖已有产物。"""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version")
    parser.add_argument("output", type=Path)
    group = parser.add_mutually_exclusive_group(required=True)
    group.add_argument("--validator", type=Path, help="本机可运行的 aProxy 二进制")
    group.add_argument("--verified-archive", type=Path, help="只读构建任务已经核验的同版本总包")
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(dir=args.output) as scratch:
        tmp = Path(scratch).resolve()
        target = tmp / ASSET
        build(args.version, target)
        body = target.read_bytes()
        if args.validator:
            # 校验器不使用安装目录，也不给它继承开发机的 home。
            subprocess.run([str(args.validator.resolve()), "skill", "__verify-package",
                            str(target), args.version], check=True,
                           env={**os.environ, "APROXY_HOME": str(tmp / "home")})
        elif args.verified_archive.read_bytes() != body:
            # 带发布凭据的任务不运行产物，只接受与 Rust 已核验的只读产物逐字节相同的包。
            raise ValueError("skill 包与已核验的发布产物不同")
        checksum = tmp / (ASSET + ".sha256")
        checksum.write_bytes(f"{hashlib.sha256(body).hexdigest()}  {ASSET}\n".encode("ascii"))
        target.replace(args.output / ASSET)
        checksum.replace(args.output / checksum.name)


if __name__ == "__main__":
    main()
