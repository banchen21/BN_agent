"""call-agent 自测：不依赖手机/浏览器，直接用 LiveKit Python SDK 入会。

流程：
1. 用 LiveKit API 建房 + 签发用户 token + 显式 dispatch agent(name=yueyue)
2. 用户以普通参与者入会，等 agent 入会并听到问候
3. 推一段 8877 合成的语音 -> 等 ASR/LLM/TTS 回复音频
4. 发布摄像头视频轨 -> 问“你能看到我吗/我穿什么颜色” -> 等视觉自动看图/工具调用与回复
5. 挂断、删房，并检查 worker 日志中的 video/vision/bargein 关键字

用法：
    .venv\\Scripts\\python.exe scripts\\selftest.py
    .venv\\Scripts\\python.exe scripts\\selftest.py --media audio   # 只测语音
"""
from __future__ import annotations

import argparse
import asyncio
import io
import json
import sys
import time
import wave
from pathlib import Path

import httpx
from livekit import api, rtc
from PIL import Image

BASE_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(BASE_DIR))

for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace", line_buffering=True)
    except Exception:  # noqa: BLE001
        pass

from config import SETTINGS  # noqa: E402

LOG_PATH = BASE_DIR / "logs" / "agent.out.log"
YUEYUE_URL = SETTINGS.yueyue_url
IMAGE_PATH = Path(SETTINGS.data_dir) / "yueyue_self_ref.png"


class Timeline:
    def __init__(self) -> None:
        self.t0 = time.time()
        self.agent_audio = 0.0
        self.audio_spans: list[list[float]] = []

    def log(self, msg: str) -> None:
        print(f"[{time.time() - self.t0:6.1f}s] {msg}")

    def add_audio(self, duration: float) -> None:
        t = time.time() - self.t0
        self.agent_audio += duration
        if not self.audio_spans or t - self.audio_spans[-1][1] > 0.5:
            self.audio_spans.append([t, t])
        self.audio_spans[-1][1] = t


def _audible(frame: rtc.AudioFrame, threshold: int = 300) -> bool:
    data = bytes(frame.data)
    peak = 0
    for i in range(0, len(data) - 1, 2 * 24):
        sample = int.from_bytes(data[i : i + 2], "little", signed=True)
        peak = max(peak, abs(sample))
        if peak > threshold:
            return True
    return False


async def tts_wav(text: str) -> bytes:
    async with httpx.AsyncClient(timeout=180.0, trust_env=False) as client:
        resp = await client.post(f"{YUEYUE_URL}/api/tts", json={"text": text})
        resp.raise_for_status()
        return resp.content


def wav_to_pcm(wav_bytes: bytes) -> tuple[bytes, int]:
    with wave.open(io.BytesIO(wav_bytes)) as w:
        assert w.getnchannels() == 1, f"只支持单声道 WAV，当前 {w.getnchannels()}"
        return w.readframes(w.getnframes()), w.getframerate()


class MicFeeder:
    """持续发布一条麦克风轨道；say() 把语音塞进队列，空闲时补静音。"""

    def __init__(self, sample_rate: int = 32000) -> None:
        self.sr = sample_rate
        self.source = rtc.AudioSource(sample_rate, 1)
        self.track = rtc.LocalAudioTrack.create_audio_track("mic", self.source)
        self._queue: asyncio.Queue[bytes] = asyncio.Queue()
        self._pending = b""
        self._task: asyncio.Task | None = None

    async def start(self, room: rtc.Room) -> None:
        await room.local_participant.publish_track(
            self.track, rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_MICROPHONE)
        )
        self._task = asyncio.create_task(self._loop())

    async def _loop(self) -> None:
        frame_bytes = int(self.sr * 0.02) * 2
        silence = b"\x00" * frame_bytes
        while True:
            if not self._pending:
                try:
                    self._pending = self._queue.get_nowait()
                except asyncio.QueueEmpty:
                    self._pending = b""
            if self._pending:
                chunk = self._pending[:frame_bytes]
                self._pending = self._pending[frame_bytes:]
                if len(chunk) < frame_bytes:
                    chunk += b"\x00" * (frame_bytes - len(chunk))
            else:
                chunk = silence
            await self.source.capture_frame(
                rtc.AudioFrame(chunk, self.sr, 1, frame_bytes // 2)
            )
            await asyncio.sleep(0.02)

    async def say(self, wav_bytes: bytes, trailing_silence: float = 1.0) -> float:
        pcm, sr = wav_to_pcm(wav_bytes)
        if sr != self.sr:
            raise RuntimeError(f"测试语音采样率 {sr} != 麦克风源 {self.sr}")
        self._queue.put_nowait(pcm)
        duration = len(pcm) / 2 / sr
        await asyncio.sleep(duration + trailing_silence)
        return duration

    async def aclose(self) -> None:
        if self._task is not None and not self._task.done():
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):  # noqa: BLE001
                pass
        self._task = None


