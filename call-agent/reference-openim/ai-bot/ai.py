"""调用本地 AI 能力：yueyue 8877（对话 / ASR / TTS）与 ComfyUI 8190（生图）。"""
from __future__ import annotations

import json
import logging
from typing import Any

import requests

from config import Settings

log = logging.getLogger("ai-bot.ai")


class AIError(RuntimeError):
    pass


class AIClient:
    def __init__(self, settings: Settings) -> None:
        self.settings = settings
        self._http = requests.Session()
        # 8877 / 8190 都只监听本机，不能被系统代理劫持
        self._http.trust_env = False

    # ---------------------------------------------------------------- 对话（SSE）
    def chat(self, text: str, *, emotion: str = "", timeout: float | None = None) -> str:
        timeout = timeout or self.settings.chat_timeout_seconds
        url = f"{self.settings.yueyue_api}/api/chat"
        body: dict[str, Any] = {"text": text}
        if emotion:
            body["emotion"] = emotion
        try:
            resp = self._http.post(url, json=body, stream=True, timeout=(10, timeout))
            resp.raise_for_status()
        except requests.RequestException as exc:
            raise AIError(f"对话服务不可用：{exc}") from exc

        parts: list[str] = []
        event = ""
        try:
            for raw in resp.iter_lines(decode_unicode=True):
                if raw is None:
                    continue
                line = raw.strip()
                if not line:
                    event = ""
                    continue
                if line.startswith("event:"):
                    event = line[6:].strip()
                    continue
                if not line.startswith("data:"):
                    continue
                payload_text = line[5:].strip()
                try:
                    payload = json.loads(payload_text) if payload_text else {}
                except json.JSONDecodeError:
                    continue
                if event == "token":
                    parts.append(str(payload.get("t", "")))
                elif event == "skip":
                    log.info("yueyue 跳过回复：%s", payload)
                elif event == "error":
                    raise AIError(f"yueyue 报错：{payload.get('t') or payload}")
                elif event == "done":
                    break
        finally:
            resp.close()

        reply = "".join(parts).strip()
        if not reply:
            raise AIError("yueyue 返回了空回复")
        return reply

    # ---------------------------------------------------------------- 生图
    def generate_image(self, prompt: str) -> bytes:
        """同步调 8190 /generate 并取回 PNG 字节。"""
        url = f"{self.settings.image_api}/generate"
        payload = {
            "prompt": prompt,
            "wait": True,
            "timeout": self.settings.image_timeout_seconds,
        }
        log.info("开始生图：%s", prompt)
        try:
            resp = self._http.post(
                url, json=payload, timeout=(10, self.settings.image_timeout_seconds + 60)
            )
            resp.raise_for_status()
            data = resp.json()
        except requests.RequestException as exc:
            raise AIError(f"生图服务不可用：{exc}") from exc
        except ValueError as exc:
            raise AIError(f"生图服务返回非 JSON：{exc}") from exc

        if data.get("status") != "ok":
            raise AIError(f"生图失败：{data.get('error') or data}")

        result = data.get("result") or {}
        filename = result.get("filename", "")
        if not filename:
            raise AIError(f"生图结果缺少 filename：{result}")
        log.info("生图完成：%s（%s 秒）", filename, result.get("elapsed_seconds"))

        try:
            img = self._http.get(
                f"{self.settings.image_api}/images/{filename}", timeout=(10, 120)
            )
            img.raise_for_status()
        except requests.RequestException as exc:
            raise AIError(f"下载生成图失败：{exc}") from exc

        if not img.content:
            raise AIError("生成图为空")
        return img.content

    # ---------------------------------------------------------------- TTS
    def tts(self, text: str) -> bytes:
        url = f"{self.settings.yueyue_api}/api/tts"
        try:
            resp = self._http.post(
                url, json={"text": text}, timeout=(10, self.settings.tts_timeout_seconds)
            )
            resp.raise_for_status()
        except requests.RequestException as exc:
            raise AIError(f"TTS 服务不可用：{exc}") from exc
        if not resp.content:
            raise AIError("TTS 返回空音频")
        return resp.content

    # ---------------------------------------------------------------- ASR
    def asr(self, wav_bytes: bytes, filename: str = "voice.wav") -> dict:
        url = f"{self.settings.yueyue_api}/api/asr"
        try:
            resp = self._http.post(
                url,
                files={"file": (filename, wav_bytes, "audio/wav")},
                timeout=(10, self.settings.chat_timeout_seconds),
            )
            resp.raise_for_status()
            return resp.json()
        except requests.RequestException as exc:
            raise AIError(f"ASR 服务不可用：{exc}") from exc
        except ValueError as exc:
            raise AIError(f"ASR 返回非 JSON：{exc}") from exc

    # ---------------------------------------------------------------- 健康检查
    def health(self) -> dict:
        result: dict[str, Any] = {}
        try:
            r = self._http.get(f"{self.settings.yueyue_api}/api/health", timeout=5)
            result["yueyue"] = r.json() if r.status_code == 200 else {"http": r.status_code}
        except Exception as exc:  # noqa: BLE001
            result["yueyue"] = {"error": str(exc)}
        try:
            r = self._http.get(f"{self.settings.image_api}/health", timeout=5)
            result["image"] = r.json() if r.status_code == 200 else {"http": r.status_code}
        except Exception as exc:  # noqa: BLE001
            result["image"] = {"error": str(exc)}
        return result
