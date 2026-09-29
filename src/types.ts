export type FeedKind = 'prs' | 'issues'
export type ActivityRelationship = 'everyone' | 'author' | 'assignee' | 'author_or_assignee'
export type LocMetric = 'total' | 'source' | 'tests'
export type TimeRange = '3M' | '1Y' | '3Y' | 'ALL'

export interface DependencyStatus {
  gh: boolean
  git: boolean
  tokei: boolean
  gh_authenticated: boolean
  authenticated?: boolean
  login?: string | null
  error?: string | null
}

export interface GitHubUser {
  login: string
  name?: string | null
  avatar_url?: string | null
}

export interface Repository {
  id: number | string
  github_id?: number | string
  owner?: string
  name: string
  name_with_owner?: string
  nameWithOwner?: string
  url?: string
  ssh_url?: string
  sshUrl?: string
  default_branch?: string
  defaultBranch?: string
  primary_language?: string | null
  primaryLanguage?: string | null
  is_private?: boolean
  isPrivate?: boolean
  star_count?: number
  stars?: number
  fork_count?: number
  forks?: number
  is_fork?: boolean
  isFork?: boolean
  is_archived?: boolean
  isArchived?: boolean
  created_at?: string | null
  createdAt?: string | null
  github_updated_at?: string | null
  updatedAt?: string | null
  pushed_at?: string | null
  pushedAt?: string | null
  local_path?: string | null
  last_sync_at?: string | null
  last_error?: string | null
  total_loc?: number
  totalLoc?: number
  source_loc?: number
  sourceLoc?: number
  test_loc?: number
  testLoc?: number
  loc_7d_change?: number
  loc_30d_change?: number
  loc_90d_change?: number
  loc_change_7d?: number
  loc_change_90d?: number
  loc_change_30d?: number
  loc_change_30d_percent?: number
  loc_available?: boolean
  loc_baseline_30d_available?: boolean
  loc7dChange?: number
  loc30dChange?: number
  loc90dChange?: number
  open_prs?: number
  openPrs?: number
  open_issues?: number
  openIssues?: number
  recent_activity_at?: string | null
  last_activity?: string | null
}

export interface LocSnapshot {
  id?: number | string
  repository_id?: number | string
  repositoryId?: number | string
  commit_sha?: string
  commitSha?: string
  commit_date?: string
  commitDate?: string
  snapshot_date?: string
  snapshotDate?: string
  total_loc?: number
  totalLoc?: number
  source_loc?: number
  sourceLoc?: number
  test_loc?: number
  testLoc?: number
}

export interface PullRequest {
  id?: number | string
  repository_id?: number | string
  repositoryId?: number | string
  repository?: string
  repository_name?: string
  repositoryName?: string
  number: number
  title: string
  state: string
  is_draft?: boolean
  isDraft?: boolean
  created_at?: string | null
  createdAt?: string | null
  updated_at?: string | null
  updatedAt?: string | null
  merged_at?: string | null
  mergedAt?: string | null
  closed_at?: string | null
  closedAt?: string | null
  url?: string
  additions?: number
  deletions?: number
  changed_files?: number
  changedFiles?: number
  ci_state?: string | null
  ciState?: string | null
  status_check_rollup?: unknown
  statusCheckRollup?: unknown
  author?: string | null
  assignees?: string[]
}

export interface Issue {
  id?: number | string
  repository_id?: number | string
  repositoryId?: number | string
  repository?: string
  repository_name?: string
  repositoryName?: string
  number: number
  title: string
  state: string
  created_at?: string | null
  createdAt?: string | null
  updated_at?: string | null
  updatedAt?: string | null
  closed_at?: string | null
  closedAt?: string | null
  url?: string
  labels?: string[]
  labels_json?: string | null
  labelsJson?: string | null
  author?: string | null
  assignees?: string[]
  assignees_json?: string | null
  assigneesJson?: string | null
}

export interface DashboardMetrics {
  repositories?: number
  repository_count?: number
  repositoryCount?: number
  total_loc?: number
  totalLoc?: number
  source_loc?: number
  sourceLoc?: number
  test_loc?: number
  testLoc?: number
  loc_30d_change?: number
  loc30dChange?: number
  open_prs?: number
  openPrs?: number
  open_issues?: number
  openIssues?: number
}

export interface DashboardTotals extends DashboardMetrics {
  repositories: number
  total_loc: number
  source_loc: number
  test_loc: number
  loc_change_30d: number
  open_prs: number
  open_issues: number
}

