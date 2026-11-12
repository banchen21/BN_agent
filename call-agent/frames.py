"""用户摄像头最新帧缓存（作为 AgentSession 的 video_sampler）。

标准路径：LiveKit WebRTC 视频轨道 -> RoomIO(video_input=True) -> AgentSession 的视频
转发任务 -> 本采样器。采样器对每帧只做时间门控，按 max_fps 保存最新一帧（RGBA），
需要截图时再编码 JPEG；返回 False 表示不把视频帧塞进 LLM 上下文（由视觉工具按需取帧）。

截图是混合兜底：视觉工具/自动看图只在需要时把“最新帧”编码成 JPEG 发给视觉模型，
不会持续把视频帧塞进对话上下文。
"""
from __future__ import annotations

import io
import logging
import threading
import time
from dataclasses import dataclass

from livekit import rtc
from PIL import Image

logger = logging.getLogger("call.frames")


@dataclass
class FrameSnapshot:
    jpeg: bytes
    width: int
    height: int
    captured_at: float  # unix 秒
    age: float  # 距离现在的秒数


class FrameStore:
    """AgentSession(video_sampler=...) 需要可调用对象：__call__(frame, session) -> bool。"""

    def __init__(self, *, max_fps: float = 1.0, max_width: int = 960, jpeg_quality: int = 80) -> None:
        self._min_interval = 1.0 / max(0.1, float(max_fps))
        self._max_width = max(160, int(max_width))
        self._jpeg_quality = max(40, min(95, int(jpeg_quality)))
        self._lock = threading.Lock()
        self._raw: tuple[bytes, int, int, float] | None = None
        self._last_store_mono = 0.0
        self._frames_seen = 0
        self._frames_stored = 0
        self._last_error = ""
        self._logged_first = False

    # ---------------------------------------------------------------- 采样
    def __call__(self, frame: rtc.VideoFrame, session=None) -> bool:
        """返回值始终 False：帧只保存在本缓存里，不进入 LLM 上下文。"""
        self._frames_seen += 1
        now = time.monotonic()
        if now - self._last_store_mono < self._min_interval:
            return False
        self._last_store_mono = now
        try:
            rgba = (
                frame
                if frame.type == rtc.VideoBufferType.RGBA
                else frame.convert(rtc.VideoBufferType.RGBA)
            )
            data = bytes(rgba.data)
            with self._lock:
                self._raw = (data, rgba.width, rgba.height, time.time())
                self._frames_stored += 1
            if not self._logged_first:
                self._logged_first = True
                logger.info("video: 收到用户首帧 %dx%d", rgba.width, rgba.height)
        except Exception as exc:  # noqa: BLE001 - 单帧失败不退出
            self._last_error = str(exc)
            logger.debug("video: 帧转换失败: %s", exc)
        return False

    # ---------------------------------------------------------------- 读取
    @property
    def has_frame(self) -> bool:
        with self._lock:
            return self._raw is not None

    @property
    def stats(self) -> dict:
        with self._lock:
            raw = self._raw
            return {
                "frames_seen": self._frames_seen,
                "frames_stored": self._frames_stored,
                "last_frame_age": round(time.time() - raw[3], 2) if raw else None,
                "size": f"{raw[1]}x{raw[2]}" if raw else None,
                "last_error": self._last_error,
            }

    def latest_jpeg(self) -> FrameSnapshot | None:
        with self._lock:
            raw = self._raw
        if raw is None:
            return None
        data, width, height, at = raw
        try:
            image = Image.frombytes("RGBA", (width, height), data).convert("RGB")
            if image.width > self._max_width:
                ratio = self._max_width / image.width
                image = image.resize((self._max_width, max(1, int(image.height * ratio))))
            buf = io.BytesIO()
            image.save(buf, format="JPEG", quality=self._jpeg_quality)
            return FrameSnapshot(
                jpeg=buf.getvalue(),
                width=image.width,
                height=image.height,
                captured_at=at,
                age=max(0.0, time.time() - at),
            )
        except Exception as exc:  # noqa: BLE001
            logger.warning("帧编码 JPEG 失败: %s", exc)
            return None

    async def aclose(self) -> None:
        logger.info("video: 停止采样 stats=%s", self.stats)
