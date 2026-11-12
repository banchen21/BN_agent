"""媒体上传：优先直传 MinIO（bucket 公开读），失败时回落到 OpenIM 对象通道。

对外 URL 形如 http://192.168.0.101:10005/openim/<object>，手机可以直接加载。
"""
from __future__ import annotations

import io
import json
import logging
import uuid
import wave
from typing import Any
from urllib.parse import quote

import requests
from minio import Minio
from PIL import Image

from config import Settings

log = logging.getLogger("ai-bot.media")

PUBLIC_READ_POLICY = {
    "Version": "2012-10-17",
    "Statement": [
        {
            "Effect": "Allow",
            "Principal": {"AWS": ["*"]},
            "Action": ["s3:GetObject"],
            "Resource": ["arn:aws:s3:::{bucket}/*"],
        }
    ],
}


def wav_duration_seconds(data: bytes) -> float:
    """WAV 时长；解析失败时按 16k/16bit 单声道估算。"""
    try:
        with wave.open(io.BytesIO(data), "rb") as wf:
            rate = wf.getframerate() or 32000
            frames = wf.getnframes() or 0
            if rate:
                return frames / float(rate)
    except Exception:  # noqa: BLE001 - 非 WAV 时退回估算
        pass
    return len(data) / float(32000 * 2) if data else 0.0


def image_dimensions(data: bytes) -> tuple[int, int]:
    with Image.open(io.BytesIO(data)) as im:
        return int(im.width), int(im.height)


class MediaStore:
    def __init__(self, settings: Settings, openim: Any | None = None) -> None:
        self.settings = settings
        self.openim = openim
        self._client = Minio(
            settings.minio_endpoint,
            access_key=settings.minio_access_key,
            secret_key=settings.minio_secret_key,
            secure=False,
        )
        self._bucket_ready = False
        self._http = requests.Session()
        self._http.trust_env = False

    # ---------------------------------------------------------------- MinIO 直传
    def ensure_bucket(self) -> None:
        if self._bucket_ready:
            return
        bucket = self.settings.minio_bucket
        if not self._client.bucket_exists(bucket):
            self._client.make_bucket(bucket)
            log.info("MinIO 已创建 bucket %s", bucket)
        policy = json.loads(json.dumps(PUBLIC_READ_POLICY).replace("{bucket}", bucket))
        try:
            self._client.set_bucket_policy(bucket, json.dumps(policy))
        except Exception as exc:  # noqa: BLE001 - 没有策略权限时只告警
            log.warning("MinIO 设置公开读策略失败（%s）：%s", bucket, exc)
        self._bucket_ready = True

    def _upload_minio(self, object_name: str, data: bytes, content_type: str) -> str:
        self.ensure_bucket()
        self._client.put_object(
            self.settings.minio_bucket,
            object_name,
            io.BytesIO(data),
            length=len(data),
            content_type=content_type,
        )
        return (
            f"{self.settings.minio_public_base}/{self.settings.minio_bucket}/"
            f"{quote(object_name)}"
        )

    # ---------------------------------------------------------------- OpenIM 对象通道（回落）
    def _upload_openim(self, object_name: str, data: bytes, content_type: str) -> str:
        if self.openim is None:
            raise RuntimeError("没有 OpenIM 客户端，无法走对象通道")
        bot = self.settings.bot_user_id
        init = self.openim.initiate_form_data(
            bot, f"{bot}/{object_name}", len(data), content_type
        ) or {}
        url = init.get("url", "")
        form = init.get("formData") or {}
        field = init.get("file") or "file"
        form_id = init.get("id", "")
        if not url or not form_id:
            raise RuntimeError(f"initiate_form_data 返回不完整：{init}")

        filename = object_name.rsplit("/", 1)[-1]
        # formData 里的 Content-Type 是 S3 POST 策略的一部分，必须原样提交；
        # 文件 part 的 Content-Type 不参与校验，这里不覆盖。
        files = {field: (filename, data)}
        resp = self._http.post(url, data=form, files=files, timeout=180)
        if resp.status_code not in (200, 201, 204):
            raise RuntimeError(f"上传对象失败 HTTP {resp.status_code}: {resp.text[:200]}")

        done = self.openim.complete_form_data(form_id) or {}
        final_url = done.get("url", "")
        if not final_url:
            raise RuntimeError(f"complete_form_data 没有返回 url：{done}")
        return final_url

    # ---------------------------------------------------------------- 对外入口
    def upload(self, object_name: str, data: bytes, content_type: str) -> str:
        object_name = object_name.lstrip("/")
        if not object_name.startswith("ai-bot/"):
            object_name = f"ai-bot/{object_name}"
        try:
            return self._upload_minio(object_name, data, content_type)
        except Exception as exc:  # noqa: BLE001 - MinIO 失败时回落 OpenIM
            log.warning("MinIO 直传失败（%s），改用 OpenIM 对象通道", exc)
            return self._upload_openim(object_name, data, content_type)

    def upload_image(self, data: bytes, ext: str = "png") -> tuple[str, int, int, str]:
        """返回 (url, size, width, height)。"""
        name = f"{uuid.uuid4().hex}.{ext}"
        width, height = image_dimensions(data)
        url = self.upload(name, data, f"image/{ext}")
        return url, len(data), width, height

    def upload_audio(self, data: bytes, ext: str = "wav") -> tuple[str, int, float]:
        """返回 (url, size, duration_seconds)。"""
        name = f"{uuid.uuid4().hex}.{ext}"
        duration = wav_duration_seconds(data)
        url = self.upload(name, data, f"audio/{ext}")
        return url, len(data), duration
