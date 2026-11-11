"""LiveKit Agents 自定义插件：STT / LLM / TTS 全部包装本地 yueyue 8877。

- STT：非流式 recognize（VAD 切句由 livekit.agents.stt.StreamAdapter 完成）
- LLM：/api/chat 的 SSE token 流；支持视觉工具标签（<look>/<snapshot/>/<tool_call>）
       与关键词自动看图兜底，工具结果回合会带视觉内容再请求一次 8877
- TTS：/api/tts_stream 的 SSE int16 PCM，首包即播；duck 时对后续 PCM 降增益

打断：
- LLM 流被取消 -> POST /api/interrupt，停掉本地生成
- TTS 流被取消 -> 关闭 HTTP 流，8877 侧生成随之结束
- 真正由 BargeInController 决策是否调用 session.interrupt()
"""
from __future__ import annotations

import asyncio
import json
import logging
import re
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
from frames import FrameStore
from vision import VisionClient, VisionError
from yueyue import YueyueClient, frame_to_wav

logger = logging.getLogger("call.plugins")

try:  # numpy 是 livekit 依赖，通常可用；失败时退回纯 Python
    import numpy as _np
except Exception:  # noqa: BLE001
    _np = None

_ALLOWED_TOOLS = {"look_at_user", "snapshot_view"}


def _apply_gain(pcm: bytes, gain: float) -> bytes:
    """对 int16 PCM 做线性增益（duck 用）。"""
    if gain >= 0.999 or not pcm:
        return pcm
    if _np is not None:
        samples = _np.frombuffer(pcm, dtype=_np.int16)
        scaled = (samples.astype(_np.float32) * gain).astype(_np.int16)
        return scaled.tobytes()
    import array

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

        started = time.perf_counter()
        try:
            result = await self._client.asr(wav)
        except Exception as exc:  # noqa: BLE001 - 8877 抖动不应打断整通电话
            logger.warning("ASR 失败（忽略本句）: %s", exc)
            return _empty()

        self.meta.update(emotion=result.emotion, tone=result.tone)
        if not result.text:
            return _empty()

        logger.info(
            "ASR: %s (emotion=%s, %.2fs)",
            result.text,
            result.emotion or "-",
            time.perf_counter() - started,
        )
        return stt.SpeechEvent(
            type=stt.SpeechEventType.FINAL_TRANSCRIPT,
            request_id=request_id,
            alternatives=[stt.SpeechData(language="zh", text=result.text)],
        )


# ---------------------------------------------------------------- 工具标签解析
_START_RE = re.compile(r"<(look|snapshot|tool_call)\b", re.IGNORECASE)
_MARKER_STARTS = ("<look", "<snapshot", "<tool_call")


def _partial_marker_len(buf: str) -> int:
    low = buf.lower()
    idx = low.rfind("<")
    if idx == -1:
        return 0
    tail = low[idx:]
    for marker in _MARKER_STARTS:
        if marker.startswith(tail) or (tail.startswith(marker) and len(tail) <= len(marker) + 1):
            return len(tail)
    return 0


def _parse_tool_payload(tag: str, inner: str) -> dict | None:
    inner = inner.strip().lstrip(">").strip()
    if tag == "look":
        question = inner.strip()
        return {"name": "look_at_user", "arguments": {"question": question}}
    if tag == "snapshot":
        return {"name": "snapshot_view", "arguments": {}}
    if tag == "tool_call":
        try:
            data = json.loads(inner)
        except json.JSONDecodeError:
            logger.warning("LLM 输出的 <tool_call> 不是合法 JSON: %r", inner[:120])
            return None
        if not isinstance(data, dict):
            return None
        name = str(data.get("name") or "")
        arguments = data.get("arguments")
        if name not in _ALLOWED_TOOLS:
            logger.warning("LLM 请求了未登记的工具: %r", name)
            return None
        if not isinstance(arguments, dict):
            arguments = {}
        return {"name": name, "arguments": arguments}
    return None


