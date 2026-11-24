# 截图前清场：托管的 Windows 11 runner 第一次登录后会弹出系统的「Choose privacy settings for your device」
# 向导（首次登录体验），它盖在所有窗口上面，截到的就是它。这里先列出带窗口的进程（日志里看得到是谁），再结束
# 首次登录体验的那几个进程，并用策略关掉它，免得再弹。runner 是一次性的虚拟机，只在这里这么做。
"windows before:"
Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | Format-Table Id, ProcessName, MainWindowTitle -AutoSize | Out-String -Width 200
$oobe = 'WWAHost', 'CloudExperienceHostBroker', 'UserOOBEBroker', 'PrivacyExperience'
Get-Process -Name $oobe -ErrorAction SilentlyContinue | ForEach-Object {
  "stopping $($_.ProcessName) ($($_.Id))"
  Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
}
$key = 'HKLM:\SOFTWARE\Policies\Microsoft\Windows\OOBE'
New-Item -Path $key -Force | Out-Null
New-ItemProperty -Path $key -Name DisablePrivacyExperience -Value 1 -PropertyType DWord -Force | Out-Null
Start-Sleep -Seconds 2
"windows after:"
Get-Process | Where-Object { $_.MainWindowHandle -ne 0 } | Format-Table Id, ProcessName, MainWindowTitle -AutoSize | Out-String -Width 200
