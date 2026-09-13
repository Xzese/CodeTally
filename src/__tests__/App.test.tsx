import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import type { AppSettings, DashboardData, DependencyStatus, Issue, LocSnapshot, MenuBarMetric, PullRequest, Repository, SyncProgress, SyncResult, ThemeMode } from '../types'

const invokeMock = vi.hoisted(() => vi.fn())
const backgroundSyncListener = vi.hoisted(() => vi.fn())
const listenMock = vi.hoisted(() => vi.fn(async (_event: string, callback: () => void) => {
  backgroundSyncListener.mockImplementation(callback)
  return vi.fn()
}))

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('@tauri-apps/api/event', () => ({ listen: listenMock }))

const repoAlpha: Repository = {
  id: 1,
  github_id: 'repo-alpha',
  owner: 'sam',
  name: 'alpha',
  name_with_owner: 'sam/alpha',
  primary_language: 'TypeScript',
  is_private: false,
  star_count: 12,
  fork_count: 3,
  url: 'https://github.com/sam/alpha',
  total_loc: 10_000,
  source_loc: 8_000,
  test_loc: 2_000,
  loc_30d_change: 1_200,
  open_prs: 2,
  open_issues: 3,
  last_activity: '2026-09-08T18:00:00.000Z'
}

const repoBeta: Repository = {
  id: 2,
  github_id: 'repo-beta',
  owner: 'sam',
  name: 'beta',
  name_with_owner: 'sam/beta',
  primary_language: 'Rust',
  is_private: true,
  star_count: 2,
  fork_count: 8,
  url: 'https://github.com/sam/beta',
  total_loc: 6_000,
  source_loc: 5_000,
  test_loc: 1_000,
  loc_30d_change: -400,
  open_prs: 0,
  open_issues: 1,
  last_activity: '2026-09-07T18:00:00.000Z'
}

const history: LocSnapshot[] = [
  { repository_id: 1, snapshot_date: '2024-01-01T00:00:00.000Z', total_loc: 7_000, source_loc: 5_500, test_loc: 1_500 },
  { repository_id: 1, snapshot_date: '2026-08-01T00:00:00.000Z', total_loc: 10_000, source_loc: 8_000, test_loc: 2_000 },
  { repository_id: 2, snapshot_date: '2024-01-01T00:00:00.000Z', total_loc: 4_500, source_loc: 3_700, test_loc: 800 },
  { repository_id: 2, snapshot_date: '2026-08-01T00:00:00.000Z', total_loc: 6_000, source_loc: 5_000, test_loc: 1_000 }
]

const prs: PullRequest[] = [
  {
    repository_id: 1,
    repository_name: 'sam/alpha',
    number: 12,
    title: 'Fix alpha flow',
    state: 'OPEN',
    updated_at: '2026-09-08T17:00:00.000Z',
    created_at: '2026-09-08T12:00:00.000Z',
    additions: 48,
    deletions: 9,
    changed_files: 3,
    url: 'https://github.com/sam/alpha/pull/12'
  },
  {
    repository_id: 2,
    repository_name: 'sam/beta',
    number: 8,
    title: 'Refactor beta tests',
    state: 'CLOSED',
    merged_at: '2026-09-07T16:00:00.000Z',
    closed_at: '2026-09-07T16:00:00.000Z',
    updated_at: '2026-09-07T16:00:00.000Z',
    created_at: '2026-09-06T12:00:00.000Z',
    additions: 120,
    deletions: 30,
    changed_files: 5,
    url: 'https://github.com/sam/beta/pull/8'
  }
]

const issues: Issue[] = [
  {
    repository_id: 1,
    repository_name: 'sam/alpha',
    number: 4,
    title: 'Alpha issue',
    state: 'OPEN',
    updated_at: '2026-09-08T15:00:00.000Z',
    created_at: '2026-09-08T10:00:00.000Z',
    labels: ['bug'],
    url: 'https://github.com/sam/alpha/issues/4'
  },
  {
    repository_id: 2,
    repository_name: 'sam/beta',
    number: 5,
    title: 'Beta issue',
    state: 'CLOSED',
    updated_at: '2026-09-06T15:00:00.000Z',
    created_at: '2026-09-05T10:00:00.000Z',
    labels: ['maintenance'],
    url: 'https://github.com/sam/beta/issues/5'
  }
]

const dashboard: DashboardData = {
  user: { login: 'sam' },
  repositories: [repoAlpha, repoBeta],
  metrics: {
    repositories: 2,
    total_loc: 16_000,
    source_loc: 13_000,
    test_loc: 3_000,
    loc_30d_change: 800,
    open_prs: 2,
    open_issues: 4
  },
  loc_history: history,
  pull_requests: prs,
  issues,
  last_sync_at: '2026-09-08T18:30:00.000Z'
}

// Keep the shared alpha/beta fixture personal for the existing dashboard tests. New
// repository-picker coverage uses this mixed portfolio so the organization grouping
// is based on the dashboard rows users actually see.
const companyGamma: Repository = {
  ...repoBeta,
  id: 3,
  github_id: 'repo-company-gamma',
  owner: 'acme',
  name: 'gamma',
  name_with_owner: 'acme/gamma',
  url: 'https://github.com/acme/gamma',
  total_loc: 4_000,
  source_loc: 3_000,
  test_loc: 1_000,
  loc_30d_change: 250
}

const mixedDashboard: DashboardData = {
  ...dashboard,
  repositories: [repoAlpha, repoBeta, companyGamma],
  metrics: {
    ...dashboard.metrics,
    repositories: 3,
    total_loc: 20_000,
    source_loc: 16_000,
    test_loc: 4_000,
    loc_30d_change: 1_050
  },
  loc_history: [...history, { repository_id: 3, snapshot_date: '2026-08-01T00:00:00.000Z', total_loc: 3_750, source_loc: 2_800, test_loc: 950 }]
}

const readyDependencies: DependencyStatus = { gh: true, git: true, tokei: true, gh_authenticated: true, authenticated: true, login: 'sam' }
const syncOk: SyncResult = { ok: true, message: 'Sync complete', repositories_synced: 2, pull_requests_synced: 2, issues_synced: 2, snapshots_created: 0, errors: [] }
const defaultAppSettings: AppSettings = {
  activity_refresh_minutes: 30,
  lines_refresh_minutes: 120,
  refresh_lines_on_change: true,
  run_in_background: true,
  menu_bar_metric: 'total_lines' as const,
  menu_bar_metrics: ['total_lines'] as MenuBarMetric[],
  show_menu_bar: true,
  theme_mode: 'system' as ThemeMode,
  include_personal_repositories: true,
  include_company_repositories: true,
  include_forks_in_totals: false,
  excluded_repository_ids: []
}

const defaultRepositorySelection = [
  { github_id: 'repo-alpha', name_with_owner: 'sam/alpha', owner: 'sam', group: 'personal' as const },
  { github_id: 'repo-beta', name_with_owner: 'acme/beta', owner: 'acme', group: 'company' as const },
  { github_id: 'repo-disabled', name_with_owner: 'sam/disabled', owner: 'sam', group: 'personal' as const }
]

function deferred<T>() {
  let resolve!: (value: T) => void
  let reject!: (reason?: unknown) => void
  const promise = new Promise<T>((resolvePromise, rejectPromise) => {
    resolve = resolvePromise
    reject = rejectPromise
  })
  return { promise, resolve, reject }
}

type BackendOptions = {
  dependencies?: DependencyStatus | DependencyStatus[]
  syncResult?: SyncResult
  cachedDashboard?: DashboardData
  dashboardResponses?: DashboardData[]
  historyPromise?: Promise<LocSnapshot[]>
  syncPromise?: Promise<SyncResult>
  activitySyncPromise?: Promise<SyncResult>
  fullSyncPromise?: Promise<SyncResult>
  progress?: SyncProgress | SyncProgress[]
  settings?: Partial<AppSettings>
  repositorySelection?: typeof defaultRepositorySelection
  appSettingsResponses?: Array<Partial<AppSettings> | Error>
  setAppSettings?: (next: Partial<AppSettings>, current: AppSettings) => AppSettings | Promise<AppSettings>
}