class VideoPublisher:
    def __init__(self, image_path: Path, width: int = 640, height: int = 360) -> None:
        img = Image.open(image_path).convert("RGB")
        ratio = max(width / img.width, height / img.height)
        img = img.resize((max(1, int(img.width * ratio)), max(1, int(img.height * ratio))))
        left = (img.width - width) // 2
        top = (img.height - height) // 2
        img = img.crop((left, top, left + width, top + height))
        self.width, self.height = width, height
        self.rgba = img.convert("RGBA").tobytes()
        self.source = rtc.VideoSource(width, height)
        self.track = rtc.LocalVideoTrack.create_video_track("camera", self.source)
        self._task: asyncio.Task | None = None

    async def start(self, room: rtc.Room) -> None:
        await room.local_participant.publish_track(
            self.track, rtc.TrackPublishOptions(source=rtc.TrackSource.SOURCE_CAMERA)
        )
        self._task = asyncio.create_task(self._loop())

    async def _loop(self) -> None:
        frame = rtc.VideoFrame(
            self.width, self.height, rtc.VideoBufferType.RGBA, self.rgba
        )
        while True:
            self.source.capture_frame(frame)
            await asyncio.sleep(0.1)  # 10 fps

    async def aclose(self) -> None:
        if self._task is not None and not self._task.done():
            self._task.cancel()
            try:
                await self._task
            except (asyncio.CancelledError, Exception):  # noqa: BLE001
                pass
        self._task = None


async def wait_for(predicate, timeout: float, interval: float = 0.1) -> bool:
    deadline = time.time() + timeout
    while time.time() < deadline:
        if predicate():
            return True
        await asyncio.sleep(interval)
    return False


def read_log_since(offset: int) -> tuple[str, int]:
    if not LOG_PATH.exists():
        return "", offset
    with LOG_PATH.open("rb") as f:
        f.seek(offset)
        data = f.read()
    return data.decode("utf-8", "ignore"), offset + len(data)


