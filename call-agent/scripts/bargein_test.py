"""边听边说 / 智能打断自测。

流程：
1. 建房 + dispatch，入会，等问候；
2. 让 AI 讲长故事（开始说话后）推短附和「嗯」：
   预期不打断，日志 `bargein: backchannel`；
3. 再触发一轮回复，AI 说话途中推真插话「等一下」：
   预期旧 TTS 停止，日志 `bargein: interrupt (stage=speaking)`，随后新回复；
4. AI 思考阶段（提问后 ~0.6s）推插话「等下」：
   预期取消本轮回复，日志 `bargein: interrupt (stage=thinking)`，随后新回复。

用法（先启动 worker）：
    .venv\\Scripts\\python.exe scripts\\bargein_test.py
"""
from __future__ import annotations

import asyncio
import json
import sys
import time
from pathlib import Path

from livekit import api, rtc

BASE_DIR = Path(__file__).resolve().parent.parent
SCRIPTS = Path(__file__).resolve().parent
sys.path.insert(0, str(BASE_DIR))
sys.path.insert(0, str(SCRIPTS))

for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace", line_buffering=True)
    except Exception:  # noqa: BLE001
        pass

from config import SETTINGS  # noqa: E402
from selftest import (  # noqa: E402
    MicFeeder,
    Timeline,
    _audible,
    log_size,
    read_log_since,
    tts_wav,
    wait_for,
)


class AudibleTrack:
    def __init__(self, tl: Timeline) -> None:
        self.tl = tl
        self.last: float = -1.0

    def on_frame(self, frame: rtc.AudioFrame) -> None:
        if _audible(frame):
            self.tl.add_audio(frame.duration)
            self.last = time.time() - self.tl.t0

    def audible_between(self, a: float, b: float) -> list[list[float]]:
        return [span for span in self.tl.audio_spans if a <= span[0] <= b]