function configureBackend({ dependencies = readyDependencies, syncResult = syncOk, cachedDashboard = dashboard, dashboardResponses, historyPromise, syncPromise, activitySyncPromise, fullSyncPromise, progress, settings, repositorySelection = defaultRepositorySelection, appSettingsResponses, setAppSettings }: BackendOptions = {}) {
  let progressIndex = 0
  let dashboardIndex = 0
  let dependencyIndex = 0
  let appSettingsReadIndex = 0
  let appSettings = { ...defaultAppSettings, ...settings }
  const writeAppSettings = setAppSettings ?? ((next: Partial<AppSettings>, current: AppSettings) => {
    appSettings = { ...current, ...next }
    return appSettings
  })
  const dashboards = dashboardResponses?.length ? dashboardResponses : [cachedDashboard]
  invokeMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case 'check_dependencies':
        return Array.isArray(dependencies) ? dependencies[Math.min(dependencyIndex++, dependencies.length - 1)] : dependencies
      case 'get_github_user':
        return { login: 'sam' }
      case 'get_dashboard':
        return dashboards[Math.min(dashboardIndex++, dashboards.length - 1)]
      case 'discover_repositories':
        return null
      case 'get_loc_history': {
        const repositoryId = args?.repository_id
        if (repositoryId === null || repositoryId === undefined) return historyPromise ?? history
        return history.filter((snapshot) => String(snapshot.repository_id) === String(repositoryId))
      }
      case 'get_activity_feed':
        return args?.kind === 'issues' ? issues : prs
      case 'sync_github_data':
        return fullSyncPromise ?? syncPromise ?? syncResult
      case 'sync_activity':
        return activitySyncPromise ?? syncPromise ?? syncResult
      case 'get_app_settings':
        if (appSettingsResponses?.length) {
          const response = appSettingsResponses[Math.min(appSettingsReadIndex++, appSettingsResponses.length - 1)]
          if (response instanceof Error) throw response
          return response
        }
        return appSettings
      case 'get_repository_selection':
        return repositorySelection
      case 'get_database_location':
        return 'file:///Users/demo/Library/Application%20Support/com.samfaid.codetally/codetally.sqlite3'
      case 'reveal_database':
        return true
      case 'set_app_settings': {
        if (!args?.settings || typeof args.settings !== 'object') throw new Error('set_app_settings requires a settings payload')
        const next = args.settings as Partial<AppSettings>
        return writeAppSettings(next, appSettings)
      }
      case 'get_sync_progress': {
        if (Array.isArray(progress) && progress.length) return progress[Math.min(progressIndex++, progress.length - 1)]
        if (progress) return progress
        return { running: false, phase: 'idle', current: 0, total: 0, message: 'Idle' }
      }
      case 'open_external_url':
        return null
      default:
        throw new Error(`Unexpected Tauri command: ${command}`)
    }
  })
}

async function renderDashboard() {
  const user = userEvent.setup()
  render(<App />)
  await screen.findByRole('heading', { name: 'Code at a glance' })
  return user
}

function savedSettings() {
  return invokeMock.mock.calls
    .filter(([command]) => command === 'set_app_settings')
    .map(([, args]) => (args as { settings: Partial<AppSettings> }).settings)
}

let restoreMatchMedia: (() => void) | undefined

function installMatchMedia(initialMatches: boolean) {
  let matches = initialMatches
  const listeners = new Set<() => void>()
  const original = window.matchMedia
  const mediaQuery = {
    get matches() { return matches },
    media: '(prefers-color-scheme: dark)',
    onchange: null,
    addEventListener: (_event: string, listener: () => void) => listeners.add(listener),
    removeEventListener: (_event: string, listener: () => void) => listeners.delete(listener),
    addListener: (listener: () => void) => listeners.add(listener),
    removeListener: (listener: () => void) => listeners.delete(listener),
    dispatchEvent: () => true
  } as unknown as MediaQueryList
  Object.defineProperty(window, 'matchMedia', { configurable: true, writable: true, value: vi.fn(() => mediaQuery) })
  restoreMatchMedia = () => {
    if (original) Object.defineProperty(window, 'matchMedia', { configurable: true, writable: true, value: original })
    else Reflect.deleteProperty(window, 'matchMedia')
  }
  return {
    setMatches(next: boolean) {
      matches = next
      for (const listener of listeners) listener()
    },
    restore: restoreMatchMedia
  }
}

async function expandSettingsSection(user: ReturnType<typeof userEvent.setup>, content: RegExp | string, section: RegExp | string) {
  if (screen.queryByText(content, { exact: false })) return
  const toggle = screen.queryByRole('button', { name: section }) ?? screen.queryByText(section, { exact: true })
  if (toggle) await user.click(toggle)
}

beforeEach(() => {
  invokeMock.mockReset()
  backgroundSyncListener.mockReset()
  configureBackend()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
  restoreMatchMedia?.()
  restoreMatchMedia = undefined
  vi.clearAllMocks()
})

