"""ai-bot 配置（归档副本，密钥已脱敏）。

所有值都可以用环境变量覆盖；本地开发直接用默认值即可。
"""
from __future__ import annotations

import os
from dataclasses import dataclass


def _env(name: str, default: str) -> str:
    value = os.environ.get(name)
    return value if value not in (None, "") else default


@dataclass(frozen=True)
class Settings:
    # HTTP 服务
    host: str
    port: int

    # OpenIM
    openim_api: str
    openim_secret: str
    openim_admin_user: str
    bot_user_id: str
    bot_nickname: str
    bot_password: str

    # yueyue 8877 / ComfyUI 8190
    yueyue_api: str
    image_api: str

    # MinIO
    minio_endpoint: str
    minio_access_key: str
    minio_secret_key: str
    minio_bucket: str
    minio_public_base: str
    minio_internal_base: str
    bot_friends: tuple[str, ...]

    # 行为
    image_timeout_seconds: int
    chat_timeout_seconds: int
    tts_timeout_seconds: int
    image_wait_call_seconds: int


def load_settings() -> Settings:
    return Settings(
        host=_env("AI_BOT_HOST", "0.0.0.0"),
        port=int(_env("AI_BOT_PORT", "9000")),
        openim_api=_env("OPENIM_API_BASE", "http://127.0.0.1:10002").rstrip("/"),
        # 归档时已脱敏：原值通过 OPENIM_SECRET 环境变量注入
        openim_secret=_env("OPENIM_SECRET", "CHANGE_ME"),
        openim_admin_user=_env("OPENIM_ADMIN_USER", "imAdmin"),
        bot_user_id=_env("BOT_USER_ID", "ai_yueyue"),
        bot_nickname=_env("BOT_NICKNAME", "悦悦"),
        # OpenIM 注册接口里 password 存 md5(明文)，App 客户端登录时同样发 md5。
        # 归档时已脱敏：原 md5 通过 BOT_PASSWORD_MD5 环境变量注入
        bot_password=_env("BOT_PASSWORD_MD5", "CHANGE_ME"),
        yueyue_api=_env("YUEYUE_API_BASE", "http://127.0.0.1:8877").rstrip("/"),
        image_api=_env("QWEN_IMAGE_API", "http://127.0.0.1:8190").rstrip("/"),
        minio_endpoint=_env("MINIO_ENDPOINT", "127.0.0.1:10005"),
        minio_access_key=_env("MINIO_ROOT_USER", "root"),
        # 归档时已脱敏：原值通过 MINIO_ROOT_PASSWORD 环境变量注入
        minio_secret_key=_env(
            "MINIO_ROOT_PASSWORD", "CHANGE_ME"
        ),
        minio_bucket=_env("MINIO_BUCKET", "openim"),
        minio_public_base=_env("MINIO_PUBLIC_BASE", "http://192.168.0.101:10005").rstrip("/"),
        minio_internal_base=_env(
            "MINIO_INTERNAL_BASE", "http://127.0.0.1:10005"
        ).rstrip("/"),
        bot_friends=tuple(
            item.strip()
            for item in _env("BOT_FRIENDS", "3370971303,4460069020").split(",")
            if item.strip()
        ),
        image_timeout_seconds=int(_env("IMAGE_TIMEOUT_SECONDS", "1200")),
        chat_timeout_seconds=int(_env("CHAT_TIMEOUT_SECONDS", "180")),
        tts_timeout_seconds=int(_env("TTS_TIMEOUT_SECONDS", "180")),
        image_wait_call_seconds=int(_env("IMAGE_WAIT_CALL_SECONDS", "60")),
    )


settings = load_settings()
