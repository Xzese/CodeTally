import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AreaChart, CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { AlertCircle, ArrowDown, ArrowLeft, ArrowUp, BarChart3, Check, ChevronDown, ChevronUp, CircleDot, ExternalLink, GitBranch, Github, HardDrive, ListFilter, LoaderCircle, Moon, RefreshCw, Settings, Sun, Terminal, X, Zap } from 'lucide-react'
import { checkDependencies, discoverRepositories, getActivityFeed, getAppSettings, getDashboard, getGithubUser, getLocHistory, getSyncProgress, openExternalUrl, setAppSettings, syncActivity, syncGithubData } from './api'
import type { AppSettings, DashboardData, DependencyStatus, FeedKind, Issue, LocSnapshot, PullRequest, Repository, SyncProgress, TimeRange } from './types'
import { aggregateHistory, filterIssues, filterPullRequests, isDraft, isMerged, sortRepositories, type FeedState } from './model'
import { activityRepo, exactDate, formatCompact, formatCount, formatSigned, normalizeDashboard, normalizeLabels, relativeTime, repoActivity, repoBaseline30Available, repoChange, repoChangePercent, repoForks, repoLocAvailable, repoName, repoOpenIssues, repoOpenPrs, repoSource, repoStars, repoTests, repoTotal, repositoryId, repositoryLabel } from './utils'
import codetallyMark from './assets/codetally-mark.png'
import './styles.css'

type Screen = 'dashboard' | 'detail'
type RepoSort = 'name' | 'loc' | 'source' | 'tests' | 'growth' | 'prs' | 'issues' | 'stars' | 'forks' | 'activity'

const EMPTY_DEPS: DependencyStatus = { gh: false, git: false, tokei: false, gh_authenticated: false, authenticated: false, login: null }
const DEFAULT_APP_SETTINGS: AppSettings = { activity_refresh_minutes: 2, lines_refresh_minutes: 45, refresh_lines_on_change: true }

function toDashboard(raw: DashboardData | null | undefined = {}) {
  return normalizeDashboard(raw ?? {})
}

function isRepoActivity(item: PullRequest | Issue, repo: Repository): boolean {
  const id = item.repository_id ?? item.repositoryId
  return (id !== undefined && String(id) === String(repositoryId(repo))) || [repositoryLabel(repo), repoName(repo)].includes(activityRepo(item))
}

function repoByActivity(item: PullRequest | Issue, repositories: Repository[]): Repository | undefined {
  const id = item.repository_id ?? item.repositoryId
  return repositories.find((repo) => (id !== undefined && String(id) === String(repositoryId(repo))) || [repositoryLabel(repo), repoName(repo)].includes(activityRepo(item)))
}

function backendRepositoryId(value: string): number | null {
  if (value === 'all') return null
  const parsed = Number(value)
  return Number.isFinite(parsed) ? parsed : null
}

function statusText(pr: PullRequest): string {
  if (String(pr.state).toLowerCase() === 'closed' && !isMerged(pr)) return 'CLOSED'
  if (isMerged(pr)) return 'MERGED'
  if (isDraft(pr)) return 'DRAFT'
  return String(pr.state || 'OPEN').toUpperCase()
}

function syncFailure(result: { ok: boolean; message: string; errors?: string[] }): string | null {
  if (result.ok && !result.errors?.length) return null
  const detail = result.errors?.filter(Boolean).slice(0, 3).join(' ')
  return detail ? `${result.message}: ${detail}` : result.message
}

function progressHasCounters(progress: SyncProgress | null): boolean {
  if (!progress) return false
  const counts = progressCounts(progress)
  return counts.repositoriesTotal > 0 || counts.snapshotsTotal > 0 || counts.repositoriesCurrent > 0 || counts.snapshotsCurrent > 0
}

