"""LiveKit Agents 自定义插件：STT / LLM / TTS 全部包装本地 8877。

- STT：非流式 recognize（VAD 切句由 livekit.agents.stt.StreamAdapter 完成）
- LLM：/api/chat 的 SSE token 流
- TTS：/api/tts_stream 的 SSE int16 PCM，首包即播

打断：
- LLM 流被取消 -> POST /api/interrupt，停掉本地 Qwen 生成
- TTS 流被取消 -> 关闭 HTTP 流，8877 侧生成随之结束
- AgentSession 在 VAD 检测到用户开口时自动取消当前 speech，播放队列一并清空
"""
from __future__ import annotations

import asyncio
import array
import logging
import time
import uuid
from dataclasses import dataclass, field
from typing import AsyncIterator

from livekit.agents import (
    DEFAULT_API_CONNECT_OPTIONS,
    NOT_GIVEN,
    APIConnectOptions,
    llm,
    stt,
    tts,
    utils,
)
from livekit.agents.types import NotGivenOr

from bargein import BargeInState
from yueyue import YueyueClient, frame_to_wav

logger = logging.getLogger("ai-call.plugins")


def _apply_gain(pcm: bytes, gain: float) -> bytes:
    """对 int16 PCM 做线性增益（duck 用）。"""
    if gain >= 0.999:
        return pcm
    samples = array.array("h")
    samples.frombytes(pcm)
    for i in range(len(samples)):
        samples[i] = int(samples[i] * gain)
    return samples.tobytes()


@dataclass
class CallMeta:
    """同一通电话内共享的语音感知信息（STT 写入，LLM 读取）。"""

    emotion: str = ""
    tone: dict = field(default_factory=dict)

    def update(self, *, emotion: str, tone: dict) -> None:
        if emotion:
            self.emotion = emotion
        if tone:
            self.tone = tone


# ---------------------------------------------------------------- STT
class YueyueSTT(stt.STT):
    def __init__(self, client: YueyueClient, meta: CallMeta) -> None:
        super().__init__(
            capabilities=stt.STTCapabilities(
                streaming=False,
                interim_results=False,
                offline_recognize=True,
            )
        )
        self._client = client
        self.meta = meta

    @property
    def model(self) -> str:
        return "sensevoice-onnx"

    @property
    def provider(self) -> str:
        return "yueyue-8877"

    async def _recognize_impl(
        self,
        buffer: "utils.AudioBuffer",
        *,
        language: NotGivenOr[str] = NOT_GIVEN,
        conn_options: APIConnectOptions = DEFAULT_API_CONNECT_OPTIONS,
    ) -> stt.SpeechEvent:
        request_id = uuid.uuid4().hex

        def _empty() -> stt.SpeechEvent:
            return stt.SpeechEvent(
                type=stt.SpeechEventType.FINAL_TRANSCRIPT,
                request_id=request_id,
                alternatives=[],
            )

        wav = frame_to_wav(buffer)
        if not wav:
            return _empty()

        try:
            result = await self._client.asr(wav)
        except Exception as exc:
            # 8877 不可用时不要让整通电话崩掉：这一句当没听见
            logger.warning("ASR 失败（忽略本句）: %s", exc)
            return _empty()

        self.meta.update(emotion=result.emotion, tone=result.tone)
        if not result.text:
            return _empty()

        logger.info("ASR: %s (emotion=%s)", result.text, result.emotion or "-")
        return stt.SpeechEvent(
            type=stt.SpeechEventType.FINAL_TRANSCRIPT,
            request_id=request_id,
            alternatives=[stt.SpeechData(language="zh", text=result.text)],
        )


# ---------------------------------------------------------------- LLM
class YueyueLLM(llm.LLM):
    def __init__(self, client: YueyueClient, meta: CallMeta) -> None:
        super().__init__()
        self.client = client
        self.meta = meta

    @property
    def model(self) -> str:
        return "qwen3-8b-local"

    @property
    def provider(self) -> str:
        return "yueyue-8877"

    def chat(
        self,
        *,
        chat_ctx: llm.ChatContext,
        tools: list[llm.Tool] | None = None,
        conn_options: APIConnectOptions = DEFAULT_API_CONNECT_OPTIONS,
        parallel_tool_calls: NotGivenOr[bool] = NOT_GIVEN,
        tool_choice: NotGivenOr[llm.ToolChoice] = NOT_GIVEN,
        extra_kwargs: NotGivenOr[dict] = NOT_GIVEN,
    ) -> llm.LLMStream:
        return YueyueLLMStream(
            self, chat_ctx=chat_ctx, tools=tools or [], conn_options=conn_options
        )


