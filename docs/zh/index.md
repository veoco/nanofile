---
layout: home

hero:
  name: Nanofile
  text: 兼容 Seafile 协议的服务端
  tagline: 单个 Rust 二进制实现的自托管文件同步与分享服务。
  actions:
    - theme: brand
      text: 快速开始
      link: /zh/guide/start/getting-started
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

- [什么是 Nanofile？](/zh/guide/start/what-is-nanofile) —— 特性、Seafile 兼容范围与限制。
- [选择部署方式](/zh/guide/start/choose-deployment) —— 桌面托盘、无头服务器或 Docker。
- [安装与首次运行](/zh/guide/start/getting-started) —— 安装、创建第一个管理员、上传并分享文件、连接客户端。

之后可以读[使用 Nanofile](/zh/guide/use/web-interface)了解网页界面与客户端，读[部署](/zh/guide/deploy/docker)准备生产环境，或读[运维](/zh/guide/operate/backup-and-upgrade)处理备份与升级。完整目录见[指南](/zh/guide/)。

- [API 参考](/zh/reference/api/) —— 全部 HTTP 接口，以及与 Seafile 官方 API 的对比。
- [命令行](/zh/reference/cli) —— 全部运维命令。
- [开发](/zh/dev/architecture) —— crate 布局、测试与 CI。
