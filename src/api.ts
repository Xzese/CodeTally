import { invoke } from '@tauri-apps/api/core'
import type { ActivityFeed, ActivityItem, AppSettings, DashboardData, DependencyStatus, FeedKind, FeedRequest, GitHubUser, HistoryRequest, LocSnapshot, PullRequest, Issue, RepositorySelection, SyncProgress, SyncResult } from './types'

export class BackendError extends Error {
  readonly command: string
  constructor(command: string, message: string) {
    super(message)
    this.name = 'BackendError'
    this.command = command
  }
}

async function call<T>(command: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(command, args)
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error)
    throw new BackendError(command, message)
  }
}

function readBoolean(source: Record<string, unknown>, ...keys: string[]): boolean {
  return keys.some((key) => source[key] === true || source[key] === 'true' || source[key] === 'installed' || source[key] === 'ok')
}

export async function checkDependencies(): Promise<DependencyStatus> {
  const raw = await call<unknown>('check_dependencies')
  if (typeof raw === 'boolean') return { gh: raw, git: raw, tokei: raw, gh_authenticated: raw, authenticated: raw }
  const source = (raw && typeof raw === 'object' ? raw : {}) as Record<string, unknown>
  const gh = readBoolean(source, 'gh', 'github_cli', 'githubCli', 'gh_installed')
  const git = readBoolean(source, 'git', 'git_installed')
  const tokei = readBoolean(source, 'tokei', 'tokei_installed')
  const authenticated = readBoolean(source, 'gh_authenticated', 'authenticated', 'logged_in', 'loggedIn', 'auth')
  return { gh, git, tokei, gh_authenticated: authenticated, authenticated, login: typeof source.login === 'string' ? source.login : null, error: typeof source.error === 'string' ? source.error : null }
}

export async function getGithubUser(): Promise<GitHubUser> {
  const raw = await call<unknown>('get_github_user')
  if (typeof raw === 'string') return { login: raw }
  const source = (raw ?? {}) as Record<string, unknown>
  return { login: String(source.login ?? source.username ?? ''), name: typeof source.name === 'string' ? source.name : null, avatar_url: typeof source.avatar_url === 'string' ? source.avatar_url : null }
}

export async function discoverRepositories(): Promise<unknown> {
  return call('discover_repositories')
}

export async function syncGithubData(): Promise<SyncResult> {
  return call<SyncResult>('sync_github_data')
}

export async function syncActivity(): Promise<SyncResult> {
  return call<SyncResult>('sync_activity')
}

export async function getAppSettings(): Promise<AppSettings> {
  return call<AppSettings>('get_app_settings')
}

export async function setAppSettings(settings: AppSettings): Promise<AppSettings> {
  return call<AppSettings>('set_app_settings', { settings })
}

export async function getDatabaseLocation(): Promise<string> {
  return call<string>('get_database_location')
}

export async function revealDatabase(): Promise<boolean> {
  return call<boolean>('reveal_database')
}

export async function getRepositorySelection(): Promise<RepositorySelection[]> {
  const raw = await call<unknown>('get_repository_selection')
  if (!Array.isArray(raw)) throw new Error('The repository selection command returned an invalid payload.')
  return raw.map((item) => {
    if (!item || typeof item !== 'object') throw new Error('The repository selection command returned an invalid repository.')
    const source = item as Record<string, unknown>
    const group = source.group
    if (group !== 'personal' && group !== 'company') throw new Error('The repository selection command returned an invalid repository group.')
    const githubId = source.github_id ?? source.githubId
    const nameWithOwner = source.name_with_owner ?? source.nameWithOwner
    const owner = source.owner
    if ((typeof githubId !== 'string' && typeof githubId !== 'number') || typeof nameWithOwner !== 'string' || typeof owner !== 'string') {
      throw new Error('The repository selection command returned an invalid repository.')
    }
    return { github_id: String(githubId), name_with_owner: nameWithOwner, owner, group }
  })
}

export async function getDashboard(): Promise<DashboardData> {
  return call<DashboardData>('get_dashboard')
}

export async function getLocHistory(request: HistoryRequest = {}): Promise<LocSnapshot[]> {
  const raw = await call<unknown>('get_loc_history', {
    repository_id: request.repositoryId ?? null,
    range: request.range ?? 'ALL'
  })
  if (Array.isArray(raw)) return raw as LocSnapshot[]
  if (raw && typeof raw === 'object' && Array.isArray((raw as { points?: unknown }).points)) return (raw as { points: LocSnapshot[] }).points
  throw new Error('The line history command returned an invalid payload.')
}

export async function getActivityFeed(request: FeedRequest): Promise<PullRequest[] | Issue[]> {
  const raw = await call<unknown>('get_activity_feed', {
    kind: request.kind,
    repository_id: request.repositoryId ?? null,
    state: request.state ?? 'all',
    ...(request.repositoryIds !== undefined ? { repository_ids: request.repositoryIds } : {}),
    limit: 1000
  })
  if (Array.isArray(raw)) return raw as PullRequest[] | Issue[]
  if (!raw || typeof raw !== 'object' || !Array.isArray((raw as ActivityFeed).items)) throw new Error('The activity feed command returned an invalid payload.')
  return (raw as ActivityFeed).items.map((item: ActivityItem) => {
    const { kind: _kind, ...activity } = item as ActivityItem & { kind?: string }
    return activity
  }) as PullRequest[] | Issue[]
}

export async function openExternalUrl(url: string): Promise<void> {
  await call('open_external_url', { url })
}

export async function getSyncProgress(): Promise<SyncProgress> {
  return call<SyncProgress>('get_sync_progress')
}

export async function syncRepository(repositoryId: number | string): Promise<unknown> {
  return call('sync_repository', { repo_id: repositoryId })
}

export async function backfillLoc(repositoryId: number | string): Promise<unknown> {
  return call('backfill_loc', { repo_id: repositoryId })
}
