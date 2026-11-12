"""从 E:\\BN_agent\\data\\yueyue 加载人设与记忆，组装通话 instructions。

只读加载，不修改数据目录。默认读取：
- persona.md（不存在时回退 CLAUDE.md）
- memory/preferences.md、memory/MEMORY.md、memory/refs.md
所有内容做长度上限截断，避免 instructions 过大。
"""
from __future__ import annotations

import logging
from pathlib import Path

logger = logging.getLogger("call.persona")

_CALL_STYLE = (
    "你现在正在和对方打语音电话。只说嘴里说出来的话：用口语、短句，像真人接电话一样自然；"
    "不要旁白、动作描写、括号说明，不要用书面语念段落。"
    "对方如果笑了、叹气或语气有变化，可以自然地呼应，但不要复述情绪标签。"
)


def _read(path: Path, limit: int) -> str:
    try:
        text = path.read_text(encoding="utf-8", errors="ignore").strip()
    except FileNotFoundError:
        return ""
    except Exception as exc:  # noqa: BLE001
        logger.warning("读取人设/记忆失败 %s: %s", path, exc)
        return ""
    if len(text) > limit:
        text = text[:limit].rstrip() + "…"
    return text


def load_instructions(data_dir: str) -> str:
    root = Path(data_dir)
    persona = _read(root / "persona.md", 4000) or _read(root / "CLAUDE.md", 4000)
    prefs = _read(root / "memory" / "preferences.md", 1500)
    memo = _read(root / "memory" / "MEMORY.md", 1500)
    refs = _read(root / "memory" / "refs.md", 800)

    parts: list[str] = []
    if persona:
        parts.append(persona)
    else:
        parts.append("你是齐悦（悦悦），正在和对方打电话。")
    parts.append(_CALL_STYLE)

    memory_bits = []
    if prefs:
        memory_bits.append("【偏好】\n" + prefs)
    if memo:
        memory_bits.append("【长期记忆】\n" + memo)
    if refs:
        memory_bits.append("【人物关系】\n" + refs)
    if memory_bits:
        parts.append("\n".join(memory_bits))

    text = "\n\n".join(parts)
    logger.info(
        "人设已加载 data_dir=%s persona=%d 字 memory=%d 字",
        root,
        len(persona),
        sum(len(x) for x in (prefs, memo, refs)),
    )
    return text


def load_greeting_hint(data_dir: str) -> str:
    """给视觉/工具提示用的简短自我描述（不参与对话主链路）。"""
    root = Path(data_dir)
    persona = _read(root / "persona.md", 600) or _read(root / "CLAUDE.md", 600)
    return persona
