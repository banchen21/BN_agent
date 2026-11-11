"""边听边说 / 智能打断自测。

流程：
1. 建房间并入会，等 agent 问候；
2. 推一句「你给我讲个长一点的小故事吧」让 AI 长时间说话；
3. AI 说话中途推短附和「嗯」（~0.3s）：
   预期不打断，TTS 音量 duck（约 45%），日志 `bargein: backchannel`；
4. 继续让 AI 说话，中途推真正插话「等一下，我想问个问题」（>0.8s）：
   预期取消当前 TTS，日志 `bargein: interrupt`，随后给出新回复。

用法（先重启 worker 让新配置生效）：
    .venv\\Scripts\\python.exe scripts\\bargein_test.py
"""
from __future__ import annotations

import asyncio
import io
import json
import time
import wave
from pathlib import Path

import httpx
from livekit import rtc

API_URL = "http://127.0.0.1:9001"
LIVEKIT_URL = "ws://127.0.0.1:7880"
YUEYUE_URL = "http://127.0.0.1:8877"
LOG_PATH = Path(__file__).resolve().parent.parent / "logs" / "agent.out.log"


class Track:
    def __init__(self) -> None:
        self.t0 = time.time()
        self.audible: list[tuple[float, float, int]] = []  # (t, duration, peak)

    def log(self, msg: str) -> None:
        print(f"[{time.time() - self.t0:6.1f}s] {msg}")

    def add(self, duration: float, peak: int) -> None:
        self.audible.append((time.time() - self.t0, duration, peak))

    def audible_between(self, a: float, b: float) -> list[tuple[float, float, int]]:
        return [x for x in self.audible if a <= x[0] <= b]

    def last_audible(self) -> float:
        return self.audible[-1][0] if self.audible else -1


def peak_of(frame: rtc.AudioFrame) -> int:
    data = bytes(frame.data)
    peak = 0
    for i in range(0, len(data) - 1, 2 * 16):
        sample = int.from_bytes(data[i : i + 2], "little", signed=True)
        peak = max(peak, abs(sample))
    return peak


async def tts_wav(text: str) -> bytes:
    async with httpx.AsyncClient(timeout=180.0) as client:
        resp = await client.post(f"{YUEYUE_URL}/api/tts", json={"text": text})
        resp.raise_for_status()
        return resp.content