describe('dashboard UI', () => {
  it('renders summary, LOC chart, repository table, and the permanent activity sidebar', async () => {
    await renderDashboard()

    expect(screen.getByText('CodeTally')).toBeInTheDocument()
    expect(screen.getByText('16,000')).toBeInTheDocument()
    expect(screen.getByText('Portfolio overview')).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Total Lines' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Repositories' })).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Recent activity' })).toBeInTheDocument()
    expect(screen.getByText('Fix alpha flow')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /sam\/alpha #12/ })).toBeInTheDocument()
  })

  it('keeps the settings retry available when an empty dashboard opens Settings after a read failure', async () => {
    const emptyDashboard: DashboardData = {
      ...dashboard,
      repositories: [],
      metrics: { repositories: 0, total_loc: 0, source_loc: 0, test_loc: 0, loc_30d_change: 0, open_prs: 0, open_issues: 0 },
      loc_history: [],
      pull_requests: [],
      issues: [],
      last_sync_at: null
    }
    configureBackend({ cachedDashboard: emptyDashboard, appSettingsResponses: [new Error('preferences unavailable'), defaultAppSettings] })
    const user = userEvent.setup()
    render(<App />)

    expect(await screen.findByRole('heading', { name: 'No repositories selected' })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Open Settings' }))
    const drawer = await screen.findByRole('dialog', { name: 'Settings' })
    expect(within(drawer).getByText('Could not load preferences: preferences unavailable')).toBeInTheDocument()
    const retry = within(drawer).getByRole('button', { name: 'Retry settings' })
    await user.click(retry)

    await waitFor(() => expect(within(drawer).getByText('Changes save automatically')).toBeInTheDocument())
    expect(within(drawer).queryByText('Could not load preferences: preferences unavailable')).not.toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
  })

  it('applies the cached dashboard when startup history resolves after a cadence edit', async () => {
    const startupHistory = deferred<LocSnapshot[]>()
    configureBackend({ historyPromise: startupHistory.promise })
    const user = userEvent.setup()
    render(<App />)

    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('get_dashboard', undefined))
    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const drawer = await screen.findByRole('dialog', { name: 'Settings' })
    const activityRefresh = within(drawer).getByRole('radiogroup', { name: 'Activity refresh' })
    await user.click(within(activityRefresh).getByRole('radio', { name: '15 minutes' }))
    await waitFor(() => expect(savedSettings()[savedSettings().length - 1]).toEqual(expect.objectContaining({ activity_refresh_minutes: 15 })))

    startupHistory.resolve(history)
    await waitFor(() => expect(screen.getByRole('heading', { name: 'Code at a glance' })).toBeInTheDocument())
    expect(screen.getByText('alpha')).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
  })

  it('keeps the table picker local and shows collapsed personal and named organization groups', async () => {
    configureBackend({ cachedDashboard: mixedDashboard })
    const user = await renderDashboard()

    expect(screen.getAllByRole('row')).toHaveLength(4)
    await user.click(screen.getByRole('button', { name: '3 tracked' }))
    const picker = screen.getByRole('dialog', { name: 'Show repositories in table' })
    expect(within(picker).getByText('3 of 3 visible')).toBeInTheDocument()
    expect(within(picker).getByRole('button', { name: 'Expand Personal repositories' })).toBeInTheDocument()
    expect(within(picker).getByRole('button', { name: 'Expand acme repositories' })).toBeInTheDocument()
    expect(within(picker).queryByRole('checkbox', { name: 'Show sam/alpha in table' })).not.toBeInTheDocument()
    expect(within(picker).queryByRole('checkbox', { name: 'Show acme/gamma in table' })).not.toBeInTheDocument()

    await user.click(within(picker).getByRole('button', { name: 'Expand Personal repositories' }))
    const alphaVisibility = within(picker).getByRole('checkbox', { name: 'Show sam/alpha in table' })
    expect(alphaVisibility).toBeChecked()
    await user.click(alphaVisibility)

    expect(screen.queryByRole('row', { name: /alpha/ })).not.toBeInTheDocument()
    expect(screen.getAllByRole('row')).toHaveLength(3)
    expect(screen.getByRole('button', { name: '3 tracked' })).toBeInTheDocument()
    expect(within(picker).getByText('2 of 3 visible')).toBeInTheDocument()
    expect((within(picker).getByRole('checkbox', { name: 'Show Personal repositories in table' }) as HTMLInputElement).indeterminate).toBe(true)
    expect(invokeMock.mock.calls.some(([command]) => command === 'set_app_settings')).toBe(false)
  })

  it('keeps the picker available when every table row is hidden and restores all rows locally', async () => {
    configureBackend({ cachedDashboard: mixedDashboard })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: '3 tracked' }))
    const picker = screen.getByRole('dialog', { name: 'Show repositories in table' })
    await user.click(within(picker).getByRole('checkbox', { name: 'Show Personal repositories in table' }))
    await user.click(within(picker).getByRole('checkbox', { name: 'Show acme repositories in table' }))

    expect(screen.getByText('All repositories hidden from this table')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '3 tracked' })).toBeInTheDocument()
    expect(within(picker).getByText('0 of 3 visible')).toBeInTheDocument()
    expect(within(picker).getByRole('button', { name: 'Show all' })).toBeEnabled()
    expect(invokeMock.mock.calls.some(([command]) => command === 'set_app_settings')).toBe(false)

    await user.click(within(picker).getByRole('button', { name: 'Show all' }))
    expect(screen.getAllByRole('row')).toHaveLength(4)
    expect(screen.getByRole('row', { name: /alpha/ })).toBeInTheDocument()
    expect(screen.getByRole('row', { name: /gamma/ })).toBeInTheDocument()
    expect(within(picker).getByText('3 of 3 visible')).toBeInTheDocument()
  })

  it('switches between PR and issue feeds and applies state and repository filters', async () => {
    const user = await renderDashboard()
    const sidebar = () => screen.getByRole('heading', { name: 'Recent activity' }).closest('aside') as HTMLElement

    await user.click(within(sidebar()).getByRole('button', { name: 'Issues' }))
    await user.click(within(sidebar()).getByRole('button', { name: 'All' }))
    expect(screen.getByText('Alpha issue')).toBeInTheDocument()
    expect(screen.getByText('Beta issue')).toBeInTheDocument()

    await user.click(within(sidebar()).getByRole('button', { name: 'Open' }))
    expect(screen.getByText('Alpha issue')).toBeInTheDocument()
    expect(screen.queryByText('Beta issue')).not.toBeInTheDocument()

    await user.click(within(sidebar()).getByRole('button', { name: /Filter by repository:/ }))
    await user.click(screen.getByRole('button', { name: 'Expand Personal repositories' }))
    await user.click(within(screen.getByRole('dialog', { name: 'Filter activity by repository' })).getByRole('button', { name: 'beta' }))
    expect(screen.queryByText('Alpha issue')).not.toBeInTheDocument()
    expect(screen.queryByText('Beta issue')).not.toBeInTheDocument()

    await user.click(within(sidebar()).getByRole('button', { name: 'Pull requests' }))
    await user.click(within(sidebar()).getByRole('button', { name: /Filter by repository:/ }))
    await user.click(within(screen.getByRole('dialog', { name: 'Filter activity by repository' })).getByRole('button', { name: 'All repositories' }))
    await user.click(within(sidebar()).getByRole('button', { name: 'All' }))
    expect(screen.getByText('Fix alpha flow')).toBeInTheDocument()
    expect(screen.getByText('Refactor beta tests')).toBeInTheDocument()

    await user.click(within(sidebar()).getByRole('button', { name: 'Merged' }))
    expect(screen.getByText('Refactor beta tests')).toBeInTheDocument()
    expect(screen.queryByText('Fix alpha flow')).not.toBeInTheDocument()
  })

  it('groups activity scopes and sends owner repository ids while retaining the scope across tabs', async () => {
    configureBackend({ cachedDashboard: { ...dashboard, repositories: [...(dashboard.repositories ?? []), companyGamma] } })
    const user = await renderDashboard()
    await user.click(screen.getByRole('button', { name: /Filter by repository:/ }))
    const picker = screen.getByRole('dialog', { name: 'Filter activity by repository' })
    expect(within(picker).getByRole('button', { name: 'Expand Personal repositories' })).toHaveAttribute('aria-expanded', 'false')
    expect(within(picker).getByRole('button', { name: 'Expand acme' })).toHaveAttribute('aria-expanded', 'false')
    expect(within(picker).queryByRole('button', { name: 'gamma' })).not.toBeInTheDocument()
    await user.click(within(picker).getByRole('button', { name: 'Expand acme' }))
    await user.click(within(picker).getByRole('button', { name: 'All acme repositories' }))
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('get_activity_feed', { kind: 'prs', repository_id: null, repository_ids: [3], state: 'open', limit: 1000 }))
    const sidebar = screen.getByRole('heading', { name: 'Recent activity' }).closest('aside') as HTMLElement
    await user.click(within(sidebar).getByRole('button', { name: 'Issues' }))
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('get_activity_feed', { kind: 'issues', repository_id: null, repository_ids: [3], state: 'open', limit: 1000 }))
    expect(screen.getByRole('button', { name: 'Filter by repository: acme' })).toBeInTheDocument()
  })

  it('passes the selected repository id through the Tauri activity request', async () => {
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: /Filter by repository:/ }))
    await user.click(screen.getByRole('button', { name: 'Expand Personal repositories' }))
    await user.click(within(screen.getByRole('dialog', { name: 'Filter activity by repository' })).getByRole('button', { name: 'beta' }))

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('get_activity_feed', {
        kind: 'prs',
        repository_id: 2,
        state: 'open',
        limit: 1000
      })
    })
  })

  it('opens a repository detail view with repository-scoped LOC history and activity', async () => {
    const user = await renderDashboard()

    const alphaRow = screen.getByRole('row', { name: /alpha/ })
    expect(alphaRow).not.toBeNull()
    await user.click(alphaRow as HTMLElement)

    expect(await screen.findByRole('heading', { name: 'alpha' })).toBeInTheDocument()
    expect(screen.getByText('Repository detail')).toBeInTheDocument()
    const detail = within(screen.getByRole('main'))
    expect(detail.getByText('Fix alpha flow')).toBeInTheDocument()
    expect(detail.getByText('Alpha issue')).toBeInTheDocument()
    expect(detail.getByText('Public', { exact: true })).toBeInTheDocument()

    for (const [label, value] of [['Stars', '12'], ['Forks', '3']]) {
      const statLabel = detail.getByText(label, { exact: true })
      const stat = statLabel.closest('.detail-stat')
      expect(stat).not.toBeNull()
      expect(stat).toHaveTextContent(value)
    }

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('get_loc_history', { repository_id: 1, range: 'ALL' })
    })
  })

  it('keeps the detail activity sidebar scoped across feed tabs and restores its repository filter on back', async () => {
    const user = await renderDashboard()

    await user.click(screen.getByRole('row', { name: /alpha/ }))
    await screen.findByRole('heading', { name: 'alpha' })
    const sidebar = () => screen.getByRole('heading', { name: 'Recent activity' }).closest('aside') as HTMLElement

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('get_activity_feed', {
        kind: 'prs',
        repository_id: 1,
        state: 'open',
        limit: 1000
      })
    })
    expect(within(sidebar()).getByRole('button', { name: 'Filter by repository: alpha' })).toBeDisabled()
    expect(within(sidebar()).getByText('Fix alpha flow')).toBeInTheDocument()
    expect(within(sidebar()).queryByText('Refactor beta tests')).not.toBeInTheDocument()

    await user.click(within(sidebar()).getByRole('button', { name: 'Issues' }))
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('get_activity_feed', {
        kind: 'issues',
        repository_id: 1,
        state: 'open',
        limit: 1000
      })
    })
    expect(within(sidebar()).getByText('Alpha issue')).toBeInTheDocument()
    expect(within(sidebar()).queryByText('Beta issue')).not.toBeInTheDocument()
    expect(within(sidebar()).getByRole('button', { name: 'Filter by repository: alpha' })).toBeDisabled()

    await user.click(within(sidebar()).getByRole('button', { name: 'Pull requests' }))
    await waitFor(() => expect(within(sidebar()).getByText('Fix alpha flow')).toBeInTheDocument())
    expect(within(sidebar()).getByRole('button', { name: 'Filter by repository: alpha' })).toBeDisabled()

    await user.click(screen.getByRole('button', { name: 'Back to portfolio' }))
    await screen.findByRole('heading', { name: 'Code at a glance' })
    const restoredSidebar = sidebar()
    const restoredRepository = within(restoredSidebar).getByRole('button', { name: /Filter by repository:/ }).textContent?.includes('All repositories') ? 'all' : '1'
    expect(['all', '1']).toContain(restoredRepository)

    await user.click(within(restoredSidebar).getByRole('button', { name: 'All' }))
    if (restoredRepository === 'all') {
      expect(within(restoredSidebar).getByText('Fix alpha flow')).toBeInTheDocument()
      expect(within(restoredSidebar).getByText('Refactor beta tests')).toBeInTheDocument()
    } else {
      expect(within(restoredSidebar).getByText('Fix alpha flow')).toBeInTheDocument()
      expect(within(restoredSidebar).queryByText('Refactor beta tests')).not.toBeInTheDocument()
    }
  })

  it('switches the chart metric and time range controls', async () => {
    const user = await renderDashboard()
    const metricTabs = within(screen.getByRole('tablist', { name: 'Lines metric' }))

    expect(screen.getByRole('heading', { name: 'Total Lines' })).toBeInTheDocument()
    expect(metricTabs.getByRole('button', { name: 'Total' })).toHaveClass('active')

    await user.click(metricTabs.getByRole('button', { name: 'Source' }))
    expect(screen.getByRole('heading', { name: 'Source Lines' })).toBeInTheDocument()
    expect(metricTabs.getByRole('button', { name: 'Source' })).toHaveClass('active')

    await user.click(metricTabs.getByRole('button', { name: 'Tests' }))
    expect(screen.getByRole('heading', { name: 'Test Lines' })).toBeInTheDocument()
    expect(metricTabs.getByRole('button', { name: 'Tests' })).toHaveClass('active')

    await user.click(screen.getByRole('button', { name: '3M' }))
    expect(screen.getByRole('button', { name: '3M' })).toHaveClass('active')
    expect(screen.getByRole('button', { name: 'ALL' })).not.toHaveClass('active')
  })

  it('uses Lines labels and sorts repositories by source and test lines', async () => {
    const sourceAndTestSortDashboard: DashboardData = {
      ...dashboard,
      repositories: [
        { ...repoAlpha, source_loc: 1_000, test_loc: 3_000 },
        { ...repoBeta, source_loc: 9_000, test_loc: 500 }
      ]
    }
    const user = userEvent.setup()
    configureBackend({ cachedDashboard: sourceAndTestSortDashboard })
    render(<App />)
    await screen.findByRole('heading', { name: 'Code at a glance' })

    expect(screen.getAllByText('Total Lines').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Source Lines').length).toBeGreaterThan(0)
    expect(screen.getAllByText('Test Lines').length).toBeGreaterThan(0)
    expect(screen.queryByText('Total LOC')).not.toBeInTheDocument()
    expect(screen.queryByText('Source LOC')).not.toBeInTheDocument()
    expect(screen.queryByText('Test LOC')).not.toBeInTheDocument()

    const rows = () => screen.getAllByRole('row').slice(1)
    const table = screen.getByRole('table')
    expect(rows()[0]).toHaveTextContent('alpha')

    await user.click(within(table).getByRole('button', { name: 'Source' }))
    expect(rows()[0]).toHaveTextContent('beta')

    await user.click(within(table).getByRole('button', { name: 'Tests' }))
    expect(rows()[0]).toHaveTextContent('alpha')
  })

  it('shows repository visibility and sorts the Stars and Forks columns', async () => {
    const popularityDashboard: DashboardData = {
      ...dashboard,
      repositories: [
        { ...repoAlpha, star_count: 5, fork_count: 12, is_private: false },
        { ...repoBeta, star_count: 9, fork_count: 2, is_private: true }
      ]
    }
    const user = userEvent.setup()
    configureBackend({ cachedDashboard: popularityDashboard })
    render(<App />)
    await screen.findByRole('heading', { name: 'Code at a glance' })

    const rows = () => screen.getAllByRole('row').slice(1)
    expect(rows()[0]).toHaveTextContent('alpha')
    expect(screen.getByText('Public')).toBeInTheDocument()
    expect(screen.getByText('Private')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Stars' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Forks' })).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Stars' }))
    expect(rows()[0]).toHaveTextContent('beta')

    await user.click(screen.getByRole('button', { name: 'Forks' }))
    expect(rows()[0]).toHaveTextContent('alpha')
  })

  it('keeps cached repositories visible when dependency/auth checks are unavailable', async () => {
    const dependencies: DependencyStatus = { gh: false, git: false, tokei: false, gh_authenticated: false, authenticated: false, login: null }
    configureBackend({ dependencies })

    await renderDashboard()

    expect(screen.getByRole('heading', { name: 'Code at a glance' })).toBeInTheDocument()
    expect(screen.getByText('alpha')).toBeInTheDocument()
    expect(screen.queryByText('Connect your local toolkit')).not.toBeInTheDocument()
  })

  it('guides unauthenticated first runs with the gh login command and rechecks without executing auth', async () => {
    const emptyDashboard: DashboardData = {
      ...dashboard,
      repositories: [],
      metrics: { repositories: 0, total_loc: 0, source_loc: 0, test_loc: 0, loc_30d_change: 0, open_prs: 0, open_issues: 0 },
      loc_history: [],
      pull_requests: [],
      issues: [],
      last_sync_at: null
    }
    const unauthenticated: DependencyStatus = { gh: true, git: true, tokei: true, gh_authenticated: false, authenticated: false, login: null }
    configureBackend({ cachedDashboard: emptyDashboard, dependencies: [unauthenticated, readyDependencies] })
    const user = userEvent.setup()
    render(<App />)

    const loginCommand = 'gh auth login --hostname github.com --web'
    expect(await screen.findByText(loginCommand, { exact: true })).toBeInTheDocument()
    const checksBefore = invokeMock.mock.calls.filter(([command]) => command === 'check_dependencies').length
    await user.click(screen.getByRole('button', { name: 'Check connection' }))
    await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'check_dependencies').length).toBeGreaterThan(checksBefore))
    await waitFor(() => expect(screen.queryByText(loginCommand, { exact: true })).not.toBeInTheDocument())
    expect(screen.getByRole('button', { name: 'Import repositories' })).toBeInTheDocument()
    expect(invokeMock.mock.calls.some(([command]) => typeof command === 'string' && /auth.*login|login.*auth/i.test(command))).toBe(false)
  })

  it('surfaces a resolved sync failure and keeps the cached dashboard available', async () => {
    const syncResult: SyncResult = {
      ...syncOk,
      ok: false,
      message: 'Sync completed with errors',
      errors: ['alpha: GitHub request failed']
    }
    configureBackend({ syncResult })

    await renderDashboard()

    expect(await screen.findByText('Sync completed with errors: alpha: GitHub request failed')).toBeInTheDocument()
    expect(screen.getByText('alpha')).toBeInTheDocument()
  })

  it('keeps the PR sidebar populated and refetches it after sync reloads the dashboard', async () => {
    const sync = deferred<SyncResult>()
    const dashboardWithoutFeeds: DashboardData = { ...dashboard, pull_requests: [], issues: [] }
    configureBackend({ syncPromise: sync.promise, dashboardResponses: [dashboardWithoutFeeds, dashboardWithoutFeeds] })

    render(<App />)
    try {
      expect(await screen.findByText('Fix alpha flow')).toBeInTheDocument()

      const activityCalls = (kind: string) => invokeMock.mock.calls.filter(([command, args]) => command === 'get_activity_feed' && (args as Record<string, unknown> | undefined)?.kind === kind)
      expect(activityCalls('prs').length).toBeGreaterThanOrEqual(1)

      sync.resolve(syncOk)
      await waitFor(() => {
        expect(invokeMock.mock.calls.filter(([command]) => command === 'get_dashboard').length).toBeGreaterThanOrEqual(2)
      })
      await waitFor(() => expect(activityCalls('prs').length).toBeGreaterThanOrEqual(2))
      expect(screen.getByText('Fix alpha flow')).toBeInTheDocument()
    } finally {
      sync.resolve(syncOk)
    }
  })

  it('opens compact sync details by click or hover and closes them with Escape', async () => {
    const sync = deferred<SyncResult>()
    const inProgress: SyncProgress = {
      running: true,
      phase: 'backfilling',
      current: 3,
      total: 12,
      repository_current: 1,
      repository_total: 2,
      snapshot_current: 3,
      snapshot_total: 12,
      repository_name: 'sam/alpha',
      message: 'Analysing alpha snapshots'
    }
    configureBackend({ syncPromise: sync.promise, progress: inProgress })
    const user = userEvent.setup()
    render(<App />)

    try {
      await screen.findByRole('heading', { name: 'Code at a glance' })
      const trigger = await screen.findByRole('button', { name: /Sync details|Syncing|Importing/i })

      await user.click(trigger)
      expect(await screen.findByText(/Repositories 1 \/ 2/)).toBeInTheDocument()
      expect(screen.getByRole('progressbar', { name: 'Overall repository progress' })).toBeInTheDocument()

      await user.keyboard('{Escape}')
      await waitFor(() => expect(screen.queryByText(/Repositories 1 \/ 2/)).not.toBeInTheDocument())

      await user.unhover(trigger)
      await user.hover(trigger)
      expect(await screen.findByText(/Repositories 1 \/ 2/)).toBeInTheDocument()
      await user.unhover(trigger)
    } finally {
      sync.resolve(syncOk)
    }
  })

  it('uses one combined refresh control and reopens sync details after hover re-entry', async () => {
    const sync = deferred<SyncResult>()
    configureBackend({ fullSyncPromise: sync.promise })
    const user = userEvent.setup()
    render(<App />)

    try {
      await screen.findByRole('heading', { name: 'Code at a glance' })
      const syncControls = () => screen.getAllByRole('button', { name: /Refresh|Syncing|Importing/i })

      await waitFor(() => {
        expect(invokeMock.mock.calls.some(([command]) => command === 'sync_activity')).toBe(true)
        expect(syncControls()).toHaveLength(1)
        expect(syncControls()[0]).toBeEnabled()
      })
      expect(syncControls()[0]).toHaveAccessibleName(/Refresh/i)

      await user.click(syncControls()[0])
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('sync_github_data', undefined))
      await waitFor(() => {
        expect(syncControls()).toHaveLength(1)
        expect(syncControls()[0]).toHaveAccessibleName(/Syncing|Importing/i)
      })

      const activeControl = syncControls()[0]
      await user.click(activeControl)
      expect(await screen.findByRole('dialog', { name: 'Sync progress' })).toBeInTheDocument()
      await user.keyboard('{Escape}')
      await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Sync progress' })).not.toBeInTheDocument())

      await user.unhover(activeControl)
      await user.hover(activeControl)
      expect(await screen.findByRole('dialog', { name: 'Sync progress' })).toBeInTheDocument()
    } finally {
      sync.resolve(syncOk)
    }
  })

  it('returns to an idle Refresh control after completed automatic sync progress', async () => {
    const activitySync = deferred<SyncResult>()
    const fullSync = deferred<SyncResult>()
    const inProgress: SyncProgress = {
      running: true,
      phase: 'backfilling',
      current: 1,
      total: 4,
      repository_current: 1,
      repository_total: 2,
      snapshot_current: 1,
      snapshot_total: 4,
      repository_name: 'sam/alpha',
      message: 'Analysing alpha snapshots'
    }
    const completed: SyncProgress = {
      running: false,
      phase: 'complete',
      current: 4,
      total: 4,
      repository_current: 2,
      repository_total: 2,
      snapshot_current: 4,
      snapshot_total: 4,
      repository_name: 'sam/beta',
      message: 'Complete'
    }
    configureBackend({ activitySyncPromise: activitySync.promise, fullSyncPromise: fullSync.promise, progress: [inProgress, completed] })
    const user = userEvent.setup()
    render(<App />)

    try {
      await screen.findByRole('heading', { name: 'Code at a glance' })
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('sync_activity', undefined))
      await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'get_sync_progress').length).toBeGreaterThanOrEqual(1))

      activitySync.resolve(syncOk)
      const refresh = await screen.findByRole('button', { name: /Refresh GitHub data/i })
      expect(refresh).toBeEnabled()
      expect(invokeMock.mock.calls.filter(([command]) => command === 'get_sync_progress').length).toBeGreaterThanOrEqual(2)

      await user.click(refresh)
      await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('sync_github_data', undefined))
    } finally {
      activitySync.resolve(syncOk)
      fullSync.resolve(syncOk)
    }
  })

  it('uses activity-only sync on startup while manual Refresh uses full sync and frontend timers do not schedule extra syncs', async () => {
    vi.useFakeTimers()
    configureBackend()
    render(<App />)

    await act(async () => {
      for (let index = 0; index < 14; index += 1) await Promise.resolve()
    })
    expect(screen.getByRole('heading', { name: 'Code at a glance' })).toBeInTheDocument()
    expect(invokeMock.mock.calls.some(([command]) => command === 'sync_activity')).toBe(true)
    expect(invokeMock.mock.calls.some(([command]) => command === 'sync_github_data')).toBe(false)

    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: /Refresh/i }))
      for (let index = 0; index < 8; index += 1) await Promise.resolve()
    })
    expect(invokeMock.mock.calls.some(([command]) => command === 'sync_github_data')).toBe(true)

    const activityCallsBeforeSchedule = invokeMock.mock.calls.filter(([command]) => command === 'sync_activity').length
    await act(async () => {
      vi.advanceTimersByTime(2 * 60 * 1000)
      for (let index = 0; index < 12; index += 1) await Promise.resolve()
    })
    const activityCallsAfterSchedule = invokeMock.mock.calls.filter(([command]) => command === 'sync_activity').length
    expect(activityCallsAfterSchedule).toBe(activityCallsBeforeSchedule)
  })

  it('reloads cached dashboard data and activity feeds after native background sync completion', async () => {
    const activitySync = deferred<SyncResult>()
    const updatedDashboard: DashboardData = {
      ...dashboard,
      metrics: { ...dashboard.metrics, total_loc: 17_000 },
      repositories: [{ ...repoAlpha, total_loc: 11_000 }, repoBeta],
      last_sync_at: '2026-09-08T19:00:00.000Z'
    }
    configureBackend({ activitySyncPromise: activitySync.promise, dashboardResponses: [dashboard, updatedDashboard] })
    render(<App />)

    try {
      await screen.findByRole('heading', { name: 'Code at a glance' })
      await waitFor(() => expect(listenMock).toHaveBeenCalledWith('background-sync-completed', expect.any(Function)))

      const dashboardCallsBeforeEvent = invokeMock.mock.calls.filter(([command]) => command === 'get_dashboard').length
      const feedCallsBeforeEvent = invokeMock.mock.calls.filter(([command]) => command === 'get_activity_feed').length
      const syncActivityCallsBeforeEvent = invokeMock.mock.calls.filter(([command]) => command === 'sync_activity').length
      await act(async () => {
        backgroundSyncListener()
        await Promise.resolve()
      })
      await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'get_dashboard').length).toBeGreaterThan(dashboardCallsBeforeEvent))
      await waitFor(() => expect(invokeMock.mock.calls.filter(([command]) => command === 'get_activity_feed').length).toBeGreaterThan(feedCallsBeforeEvent))
      expect(screen.getByText('17,000')).toBeInTheDocument()
      expect(invokeMock.mock.calls.filter(([command]) => command === 'sync_activity').length).toBe(syncActivityCallsBeforeEvent)
    } finally {
      activitySync.resolve(syncOk)
    }
  })

  it('loads cadence settings, autosaves each updated refresh choice, and keeps Settings open', async () => {
    configureBackend({ settings: { activity_refresh_minutes: 30, lines_refresh_minutes: 120, refresh_lines_on_change: false } })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()

    const activityRefresh = screen.getByRole('radiogroup', { name: 'Activity refresh' })
    const lineRefresh = screen.getByRole('radiogroup', { name: 'Line count refresh' })
    const codeChangeRefresh = screen.getByRole('switch', { name: /Refresh line counts when code changes/i })
    await waitFor(() => {
      expect(within(activityRefresh).getByRole('radio', { name: '30 minutes' })).toBeChecked()
      expect(within(lineRefresh).getByRole('radio', { name: '120 minutes' })).toBeChecked()
      expect(codeChangeRefresh).not.toBeChecked()
    })
    for (const minutes of [15, 30, 60, 120]) {
      expect(within(activityRefresh).getByRole('radio', { name: `${minutes} minutes` })).toBeInTheDocument()
    }
    for (const minutes of [60, 120, 240, 480]) {
      expect(within(lineRefresh).getByRole('radio', { name: `${minutes} minutes` })).toBeInTheDocument()
    }

    await user.click(within(activityRefresh).getByRole('radio', { name: '15 minutes' }))
    await user.click(within(lineRefresh).getByRole('radio', { name: '240 minutes' }))
    await user.click(codeChangeRefresh)

    await waitFor(() => {
      const writes = savedSettings()
      expect(writes[writes.length - 1]).toEqual(expect.objectContaining({
        activity_refresh_minutes: 15,
        lines_refresh_minutes: 240,
        refresh_lines_on_change: true
      }))
    })
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Save settings/i })).not.toBeInTheDocument()
  })

  it('supports bounded custom refresh intervals, rejects invalid drafts, and restores saved values', async () => {
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const drawer = await screen.findByRole('dialog', { name: 'Settings' })
    const activityRefresh = within(drawer).getByRole('radiogroup', { name: 'Activity refresh' })
    const lineRefresh = within(drawer).getByRole('radiogroup', { name: 'Line count refresh' })

    await user.click(within(activityRefresh).getByRole('radio', { name: 'Custom' }))
    const activityInput = within(drawer).getByRole('textbox', { name: 'Custom activity refresh minutes' }) as HTMLInputElement
    expect(activityInput).toHaveAttribute('inputmode', 'numeric')
    expect(activityInput).toHaveValue('30')

    await user.clear(activityInput)
    fireEvent.blur(activityInput)
    expect(await within(drawer).findByRole('alert')).toHaveTextContent('Enter a whole number from 1 to 1440 minutes.')
    const writesAfterEmpty = savedSettings().length

    await user.click(activityInput)
    await user.clear(activityInput)
    await user.type(activityInput, '1441')
    fireEvent.blur(activityInput)
    expect(await within(drawer).findByRole('alert')).toHaveTextContent('Enter a whole number from 1 to 1440 minutes.')
    expect(savedSettings()).toHaveLength(writesAfterEmpty)

    await user.click(activityInput)
    await user.clear(activityInput)
    await user.type(activityInput, '12')
    fireEvent.blur(activityInput)
    await waitFor(() => expect(savedSettings()[savedSettings().length - 1]).toEqual(expect.objectContaining({ activity_refresh_minutes: 12 })))

    await user.click(within(lineRefresh).getByRole('radio', { name: 'Custom' }))
    const lineInput = within(drawer).getByRole('textbox', { name: 'Custom line count refresh minutes' }) as HTMLInputElement
    await user.clear(lineInput)
    await user.type(lineInput, '90')
    await user.keyboard('{Enter}')
    await waitFor(() => expect(savedSettings()[savedSettings().length - 1]).toEqual(expect.objectContaining({ lines_refresh_minutes: 90 })))

    await user.click(within(drawer).getByRole('button', { name: 'Close settings' }))
    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const reopened = await screen.findByRole('dialog', { name: 'Settings' })
    expect(within(within(reopened).getByRole('radiogroup', { name: 'Activity refresh' })).getByRole('radio', { name: 'Custom' })).toBeChecked()
    expect(within(reopened).getByRole('textbox', { name: 'Custom activity refresh minutes' })).toHaveValue('12')
    expect(within(reopened).getByRole('textbox', { name: 'Custom line count refresh minutes' })).toHaveValue('90')
  })

  it('serializes rapid setting changes and persists the latest intended value', async () => {
    const firstWrite = deferred<typeof defaultAppSettings>()
    const secondWrite = deferred<typeof defaultAppSettings>()
    const writes: Array<{ settings: typeof defaultAppSettings }> = []
    let persistedSettings = { ...defaultAppSettings }
    configureBackend({
      setAppSettings: (next, current) => {
        const settings = { ...current, ...next }
        writes.push({ settings })
        const pending = writes.length === 1 ? firstWrite : secondWrite
        return pending.promise.then((result) => {
          persistedSettings = { ...result }
          return result
        })
      }
    })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const activityRefresh = screen.getByRole('radiogroup', { name: 'Activity refresh' })
    await user.click(within(activityRefresh).getByRole('radio', { name: '15 minutes' }))
    await waitFor(() => expect(writes).toHaveLength(1))

    await user.click(within(activityRefresh).getByRole('radio', { name: '60 minutes' }))
    expect(within(activityRefresh).getByRole('radio', { name: '60 minutes' })).toBeChecked()
    expect(writes).toHaveLength(1)

    firstWrite.resolve(writes[0].settings)
    await waitFor(() => expect(writes).toHaveLength(2))
    expect(writes[1].settings.activity_refresh_minutes).toBe(60)

    secondWrite.resolve(writes[1].settings)
    await waitFor(() => expect(persistedSettings.activity_refresh_minutes).toBe(60))
    expect(savedSettings()[savedSettings().length - 1]).toEqual(expect.objectContaining({ activity_refresh_minutes: 60 }))
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
  })

  it('shows a failed settings write and retries the latest intent to recovery', async () => {
    const failedWrite = deferred<typeof defaultAppSettings>()
    const retryWrite = deferred<typeof defaultAppSettings>()
    const writes: Array<{ settings: typeof defaultAppSettings }> = []
    configureBackend({
      setAppSettings: (next, current) => {
        const settings = { ...current, ...next }
        writes.push({ settings })
        return (writes.length === 1 ? failedWrite : retryWrite).promise
      }
    })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const includeForks = await screen.findByRole('switch', { name: 'Include forks in line totals' })
    await user.click(includeForks)
    await waitFor(() => expect(writes).toHaveLength(1))

    failedWrite.reject(new Error('Settings write failed'))
    await waitFor(() => expect(screen.getByText('Settings write failed')).toBeInTheDocument())
    const settingsDrawer = screen.getByRole('heading', { name: 'Settings' }).closest('aside')
    expect(settingsDrawer).not.toBeNull()
    const retry = within(settingsDrawer as HTMLElement).getByRole('button', { name: /Retry/i })
    await user.click(retry)
    await waitFor(() => expect(writes).toHaveLength(2))
    expect(writes[1].settings.include_forks_in_totals).toBe(true)

    retryWrite.resolve(writes[1].settings)
    await waitFor(() => expect(screen.queryByText('Settings write failed')).not.toBeInTheDocument())
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
  })

  it('previews menu bar metrics on hover and focus without saving until selected', async () => {
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const metric = screen.getByRole('group', { name: 'Menu bar metrics' })
    const source = within(metric).getByRole('checkbox', { name: 'Source lines' })
    const preview = screen.getByLabelText('Menu bar preview')
    expect(preview).toHaveTextContent('16.0K lines')

    await user.hover(source)
    expect(preview).toHaveTextContent('13.0K source')
    expect(screen.getByText('Preview · select to add')).toBeInTheDocument()
    expect(source).not.toBeChecked()
    expect(savedSettings()).toHaveLength(0)

    await user.unhover(source)
    expect(preview).toHaveTextContent('16.0K lines')
    fireEvent.focus(source)
    expect(preview).toHaveTextContent('13.0K source')
    expect(source).not.toBeChecked()
    expect(savedSettings()).toHaveLength(0)
    fireEvent.blur(source)

    await user.click(source)
    await waitFor(() => expect(savedSettings()[savedSettings().length - 1]).toEqual(expect.objectContaining({ menu_bar_metric: 'total_lines', menu_bar_metrics: ['total_lines', 'source_lines'] })))
    expect(source).toBeChecked()
    expect(screen.getByText('Your menu bar')).toBeInTheDocument()
  })

  it('restores menu bar metric selections in canonical order and prevents removing the last metric', async () => {
    configureBackend({ settings: { menu_bar_metric: 'open_issues', menu_bar_metrics: ['open_issues', 'test_lines', 'total_lines'] } })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const drawer = await screen.findByRole('dialog', { name: 'Settings' })
    const metric = within(drawer).getByRole('group', { name: 'Menu bar metrics' })
    const checkboxes = () => within(metric).getAllByRole('checkbox') as HTMLInputElement[]
    expect(checkboxes().map((checkbox) => checkbox.getAttribute('aria-label'))).toEqual(['Total lines', 'Source lines', 'Test lines', 'Open PRs', 'Open issues'])
    expect(within(metric).getByRole('checkbox', { name: 'Total lines' })).toBeChecked()
    expect(within(metric).getByRole('checkbox', { name: 'Test lines' })).toBeChecked()
    expect(within(metric).getByRole('checkbox', { name: 'Open issues' })).toBeChecked()
    expect(within(metric).getByRole('checkbox', { name: 'Source lines' })).not.toBeChecked()
    expect(within(metric).getByRole('checkbox', { name: 'Open PRs' })).not.toBeChecked()

    await user.click(within(metric).getByRole('checkbox', { name: 'Source lines' }))
    await waitFor(() => expect(savedSettings()[savedSettings().length - 1]).toEqual(expect.objectContaining({ menu_bar_metric: 'total_lines', menu_bar_metrics: ['total_lines', 'source_lines', 'test_lines', 'open_issues'] })))
    await user.click(within(metric).getByRole('checkbox', { name: 'Open PRs' }))
    await waitFor(() => expect(savedSettings()[savedSettings().length - 1]).toEqual(expect.objectContaining({ menu_bar_metric: 'total_lines', menu_bar_metrics: ['total_lines', 'source_lines', 'test_lines', 'open_prs', 'open_issues'] })))

    await user.click(within(drawer).getByRole('button', { name: 'Close settings' }))
    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const reopened = await screen.findByRole('dialog', { name: 'Settings' })
    const restoredMetric = within(reopened).getByRole('group', { name: 'Menu bar metrics' })
    for (const label of ['Total lines', 'Source lines', 'Test lines', 'Open PRs', 'Open issues']) {
      expect(within(restoredMetric).getByRole('checkbox', { name: label })).toBeChecked()
    }

    for (const label of ['Source lines', 'Test lines', 'Total lines', 'Open PRs']) {
      const checkbox = within(restoredMetric).getByRole('checkbox', { name: label })
      await user.click(checkbox)
      await waitFor(() => expect(within(restoredMetric).getByRole('checkbox', { name: label })).not.toBeChecked())
    }
    const lastMetric = within(restoredMetric).getByRole('checkbox', { name: 'Open issues' })
    const writesBeforeLastRemoval = savedSettings().length
    await user.click(lastMetric)
    expect(lastMetric).toBeChecked()
    expect(savedSettings()).toHaveLength(writesBeforeLastRemoval)
  })

  it('uses the Appearance radiogroup, follows system changes, and removes the header theme toggle', async () => {
    const colorScheme = installMatchMedia(true)
    configureBackend({ appSettingsResponses: [{ ...defaultAppSettings, theme_mode: undefined } as Partial<AppSettings>] })
    const user = await renderDashboard()

    expect(screen.queryByTitle('Toggle theme')).not.toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const drawer = await screen.findByRole('dialog', { name: 'Settings' })
    const appearance = within(drawer).getByRole('radiogroup', { name: 'Appearance' })
    expect(within(appearance).getByRole('radio', { name: 'Light' })).toBeInTheDocument()
    expect(within(appearance).getByRole('radio', { name: 'Dark' })).toBeInTheDocument()
    const system = within(appearance).getByRole('radio', { name: 'Follow system' })
    expect(system).toBeChecked()
    expect(document.querySelector('.app-shell')).not.toHaveClass('light')

    await user.click(within(appearance).getByRole('radio', { name: 'Light' }))
    expect(document.querySelector('.app-shell')).toHaveClass('light')
    await user.click(within(appearance).getByRole('radio', { name: 'Dark' }))
    expect(document.querySelector('.app-shell')).not.toHaveClass('light')
    await user.click(system)
    expect(document.querySelector('.app-shell')).not.toHaveClass('light')

    // Changing the installed media-query result exercises the same `change`
    // listener used by macOS appearance events.
    act(() => colorScheme.setMatches(false))
    await waitFor(() => expect(document.querySelector('.app-shell')).toHaveClass('light'))
    act(() => colorScheme.setMatches(true))
    await waitFor(() => expect(document.querySelector('.app-shell')).not.toHaveClass('light'))
  })

  it('shows the local database location and GitHub CLI commands in Settings', async () => {
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()

    await expandSettingsSection(user, /codetally\.sqlite3/i, /Data storage/i)
    const databaseLocation = await screen.findByRole('link', { name: /codetally\.sqlite3/i })
    expect(databaseLocation).toHaveAttribute('href', 'file:///Users/demo/Library/Application%20Support/com.samfaid.codetally/codetally.sqlite3')
    await user.click(databaseLocation)
    await waitFor(() => expect(invokeMock).toHaveBeenCalledWith('reveal_database', undefined))
    expect(invokeMock.mock.calls.filter(([command]) => command === 'reveal_database').every(([, args]) => args === undefined)).toBe(true)

    await expandSettingsSection(user, 'gh --version', /GitHub connection/i)
    expect(screen.queryByText('Line classifier', { exact: true })).not.toBeInTheDocument()
    for (const snippet of ['gh --version', 'gh auth status', 'gh api user --jq .login']) {
      expect(screen.getByText(snippet, { exact: false })).toBeInTheDocument()
    }
  })

  it('loads and autosaves background execution and supports every menu bar metric', async () => {
    configureBackend({ settings: { run_in_background: true, menu_bar_metric: 'total_lines', menu_bar_metrics: ['total_lines'] } })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()

    const background = screen.getByRole('switch', { name: /Keep running in the background/i })
    const menuVisibility = screen.getByRole('switch', { name: 'Show CodeTally in the menu bar' })
    const metric = screen.getByRole('group', { name: 'Menu bar metrics' })
    expect(background).toBeChecked()
    expect(menuVisibility).toBeChecked()
    expect(within(metric).getByRole('checkbox', { name: 'Total lines' })).toBeChecked()
    for (const label of ['Total lines', 'Source lines', 'Test lines', 'Open PRs', 'Open issues']) {
      expect(within(metric).getByRole('checkbox', { name: label })).toBeInTheDocument()
    }

    await user.click(background)
    await user.click(menuVisibility)
    await user.click(within(metric).getByRole('checkbox', { name: 'Open issues' }))

    await waitFor(() => {
      const writes = savedSettings()
      expect(writes[writes.length - 1]).toEqual(expect.objectContaining({ run_in_background: false, show_menu_bar: false, menu_bar_metric: 'total_lines', menu_bar_metrics: ['total_lines', 'open_issues'] }))
    })
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Save settings/i })).not.toBeInTheDocument()
  })

  it('persists the fork inclusion setting through off, on, and off states', async () => {
    configureBackend({ settings: { include_forks_in_totals: false } })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    const includeForks = await screen.findByRole('switch', { name: 'Include forks in line totals' })
    expect(includeForks).not.toBeChecked()

    await user.click(includeForks)
    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('set_app_settings', expect.objectContaining({
        settings: expect.objectContaining({ include_forks_in_totals: true })
      }))
    })

    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
    const enabledForks = screen.getByRole('switch', { name: 'Include forks in line totals' })
    expect(enabledForks).toBeChecked()
    await user.click(enabledForks)
    await waitFor(() => {
      expect(savedSettings().map((settings) => settings.include_forks_in_totals)).toEqual([true, false])
    })
  })

  it('loads named organization groups and persists tracking choices separately from the global company switch', async () => {
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()

    const companySwitch = await screen.findByRole('switch', { name: 'Track company repositories' })
    const personalSwitch = await screen.findByRole('switch', { name: 'Track personal repositories' })
    const organizationGroup = await screen.findByRole('switch', { name: 'Track acme repositories' })
    expect(screen.getByRole('button', { name: 'Expand acme repositories' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Expand Personal repositories' })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Expand acme repositories' }))
    await user.click(screen.getByRole('button', { name: 'Expand Personal repositories' }))
    const companyRepository = screen.getByRole('checkbox', { name: 'acme/beta' })
    const personalRepository = screen.getByRole('checkbox', { name: 'sam/alpha' })
    const disabledRepository = screen.getByRole('checkbox', { name: 'sam/disabled' })

    await waitFor(() => {
      expect(companySwitch).toBeChecked()
      expect(personalSwitch).toBeChecked()
      expect(organizationGroup).toBeChecked()
      expect(companyRepository).toBeChecked()
      expect(personalRepository).toBeChecked()
      expect(disabledRepository).toBeChecked()
      expect(invokeMock.mock.calls.some(([command]) => command === 'get_repository_selection')).toBe(true)
    })

    const collapseCompany = screen.getByRole('button', { name: 'Collapse acme repositories' })
    await user.click(collapseCompany)
    expect(screen.queryByRole('checkbox', { name: 'acme/beta' })).not.toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Expand acme repositories' }))
    expect(screen.getByRole('checkbox', { name: 'acme/beta' })).toBeChecked()

    await user.click(companySwitch)
    expect(companySwitch).not.toBeChecked()
    expect(screen.getByRole('checkbox', { name: 'acme/beta' })).toBeChecked()
    expect(screen.getByRole('checkbox', { name: 'acme/beta' })).toBeDisabled()

    await user.click(personalRepository)
    expect(personalRepository).not.toBeChecked()

    await waitFor(() => {
      const writes = savedSettings()
      expect(writes[writes.length - 1]).toEqual(expect.objectContaining({
        include_personal_repositories: true,
        include_company_repositories: false,
        include_forks_in_totals: false,
        excluded_repository_ids: ['repo-alpha']
      }))
    })
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Save settings/i })).not.toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Close settings' }))
    await user.click(screen.getByRole('button', { name: 'Settings' }))
    await user.click(await screen.findByRole('button', { name: 'Expand Personal repositories' }))
    await user.click(screen.getByRole('button', { name: 'Expand acme repositories' }))
    const savedPersonal = await screen.findByRole('checkbox', { name: 'sam/alpha' })
    expect(savedPersonal).not.toBeChecked()
    expect(screen.getByRole('switch', { name: 'Track company repositories' })).not.toBeChecked()
    expect(screen.getByRole('switch', { name: 'Track acme repositories' })).toBeChecked()
    expect(screen.getByRole('checkbox', { name: 'acme/beta' })).toBeChecked()
    expect(screen.getByRole('checkbox', { name: 'acme/beta' })).toBeDisabled()
  })

  it('keeps Settings open while excluding the active repository and closes its detail view', async () => {
    const user = await renderDashboard()

    await user.click(screen.getByRole('row', { name: /alpha/ }))
    expect(await screen.findByRole('heading', { name: 'alpha' })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()
    await user.click(screen.getByRole('button', { name: 'Expand Personal repositories' }))

    await user.click(screen.getByRole('checkbox', { name: 'sam/alpha' }))

    await waitFor(() => {
      expect(screen.queryByRole('heading', { name: 'alpha' })).not.toBeInTheDocument()
      expect(screen.getByRole('heading', { name: 'Code at a glance' })).toBeInTheDocument()
    })
    expect(screen.getByRole('heading', { name: 'Settings' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: /Save settings/i })).not.toBeInTheDocument()
  })

  it('deduplicates a dashboard error repeated by the top-level and repository fields', async () => {
    const message = 'sam/alpha: Sync failed'
    configureBackend({
      cachedDashboard: {
        ...dashboard,
        repositories: [{ ...repoAlpha, last_error: 'Sync failed' }, repoBeta],
        errors: [message]
      }
    })

    await renderDashboard()

    await waitFor(() => expect(screen.getByText(message)).toBeInTheDocument())
    expect(screen.getAllByText(message)).toHaveLength(1)
  })

  it('keeps overall repository progress stable while per-repository snapshot progress resets', async () => {
    vi.useFakeTimers()
    const sync = deferred<SyncResult>()
    const firstRepository: SyncProgress = {
      running: true,
      phase: 'backfilling',
      current: 43,
      total: 43,
      repository_current: 1,
      repository_total: 7,
      snapshot_current: 43,
      snapshot_total: 43,
      repository_name: 'sam/alpha',
      message: 'Analysing alpha snapshots'
    }
    const secondRepository: SyncProgress = {
      running: true,
      phase: 'backfilling',
      current: 2,
      total: 2,
      repository_current: 2,
      repository_total: 7,
      snapshot_current: 2,
      snapshot_total: 2,
      repository_name: 'sam/beta',
      message: 'Analysing beta snapshots'
    }
    const completed: SyncProgress = {
      running: false,
      phase: 'complete',
      current: 19,
      total: 19,
      repository_current: 19,
      repository_total: 19,
      snapshot_current: 19,
      snapshot_total: 19,
      repository_name: 'sam/last-repository',
      message: 'Complete'
    }
    configureBackend({ syncPromise: sync.promise, progress: [firstRepository, secondRepository, completed] })

    render(<App />)
    try {
      await act(async () => {
        for (let index = 0; index < 12; index += 1) await Promise.resolve()
      })
      expect(screen.getByRole('heading', { name: 'Code at a glance' })).toBeInTheDocument()
      fireEvent.click(screen.getByRole('button', { name: /Sync details|Syncing|Importing/i }))
      const firstProgress = screen.getByText(/Repositories 1 \/ 7/)
      const firstPanel = firstProgress.closest('.sync-progress-panel')
      expect(firstPanel).not.toBeNull()
      const firstBar = within(firstPanel as HTMLElement).getByRole('progressbar', { name: 'Overall repository progress' })
      expect(firstBar).not.toBeNull()
      expect(Number(firstBar.getAttribute('aria-valuenow'))).toBe(1)
      expect(Number(firstBar.getAttribute('aria-valuemax'))).toBe(7)

      await act(async () => {
        vi.advanceTimersByTime(650)
        for (let index = 0; index < 8; index += 1) await Promise.resolve()
      })

      const secondProgress = screen.getByText(/Repositories 2 \/ 7/)
      const secondPanel = secondProgress.closest('.sync-progress-panel')
      expect(secondPanel).not.toBeNull()
      const secondBar = within(secondPanel as HTMLElement).getByRole('progressbar', { name: 'Overall repository progress' })
      expect(secondBar).not.toBeNull()
      expect(Number(secondBar.getAttribute('aria-valuenow'))).toBe(2)
      expect(Number(secondBar.getAttribute('aria-valuemax'))).toBe(7)
      expect(Number(secondBar.getAttribute('aria-valuenow'))).toBeGreaterThanOrEqual(Number(firstBar.getAttribute('aria-valuenow')))

      for (const panel of [firstPanel, secondPanel]) {
        const ratios = (panel?.textContent ?? '').match(/\b(\d+)\s*\/\s*(\d+)\b/g) ?? []
        for (const ratio of ratios) {
          const [, numerator, denominator] = ratio.match(/(\d+)\s*\/\s*(\d+)/) ?? []
          expect(Number(numerator)).toBeLessThanOrEqual(Number(denominator))
        }
      }

      await act(async () => {
        vi.advanceTimersByTime(650)
        for (let index = 0; index < 8; index += 1) await Promise.resolve()
      })
      const completedProgress = screen.getByText(/Repositories 19 \/ 19/)
      const completedPanel = completedProgress.closest('.sync-progress-panel')
      expect(completedPanel).not.toBeNull()
      expect(completedPanel?.querySelector('strong')).toHaveTextContent('Complete')
      const completedBar = within(completedPanel as HTMLElement).getByRole('progressbar', { name: 'Overall repository progress' })
      expect(Number(completedBar.getAttribute('aria-valuenow'))).toBe(19)
      expect(Number(completedBar.getAttribute('aria-valuemax'))).toBe(19)
    } finally {
      sync.resolve(syncOk)
      await act(async () => {
        await Promise.resolve()
        await Promise.resolve()
      })
    }
  })

  it('refreshes cached dashboard and LOC history while an initial import is still running', async () => {
    const sync = deferred<SyncResult>()
    const emptyDashboard: DashboardData = {
      user: { login: 'sam' },
      repositories: [],
      metrics: { repositories: 0, total_loc: 0, source_loc: 0, test_loc: 0, loc_30d_change: 0, open_prs: 0, open_issues: 0 },
      loc_history: [],
      pull_requests: [],
      issues: [],
      last_sync_at: null
    }
    const inProgress: SyncProgress = {
      running: true,
      phase: 'backfilling',
      current: 3,
      total: 12,
      repository_current: 1,
      repository_total: 2,
      snapshot_current: 3,
      snapshot_total: 12,
      repository_name: 'sam/alpha',
      message: 'Analysing alpha snapshots'
    }
    configureBackend({ dashboardResponses: [emptyDashboard, dashboard, dashboard], syncPromise: sync.promise, progress: inProgress })
    const user = userEvent.setup()
    render(<App />)

    try {
      await user.click(await screen.findByRole('button', { name: 'Import repositories' }))
      expect(await screen.findByRole('heading', { name: 'Code at a glance' })).toBeInTheDocument()
      expect(screen.getByRole('heading', { name: 'Total Lines' })).toBeInTheDocument()
      expect(screen.queryByText('No LOC history for this range')).not.toBeInTheDocument()
      await user.click(screen.getByRole('button', { name: /Sync details|Syncing|Importing/i }))
      expect(screen.getByText(/Repositories 1 \/ 2/)).toBeInTheDocument()

      const dashboardCalls = invokeMock.mock.calls.filter(([command]) => command === 'get_dashboard')
      const historyCalls = invokeMock.mock.calls.filter(([command]) => command === 'get_loc_history')
      expect(dashboardCalls.length).toBeGreaterThanOrEqual(2)
      expect(historyCalls.length).toBeGreaterThanOrEqual(2)

      const progressPanel = screen.getByText(/Repositories 1 \/ 2/).closest('.sync-progress-panel')
      expect(progressPanel).not.toBeNull()
      const panelStyle = getComputedStyle(progressPanel as HTMLElement)
      expect(panelStyle.display).toBe('grid')
      expect(Number.parseFloat(panelStyle.minHeight)).toBeGreaterThan(0)
      const status = progressPanel?.querySelector('.sync-progress-heading')
      const bars = progressPanel?.querySelector('.sync-progress-bars')
      expect(status).not.toBeNull()
      expect(bars).not.toBeNull()
      expect(progressPanel?.firstElementChild).toBe(status)
      expect(progressPanel?.lastElementChild).toBe(bars)
      const progressRows = bars?.querySelectorAll('.sync-progress-row') ?? []
      expect(progressRows.length).toBe(2)
      expect(within(bars as HTMLElement).getByText('Overall repository progress')).toBeInTheDocument()
      expect(within(bars as HTMLElement).getByText('Current repository samples')).toBeInTheDocument()
      expect(within(bars as HTMLElement).getAllByRole('progressbar')).toHaveLength(2)
    } finally {
      sync.resolve(syncOk)
    }
  })
})
