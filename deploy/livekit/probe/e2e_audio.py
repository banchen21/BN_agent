"""端到端探针（对着本机 LiveKit + 真后端联调用）：

  # 房间由 /api/call/start 创建，把完整响应存成 <start>.json 再喂给本脚本。
  # 默认：一个观察者订阅 yueyue 音频/视频，另开一个「麦克风+摄像头」参与者说一句
  LIVEKIT_CONFIG=deploy/livekit/livekit.yaml \
    .venv-call/Scripts/python.exe deploy/livekit/probe/e2e_audio.py <start>.json [秒数] [--video] [--phrase 1|2]

  # 真机已进房：只观察订阅（不发言），适合手机真的在说话时检查悦悦有没有回话
  ... --observe

说明：
- LiveKit 同一个 identity 多次进房会互踢（agent 已占用 agent-yueyue-<room>），
  发布者、观察者各用独立 identity。
- token 用服务端 API key 自签：LIVEKIT_API_KEY / LIVEKIT_API_SECRET，或 LIVEKIT_CONFIG
  指向 deploy/livekit/livekit.yaml（本机部署配置，不入库）。
- 合成语音用 edge-tts（中文女声）→ ffmpeg 16k wav → 48k 线性插值推流；
  缓存文件与收到的 yueyue 音频（yueyue_out.wav）都写在脚本目录。
- 两句以上时同一个发布者连续说：换发布者会被 agent 当成「对方离房」直接收尾。
"""
import array
import asyncio
import json
import os
import struct
import subprocess
import sys
import time

import numpy as np
from livekit import rtc

RATE = 48_000
HERE = os.path.dirname(os.path.abspath(__file__))
T0 = time.monotonic()


def ts():
    return f"{time.monotonic()-T0:6.2f}s"


def load(path):
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def token_for(room, identity):
    """用 LiveKit 服务端 API key 自签 token；key/secret 从环境变量或部署配置读。"""
    from livekit import api as lk_api

    key = os.environ.get("LIVEKIT_API_KEY", "")
    secret = os.environ.get("LIVEKIT_API_SECRET", "")
    if not key or not secret:
        cfg_path = os.environ.get("LIVEKIT_CONFIG", "")
        if cfg_path and os.path.exists(cfg_path):
            with open(cfg_path, encoding="utf-8") as f:
                for line in f:
                    name, _, value = line.strip().partition(":")
                    value = value.strip().strip('"')
                    if name == "key":
                        key = value
                    elif name == "secret":
                        secret = value
    if not key or not secret:
        raise SystemExit("需要 LIVEKIT_API_KEY / LIVEKIT_API_SECRET（或 LIVEKIT_CONFIG 指向 livekit.yaml）")
    return lk_api.AccessToken(key, secret) \
        .with_identity(identity).with_name(identity).with_grants(lk_api.VideoGrants(room_join=True, room=room)).to_jwt()


def say_16k(text, tag):
    """edge-tts → ffmpeg 16k mono wav，缓存到 qa/call/<tag>_16k.wav。"""
    out = os.path.join(HERE, f"{tag}_16k.wav")
    if os.path.exists(out):
        return out
    import edge_tts

    mp3 = out + ".mp3"
    asyncio.run(edge_tts.Communicate(text, "zh-CN-XiaoxiaoNeural").save(mp3))
    r = subprocess.run(
        ["ffmpeg", "-y", "-loglevel", "error", "-i", mp3, "-ar", "16000", "-ac", "1", out],
        capture_output=True,
    )
    if r.returncode != 0 or not os.path.exists(out):
        raise RuntimeError(f"合成失败：{r.stderr.decode(errors='replace')[:300]}")
    return out


def read_pcm16(path):
    with open(path, "rb") as f:
        data = f.read()
    pos = 12
    while pos + 8 <= len(data):
        cid = data[pos : pos + 4]
        size = struct.unpack("<I", data[pos + 4 : pos + 8])[0]
        if cid == b"data":
            a = array.array("h")
            a.frombytes(data[pos + 8 : pos + 8 + size - (size % 2)])
            return a
        pos += 8 + size + (size & 1)
    raise RuntimeError("WAV 缺 data")