function App() {
  const [deps, setDeps] = useState<DependencyStatus>(EMPTY_DEPS)
  const [login, setLogin] = useState('')
  const [dashboard, setDashboard] = useState(toDashboard())
  const [busy, setBusy] = useState(true)
  const [syncing, setSyncing] = useState(false)
  const [importing, setImporting] = useState(false)
  const [importStep, setImportStep] = useState('Preparing import')
  const [syncProgress, setSyncProgress] = useState<SyncProgress | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [screen, setScreen] = useState<Screen>('dashboard')
  const [selectedRepo, setSelectedRepo] = useState<Repository | null>(null)
  const [detailHistory, setDetailHistory] = useState<LocSnapshot[] | null>(null)
  const [detailPrs, setDetailPrs] = useState<PullRequest[]>([])
  const [detailIssues, setDetailIssues] = useState<Issue[]>([])
  const [detailLoading, setDetailLoading] = useState(false)
  const [feedKind, setFeedKind] = useState<FeedKind>('prs')
  const [feedState, setFeedState] = useState<FeedState>('open')
  const [feedRepo, setFeedRepo] = useState('all')
  const [feedRevision, setFeedRevision] = useState(0)
  const [metric, setMetric] = useState<'total' | 'source' | 'tests'>('total')
  const [range, setRange] = useState<TimeRange>('ALL')
  const [repoSort, setRepoSort] = useState<RepoSort>('loc')
  const [repoSortDirection, setRepoSortDirection] = useState<'asc' | 'desc'>('desc')
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [appSettings, setAppSettingsState] = useState<AppSettings>(DEFAULT_APP_SETTINGS)
  const [settingsSaving, setSettingsSaving] = useState(false)
  const [settingsError, setSettingsError] = useState<string | null>(null)
  const [lightMode, setLightMode] = useState(false)
  const [syncPopoverOpen, setSyncPopoverOpen] = useState(false)
  const [syncPopoverPinned, setSyncPopoverPinned] = useState(false)
  const syncPopoverRef = useRef<HTMLDivElement | null>(null)
  const syncPopoverDismissedRef = useRef(false)
  const feedRepoBeforeDetailRef = useRef('all')
  const syncingRef = useRef(false)
  const importingRef = useRef(false)
  const startupRef = useRef(false)
  const feedRequestRef = useRef(0)
  const lastActiveProgressRef = useRef<SyncProgress | null>(null)
  const cachedRefreshRef = useRef(false)
  const cachedRefreshVersionRef = useRef(0)
  const finalRefreshRef = useRef(false)

  const activeRepositories = useMemo(() => dashboard.repositories.filter((repo) => !(repo.is_archived ?? repo.isArchived)), [dashboard.repositories])
  const partialLoc = useMemo(() => activeRepositories.some((repo) => !(repo.is_fork ?? repo.isFork) && !repoLocAvailable(repo)), [activeRepositories])
  const selectedRepoId = useMemo(() => selectedRepo ? repositoryId(selectedRepo) : null, [selectedRepo])
  const lastSync = dashboard.last_sync_at

  const applyDashboard = useCallback((raw: DashboardData, reloadFeeds = true) => {
    const next = toDashboard(raw)
    setDashboard((current) => ({
      ...next,
      pull_requests: current.pull_requests.length ? current.pull_requests : next.pull_requests,
      issues: current.issues.length ? current.issues : next.issues
    }))
    if (reloadFeeds) setFeedRevision((revision) => revision + 1)
    setSelectedRepo((previous) => previous ? next.repositories.find((repo) => String(repositoryId(repo)) === String(repositoryId(previous))) ?? previous : previous)
    if (next.user?.login) setLogin(next.user.login)
    const repositoryErrors = next.repositories.filter((repo) => repo.last_error).map((repo) => `${repositoryLabel(repo)}: ${repo.last_error}`).slice(0, 3)
    if (next.errors.length || repositoryErrors.length) setError([...next.errors, ...repositoryErrors].join(' '))
  }, [])

  const loadData = useCallback(async (showBusy = false): Promise<boolean> => {
    if (showBusy) setBusy(true)
    const dashboardResult = await Promise.allSettled([getDashboard()])
    let hasCachedRepositories = false
    if (dashboardResult[0].status === 'fulfilled') {
      const next = dashboardResult[0].value
      const historyResult = await Promise.allSettled([getLocHistory({ range: 'ALL' })])
      const history = historyResult[0].status === 'fulfilled' ? historyResult[0].value : []
      applyDashboard({ ...next, loc_history: next.loc_history ?? next.locHistory ?? next.history ?? history })
      hasCachedRepositories = next.repositories?.some((repo) => !(repo.is_archived ?? repo.isArchived)) ?? false
      if (historyResult[0].status === 'rejected' && !next.loc_history && !next.locHistory && !next.history) setError('Line history is not available yet. Import a repository to begin collecting snapshots.')
    } else {
      const reason = dashboardResult[0].reason
      setError(reason instanceof Error ? reason.message : String(reason))
    }
    setBusy(false)
    void checkDependencies().then(setDeps).catch((reason) => {
      if (!hasCachedRepositories) setError(reason instanceof Error ? reason.message : String(reason))
    })
    void getGithubUser().then((user) => { if (user.login) setLogin(user.login) }).catch(() => undefined)
    return hasCachedRepositories
  }, [applyDashboard])

  const loadFinalData = useCallback(async () => {
    finalRefreshRef.current = true
    cachedRefreshVersionRef.current += 1
    try {
      await loadData(false)
    } finally {
      finalRefreshRef.current = false
    }
  }, [loadData])

  const refreshCachedData = useCallback(async () => {
    if (cachedRefreshRef.current || finalRefreshRef.current) return
    cachedRefreshRef.current = true
    const requestVersion = ++cachedRefreshVersionRef.current
    try {
      const [dashboardResult, historyResult] = await Promise.allSettled([getDashboard(), getLocHistory({ range: 'ALL' })])
      if (finalRefreshRef.current || requestVersion !== cachedRefreshVersionRef.current || dashboardResult.status !== 'fulfilled') return
      const next = dashboardResult.value
      const history = historyResult.status === 'fulfilled' ? historyResult.value : []
      applyDashboard({ ...next, loc_history: next.loc_history ?? next.locHistory ?? next.history ?? history }, false)
      if (screen === 'detail' && selectedRepoId !== null && historyResult.status === 'fulfilled') {
        try {
          const scopedHistory = await getLocHistory({ repositoryId: selectedRepoId, range: 'ALL' })
          if (!finalRefreshRef.current && requestVersion === cachedRefreshVersionRef.current) setDetailHistory(scopedHistory)
        } catch {
          // The last repository-scoped history remains visible when a cache read fails.
        }
      }
    } finally {
      cachedRefreshRef.current = false
    }
  }, [applyDashboard, screen, selectedRepoId])

  const refreshData = useCallback(async (automatic = false) => {
    if (syncingRef.current || importingRef.current) return
    syncingRef.current = true
    setSyncing(true)
    setSyncProgress(null)
    lastActiveProgressRef.current = null
    setError(null)
    let completionMessage = ''
    let completionError: string | null = null
    let completionProgress: SyncProgress | null = null
    try {
      const result = await (automatic ? syncActivity() : syncGithubData())
      completionProgress = await getSyncProgress().catch(() => null)
      completionMessage = result.message
      completionError = result.errors?.[0] ?? null
      const failure = syncFailure(result)
      if (failure) setError(failure)
      await loadFinalData()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
      if (!automatic) await loadFinalData()
    } finally {
      setSyncProgress((previous) => {
        const authoritative = progressHasCounters(completionProgress) ? completionProgress : previous
        return authoritative ? { ...authoritative, running: false, message: completionMessage || authoritative.message || 'Sync complete', error: completionError ?? authoritative.error } : previous
      })
      syncingRef.current = false
      setSyncing(false)
    }
  }, [loadFinalData])

  useEffect(() => {
    if (startupRef.current) return
    startupRef.current = true
    void loadData(true).then((hasCachedRepositories) => { if (hasCachedRepositories) void refreshData(true) })
  }, [loadData, refreshData])

  useEffect(() => {
    void getAppSettings().then((settings) => setAppSettingsState({ ...DEFAULT_APP_SETTINGS, ...settings })).catch(() => undefined)
  }, [])

  useEffect(() => {
    const minutes = Math.max(1, Number(appSettings.activity_refresh_minutes) || DEFAULT_APP_SETTINGS.activity_refresh_minutes)
    const timer = window.setInterval(() => { void refreshData(true) }, minutes * 60 * 1000)
    return () => window.clearInterval(timer)
  }, [appSettings.activity_refresh_minutes, refreshData])

  useEffect(() => {
    if (!syncing && !importing) return
    let alive = true
    const poll = async () => {
      try {
        const progress = await getSyncProgress()
        if (!alive) return
        if (progress.running) {
          lastActiveProgressRef.current = progress
          setSyncProgress(progress)
        } else if (progressHasCounters(progress)) {
          setSyncProgress(progress)
        } else if (lastActiveProgressRef.current) {
          setSyncProgress({ ...lastActiveProgressRef.current, phase: progress.phase, message: progress.message || 'Completed', error: progress.error })
        }
      } catch { /* cached sync UI remains usable */ }
    }
    void poll()
    const timer = window.setInterval(() => void poll(), 650)
    return () => { alive = false; window.clearInterval(timer) }
  }, [importing, syncing])

  useEffect(() => {
    if (!syncing && !importing) return
    void refreshCachedData()
    const timer = window.setInterval(() => void refreshCachedData(), 2000)
    return () => {
      window.clearInterval(timer)
      cachedRefreshVersionRef.current += 1
    }
  }, [importing, refreshCachedData, syncing])

  useEffect(() => {
    if (!syncPopoverOpen) return
    const handlePointerDown = (event: PointerEvent) => {
      if (syncPopoverRef.current?.contains(event.target as Node)) return
      syncPopoverDismissedRef.current = true
      setSyncPopoverOpen(false)
      setSyncPopoverPinned(false)
    }
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      syncPopoverDismissedRef.current = true
      setSyncPopoverOpen(false)
      setSyncPopoverPinned(false)
    }
    document.addEventListener('pointerdown', handlePointerDown)
    document.addEventListener('keydown', handleKeyDown)
    return () => {
      document.removeEventListener('pointerdown', handlePointerDown)
      document.removeEventListener('keydown', handleKeyDown)
    }
  }, [syncPopoverOpen])

  const importRepositories = useCallback(async () => {
    if (importingRef.current || syncingRef.current) return
    importingRef.current = true
    setImporting(true)
    setSyncProgress(null)
    lastActiveProgressRef.current = null
    setError(null)
    let completionMessage = ''
    let completionError: string | null = null
    let completionProgress: SyncProgress | null = null
    try {
      setImportStep('Discovering repositories from GitHub')
      await discoverRepositories()
      setImportStep('Syncing pull requests and issues')
      const result = await syncGithubData()
      completionProgress = await getSyncProgress().catch(() => null)
      completionMessage = result.message
      completionError = result.errors?.[0] ?? null
      const failure = syncFailure(result)
      if (failure) setError(failure)
      setImportStep('Loading local line history')
      await loadFinalData()
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
      await loadFinalData()
    } finally {
      setSyncProgress((previous) => {
        const authoritative = progressHasCounters(completionProgress) ? completionProgress : previous
        return authoritative ? { ...authoritative, running: false, message: completionMessage || authoritative.message || 'Import complete', error: completionError ?? authoritative.error } : previous
      })
      importingRef.current = false
      setImporting(false)
      setImportStep('Preparing import')
    }
  }, [loadFinalData])

  const openRepository = useCallback(async (repo: Repository) => {
    feedRepoBeforeDetailRef.current = feedRepo
    setFeedRepo(String(repositoryId(repo)))
    setSelectedRepo(repo)
    setScreen('detail')
    setDetailLoading(true)
    try {
      const repoId = repositoryId(repo)
      const [history, prs, issues] = await Promise.all([getLocHistory({ repositoryId: repoId, range: 'ALL' }), getActivityFeed({ kind: 'prs', repositoryId: repoId }), getActivityFeed({ kind: 'issues', repositoryId: repoId })])
      setDetailHistory(history)
      setDetailPrs(prs as PullRequest[])
      setDetailIssues(issues as Issue[])
    } catch (reason) {
      setDetailHistory([])
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setDetailLoading(false)
    }
  }, [feedRepo])

  useEffect(() => {
    if (screen !== 'detail' || !selectedRepo || !dashboard.last_sync_at) return
    let alive = true
    const refreshScoped = async () => {
      try {
        const repoId = repositoryId(selectedRepo)
        const [history, prs, issues] = await Promise.all([getLocHistory({ repositoryId: repoId, range: 'ALL' }), getActivityFeed({ kind: 'prs', repositoryId: repoId }), getActivityFeed({ kind: 'issues', repositoryId: repoId })])
        if (!alive) return
        setDetailHistory(history)
        setDetailPrs(prs as PullRequest[])
        setDetailIssues(issues as Issue[])
      } catch {
        // Keep the last repository-scoped cache visible after a refresh failure.
      }
    }
    void refreshScoped()
    return () => { alive = false }
  }, [dashboard.last_sync_at, screen, selectedRepo])

  const backToDashboard = useCallback(() => {
    setScreen('dashboard')
    setFeedRepo(feedRepoBeforeDetailRef.current)
    setSelectedRepo(null)
    setDetailHistory(null)
    setDetailPrs([])
    setDetailIssues([])
  }, [])

  const handleFeedKind = useCallback((kind: FeedKind) => {
    setFeedKind(kind)
    setFeedState('open')
    setFeedRepo(screen === 'detail' && selectedRepo ? String(repositoryId(selectedRepo)) : 'all')
  }, [screen, selectedRepo])

  const handleFeedState = useCallback((state: FeedState) => {
    setFeedState(state)
  }, [])

  const handleFeedRepository = useCallback((repository: string) => {
    if (screen === 'detail' && selectedRepo) return
    setFeedRepo(repository)
  }, [screen, selectedRepo])

  useEffect(() => {
    const requestId = ++feedRequestRef.current
    let alive = true
    void getActivityFeed({ kind: feedKind, state: feedState, repositoryId: backendRepositoryId(feedRepo) }).then((items) => {
      if (!alive || requestId !== feedRequestRef.current) return
      if (feedKind === 'prs') setDashboard((current) => ({ ...current, pull_requests: items as PullRequest[] }))
      else setDashboard((current) => ({ ...current, issues: items as Issue[] }))
    }).catch((reason) => {
      if (!alive || requestId !== feedRequestRef.current) return
      const message = reason instanceof Error ? reason.message : String(reason)
      setError(`Unable to load ${feedKind === 'prs' ? 'pull requests' : 'issues'}: ${message}`)
    })
    return () => { alive = false }
  }, [feedKind, feedRepo, feedRevision, feedState])

  const visibleHistory = useMemo(() => aggregateHistory(
    screen === 'detail' && detailHistory !== null ? detailHistory : dashboard.loc_history,
    screen === 'detail' && detailHistory !== null ? null : screen === 'detail' && selectedRepo ? repositoryId(selectedRepo) : null,
    range
  ), [dashboard.loc_history, detailHistory, range, screen, selectedRepo])

  const metrics = dashboard.metrics
  const sortedRepositories = useMemo(() => sortRepositories(activeRepositories, repoSort, repoSortDirection), [activeRepositories, repoSort, repoSortDirection])
  const filteredPrs = useMemo(() => filterPullRequests(dashboard.pull_requests, feedState, feedRepo), [dashboard.pull_requests, feedRepo, feedState])
  const filteredIssues = useMemo(() => filterIssues(dashboard.issues, feedState === 'merged' ? 'all' : feedState, feedRepo), [dashboard.issues, feedRepo, feedState])
  const isSetup = activeRepositories.length === 0 && !busy
  const syncActive = syncing || importing
  const syncDetailsAvailable = syncActive || Boolean(syncProgress)
  const syncPopoverProgress = syncProgress ?? { running: true, phase: importing ? 'importing' : 'starting', current: 0, total: 0, message: importing ? importStep : 'Starting sync' }
  const saveSettings = useCallback(async (next: AppSettings) => {
    setSettingsSaving(true)
    setSettingsError(null)
    try {
      const persisted = await setAppSettings(next)
      setAppSettingsState({ ...DEFAULT_APP_SETTINGS, ...(persisted ?? next) })
      setSettingsOpen(false)
    } catch (reason) {
      setSettingsError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setSettingsSaving(false)
    }
  }, [])

  return (
    <div className={lightMode ? 'app-shell light' : 'app-shell'}>
      <header className="topbar">
        <div className="brand-mark"><img src={codetallyMark} alt="" aria-hidden="true" /><span>CodeTally</span></div>
        <div className="topbar-status">
          {login ? <span className="identity"><span className="status-dot" />{login}</span> : <span className="muted">Local desktop dashboard</span>}
          <button className="icon-button" title="Toggle theme" onClick={() => setLightMode((light) => !light)}>{lightMode ? <Moon size={17} /> : <Sun size={17} />}</button>
          <button className="icon-button" title="Settings" onClick={() => { setSettingsError(null); setSettingsOpen(true) }}><Settings size={17} /></button>
          <div
            className={syncDetailsAvailable ? 'sync-popover-wrap active' : 'sync-popover-wrap'}
            ref={syncPopoverRef}
            onMouseEnter={() => { syncPopoverDismissedRef.current = false; if (syncDetailsAvailable) setSyncPopoverOpen(true) }}
            onMouseLeave={() => { syncPopoverDismissedRef.current = false; if (!syncPopoverPinned) setSyncPopoverOpen(false) }}
            onFocusCapture={() => { if (syncDetailsAvailable && !syncPopoverDismissedRef.current) setSyncPopoverOpen(true) }}
            onBlurCapture={(event) => {
              if (!syncPopoverPinned && !event.currentTarget.contains(event.relatedTarget as Node | null)) setSyncPopoverOpen(false)
            }}
          >
            <button
              className={syncActive ? 'sync-status sync-status-trigger' : 'button primary compact refresh-button'}
              type="button"
              aria-label={syncActive ? `${importing ? 'Importing' : 'Syncing'}; show sync details` : 'Refresh GitHub data'}
              aria-expanded={syncDetailsAvailable ? syncPopoverOpen : undefined}
              aria-haspopup={syncDetailsAvailable ? 'dialog' : undefined}
              title={lastSync ? `${syncActive ? 'Show sync details' : 'Refresh GitHub data'}. Last sync: ${exactDate(lastSync)}` : syncActive ? 'Show sync details' : 'Refresh GitHub data'}
              onClick={() => {
                if (!syncActive) {
                  void refreshData()
                  return
                }
                if (syncPopoverOpen && syncPopoverPinned) {
                  syncPopoverDismissedRef.current = true
                  setSyncPopoverOpen(false)
                  setSyncPopoverPinned(false)
                } else if (syncPopoverOpen) {
                  syncPopoverDismissedRef.current = false
                  setSyncPopoverPinned(true)
                } else {
                  syncPopoverDismissedRef.current = false
                  setSyncPopoverOpen(true)
                  setSyncPopoverPinned(true)
                }
              }}
            >
              {syncActive ? <LoaderCircle className="spin" size={14} /> : <RefreshCw size={15} />}
              <span className="refresh-button-copy"><strong>{syncActive ? importing ? 'Importing' : 'Syncing' : 'Refresh'}</strong><small>Last sync: {relativeTime(lastSync)}</small></span>
            </button>
            {syncDetailsAvailable && syncPopoverOpen && <div className="sync-popover" role="dialog" aria-label="Sync progress">
              <div className="sync-popover-heading"><span>Sync details</span><button className="icon-button subtle" type="button" aria-label="Close sync details" onClick={() => { syncPopoverDismissedRef.current = true; setSyncPopoverOpen(false); setSyncPopoverPinned(false) }}><X size={14} /></button></div>
              <SyncProgressPanel progress={syncPopoverProgress} />
            </div>}
          </div>
        </div>
      </header>

      {error && <div className="error-banner"><AlertCircle size={16} /><span>{error}</span><button className="icon-button subtle" onClick={() => setError(null)} aria-label="Dismiss error"><X size={15} /></button></div>}

      {busy && !dashboard.repositories.length ? <LoadingScreen /> : isSetup ? <SetupScreen deps={deps} login={login} error={error} importing={importing} importStep={importStep} onImport={() => void importRepositories()} onRetry={() => void loadData(true)} /> : (
        <div className="content-shell">
          {screen === 'dashboard' ? (
            <main className="main-column">
              <div className="page-heading"><div><p className="eyebrow">Portfolio overview</p><h1>Code at a glance</h1></div><span className="small-note"><CircleDot size={13} /> {activeRepositories.length} active repositories</span></div>
              <SummaryStrip metrics={metrics} partialLoc={partialLoc} />
              <LocChart history={visibleHistory} metric={metric} range={range} onMetric={setMetric} onRange={setRange} />
              <RepositoryTable repositories={sortedRepositories} sort={repoSort} direction={repoSortDirection} onSort={(next) => { if (next === repoSort) setRepoSortDirection((current) => current === 'asc' ? 'desc' : 'asc'); else { setRepoSort(next); setRepoSortDirection(next === 'name' ? 'asc' : 'desc') } }} onSelect={(repo) => void openRepository(repo)} />
            </main>
          ) : selectedRepo ? (
            <RepositoryDetails repo={selectedRepo} history={visibleHistory} historyLoading={detailLoading} metric={metric} range={range} onMetric={setMetric} onRange={setRange} onBack={backToDashboard} pullRequests={detailPrs} issues={detailIssues} onOpenUrl={(url) => void openUrl(url, setError)} />
          ) : null}
          <ActivitySidebar kind={feedKind} state={feedState} repository={feedRepo} lockedRepository={screen === 'detail' && selectedRepo ? String(repositoryId(selectedRepo)) : null} repositories={activeRepositories} prs={filteredPrs} issues={filteredIssues} onKind={handleFeedKind} onState={handleFeedState} onRepository={handleFeedRepository} onOpenUrl={(url) => void openUrl(url, setError)} />
        </div>
      )}

      {settingsOpen && <SettingsDrawer settings={appSettings} saving={settingsSaving} error={settingsError} onSave={(next) => void saveSettings(next)} onClose={() => setSettingsOpen(false)} />}
    </div>
  )
}

