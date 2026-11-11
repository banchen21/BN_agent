"""边听边说 / 智能打断（只由本控制器做决策）。

行为：
- AI 说话/思考期间 VAD/ASR 常开（RoomIO 不 mute 用户麦克风）；
- 用户开口时先把 TTS 音量 duck 到 45%，短附和（嗯/对/好/哦…）不打断；
- 真正的插话（非短附和）在 final transcript 到达时取消当前 TTS/LLM 并回复新内容；
- 框架层 `turn_handling.interruption.min_duration` 只做“超长连续说话（>=3s）”兜底，
  不再用 0.8s 的 VAD 阈值抢先打断——短附和的判定权在本控制器；
- 日志统一格式：
    bargein: backchannel text=... chars=... duration=...s duck=0.45
    bargein: interrupt (stage=thinking|speaking|vad) text=... duration=...s cancelled_tts=...
"""
from __future__ import annotations

import asyncio
import logging
import re
import time
from dataclasses import dataclass

from config import BargeInSettings

logger = logging.getLogger("call.bargein")

_STRIP_RE = re.compile(r"[\s，。！？、,.!?；;：:…~～\"'“”‘’()（）\[\]【】<>《》]")

_BACKCHANNELS = {
    "嗯", "嗯嗯", "嗯呢", "哦", "哦哦", "噢", "喔",
    "对", "对对", "对啊", "对呀",
    "好", "好嘞", "好啊", "好呀", "行", "行啊",
    "是", "是的", "是啊", "对的对的",
    "啊", "诶", "欸", "唉", "哈", "哈哈", "呵", "嘿嘿",
    "可以", "可以啊", "ok", "okay",
}


