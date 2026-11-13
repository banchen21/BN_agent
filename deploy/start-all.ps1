# BN_agent 本地服务一键启动（幂等：端口已监听则跳过）
# 用法：powershell -NoProfile -ExecutionPolicy Bypass -File E:\BN_agent\deploy\start-all.ps1
$ErrorActionPreference = 'Continue'
$logDir = 'E:\BN_agent\logs'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null

function Test-Port([int]$port) {
    try {
        $c = New-Object Net.Sockets.TcpClient
        $ok = $c.ConnectAsync('127.0.0.1', $port).Wait(800)
        $c.Close()
        return $ok
    } catch { return $false }
}

# 1) LiveKit 7880 / 7881 / 7882-7892
if (-not (Test-Port 7880)) {
    Start-Process -FilePath 'E:\BN_agent\deploy\livekit\bin\livekit-server.exe' `
        -ArgumentList '--config', 'E:\BN_agent\deploy\livekit\livekit.yaml' `
        -WorkingDirectory 'E:\BN_agent\deploy\livekit' -WindowStyle Hidden `
        -RedirectStandardOutput "$logDir\livekit.out.log" -RedirectStandardError "$logDir\livekit.err.log"
    Start-Sleep -Seconds 2
    Write-Host 'livekit: started'
} else { Write-Host 'livekit: already listening' }

# 2) Caddy 7443 (wss 反代)
if (-not (Test-Port 7443)) {
    Start-Process -FilePath 'E:\BN_agent\deploy\livekit\bin\caddy.exe' `
        -ArgumentList 'run', '--config', 'E:\BN_agent\deploy\livekit\Caddyfile', '--adapter', 'caddyfile' `
        -WorkingDirectory 'E:\BN_agent\deploy\livekit' -WindowStyle Hidden `
        -RedirectStandardOutput "$logDir\caddy.out.log" -RedirectStandardError "$logDir\caddy.err.log"
    Write-Host 'caddy: started'
} else { Write-Host 'caddy: already listening' }

# 3) main-app 18080 / 18443
if (-not (Test-Port 18080)) {
    Start-Process -FilePath 'E:\BN_agent\target\debug\main-app.exe' `
        -WorkingDirectory 'E:\BN_agent' -WindowStyle Hidden `
        -RedirectStandardOutput "$logDir\main-app.out.log" -RedirectStandardError "$logDir\main-app.err.log"
    Write-Host 'main-app: started'
} else { Write-Host 'main-app: already listening' }

# 4) 通话由 main-app 的 call-livekit-plugin 直接接房（官方 Rust livekit SDK），
#    不再需要独立的 call-agent worker 进程。

# 5) yueyue 8877 / GSV-TTS 9880 / qwen 8190
if (-not (Test-Port 8877)) {
    Start-Process -FilePath 'D:\Python\Python313\python.exe' -ArgumentList 'server.py' `
        -WorkingDirectory 'E:\yueyue\call' -WindowStyle Hidden `
        -RedirectStandardOutput "$logDir\yueyue8877.out.log" -RedirectStandardError "$logDir\yueyue8877.err.log"
    Write-Host 'yueyue 8877: started'
} else { Write-Host 'yueyue 8877: already listening' }

if (-not (Test-Port 9880)) {
    Start-Process -FilePath 'D:\Python\Python313\python.exe' `
        -ArgumentList 'API\dsh_stream_api.py', '--models_dir', 'E:\GSV-TTS-Lite\models', '-p', '9880' `
        -WorkingDirectory 'E:\GSV-TTS-Lite' -WindowStyle Hidden `
        -RedirectStandardOutput "$logDir\tts9880.out.log" -RedirectStandardError "$logDir\tts9880.err.log"
    Write-Host 'tts 9880: started'
} else { Write-Host 'tts 9880: already listening' }

if (-not (Test-Port 8190)) {
    Start-Process -FilePath 'D:\Python\Python313\python.exe' `
        -ArgumentList 'qwen_api.py', '--port', '8190' `
        -WorkingDirectory 'E:\yueyue\tools' -WindowStyle Hidden `
        -RedirectStandardOutput "$logDir\qwen8190.out.log" -RedirectStandardError "$logDir\qwen8190.err.log"
    Write-Host 'qwen 8190: started'
} else { Write-Host 'qwen 8190: already listening' }

# 6) ComfyUI watchdog（脚本自身负责拉起 8188）
$wd = Get-CimInstance Win32_Process -ErrorAction SilentlyContinue | Where-Object {
    $_.Name -eq 'powershell.exe' -and $_.ProcessId -ne $PID -and $_.CommandLine -and $_.CommandLine -match 'comfyui-watchdog\.ps1'
}
if (-not $wd) {
    Start-Process -FilePath 'powershell.exe' `
        -ArgumentList '-NoProfile', '-ExecutionPolicy', 'Bypass', '-WindowStyle', 'Hidden', '-File', 'E:\yueyue\tools\comfyui-watchdog.ps1' `
        -WindowStyle Hidden
    Write-Host 'comfyui-watchdog: started'
} else { Write-Host 'comfyui-watchdog: already running' }

Write-Host 'start-all done'