async function openUrl(url: string | undefined, setError: (error: string | null) => void) {
  if (!url) return
  try { await openExternalUrl(url) } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)) }
}

function LoadingScreen() {
  return <div className="center-state"><LoaderCircle size={27} className="spin" /><p>Opening your local portfolio</p><span>Reading cached GitHub data…</span></div>
}

function progressCounts(progress: SyncProgress) {
  const repositoriesCurrent = progress.repository_current ?? progress.current ?? 0
  const repositoriesTotal = progress.repository_total ?? progress.total ?? 0
  const snapshotsCurrent = progress.snapshot_current ?? ((progress.phase === 'backfilling' || progress.phase === 'scanning_current') ? progress.current : 0)
  const snapshotsTotal = progress.snapshot_total ?? ((progress.phase === 'backfilling' || progress.phase === 'scanning_current') ? progress.total : 0)
  return { repositoriesCurrent, repositoriesTotal, snapshotsCurrent, snapshotsTotal }
}

function ProgressMeter({ value, max, label }: { value: number; max: number; label: string }) {
  const safeMax = Math.max(0, max)
  const safeValue = safeMax > 0 ? Math.min(safeMax, Math.max(0, value)) : 0
  const percent = safeMax > 0 ? safeValue / safeMax * 100 : 0
  return <div className="progress-meter" role="progressbar" aria-label={label} aria-valuemin={0} aria-valuemax={safeMax} aria-valuenow={safeValue}><span style={{ width: `${percent}%` }} /></div>
}

