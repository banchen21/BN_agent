"""从聊天文本里识别「生图 / 语音」指令。"""
from __future__ import annotations

IMAGE_KEYWORDS = (
    "画",
    "绘制",
    "生成图",
    "生成一张",
    "生成一幅",
    "来张图",
    "来一张图",
    "出张图",
    "出图",
    "图片生成",
    "帮我作图",
)

VOICE_KEYWORDS = (
    "说句话",
    "说一句",
    "说句话听听",
    "语音回复",
    "语音说",
    "用语音",
    "发语音",
    "念一下",
    "读出来",
    "读一下",
    "用声音",
)

# 生图时去掉这些前缀，保留真正的画面描述
_PROMPT_PREFIXES = (
    "帮我",
    "请帮我",
    "能不能",
    "可以",
    "麻烦",
    "给我",
)


def classify(text: str) -> str:
    """返回 image / voice / text。"""
    normalized = (text or "").strip()
    if not normalized:
        return "text"
    if any(k in normalized for k in VOICE_KEYWORDS):
        return "voice"
    if any(k in normalized for k in IMAGE_KEYWORDS):
        return "image"
    return "text"


def image_prompt(text: str) -> str:
    """把「帮我画一只猫」这类指令整理成画面描述。"""
    prompt = (text or "").strip()
    changed = True
    while changed:
        changed = False
        for prefix in _PROMPT_PREFIXES:
            if prompt.startswith(prefix) and len(prompt) > len(prefix):
                prompt = prompt[len(prefix):].strip()
                changed = True
        for prefix in ("画一张", "画一幅", "画个", "画一只", "画一个", "画", "绘制"):
            if prompt.startswith(prefix) and len(prompt) > len(prefix):
                prompt = prompt[len(prefix):].strip()
                changed = True
    prompt = prompt.strip("，,。.！!～~ ")
    return prompt or (text or "").strip()
