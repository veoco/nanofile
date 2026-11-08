---
description: 如何撰写与审校 Nanofile 文档页面。
---
# 文档

站点用 VitePress 从 `docs/` 构建。每个英文页面在 `docs/zh/` 下都有简体中文镜像；两者在同一个提交里修改。本页是指南、参考与开发页面的写作规范。

## 页面类型

- **教程** —— 面向第一次使用者的线性路径。一个结果，尽量少解释，最后有验证。
- **操作指南** —— 给已经了解产品的人做一件事。前置条件、步骤、验证、排障。
- **解释** —— 说明某物是什么、为什么这样工作。不写编号步骤。
- **参考** —— 穷举式查找，通常放在 `/reference/`。指南链接过去，不复制。

## 页面模板

```md
---
description: 一句话说明本页解决什么问题。
---
# 任务或概念

一句话说明结果。**适合谁：** … **前置条件：** …

## Before you start
## Steps
## Verify
## Troubleshoot
## Related
```

解释页把 `Steps`/`Verify` 换成概念小节，并保持标题层级稳定。

## 语气与结构

- 任务页用第二人称和祈使句；解释页用陈述句。
- 一段只讲一件事。短段落优于长段落。
- 关键警告放在 `::: warning` 或 `::: danger` 块里，不要埋在段落中间。
- 每个任务页都有 **Verify** 小节和 **Related** 列表。
- 表格用于选项与对照，不用来写多步流程。
- 目标 80–120 行；超过约 150 行就拆分。
- 一个主题只有一个所有者页面。其他页面链接过去，不重复同一段话。

## 术语

| English | 中文 |
|---|---|
| library | 资料库 |
| share link | 分享链接 |
| upload link | 上传链接 |
| WebDAV key | WebDAV 密钥 |
| API key | API 密钥 |
| sync token | 同步令牌 |
| session | 会话 |
| credential | 凭据 |
| site URL | 对外地址（`site_url`） |
| master secret | 主密钥 |
| sandbox | 沙盒 |
| full-text index | 全文索引 |
| backfill | 补扫 |
| re-index | 重新索引 |

用“资料库”，不要用“repo”，除非在引用 Seafile 路径或 API。用户可见的“任务管理”条目用“任务”（task）；引用代码时才用“作业”（job）。

## UI 文案

UI 文案以 `server/locales/en.toml` 与 `server/locales/zh.toml` 为准，不要凭记忆写。尤其是：

| English | 中文 |
|---|---|
| System Settings | 系统设置 |
| User Management | 用户管理 |
| Share Management | 分享管理 |
| Task Management | 任务管理 |
| Email Management | 邮件管理 |
| Restart server | 重启服务 |
| Sessions & Credentials | 会话与凭据 |
| Authentication | 身份验证 |
| Two-Factor Authentication | 两步验证 |

如果 UI 改了标签，在同一个改动里改 locale 文件和引用它的页面。

## 双语页面

- 英文是源语言。中文页面是完整镜像，不是摘要。
- 保持相同的标题层级和相同的页面集合；checker 会检查这一点。
- 翻译正文；代码、命令、路径和字段名保持不变。
- 使用中文标点和中文引号。中文正文里不要留 ASCII 引号。
- 中文由母语者审校；checker 判断不了语义。

## 链接与锚点

- 站内链接用绝对路径和 clean URL：`/guide/use/files-and-sharing`，不要用相对的 `.md` 路径。
- 需要稳定的跨页链接时用显式 anchor（`## Sharing policy {#sharing-policy}`）。checker 会解析显式 anchor 与标题 slug。
- 移动页面时，在 `docs/public/` 下加静态重定向，并更新所有内部链接、sidebar、指南索引与 README。
- 不要把 `http://localhost` 写成自动链接；放进反引号里，免得 VitePress 把它当成死页面。

## 图片

- 图片放 `docs/public/images/`，文件名要有描述性。
- 每张图都要 alt 文本；有信息量时再加 caption。
- UI 截图使用固定的演示数据，不能有个人信息。对比度重要时提供亮/暗两版。
- 截图是维护成本：UI 变了就更新，否则不要放。

## 审校清单

- [ ] 页面说明适合谁、假设了什么。
- [ ] 任务页有前置条件、步骤和验证。
- [ ] 概念页解释“为什么”，不只说“是什么”。
- [ ] UI 文案与 locale 文件一致。
- [ ] 两种语言的所有内部链接和锚点都能解析。
- [ ] 中文页面深度与标题结构一致。
- [ ] `npm run --prefix docs check` 与 `npm run --prefix docs build` 通过。
- [ ] 页面在 sidebar 中；关键页面还会从指南索引链接过去。

## 命令

```bash
npm ci --prefix docs
npm run --prefix docs check
npm run --prefix docs build
cargo test -p server --test api docs_api_reference
```

checker 会遍历 `docs/guide`、`docs/reference` 与 `docs/dev`，比较中英页面集合、指南标题层级、所有内部链接与锚点、sidebar 覆盖，以及 `description` frontmatter。

## 相关

- [贡献指南](/zh/dev/contributing) —— 代码与提交规则。
- [测试](/zh/dev/testing) —— Rust、前端与浏览器套件。
- [构建](/zh/dev/build) —— 工具链与前端资源。