function readablePhase(phase: string): string {
  if (!phase) return 'Working'
  return phase.replaceAll('_', ' ').replace(/\b\w/g, (character) => character.toUpperCase())
}

function SyncProgressPanel({ progress }: { progress: SyncProgress }) {
  const counts = progressCounts(progress)
  const overallLabel = counts.repositoriesTotal > 0 ? `Repositories ${counts.repositoriesCurrent} / ${counts.repositoriesTotal}` : 'Repository count pending'
  const samplesLabel = counts.snapshotsTotal > 0 ? `Samples processed ${counts.snapshotsCurrent} / ${counts.snapshotsTotal}` : null
  return <section className="sync-progress-panel" aria-live="polite"><div className="sync-progress-heading"><div><p className="eyebrow">{progress.running ? readablePhase(progress.phase) : 'Last sync'}</p><strong>{progress.message || readablePhase(progress.phase)}</strong>{progress.repository_name && <span>{progress.repository_name}</span>}</div><span className={progress.running ? 'sync-progress-state active' : 'sync-progress-state'}>{progress.running ? 'In progress' : 'Complete'}</span>{progress.error && <div className="sync-progress-error"><AlertCircle size={13} /> {progress.error}</div>}</div><div className="sync-progress-bars"><div className="sync-progress-row"><div><span>Overall repository progress</span><b>{overallLabel}</b></div><ProgressMeter value={counts.repositoriesCurrent} max={counts.repositoriesTotal} label="Overall repository progress" /></div><div className="sync-progress-row samples"><div><span>Current repository samples</span><b>{samplesLabel ?? 'Samples pending'}</b></div><ProgressMeter value={counts.snapshotsCurrent} max={counts.snapshotsTotal} label="Current repository samples processed" /></div></div></section>
}

