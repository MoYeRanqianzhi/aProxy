# 高并发压测编排 v2：10 起翻倍上不封顶。
# 终止条件（三重）：Private 峰值 >= 900MB（实测触线）｜预测峰值 > 1050MB（翻倍前守门，1GB 硬封顶）｜负载错误。
# 采样器每秒一拍（job 内参数名避开只读自动变量 $pid）。
param(
    [int]$ProxyPort = 25990,
    [int]$ProxyPid = 29204
)
$env:NO_PROXY = '127.0.0.1,localhost'
$loadgen = 'G:\ClaudeProjects\aProxy\target\release\examples\loadgen.exe'
$T0 = Get-Date

# 上一档实测的每并发在途内存成本（MB/并发），首档用估计值 9.5（请求 2.1 + spool 7.3）
$perConn = 9.5

$sampler = Start-Job -ScriptBlock {
    param($procId, $t0)
    while ($true) {
        $p = Get-Process -Id $procId -ErrorAction SilentlyContinue
        if ($p) {
            [pscustomobject]@{
                t       = [math]::Round(((Get-Date) - $t0).TotalSeconds, 1)
                ws_mb   = [math]::Round($p.WorkingSet64 / 1MB, 1)
                priv_mb = [math]::Round($p.PrivateMemorySize64 / 1MB, 1)
                threads = $p.Threads.Count
                handles = $p.HandleCount
            }
        }
        Start-Sleep -Milliseconds 1000
    }
} -ArgumentList $ProxyPid, $T0

$results = [System.Collections.Generic.List[object]]::new()
$c = 10
while ($c -le 4096) {
    # 翻倍前守门：预计峰值超 1050MB 的档位不开跑（1GB 硬封顶）
    $idleMb = [math]::Round((Get-Process -Id $ProxyPid).PrivateMemorySize64 / 1MB, 1)
    $predicted = $idleMb + $c * $perConn
    if ($predicted -gt 1050) {
        Write-Output "=== 预测峰值 $([math]::Round($predicted)) MB 超 1GB 封顶线，停止翻倍（末档实测: $c_prev）==="
        break
    }

    Start-Sleep -Seconds 12   # 档间静置：观察上一档内存回落
    $idleMb = [math]::Round((Get-Process -Id $ProxyPid).PrivateMemorySize64 / 1MB, 1)
    $s0 = [math]::Round(((Get-Date) - $T0).TotalSeconds, 1)
    Write-Output "=== 并发 $c | 档前空闲 Private: ${idleMb} MB | 预测峰值: $([math]::Round($predicted)) MB ==="
    $out = & $loadgen --target "http://127.0.0.1:$ProxyPort" -c $c -n ($c * 2) --timeout-secs 180 2>$null
    $s1 = [math]::Round(((Get-Date) - $T0).TotalSeconds, 1)
    $json = $out | Select-Object -Last 1 | ConvertFrom-Json
    Write-Output ($out | Select-Object -Last 1)
    if ($json.failed -gt 0) { Write-Output "出现失败请求，终止压测"; break }

    Start-Sleep -Seconds 2
    $win = Receive-Job $sampler -Keep | Where-Object { $_.t -ge $s0 -and $_.t -le ($s1 + 2) }
    $peak = [math]::Round(($win | Measure-Object -Property priv_mb -Maximum).Maximum, 1)
    $avg  = [math]::Round(($win | Measure-Object -Property priv_mb -Average).Average, 1)
    $threads = ($win | Measure-Object -Property threads -Maximum).Maximum
    Write-Output ">>> 并发 $c | Private 峰值: $peak MB（增量 $([math]::Round($peak - $idleMb, 1)) MB）| 均值: $avg MB | 线程峰值: $threads"
    $results.Add([pscustomobject]@{
        concurrency = $c; requests = $json.requests; ok = $json.ok; failed = $json.failed
        elapsed_s = $json.elapsed_s; rps = $json.rps; rx_mb = $json.bytes_rx_mb
        throughput_mb_s = $json.throughput_mb_s; p50_s = $json.latency_s.p50; p99_s = $json.latency_s.p99
        idle_mb = $idleMb; predicted_mb = [math]::Round($predicted); peak_mb = $peak
        delta_mb = [math]::Round($peak - $idleMb, 1); threads_max = $threads
    })
    $c_prev = $c
    # 实测校准每并发成本（用于下一档守门预测）
    if ($c -gt 0) { $perConn = [math]::Max(1.0, ($peak - $idleMb) / $c) }
    if ($peak -ge 900) { Write-Output "实测峰值 $peak MB 接近 1GB 封顶线，停止翻倍"; break }
    $c *= 2
}

Stop-Job $sampler
Receive-Job $sampler -Keep | Export-Csv -Path 'G:\ClaudeProjects\aProxy\.tmp-mem\samples.csv' -NoTypeInformation
$results | Export-Csv -Path 'G:\ClaudeProjects\aProxy\.tmp-mem\levels.csv' -NoTypeInformation
Write-Output "--- 压测结束：档位 $results.Count 个已存 levels.csv，全程采样已存 samples.csv ---"
