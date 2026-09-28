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