def normalize_text(text: str) -> str:
    return _STRIP_RE.sub("", text or "").lower()


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
    overlap: bool = False  # 用户开口时 agent 正在思考/说话
    stage_at_user_start: str = ""
    user_speech_started_at: float = 0.0
    vad_interrupted: bool = False  # 框架 VAD 已经先把 agent 停下来了
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
    def __init__(
        self,
        session,
        state: BargeInState,
        cfg: BargeInSettings,
        *,
        framework_min_duration: float = 3.0,
    ) -> None:
        self._session = session
        self.state = state
        self.cfg = cfg
        self._framework_min_duration = framework_min_duration
        self._restore_task: asyncio.Task | None = None
        self._max_duck_handle: asyncio.TimerHandle | None = None
        self._vad_fallback_handle: asyncio.TimerHandle | None = None
        self._transcript_guard_handle: asyncio.TimerHandle | None = None

    def attach(self) -> None:
        self._session.on("agent_state_changed", self._on_agent_state)
        self._session.on("user_state_changed", self._on_user_state)
        self._session.on("user_input_transcribed", self._on_transcript)
        logger.info(
            "bargein: controller attached duck=%.2f framework_min_duration=%.1fs",
            self.cfg.duck_gain,
            self._framework_min_duration,
        )

    # ---------------------------------------------------------------- 事件
    def _on_agent_state(self, ev) -> None:
        prev = self.state.agent_state
        new = ev.new_state
        self.state.agent_state = new

        became_inactive = prev in ("thinking", "speaking") and new not in ("thinking", "speaking")
        if became_inactive and self.state.overlap and self.state.user_speaking:
            duration = time.time() - (self.state.user_speech_started_at or time.time())
            if duration >= self._framework_min_duration * 0.75:
                # 框架 VAD 抢先打断了本轮回复（>= min_duration 的长语音）。
                # 给 ASR 转写留 1.5s 做分类；转写没到就补一条 stage=vad 日志。
                self.state.vad_interrupted = True
                self._schedule_vad_fallback()
            else:
                # 回复恰好说完，用户这句按新的一轮处理，不记成 VAD 打断
                self.state.overlap = False
            self._restore()
        elif not self.state.agent_active:
            self._restore()

    def _on_user_state(self, ev) -> None:
        if ev.new_state == "speaking":
            self.state.user_speaking = True
            if self.state.agent_active:
                if not self.state.overlap:
                    self.state.overlap = True
                    self.state.stage_at_user_start = self.state.agent_state
                    self.state.user_speech_started_at = time.time()
                    self.state.vad_interrupted = False
                    logger.info(
                        "bargein: user started while agent active (stage=%s) -> duck=%.2f",
                        self.state.agent_state,
                        self.cfg.duck_gain,
                    )
                self._duck()
            return

        if ev.old_state == "speaking":
            self.state.user_speaking = False
            if self.state.ducked:
                self._schedule_restore()
            if self.state.overlap:
                # 转写迟迟不来时清理 overlap，避免状态泄漏
                self._schedule_transcript_guard()

    def _on_transcript(self, ev) -> None:
        if not getattr(ev, "is_final", False):
            return
        text = (ev.transcript or "").strip()
        if not text or not self.state.overlap:
            return

        started = self.state.user_speech_started_at
        duration = time.time() - started if started else 0.0
        stage_at_start = self.state.stage_at_user_start or self.state.agent_state
        still_active = self.state.agent_active
        vad_interrupted = self.state.vad_interrupted

        # 先清状态，再决定是否 interrupt（避免 interrupt 触发的 agent_state 变化误判为 VAD 打断）
        self.state.overlap = False
        self.state.vad_interrupted = False
        self._cancel_vad_fallback()
        self._cancel_transcript_guard()

        chars = len(normalize_text(text))
        if is_backchannel(text):
            logger.info(
                "bargein: backchannel text=%r chars=%d duration=%.2fs duck=%.2f%s",
                text,
                chars,
                duration,
                self.cfg.duck_gain,
                " (framework_vad_already_interrupted)" if vad_interrupted else "",
            )
            self._schedule_restore()
            return

        stage = stage_at_start if still_active else "vad"
        cancelled = self.state.last_tts_sentence
        logger.info(
            "bargein: interrupt (stage=%s) text=%r chars=%d duration=%.2fs cancelled_tts=%r",
            stage,
            text,
            chars,
            duration,
            cancelled[-40:],
        )
        if still_active:
            try:
                fut = self._session.interrupt()
                if fut is not None:
                    fut.add_done_callback(self._on_interrupt_done)
            except Exception as exc:  # noqa: BLE001
                logger.warning("bargein: session.interrupt 调用失败: %s", exc)
        self._restore()

    @staticmethod
    def _on_interrupt_done(fut: asyncio.Future) -> None:
        try:
            fut.result()
        except Exception as exc:  # noqa: BLE001
            logger.debug("bargein: interrupt 未生效: %s", exc)

    # ---------------------------------------------------------------- VAD 兜底
    def _schedule_vad_fallback(self) -> None:
        self._cancel_vad_fallback()
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return
        self._vad_fallback_handle = loop.call_later(1.5, self._vad_fallback_log)

    def _vad_fallback_log(self) -> None:
        if not self.state.overlap:
            return
        started = self.state.user_speech_started_at
        duration = time.time() - started if started else 0.0
        logger.info(
            "bargein: interrupt (stage=vad) duration=%.2fs cancelled_tts=%r",
            duration,
            self.state.last_tts_sentence[-40:],
        )
        self.state.overlap = False
        self.state.vad_interrupted = False

    def _cancel_vad_fallback(self) -> None:
        if self._vad_fallback_handle is not None:
            self._vad_fallback_handle.cancel()
            self._vad_fallback_handle = None

    def _schedule_transcript_guard(self) -> None:
        self._cancel_transcript_guard()
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return
        self._transcript_guard_handle = loop.call_later(4.5, self._clear_stale_overlap)

    def _clear_stale_overlap(self) -> None:
        if self.state.overlap and not self.state.agent_active:
            logger.debug("bargein: transcript timeout, 清理 overlap（ASR 可能为空）")
            self.state.overlap = False
            self.state.vad_interrupted = False

    def _cancel_transcript_guard(self) -> None:
        if self._transcript_guard_handle is not None:
            self._transcript_guard_handle.cancel()
            self._transcript_guard_handle = None

    # ---------------------------------------------------------------- duck
    def _duck(self) -> None:
        self._cancel_restore()
        self.state.ducked = True
        try:
            loop = asyncio.get_running_loop()
        except RuntimeError:
            return
        if self._max_duck_handle is not None:
            self._max_duck_handle.cancel()
        self._max_duck_handle = loop.call_later(self.cfg.max_duck, self._restore)

    def _schedule_restore(self) -> None:
        self._cancel_restore()

        async def _restore_later() -> None:
            await asyncio.sleep(self.cfg.restore_after)
            self._restore()

        self._restore_task = asyncio.create_task(_restore_later())

    def _restore(self) -> None:
        if self.state.ducked:
            logger.info("bargein: restore tts volume")
        self.state.ducked = False
        if self._max_duck_handle is not None:
            self._max_duck_handle.cancel()
            self._max_duck_handle = None

    def _cancel_restore(self) -> None:
        if self._restore_task is not None and not self._restore_task.done():
            self._restore_task.cancel()
        self._restore_task = None