class _ToolTagStreamer:
    """把 8877 的 token 流按视觉工具标签切开。

    feed() 返回 (可立即输出的文本, 工具调用或 None)。发现完整工具标签后，
    后续文本丢弃（本轮工作交给工具执行后的第二次生成）。
    """

    def __init__(self) -> None:
        self._buf = ""
        self._found: dict | None = None

    def feed(self, text: str) -> tuple[str, dict | None]:
        if self._found is not None:
            return "", self._found
        self._buf += text
        out: list[str] = []
        while True:
            m = _START_RE.search(self._buf)
            if m is None:
                hold = _partial_marker_len(self._buf)
                if hold:
                    out.append(self._buf[:-hold])
                    self._buf = self._buf[-hold:]
                else:
                    out.append(self._buf)
                    self._buf = ""
                return "".join(out), None

            tag = m.group(1).lower()
            if m.start() > 0:
                out.append(self._buf[: m.start()])
                self._buf = self._buf[m.start():]

            if tag == "snapshot":
                end = re.search(r"/>|</snapshot\s*>|>", self._buf, re.IGNORECASE)
                if end is None:
                    if len(self._buf) > 200:
                        out.append(self._buf)
                        self._buf = ""
                    return "".join(out), None
                self._found = _parse_tool_payload("snapshot", "")
                self._buf = ""
                return "".join(out), self._found

            close = f"</{tag}>"
            idx = self._buf.lower().find(close)
            if idx == -1:
                if len(self._buf) > 800:
                    # 标签一直不闭合，按普通文本吐出，避免卡住回复
                    out.append(self._buf)
                    self._buf = ""
                return "".join(out), None

            inner = self._buf[m.end(): idx]
            self._buf = ""
            self._found = _parse_tool_payload(tag, inner)
            return "".join(out), self._found

    def flush(self) -> str:
        text, _ = self._buf, None
        self._buf = ""
        return text


# ---------------------------------------------------------------- LLM
class YueyueLLM(llm.LLM):
    def __init__(
        self,
        client: YueyueClient,
        meta: CallMeta,
        *,
        vision: VisionClient | None = None,
        frames: FrameStore | None = None,
    ) -> None:
        super().__init__()
        self.client = client
        self.meta = meta
        self.vision = vision
        self.frames = frames

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


def _last_user_text(chat_ctx: llm.ChatContext) -> str:
    for message in reversed(chat_ctx.messages()):
        if message.role != "user":
            continue
        content = message.raw_text_content
        if content and content.strip():
            return content.strip()
    return ""


def _pending_tool_output(chat_ctx: llm.ChatContext) -> tuple[str, str, str] | None:
    """若上一轮以工具结果结尾，返回 (工具名, 结果, 最近的用户原话)。"""
    items = chat_ctx.items
    if not items:
        return None
    for item in reversed(items):
        if isinstance(item, llm.FunctionCallOutput):
            return (
                item.name,
                str(item.output or ""),
                _last_user_text(chat_ctx),
            )
        if isinstance(item, llm.ChatMessage) and item.role == "user":
            return None
    return None


