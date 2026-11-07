import { defineConfig } from 'vitepress'

const enSidebar = {
  '/guide/': [
    {
      text: 'Start here',
      items: [
        { text: 'Nanofile guide', link: '/guide/' },
        { text: 'What is Nanofile?', link: '/guide/start/what-is-nanofile' },
        { text: 'Choose a deployment', link: '/guide/start/choose-deployment' },
        { text: 'Installing and first run', link: '/guide/start/getting-started' }
      ]
    },
    {
      text: 'Using Nanofile',
      items: [
        { text: 'Web interface', link: '/guide/use/web-interface' },
        { text: 'Files and sharing', link: '/guide/use/files-and-sharing' },
        { text: 'Connecting clients', link: '/guide/use/clients' },
        { text: 'Account security', link: '/guide/use/account-security' },
        { text: 'Automatic people tags', link: '/guide/use/people-tags' },
        { text: 'Text in images and PDFs (OCR)', link: '/guide/use/ocr' }
      ]
    },
    {
      text: 'Deployment',
      items: [
        { text: 'Docker', link: '/guide/deploy/docker' },
        { text: 'Running in the background', link: '/guide/deploy/running-in-background' },
        { text: 'Windows service and desktop tray', link: '/guide/deploy/desktop-and-service' },
        { text: 'HTTPS and reverse proxy', link: '/guide/deploy/https-and-reverse-proxy' }
      ]
    },
    {
      text: 'Configuration',
      items: [
        { text: 'Configuration', link: '/guide/configure/configuration' },
        { text: 'Data directory', link: '/guide/configure/data-directory' },
        { text: 'Email notifications', link: '/guide/configure/email' }
      ]
    },
    {
      text: 'Administration and security',
      items: [
        { text: 'Users and sharing', link: '/guide/admin/users-and-sharing' },
        { text: 'Security', link: '/guide/admin/security' },
        { text: 'Sandbox', link: '/guide/admin/sandbox' }
      ]
    },
    {
      text: 'Operations',
      items: [
        { text: 'Backup, restore and upgrade', link: '/guide/operate/backup-and-upgrade' },
        { text: 'Maintenance', link: '/guide/operate/maintenance' },
        { text: 'Monitoring', link: '/guide/operate/monitoring' },
        { text: 'Troubleshooting', link: '/guide/operate/troubleshooting' }
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
        { text: 'OCR internals', link: '/dev/ocr' },
        { text: 'Frontend', link: '/dev/frontend' },
        { text: 'Testing', link: '/dev/testing' },
        { text: 'CI and releases', link: '/dev/ci-releases' },
        { text: 'Contributing', link: '/dev/contributing' },
        { text: 'Documentation', link: '/dev/documentation' }
      ]
    }
  ]
}

const zhSidebar = {
  '/zh/guide/': [
    {
      text: '上手',
      items: [
        { text: 'Nanofile 指南', link: '/zh/guide/' },
        { text: '什么是 Nanofile？', link: '/zh/guide/start/what-is-nanofile' },
        { text: '选择部署方式', link: '/zh/guide/start/choose-deployment' },
        { text: '安装与首次运行', link: '/zh/guide/start/getting-started' }
      ]
    },
    {
      text: '使用 Nanofile',
      items: [
        { text: '网页界面', link: '/zh/guide/use/web-interface' },
        { text: '文件与分享', link: '/zh/guide/use/files-and-sharing' },
        { text: '客户端接入', link: '/zh/guide/use/clients' },
        { text: '账号安全', link: '/zh/guide/use/account-security' },
        { text: '自动人物标签', link: '/zh/guide/use/people-tags' },
        { text: '图片与 PDF 文字识别（OCR）', link: '/zh/guide/use/ocr' }
      ]
    },
    {
      text: '部署',
      items: [
        { text: 'Docker 部署', link: '/zh/guide/deploy/docker' },
        { text: '后台运行与自启动', link: '/zh/guide/deploy/running-in-background' },
        { text: 'Windows 服务与桌面托盘', link: '/zh/guide/deploy/desktop-and-service' },
        { text: 'HTTPS 与反向代理', link: '/zh/guide/deploy/https-and-reverse-proxy' }
      ]
    },
    {
      text: '配置',
      items: [
        { text: '配置', link: '/zh/guide/configure/configuration' },
        { text: '数据目录', link: '/zh/guide/configure/data-directory' },
        { text: '邮件通知', link: '/zh/guide/configure/email' }
      ]
    },
    {
      text: '管理与安全',
      items: [
        { text: '用户与分享', link: '/zh/guide/admin/users-and-sharing' },
        { text: '安全', link: '/zh/guide/admin/security' },
        { text: '沙盒', link: '/zh/guide/admin/sandbox' }
      ]
    },
    {
      text: '运维',
      items: [
        { text: '备份、恢复与升级', link: '/zh/guide/operate/backup-and-upgrade' },
        { text: '维护', link: '/zh/guide/operate/maintenance' },
        { text: '监控', link: '/zh/guide/operate/monitoring' },
        { text: '故障排查', link: '/zh/guide/operate/troubleshooting' }
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
        { text: 'OCR 内部', link: '/zh/dev/ocr' },
        { text: '前端', link: '/zh/dev/frontend' },
        { text: '测试', link: '/zh/dev/testing' },
        { text: 'CI 与发布', link: '/zh/dev/ci-releases' },
        { text: '贡献指南', link: '/zh/dev/contributing' },
        { text: '文档', link: '/zh/dev/documentation' }
      ]
    }
  ]
}

