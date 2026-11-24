"""README 截图的场景：用桌面窗口的演示数据（APROXY_DESKTOP_DEMO，与浏览器里预览时同一套）打开窗口，
预设界面语言与深浅色，截全屏并按窗口裁出来。窗口外壳是真的（标题栏、交通灯、原生按钮），数据是演示的。

用法：python scene.py <aproxy 可执行文件> <输出目录> <lang: en|zh-CN> <mode: light|dark>
只用标准库起场景；截屏与裁剪用各平台自带的工具加 Pillow（工作流里 pip 装）。临时 home，不碰 ~/.aproxy。
"""

import os
import platform
import subprocess
import sys
import tempfile
import time

AP, OUT, LANG, MODE = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
os.makedirs(OUT, exist_ok=True)
SYSTEM = platform.system()

home = tempfile.mkdtemp(prefix="aproxy-shots-")
env = dict(os.environ, APROXY_HOME=home, APROXY_LANG="en", APROXY_DESKTOP_DEMO=f"{LANG},{MODE}")


def run(*args):
    r = subprocess.run([AP, *args], env=env, capture_output=True, text=True, timeout=120)
    print(f"$ aproxy {' '.join(args)} -> {r.returncode}\n{r.stdout[-2000:]}{r.stderr[-2000:]}", flush=True)
    return r


# ---------- 打开窗口 ----------
log = open(os.path.join(OUT, "desktop.log"), "w")
desk = subprocess.Popen([AP, "desktop"], env=env, stdout=log, stderr=subprocess.STDOUT)
time.sleep(14)

# ---------- 截屏与窗口位置 ----------
full = os.path.join(OUT, "full.png")
rect = None  # 屏幕像素 (left, top, right, bottom)
if SYSTEM == "Darwin":
    subprocess.run(["screencapture", "-x", full], check=True)
    q = subprocess.run(["osascript", "-e",
        f'tell application "System Events" to get {{position, size}} of window 1 of '
        f'(first process whose unix id is {desk.pid})'], capture_output=True, text=True)
    b = subprocess.run(["osascript", "-e", 'tell application "Finder" to get bounds of window of desktop'],
                       capture_output=True, text=True)
    print("window:", q.stdout, q.stderr, "screen:", b.stdout, flush=True)
    try:
        x, y, w, h = [float(v) for v in q.stdout.replace(" ", "").split(",")]
        sw = float(b.stdout.replace(" ", "").split(",")[2])
        from PIL import Image
        scale = Image.open(full).width / sw
        rect = (x * scale, y * scale, (x + w) * scale, (y + h) * scale)
    except Exception as e:
        print("no rect:", e)
elif SYSTEM == "Windows":
    ps = r"""
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class W {
  [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
  [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h, int a, out RECT r, int s);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern void keybd_event(byte vk, byte scan, int flags, IntPtr extra);
  public struct RECT { public int L, T, R, B; }
}
"@
[W]::SetProcessDPIAware() | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
# 从共享控制台启动时 aproxy desktop 会另起一个独立进程再退出（src/commands/desktop.rs），
# 所以按「有主窗口的 aproxy 进程」找，而不是用启动的那个 pid
$p = Get-Process aproxy | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
# 先把窗口切到前台再截：从后台进程调用 SetForegroundWindow 会被前台锁挡住，先按一下 Alt 就放行
[W]::keybd_event(0x12, 0, 0, [IntPtr]::Zero)
[W]::SetForegroundWindow($p.MainWindowHandle) | Out-Null
[W]::keybd_event(0x12, 0, 2, [IntPtr]::Zero)
Start-Sleep -Milliseconds 800
$g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save($args[0], [System.Drawing.Imaging.ImageFormat]::Png)
$r = New-Object W+RECT
# DWMWA_EXTENDED_FRAME_BOUNDS = 9：不含不可见的缩放边
[W]::DwmGetWindowAttribute($p.MainWindowHandle, 9, [ref]$r, 16) | Out-Null
"$($r.L),$($r.T),$($r.R),$($r.B)"
"""
    script = os.path.join(OUT, "shot.ps1")
    with open(script, "w", encoding="utf-8") as f:
        f.write(ps)
    q = subprocess.run(["powershell", "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", script, full, str(desk.pid)],
                       capture_output=True, text=True)
    print("window:", q.stdout, q.stderr, flush=True)
    try:
        rect = tuple(float(v) for v in q.stdout.strip().splitlines()[-1].split(","))
    except Exception as e:
        print("no rect:", e)
else:
    subprocess.run(["import", "-window", "root", full], check=True)
    wid = subprocess.run(["xdotool", "search", "--onlyvisible", "--pid", str(desk.pid)],
                         capture_output=True, text=True).stdout.split()
    print("windows:", wid, flush=True)
    if wid:
        info = subprocess.run(["xwininfo", "-frame", "-id", wid[-1]], capture_output=True, text=True).stdout
        print(info, flush=True)
        try:
            vals = {k.strip(): v.strip() for k, v in (l.split(":", 1) for l in info.splitlines() if ":" in l)}
            x, y = int(vals["Absolute upper-left X"]), int(vals["Absolute upper-left Y"])
            w, h = int(vals["Width"]), int(vals["Height"])
            # 窗口管理器画的边框（标题栏、边）不在内容区的几何里：按 _NET_FRAME_EXTENTS（左、右、上、下）向外扩
            ext = subprocess.run(["xprop", "-id", wid[-1], "_NET_FRAME_EXTENTS"], capture_output=True, text=True).stdout
            print(ext, flush=True)
            fl, fr, ft, fb = 0, 0, 0, 0
            if "=" in ext:
                fl, fr, ft, fb = [int(v) for v in ext.split("=", 1)[1].split(",")]
            rect = (x - fl, y - ft, x + w + fr, y + h + fb)
        except Exception as e:
            print("no rect:", e)

if rect:
    from PIL import Image
    im = Image.open(full)
    l, t, r, b = [int(round(v)) for v in rect]
    im.crop((max(l, 0), max(t, 0), min(r, im.width), min(b, im.height))).save(os.path.join(OUT, "window.png"))
    print("cropped", rect, flush=True)

# 关掉窗口，下一个场景才不会截到两层窗口。这是 CI runner 上本脚本自己起的进程
desk.terminate()
if SYSTEM == "Windows":
    subprocess.run(["powershell", "-NoProfile", "-Command",
                    "Get-Process aproxy -ErrorAction SilentlyContinue | Where-Object { $_.MainWindowHandle -ne 0 } | Stop-Process"])
time.sleep(2)
run("stop", "all")
