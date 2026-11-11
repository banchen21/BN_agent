"""ai-call 自测：不依赖手机/Electron，直接用 LiveKit Python SDK 入会。

流程：
1. POST /ai/call/start 建房拿 token
2. 以普通参与者入会，等 agent participant
3. 等入会问候语（agent 音频帧）
4. 用 8877 合成一段语音，作为麦克风推流 -> 等 ASR/LLM/TTS 回复音频
5. AI 说话途中再推一段语音，检查打断（旧音频停止、新回复开始）
6. POST /ai/call/end

用法：
    .venv\\Scripts\\python.exe scripts\\selftest.py
"""
from __future__ import annotations

import asyncio
import io
import time
import wave

import httpx
from livekit import rtc

API_URL = "http://127.0.0.1:9001"
LIVEKIT_URL = "ws://127.0.0.1:7880"
YUEYUE_URL = "http://127.0.0.1:8877"


class Timeline:
    def __init__(self) -> None:
        self.t0 = time.time()
        self.events: list[tuple[float, str]] = []
        self.agent_audio = 0.0
        self.audio_spans: list[list[float]] = []  # [start, end]

    def log(self, msg: str) -> None:
        t = time.time() - self.t0
        self.events.append((t, msg))
        print(f"[{t:6.1f}s] {msg}")

    def add_audio(self, duration: float) -> None:
        t = time.time() - self.t0
        self.agent_audio += duration
        if not self.audio_spans or t - self.audio_spans[-1][1] > 0.5:
            self.audio_spans.append([t, t])
        self.audio_spans[-1][1] = t


def _audible(frame: rtc.AudioFrame, threshold: int = 300) -> bool:
    """AudioStream 订阅的轨道会持续送静音帧；用振幅判断真的是否在说话。"""
    data = bytes(frame.data)
    peak = 0
    for i in range(0, len(data) - 1, 2 * 24):  # 抽样，够判断即可
        sample = int.from_bytes(data[i : i + 2], "little", signed=True)
        peak = max(peak, abs(sample))
        if peak > threshold:
            return True
    return False


async def tts_wav(text: str) -> bytes:
    async with httpx.AsyncClient(timeout=180.0) as client:
        resp = await client.post(f"{YUEYUE_URL}/api/tts", json={"text": text})
        resp.raise_for_status()
        return resp.content


