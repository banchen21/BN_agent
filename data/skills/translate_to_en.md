---
name: translate_to_en
description: "将中文文本翻译成英文"
parameters:
  type: object
  properties:
    text:
      type: string
      description: "要翻译的中文文本"
  required: [text]
---
请将以下中文文本翻译成英文，保持原意和语气：

{{text}}
