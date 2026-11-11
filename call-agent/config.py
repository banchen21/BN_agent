"""call-agent 配置：config.json + 环境变量覆盖。

优先级：环境变量 > 本目录 config.json > LiveKit 插件配置（仅 key/secret/url）。
所有密钥都只在运行时读取，不写进本文件、不打印日志。
"""
from __future__ import annotations

import json
import logging
import os
from dataclasses import dataclass, field
from pathlib import Path

logger = logging.getLogger("call.config")

BASE_DIR = Path(__file__).resolve().parent
DEFAULT_CONFIG_PATH = BASE_DIR / "config.json"

# LiveKit Agent worker 的 aiohttp 会把 ws://127.0.0.1:7880 的握手错误地交给系统 HTTP 代理，
# 表现为 400 Invalid response status。本服务只访问本机/局域网，进程内直接清掉代理变量。
for _proxy_key in (
    "HTTP_PROXY",
    "HTTPS_PROXY",
    "ALL_PROXY",
    "http_proxy",
    "https_proxy",
    "all_proxy",
):
    os.environ.pop(_proxy_key, None)
os.environ.setdefault("NO_PROXY", "localhost,127.0.0.1,::1")


def _env(name: str, default: str = "") -> str:
    value = os.environ.get(name)
    return value if value not in (None, "") else default


def _env_int(name: str, default: int) -> int:
    try:
        return int(_env(name, str(default)))
    except ValueError:
        return default


def _env_bool(name: str, default: bool) -> bool:
    return _env(name, "1" if default else "0").strip().lower() in ("1", "true", "yes", "on")


def _load_json(path: Path | str | None) -> dict:
    if not path:
        return {}
    p = Path(path).expanduser()
    try:
        data = json.loads(p.read_text(encoding="utf-8"))
        return data if isinstance(data, dict) else {}
    except FileNotFoundError:
        return {}
    except Exception as exc:  # noqa: BLE001 - 坏配置按空处理并告警
        logger.warning("配置文件读取失败 %s: %s", p, exc)
        return {}


def _section(raw: dict, key: str) -> dict:
    value = raw.get(key)
    return value if isinstance(value, dict) else {}


@dataclass(frozen=True)
class TurnSettings:
    framework_interrupt_min_duration: float
    endpointing_min_delay: float
    endpointing_max_delay: float
    vad_min_silence: float
    vad_prefix_padding: float


@dataclass(frozen=True)
class BargeInSettings:
    duck_gain: float
    restore_after: float
    max_duck: float


@dataclass(frozen=True)
class VideoSettings:
    max_fps: float
    max_width: int
    jpeg_quality: int


@dataclass(frozen=True)
class VisionSettings:
    provider: str  # magpie | bn_agent | openai
    model: str
    base_url: str
    api_key: str
    api_key_env: str
    api_key_file: str
    claude_settings_path: str
    auth_header: str
    auth_prefix: str
    max_tokens: int
    timeout: float
    auto_look: bool
    auto_look_keywords: tuple[str, ...]
    bn_agent_main_app_url: str
    bn_agent_api_key_file: str
    bn_agent_tool_name: str


@dataclass(frozen=True)
class Settings:
    agent_name: str
    livekit_url: str
    livekit_key: str
    livekit_secret: str
    yueyue_url: str
    data_dir: str
    greeting: str
    worker_http_port: int
    log_level: str
    turn: TurnSettings
    bargein: BargeInSettings
    video: VideoSettings
    vision: VisionSettings
    raw: dict = field(default_factory=dict, repr=False)

    @property
    def persona_path(self) -> Path:
        return Path(self.data_dir) / "persona.md"

    @property
    def memory_dir(self) -> Path:
        return Path(self.data_dir) / "memory"


