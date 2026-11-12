"""GPU 争用协调：生图串行，通话期间不抢显存。

- `image_slot()`：生图互斥锁；通话中会先等待，超时抛 GpuBusy。
- `begin_call()/end_call()`：LiveKit Agent 在通话开始/结束时调用。
"""
from __future__ import annotations

import logging
import threading
import time
from contextlib import contextmanager

log = logging.getLogger("ai-bot.gpu")


class GpuBusy(RuntimeError):
    """通话占用 GPU 太久，本次生图放弃。"""


class GpuCoordinator:
    def __init__(self, wait_call_seconds: int = 60) -> None:
        self._image_lock = threading.Lock()
        self._state = threading.Lock()
        self._idle = threading.Condition(self._state)
        self._in_call = 0
        self._wait_call_seconds = wait_call_seconds
        self._image_running = False

    # ---------------------------------------------------------------- 通话状态
    def begin_call(self) -> int:
        with self._state:
            self._in_call += 1
            count = self._in_call
        log.info("通话开始，当前通话数=%d，生图进行中=%s", count, self._image_running)
        return count

    def end_call(self) -> int:
        with self._state:
            self._in_call = max(0, self._in_call - 1)
            count = self._in_call
            self._idle.notify_all()
        log.info("通话结束，当前通话数=%d", count)
        return count

    def status(self) -> dict:
        with self._state:
            return {
                "in_call": self._in_call,
                "image_running": self._image_running,
            }

    def wait_image_idle(self, timeout: float = 30.0) -> bool:
        """等当前生图结束（Phase 3 建房间前可以调一下，避免显存打架）。"""
        deadline = time.monotonic() + timeout
        acquired = self._image_lock.acquire(timeout=max(0.0, timeout))
        if acquired:
            self._image_lock.release()
            return True
        return time.monotonic() >= deadline

    # ---------------------------------------------------------------- 生图互斥
    @contextmanager
    def image_slot(self, wait_call_seconds: float | None = None):
        wait_call = (
            self._wait_call_seconds if wait_call_seconds is None else wait_call_seconds
        )
        deadline = time.monotonic() + wait_call
        with self._state:
            while self._in_call > 0:
                remaining = deadline - time.monotonic()
                if remaining <= 0:
                    raise GpuBusy("正在通话中，先不生成图片")
                self._idle.wait(timeout=min(remaining, 5.0))

        # 生图串行
        acquired = self._image_lock.acquire(timeout=wait_call + 300)
        if not acquired:
            raise GpuBusy("生图排队超时")
        with self._state:
            self._image_running = True
        try:
            yield
        finally:
            with self._state:
                self._image_running = False
            self._image_lock.release()
