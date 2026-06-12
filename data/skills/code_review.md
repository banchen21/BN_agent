---
name: code_review
description: "对代码进行审查，指出潜在问题、安全漏洞和改进建议"
parameters:
  type: object
  properties:
    code:
      type: string
      description: "要审查的代码"
    language:
      type: string
      description: "编程语言（如 Rust、Python、JavaScript）"
  required: [code]
---
请对以下 {{language}} 代码进行审查，关注：
1. **正确性** — 是否存在逻辑错误或边界情况
2. **安全性** — 是否存在注入、内存安全等问题
3. **性能** — 是否有明显可优化的地方
4. **可维护性** — 命名、结构、注释是否清晰

```{{language}}
{{code}}
```
