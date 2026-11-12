"""TTS-only 103 冒烟：不启动 8877，直接调 GSV-TTS 9880。

流程：9880 /tts/stream → 拼 WAV → MinIO → OpenIM 103 SoundElem。
用法：
    .venv\\Scripts\\python.exe scripts\\tts_only_103.py [recvID] [text]
"""
from __future__ import annotations

import array
import base64
import io
import json
import sys
import time
import wave

import requests

sys.path.insert(0, str(__import__("pathlib").Path(__file__).resolve().parents[1]))

from config import load_settings  # noqa: E402
from media import MediaStore  # noqa: E402
from openim import OpenIMClient, sound_elem  # noqa: E402

TTS_ENGINE = "http://127.0.0.1:9880"
YUEYUE_CFG = r"E:\yueyue\call\config.json"

DEFAULT_TEXT = "我在呢，刚刚说到哪里啦？"
DEFAULT_RECV = "3370971303"


def synth(text: str, cfg: dict) -> tuple[bytes, int, float]:
    payload = {
        "text": text,
        "speaker_audio": cfg["speaker_audio"],
        "prompt_audio": cfg["prompt_audio"],
        "prompt_text": cfg["prompt_text"],
        "stream_mode": "token",
        "speed": float(cfg.get("speed", 1.0)),
    }
    session = requests.Session()
    session.trust_env = False
    started = time.time()
    chunks: list[bytes] = []
    sample_rate = 32000
    with session.post(
        f"{TTS_ENGINE}/tts/stream", json=payload, stream=True, timeout=180
    ) as resp:
        resp.raise_for_status()
        for line in resp.iter_lines(decode_unicode=True):
            if not line or not line.startswith("data:"):
                continue
            evt = json.loads(line[5:].strip())
            if "audio" in evt:
                sample_rate = int(evt.get("sample_rate", sample_rate))
                chunks.append(base64.b64decode(evt["audio"]))
            elif "error" in evt:
                raise RuntimeError(evt["error"])
    elapsed = time.time() - started
    if not chunks:
        raise RuntimeError("TTS 没有返回音频")

    floats = array.array("f")
    floats.frombytes(b"".join(chunks))
    pcm16 = array.array(
        "h", (int(max(-1.0, min(1.0, value)) * 32767) for value in floats)
    )
    buf = io.BytesIO()
    with wave.open(buf, "wb") as wf:
        wf.setnchannels(1)
        wf.setsampwidth(2)
        wf.setframerate(sample_rate)
        wf.writeframes(pcm16.tobytes())
    return buf.getvalue(), sample_rate, elapsed


def main() -> None:
    recv_id = sys.argv[1] if len(sys.argv) > 1 else DEFAULT_RECV
    text = sys.argv[2] if len(sys.argv) > 2 else DEFAULT_TEXT
    cfg = json.loads(open(YUEYUE_CFG, encoding="utf-8").read())["tts"]

    wav, sample_rate, tts_seconds = synth(text, cfg)
    print(f"[tts] {tts_seconds:.2f}s 合成完成，{len(wav)} bytes，{sample_rate}Hz")

    settings = load_settings()
    store = MediaStore(settings)
    t0 = time.time()
    url, size, duration = store.upload_audio(wav)
    print(f"[minio] {time.time()-t0:.2f}s 上传完成：{url} ({size} bytes, {duration:.2f}s)")

    elem = sound_elem(url, size, duration)
    client = OpenIMClient(settings.openim_api, settings.openim_secret, settings.openim_admin_user)
    t0 = time.time()
    result = client.send_msg(settings.bot_user_id, recv_id, elem, 103)
    print(f"[openim] {time.time()-t0:.2f}s 发送完成 -> {recv_id}")
    print("[elem]", json.dumps(elem, ensure_ascii=False))
    print("[result]", json.dumps(result, ensure_ascii=False))


if __name__ == "__main__":
    main()
