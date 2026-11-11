"""视觉理解自测（不需要 LiveKit 通话）。

用一张真实图片（默认 data/yueyue/yueyue_self_ref.png，可 --image 指定）调用
VisionClient 的 look_at_user / snapshot_view，验证 vision.provider 配置可用。

用法：
    .venv\\Scripts\\python.exe scripts\\vision_test.py
    .venv\\Scripts\\python.exe scripts\\vision_test.py --image E:\\tmp\\test.jpg --question "穿什么颜色"
"""
from __future__ import annotations

import argparse
import asyncio
import io
import sys
from pathlib import Path

from PIL import Image

BASE_DIR = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(BASE_DIR))

for _stream in (sys.stdout, sys.stderr):
    try:
        _stream.reconfigure(encoding="utf-8", errors="replace", line_buffering=True)
    except Exception:  # noqa: BLE001
        pass

from config import SETTINGS  # noqa: E402
from vision import VisionClient, VisionError  # noqa: E402

DEFAULT_IMAGE = Path(SETTINGS.data_dir) / "yueyue_self_ref.png"


async def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--image", default=str(DEFAULT_IMAGE))
    parser.add_argument("--question", default="画面里的人长什么样、穿什么颜色的衣服？")
    args = parser.parse_args()

    path = Path(args.image)
    if not path.exists():
        print(f"图片不存在: {path}")
        return 1
    image = Image.open(path).convert("RGB")
    if image.width > 960:
        ratio = 960 / image.width
        image = image.resize((960, max(1, int(image.height * ratio))))
    buf = io.BytesIO()
    image.save(buf, format="JPEG", quality=85)
    jpeg = buf.getvalue()

    cfg = SETTINGS.vision
    print(f"provider={cfg.provider} model={cfg.model} image={path.name} {len(jpeg)//1024}KB")

    vision = VisionClient(cfg)
    code = 0
    try:
        try:
            text = await vision.look_at_user(jpeg, args.question)
            print(f"look_at_user  -> {text}")
        except VisionError as exc:
            print(f"look_at_user  FAIL: {exc}")
            code = 2
        try:
            text = await vision.snapshot_view(jpeg)
            print(f"snapshot_view -> {text}")
        except VisionError as exc:
            print(f"snapshot_view FAIL: {exc}")
            code = 2
    finally:
        await vision.aclose()
    print("PASS" if code == 0 else "FAIL")
    return code


if __name__ == "__main__":
    import os

    rc = asyncio.run(main())
    sys.stdout.flush()
    sys.stderr.flush()
    os._exit(rc)