function SetupScreen({ deps, login, error, importing, importStep, onImport, onRetry }: { deps: DependencyStatus; login: string; error: string | null; importing: boolean; importStep: string; onImport: () => void; onRetry: () => void }) {
  const checks = [
    { label: 'Git installed', ok: deps.git, icon: GitBranch },
    { label: 'GitHub CLI installed', ok: deps.gh, icon: Terminal },
    { label: deps.authenticated ? `Logged in as ${login || deps.login}` : 'GitHub authentication', ok: deps.authenticated, icon: Github },
    { label: 'Tokei installed', ok: deps.tokei, icon: Zap }
  ]
  const ready = checks.every((check) => check.ok)
  return <div className="setup-wrap"><div className="setup-card"><div className="setup-icon"><img src={codetallyMark} alt="" aria-hidden="true" /></div><p className="eyebrow">First run</p><h1>Connect your local toolkit</h1><p className="setup-copy">CodeTally uses the tools already installed on this Mac. Nothing is uploaded and no token is requested.</p><div className="setup-checks">{checks.map(({ label, ok, icon: Icon }) => <div className={ok ? 'setup-check ok' : 'setup-check'} key={label}><span className="check-icon">{ok ? <Check size={15} /> : <Icon size={15} />}</span><span>{label}</span>{ok ? <span className="check-state">Ready</span> : <span className="check-state missing">Missing</span>}</div>)}</div>{error && <div className="setup-error"><AlertCircle size={15} /> {error}</div>}{importing ? <div className="setup-import-note" role="status"><LoaderCircle size={15} className="spin" /><span>{importStep}</span><small>Follow progress from the Importing status in the header.</small></div> : <button className="button primary wide" onClick={ready ? onImport : onRetry}>{ready ? <><HardDrive size={16} /> {error ? 'Retry import' : 'Import repositories'}</> : 'Retry dependency checks'}</button>}</div></div>
}

function SummaryStrip({ metrics, partialLoc }: { metrics: ReturnType<typeof toDashboard>['metrics']; partialLoc: boolean }) {
  const values = [
    { label: 'Repositories', value: formatCount(metrics.repositories), icon: Github, partial: false },
    { label: 'Total Lines', value: formatCount(metrics.total_loc), icon: GitBranch, partial: partialLoc },
    { label: 'Source Lines', value: formatCount(metrics.source_loc), icon: CircleDot, partial: partialLoc },
    { label: 'Test Lines', value: formatCount(metrics.test_loc), icon: Check, partial: partialLoc },
    { label: '30d net change', value: formatSigned(metrics.loc_30d_change, true), icon: metrics.loc_30d_change >= 0 ? ArrowUp : ArrowDown, accent: metrics.loc_30d_change >= 0 ? 'positive' : 'negative', partial: partialLoc },
    { label: 'Open PRs', value: formatCount(metrics.open_prs), icon: GitBranch, partial: false },
    { label: 'Open issues', value: formatCount(metrics.open_issues), icon: ListFilter, partial: false }
  ]
  return <div className="summary-strip">{values.map(({ label, value, icon: Icon, accent, partial }) => <div className="summary-item" key={label}><span className="summary-label"><Icon size={13} /> {label}{partial && <em className="summary-partial">Partial</em>}</span><strong className={accent ?? ''}>{value}</strong></div>)}</div>
}

