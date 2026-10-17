---
layout: home

hero:
  name: Nanofile
  text: 兼容 Seafile 协议的服务端
  tagline: 单个 Rust 二进制实现的自托管文件同步与分享服务。
  actions:
    - theme: brand
      text: 快速开始
      link: /zh/guide/getting-started
    - theme: alt
      text: GitHub
      link: https://github.com/veoco/nanofile

features:
  - icon: 🔄
    title: 兼容 Seafile 的同步
    details: 官方 Seafile 桌面端与移动端客户端可直接连接，无需另装 seaf-server + seahub。
  - icon: 🖥️
    title: 网页界面
    details: 在浏览器里浏览、预览、下载、收藏，查看动态与回收站。
  - icon: 🔗
    title: 分享
    details: 分享链接支持密码、过期时间与查看次数；支持匿名上传链接。
  - icon: 🔐
    title: 登录安全
    details: 两步验证（TOTP）、受信设备、登录失败锁定，以及供第三方工具使用的 API 密钥。
  - icon: ⚙️
    title: 管理后台
    details: 用户、配额、邮件、系统设置与后台任务，全部在网页里管理。
  - icon: 🧱
    title: 受限解析
    details: 所有读取用户内容的解析器都在独立进程中运行，受内存、CPU 与时间上限约束。
---

## 从这里开始

- [快速开始](/zh/guide/getting-started) — 构建二进制并完成首次初始化。
- [配置](/zh/guide/configuration) — 配置文件、`NANOFILE_*` 变量与设置页。
- [Docker 部署](/zh/guide/docker) — 运行已发布的镜像。
- [沙盒](/zh/guide/sandbox) — 服务约束了什么、没有约束什么。
- [命令行](/zh/reference/cli) — 全部运维命令。
- [开发](/zh/dev/architecture) — crate 布局、测试与 CI。
