---
name: summarize_text
description: "对文本进行摘要总结，提取关键信息"
parameters:
  type: object
  properties:
    text:
      type: string
      description: "要总结的文本内容"
  required: [text]
---
请对以下文本进行简明扼要的总结，提取关键信息，控制在 200 字以内：

{{text}}
