# 启动 BN_agent 独立 LiveKit server（Windows 原生二进制 + Caddy TLS 反代）
# 说明：本机 Docker Desktop(WSL2) 的端口发布只在本机 127.0.0.1 可用，局域网/手机访问不到，
# 因此使用 Windows 原生 livekit-server.exe 监听 0.0.0.0。
# 浏览器 HTTPS 页面需要 wss：Caddy 在 7443 终止 TLS 并反代 127.0.0.1:7880。
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$exe = Join-Path $PSScriptRoot "bin\livekit-server.exe"
$config = Join-Path $PSScriptRoot "livekit.yaml"
$log = Join-Path $PSScriptRoot "livekit-server.log"
$err = Join-Path $PSScriptRoot "livekit-server.err.log"
$pidFile = Join-Path $PSScriptRoot "livekit-server.pid"

if (-not (Test-Path $exe)) {
    throw "缺少 $exe，请从 https://github.com/livekit/livekit/releases 下载 livekit_<version>_windows_amd64.zip 解压到 bin\"
}

$existing = Get-Process livekit-server -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -eq $exe }
if ($existing) {
    Write-Host "LiveKit server 已在运行 (PID $($existing.Id -join ','))"
} else {
    $proc = Start-Process -FilePath $exe `
        -ArgumentList @("--config", $config) `
        -WorkingDirectory $PSScriptRoot `
        -RedirectStandardOutput $log `
        -RedirectStandardError $err `
        -WindowStyle Hidden -PassThru
    $proc.Id | Set-Content -Path $pidFile -Encoding ascii

    $ready = $false
    for ($i = 1; $i -le 15; $i++) {
        Start-Sleep -Seconds 1
        try {
            $resp = Invoke-WebRequest -Uri "http://127.0.0.1:7880/" -UseBasicParsing -TimeoutSec 3
            Write-Host "LiveKit 已就绪 (PID $($proc.Id), HTTP $($resp.StatusCode))"
            $ready = $true
            break
        } catch {}
    }
    if (-not $ready) {
        Write-Warning "LiveKit 未在预期时间内响应，请查看 $log / $err"
        exit 1
    }
}

# ── Caddy：wss 信令 TLS 反代（7443 → 7880）──
$caddy = Join-Path $PSScriptRoot "bin\caddy.exe"
$caddyfile = Join-Path $PSScriptRoot "Caddyfile"
$caddyLog = Join-Path $PSScriptRoot "caddy.log"
$caddyErr = Join-Path $PSScriptRoot "caddy.err.log"
$caddyPid = Join-Path $PSScriptRoot "caddy.pid"

if (Test-Path $caddy) {
    $existingCaddy = Get-Process caddy -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -eq $caddy }
    if (-not $existingCaddy) {
        $caddyProc = Start-Process -FilePath $caddy `
            -ArgumentList @("run", "--config", $caddyfile, "--adapter", "caddyfile") `
            -WorkingDirectory $PSScriptRoot `
            -RedirectStandardOutput $caddyLog `
            -RedirectStandardError $caddyErr `
            -WindowStyle Hidden -PassThru
        $caddyProc.Id | Set-Content -Path $caddyPid -Encoding ascii
        Start-Sleep -Seconds 2
        Write-Host "Caddy TLS 反代已启动 (PID $($caddyProc.Id), wss://192.168.0.101:7443 -> ws://127.0.0.1:7880)"
    } else {
        Write-Host "Caddy 已在运行 (PID $($existingCaddy.Id -join ','))"
    }
} else {
    Write-Warning "未找到 bin\caddy.exe：浏览器 HTTPS 页面将无法通过 wss 连接（可只用 localhost HTTP 调试）"
}

Write-Host "  信令: wss://192.168.0.101:7443  (本机 wss://127.0.0.1:7443)"
Write-Host "  媒体: tcp 7881 / udp 7882-7892"