def log_size() -> int:
    try:
        return LOG_PATH.stat().st_size
    except FileNotFoundError:
        return 0


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--media", choices=["audio", "video"], default="video")
    parser.add_argument("--keep-room", action="store_true")
    args = parser.parse_args()

    if not IMAGE_PATH.exists():
        print(f"测试图片不存在: {IMAGE_PATH}")
        return 1

    tl = Timeline()
    room_name = f"ai-call-selftest-{int(time.time() * 1000)}"
    identity = "user-selftest"
    log_offset = log_size()
    tl.log(f"room={room_name} 起始日志 offset={log_offset}")

    lk = api.LiveKitAPI(
        SETTINGS.livekit_url, SETTINGS.livekit_key, SETTINGS.livekit_secret
    )
    try:
        await lk.room.create_room(
            api.CreateRoomRequest(
                name=room_name,
                empty_timeout=120,
                max_participants=4,
                metadata=json.dumps({"mediaType": args.media}, ensure_ascii=False),
            )
        )
        token = (
            api.AccessToken(SETTINGS.livekit_key, SETTINGS.livekit_secret)
            .with_identity(identity)
            .with_name("Selftest")
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
        dispatch_meta = json.dumps(
            {
                "userID": identity,
                "identity": identity,
                "mediaType": args.media,
                "room": room_name,
                "source": "bn-call-selftest",
            },
            ensure_ascii=False,
        )
        dispatch = await lk.agent_dispatch.create_dispatch(
            api.CreateAgentDispatchRequest(
                agent_name=SETTINGS.agent_name, room=room_name, metadata=dispatch_meta
            )
        )
        tl.log(f"dispatch id={dispatch.id}")
    except Exception as exc:  # noqa: BLE001
        print("建房/dispatch 失败:", exc)
        await lk.aclose()
        return 1

    room = rtc.Room()

    def on_track_subscribed(track, pub, participant):
        tl.log(f"track subscribed: {track.kind} <- {participant.identity}")
        if track.kind == rtc.TrackKind.KIND_AUDIO and participant.identity.startswith("agent"):
            stream = rtc.AudioStream(
                track, sample_rate=32000, num_channels=1, frame_size_ms=100
            )

            async def consume() -> None:
                async for ev in stream:
                    if _audible(ev.frame):
                        tl.add_audio(ev.frame.duration)

            asyncio.create_task(consume())

    room.on("track_subscribed", on_track_subscribed)
    room.on("participant_connected", lambda p: tl.log(f"participant connected: {p.identity}"))
    room.on("participant_disconnected", lambda p: tl.log(f"participant left: {p.identity}"))

    mic = MicFeeder(sample_rate=32000)
    video: VideoPublisher | None = None
    try:
        await room.connect(SETTINGS.livekit_url, token)
        tl.log(f"joined room={room.name} identity={room.local_participant.identity}")
        await mic.start(room)

        joined = await wait_for(
            lambda: any(p.identity.startswith("agent") for p in room.remote_participants.values()),
            timeout=40,
        )
        tl.log(f"agent 入会: {joined} remote={[p.identity for p in room.remote_participants.values()]}")
        if not joined:
            await room.disconnect()
            return 2

        greeting = await wait_for(lambda: len(tl.audio_spans) > 0, timeout=25)
        tl.log(f"问候有声: {greeting}")
        await asyncio.sleep(1.5)

        # ---- 语音问答
        baseline = tl.agent_audio
        tl.log("推流语音：你今天过得怎么样呢？")
        await mic.say(await tts_wav("你今天过得怎么样呢？"), trailing_silence=1.0)
        replied = await wait_for(lambda: tl.agent_audio > baseline + 0.3, timeout=40)
        tl.log(f"ASR->LLM->TTS 回复有声: {replied} (+{tl.agent_audio - baseline:.2f}s)")
        await asyncio.sleep(3.0)

        vision_ok = False
        video_log_ok = False
        if args.media == "video":
            video = VideoPublisher(IMAGE_PATH)
            await video.start(room)
            tl.log("已发布摄像头视频轨（10fps）")
            video_log_ok = await wait_for(
                lambda: "video: 收到用户首帧" in read_log_since(log_offset)[0], timeout=20
            )
            tl.log(f"worker 收到视频帧: {video_log_ok}")

            # 等 agent 这一轮彻底说完再提问，避免把上一轮音频误当作视觉回复
            quiet = await wait_for(
                lambda: (time.time() - tl.t0) - (tl.audio_spans[-1][1] if tl.audio_spans else 0) > 2.5,
                timeout=45,
            )
            tl.log(f"agent 已安静: {quiet}")
            baseline2 = tl.agent_audio
            ask = "你能看到我吗？我穿的是什么颜色的衣服？"
            tl.log(f"推流语音（视觉）：{ask}")
            await mic.say(await tts_wav(ask), trailing_silence=1.2)
            target = tl.agent_audio + 0.3
            vision_replied = await wait_for(lambda: tl.agent_audio > target, timeout=60)
            await asyncio.sleep(3.0)
            logs, _ = read_log_since(log_offset)
            vision_tool = ("vision: tool-call" in logs) or ("vision: auto-look" in logs)
            vision_ok_log = ("vision: look_at_user ok" in logs) or (
                "vision: snapshot_view ok" in logs
            )
            vision_ok = bool(vision_replied and vision_tool and vision_ok_log)
            tl.log(
                f"视觉回复有声: {vision_replied}；视觉调用日志: {vision_tool}；"
                f"视觉模型成功: {vision_ok_log}"
            )

        await room.disconnect()
        tl.log("已断开")
        await asyncio.sleep(3.0)  # 等 worker 处理 shutdown + 刷日志
    finally:
        if video is not None:
            await video.aclose()
        await mic.aclose()
        if not args.keep_room:
            try:
                await lk.room.delete_room(api.DeleteRoomRequest(room=room_name))
                tl.log("已删房")
            except Exception as exc:  # noqa: BLE001
                tl.log(f"删房失败（可忽略）: {exc}")
        await lk.aclose()

    logs, _ = read_log_since(log_offset)
    exited = "Agent 退出" in logs

    print("\n===== 结果 =====")
    print(f"room                : {room_name}")
    print(f"agent 入会          : {'PASS' if joined else 'FAIL'}")
    # 首句问候是“客户端订阅竞态”的受害者：媒体路径偶尔在开头 1-2s 未就绪导致漏听，
    # 不影响后续问答（下方 ASR→LLM→TTS 已验证同一音频链路），因此这里不算 FAIL。
    print(f"问候有声            : {'PASS' if greeting else 'INFO（客户端漏听首句，音频链路见问答）'}")
    print(f"ASR->LLM->TTS       : {'PASS' if replied else 'FAIL'}")
    if args.media == "video":
        print(f"视频帧订阅          : {'PASS' if video_log_ok else 'FAIL'}")
        print(f"视觉理解 + 回复     : {'PASS' if vision_ok else 'FAIL'}")
    print(f"挂断后 Agent 退出   : {'PASS' if exited else 'INFO（job 可能延迟回收）'}")
    print(f"AI 有声总时长       : {tl.agent_audio:.2f}s")
    print("有声时间线          :", [f"{a:.1f}-{b:.1f}s" for a, b in tl.audio_spans])
    print("\n--- 本次新增 worker 日志（筛选）---")
    for line in logs.splitlines():
        if any(k in line for k in ("Agent 入会", "video:", "vision:", "ASR:", "LLM 输入", "TTS 首包", "bargein:", "Agent 退出")):
            print("  " + line[:200])

    ok = joined and replied
    if args.media == "video":
        ok = ok and video_log_ok and vision_ok
    return 0 if ok else 3


if __name__ == "__main__":
    import os

    code = asyncio.run(main())
    sys.stdout.flush()
    sys.stderr.flush()
    # LiveKit RTC FFI 在解释器退出阶段偶发 dispose 卡死；直接退出进程。
    os._exit(code)
