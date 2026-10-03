import type { KanbanItem } from '../types'
export const COLUMNS = ['Todo', 'In progress', 'Review', 'Blocked', 'Done'] as const
export type Column = typeof COLUMNS[number]
export function columnFor(item: KanbanItem): Column {
  if (item.closed_at || item.merged_at || ['closed', 'merged'].includes(item.state.toLowerCase())) return 'Done'
  if (COLUMNS.slice(0, -1).includes(item.manual_column as Column)) return item.manual_column as Column
  return item.kind === 'issue' ? 'Todo' : item.is_draft ? 'In progress' : 'Review'
}

export function isGitHubWorkUrl(value: string): boolean {
  try {
    const url = new URL(value)
    return url.href === value && url.protocol === 'https:' && url.hostname === 'github.com' && !url.port && !url.username && !url.password && !url.search && !url.hash && /^\/[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+\/(issues|pull)\/[1-9]\d*$/.test(url.pathname)
  } catch { return false }
}

export function sourceStateLabel(item: KanbanItem): string {
  if (item.kind === 'pr' && (item.merged_at || item.state.toLowerCase() === 'merged')) return 'Merged'
  if (columnFor(item) === 'Done') {
    if (item.kind === 'pr') return 'Closed without merge'
    return item.completion_reason ? `Closed · ${item.completion_reason.replaceAll('_', ' ').toLowerCase()}` : 'Closed'
  }
  return item.kind === 'pr' && item.is_draft ? 'Draft' : 'Open'
}

export function isSafeHttpsUrl(value: string): boolean {
  try {
    const url = new URL(value)
    return url.href === value && url.protocol === 'https:' && !!url.hostname && !url.username && !url.password
  } catch { return false }
}

export function isGitHubCommentUrl(value: string, itemUrl: string): boolean {
  if (!isGitHubWorkUrl(itemUrl) || !isSafeHttpsUrl(value)) return false
  const comment = new URL(value)
  const item = new URL(itemUrl)
  return comment.origin === item.origin && comment.pathname === item.pathname && /^#issuecomment-\d+$/.test(comment.hash)
}

export function isGitHubCommitUrl(value: string, itemUrl: string): boolean {
  if (!isGitHubWorkUrl(itemUrl) || !isSafeHttpsUrl(value)) return false
  const commit = new URL(value)
  const item = new URL(itemUrl)
  const repositoryPath = item.pathname.split('/').slice(0, 3).join('/')
  return commit.origin === item.origin && !commit.search && !commit.hash && new RegExp(`^${repositoryPath.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')}/commit/[a-fA-F0-9]{7,64}$`).test(commit.pathname)
}
