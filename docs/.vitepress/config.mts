import { defineConfig } from 'vitepress'

const enSidebar = {
  '/guide/': [
    {
      text: 'Guide',
      items: [
        { text: 'Installing and first run', link: '/guide/getting-started' },
        { text: 'Web interface', link: '/guide/web-interface' },
        { text: 'Connecting clients', link: '/guide/clients' },
        { text: 'Docker', link: '/guide/docker' },
        { text: 'Configuration', link: '/guide/configuration' },
        { text: 'Data directory', link: '/guide/data-directory' },
        { text: 'Users and sharing', link: '/guide/users-and-sharing' },
        { text: 'Account security', link: '/guide/account-security' },
        { text: 'Email notifications', link: '/guide/email' },
        { text: 'Running in the background', link: '/guide/running-in-background' },
        { text: 'Backup, restore and upgrade', link: '/guide/backup-and-upgrade' },
        { text: 'Maintenance and troubleshooting', link: '/guide/maintenance' },
        { text: 'Sandbox', link: '/guide/sandbox' },
        { text: 'Security', link: '/guide/security' },
        { text: 'Desktop tray and service', link: '/guide/desktop-and-service' }
      ]
    }
  ],
  '/reference/': [
    {
      text: 'API reference',
      items: [
        { text: 'Overview', link: '/reference/api/' },
        { text: 'Authentication', link: '/reference/api/authentication' },
        { text: 'API keys and credentials', link: '/reference/api/api-keys' },
        { text: 'Accounts and administration', link: '/reference/api/accounts' },
        { text: 'Libraries', link: '/reference/api/libraries' },
        { text: 'Directories', link: '/reference/api/directories' },
        { text: 'Files and batch operations', link: '/reference/api/files' },
        { text: 'Uploads and downloads', link: '/reference/api/transfers' },
        { text: 'History, revisions and trash', link: '/reference/api/history' },
        { text: 'Sharing', link: '/reference/api/sharing' },
        { text: 'Metadata and search', link: '/reference/api/metadata' },
        { text: 'Seafile API compatibility', link: '/reference/api/compatibility' }
      ]
    },
    {
      text: 'Protocols',
      items: [
        { text: 'Sync protocol', link: '/reference/api/sync' },
        { text: 'Notification WebSocket', link: '/reference/api/notification' },
        { text: 'WebDAV', link: '/reference/api/webdav' }
      ]
    },
    {
      text: 'Command line',
      items: [{ text: 'Command line', link: '/reference/cli' }]
    }
  ],
  '/dev/': [
    {
      text: 'Development',
      items: [
        { text: 'Architecture', link: '/dev/architecture' },
        { text: 'Building', link: '/dev/build' },
        { text: 'Seafile compatibility', link: '/dev/protocol' },
        { text: 'Sandbox internals', link: '/dev/sandbox' },
        { text: 'Frontend', link: '/dev/frontend' },
        { text: 'Testing', link: '/dev/testing' },
        { text: 'CI and releases', link: '/dev/ci-releases' },
        { text: 'Contributing', link: '/dev/contributing' }
      ]
    }
  ]
}