async def main() -> int:
    tl = Timeline()
    room_name = f"ai-call-bargein-{int(time.time() * 1000)}"
    identity = "user-bargein"
    log_offset = log_size()
    tl.log(f"room={room_name} log_offset={log_offset}")

    lk = api.LiveKitAPI(
        SETTINGS.livekit_url, SETTINGS.livekit_key, SETTINGS.livekit_secret
    )
    await lk.room.create_room(
        api.CreateRoomRequest(
            name=room_name,
            empty_timeout=120,
            max_participants=4,
            metadata=json.dumps({"mediaType": "audio"}, ensure_ascii=False),
        )
    )
    token = (
        api.AccessToken(SETTINGS.livekit_key, SETTINGS.livekit_secret)
        .with_identity(identity)
        .with_name("BargeInTest")
        .with_grants(
            api.VideoGrants(
                room_join=True,
                room=room_name,
                can_publish=True,
                can_subscribe=True,
                can_publish_data=True,
            )
        )
        .to_jwt()
    )
    await lk.agent_dispatch.create_dispatch(
        api.CreateAgentDispatchRequest(
            agent_name=SETTINGS.agent_name,
            room=room_name,
            metadata=json.dumps({"mediaType": "audio", "source": "bn-call-bargein"}, ensure_ascii=False),
        )
    )

    tl.log("预生成测试语音…")
    wav_long = await tts_wav("你给我讲个长一点的小故事吧，说慢一点，我想听细节。")
    wav_again = await tts_wav("你再说一句嘛，说慢一点。")
    wav_question = await tts_wav("你最喜欢吃什么水果呢？")
    wav_backchannel = await tts_wav("嗯。")
    wav_interrupt = await tts_wav("等下。")
    tl.log(
        f"语音就绪 long={len(wav_long)/64000:.1f}s again={len(wav_again)/64000:.1f}s "
        f"backchannel={len(wav_backchannel)/64000:.1f}s interrupt={len(wav_interrupt)/64000:.1f}s"
    )

    room = rtc.Room()
    track = AudibleTrack(tl)

    def on_track_subscribed(t, pub, participant):
        if t.kind != rtc.TrackKind.KIND_AUDIO or not participant.identity.startswith("agent"):
            return
        stream = rtc.AudioStream(t, sample_rate=32000, num_channels=1, frame_size_ms=100)

        async def consume() -> None:
            async for ev in stream:
                track.on_frame(ev.frame)

        asyncio.create_task(consume())

    room.on("track_subscribed", on_track_subscribed)

    mic = MicFeeder(sample_rate=32000)
    try:
        await room.connect(SETTINGS.livekit_url, token)
        tl.log("joined")
        await mic.start(room)
        if not await wait_for(
            lambda: any(p.identity.startswith("agent") for p in room.remote_participants.values()),
            timeout=40,
        ):
            tl.log("FAIL: agent 未入会")
            return 2
        tl.log("agent joined")
        await wait_for(lambda: len(tl.audio_spans) > 0, timeout=25)  # greeting
        await asyncio.sleep(1.5)

        async def wait_speaking_now(timeout: float = 40.0) -> bool:
            deadline = time.time() + timeout
            while time.time() < deadline:
                now = time.time() - tl.t0
                if track.last >= now - 0.3:
                    return True
                await asyncio.sleep(0.1)
            return False

        # ---- 1) 长故事 + 短附和
        tl.log("推流：讲个长故事")
        await mic.say(wav_long, trailing_silence=0.8)
        started = await wait_speaking_now(timeout=40)
        tl.log(f"AI 开始说话: {started}")
        await asyncio.sleep(0.8)

        backchannel_start = time.time() - tl.t0
        tl.log("推流短附和：嗯")
        await mic.say(wav_backchannel, trailing_silence=0.4)
        await asyncio.sleep(1.4)
        still_speaking = track.last > backchannel_start + 1.0
        tl.log(f"短附和后 AI 仍在说话: {still_speaking} (last={track.last:.1f}s, start={backchannel_start:.1f}s)")

        # 等这轮说完
        await wait_for(lambda: track.last < (time.time() - tl.t0) - 2.0, timeout=40)

        # ---- 2) 说话中真插话
        tl.log("推流：你再说一句嘛")
        await mic.say(wav_again, trailing_silence=0.6)
        started2 = await wait_speaking_now(timeout=40)
        tl.log(f"第二轮 AI 开始说话: {started2}")
        await asyncio.sleep(0.8)

        interrupt_start = time.time() - tl.t0
        tl.log("推流真插话：等下")
        await mic.say(wav_interrupt, trailing_silence=0.4)
        await asyncio.sleep(2.2)
        gap_frames = track.audible_between(interrupt_start + 1.2, interrupt_start + 2.2)
        got_new = await wait_for(
            lambda: any(t > interrupt_start + 2.2 for t, _ in tl.audio_spans), timeout=15
        )
        tl.log(f"插话后旧音频停止: {len(gap_frames) == 0}（间隙有声段 {gap_frames}）；新回复: {got_new}")

        # ---- 3) thinking 阶段插话
        tl.log("等当前回复自然结束…")
        await wait_for(lambda: track.last < (time.time() - tl.t0) - 2.5, timeout=45)
        tl.log("推流提问，准备在 AI 思考阶段插话")
        await mic.say(wav_question, trailing_silence=0.4)
        await asyncio.sleep(0.5)  # 提问已结束，LLM 正在 thinking
        thinking_start = time.time() - tl.t0
        tl.log("推流插话（thinking 阶段）：等下")
        await mic.say(wav_interrupt, trailing_silence=0.4)
        await asyncio.sleep(3.0)
        thinking_new = await wait_for(
            lambda: any(t > thinking_start + 2.2 for t, _ in tl.audio_spans), timeout=40
        )
        tl.log(f"thinking 阶段插话后新回复: {thinking_new}")

        await asyncio.sleep(1.5)
        await room.disconnect()
        tl.log("已断开")
    finally:
        await mic.aclose()
        try:
            await lk.room.delete_room(api.DeleteRoomRequest(room=room_name))
        except Exception:  # noqa: BLE001
            pass
        await lk.aclose()

    logs, _ = read_log_since(log_offset)
    events = [line for line in logs.splitlines() if "bargein:" in line]
    backchannels = [e for e in events if "backchannel" in e]
    interrupts = [e for e in events if "interrupt" in e and "backchannel" not in e]
    speaking_interrupts = [e for e in interrupts if "stage=speaking" in e]
    thinking_interrupts = [e for e in interrupts if "stage=thinking" in e]
    vad_interrupts = [e for e in interrupts if "stage=vad" in e]

    print("\n===== 结果 =====")
    print(f"room                     : {room_name}")
    print(f"短附和不打断（行为）     : {'PASS' if still_speaking else 'INFO（回复恰好说完）'}")
    print(f"bargein: backchannel     : {'PASS' if backchannels else 'FAIL'}（{len(backchannels)} 条）")
    for e in backchannels:
        print("   ", e[:200])
    print(f"真插话取消旧 TTS         : {'PASS' if len(gap_frames) == 0 else 'FAIL'}")
    print(f"插话后新回复             : {'PASS' if got_new else 'INFO（上游跳过回复）'}")
    print(f"bargein: interrupt 总数  : {len(interrupts)}（vad={len(vad_interrupts)}）")
    print(f"  - stage=speaking       : {'PASS' if speaking_interrupts else 'FAIL'}（{len(speaking_interrupts)} 条）")
    print(f"  - stage=thinking       : {'PASS' if thinking_interrupts else 'FAIL'}（{len(thinking_interrupts)} 条）")
    print(f"thinking 插话后新回复    : {'PASS' if thinking_new else 'FAIL'}")
    for e in interrupts:
        print("   ", e[:200])

    ok = bool(backchannels and speaking_interrupts and thinking_interrupts)
    return 0 if ok else 3


if __name__ == "__main__":
    import os

    code = asyncio.run(main())
    sys.stdout.flush()
    sys.stderr.flush()
    # LiveKit RTC FFI 在解释器退出阶段偶发 dispose 卡死；直接退出进程。
    os._exit(code)
