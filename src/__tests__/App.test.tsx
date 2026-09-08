import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import App from '../App'
import type { DashboardData, DependencyStatus, Issue, LocSnapshot, PullRequest, Repository, SyncProgress, SyncResult } from '../types'

const invokeMock = vi.hoisted(() => vi.fn())

vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))

const repoAlpha: Repository = {
  id: 1,
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

const readyDependencies: DependencyStatus = { gh: true, git: true, tokei: true, gh_authenticated: true, authenticated: true, login: 'sam' }
const syncOk: SyncResult = { ok: true, message: 'Sync complete', repositories_synced: 2, pull_requests_synced: 2, issues_synced: 2, snapshots_created: 0, errors: [] }
const defaultAppSettings = { activity_refresh_minutes: 2, lines_refresh_minutes: 45, refresh_lines_on_change: true }

function deferred<T>() {
  let resolve!: (value: T) => void
  const promise = new Promise<T>((resolvePromise) => { resolve = resolvePromise })
  return { promise, resolve }
}

type BackendOptions = {
  dependencies?: DependencyStatus
  syncResult?: SyncResult
  cachedDashboard?: DashboardData
  dashboardResponses?: DashboardData[]
  syncPromise?: Promise<SyncResult>
  activitySyncPromise?: Promise<SyncResult>
  fullSyncPromise?: Promise<SyncResult>
  progress?: SyncProgress | SyncProgress[]
  settings?: Partial<typeof defaultAppSettings>
}

function configureBackend({ dependencies = readyDependencies, syncResult = syncOk, cachedDashboard = dashboard, dashboardResponses, syncPromise, activitySyncPromise, fullSyncPromise, progress, settings }: BackendOptions = {}) {
  let progressIndex = 0
  let dashboardIndex = 0
  let appSettings = { ...defaultAppSettings, ...settings }
  const dashboards = dashboardResponses?.length ? dashboardResponses : [cachedDashboard]
  invokeMock.mockImplementation(async (command: string, args?: Record<string, unknown>) => {
    switch (command) {
      case 'check_dependencies':
        return dependencies
      case 'get_github_user':
        return { login: 'sam' }
      case 'get_dashboard':
        return dashboards[Math.min(dashboardIndex++, dashboards.length - 1)]
      case 'discover_repositories':
        return null
      case 'get_loc_history': {
        const repositoryId = args?.repository_id
        if (repositoryId === null || repositoryId === undefined) return history
        return history.filter((snapshot) => String(snapshot.repository_id) === String(repositoryId))
      }
      case 'get_activity_feed':
        return args?.kind === 'issues' ? issues : prs
      case 'sync_github_data':
        return fullSyncPromise ?? syncPromise ?? syncResult
      case 'sync_activity':
        return activitySyncPromise ?? syncPromise ?? syncResult
      case 'get_app_settings':
        return appSettings
      case 'set_app_settings': {
        if (!args?.settings || typeof args.settings !== 'object') throw new Error('set_app_settings requires a settings payload')
        const next = args.settings as Partial<typeof defaultAppSettings>
        appSettings = { ...appSettings, ...(next ?? {}) }
        return appSettings
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

beforeEach(() => {
  invokeMock.mockReset()
  configureBackend()
})

afterEach(() => {
  cleanup()
  vi.useRealTimers()
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

    const repositoryFilter = within(sidebar()).getByRole('combobox', { name: 'Filter by repository' })
    await user.selectOptions(repositoryFilter, '2')
    expect(screen.queryByText('Alpha issue')).not.toBeInTheDocument()
    expect(screen.queryByText('Beta issue')).not.toBeInTheDocument()

    await user.click(within(sidebar()).getByRole('button', { name: 'Pull requests' }))
    await user.click(within(sidebar()).getByRole('button', { name: 'All' }))
    expect(screen.getByText('Fix alpha flow')).toBeInTheDocument()
    expect(screen.getByText('Refactor beta tests')).toBeInTheDocument()

    await user.click(within(sidebar()).getByRole('button', { name: 'Merged' }))
    expect(screen.getByText('Refactor beta tests')).toBeInTheDocument()
    expect(screen.queryByText('Fix alpha flow')).not.toBeInTheDocument()
  })

  it('passes the selected repository id through the Tauri activity request', async () => {
    const user = await renderDashboard()

    await user.selectOptions(screen.getByRole('combobox', { name: 'Filter by repository' }), '2')

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
    expect(within(sidebar()).getByRole('combobox', { name: 'Filter by repository' })).toHaveValue('1')
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
    expect(within(sidebar()).getByRole('combobox', { name: 'Filter by repository' })).toHaveValue('1')

    await user.click(within(sidebar()).getByRole('button', { name: 'Pull requests' }))
    await waitFor(() => expect(within(sidebar()).getByText('Fix alpha flow')).toBeInTheDocument())
    expect(within(sidebar()).getByRole('combobox', { name: 'Filter by repository' })).toHaveValue('1')

    await user.click(screen.getByRole('button', { name: 'Back to portfolio' }))
    await screen.findByRole('heading', { name: 'Code at a glance' })
    const restoredSidebar = sidebar()
    const restoredRepository = (within(restoredSidebar).getByRole('combobox', { name: 'Filter by repository' }) as HTMLSelectElement).value
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

  it('uses activity-only sync on startup and scheduled refreshes while manual Refresh uses full sync', async () => {
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
    expect(activityCallsAfterSchedule).toBeGreaterThan(activityCallsBeforeSchedule)
  })

  it('loads cadence settings and persists updated refresh choices', async () => {
    configureBackend({ settings: { activity_refresh_minutes: 10, lines_refresh_minutes: 30, refresh_lines_on_change: false } })
    const user = await renderDashboard()

    await user.click(screen.getByRole('button', { name: 'Settings' }))
    expect(await screen.findByRole('heading', { name: 'Settings' })).toBeInTheDocument()

    const activityRefresh = screen.getByRole('combobox', { name: /Activity refresh/i })
    const lineRefresh = screen.getByRole('combobox', { name: /Line count refresh/i })
    const codeChangeRefresh = screen.getByRole('checkbox', { name: /Refresh line counts when code changes/i })
    await waitFor(() => {
      expect(activityRefresh).toHaveValue('10')
      expect(lineRefresh).toHaveValue('30')
      expect(codeChangeRefresh).not.toBeChecked()
    })

    await user.selectOptions(activityRefresh, '5')
    await user.selectOptions(lineRefresh, '60')
    await user.click(codeChangeRefresh)
    await user.click(screen.getByRole('button', { name: 'Save settings' }))

    await waitFor(() => {
      expect(invokeMock).toHaveBeenCalledWith('set_app_settings', {
        settings: {
          activity_refresh_minutes: 5,
          lines_refresh_minutes: 60,
          refresh_lines_on_change: true
        }
      })
    })
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