export interface DashboardData {
  last_lines_refresh_at?: string | null
  last_full_refresh_at?: string | null
  last_activity_refresh_at?: string | null
  last_personal_refresh_at?: string | null
  user?: GitHubUser | null
  repositories?: Repository[]
  repos?: Repository[]
  metrics?: DashboardMetrics
  totals?: DashboardTotals
  loc_history?: LocSnapshot[]
  locHistory?: LocSnapshot[]
  history?: LocSnapshot[]
  pull_requests?: PullRequest[]
  pullRequests?: PullRequest[]
  issues?: Issue[]
  last_sync_at?: string | null
  lastSyncAt?: string | null
  errors?: string[]
}

export interface SyncProgress {
  running: boolean
  phase: string
  current: number
  total: number
  repository_current?: number
  repository_total?: number
  snapshot_current?: number
  snapshot_total?: number
  repository_name?: string | null
  message: string
  error?: string | null
}

export interface SyncResult {
  ok: boolean
  message: string
  repositories_synced: number
  pull_requests_synced: number
  issues_synced: number
  snapshots_created: number
  errors: string[]
}

export type MenuBarMetric = 'total_lines' | 'test_lines' | 'source_lines' | 'open_prs' | 'open_issues'

export type RepositoryGroup = 'personal' | 'company'

export interface RepositorySelection {
  github_id: string
  name_with_owner: string
  owner: string
  group: RepositoryGroup
}

export type ThemeMode = 'light' | 'dark' | 'system'
export type UpdateCheckInterval = 'daily' | 'weekly' | 'monthly' | 'never'

export interface AppSettings {
  kanban_enabled: boolean
  theme_mode: ThemeMode
  activity_relationship: ActivityRelationship
  update_check_interval: UpdateCheckInterval
  run_in_background: boolean
  menu_bar_metric: MenuBarMetric
  menu_bar_metrics: MenuBarMetric[]
  menu_bar_compact_metrics: MenuBarMetric[]
  show_menu_bar: boolean
  personal_refresh_minutes: number
  activity_refresh_minutes: number
  lines_refresh_minutes: number
  refresh_lines_on_change: boolean
  include_forks_in_totals: boolean
  include_personal_repositories: boolean
  include_company_repositories: boolean
  excluded_repository_ids: string[]
}

export interface ActivityFeed {
  kind: string
  items: ActivityItem[]
}

export type ActivityItem = (PullRequest & { kind?: 'pull_request' }) | (Issue & { kind?: 'issue' })

export interface HistoryRequest {
  repositoryId?: number | string | null
  range?: TimeRange
}

export interface FeedRequest {
  kind: FeedKind
  repositoryId?: number | string | null
  repositoryIds?: number[]
  state?: string
  relationship?: ActivityRelationship
}

export type KanbanKind = 'prs' | 'issues' | 'both' | 'none'
export interface KanbanMetadata {
  item_key: string
  manual_column: string | null
  priority: string
  notes: string
  sort_rank: number
  revision: number
}
export interface KanbanItem extends KanbanMetadata {
  unavailable?: boolean
  kind: 'pr' | 'issue'
  repository_id: number
  repository: string
  number: number
  title: string
  state: string
  is_draft: boolean
  updated_at: string
  url: string
  author: string | null
  assignees: string[]
  ci_state: string | null
  closed_at: string | null
  merged_at: string | null
  completion_reason: string | null
}
export interface KanbanPage {
  items: KanbanItem[]
  total: number
  active_count: number
  completed_count: number
  last_successful_refresh?: string | null
  partial: boolean
  errors: string[]
}
export interface KanbanLinks {
  items: Array<{ item_key?: string; kind: string; repository: string; number: number; title: string; url: string; state?: string }>
  partial: boolean
  message?: string | null
}

export interface KanbanDiscussion {
  comments: Array<{ id: string; author: string | null; body_text: string; body_markdown?: string | null; created_at: string; updated_at: string; url: string }>
  comment_count: number
  comments_has_more: boolean
  checks: Array<{ id: string; name: string; status: string; details_url: string | null }>
  check_count: number
  checks_has_more: boolean
  commits: { oid: string; headline: string; committed_at: string; author: string | null; url: string }[]
  commit_count: number
  commits_has_more: boolean
  refreshed_at: string | null
  partial: boolean
  message: string | null
}

export interface KanbanPreferences {
  kind: KanbanKind
  repository_scope: string
  relationship: ActivityRelationship
  search: string
  show_completed: boolean
}
