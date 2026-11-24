"""README 截图的场景：在临时 home 里起两个实例，经一个本地模拟上游打一批请求（一部分先 429 再成功，
界面上能看到重试与「救回」），预设界面语言与深浅色，打开桌面窗口，截全屏并按窗口裁出来。

用法：python scene.py <aproxy 可执行文件> <输出目录> <lang: en|zh-CN> <mode: light|dark>
只用标准库起场景；截屏与裁剪用各平台自带的工具加 Pillow（工作流里 pip 装）。不碰真实的 ~/.aproxy。
"""

import json
import os
import platform
import random
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

AP, OUT, LANG, MODE = sys.argv[1], sys.argv[2], sys.argv[3], sys.argv[4]
os.makedirs(OUT, exist_ok=True)
SYSTEM = platform.system()
random.seed(7)

# ---------- 模拟上游：每 5 个请求里有 2 个先回 429，下一次尝试成功 ----------
counter = {"n": 0}
lock = threading.Lock()


class Upstream(BaseHTTPRequestHandler):
    def do_POST(self):
        body = self.rfile.read(int(self.headers.get("content-length", 0) or 0))
        with lock:
            counter["n"] += 1
            n = counter["n"]
        time.sleep(random.uniform(0.3, 1.8))
        if n % 5 in (1, 3):
            data = b'{"type":"error","error":{"type":"rate_limit_error","message":"busy"}}'
            self.send_response(429)
            self.send_header("retry-after", "1")
        else:
            model = "claude-opus-5-5"
            try:
                model = json.loads(body or b"{}").get("model", model)
            except ValueError:
                pass
            data = json.dumps({
                "id": f"msg_{n:04d}", "type": "message", "role": "assistant", "model": model,
                "content": [{"type": "text", "text": "Done."}],
                "usage": {"input_tokens": random.randint(800, 9000), "output_tokens": random.randint(40, 900)},
            }).encode()
            self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *args):
        pass


server = ThreadingHTTPServer(("127.0.0.1", 0), Upstream)
UP = server.server_address[1]
threading.Thread(target=server.serve_forever, daemon=True).start()

# ---------- 临时 home、两份配置、界面偏好 ----------
home = tempfile.mkdtemp(prefix="aproxy-shots-")
env = dict(os.environ, APROXY_HOME=home, APROXY_LANG="en", NO_PROXY="127.0.0.1,localhost")
for name in ("config", "work"):
    with open(os.path.join(home, f"{name}.toml"), "w", encoding="utf-8") as f:
        f.write(f'base_url = "http://127.0.0.1:{UP}"\nlisten_addr = "127.0.0.1:0"\n')
os.makedirs(os.path.join(home, "desktop"), exist_ok=True)
with open(os.path.join(home, "desktop", "desktop.toml"), "w", encoding="utf-8") as f:
    f.write(f'lang = "{LANG}"\nmode = "{MODE}"\n')


def run(*args):
    r = subprocess.run([AP, *args], env=env, capture_output=True, text=True, timeout=120)
    print(f"$ aproxy {' '.join(args)} -> {r.returncode}\n{r.stdout[-2000:]}{r.stderr[-2000:]}", flush=True)
    return r


ports = []
for name in ("config", "work"):
    r = run("start", "--json", "--config", os.path.join(home, f"{name}.toml"))
    ports.append(json.loads(r.stdout)["port"])

# ---------- 一批请求：两种模型、两种路径，并发几路 ----------
MODELS = ["claude-opus-5-5", "claude-sonnet-5-5", "gpt-5.5"]


def send(i):
    port = ports[i % 2]
    model = MODELS[i % 3]
    path = "/v1/responses" if model.startswith("gpt") else "/v1/messages"
    req = urllib.request.Request(
        f"http://127.0.0.1:{port}{path}", method="POST",
        data=json.dumps({"model": model, "max_tokens": 512, "messages": [{"role": "user", "content": "hi"}]}).encode(),
        headers={"content-type": "application/json", "x-api-key": "sk-demo", "anthropic-version": "2023-06-01"},
    )
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    try:
        opener.open(req, timeout=90).read()
    except Exception as e:  # 截图只要界面里有记录，个别失败不影响
        print(f"request {i}: {e}", flush=True)


threads = []
for i in range(28):
    t = threading.Thread(target=send, args=(i,))
    t.start()
    threads.append(t)
    time.sleep(0.25)
for t in threads:
    t.join(120)
run("status")

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
  public struct RECT { public int L, T, R, B; }
}
"@
[W]::SetProcessDPIAware() | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
$b = [System.Windows.Forms.Screen]::PrimaryScreen.Bounds
$bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen($b.Location, [System.Drawing.Point]::Empty, $b.Size)
$bmp.Save($args[0], [System.Drawing.Imaging.ImageFormat]::Png)
# 从共享控制台启动时 aproxy desktop 会另起一个独立进程再退出（src/commands/desktop.rs），
# 所以按「有主窗口的 aproxy 进程」找，而不是用启动的那个 pid
$p = Get-Process aproxy | Where-Object { $_.MainWindowHandle -ne 0 } | Select-Object -First 1
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
            rect = (x, y, x + w, y + h)
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
