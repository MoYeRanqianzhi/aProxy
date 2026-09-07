# 优化验证用 mock 上游（同前次内存分析的 mock：/err 奇 500 偶 200）
param([int]$Port = 28080)
$listener = [System.Net.HttpListener]::new()
$listener.Prefixes.Add("http://127.0.0.1:$Port/")
$listener.Start()
Write-Output "mock-upstream listening on $Port"
$script:count = 0
$small = [System.Text.Encoding]::UTF8.GetBytes('{"ok":true,"data":"mock-small-response"}')
$errBody = [System.Text.Encoding]::UTF8.GetBytes('{"type":"error","error":{"type":"overloaded_error","message":"mock upstream overloaded"}}')
while ($listener.IsListening) {
    $ctx = $listener.GetContext()
    try {
        $out = $ctx.Response.OutputStream
        $p = $ctx.Request.Url.AbsolutePath
        if ($p -like '*err*') {
            $script:count++
            if ($script:count % 2 -eq 1) {
                $ctx.Response.StatusCode = 500
                $ctx.Response.ContentType = 'application/json'
                $ctx.Response.ContentLength64 = $errBody.Length
                $out.Write($errBody, 0, $errBody.Length)
                $out.Close()
            } else {
                $ctx.Response.ContentType = 'application/json'
                $ctx.Response.ContentLength64 = $small.Length
                $out.Write($small, 0, $small.Length)
                $out.Close()
            }
        } else {
            $ctx.Response.ContentType = 'application/json'
            $ctx.Response.ContentLength64 = $small.Length
            $out.Write($small, 0, $small.Length)
            $out.Close()
        }
    }
    catch { try { $ctx.Response.Abort() } catch {} }
}