const zhSidebar = {
  '/zh/guide/': [
    {
      text: '指南',
      items: [
        { text: '安装与首次运行', link: '/zh/guide/getting-started' },
        { text: '网页界面', link: '/zh/guide/web-interface' },
        { text: '客户端接入', link: '/zh/guide/clients' },
        { text: 'Docker 部署', link: '/zh/guide/docker' },
        { text: '配置', link: '/zh/guide/configuration' },
        { text: '数据目录', link: '/zh/guide/data-directory' },
        { text: '用户与分享', link: '/zh/guide/users-and-sharing' },
        { text: '账号安全', link: '/zh/guide/account-security' },
        { text: '邮件通知', link: '/zh/guide/email' },
        { text: '后台运行与自启动', link: '/zh/guide/running-in-background' },
        { text: '备份、恢复与升级', link: '/zh/guide/backup-and-upgrade' },
        { text: '维护与故障排查', link: '/zh/guide/maintenance' },
        { text: '沙盒', link: '/zh/guide/sandbox' },
        { text: '安全', link: '/zh/guide/security' },
        { text: '桌面托盘与服务', link: '/zh/guide/desktop-and-service' }
      ]
    }
  ],
  '/zh/reference/': [
    {
      text: 'API 参考',
      items: [
        { text: '概述', link: '/zh/reference/api/' },
        { text: '认证', link: '/zh/reference/api/authentication' },
        { text: 'API 密钥与凭据', link: '/zh/reference/api/api-keys' },
        { text: '账号与管理', link: '/zh/reference/api/accounts' },
        { text: '资料库', link: '/zh/reference/api/libraries' },
        { text: '目录', link: '/zh/reference/api/directories' },
        { text: '文件与批量操作', link: '/zh/reference/api/files' },
        { text: '上传与下载', link: '/zh/reference/api/transfers' },
        { text: '历史、版本与回收站', link: '/zh/reference/api/history' },
        { text: '分享', link: '/zh/reference/api/sharing' },
        { text: '元数据与搜索', link: '/zh/reference/api/metadata' },
        { text: '与 Seafile API 的兼容性', link: '/zh/reference/api/compatibility' }
      ]
    },
    {
      text: '协议',
      items: [
        { text: '同步协议', link: '/zh/reference/api/sync' },
        { text: '通知 WebSocket', link: '/zh/reference/api/notification' },
        { text: 'WebDAV', link: '/zh/reference/api/webdav' }
      ]
    },
    {
      text: '命令行',
      items: [{ text: '命令行', link: '/zh/reference/cli' }]
    }
  ],
  '/zh/dev/': [
    {
      text: '开发',
      items: [
        { text: '架构', link: '/zh/dev/architecture' },
        { text: '构建', link: '/zh/dev/build' },
        { text: 'Seafile 兼容性', link: '/zh/dev/protocol' },
        { text: '沙盒内部', link: '/zh/dev/sandbox' },
        { text: '前端', link: '/zh/dev/frontend' },
        { text: '测试', link: '/zh/dev/testing' },
        { text: 'CI 与发布', link: '/zh/dev/ci-releases' },
        { text: '贡献指南', link: '/zh/dev/contributing' }
      ]
    }
  ]
}

export default defineConfig({
  title: 'Nanofile',
  description: 'A wire-compatible Seafile server written in Rust.',
  base: '/nanofile/',
  cleanUrls: true,
  lastUpdated: true,
  sitemap: {
    hostname: 'https://veoco.github.io',
    // VitePress builds sitemap URLs without the deployment base; the site lives
    // under /nanofile/, so prefix each entry (and its locale alternate).
    transformItems: (items) => {
      const withBase = (url: string) => `/nanofile/${url.replace(/^\/+/, '')}`
      return items.map((item) => ({
        ...item,
        url: withBase(item.url),
        links: item.links?.map((link) => ({
          ...link,
          url: withBase(link.url)
        }))
      }))
    }
  },
  head: [['link', { rel: 'icon', href: '/nanofile/favicon.svg' }]],
  themeConfig: {
    logo: '/favicon.svg',
    socialLinks: [
      { icon: 'github', link: 'https://github.com/veoco/nanofile' }
    ],
    search: { provider: 'local' },
    editLink: {
      pattern: 'https://github.com/veoco/nanofile/edit/main/docs/:path'
    },
    footer: {
      message: 'Released under the MIT License.',
      copyright: 'Copyright © 2026 veoco'
    }
  },
  locales: {
    root: {
      label: 'English',
      lang: 'en-US',
      themeConfig: {
        nav: [
          { text: 'Guide', link: '/guide/getting-started' },
          { text: 'Reference', link: '/reference/api/' },
          { text: 'Development', link: '/dev/architecture' }
        ],
        sidebar: enSidebar
      }
    },
    zh: {
      label: '简体中文',
      lang: 'zh-CN',
      link: '/zh/',
      themeConfig: {
        nav: [
          { text: '指南', link: '/zh/guide/getting-started' },
          { text: '参考', link: '/zh/reference/api/' },
          { text: '开发', link: '/zh/dev/architecture' }
        ],
        sidebar: zhSidebar
      }
    }
  }
})
