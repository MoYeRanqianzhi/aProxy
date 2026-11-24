"""Linux 自定义标题栏的实机核对：X11（Xvfb + openbox）上用 xdotool 模拟鼠标，逐项操作窗口并读回结果。

窗口没有窗口管理器的装饰（aproxy-desktop/src/lib.rs 的 decorations(false)），标题栏、三个窗口按钮、
拖动与双击最大化都是页面做的（components/Titlebar.svelte）。这里核对：没有装饰、拖动、双击最大化与还原、
最大化/还原按钮、右边缘改大小、最小化、关闭；每一项打印 PASS/FAIL 与读到的几何或状态，关键步骤截全屏。

用法：python linux-titlebar.py <aproxy 可执行文件> <输出目录>
临时 home、演示数据，不碰 ~/.aproxy。有一项 FAIL 时退出码为 1。
"""

import os
import subprocess
import sys
import tempfile
import time

AP, OUT = sys.argv[1], sys.argv[2]
os.makedirs(OUT, exist_ok=True)
home = tempfile.mkdtemp(prefix="aproxy-titlebar-")
env = dict(os.environ, APROXY_HOME=home, APROXY_LANG="en", APROXY_DESKTOP_DEMO="en,light")

# 标题栏的尺寸（Titlebar.svelte）：栏高 44px，按钮 32px、间隔 4px、离右边 8px，竖直居中
BAR, BUTTON, GAP, RIGHT = 44, 32, 4, 8


def sh(*args):
    return subprocess.run([str(a) for a in args], capture_output=True, text=True).stdout.strip()


def geom(wid):
    out = dict(line.split("=", 1) for line in sh("xdotool", "getwindowgeometry", "--shell", wid).splitlines() if "=" in line)
    return int(out["X"]), int(out["Y"]), int(out["WIDTH"]), int(out["HEIGHT"])


def state(wid):
    return sh("xprop", "-id", wid, "_NET_WM_STATE", "WM_STATE").replace("\n", " | ")


def shot(name):
    subprocess.run(["import", "-window", "root", os.path.join(OUT, name + ".png")])


def button(wid, i):
    """右起第 i 个按钮的中心：0 关闭，1 最大化/还原，2 最小化"""
    x, y, w, _ = geom(wid)
    return x + w - RIGHT - BUTTON // 2 - i * (BUTTON + GAP), y + BAR // 2


def click(px, py, times=1):
    sh("xdotool", "mousemove", px, py)
    time.sleep(0.3)
    sh("xdotool", "click", "--repeat", times, "--delay", 90, 1)
    time.sleep(1.2)


def drag(px, py, dx, dy):
    sh("xdotool", "mousemove", px, py)
    time.sleep(0.3)
    sh("xdotool", "mousedown", 1)
    time.sleep(0.2)
    for i in range(1, 11):
        sh("xdotool", "mousemove", px + dx * i // 10, py + dy * i // 10)
        time.sleep(0.05)
    sh("xdotool", "mouseup", 1)
    time.sleep(0.8)


failed = []


def check(name, ok, detail):
    if not ok:
        failed.append(name)
    print(f"[{'PASS' if ok else 'FAIL'}] {name}: {detail}", flush=True)


log = open(os.path.join(OUT, "desktop.log"), "w")
desk = subprocess.Popen([AP, "desktop"], env=env, stdout=log, stderr=subprocess.STDOUT)
time.sleep(12)
wids = sh("xdotool", "search", "--onlyvisible", "--pid", desk.pid).split()
if not wids:
    print("no window found", flush=True)
    sys.exit(1)
wid = wids[-1]
x, y, w, h = geom(wid)
print(f"window {wid}: {x},{y} {w}x{h}; {state(wid)}", flush=True)
shot("00-start")

ext = sh("xprop", "-id", wid, "_NET_FRAME_EXTENTS")
check("no window-manager decorations", "not found" in ext or ext.replace(" ", "").endswith("=0,0,0,0"), ext)

# 拖动品牌区（不接收指针，按下落在栏本身上）
x, y, w, h = geom(wid)
drag(x + 70, y + BAR // 2, 150, 100)
x2, y2, _, _ = geom(wid)
# 窗口管理器从收到 _NET_WM_MOVERESIZE 才开始接管指针，之前的第一小段移动不算数：只要求走了大半段路，且没有走过头
check("dragging the bar moves the window", 100 <= x2 - x <= 174 and 60 <= y2 - y <= 124, f"{x},{y} -> {x2},{y2}")
shot("01-dragged")

# 双击栏的空白处：最大化，再双击还原
x, y, w, h = geom(wid)
click(x + 70, y + BAR // 2, times=2)
check("double-click maximizes", "MAXIMIZED" in state(wid), state(wid))
shot("02-double-click-maximized")
x, y, w, h = geom(wid)
click(x + 70, y + BAR // 2, times=2)
check("double-click restores", "MAXIMIZED" not in state(wid), state(wid))

# 最大化按钮，再点一次还原（按钮图标跟着变，截图里看）
click(*button(wid, 1))
check("maximize button maximizes", "MAXIMIZED" in state(wid), state(wid))
shot("03-maximize-button")
click(*button(wid, 1))
check("restore button restores", "MAXIMIZED" not in state(wid), state(wid))

# 右边缘往里拖 120px：Tauri 在 webview 四边留了 5px 的改大小区（tauri-runtime-wry 的 undecorated_resizing）。
# 往里而不是往外拖：前面拖动标题栏后窗口的右边可能正贴着屏幕右边，往外拖指针被屏幕边挡住、窗口不会变
x, y, w, h = geom(wid)
drag(x + w - 2, y + h // 2, -120, 0)
_, _, w2, _ = geom(wid)
check("dragging the right edge resizes", 60 <= w - w2 <= 150, f"width {w} -> {w2}")

# 悬停关闭按钮：截图看珊瑚色的悬停底
sh("xdotool", "mousemove", *button(wid, 0))
time.sleep(0.6)
shot("04-close-hover")

# 最小化，再把窗口找回来
click(*button(wid, 2))
st = state(wid)
check("minimize button minimizes", "Iconic" in st or "HIDDEN" in st, st)
sh("xdotool", "windowmap", wid)
sh("xdotool", "windowactivate", wid)
time.sleep(1.2)

# 关闭按钮：窗口关掉，进程退出
click(*button(wid, 0))
try:
    desk.wait(timeout=10)
    closed = True
except subprocess.TimeoutExpired:
    closed = False
check("close button closes the window", closed, f"exit code {desk.returncode}")
if not closed:
    desk.terminate()

subprocess.run([AP, "stop", "all"], env=env, capture_output=True, timeout=60)
print(f"{len(failed)} failed: {failed}" if failed else "all passed", flush=True)
sys.exit(1 if failed else 0)