function LocChart({ history, metric, range, onMetric, onRange }: { history: { date: string; timestamp: number; total: number; source: number; tests: number }[]; metric: 'total' | 'source' | 'tests'; range: TimeRange; onMetric: (metric: 'total' | 'source' | 'tests') => void; onRange: (range: TimeRange) => void }) {
  const label = metric === 'total' ? 'Total Lines' : metric === 'source' ? 'Source Lines' : 'Test Lines'
  const color = metric === 'total' ? '#68a6ff' : metric === 'source' ? '#52d2b1' : '#d3a9ff'
  return <section className="chart-panel"><div className="panel-heading"><div><p className="eyebrow">Growth over time</p><h2>{label}</h2></div><div className="chart-controls"><div className="segmented" role="tablist" aria-label="Lines metric">{(['total', 'source', 'tests'] as const).map((key) => <button key={key} className={metric === key ? 'active' : ''} onClick={() => onMetric(key)}>{key === 'total' ? 'Total' : key === 'source' ? 'Source' : 'Tests'}</button>)}</div><div className="range-controls" aria-label="Time range">{(['3M', '1Y', '3Y', 'ALL'] as const).map((key) => <button key={key} className={range === key ? 'active' : ''} onClick={() => onRange(key)}>{key}</button>)}</div></div></div>{history.length ? <div className="chart-wrap"><ResponsiveContainer width="100%" height="100%"><LineChart data={history} margin={{ top: 18, right: 18, left: 2, bottom: 4 }}><CartesianGrid stroke="rgba(148, 163, 184, .11)" vertical={false} /><XAxis type="number" dataKey="timestamp" domain={['dataMin', 'dataMax']} tickFormatter={(value) => { const parsed = new Date(Number(value)); return Number.isNaN(parsed.getTime()) ? '' : new Intl.DateTimeFormat(undefined, { month: 'short', year: '2-digit' }).format(parsed) }} tick={{ fill: '#8491a7', fontSize: 11 }} tickLine={false} axisLine={false} minTickGap={38} /><YAxis tickFormatter={formatCompact} tick={{ fill: '#8491a7', fontSize: 11 }} tickLine={false} axisLine={false} width={43} /><Tooltip content={<LocTooltip />} cursor={{ stroke: 'rgba(137,162,203,.35)', strokeDasharray: '4 4' }} /><Line type="monotone" dataKey="total" name="Total" stroke="#68a6ff" strokeWidth={metric === 'total' ? 2.7 : 1.4} strokeOpacity={metric === 'total' ? 1 : .26} dot={false} activeDot={{ r: 4, strokeWidth: 2, fill: '#0d1421' }} /><Line type="monotone" dataKey="source" name="Source" stroke="#52d2b1" strokeWidth={metric === 'source' ? 2.7 : 1.4} strokeOpacity={metric === 'source' ? 1 : .26} dot={false} activeDot={{ r: 4, strokeWidth: 2, fill: '#0d1421' }} /><Line type="monotone" dataKey="tests" name="Tests" stroke="#d3a9ff" strokeWidth={metric === 'tests' ? 2.7 : 1.4} strokeOpacity={metric === 'tests' ? 1 : .26} dot={false} activeDot={{ r: 4, strokeWidth: 2, fill: '#0d1421' }} /></LineChart></ResponsiveContainer><div className="chart-legend"><span style={{ color }}><i /> {label}</span><span className="chart-hint">Hover for all series</span></div></div> : <div className="empty-chart"><BarChart3 size={24} /><p>No line history for this range</p><span>Import a repository to create the first monthly snapshot.</span></div>}</section>
}

function LocTooltip({ active, payload, label }: { active?: boolean; payload?: Array<{ name?: string; value?: number; color?: string }>; label?: string | number }) {
  if (!active || !payload?.length) return null
  return <div className="chart-tooltip"><strong>{exactDate(label)}</strong>{payload.map((entry) => <div key={entry.name} className="tooltip-row"><span><i style={{ background: entry.color }} />{entry.name}</span><b>{formatCount(Number(entry.value ?? 0))}</b></div>)}</div>
}

function RepositoryTable({ repositories, sort, direction, onSort, onSelect }: { repositories: Repository[]; sort: RepoSort; direction: 'asc' | 'desc'; onSort: (sort: RepoSort) => void; onSelect: (repo: Repository) => void }) {
  const columns: Array<{ key: RepoSort; label: string; title: string }> = [{ key: 'name', label: 'Repository', title: 'Repository' }, { key: 'loc', label: 'Total', title: 'Total lines' }, { key: 'source', label: 'Source', title: 'Source lines' }, { key: 'tests', label: 'Tests', title: 'Test lines' }, { key: 'growth', label: '30d', title: '30-day change' }, { key: 'prs', label: 'PRs', title: 'Open pull requests' }, { key: 'issues', label: 'Issues', title: 'Open issues' }, { key: 'stars', label: 'Stars', title: 'GitHub stars' }, { key: 'forks', label: 'Forks', title: 'GitHub forks' }, { key: 'activity', label: 'Activity', title: 'Last activity' }]
  return <section className="repos-panel"><div className="panel-heading table-heading"><div><p className="eyebrow">Inventory</p><h2>Repositories</h2></div><span className="small-note">{repositories.length} tracked</span></div>{repositories.length ? <div className="table-scroll"><table><thead><tr>{columns.map((column) => <th key={column.key}><button className={sort === column.key ? 'sort-button active' : 'sort-button'} title={`Sort by ${column.title}`} onClick={() => onSort(column.key)}>{column.label}{sort === column.key && (direction === 'asc' ? <ChevronUp size={13} /> : <ChevronDown size={13} />)}</button></th>)}</tr></thead><tbody>{repositories.map((repo) => { const isPrivate = repo.is_private ?? repo.isPrivate ?? false; return <tr key={String(repositoryId(repo))} onClick={() => onSelect(repo)} tabIndex={0} onKeyDown={(event) => { if (event.key === 'Enter') onSelect(repo) }}><td><div className="repo-cell"><span className="repo-glyph"><GitBranch size={14} /></span><span><strong>{repoName(repo)}</strong><small><span className="repo-meta-text">{repositoryLabel(repo)}{repo.primary_language ?? repo.primaryLanguage ? ` · ${repo.primary_language ?? repo.primaryLanguage}` : ''}</span><em className={`repo-visibility ${isPrivate ? 'private' : 'public'}`}>{isPrivate ? 'Private' : 'Public'}</em></small></span></div></td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoTotal(repo))}</td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoSource(repo))}</td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoTests(repo))}</td><td className={repoGrowthClass(repo)} title={repoGrowthTitle(repo)}>{repoChangeDisplay(repo)}</td><td className="number-cell">{formatCount(repoOpenPrs(repo))}</td><td className="number-cell">{formatCount(repoOpenIssues(repo))}</td><td className="number-cell">{formatCount(repoStars(repo))}</td><td className="number-cell">{formatCount(repoForks(repo))}</td><td><span className="relative" title={exactDate(repoActivity(repo))}>{relativeTime(repoActivity(repo))}</span></td></tr> })}</tbody></table></div> : <div className="empty-state"><GitBranch size={22} /><p>No repositories in the portfolio</p></div>}</section>
}

