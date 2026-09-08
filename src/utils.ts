import type { DashboardData, DashboardMetrics, Issue, LocSnapshot, PullRequest, Repository, TimeRange } from './types'

export const value = <T>(object: Record<string, unknown> | undefined, ...keys: string[]): T | undefined => {
  if (!object) return undefined
  for (const key of keys) {
    const candidate = object[key]
    if (candidate !== undefined && candidate !== null) return candidate as T
  }
  return undefined
}

export const repositoryId = (repo: Repository): number | string => repo.id ?? repo.github_id ?? repo.name_with_owner ?? repo.name
export const repositoryLabel = (repo: Repository): string => repo.name_with_owner ?? repo.nameWithOwner ?? (repo.owner ? `${repo.owner}/${repo.name}` : repo.name)
export const repoName = (repo: Repository): string => repo.name || repositoryLabel(repo).split('/').pop() || repositoryLabel(repo)
export const repoUrl = (repo: Repository): string | undefined => repo.url

export const repoTotal = (repo: Repository): number => repo.total_loc ?? repo.totalLoc ?? 0
export const repoSource = (repo: Repository): number => repo.source_loc ?? repo.sourceLoc ?? 0
export const repoTests = (repo: Repository): number => repo.test_loc ?? repo.testLoc ?? 0
export const repoLocAvailable = (repo: Repository): boolean => repo.loc_available !== false
export const repoBaseline30Available = (repo: Repository): boolean => repo.loc_baseline_30d_available !== false
export const repoChange = (repo: Repository): number => repo.loc_30d_change ?? repo.loc_change_30d ?? repo.loc30dChange ?? 0
export const repoChangePercent = (repo: Repository): number => repo.loc_change_30d_percent ?? 0
export const repoOpenPrs = (repo: Repository): number => repo.open_prs ?? repo.openPrs ?? 0
export const repoOpenIssues = (repo: Repository): number => repo.open_issues ?? repo.openIssues ?? 0
export const repoStars = (repo: Repository): number => repo.star_count ?? repo.stars ?? 0
export const repoForks = (repo: Repository): number => repo.fork_count ?? repo.forks ?? 0
export const repoActivity = (repo: Repository): string | undefined => repo.last_activity ?? repo.recent_activity_at ?? repo.pushed_at ?? repo.pushedAt ?? repo.github_updated_at ?? repo.updatedAt ?? undefined

export const snapshotDate = (snapshot: LocSnapshot): string => snapshot.snapshot_date ?? snapshot.snapshotDate ?? snapshot.commit_date ?? snapshot.commitDate ?? ''
export const snapshotTotal = (snapshot: LocSnapshot): number => snapshot.total_loc ?? snapshot.totalLoc ?? 0
export const snapshotSource = (snapshot: LocSnapshot): number => snapshot.source_loc ?? snapshot.sourceLoc ?? 0
export const snapshotTests = (snapshot: LocSnapshot): number => snapshot.test_loc ?? snapshot.testLoc ?? 0

export const prUpdated = (pr: PullRequest): string => pr.updated_at ?? pr.updatedAt ?? pr.created_at ?? pr.createdAt ?? ''
export const issueUpdated = (issue: Issue): string => issue.updated_at ?? issue.updatedAt ?? issue.created_at ?? issue.createdAt ?? ''
export const activityRepo = (item: PullRequest | Issue): string => item.repository_name ?? item.repositoryName ?? item.repository ?? ''

export const formatCount = (number: number): string => new Intl.NumberFormat('en-US').format(Math.round(number))
export const formatCompact = (number: number): string => {
  const absolute = Math.abs(number)
  if (absolute >= 1_000_000) return `${(number / 1_000_000).toFixed(1)}m`
  if (absolute >= 1_000) return `${(number / 1_000).toFixed(1)}k`
  return `${Math.round(number)}`
}
export const formatSigned = (number: number, compact = false): string => `${number >= 0 ? '+' : ''}${compact ? formatCompact(number) : formatCount(number)}`

