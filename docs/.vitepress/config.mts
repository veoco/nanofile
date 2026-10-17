import { defineConfig } from 'vitepress'

const enSidebar = {
  '/guide/': [
    {
      text: 'Guide',
      items: [
        { text: 'Getting started', link: '/guide/getting-started' },
        { text: 'Docker', link: '/guide/docker' },
        { text: 'Configuration', link: '/guide/configuration' },
        { text: 'Users and sharing', link: '/guide/users-and-sharing' },
        { text: 'Account security', link: '/guide/account-security' },
        { text: 'Email notifications', link: '/guide/email' },
        { text: 'Sandbox', link: '/guide/sandbox' },
        { text: 'Security', link: '/guide/security' },
        { text: 'Desktop tray and service', link: '/guide/desktop-and-service' },
        { text: 'Data directory', link: '/guide/data-directory' }
      ]
    }
  ],
  '/reference/': [
    {
      text: 'Reference',
      items: [{ text: 'Command line', link: '/reference/cli' }]
    }
  ],
  '/dev/': [
    {
      text: 'Development',
      items: [
        { text: 'Architecture', link: '/dev/architecture' },
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
        { text: '快速开始', link: '/zh/guide/getting-started' },
        { text: 'Docker 部署', link: '/zh/guide/docker' },
        { text: '配置', link: '/zh/guide/configuration' },
        { text: '用户与分享', link: '/zh/guide/users-and-sharing' },
        { text: '账号安全', link: '/zh/guide/account-security' },
        { text: '邮件通知', link: '/zh/guide/email' },
        { text: '沙盒', link: '/zh/guide/sandbox' },
        { text: '安全', link: '/zh/guide/security' },
        { text: '桌面托盘与服务', link: '/zh/guide/desktop-and-service' },
        { text: '数据目录', link: '/zh/guide/data-directory' }
      ]
    }
  ],
  '/zh/reference/': [
    {
      text: '参考',
      items: [{ text: '命令行', link: '/zh/reference/cli' }]
    }
  ],
  '/zh/dev/': [
    {
      text: '开发',
      items: [
        { text: '架构', link: '/zh/dev/architecture' },
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
          { text: 'Reference', link: '/reference/cli' },
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
          { text: '参考', link: '/zh/reference/cli' },
          { text: '开发', link: '/zh/dev/architecture' }
        ],
        sidebar: zhSidebar
      }
    }
  }
})
