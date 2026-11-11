"""yueyue 8877 异步客户端：ASR / 流式对话 / 流式 TTS / 打断。

接口约定（实测）：
- POST /api/asr          multipart file=WAV -> {"text", "emotion", "summary", ...}
- POST /api/chat         SSE: event: token / data: {"t": "..."}，event: done/skip 结束
- POST /api/tts_stream   SSE: event: audio / data: {"pcm": base64(int16), "sr": 32000}
- POST /api/tts          非流式，返回 WAV（测试脚本用）
- POST /api/interrupt    中止当前正在生成的回复
"""
from __future__ import annotations

import base64
import io
import json
import logging
import wave
from dataclasses import dataclass, field
from typing import AsyncIterator

import httpx
from livekit import rtc

logger = logging.getLogger("call.yueyue")


@dataclass
class AsrResult:
    text: str
    emotion: str = ""
    tone: dict = field(default_factory=dict)


def frame_to_wav(buffer: "rtc.AudioFrame | list[rtc.AudioFrame]") -> bytes:
    """把 LiveKit 的 AudioBuffer 转成 WAV（8877 的 /api/asr 用 soundfile 读取）。"""
    frames = buffer if isinstance(buffer, list) else [buffer]
    frames = [f for f in frames if f is not None]
    if not frames:
        return b""
    frame = frames[0] if len(frames) == 1 else rtc.combine_audio_frames(frames)
    buf = io.BytesIO()
    with wave.open(buf, "wb") as w:
        w.setnchannels(frame.num_channels)
        w.setsampwidth(2)
        w.setframerate(frame.sample_rate)
        w.writeframes(bytes(frame.data))
    return buf.getvalue()


async def iter_sse(response: httpx.Response) -> AsyncIterator[tuple[str, str]]:
    """最小 SSE 解析：返回 (event, data)。"""
    event: str | None = None
    data: list[str] = []
    async for line in response.aiter_lines():
        if line == "":
            if event is not None or data:
                yield event or "message", "\n".join(data)
            event, data = None, []
            continue
        if line.startswith(":"):
            continue
        if line.startswith("event:"):
            event = line[6:].strip()
        elif line.startswith("data:"):
            data.append(line[5:].lstrip())
        else:
            # 容错：非标准行并入 data
            if line.strip():
                data.append(line.strip())
    if event is not None or data:
        yield event or "message", "\n".join(data)


class YueyueClient:
    def __init__(self, base_url: str, *, timeout: float = 60.0) -> None:
        self.base_url = base_url.rstrip("/")
        self._client = httpx.AsyncClient(
            timeout=httpx.Timeout(timeout, connect=5.0),
            limits=httpx.Limits(max_connections=8, max_keepalive_connections=4),
            trust_env=False,
        )

    async def aclose(self) -> None:
        await self._client.aclose()

    # ---------------------------------------------------------------- ASR
    async def asr(self, wav: bytes) -> AsrResult:
        resp = await self._client.post(
            f"{self.base_url}/api/asr",
            files={"file": ("speech.wav", wav, "audio/wav")},
        )
        resp.raise_for_status()
        data = resp.json()
        tone = {
            key: data[key]
            for key in ("volume", "pitch", "speech_rate", "summary")
            if key in data
        }
        return AsrResult(
            text=str(data.get("text") or "").strip(),
            emotion=str(data.get("emotion") or ""),
            tone=tone,
        )

    # ---------------------------------------------------------------- LLM
    async def chat(
        self, text: str, *, emotion: str = "", tone: dict | None = None
    ) -> AsyncIterator[tuple[str, str]]:
        """产出 (kind, payload)：token / skip / done / error。"""
        payload: dict = {"text": text}
        if emotion:
            payload["emotion"] = emotion
        if tone:
            payload["tone"] = tone
        async with self._client.stream(
            "POST", f"{self.base_url}/api/chat", json=payload
        ) as resp:
            resp.raise_for_status()
            async for event, data in iter_sse(resp):
                if event == "token":
                    try:
                        token = json.loads(data).get("t", "")
                    except json.JSONDecodeError:
                        token = ""
                    if token:
                        yield "token", token
                elif event in ("skip", "done", "error"):
                    yield event, data
                    return

    async def interrupt(self) -> None:
        try:
            await self._client.post(
                f"{self.base_url}/api/interrupt", timeout=httpx.Timeout(2.0)
            )
        except Exception:  # noqa: BLE001 - 打断是 best-effort
            pass

    # ---------------------------------------------------------------- TTS
    async def tts_stream(self, text: str) -> AsyncIterator[tuple[bytes, int]]:
        """逐块产出 (int16 PCM bytes, sample_rate)。"""
        async with self._client.stream(
            "POST", f"{self.base_url}/api/tts_stream", json={"text": text}
        ) as resp:
            resp.raise_for_status()
            async for event, data in iter_sse(resp):
                if event == "audio":
                    try:
                        payload = json.loads(data)
                        pcm = base64.b64decode(payload.get("pcm") or "")
                    except (json.JSONDecodeError, ValueError) as exc:
                        logger.warning("tts_stream 无法解析音频块: %s", exc)
                        continue
                    if pcm:
                        yield pcm, int(payload.get("sr", 32000))
                elif event in ("done", "error"):
                    return

    async def tts_wav(self, text: str) -> bytes:
        """非流式合成，返回 WAV 字节（测试脚本 / 预生成提示音用）。"""
        resp = await self._client.post(
            f"{self.base_url}/api/tts", json={"text": text}, timeout=httpx.Timeout(180.0)
        )
        resp.raise_for_status()
        return resp.content

    # ---------------------------------------------------------------- health
    async def health(self) -> dict:
        try:
            resp = await self._client.get(
                f"{self.base_url}/api/health", timeout=httpx.Timeout(3.0)
            )
            resp.raise_for_status()
            data = resp.json()
            return data if isinstance(data, dict) else {}
        except Exception:  # noqa: BLE001
            return {}

    async def healthy(self) -> bool:
        data = await self.health()
        return bool(data.get("ok"))