export const relativeTime = (date?: string | null): string => {
  if (!date) return '—'
  const time = new Date(date).getTime()
  if (Number.isNaN(time)) return '—'
  const seconds = Math.max(0, Math.round((Date.now() - time) / 1000))
  if (seconds < 60) return 'now'
  const minutes = Math.round(seconds / 60)
  if (minutes < 60) return `${minutes}m`
  const hours = Math.round(minutes / 60)
  if (hours < 24) return `${hours}h`
  const days = Math.round(hours / 24)
  if (days === 1) return 'Yesterday'
  if (days < 30) return `${days}d`
  const months = Math.round(days / 30)
  if (months < 12) return `${months}mo`
  return `${Math.round(months / 12)}y`
}

export const exactDate = (date?: string | number | null): string => {
  if (!date) return 'Unknown date'
  const parsed = new Date(date)
  if (Number.isNaN(parsed.getTime())) return String(date)
  return new Intl.DateTimeFormat(undefined, { dateStyle: 'medium', timeStyle: 'short' }).format(parsed)
}

export interface NormalizedMetrics {
  repositories: number
  total_loc: number
  source_loc: number
  test_loc: number
  loc_30d_change: number
  open_prs: number
  open_issues: number
}

export const normalizeMetrics = (data: DashboardData): NormalizedMetrics => {
  const source = data.totals ?? data.metrics ?? {}
  const repositories = data.repositories ?? data.repos ?? []
  return {
    repositories: source.repositories ?? source.repository_count ?? source.repositoryCount ?? repositories.length,
    total_loc: source.total_loc ?? source.totalLoc ?? repositories.reduce((sum, repo) => sum + repoTotal(repo), 0),
    source_loc: source.source_loc ?? source.sourceLoc ?? repositories.reduce((sum, repo) => sum + repoSource(repo), 0),
    test_loc: source.test_loc ?? source.testLoc ?? repositories.reduce((sum, repo) => sum + repoTests(repo), 0),
    loc_30d_change: source.loc_30d_change ?? source.loc30dChange ?? repositories.reduce((sum, repo) => sum + repoChange(repo), 0),
    open_prs: source.open_prs ?? source.openPrs ?? repositories.reduce((sum, repo) => sum + repoOpenPrs(repo), 0),
    open_issues: source.open_issues ?? source.openIssues ?? repositories.reduce((sum, repo) => sum + repoOpenIssues(repo), 0)
  }
}

export const normalizeDashboard = (data: DashboardData): Required<Pick<DashboardData, 'repositories' | 'loc_history' | 'pull_requests' | 'issues'>> & { metrics: NormalizedMetrics; last_sync_at?: string | null; errors: string[]; user?: DashboardData['user'] } => ({
  repositories: data.repositories ?? data.repos ?? [],
  loc_history: data.loc_history ?? data.locHistory ?? data.history ?? [],
  pull_requests: data.pull_requests ?? data.pullRequests ?? [],
  issues: data.issues ?? [],
  metrics: normalizeMetrics(data),
  last_sync_at: data.last_sync_at ?? data.lastSyncAt,
  errors: data.errors ?? [],
  user: data.user
})

export const normalizeLabels = (labels: unknown, json?: string | null): string[] => {
  if (Array.isArray(labels)) return labels.map((label) => typeof label === 'string' ? label : typeof label === 'object' && label !== null && 'name' in label ? String((label as { name: unknown }).name) : String(label))
  if (typeof json === 'string') {
    try { return normalizeLabels(JSON.parse(json)) } catch { return [] }
  }
  return []
}

export const inTimeRange = (date: string, range: TimeRange): boolean => {
  if (range === 'ALL') return true
  const now = new Date()
  const start = new Date(now)
  if (range === '3M') start.setMonth(now.getMonth() - 3)
  if (range === '1Y') start.setFullYear(now.getFullYear() - 1)
  if (range === '3Y') start.setFullYear(now.getFullYear() - 3)
  return new Date(date) >= start
}

export const chartDateLabel = (date: string): string => {
  const parsed = new Date(date)
  if (Number.isNaN(parsed.getTime())) return date
  return new Intl.DateTimeFormat(undefined, { month: 'short', year: 'numeric' }).format(parsed)
}
