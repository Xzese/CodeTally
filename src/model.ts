import type { Issue, LocSnapshot, PullRequest, Repository, TimeRange } from './types'
import { activityRepo, inTimeRange, issueUpdated, prUpdated, repoActivity, repoChange, repoForks, repoOpenIssues, repoOpenPrs, repoName, repoSource, repoStars, repoTests, repoTotal, snapshotDate, snapshotSource, snapshotTests, snapshotTotal } from './utils'

export type FeedState = 'all' | 'open' | 'merged' | 'closed'

export function isMerged(pr: PullRequest): boolean {
  return Boolean(pr.merged_at ?? pr.mergedAt) || String(pr.state).toLowerCase() === 'merged'
}

export function isDraft(pr: PullRequest): boolean {
  return Boolean(pr.is_draft ?? pr.isDraft)
}

export function filterPullRequests(items: PullRequest[], state: FeedState, repository = 'all'): PullRequest[] {
  return items
    .filter((pr) => repository === 'all' || String(pr.repository_id ?? pr.repositoryId ?? '') === repository || activityRepo(pr) === repository)
    .filter((pr) => state === 'all' || (state === 'merged' ? isMerged(pr) : state === 'open' ? !isMerged(pr) && String(pr.state).toLowerCase() === 'open' : String(pr.state).toLowerCase() === state))
    .sort((a, b) => new Date(prUpdated(b)).getTime() - new Date(prUpdated(a)).getTime())
}

export function filterIssues(items: Issue[], state: FeedState, repository = 'all'): Issue[] {
  return items
    .filter((issue) => repository === 'all' || String(issue.repository_id ?? issue.repositoryId ?? '') === repository || activityRepo(issue) === repository)
    .filter((issue) => state === 'all' || String(issue.state).toLowerCase() === state)
    .sort((a, b) => new Date(issueUpdated(b)).getTime() - new Date(issueUpdated(a)).getTime())
}

export function sortRepositories(items: Repository[], sort: 'name' | 'loc' | 'source' | 'tests' | 'growth' | 'prs' | 'issues' | 'stars' | 'forks' | 'activity', direction: 'asc' | 'desc' = 'desc'): Repository[] {
  return [...items].sort((a, b) => {
    let result: number
    if (sort === 'name') result = repoName(a).localeCompare(repoName(b))
    else if (sort === 'loc') result = repoTotal(a) - repoTotal(b)
    else if (sort === 'source') result = repoSource(a) - repoSource(b)
    else if (sort === 'tests') result = repoTests(a) - repoTests(b)
    else if (sort === 'growth') result = repoChange(a) - repoChange(b)
    else if (sort === 'prs') result = repoOpenPrs(a) - repoOpenPrs(b)
    else if (sort === 'issues') result = repoOpenIssues(a) - repoOpenIssues(b)
    else if (sort === 'stars') result = repoStars(a) - repoStars(b)
    else if (sort === 'forks') result = repoForks(a) - repoForks(b)
    else result = new Date(repoActivity(a) ?? 0).getTime() - new Date(repoActivity(b) ?? 0).getTime()
    return direction === 'asc' ? result : -result
  })
}

export interface ChartPoint {
  date: string
  timestamp: number
  total: number
  source: number
  tests: number
}

export function aggregateHistory(snapshots: LocSnapshot[], repositoryId?: number | string | null, range: TimeRange = 'ALL'): ChartPoint[] {
  const rows = new Map<string, ChartPoint>()
  snapshots
    .filter((snapshot) => repositoryId == null || String(snapshot.repository_id ?? snapshot.repositoryId ?? '') === String(repositoryId))
    .filter((snapshot) => inTimeRange(snapshotDate(snapshot), range))
    .forEach((snapshot) => {
      const date = snapshotDate(snapshot)
      if (!date) return
      const row = rows.get(date) ?? { date, timestamp: new Date(date).getTime(), total: 0, source: 0, tests: 0 }
      row.total += snapshotTotal(snapshot)
      row.source += snapshotSource(snapshot)
      row.tests += snapshotTests(snapshot)
      rows.set(date, row)
    })
  return [...rows.values()].sort((a, b) => new Date(a.date).getTime() - new Date(b.date).getTime())
}