function repoLocDisplay(repo: Repository, count: number): string {
  return repoLocAvailable(repo) ? formatCompact(count) : '—'
}

function repoChangeDisplay(repo: Repository): string {
  return repoLocAvailable(repo) && repoBaseline30Available(repo) ? formatSigned(repoChange(repo), true) : '—'
}

function repoGrowthClass(repo: Repository): string {
  if (!repoLocAvailable(repo) || !repoBaseline30Available(repo)) return 'number-cell'
  return repoChange(repo) >= 0 ? 'number-cell positive' : 'number-cell negative'
}

function repoGrowthTitle(repo: Repository): string {
  if (!repoLocAvailable(repo)) return 'Lines are not available for this repository'
  if (!repoBaseline30Available(repo)) return 'No 30-day baseline available'
  const current = repoTotal(repo)
  const change = repoChange(repo)
  const percent = repoChangePercent(repo)
  const start = current - change
  return `30-day change\nStart ${formatCount(start)}\nCurrent ${formatCount(current)}\nChange ${formatSigned(change)}\nGrowth ${percent.toFixed(1)}%`
}

function detailLocValue(repo: Repository, count: number): string {
  return repoLocAvailable(repo) ? formatCount(count) : '—'
}

function detailChangeValue(repo: Repository): string {
  return repoLocAvailable(repo) && repoBaseline30Available(repo) ? formatSigned(repoChange(repo), true) : '—'
}

function ActivitySidebar({ kind, state, repository, lockedRepository, repositories, prs, issues, onKind, onState, onRepository, onOpenUrl }: { kind: FeedKind; state: FeedState; repository: string; lockedRepository?: string | null; repositories: Repository[]; prs: PullRequest[]; issues: Issue[]; onKind: (kind: FeedKind) => void; onState: (state: FeedState) => void; onRepository: (repository: string) => void; onOpenUrl: (url: string | undefined) => void }) {
  const feed = kind === 'prs' ? prs : issues
  const selectedRepository = lockedRepository ?? repository
  return <aside className="activity-sidebar"><div className="sidebar-sticky"><div className="sidebar-heading"><div><p className="eyebrow">Live feed</p><h2>Recent activity</h2></div><span className="feed-count">{feed.length}</span></div><div className="feed-tabs" role="tablist"><button className={kind === 'prs' ? 'active' : ''} onClick={() => onKind('prs')}>Pull requests</button><button className={kind === 'issues' ? 'active' : ''} onClick={() => onKind('issues')}>Issues</button></div><div className="feed-filters"><div className="filter-pills">{(kind === 'prs' ? (['all', 'open', 'merged'] as FeedState[]) : (['all', 'open', 'closed'] as FeedState[])).map((key) => <button key={key} className={state === key ? 'active' : ''} onClick={() => onState(key)}>{key[0].toUpperCase() + key.slice(1)}</button>)}</div><label className="select-wrap"><ListFilter size={14} /><select value={selectedRepository} disabled={Boolean(lockedRepository)} onChange={(event) => onRepository(event.target.value)} aria-label="Filter by repository"><option value="all">All repositories</option>{repositories.map((repo) => <option key={String(repositoryId(repo))} value={String(repositoryId(repo))}>{repositoryLabel(repo)}</option>)}</select><ChevronDown size={14} /></label></div></div><div className="feed-list">{feed.length ? feed.map((item) => kind === 'prs' ? <PullRequestItem key={`${activityRepo(item)}-${item.number}`} item={item} onOpen={() => onOpenUrl(item.url)} /> : <IssueItem key={`${activityRepo(item)}-${item.number}`} item={item} onOpen={() => onOpenUrl(item.url)} />) : <div className="feed-empty"><CircleDot size={21} /><p>No {kind === 'prs' ? 'pull requests' : 'issues'} match these filters</p><span>Activity will appear here after the next sync.</span></div>}</div></aside>
}

function PullRequestItem({ item, onOpen }: { item: PullRequest; onOpen: () => void }) {
  const state = statusText(item)
  const date = item.updated_at ?? item.updatedAt ?? item.created_at ?? item.createdAt
  const ciState = String(item.ci_state ?? item.ciState ?? '').toLowerCase()
  const CiIcon = ciState === 'success' ? Check : ciState === 'failure' ? X : ciState === 'pending' ? LoaderCircle : null
  return <button className="feed-item" onClick={onOpen}><div className="feed-item-top"><span className="feed-repo">{activityRepo(item) || 'Repository'} <b>#{item.number}</b></span><ExternalLink size={13} /></div><strong className="feed-title">{item.title}</strong><div className="feed-meta"><span className={`badge ${state.toLowerCase()}`}>{state}</span><span title={exactDate(date)}>{relativeTime(date)}</span></div><div className="feed-detail"><span className="diff positive">+{formatCount(item.additions ?? 0)}</span><span className="diff negative">−{formatCount(item.deletions ?? 0)}</span><span>{item.changed_files ?? item.changedFiles ?? 0} files</span>{CiIcon ? <span className={`ci-state ${ciState}`}><CiIcon size={12} className={ciState === 'pending' ? 'spin' : ''} /> CI {ciState}</span> : null}</div></button>
}

function IssueItem({ item, onOpen }: { item: Issue; onOpen: () => void }) {
  const state = String(item.state || 'open').toUpperCase()
  const date = item.updated_at ?? item.updatedAt ?? item.created_at ?? item.createdAt
  const labels = normalizeLabels(item.labels, item.labels_json ?? item.labelsJson)
  return <button className="feed-item issue-item" onClick={onOpen}><div className="feed-item-top"><span className="feed-repo">{activityRepo(item) || 'Repository'} <b>#{item.number}</b></span><ExternalLink size={13} /></div><strong className="feed-title">{item.title}</strong><div className="feed-meta"><span className={`badge ${state.toLowerCase()}`}>{state}</span><span title={exactDate(date)}>Updated {relativeTime(date)}</span></div>{labels.length ? <div className="label-row">{labels.slice(0, 4).map((label) => <span key={label}>{label}</span>)}</div> : null}</button>
}

