"""边听边说 / 智能打断。

行为：
- AI 说话期间 VAD/ASR 常开（RoomIO 默认即常开，不做 mute）；
- 用户开口时先轻 duck TTS 音量（45%），短附和（嗯/对/好/哦…）不打断；
- 真正的插话（非短附和，或语音时长 ≥ min_duration）取消当前 TTS 并回复新内容。

框架层：`turn_handling.interruption.min_duration` 负责长语音打断；
本模块负责：duck、短附和/插话分类、短插话（< min_duration 但 ≥2 字）的补打断、日志。
"""
from __future__ import annotations

import asyncio
import logging
import re
import time
from dataclasses import dataclass

logger = logging.getLogger("ai-call.bargein")

_DUCK_GAIN = 0.45
_RESTORE_AFTER = 0.6
_MAX_DUCK = 2.0

_STRIP_RE = re.compile(r"[\s，。！？、,.!?；;：:…~～\"'“”‘’()（）\[\]【】<>《》]")

_BACKCHANNELS = {
    "嗯",
    "嗯嗯",
    "哦",
    "哦哦",
    "噢",
    "对",
    "对对",
    "好",
    "好的",
    "好嘞",
    "行",
    "是",
    "是的",
    "啊",
    "诶",
    "欸",
    "唉",
    "哈",
    "哈哈",
    "呵",
    "可以",
}


def normalize_text(text: str) -> str:
    return _STRIP_RE.sub("", text or "")


def is_backchannel(text: str) -> bool:
    normalized = normalize_text(text)
    if not normalized:
        return False
    return normalized in _BACKCHANNELS


@dataclass
class BargeInState:
    agent_state: str = "idle"
    user_speaking: bool = False
    ducked: bool = False
    user_speech_started_at: float = 0.0
    agent_active_when_user_started: bool = False
    stage_at_user_start: str = ""
    last_tts_sentence: str = ""
    last_tts_at: float = 0.0

    @property
    def agent_speaking(self) -> bool:
        return self.agent_state == "speaking"

    @property
    def agent_active(self) -> bool:
        # thinking 阶段回复尚未出声，但用户此时插话同样应取消这一轮回复
        return self.agent_state in ("thinking", "speaking")

    def note_tts(self, text: str) -> None:
        self.last_tts_sentence = text
        self.last_tts_at = time.time()


class BargeInController:
    def __init__(self, session, state: BargeInState) -> None:
        self._session = session
        self.state = state
        self._restore_task: asyncio.Task | None = None

    def attach(self) -> None:
        self._session.on("agent_state_changed", self._on_agent_state)
        self._session.on("user_state_changed", self._on_user_state)
        self._session.on("user_input_transcribed", self._on_transcript)

    # ---------------------------------------------------------------- events
    def _on_agent_state(self, ev) -> None:
        prev_state = self.state.agent_state
        self.state.agent_state = ev.new_state
        if not self.state.agent_active:
            if (
                prev_state in ("thinking", "speaking")
                and self.state.agent_active_when_user_started
                and self.state.user_speaking
            ):
                # 框架可能因 VAD min_duration 直接打断，转写还没到；
                # 给转写留 1.2s 做分类，超过则记一次 VAD 打断兜底日志。
                try:
                    asyncio.get_running_loop().call_later(1.2, self._fallback_interrupt_log)
                except RuntimeError:
                    pass
            self._restore()

    def _fallback_interrupt_log(self) -> None:
        if not self.state.agent_active_when_user_started:
            return
        started = self.state.user_speech_started_at
        duration = time.time() - started if started else 0.0
        logger.info(
            "bargein: interrupt (vad) duration=%.2fs cancelled_tts=%r",
            duration,
            self.state.last_tts_sentence[-40:],
        )
        self.state.agent_active_when_user_started = False

    def _on_user_state(self, ev) -> None:
        now = time.time()
        if ev.new_state == "speaking":
            if self.state.agent_active:
                self.state.user_speaking = True
                self.state.user_speech_started_at = now
                self.state.agent_active_when_user_started = True
                self.state.stage_at_user_start = self.state.agent_state
                self._duck()
                logger.info(
                    "bargein: user started while agent active (stage=%s) -> duck",
                    self.state.agent_state,
                )
            return

        if ev.old_state == "speaking":
            self.state.user_speaking = False
            if self.state.ducked:
                self._schedule_restore()

    def _on_transcript(self, ev) -> None:
        if not ev.is_final:
            return
        text = (ev.transcript or "").strip()
        if not text:
            return

        started = self.state.user_speech_started_at
        duration = time.time() - started if started else 0.0
        was_overlap = self.state.agent_active_when_user_started
        stage = self.state.stage_at_user_start or self.state.agent_state
        self.state.agent_active_when_user_started = False

        if not was_overlap:
            return
        # 转写已到达，取消 VAD 兜底日志

        chars = len(normalize_text(text))
        if is_backchannel(text):
            # 短附和：不打断，恢复音量
            logger.info(
                "bargein: backchannel text=%r chars=%d duration=%.2fs duck=%.2f",
                text,
                chars,
                duration,
                _DUCK_GAIN,
            )
            self._schedule_restore()
            return

        # 真正的插话：>=2 字或时长达到阈值
        cancelled = self.state.last_tts_sentence
        logger.info(
            "bargein: interrupt (stage=%s) text=%r chars=%d duration=%.2fs cancelled_tts=%r",
            stage,
            text,
            chars,
            duration,
            cancelled[-40:],
        )
        if self.state.agent_active:
            # VAD min_duration 未到时，靠转写补一次打断
            try:
                fut = self._session.interrupt()
                fut.add_done_callback(self._on_interrupt_done)
            except Exception as exc:
                logger.warning("bargein: session.interrupt 调用失败: %s", exc)
        self._restore()

    @staticmethod
    def _on_interrupt_done(fut: asyncio.Future) -> None:
        try:
            fut.result()
        except Exception as exc:
            logger.debug("bargein: interrupt 未生效: %s", exc)

    # ---------------------------------------------------------------- duck
    def _duck(self) -> None:
        self._cancel_restore()
        self.state.ducked = True
        # 兜底：最长 duck 2 秒，避免事件丢失导致音量一直低
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return
        loop.call_later(_MAX_DUCK, self._restore)

    def _schedule_restore(self) -> None:
        self._cancel_restore()

        async def _restore_later() -> None:
            await asyncio.sleep(_RESTORE_AFTER)
            self._restore()

        self._restore_task = asyncio.create_task(_restore_later())

    def _restore(self) -> None:
        if self.state.ducked:
            logger.info("bargein: restore tts volume")
        self.state.ducked = False

    def _cancel_restore(self) -> None:
        if self._restore_task is not None and not self._restore_task.done():
            self._restore_task.cancel()
        self._restore_task = None
