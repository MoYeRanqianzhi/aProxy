"""把构建出的二进制按发布资产名放进暂存目录，并写出 `<资产名>.sha256`。

`.sha256` 的内容是「64 位小写十六进制、两个空格、资产名」，末尾不换行：普通在线安装的 github 渠道与
安装脚本取第一个空白之前的部分比对，release 说明里的 SHA256 表也从这里读。用 Python 写，是因为同一段
暂存要在 Windows、macOS、Linux 宿主机与 ubuntu:22.04 容器里都能跑，各处现成的哈希命令不一样。

用法：stage-asset.py <构建出的二进制> <暂存目录> <资产名>
"""

import hashlib
import os
import shutil
import sys

src, dest_dir, asset = sys.argv[1:4]
os.makedirs(dest_dir, exist_ok=True)
dest = os.path.join(dest_dir, asset)
shutil.copyfile(src, dest)
digest = hashlib.sha256()
with open(dest, "rb") as f:
    for chunk in iter(lambda: f.read(1 << 20), b""):
        digest.update(chunk)
with open(dest + ".sha256", "w", encoding="ascii", newline="") as f:
    f.write(f"{digest.hexdigest()}  {asset}")
print(f"{asset}  {digest.hexdigest()}")
