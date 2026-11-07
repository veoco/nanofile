#!/usr/bin/env node
// Documentation guard for the VitePress site.
//
// Checks that the English and Chinese trees describe the same pages and
// headings, that every internal link and anchor resolves, and that every guide
// page is reachable from the sidebar. Run with:
//
//   node docs/scripts/check-docs.mjs
//
// No dependencies: the script must run before `npm ci --prefix docs`.

import fs from 'node:fs'
import path from 'node:path'
import { fileURLToPath } from 'node:url'

const scriptDir = path.dirname(fileURLToPath(import.meta.url))
const repoRoot = path.resolve(scriptDir, '../..')
const docsDir = path.join(repoRoot, 'docs')

const errors = []
const warnings = []

const SECTIONS = [
  { name: 'guide', en: 'guide', zh: 'zh/guide' },
  { name: 'reference', en: 'reference', zh: 'zh/reference' },
  { name: 'dev', en: 'dev', zh: 'zh/dev' }
]

function walk(dir) {
  if (!fs.existsSync(dir)) return []
  const files = []
  for (const entry of fs.readdirSync(dir, { withFileTypes: true })) {
    if (entry.name === 'node_modules' || entry.name === '.vitepress') continue
    const full = path.join(dir, entry.name)
    if (entry.isDirectory()) files.push(...walk(full))
    else if (entry.isFile() && entry.name.endsWith('.md')) files.push(full)
  }
  return files
}

function read(file) {
  return fs.readFileSync(file, 'utf8')
}

function splitFrontmatter(text) {
  if (!text.startsWith('---\n')) return { frontmatter: '', body: text }
  const end = text.indexOf('\n---\n', 4)
  if (end === -1) return { frontmatter: '', body: text }
  return { frontmatter: text.slice(4, end), body: text.slice(end + 5) }
}

function cleanHeadingText(value) {
  return value
    .replace(/\[([^\]]+)\]\([^)]+\)/g, '$1')
    .replace(/<[^>]+>/g, '')
    .replace(/[*_`]/g, '')
    .replace(/\s*\{#[^}]+\}\s*$/, '')
    .trim()
}

function slugify(value) {
  return encodeURIComponent(cleanHeadingText(value).toLowerCase().replace(/\s+/g, '-'))
}

function lenientSlug(value) {
  return cleanHeadingText(value)
    .toLowerCase()
    .replace(/[^\p{L}\p{N}\s-]/gu, '')
    .trim()
    .replace(/\s+/g, '-')
}

function parseHeadings(body) {
  const headings = []
  let fenced = false
  for (const line of body.split('\n')) {
    if (/^\s*(```|~~~)/.test(line)) {
      fenced = !fenced
      continue
    }
    if (fenced) continue
    const match = /^(#{1,6})\s+(.*)$/.exec(line)
    if (match) headings.push({ level: match[1].length, text: cleanHeadingText(match[2]) })
  }
  return headings
}

