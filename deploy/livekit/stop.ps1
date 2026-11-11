# 停止 BN_agent 独立 LiveKit server 与 Caddy TLS 反代
$ErrorActionPreference = "Continue"
Set-Location $PSScriptRoot

function Stop-ByPidFile($pidFile, $exe, $name) {
    $stopped = $false
    if (Test-Path $pidFile) {
        $procId = (Get-Content $pidFile -ErrorAction SilentlyContinue | Select-Object -First 1)
        if ($procId -and ($procId -as [int])) {
            $proc = Get-Process -Id ([int]$procId) -ErrorAction SilentlyContinue
            if ($proc -and $proc.Path -eq $exe) {
                Stop-Process -Id $proc.Id -Force
                $stopped = $true
                Write-Host "已停止 $name (PID $($proc.Id))"
            }
        }
        Remove-Item $pidFile -ErrorAction SilentlyContinue
    }
    if (-not $stopped) {
        Get-Process -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -eq $exe } |
            ForEach-Object {
                Stop-Process -Id $_.Id -Force
                Write-Host "已停止 $name (PID $($_.Id))"
                $stopped = $true
            }
    }
    if (-not $stopped) { Write-Host "$name 未在运行" }
}

Stop-ByPidFile (Join-Path $PSScriptRoot "caddy.pid") (Join-Path $PSScriptRoot "bin\caddy.exe") "Caddy"
Stop-ByPidFile (Join-Path $PSScriptRoot "livekit-server.pid") (Join-Path $PSScriptRoot "bin\livekit-server.exe") "LiveKit server"

# 兼容旧 Docker 部署：如仍存在则一并清理
$dockerBin = "C:\Program Files\Docker\Docker\resources\bin"
if (Test-Path $dockerBin) { $env:PATH = "$dockerBin;$env:PATH" }
$compose = Join-Path $PSScriptRoot "docker-compose.yml"
if ((Test-Path $compose) -and (Get-Command docker -ErrorAction SilentlyContinue)) {
    $running = docker ps --filter "name=bn-livekit" --format "{{.Names}}" 2>$null
    if ($running) {
        docker compose down | Out-Null
        Write-Host "已停止旧的 Docker LiveKit 容器"
    }
}
