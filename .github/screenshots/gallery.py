"""生成截图图集的首页 README.md：图集分支的根目录下是 windows/、macos/、linux/ 三个目录，
每个目录里的图按「场景-语言-深浅色.png」命名（截图工作流的 Collect the gallery 一步整理出来的）。

每个场景一节：一张三列（三个平台）、三行（英文浅色、英文深色、中文浅色）的表，格子里是缩小显示的原图，
点开是原尺寸。缺的图留空，不报错：某个平台某个场景没截成，首页上一眼就看得出来。

用法：python gallery.py <图集根目录> <生成说明（一行，例如提交号与日期）>
"""

import os
import sys

ROOT, NOTE = sys.argv[1], sys.argv[2]

PLATFORMS = [("windows", "Windows 11"), ("macos", "macOS"), ("linux", "Linux")]
VARIANTS = [("en-light", "English · light"), ("en-dark", "English · dark"), ("zh-CN-light", "简体中文 · 浅色")]
# 场景名与 aproxy-desktop/ui/src/lib/backend/demo/scenes.ts 的表一致，顺序就是首页上的顺序
SCENES = [
    ("overview", "Overview", "总览"),
    ("overview-lower", "Overview: watchdog and recent rescues", "总览：看护者与最近救回"),
    ("instance", "Instance retrying an unstable upstream", "实例：上游不稳、正在重试"),
    ("instance-restore", "Instance waiting to be restored", "实例：等待恢复"),
    ("logs", "Request log", "请求日志"),
    ("log-detail", "Request detail: overview", "请求详情：概览"),
    ("log-request", "Request detail: request", "请求详情：请求"),
    ("log-response", "Request detail: response, attempt by attempt", "请求详情：逐次尝试的响应"),
    ("log-format", "Request detail: what the format programs did", "请求详情：format 做了什么"),
    ("config", "Configuration editor", "配置"),
    ("update", "Updates", "更新"),
    ("settings", "Settings", "设置"),
    ("settings-packages", "Themes and extensions", "主题与扩展"),
    ("pulse", "A page added by an extension", "扩展加的页面"),
    ("palette", "Command palette", "命令面板"),
    ("start-menu", "Starting an instance", "启动实例"),
    ("empty", "No instances yet", "还没有实例"),
]


def cell(platform, scene, variant):
    path = f"{platform}/{scene}-{variant}.png"
    if not os.path.isfile(os.path.join(ROOT, path)):
        return " "
    return f'<a href="{path}"><img src="{path}" width="280"></a>'


lines = [
    "# aProxy desktop screenshots · 桌面界面截图",
    "",
    "Every page of the desktop window on Windows 11, macOS and Linux, with the built-in demo data. "
    "Click a picture for full size.",
    "",
    "桌面窗口在三个平台上的每一页，数据是内置的演示数据。点图片看原尺寸。",
    "",
    f"<sub>{NOTE}</sub>",
    "",
]
for scene, en, zh in SCENES:
    lines += [f"## {en} · {zh}", "", "| | " + " | ".join(name for _, name in PLATFORMS) + " |", "|---|---|---|---|"]
    for variant, label in VARIANTS:
        lines.append(f"| {label} | " + " | ".join(cell(p, scene, variant) for p, _ in PLATFORMS) + " |")
    lines.append("")

with open(os.path.join(ROOT, "README.md"), "w", encoding="utf-8", newline="\n") as f:
    f.write("\n".join(lines))
print(f"wrote {os.path.join(ROOT, 'README.md')}")
