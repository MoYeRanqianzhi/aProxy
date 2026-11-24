# 把 Windows runner 的主显示器调到给定分辨率（默认 1920×1080），列出可用的模式，打印改完以后的结果。
# 托管 runner 的屏幕默认只有 1024×768，桌面窗口会被压到 924×680 的窄布局；README 的截图要按默认的
# 1200×780 打开窗口，屏幕至少要比它大。ChangeDisplaySettings 只改当前会话，runner 用完即弃。
param([int]$Width = 1920, [int]$Height = 1080)
Add-Type @"
using System; using System.Runtime.InteropServices;
public static class Disp {
  [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
  public struct DEVMODE {
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmDeviceName;
    public short dmSpecVersion, dmDriverVersion, dmSize, dmDriverExtra;
    public int dmFields, dmPositionX, dmPositionY, dmDisplayOrientation, dmDisplayFixedOutput;
    public short dmColor, dmDuplex, dmYResolution, dmTTOption, dmCollate;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst = 32)] public string dmFormName;
    public short dmLogPixels;
    public int dmBitsPerPel, dmPelsWidth, dmPelsHeight, dmDisplayFlags, dmDisplayFrequency;
    public int dmICMMethod, dmICMIntent, dmMediaType, dmDitherType, dmReserved1, dmReserved2, dmPanningWidth, dmPanningHeight;
  }
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern bool EnumDisplaySettings(string dev, int mode, ref DEVMODE dm);
  [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int ChangeDisplaySettings(ref DEVMODE dm, int flags);
}
"@
$dm = New-Object Disp+DEVMODE
$dm.dmSize = [Runtime.InteropServices.Marshal]::SizeOf($dm)
$modes = @()
for ($i = 0; [Disp]::EnumDisplaySettings($null, $i, [ref]$dm); $i++) { $modes += "$($dm.dmPelsWidth)x$($dm.dmPelsHeight)" }
"available: " + (($modes | Sort-Object -Unique) -join ' ')
[Disp]::EnumDisplaySettings($null, -1, [ref]$dm) | Out-Null   # ENUM_CURRENT_SETTINGS
"before: $($dm.dmPelsWidth)x$($dm.dmPelsHeight)"
$dm.dmPelsWidth = $Width
$dm.dmPelsHeight = $Height
$dm.dmFields = 0x80000 -bor 0x100000                           # DM_PELSWIDTH | DM_PELSHEIGHT
"ChangeDisplaySettings: $([Disp]::ChangeDisplaySettings([ref]$dm, 0))"   # 0 = DISP_CHANGE_SUCCESSFUL
[Disp]::EnumDisplaySettings($null, -1, [ref]$dm) | Out-Null
"after: $($dm.dmPelsWidth)x$($dm.dmPelsHeight)"