const editPattern = 'https://github.com/veoco/nanofile/edit/main/docs/:path'

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
    outline: { level: [2, 3], label: 'On this page' },
    docFooter: { prev: 'Previous page', next: 'Next page' },
    lastUpdated: { text: 'Last updated' },
    darkModeSwitchLabel: 'Appearance',
    lightModeSwitchTitle: 'Switch to light theme',
    darkModeSwitchTitle: 'Switch to dark theme',
    sidebarMenuLabel: 'Menu',
    returnToTopLabel: 'Return to top',
    langMenuLabel: 'Change language',
    skipToContentLabel: 'Skip to content',
    search: {
      provider: 'local',
      options: {
        translations: {
          button: { buttonText: 'Search', buttonAriaLabel: 'Search' },
          modal: {
            displayDetails: 'Display detailed list',
            resetButtonTitle: 'Reset search',
            backButtonTitle: 'Close search',
            noResultsText: 'No results for',
            footer: {
              selectText: 'to select',
              selectKeyAriaLabel: 'enter',
              navigateText: 'to navigate',
              navigateUpKeyAriaLabel: 'up arrow',
              navigateDownKeyAriaLabel: 'down arrow',
              closeText: 'to close',
              closeKeyAriaLabel: 'escape'
            }
          }
        },
        locales: {
          zh: {
            translations: {
              button: { buttonText: '搜索', buttonAriaLabel: '搜索' },
              modal: {
                displayDetails: '显示详细列表',
                resetButtonTitle: '清除搜索',
                backButtonTitle: '关闭搜索',
                noResultsText: '未找到结果',
                footer: {
                  selectText: '选择',
                  selectKeyAriaLabel: '回车',
                  navigateText: '切换',
                  navigateUpKeyAriaLabel: '上箭头',
                  navigateDownKeyAriaLabel: '下箭头',
                  closeText: '关闭',
                  closeKeyAriaLabel: 'Esc'
                }
              }
            }
          }
        }
      }
    },
    editLink: {
      pattern: editPattern,
      text: 'Edit this page'
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
          { text: 'Guide', link: '/guide/' },
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
          { text: '指南', link: '/zh/guide/' },
          { text: '参考', link: '/zh/reference/api/' },
          { text: '开发', link: '/zh/dev/architecture' }
        ],
        sidebar: zhSidebar,
        outline: { level: [2, 3], label: '本页目录' },
        docFooter: { prev: '上一页', next: '下一页' },
        lastUpdated: { text: '最后更新' },
        darkModeSwitchLabel: '外观',
        lightModeSwitchTitle: '切换到浅色主题',
        darkModeSwitchTitle: '切换到深色主题',
        sidebarMenuLabel: '菜单',
        returnToTopLabel: '回到顶部',
        langMenuLabel: '切换语言',
        skipToContentLabel: '跳到内容',
        editLink: {
          pattern: editPattern,
          text: '在 GitHub 上编辑此页'
        },
        footer: {
          message: '基于 MIT 许可证发布。',
          copyright: '版权所有 © 2026 veoco'
        }
      }
    }
  }
})
