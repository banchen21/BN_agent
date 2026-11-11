"""视觉理解客户端：把一帧 JPEG 发给多模态模型，返回中文描述。

支持三种 provider（config.json -> vision.provider）：
- magpie   ：本机 Magpie 网关（默认），OpenAI 兼容 /chat/completions，模型 group/ds-v4.1。
             token 依次取 vision.api_key -> 环境变量（默认 ANTHROPIC_AUTH_TOKEN）
             -> ~/.claude/settings.json 的 env.ANTHROPIC_AUTH_TOKEN。
- openai   ：任意 OpenAI 兼容多模态端点（vision.base_url/api_key/model）。
- bn_agent ：调用 main-app 的 /api/tools/call -> image_understand 工具。
             注意：该工具当前依赖 MiMo 视觉后端，若上游 key 失效会返回 401。

所有密钥只在运行时读取，不打印。日志里只保留模型名与耗时。
"""
from __future__ import annotations

import base64
import json
import logging
import os
import time
from pathlib import Path

import httpx

from config import VisionSettings

logger = logging.getLogger("call.vision")


class VisionError(RuntimeError):
    pass


def _read_text(path: str | Path) -> str:
    try:
        return Path(path).expanduser().read_text(encoding="utf-8").strip()
    except Exception:  # noqa: BLE001
        return ""


