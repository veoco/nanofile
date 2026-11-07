---
description: How to write and review a Nanofile documentation page.
---
# Documentation

The site is built with VitePress from `docs/`. Every English page has a Simplified
Chinese mirror under `docs/zh/`; both are changed in the same commit. This page is the
style guide for the guide, the reference and the developer pages.

## Page types

- **Tutorial** — a linear first-time path. One outcome, minimal explanation, a
  verification at the end.
- **How-to** — one task for somebody who already knows the product. Prerequisites,
  steps, verification, troubleshooting.
- **Explanation** — what something is and why it works that way. No numbered steps.
- **Reference** — exhaustive lookup, usually in `/reference/`. The guide links to it
  instead of copying it.

## Page template

```md
---
description: One sentence about the page.
---
# Task or concept

One-sentence outcome. **Who this is for:** … **Before you start:** …

## Before you start
## Steps
## Verify
## Troubleshoot
## Related
```

An explanation page swaps `Steps`/`Verify` for concept sections and keeps the heading
levels stable.

## Voice and structure

- Use the second person and the imperative for tasks; use the declarative for
  explanations.
- One idea per paragraph. Short paragraphs beat long ones.
- Put a critical warning in a `::: warning` or `::: danger` block, not in the middle
  of a paragraph.
- Every task page has a **Verify** section and a **Related** list.
- Tables are for options and comparisons, not for multi-step instructions.
- Aim for 80–120 lines. Split a page past about 150 lines.
- One topic has one owner page. Link to it instead of repeating the same paragraph.

## Terminology

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

Use “library”, not “repo”, except when quoting a Seafile path or API. Use “task” for
the user-facing Task Management entries; “job” only when quoting the code.

## UI labels

UI labels come from `server/locales/en.toml` and `server/locales/zh.toml`, not from
memory. In particular:

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

If a label changes in the UI, change the locale file and the pages that name it in the
same change.

## Bilingual pages

- English is the source. The Chinese page is a complete mirror, not a summary.
- Keep the same heading levels and the same set of pages. The checker enforces this.
- Translate the prose; keep code, commands, paths and field names unchanged.
- Use Chinese punctuation and Chinese quotation marks. Do not leave ASCII quotes in
  Chinese prose.
- A native speaker reviews the Chinese page; the checker cannot judge meaning.

## Links and anchors

- Site-internal links are absolute and clean: `/guide/use/files-and-sharing`, not a
  relative `.md` path.
- A stable cross-page link uses an explicit anchor (`## Sharing policy {#sharing-policy}`).
  The checker resolves explicit anchors and heading slugs.
- Moving a page means adding a static redirect under `docs/public/` and updating every
  internal link, the sidebar, the guide index and the READMEs.
- Do not link to `http://localhost` as an autolink; put it in backticks so VitePress
  does not treat it as a dead page.

## Images

- Put images in `docs/public/images/`; give them a descriptive file name.
- Every image needs alt text and, when it carries meaning, a caption.
- UI screenshots use a fixed demo dataset with no personal data. Provide light and dark
  variants when the contrast matters.
- A screenshot is a maintenance cost: update it when the UI changes, or leave it out.

## Review checklist

- [ ] The page states who it is for and what it assumes.
- [ ] A task page has prerequisites, steps and a verification.
- [ ] A concept page explains “why”, not only “what”.
- [ ] UI labels match the locale files.
- [ ] Every internal link and anchor resolves, in both languages.
- [ ] The Chinese page has the same depth and heading structure.
- [ ] `npm run --prefix docs check` and `npm run --prefix docs build` pass.
- [ ] The page is in the sidebar; a key page is also linked from the guide index.

## Commands

```bash
npm ci --prefix docs
npm run --prefix docs check
npm run --prefix docs build
cargo test -p server --test docs_api_reference_test
```

The checker walks `docs/guide`, `docs/reference` and `docs/dev`; it compares the
English and Chinese page sets, the guide heading levels, every internal link and
anchor, the sidebar coverage and the `description` frontmatter.

## Related

- [Contributing](/dev/contributing) — the code and commit rules.
- [Testing](/dev/testing) — the Rust, frontend and browser suites.
- [Building](/dev/build) — the toolchain and the frontend assets.