function parseAnchors(body, headings) {
  const anchors = new Set()
  for (const match of body.matchAll(/\{#([^}]+)\}/g)) anchors.add(match[1])
  for (const heading of headings) {
    const slug = slugify(heading.text)
    anchors.add(slug)
    anchors.add(decodeURIComponent(slug))
    anchors.add(slug.toLowerCase())
    anchors.add(lenientSlug(heading.text))
  }
  return anchors
}

function stripFences(body) {
  const lines = []
  let fenced = false
  for (const line of body.split('\n')) {
    if (/^\s*(```|~~~)/.test(line)) {
      fenced = !fenced
      lines.push('')
      continue
    }
    lines.push(fenced ? '' : line)
  }
  return lines.join('\n')
}

function parseLinks(body) {
  const links = []
  const text = stripFences(body)
  for (const match of text.matchAll(/\]\(([^)\s]+)(?:\s+"[^"]*")?\)/g)) links.push(match[1])
  return links
}

function fileUrl(file) {
  const rel = path.relative(docsDir, file).split(path.sep).join('/').replace(/\.md$/, '')
  return '/' + rel
}

function urlAliases(url) {
  const aliases = new Set([url])
  if (url.endsWith('/index')) aliases.add(url.slice(0, -'/index'.length) || '/')
  if (url === '/index') aliases.add('/')
  return aliases
}

function normalizeUrl(url) {
  const parsed = new URL(url, 'https://nanofile.invalid')
  let pathname = decodeURIComponent(parsed.pathname)
  pathname = pathname.replace(/\.md$/, '')
  if (pathname.length > 1 && pathname.endsWith('/')) pathname = pathname.slice(0, -1)
  return { pathname, hash: parsed.hash ? decodeURIComponent(parsed.hash.slice(1)) : '' }
}

function pageKey(file, root) {
  return fileUrl(file).replace(new RegExp(`^/${root}/`), '')
}

const pages = new Map() // url -> { file, section, language, headings, anchors }
const sectionPages = new Map()

for (const section of SECTIONS) {
  for (const language of ['en', 'zh']) {
    const root = language === 'en' ? section.en : section.zh
    const files = walk(path.join(docsDir, root))
    const list = []
    for (const file of files) {
      const text = read(file)
      const { body } = splitFrontmatter(text)
      const headings = parseHeadings(body)
      const anchors = parseAnchors(body, headings)
      const url = fileUrl(file)
      const entry = { file, section, language, url, headings, anchors, body }
      list.push(entry)
      for (const alias of urlAliases(url)) pages.set(alias, entry)
    }
    sectionPages.set(`${section.name}:${language}`, list)
  }
}

function report(location, message) {
  errors.push(`${location}: ${message}`)
}

function warn(location, message) {
  warnings.push(`${location}: ${message}`)
}

// 1. Page-set parity.
for (const section of SECTIONS) {
  const en = sectionPages.get(`${section.name}:en`) ?? []
  const zh = sectionPages.get(`${section.name}:zh`) ?? []
  const enKeys = new Set(en.map((p) => pageKey(p.file, section.en)))
  const zhKeys = new Set(zh.map((p) => pageKey(p.file, section.zh)))
  for (const key of enKeys) {
    if (!zhKeys.has(key)) report(`docs/${section.en}/${key}.md`, `missing Chinese mirror docs/${section.zh}/${key}.md`)
  }
  for (const key of zhKeys) {
    if (!enKeys.has(key)) report(`docs/${section.zh}/${key}.md`, `missing English original docs/${section.en}/${key}.md`)
  }
}

// 2. Guide heading parity and one H1 per page.
for (const language of ['en', 'zh']) {
  const list = sectionPages.get(`guide:${language}`) ?? []
  for (const page of list) {
    const h1 = page.headings.filter((h) => h.level === 1)
    if (h1.length !== 1) report(page.file, `expected exactly one level-one heading, found ${h1.length}`)
    if (page.headings[0]?.level !== 1) report(page.file, 'the first heading must be level one')
  }
}
const enGuide = sectionPages.get('guide:en') ?? []
for (const page of enGuide) {
  const key = pageKey(page.file, 'guide')
  const mirror = (sectionPages.get('guide:zh') ?? []).find((p) => pageKey(p.file, 'zh/guide') === key)
  if (!mirror) continue
  const enLevels = page.headings.map((h) => h.level).join(',')
  const zhLevels = mirror.headings.map((h) => h.level).join(',')
  if (enLevels !== zhLevels) {
    report(mirror.file, `heading levels differ from English (${enLevels} vs ${zhLevels})`)
  }
}

// 3. Internal links and anchors.
for (const section of SECTIONS) {
  for (const language of ['en', 'zh']) {
    const root = language === 'en' ? section.en : section.zh
    for (const page of sectionPages.get(`${section.name}:${language}`) ?? []) {
      const baseDir = path.posix.dirname(page.url)
      for (let href of parseLinks(page.body)) {
        if (/^(?:[a-z][a-z0-9+.-]*:|\/\/)/i.test(href)) continue
        if (/\.(?:png|jpe?g|webp|svg|gif|ico|pdf|zip|txt|css|js|json|toml)$/i.test(href.split('#')[0].split('?')[0])) continue
        if (href.startsWith('#')) {
          const anchor = decodeURIComponent(href.slice(1))
          if (anchor && !page.anchors.has(anchor) && !page.anchors.has(anchor.toLowerCase())) {
            report(page.file, `missing same-page anchor #${anchor}`)
          }
          continue
        }
        if (href.startsWith('/nanofile/')) {
          // A hand-written absolute link to this site; normalize the deployment base.
          href = href.slice('/nanofile'.length)
        }
        if (!href.startsWith('/')) {
          const resolved = new URL(href, `https://nanofile.invalid${baseDir}/`).pathname
          href = decodeURIComponent(resolved)
        }
        const { pathname, hash } = normalizeUrl(href)
        const target = pages.get(pathname)
        if (!target) {
          report(page.file, `link target does not exist: ${href}`)
          continue
        }
        if (hash) {
          if (!target.anchors.has(hash) && !target.anchors.has(hash.toLowerCase())) {
            report(page.file, `link target ${href} has no anchor #${hash}`)
          }
        }
      }
    }
  }
}

// 4. Sidebar coverage.
const configFile = path.join(docsDir, '.vitepress/config.mts')
if (!fs.existsSync(configFile)) {
  report('docs/.vitepress/config.mts', 'file is missing')
} else {
  const config = read(configFile)
  const sidebarLinks = new Set()
  for (const match of config.matchAll(/link:\s*'(\/[^']+)'/g)) sidebarLinks.add(match[1])
  for (const language of ['en', 'zh']) {
    const prefix = language === 'en' ? '/guide' : '/zh/guide'
    const inSidebar = new Set()
    for (const link of sidebarLinks) {
      if (link === prefix || link.startsWith(`${prefix}/`)) {
        const normalized = link === prefix || link.endsWith('/') ? `${link === prefix ? prefix : link}index` : link
        inSidebar.add(normalized)
      }
    }
    for (const page of sectionPages.get(`guide:${language}`) ?? []) {
      const url = page.url
      const normalized = url.endsWith('/index') ? url : url
      if (!inSidebar.has(normalized) && !inSidebar.has(url)) {
        report(page.file, `page is not listed in the ${language === 'en' ? 'English' : 'Chinese'} guide sidebar`)
      }
    }
  }
}

// 5. Description frontmatter. This becomes an error once every page is updated;
// until then it is reported as a warning so the structural guard can land first.
for (const language of ['en', 'zh']) {
  for (const page of sectionPages.get(`guide:${language}`) ?? []) {
    const frontmatter = splitFrontmatter(read(page.file)).frontmatter
    const match = /^description:\s*(.*)$/m.exec(frontmatter)
    const description = match?.[1]?.trim().replace(/^['"]|['"]$/g, '') ?? ''
    if (!description) warn(page.file, 'missing a non-empty description in frontmatter')
  }
}

// 6. Every moved page must have a static redirect in docs/public.
const MOVED = [
  ['/guide/start/web-interface', '/guide/use/web-interface'],
  ['/guide/start/clients', '/guide/use/clients'],
  ['/guide/start/people-tags', '/guide/use/people-tags'],
  ['/guide/start/ocr', '/guide/use/ocr'],
  ['/guide/admin/account-security', '/guide/use/account-security']
]
for (const [from, to] of MOVED) {
  for (const prefix of ['', '/zh']) {
    const oldUrl = `${prefix}${from}`
    const targetUrl = `${prefix}${to}`
    const target = pages.get(targetUrl)
    if (!target) {
      warn(`redirect ${oldUrl}`, `target ${targetUrl} does not exist yet`)
      continue
    }
    const redirectFile = path.join(docsDir, 'public', `${oldUrl.replace(/^\//, '')}.html`)
    if (!fs.existsSync(redirectFile)) {
      warn(oldUrl, `missing static redirect ${path.relative(repoRoot, redirectFile)}`)
      continue
    }
    const html = read(redirectFile)
    if (!html.includes(targetUrl)) warn(redirectFile, `does not point at ${targetUrl}`)
  }
}

// 7. Report.
for (const warning of warnings) console.warn(`warning: ${warning}`)
for (const error of errors) console.error(`error: ${error}`)
const summary = `${errors.length} error(s), ${warnings.length} warning(s)`
if (errors.length > 0) {
  console.error(summary)
  process.exit(1)
}
console.log(`docs check passed: ${summary}`)