def load_settings() -> Settings:
    raw = _load_json(Path(_env("CALL_AGENT_CONFIG", str(DEFAULT_CONFIG_PATH))))
    plugin = _load_json(raw.get("livekit_plugin_config"))

    livekit_url = _env("LIVEKIT_URL", str(raw.get("livekit_url") or plugin.get("api_url") or "ws://127.0.0.1:7880"))
    livekit_key = _env("LIVEKIT_KEY", str(raw.get("livekit_key") or plugin.get("api_key") or ""))
    livekit_secret = _env("LIVEKIT_SECRET", str(raw.get("livekit_secret") or plugin.get("api_secret") or ""))

    turn = _section(raw, "turn")
    bargein = _section(raw, "bargein")
    video = _section(raw, "video")
    vision = _section(raw, "vision")
    bn_agent = vision.get("bn_agent") if isinstance(vision.get("bn_agent"), dict) else {}

    keywords = vision.get("auto_look_keywords")
    if not isinstance(keywords, list):
        keywords = []

    return Settings(
        agent_name=_env("AGENT_NAME", str(raw.get("agent_name") or "yueyue")),
        livekit_url=livekit_url,
        livekit_key=livekit_key,
        livekit_secret=livekit_secret,
        yueyue_url=_env("YUEYUE_URL", _env("YUEYUE_API", str(raw.get("yueyue_url") or "http://127.0.0.1:8877"))).rstrip("/"),
        data_dir=_env("DATA_DIR", str(raw.get("data_dir") or "E:/BN_agent/data/yueyue")),
        greeting=_env("GREETING", str(raw.get("greeting") if raw.get("greeting") is not None else "喂？")),
        worker_http_port=_env_int("WORKER_HTTP_PORT", int(raw.get("worker_http_port") or 8081)),
        log_level=_env("LOG_LEVEL", str(raw.get("log_level") or "INFO")).upper(),
        turn=TurnSettings(
            framework_interrupt_min_duration=float(
                turn.get("framework_interrupt_min_duration", 3.0)
            ),
            endpointing_min_delay=float(turn.get("endpointing_min_delay", 0.3)),
            endpointing_max_delay=float(turn.get("endpointing_max_delay", 3.0)),
            vad_min_silence=float(turn.get("vad_min_silence", 0.45)),
            vad_prefix_padding=float(turn.get("vad_prefix_padding", 0.4)),
        ),
        bargein=BargeInSettings(
            duck_gain=float(bargein.get("duck_gain", 0.45)),
            restore_after=float(bargein.get("restore_after", 0.6)),
            max_duck=float(bargein.get("max_duck", 6.0)),
        ),
        video=VideoSettings(
            max_fps=float(video.get("max_fps", 1.0)),
            max_width=int(video.get("max_width", 960)),
            jpeg_quality=int(video.get("jpeg_quality", 80)),
        ),
        vision=VisionSettings(
            provider=_env("VISION_PROVIDER", str(vision.get("provider") or "magpie")).strip().lower(),
            model=_env("VISION_MODEL", str(vision.get("model") or "group/ds-v4.1")),
            base_url=_env("VISION_BASE_URL", str(vision.get("base_url") or "http://127.0.0.1:3425/v1")).rstrip("/"),
            api_key=_env("VISION_API_KEY", str(vision.get("api_key") or "")),
            api_key_env=str(vision.get("api_key_env") or "ANTHROPIC_AUTH_TOKEN"),
            api_key_file=_env("VISION_API_KEY_FILE", str(vision.get("api_key_file") or "")),
            claude_settings_path=str(vision.get("claude_settings_path") or "~/.claude/settings.json"),
            auth_header=str(vision.get("auth_header") or "Authorization"),
            auth_prefix=str(vision.get("auth_prefix") if vision.get("auth_prefix") is not None else "Bearer "),
            max_tokens=int(vision.get("max_tokens", 400)),
            timeout=float(vision.get("timeout", 45.0)),
            auto_look=_env_bool("VISION_AUTO_LOOK", bool(vision.get("auto_look", True))),
            auto_look_keywords=tuple(str(k) for k in keywords),
            bn_agent_main_app_url=_env(
                "MAIN_APP_URL", str(bn_agent.get("main_app_url") or "http://127.0.0.1:18080")
            ).rstrip("/"),
            bn_agent_api_key_file=str(
                bn_agent.get("api_key_file") or "E:/BN_agent/.secrets/api_key.txt"
            ),
            bn_agent_tool_name=str(bn_agent.get("tool_name") or "image_understand"),
        ),
        raw=raw,
    )


SETTINGS = load_settings()