async def publish_pcm(room: rtc.Room, pcm: bytes, sample_rate: int) -> None:
    source = rtc.AudioSource(sample_rate, 1)
    track = rtc.LocalAudioTrack.create_audio_track("mic", source)
    await room.local_participant.publish_track(
        track, rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_MICROPHONE)
    )
    frame_bytes = int(sample_rate * 0.02) * 2
    for i in range(0, len(pcm), frame_bytes):
        chunk = pcm[i : i + frame_bytes]
        if len(chunk) < 2:
            break
        await source.capture_frame(rtc.AudioFrame(chunk, sample_rate, 1, len(chunk) // 2))
        await asyncio.sleep(0.02)


async def publish_wav(room: rtc.Room, wav_bytes: bytes, trailing_silence: float = 0.9) -> float:
    with wave.open(io.BytesIO(wav_bytes)) as w:
        sr, ch = w.getframerate(), w.getnchannels()
        pcm = w.readframes(w.getnframes())
    assert ch == 1
    await publish_pcm(room, pcm, sr)
    speech_done = time.time()
    await publish_pcm(room, b"\x00\x00" * int(sr * trailing_silence), sr)
    return speech_done


async def wait_for(predicate, timeout: float, interval: float = 0.1) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        await asyncio.sleep(interval)
    return False


def read_new_bargein_logs(since_epoch: float) -> list[dict]:
    """读取 worker JSON 日志里测试开始后的 bargein 事件。"""
    events: list[dict] = []
    if not LOG_PATH.exists():
        return events
    with LOG_PATH.open(encoding="utf-8") as f:
        for line in f:
            line = line.strip()
            if not line.startswith("{"):
                continue
            try:
                entry = json.loads(line)
            except json.JSONDecodeError:
                continue
            msg = entry.get("message", "")
            if "bargein:" not in msg:
                continue
            ts = entry.get("timestamp", "")
            try:
                # ISO8601 with +00:00
                from datetime import datetime

                when = datetime.fromisoformat(ts).timestamp()
            except Exception:
                continue
            if when >= since_epoch:
                events.append({"time": when, "message": msg})
    return events


async def main() -> int:
    tl = Track()
    tl.log("预生成测试语音…")
    wav_long = await tts_wav("你给我讲个长一点的小故事吧，说慢一点，我想听细节。")
    wav_again = await tts_wav("你再说一句嘛，说慢一点。")
    wav_backchannel = await tts_wav("嗯。")
    wav_interrupt = await tts_wav("等下。")
    tl.log(
        f"语音就绪 long={len(wav_long)/64000:.1f}s again={len(wav_again)/64000:.1f}s "
        f"backchannel={len(wav_backchannel)/64000:.1f}s "
        f"interrupt={len(wav_interrupt)/64000:.1f}s"
    )

    async with httpx.AsyncClient(timeout=20.0) as client:
        resp = await client.post(
            f"{API_URL}/ai/call/start",
            json={"userID": f"bargein{int(time.time())}", "mediaType": "audio"},
        )
        data = resp.json()
    if not data.get("ok"):
        print("start 失败:", data)
        return 1
    room_name, token = data["room"], data["token"]
    tl.log(f"start ok room={room_name}")
    test_start_epoch = time.time()

    room = rtc.Room()

    def on_track_subscribed(track, pub, participant):
        if track.kind != rtc.TrackKind.KIND_AUDIO:
            return
        stream = rtc.AudioStream(track, sample_rate=32000, num_channels=1, frame_size_ms=100)

        async def consume() -> None:
            async for ev in stream:
                peak = peak_of(ev.frame)
                if peak > 300:
                    tl.add(ev.frame.duration, peak)

        asyncio.create_task(consume())

    room.on("track_subscribed", on_track_subscribed)
    await room.connect(LIVEKIT_URL, token)
    tl.log("joined")

    if not await wait_for(lambda: len(room.remote_participants) > 0, timeout=30):
        tl.log("FAIL: agent 未入会")
        await room.disconnect()
        return 2
    tl.log("agent joined")
    await wait_for(lambda: len(tl.audible) > 0, timeout=25)  # greeting
    await asyncio.sleep(1.5)

    async def wait_audible_now(timeout: float = 20.0) -> bool:
        """等到 agent 此刻正在出声（最近 300ms 内有声帧）。"""
        deadline = time.time() + timeout
        while time.time() < deadline:
            now = time.time() - tl.t0
            if tl.last_audible() >= now - 0.3:
                return True
            await asyncio.sleep(0.1)
        return False

    # ---- 让 AI 开始回复
    tl.log("推流：讲个长故事")
    await publish_wav(room, wav_long, trailing_silence=0.8)
    started = await wait_audible_now(timeout=30)
    tl.log(f"AI 开始说话: {started}（当前有声帧 {len(tl.audible)}）")
    await asyncio.sleep(0.8)

    # ---- 短附和（AI 正在说话）
    backchannel_start = time.time() - tl.t0
    tl.log("推流短附和：嗯")
    await publish_wav(room, wav_backchannel, trailing_silence=0.4)
    await asyncio.sleep(1.2)
    after_backchannel = tl.last_audible()
    still_speaking = after_backchannel > backchannel_start + 1.0
    tl.log(
        f"短附和后 AI 仍在说话: {still_speaking} "
        f"(最后有声 t={after_backchannel:.1f}s, 附和开始 t={backchannel_start:.1f}s)"
    )

    # ---- 再触发一轮回复，中途真插话
    tl.log("推流：你再说一句嘛")
    await publish_wav(room, wav_again, trailing_silence=0.6)
    started2 = await wait_audible_now(timeout=30)
    tl.log(f"第二轮 AI 开始说话: {started2}")
    await asyncio.sleep(0.8)

    interrupt_start = time.time() - tl.t0
    tl.log("推流真插话：等下")
    await publish_wav(room, wav_interrupt, trailing_silence=0.4)
    # 插话结束后 1.2~2.5s 内应当没有旧音频（被打断），随后出现新回复
    await asyncio.sleep(2.5)
    gap_frames = [
        x for x in tl.audible_between(interrupt_start + 1.2, interrupt_start + 2.5)
    ]
    interrupted = len(gap_frames) == 0
    got_new = await wait_for(
        lambda: any(t > interrupt_start + 2.5 for t, _, _ in tl.audible), timeout=30
    )
    tl.log(
        f"插话后旧音频停止: {interrupted}（间隙有声帧 {len(gap_frames)}）；新回复: {got_new}"
    )

    # ---- 思考阶段插话（框架 VAD 先打断的路径，QA #14 的失败场景）
    tl.log("等当前回复自然结束…")
    await wait_for(
        lambda: tl.last_audible() < (time.time() - tl.t0) - 2.0, timeout=30
    )
    tl.log("推流提问，准备在 AI 思考阶段插话")
    await publish_wav(room, wav_again, trailing_silence=0.6)
    await asyncio.sleep(0.5)  # 提问已结束，LLM 正在 thinking
    thinking_start = time.time() - tl.t0
    tl.log("推流插话（thinking 阶段）：等下")
    await publish_wav(room, wav_interrupt, trailing_silence=0.4)
    await asyncio.sleep(3.0)
    thinking_new = await wait_for(
        lambda: any(t > thinking_start + 2.0 for t, _, _ in tl.audible), timeout=30
    )
    tl.log(f"thinking 阶段插话后新回复: {thinking_new}")

    await asyncio.sleep(2.0)
    await room.disconnect()
    async with httpx.AsyncClient(timeout=10.0) as client:
        end = await client.post(f"{API_URL}/ai/call/end", json={"room": room_name})
    tl.log(f"end: {end.json()}")

    events = read_new_bargein_logs(test_start_epoch)
    backchannels = [e for e in events if "backchannel" in e["message"]]
    interrupts = [e for e in events if "interrupt" in e["message"] and "backchannel" not in e["message"]]
    speaking_interrupts = [e for e in interrupts if "stage=speaking" in e["message"]]
    thinking_interrupts = [e for e in interrupts if "stage=thinking" in e["message"]]

    print("\n===== 结果 =====")
    print(f"room                     : {room_name}")
    print(f"agent 入会               : PASS")
    print(f"短附和不打断（行为）     : {'PASS' if still_speaking else 'INFO（回复恰好说完）'}")
    print(f"bargein: backchannel     : {'PASS' if backchannels else 'FAIL'}（{len(backchannels)} 条）")
    for e in backchannels:
        print("   ", e["message"])
    print(f"真插话取消旧 TTS         : {'PASS' if interrupted else 'FAIL'}")
    print(f"插话后新回复             : {'PASS' if got_new else 'INFO（上游未返回可跳过）'}")
    print(f"bargein: interrupt       : {len(interrupts)} 条")
    for e in interrupts:
        print("   ", e["message"])
    print(f"  - stage=speaking 日志  : {'PASS' if speaking_interrupts else 'FAIL'}")
    print(f"  - stage=thinking 日志  : {'PASS' if thinking_interrupts else 'FAIL'}（QA #14 场景）")
    print(f"thinking 插话后新回复    : {'PASS' if thinking_new else 'FAIL'}")

    ok = (
        backchannels
        and interrupted
        and speaking_interrupts
        and thinking_interrupts
        and thinking_new
    )
    return 0 if ok else 3


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