def upsample3(pcm16):
    x = np.frombuffer(pcm16.tobytes(), dtype=np.int16).astype(np.float32)
    n = len(x)
    xi = np.linspace(0, n - 1, n * 3)
    idx = np.floor(xi).astype(np.int64)
    frac = xi - idx
    y = x[idx] * (1 - frac) + x[np.minimum(idx + 1, n - 1)] * frac
    return np.clip(y, -32768, 32767).astype(np.int16).tobytes()


class Observer:
    """订阅 yueyue 的音视频（不发布）。"""

    def __init__(self, room_name, identity):
        self.room_name = room_name
        self.identity = identity
        self.received = []
        self.video_frames = 0

    async def run(self, duration):
        room = rtc.Room()

        @room.on("track_subscribed")
        def _on_track(track, publication, participant):
            print(f"{ts()} [obs-sub] {publication.name!r} kind={track.kind} from={participant.identity}", flush=True)
            if track.kind == rtc.TrackKind.KIND_AUDIO and publication.name == "yueyue":
                asyncio.ensure_future(self._collect(track))
            elif track.kind == rtc.TrackKind.KIND_VIDEO:
                asyncio.ensure_future(self._video(track))

        await room.connect("ws://127.0.0.1:7880", token_for(self.room_name, self.identity), rtc.RoomOptions(auto_subscribe=True))
        print(f"{ts()} [obs] observer joined; remotes={[p.identity for p in room.remote_participants.values()]}", flush=True)
        await asyncio.sleep(duration)
        await room.disconnect()
        if self.received:
            pcm = b"".join(self.received)
            wav = (
                b"RIFF" + struct.pack("<I", 36 + len(pcm)) + b"WAVE"
                + b"fmt " + struct.pack("<IHHIIHH", 16, 1, 1, 16000, 32000, 2, 16)
                + b"data" + struct.pack("<I", len(pcm)) + pcm
            )
            with open(os.path.join(HERE, "yueyue_out.wav"), "wb") as f:
                f.write(wav)
            a = np.frombuffer(pcm, dtype=np.int16).astype(np.float64)
            loud = a[np.abs(a) > 300]
            print(
                f"{ts()} [obs] 收到 yueyue {len(pcm)/32000:.1f}s，有声样本 {len(loud)}，最大幅度 {int(np.abs(a).max()) if a.size else 0}"
                f" -> qa/call/yueyue_out.wav，视频帧 {self.video_frames}",
                flush=True,
            )
        else:
            print(f"{ts()} [obs] 没收到 yueyue 音频（视频帧 {self.video_frames}）", flush=True)

    async def _collect(self, track):
        stream = rtc.AudioStream(track, sample_rate=16000, num_channels=1, frame_size_ms=100)
        async for ev in stream:
            pcm = bytes(ev.frame.data)
            self.received.append(pcm)
            a = np.frombuffer(pcm, dtype=np.int16).astype(np.float64)
            rms = float(np.sqrt((a * a).mean())) if a.size else 0.0
            if rms > 1:
                print(f"{ts()} [obs-rx] rms={rms:6.0f}", flush=True)

    async def _video(self, track):
        stream = rtc.VideoStream(track)
        last = 0.0
        async for ev in stream:
            self.video_frames += 1
            if time.monotonic() - last > 2.0:
                last = time.monotonic()
                f = ev.frame
                print(f"{ts()} [obs-vid] {f.width}x{f.height}", flush=True)