def _tools_note(tools: list[llm.Tool]) -> str:
    available = []
    for tool in tools:
        info = getattr(tool, "info", None)
        name = getattr(info, "name", "") or ""
        if name in _ALLOWED_TOOLS:
            available.append(f"- {name}: {getattr(info, 'description', '') or ''}")
    if not available:
        return ""
    return (
        "\n\n[通话视觉能力] 对方开着摄像头时你可以看到画面。"
        "只有当对方问到你看到了什么、他长什么样、穿的什么、画面/环境/房间里有什么，"
        "或你确实需要看画面才能回答时，才输出下面其中一个标签（不要编造看到的内容）：\n"
        "<look>你想确认的问题</look>（看一眼对方摄像头）\n"
        "<snapshot/>（截一帧描述场景）\n"
        "系统会把画面结果给你，你再自然地用口语回复。其他时候不要输出这些标签。"
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
        tool_result = _pending_tool_output(self._chat_ctx)
        auto_desc: str | None = None
        use_tools = False

        if tool_result is not None:
            name, output, user_text = tool_result
            logger.info("vision: tool-result name=%s chars=%d", name, len(output))
            prompt = (
                f"（系统补充：这是你刚才通过摄像头看到的内容：{output}。"
                f"对方刚才的问题是：{user_text}。"
                "请直接用口语回应对方，不要提到系统、工具或“画面描述”。）"
            )
        else:
            text = _last_user_text(self._chat_ctx)
            if not text:
                return
            prompt = text
            vision = self._llm.vision
            frames = self._llm.frames
            if vision is not None and frames is not None and vision.should_auto_look(text):
                snapshot = frames.latest_jpeg()
                if snapshot is None:
                    auto_desc = ""
                    logger.info("vision: auto-look 触发，但当前没有视频帧")
                else:
                    try:
                        auto_desc = await vision.look_at_user(snapshot.jpeg, text)
                        logger.info(
                            "vision: auto-look used text=%r frame_age=%.1fs",
                            text[:40],
                            snapshot.age,
                        )
                    except VisionError as exc:
                        auto_desc = ""
                        logger.warning("vision: auto-look 失败: %s", exc)
                if auto_desc:
                    prompt = (
                        f"{text}\n\n（系统补充：你刚看了一眼对方摄像头，看到：{auto_desc}。"
                        "请直接用口语回应对方，不要提到系统、工具或“画面描述”。）"
                    )
                else:
                    prompt = (
                        f"{text}\n\n（系统补充：你看了一眼，但摄像头没有可用画面。"
                        "请自然、诚实地告诉对方你现在看不到。）"
                    )
            else:
                note = _tools_note(self._tools)
                if note:
                    prompt = text + note
                    use_tools = True

        request_id = uuid.uuid4().hex
        logger.info("LLM 输入: %s", prompt[:240].replace("\n", " "))
        started = time.perf_counter()
        first_token_logged = False
        streamer = _ToolTagStreamer()
        tool_call: dict | None = None

        def _emit(text: str) -> None:
            nonlocal first_token_logged
            if not text:
                return
            if not first_token_logged:
                first_token_logged = True
                logger.info("LLM 首 token: %.2fs", time.perf_counter() - started)
            self._event_ch.send_nowait(
                llm.ChatChunk(
                    id=request_id,
                    delta=llm.ChoiceDelta(role="assistant", content=text),
                )
            )

        async def _consume(gen) -> None:
            nonlocal tool_call
            async for kind, payload in gen:
                if kind == "token":
                    text_out, found = streamer.feed(payload)
                    if text_out:
                        _emit(text_out)
                    if found is not None:
                        tool_call = found
                        break
                    continue
                if kind == "skip":
                    logger.info("LLM 跳过回复（8877 回复策略）: %s", payload[:120])
                    return
                break

        try:
            await _consume(
                self._llm.client.chat(
                    prompt,
                    emotion=self._llm.meta.emotion,
                    tone=self._llm.meta.tone,
                )
            )
            if tool_call is None:
                tail = streamer.flush()
                if tail:
                    _emit(tail)
        except asyncio.CancelledError:
            # 用户打断：让 8877 停掉本地生成，避免继续占 GPU
            await asyncio.shield(self._llm.client.interrupt())
            raise
        except Exception as exc:  # noqa: BLE001 - 8877 抖动不要打断通话
            logger.warning("LLM 失败（本轮不回话）: %s", exc)
            return

        if tool_call is not None:
            name = tool_call.get("name", "")
            args = tool_call.get("arguments") or {}
            logger.info(
                "vision: tool-call name=%s args=%s",
                name,
                json.dumps(args, ensure_ascii=False)[:160],
            )
            self._event_ch.send_nowait(
                llm.ChatChunk(
                    id=request_id,
                    delta=llm.ChoiceDelta(
                        role="assistant",
                        content=None,
                        tool_calls=[
                            llm.FunctionToolCall(
                                name=name,
                                arguments=json.dumps(args, ensure_ascii=False),
                                call_id="call_" + uuid.uuid4().hex[:12],
                            )
                        ],
                    ),
                )
            )
        elif use_tools and not first_token_logged:
            logger.info("LLM 本轮无文本输出")


# ---------------------------------------------------------------- TTS
class YueyueTTS(tts.TTS):
    def __init__(
        self,
        client: YueyueClient,
        *,
        bargein: BargeInState | None = None,
        duck_gain: float = 0.45,
    ) -> None:
        super().__init__(
            capabilities=tts.TTSCapabilities(streaming=True),
            sample_rate=32000,
            num_channels=1,
        )
        self.client = client
        self.bargein = bargein
        self.duck_gain = duck_gain

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
                    pcm = _apply_gain(pcm, self._tts.duck_gain)
                output_emitter.push(pcm)
                pushed = True
        except asyncio.CancelledError:
            raise
        except Exception as exc:  # noqa: BLE001
            logger.warning("TTS 失败（本句静音）: %s", exc)
        return pushed
