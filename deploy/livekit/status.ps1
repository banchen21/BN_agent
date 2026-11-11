# 查看 BN_agent LiveKit server / Caddy TLS 反代状态与端口
$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot

$exe = Join-Path $PSScriptRoot "bin\livekit-server.exe"
$caddy = Join-Path $PSScriptRoot "bin\caddy.exe"

$proc = Get-Process livekit-server -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $exe }
if ($proc) { Write-Host "LiveKit: PID $($proc.Id -join ',')" } else { Write-Warning "LiveKit: 未运行（.\start.ps1 启动）" }
$caddyProc = Get-Process caddy -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $caddy }
if ($caddyProc) { Write-Host "Caddy: PID $($caddyProc.Id -join ',')" } else { Write-Warning "Caddy: 未运行（HTTPS/wss 不可用）" }

Write-Host "`n--- TCP 监听（7443/7880/7881/18443/18080）---"
Get-NetTCPConnection -State Listen -ErrorAction SilentlyContinue |
    Where-Object { $_.LocalPort -in 7443, 7880, 7881, 18443, 18080 } |
    Select-Object LocalAddress, LocalPort, OwningProcess | Sort-Object LocalPort | Format-Table

Write-Host "--- UDP 监听（7882-7892，媒体按需打开）---"
Get-NetUDPEndpoint -ErrorAction SilentlyContinue |
    Where-Object { $_.LocalPort -ge 7882 -and $_.LocalPort -le 7892 } |
    Select-Object LocalAddress, LocalPort | Sort-Object LocalPort | Format-Table

Write-Host "--- HTTP 探活 ---"
foreach ($url in @("http://127.0.0.1:7880/", "http://192.168.0.101:7880/", "https://127.0.0.1:7443/", "https://192.168.0.101:7443/")) {
    $insecure = $url.StartsWith("https:")
    if (Get-Command curl.exe -ErrorAction SilentlyContinue) {
        $curlArgs = @("-s", "-o", "NUL", "-m", "3", "-w", "%{http_code}")
        if ($insecure) { $curlArgs += "-k" }
        $curlArgs += $url
        $code = & curl.exe @curlArgs
        if ($code -and $code -ne "000") {
            Write-Host "OK   $url -> HTTP $code"
        } else {
            Write-Warning "FAIL $url"
        }
    } else {
        try {
            $resp = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 3
            Write-Host "OK   $url -> HTTP $($resp.StatusCode)"
        } catch {
            Write-Warning "FAIL $url -> $($_.Exception.Message)"
        }
    }
}