function RepositoryDetails({ repo, history, historyLoading, metric, range, onMetric, onRange, onBack, pullRequests, issues, onOpenUrl }: { repo: Repository; history: { date: string; timestamp: number; total: number; source: number; tests: number }[]; historyLoading: boolean; metric: 'total' | 'source' | 'tests'; range: TimeRange; onMetric: (metric: 'total' | 'source' | 'tests') => void; onRange: (range: TimeRange) => void; onBack: () => void; pullRequests: PullRequest[]; issues: Issue[]; onOpenUrl: (url: string | undefined) => void }) {
  const isPrivate = repo.is_private ?? repo.isPrivate ?? false
  return <main className="main-column detail-column"><button className="back-button" onClick={onBack}><ArrowLeft size={15} /> Back to portfolio</button><div className="detail-heading"><div><p className="eyebrow">Repository detail</p><h1>{repoName(repo)}</h1><p className="detail-subtitle">{repositoryLabel(repo)} {repo.primary_language ?? repo.primaryLanguage ? <><span>·</span> {repo.primary_language ?? repo.primaryLanguage}</> : null} <em className={`repo-visibility ${isPrivate ? 'private' : 'public'}`}>{isPrivate ? 'Private' : 'Public'}</em></p></div>{repo.url && <button className="button secondary" onClick={() => onOpenUrl(repo.url)}><Github size={15} /> Open on GitHub <ExternalLink size={14} /></button>}</div><div className="detail-stats"><DetailStat label="Total Lines" value={detailLocValue(repo, repoTotal(repo))} /><DetailStat label="Source Lines" value={detailLocValue(repo, repoSource(repo))} /><DetailStat label="Test Lines" value={detailLocValue(repo, repoTests(repo))} /><DetailStat label="30d net change" value={detailChangeValue(repo)} tone={repoLocAvailable(repo) && repoBaseline30Available(repo) ? (repoChange(repo) >= 0 ? 'positive' : 'negative') : undefined} title={repoGrowthTitle(repo)} /><DetailStat label="Open PRs" value={formatCount(repoOpenPrs(repo))} /><DetailStat label="Open issues" value={formatCount(repoOpenIssues(repo))} /><DetailStat label="Stars" value={formatCount(repoStars(repo))} /><DetailStat label="Forks" value={formatCount(repoForks(repo))} /><DetailStat label="Last activity" value={relativeTime(repoActivity(repo))} title={exactDate(repoActivity(repo))} /></div>{historyLoading ? <div className="detail-loading"><LoaderCircle size={16} className="spin" /> Loading repository history…</div> : <LocChart history={history} metric={metric} range={range} onMetric={onMetric} onRange={onRange} />}<div className="detail-activity"><ActivityList title="Recent pull requests" items={pullRequests.slice(0, 5)} kind="prs" onOpenUrl={onOpenUrl} /><ActivityList title="Recent issues" items={issues.slice(0, 5)} kind="issues" onOpenUrl={onOpenUrl} /></div></main>
}

function DetailStat({ label, value, tone, title }: { label: string; value: string; tone?: string; title?: string }) {
  return <div className="detail-stat"><span>{label}</span><strong className={tone ?? ''} title={title}>{value}</strong></div>
}

function ActivityList({ title, items, kind, onOpenUrl }: { title: string; items: Array<PullRequest | Issue>; kind: FeedKind; onOpenUrl: (url: string | undefined) => void }) {
  return <section className="detail-list"><div className="list-title"><h3>{title}</h3><span>{items.length}</span></div>{items.length ? items.map((item) => kind === 'prs' ? <PullRequestItem item={item as PullRequest} key={item.number} onOpen={() => onOpenUrl(item.url)} /> : <IssueItem item={item as Issue} key={item.number} onOpen={() => onOpenUrl(item.url)} />) : <div className="detail-empty">No recent activity</div>}</section>
}

function SettingsDrawer({ settings, saving, error, onSave, onClose }: { settings: AppSettings; saving: boolean; error: string | null; onSave: (settings: AppSettings) => void; onClose: () => void }) {
  const [draft, setDraft] = useState<AppSettings>(settings)
  return <div className="drawer-backdrop" onClick={onClose}><aside className="settings-drawer" onClick={(event) => event.stopPropagation()}><div className="drawer-heading"><div><p className="eyebrow">Local settings</p><h2>Settings</h2></div><button className="icon-button" type="button" aria-label="Close settings" onClick={onClose}><X size={17} /></button></div><div className="setting-block"><span className="setting-label">Refresh cadence</span><p>Activity refreshes keep pull requests and issues current. Line counts run less often and can also run when a repository changes.</p><label className="setting-control"><span>Activity refresh</span><select value={draft.activity_refresh_minutes} onChange={(event) => setDraft((current) => ({ ...current, activity_refresh_minutes: Number(event.target.value) }))}><option value={1}>Every 1 minute</option><option value={2}>Every 2 minutes</option><option value={5}>Every 5 minutes</option><option value={10}>Every 10 minutes</option><option value={15}>Every 15 minutes</option></select></label><label className="setting-control"><span>Line count refresh</span><select value={draft.lines_refresh_minutes} onChange={(event) => setDraft((current) => ({ ...current, lines_refresh_minutes: Number(event.target.value) }))}><option value={30}>Every 30 minutes</option><option value={45}>Every 45 minutes</option><option value={60}>Every 60 minutes</option></select></label><label className="setting-checkbox"><input type="checkbox" checked={draft.refresh_lines_on_change} onChange={(event) => setDraft((current) => ({ ...current, refresh_lines_on_change: event.target.checked }))} /><span>Refresh line counts when code changes</span></label>{error && <div className="settings-error" role="alert"><AlertCircle size={14} /> {error}</div>}<button className="button primary wide settings-save" type="button" disabled={saving} onClick={() => onSave(draft)}>{saving ? <><LoaderCircle size={15} className="spin" /> Saving…</> : 'Save settings'}</button></div><div className="setting-block"><span className="setting-label">Data storage</span><p>Repositories, snapshots, and activity are stored in the application data directory.</p><span className="setting-value"><HardDrive size={14} /> Local SQLite cache</span></div><div className="setting-block"><span className="setting-label">Line classifier</span><p>Source and test paths follow the backend classifier rules. Per-repository overrides are stored locally by the backend when supported.</p><span className="setting-value"><GitBranch size={14} /> Backend-managed classification rules</span></div><div className="setting-block"><span className="setting-label">GitHub connection</span><p>The app uses your existing GitHub CLI login and never stores an authentication token.</p></div></aside></div>
}

export default App