async def publish_pcm(room: rtc.Room, pcm: bytes, sample_rate: int) -> None:
    """把 int16 单声道 PCM 以麦克风轨道推流（20ms/帧）。"""
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
        await source.capture_frame(
            rtc.AudioFrame(chunk, sample_rate, 1, len(chunk) // 2)
        )
        await asyncio.sleep(0.02)


async def publish_wav(room: rtc.Room, wav_bytes: bytes, trailing_silence: float = 1.2) -> float:
    """推流一段 WAV；返回“语音部分”推完的墙钟时间。"""
    with wave.open(io.BytesIO(wav_bytes)) as w:
        sr, ch = w.getframerate(), w.getnchannels()
        pcm = w.readframes(w.getnframes())
    assert ch == 1, f"只支持单声道 WAV，当前 {ch}"
    await publish_pcm(room, pcm, sr)
    speech_done = time.time()
    # 尾部静音，让 VAD 触发 END_OF_SPEECH
    await publish_pcm(room, b"\x00\x00" * int(sr * trailing_silence), sr)
    return speech_done


async def wait_for(predicate, timeout: float, interval: float = 0.1) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        await asyncio.sleep(interval)
    return False


async def main() -> int:
    tl = Timeline()
    user_id = f"selftest{int(time.time())}"

    # 预先合成两段语音，避免通话中占用 TTS 引擎、干扰 agent 的 TTS 首包延迟
    tl.log("预生成测试语音…")
    wav1 = await tts_wav("你今天过得怎么样呢？")
    wav2 = await tts_wav("等一下，我想先问你一个问题。")
    tl.log(f"测试语音就绪 wav1={len(wav1)//2/32000:.1f}s wav2={len(wav2)//2/32000:.1f}s")

    async with httpx.AsyncClient(timeout=20.0) as client:
        resp = await client.post(
            f"{API_URL}/ai/call/start", json={"userID": user_id, "mediaType": "audio"}
        )
        data = resp.json()
    if not data.get("ok"):
        print("start 失败:", data)
        return 1
    room_name, token = data["room"], data["token"]
    tl.log(f"start ok room={room_name} url={data['url']} dispatch={data.get('agentDispatchId')}")

    room = rtc.Room()

    def on_track_subscribed(track, pub, participant):
        tl.log(f"track subscribed: {track.kind} <- {participant.identity}")
        if track.kind == rtc.TrackKind.KIND_AUDIO:
            stream = rtc.AudioStream(
                track, sample_rate=32000, num_channels=1, frame_size_ms=100
            )

            async def consume() -> None:
                async for ev in stream:
                    # 轨道会持续送静音帧，只统计真的有声音的帧
                    if _audible(ev.frame):
                        tl.add_audio(ev.frame.duration)

            asyncio.create_task(consume())

    room.on("track_subscribed", on_track_subscribed)
    room.on("participant_connected", lambda p: tl.log(f"participant connected: {p.identity}"))
    room.on("participant_disconnected", lambda p: tl.log(f"participant left: {p.identity}"))

    await room.connect(LIVEKIT_URL, token)
    tl.log(f"joined room={room.name} identity={room.local_participant.identity}")

    joined = await wait_for(lambda: len(room.remote_participants) > 0, timeout=30)
    tl.log(f"remote participants: {[p.identity for p in room.remote_participants.values()]}")
    if not joined:
        tl.log("FAIL: 30s 内 agent 未入会")
        await room.disconnect()
        return 2

    # ---- 1) 入会问候
    greeting = await wait_for(lambda: len(tl.audio_spans) > 0, timeout=25)
    tl.log(f"问候音频: {'有' if greeting else '无'} total={tl.agent_audio:.2f}s")
    await asyncio.sleep(2.0)

    # ---- 2) 说话 -> ASR/LLM/TTS（一句短问句，减少段间打断干扰）
    baseline_audio = tl.agent_audio
    tl.log("推流语音：你今天过得怎么样呢？")
    t_speech_done = await publish_wav(room, wav1, trailing_silence=0.8)
    tl.log("语音推流结束，等回复…")

    replied = await wait_for(
        lambda: tl.agent_audio > baseline_audio + 0.3, timeout=35
    )
    first_audio = next(
        (span[0] for span in tl.audio_spans if span[0] > t_speech_done - tl.t0), -1
    )
    tl.log(
        f"ASR/LLM/TTS 回复: {'有' if replied else '无'} "
        f"(语音结束到首块声音约 {first_audio - (t_speech_done - tl.t0):.1f}s)"
        if replied
        else "ASR/LLM/TTS 回复: 无"
    )
    await asyncio.sleep(6.0)  # 让第一轮说完（TTS 可能需要几秒）

    # ---- 3) 打断：AI 说话途中再次开口
    interrupted = False
    if replied:
        before = tl.agent_audio
        tl.log("插话推流（AI 刚刚/正在说话）")
        await publish_wav(room, wav2, trailing_silence=0.8)
        mid = tl.agent_audio
        got_new = await wait_for(lambda: tl.agent_audio > mid + 0.3, timeout=30)
        tl.log(
            f"打断后新回复: {'有' if got_new else '无'} "
            f"(插话前 {before:.2f}s，插话后 {mid:.2f}s，总计 {tl.agent_audio:.2f}s)"
        )
        interrupted = got_new

    await room.disconnect()
    async with httpx.AsyncClient(timeout=10.0) as client:
        end = await client.post(f"{API_URL}/ai/call/end", json={"room": room_name})
    tl.log(f"end: {end.json()}")

    print("\n===== 结果 =====")
    print(f"room           : {room_name}")
    print(f"agent 入会     : {'PASS' if joined else 'FAIL'}")
    print(f"问候有声       : {'PASS' if greeting else 'FAIL'}")
    print(f"ASR->LLM->TTS  : {'PASS' if replied else 'FAIL'}")
    print(f"打断后继续对话 : {'PASS' if interrupted else 'SKIP/FAIL'}")
    print(f"AI 有声总时长  : {tl.agent_audio:.2f}s")
    print("有声时间线     :", [f"{a:.1f}-{b:.1f}s" for a, b in tl.audio_spans])
    return 0 if (joined and greeting and replied) else 3


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
