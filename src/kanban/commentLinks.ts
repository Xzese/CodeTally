import { isSafeHttpsUrl } from './model'

export type CommentSegment = { text: string; url?: string }
export type CommentLink = { label: string; url: string }

const visibleUrl = /https:\/\/[^\s<>"'`]+/gi
const markdownLink = /\[([^\]\n]{1,160})\]\((https:\/\/[^\s)]+)\)/gi

function trimUrl(value: string): string {
  let end = value.length
  while (end > 0) {
    const last = value[end - 1]
    if (/[.,;:!?\]}]/.test(last)) { end--; continue }
    if (last === ')' && (value.slice(0, end).match(/\)/g)?.length ?? 0) > (value.slice(0, end).match(/\(/g)?.length ?? 0)) { end--; continue }
    break
  }
  return value.slice(0, end)
}

function safeUrl(value: string): string | null {
  try {
    const normalized = new URL(value).href
    return isSafeHttpsUrl(normalized) ? normalized : null
  } catch { return null }
}

export function commentSegments(text: string): CommentSegment[] {
  const segments: CommentSegment[] = []
  let cursor = 0
  for (const match of text.matchAll(visibleUrl)) {
    const display = trimUrl(match[0])
    const url = safeUrl(display)
    if (!url || match.index === undefined) continue
    if (match.index > cursor) segments.push({ text: text.slice(cursor, match.index) })
    segments.push({ text: display, url })
    cursor = match.index + display.length
  }
  if (cursor < text.length || !segments.length) segments.push({ text: text.slice(cursor) })
  return segments
}

export function namedCommentLinks(markdown: string | null | undefined, plainText: string): CommentLink[] {
  if (!markdown) return []
  const visible = new Set(commentSegments(plainText).flatMap((segment) => segment.url ? [segment.url] : []))
  const links: CommentLink[] = []
  const seen = new Set<string>()
  for (const match of markdown.matchAll(markdownLink)) {
    if (match.index === undefined || markdown[match.index - 1] === '!') continue
    const url = safeUrl(match[2])
    const label = match[1].trim()
    if (!url || !label || visible.has(url) || seen.has(url)) continue
    links.push({ label, url })
    seen.add(url)
    if (links.length === 20) break
  }
  return links
}