class Speaker:
    """独立参与者：发布麦克风（说一句话）+ 可选摄像头。"""

    def __init__(self, room_name, identity):
        self.room_name = room_name
        self.identity = identity
        self.source = None

    async def run(self, at, text, with_video, duration):
        await asyncio.sleep(max(0.0, at - (time.monotonic() - T0)))
        room = rtc.Room()
        await room.connect("ws://127.0.0.1:7880", token_for(self.room_name, self.identity), rtc.RoomOptions(auto_subscribe=False))
        print(f"{ts()} [spk] {self.identity} joined", flush=True)
        tasks = [asyncio.ensure_future(self._speak(room, text))]
        if with_video:
            tasks.append(asyncio.ensure_future(self._video(room)))
        await asyncio.sleep(duration)
        for t in tasks:
            t.cancel()
        await room.disconnect()
        print(f"{ts()} [spk] left", flush=True)

    async def say_again(self, text):
        p16 = await asyncio.to_thread(say_16k, text, "say2")
        pcm48 = upsample3(read_pcm16(p16))
        fb = RATE // 50 * 2
        print(f"{ts()} [spk] 再说「{text}」", flush=True)
        for i in range(0, len(pcm48), fb):
            chunk = pcm48[i : i + fb]
            if len(chunk) < fb:
                chunk += b"\x00" * (fb - len(chunk))
            await room_capture(self.source, chunk, fb)
            await asyncio.sleep(0.02)
        for _ in range(75):
            await room_capture(self.source, b"\x00" * fb, fb)
            await asyncio.sleep(0.02)
        print(f"{ts()} [spk] 第二句说完", flush=True)

    async def _speak(self, room, text):
        p16 = say_16k(text, "say")
        pcm48 = upsample3(read_pcm16(p16))
        src = rtc.AudioSource(RATE, 1)
        tr = rtc.LocalAudioTrack.create_audio_track("mic", src)
        opts = rtc.TrackPublishOptions()
        opts.source = rtc.TrackSource.SOURCE_MICROPHONE
        await room.local_participant.publish_track(tr, opts)
        self.source = src
        fb = RATE // 50 * 2
        print(f"{ts()} [spk] 说「{text}」({len(pcm48)/2/RATE:.1f}s)", flush=True)
        for i in range(0, len(pcm48), fb):
            chunk = pcm48[i : i + fb]
            if len(chunk) < fb:
                chunk += b"\x00" * (fb - len(chunk))
            await src.capture_frame(rtc.AudioFrame(chunk, RATE, 1, fb // 2))
            await asyncio.sleep(0.02)
        for _ in range(75):  # 1.5s 静音让 VAD 收句
            await src.capture_frame(rtc.AudioFrame(b"\x00" * fb, RATE, 1, fb // 2))
            await asyncio.sleep(0.02)
        print(f"{ts()} [spk] 说完", flush=True)

    async def _video(self, room):
        w, h = 320, 240
        src = rtc.VideoSource(w, h)
        tr = rtc.LocalVideoTrack.create_video_track("cam", src)
        opts = rtc.TrackPublishOptions()
        opts.source = rtc.TrackSource.SOURCE_CAMERA
        await room.local_participant.publish_track(tr, opts)
        print(f"{ts()} [spk] 摄像头开", flush=True)
        while True:
            arr = np.zeros((h, w, 3), dtype=np.uint8)
            arr[:, :] = (10, 128, 245)  # RGB 近蓝
            arr[50:150, 90:230] = (245, 30, 30)  # 红条
            arr[160:215, 120:200] = (255, 255, 255)  # 白块
            src.capture_frame(rtc.VideoFrame(w, h, rtc.VideoBufferType.RGB24, arr.tobytes()))
            await asyncio.sleep(1 / 15)


async def room_capture(src, chunk, fb):
    await src.capture_frame(rtc.AudioFrame(chunk, RATE, 1, fb // 2))


async def main():
    cfg = load(sys.argv[1])
    duration = float(sys.argv[2]) if len(sys.argv) > 2 else 40.0
    with_video = "--video" in sys.argv
    observe_only = "--observe" in sys.argv
    mode = 1
    if "--phrase" in sys.argv:
        mode = int(sys.argv[sys.argv.index("--phrase") + 1])
    room_name = cfg["room"]

    obs = Observer(room_name, "observer-" + cfg.get("identity", "x")[:20])
    jobs = [asyncio.ensure_future(obs.run(duration))]
    if not observe_only:
        f1 = os.path.join(HERE, "phrase1.txt")
        t1 = open(f1, encoding="utf-8").read().strip() if os.path.exists(f1) else "你好呀，今天想聊点什么"
        first = t1
        if mode >= 2:
            # 两句都从同一个发布者说：换发布者会被 agent 当成「对方离房」直接收尾。
            first = t1 + "。"
        spk = Speaker(room_name, "speaker-" + cfg.get("identity", "x")[:20])
        jobs.append(asyncio.ensure_future(spk.run(3.0, first, with_video, duration - 4.0)))
        if mode >= 2:
            f2 = os.path.join(HERE, "phrase2.txt")
            t2 = open(f2, encoding="utf-8").read().strip() if os.path.exists(f2) else "你能看到我吗？我穿的什么颜色的衣服"
            await asyncio.sleep(24.0)
            await spk.say_again(t2)
    await asyncio.gather(*jobs)


if __name__ == "__main__":
    asyncio.run(main())