class YueyueLLMStream(llm.LLMStream):
    def __init__(
        self,
        llm_instance: YueyueLLM,
        *,
        chat_ctx: llm.ChatContext,
        tools: list[llm.Tool],
        conn_options: APIConnectOptions,
    ) -> None:
        super().__init__(
            llm_instance,
            chat_ctx=chat_ctx,
            tools=tools,
            conn_options=conn_options,
        )

    async def _run(self) -> None:
        text = _last_user_text(self._chat_ctx)
        if not text:
            return
        request_id = uuid.uuid4().hex
        logger.info("LLM 输入: %s", text)
        started = time.perf_counter()
        first_token_logged = False
        try:
            async for kind, payload in self._llm.client.chat(
                text,
                emotion=self._llm.meta.emotion,
                tone=self._llm.meta.tone,
            ):
                if kind == "token":
                    if not first_token_logged:
                        first_token_logged = True
                        logger.info(
                            "LLM 首 token: %.2fs", time.perf_counter() - started
                        )
                    self._event_ch.send_nowait(
                        llm.ChatChunk(
                            id=request_id,
                            delta=llm.ChoiceDelta(role="assistant", content=payload),
                        )
                    )
                elif kind == "skip":
                    logger.info("LLM 跳过回复（8877 回复策略）: %s", payload[:120])
                else:
                    break
        except asyncio.CancelledError:
            # 用户打断：让 8877 停掉本地生成，避免继续占 GPU
            await asyncio.shield(self._llm.client.interrupt())
            raise
        except Exception as exc:
            # 8877 不可用/模型加载中：这一轮不回话，但不要打断通话
            logger.warning("LLM 失败（本轮不回话）: %s", exc)


def _last_user_text(chat_ctx: llm.ChatContext) -> str:
    for message in reversed(chat_ctx.messages()):
        if message.role != "user":
            continue
        content = message.raw_text_content
        if content and content.strip():
            return content.strip()
    return ""


# ---------------------------------------------------------------- TTS
class YueyueTTS(tts.TTS):
    def __init__(
        self, client: YueyueClient, bargein: BargeInState | None = None
    ) -> None:
        super().__init__(
            capabilities=tts.TTSCapabilities(streaming=True),
            sample_rate=32000,
            num_channels=1,
        )
        self.client = client
        self.bargein = bargein

    @property
    def model(self) -> str:
        return "gsv-tts-lite"

    @property
    def provider(self) -> str:
        return "yueyue-8877"

    def synthesize(
        self, text: str, *, conn_options: APIConnectOptions = DEFAULT_API_CONNECT_OPTIONS
    ) -> tts.ChunkedStream:
        return self._synthesize_with_stream(text, conn_options=conn_options)

    def stream(
        self, *, conn_options: APIConnectOptions = DEFAULT_API_CONNECT_OPTIONS
    ) -> tts.SynthesizeStream:
        return YueyueSynthesizeStream(tts=self, conn_options=conn_options)


class YueyueSynthesizeStream(tts.SynthesizeStream):
    async def _run(self, output_emitter: tts.AudioEmitter) -> None:
        output_emitter.initialize(
            request_id=uuid.uuid4().hex,
            sample_rate=self._tts.sample_rate,
            num_channels=self._tts.num_channels,
            mime_type="audio/pcm",
            frame_size_ms=100,
            stream=False,
        )
        pending = ""
        pushed = False
        async for item in self._input_ch:
            if isinstance(item, self._FlushSentinel):
                text, pending = pending.strip(), ""
                if text:
                    pushed = (await self._synthesize(text, output_emitter)) or pushed
            else:
                pending += item
        text = pending.strip()
        if text:
            pushed = (await self._synthesize(text, output_emitter)) or pushed

        if not pushed:
            # 至少推一小段静音：TTS 下游不可用时，避免 SynthesizeStream 抛
            # "no audio frames were pushed" 把整通电话打断。
            output_emitter.push(b"\x00\x00" * int(self._tts.sample_rate * 0.1))

    async def _synthesize(self, text: str, output_emitter: tts.AudioEmitter) -> bool:
        first = True
        pushed = False
        started = time.perf_counter()
        bargein = self._tts.bargein
        if bargein is not None:
            bargein.note_tts(text)
        try:
            async for pcm, sample_rate in self._tts.client.tts_stream(text):
                if sample_rate != self._tts.sample_rate:
                    logger.warning(
                        "TTS 采样率 %s 与声明 %s 不一致，按声明播放",
                        sample_rate,
                        self._tts.sample_rate,
                    )
                if first:
                    logger.info(
                        "TTS 首包 (%.2fs): %s",
                        time.perf_counter() - started,
                        text[:30],
                    )
                    first = False
                if bargein is not None and bargein.ducked:
                    pcm = _apply_gain(pcm, 0.45)
                output_emitter.push(pcm)
                pushed = True
        except asyncio.CancelledError:
            raise
        except Exception as exc:
            logger.warning("TTS 失败（本句静音）: %s", exc)
        return pushed