class VisionClient:
    def __init__(self, cfg: VisionSettings) -> None:
        self.cfg = cfg
        self._client = httpx.AsyncClient(
            timeout=httpx.Timeout(cfg.timeout, connect=5.0),
            trust_env=False,
        )
        self._token_cache: str = ""
        self.last_error: str = ""
        logger.info(
            "vision: provider=%s model=%s base_url=%s auto_look=%s",
            cfg.provider,
            cfg.model,
            cfg.base_url if cfg.provider != "bn_agent" else cfg.bn_agent_main_app_url,
            cfg.auto_look,
        )

    async def aclose(self) -> None:
        await self._client.aclose()

    # ---------------------------------------------------------------- token
    def _load_token(self) -> str:
        if self._token_cache:
            return self._token_cache
        cfg = self.cfg
        if cfg.api_key:
            self._token_cache = cfg.api_key
            return self._token_cache
        if cfg.api_key_file:
            token = _read_text(cfg.api_key_file)
            if token:
                self._token_cache = token.splitlines()[0].strip()
                return self._token_cache
        token = (os.environ.get(cfg.api_key_env) or os.environ.get("MAGPIE_AUTH_TOKEN") or "").strip()
        if token:
            self._token_cache = token
            return token
        try:
            data = json.loads(Path(cfg.claude_settings_path).expanduser().read_text(encoding="utf-8"))
            env = data.get("env") if isinstance(data, dict) else None
            if isinstance(env, dict):
                token = str(env.get("ANTHROPIC_AUTH_TOKEN") or "").strip()
        except Exception:  # noqa: BLE001
            token = ""
        self._token_cache = token
        return token

    # ---------------------------------------------------------------- 对外
    async def look_at_user(self, jpeg: bytes, question: str) -> str:
        prompt = (
            "这是通话对方摄像头的一帧画面。请用中文口语回答这个问题："
            f"{question or '你看到了什么？'}"
            "只说你真实看到的内容；看不清就说看不清，不要编造。回答控制在两句话内。"
        )
        return await self._describe(jpeg, prompt, kind="look_at_user")

    async def snapshot_view(self, jpeg: bytes) -> str:
        prompt = (
            "这是通话对方摄像头的一帧画面。请用一两句中文口语描述：画面里是谁、在做什么、"
            "周围环境有什么值得注意的。只描述真实可见的内容，不要编造。"
        )
        return await self._describe(jpeg, prompt, kind="snapshot_view")

    def should_auto_look(self, text: str) -> bool:
        """关键词兜底：本地小模型不一定按约定输出 <look> 标签，命中关键词先自动看一眼。"""
        if not self.cfg.auto_look or not text:
            return False
        return any(k in text for k in self.cfg.auto_look_keywords)

    # ---------------------------------------------------------------- 实现
    async def _describe(self, jpeg: bytes, prompt: str, *, kind: str) -> str:
        if not jpeg:
            raise VisionError("没有可用的视频帧")
        started = time.perf_counter()
        try:
            if self.cfg.provider == "bn_agent":
                text = await self._call_bn_agent(jpeg, prompt)
            else:
                text = await self._call_openai_compatible(jpeg, prompt)
        except VisionError as exc:
            self.last_error = str(exc)
            raise
        except Exception as exc:  # noqa: BLE001
            self.last_error = f"{type(exc).__name__}: {exc}"
            raise VisionError(self.last_error) from exc
        text = (text or "").strip()
        if not text:
            self.last_error = "视觉模型返回空内容"
            raise VisionError(self.last_error)
        self.last_error = ""
        logger.info(
            "vision: %s ok %.2fs chars=%d result=%r",
            kind,
            time.perf_counter() - started,
            len(text),
            text[:80],
        )
        return text

    async def _call_openai_compatible(self, jpeg: bytes, prompt: str) -> str:
        cfg = self.cfg
        token = self._load_token()
        if not token:
            raise VisionError(
                f"视觉模型 token 未配置（vision.api_key / 环境变量 {cfg.api_key_env} / "
                f"{cfg.claude_settings_path}）"
            )
        headers = {"Content-Type": "application/json"}
        header_name = cfg.auth_header or "Authorization"
        if header_name.lower() == "api-key":
            headers["api-key"] = token
        else:
            headers[header_name] = f"{cfg.auth_prefix}{token}"
        # 推理型模型会先消耗 reasoning token，内容为空时加倍 max_tokens 重试一次
        attempts = (cfg.max_tokens, min(2400, cfg.max_tokens * 2))
        last_content = ""
        for index, max_tokens in enumerate(attempts):
            body = {
                "model": cfg.model,
                "messages": [
                    {
                        "role": "user",
                        "content": [
                            {
                                "type": "image_url",
                                "image_url": {
                                    "url": "data:image/jpeg;base64,"
                                    + base64.b64encode(jpeg).decode("ascii")
                                },
                            },
                            {"type": "text", "text": prompt},
                        ],
                    }
                ],
                "max_tokens": max_tokens,
                "temperature": 0.2,
            }
            if cfg.provider == "magpie":
                body["reasoning_effort"] = "low"
            resp = await self._client.post(
                f"{cfg.base_url}/chat/completions", json=body, headers=headers
            )
            if resp.status_code >= 400:
                detail = resp.text[:300].replace("\n", " ")
                raise VisionError(f"视觉接口 HTTP {resp.status_code}: {detail}")
            data = resp.json()
            try:
                content = data["choices"][0]["message"].get("content")
            except (KeyError, IndexError, TypeError) as exc:
                raise VisionError(f"视觉接口响应格式异常: {str(data)[:200]}") from exc
            if isinstance(content, list):
                content = "\n".join(
                    str(part.get("text") or "")
                    for part in content
                    if isinstance(part, dict) and part.get("type") in ("text", "output_text")
                )
            last_content = str(content or "").strip()
            if last_content:
                return last_content
            if index + 1 < len(attempts):
                logger.info("vision: 空内容，max_tokens=%s 重试一次", attempts[index + 1])
        return last_content

    async def _call_bn_agent(self, jpeg: bytes, prompt: str) -> str:
        cfg = self.cfg
        key = _read_text(cfg.bn_agent_api_key_file).splitlines()[0].strip() if cfg.bn_agent_api_key_file else ""
        headers = {"Content-Type": "application/json"}
        if key:
            headers["X-API-Key"] = key
        body = {
            "tool_name": cfg.bn_agent_tool_name,
            "arguments": {
                "image_base64": base64.b64encode(jpeg).decode("ascii"),
                "prompt": prompt,
                "mime_type": "image/jpeg",
            },
        }
        resp = await self._client.post(
            f"{cfg.bn_agent_main_app_url}/api/tools/call", json=body, headers=headers
        )
        if resp.status_code >= 400:
            detail = resp.text[:300].replace("\n", " ")
            raise VisionError(f"main-app tool HTTP {resp.status_code}: {detail}")
        data = resp.json()
        if not data.get("success"):
            raise VisionError(f"image_understand 失败: {data.get('error') or data}")
        return str(data.get("content") or "")
