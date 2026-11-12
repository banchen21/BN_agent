"""ai-call 服务与 LiveKit Agent 的配置（归档副本，密钥已脱敏）。

所有值都可以用环境变量覆盖；默认值对应本机局域网自用部署。
"""
from __future__ import annotations

import os
from dataclasses import dataclass

# LiveKit Agent worker 的 aiohttp 会把 ws://127.0.0.1:7880 的握手错误地交给系统 HTTP 代理
# （表现为 400 Invalid response status）。本服务只访问本机/局域网，进程内直接清掉代理变量。
for _proxy_key in ("HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY", "http_proxy", "https_proxy", "all_proxy"):
    os.environ.pop(_proxy_key, None)
os.environ.setdefault("NO_PROXY", "localhost,127.0.0.1,::1")


def _env(name: str, default: str) -> str:
    value = os.environ.get(name)
    return value if value not in (None, "") else default


def _env_int(name: str, default: int) -> int:
    try:
        return int(_env(name, str(default)))
    except ValueError:
        return default


def _env_bool(name: str, default: bool) -> bool:
    return _env(name, "1" if default else "0").strip().lower() in ("1", "true", "yes", "on")


@dataclass(frozen=True)
class Settings:
    # ---- FastAPI :9001
    host: str
    port: int

    # ---- LiveKit
    # 服务端 API 用本机地址（宿主 hairpin 限制只影响 192.168.0.101 回环）
    livekit_url: str
    # 发给客户端的地址：手机走局域网 IP，Windows 宿主走 127.0.0.1
    livekit_public_url: str
    livekit_local_url: str
    livekit_key: str
    livekit_secret: str
    agent_name: str
    room_prefix: str
    token_ttl_seconds: int
    room_empty_timeout: int
    room_departure_timeout: int

    # ---- AI 能力（E:\yueyue\call）
    yueyue_api: str

    # ---- ai-bot GPU 协调（可选）
    ai_bot_api: str
    gpu_coordination: bool

    # ---- Agent 行为
    greeting: str
    worker_http_port: int


def load_settings() -> Settings:
    return Settings(
        host=_env("AI_CALL_HOST", "0.0.0.0"),
        port=_env_int("AI_CALL_PORT", 9001),
        livekit_url=_env("LIVEKIT_URL", "ws://127.0.0.1:7880"),
        livekit_public_url=_env("LIVEKIT_PUBLIC_URL", "ws://192.168.0.101:7880"),
        livekit_local_url=_env("LIVEKIT_LOCAL_URL", "ws://127.0.0.1:7880"),
        # 归档时已脱敏：原值通过 LIVEKIT_KEY / LIVEKIT_SECRET 环境变量注入
        livekit_key=_env("LIVEKIT_KEY", "CHANGE_ME"),
        livekit_secret=_env("LIVEKIT_SECRET", "CHANGE_ME"),
        agent_name=_env("AI_AGENT_NAME", "yueyue"),
        room_prefix=_env("AI_CALL_ROOM_PREFIX", "ai-call"),
        token_ttl_seconds=_env_int("AI_CALL_TOKEN_TTL", 6 * 3600),
        room_empty_timeout=_env_int("AI_CALL_ROOM_EMPTY_TIMEOUT", 120),
        room_departure_timeout=_env_int("AI_CALL_ROOM_DEPARTURE_TIMEOUT", 25),
        yueyue_api=_env("YUEYUE_API", "http://127.0.0.1:8877").rstrip("/"),
        ai_bot_api=_env("AI_BOT_API", "http://127.0.0.1:9000").rstrip("/"),
        gpu_coordination=_env_bool("AI_CALL_GPU_COORDINATION", True),
        greeting=_env("AI_CALL_GREETING", "喂？"),
        worker_http_port=_env_int("AI_WORKER_HTTP_PORT", 8081),
    )


SETTINGS = load_settings()
