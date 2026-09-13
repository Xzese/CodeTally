import { listen } from '@tauri-apps/api/event'
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { AreaChart, CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { AlertCircle, ArrowDown, ArrowLeft, ArrowUp, BarChart3, Check, ChevronDown, ChevronUp, CircleDot, ExternalLink, GitBranch, Github, HardDrive, ListFilter, LoaderCircle, RefreshCw, Settings, Terminal, X, Zap } from 'lucide-react'
import { checkDependencies, discoverRepositories, getActivityFeed, getAppSettings, getDashboard, getGithubUser, getLocHistory, getSyncProgress, openExternalUrl, setAppSettings, syncActivity, syncGithubData } from './api'
import type { AppSettings, DashboardData, DependencyStatus, FeedKind, Issue, LocSnapshot, PullRequest, Repository, SyncProgress, TimeRange } from './types'
import { aggregateHistory, filterIssues, filterPullRequests, isDraft, isMerged, sortRepositories, type FeedState } from './model'
import { activityRepo, exactDate, formatCompact, formatCount, formatSigned, normalizeDashboard, normalizeLabels, relativeTime, repoActivity, repoBaseline30Available, repoChange, repoChangePercent, repoForks, repoLocAvailable, repoName, repoOpenIssues, repoOpenPrs, repoSource, repoStars, repoTests, repoTotal, repositoryId, repositoryLabel } from './utils'
import codetallyMark from './assets/codetally-mark.png'
import SettingsDrawer from './SettingsDrawer'
import { DEFAULT_APP_SETTINGS, normalizeAppSettings } from './settings'
import GitHubConnection from './GitHubConnection'
import ActivityRepositoryMenu, { activityScopeRepositories } from './ActivityRepositoryMenu'
import './styles.css'

type Screen = 'dashboard' | 'detail'
type RepoSort = 'name' | 'loc' | 'source' | 'tests' | 'growth' | 'prs' | 'issues' | 'stars' | 'forks' | 'activity'

const EMPTY_DEPS: DependencyStatus = { gh: false, git: false, tokei: false, gh_authenticated: false, authenticated: false, login: null }

function dependenciesAuthenticated(deps: DependencyStatus): boolean {
  return deps.gh_authenticated || deps.authenticated === true
}

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
  const [checkingDependencies, setCheckingDependencies] = useState(false)
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
  const [settingsLoaded, setSettingsLoaded] = useState(false)
  const [settingsSaving, setSettingsSaving] = useState(false)
  const [settingsError, setSettingsError] = useState<string | null>(null)
  const settingsIntentRef = useRef(DEFAULT_APP_SETTINGS)
  const settingsPersistedRef = useRef(DEFAULT_APP_SETTINGS)
  const settingsRevisionRef = useRef(0)
  const selectionRevisionRef = useRef(0)
  const settingsPendingRef = useRef(false)
  const settingsWorkerRef = useRef(false)
  const settingsRefreshRef = useRef(false)
  const [systemDark, setSystemDark] = useState(() => window.matchMedia?.('(prefers-color-scheme: dark)').matches ?? true)
  useEffect(() => {
    const preference = window.matchMedia?.('(prefers-color-scheme: dark)')
    if (!preference) return
    const update = () => setSystemDark(preference.matches)
    update()
    preference.addEventListener('change', update)
    return () => preference.removeEventListener('change', update)
  }, [])
  const lightMode = appSettings.theme_mode === 'light' || (appSettings.theme_mode === 'system' && !systemDark)
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
  const partialLoc = useMemo(() => activeRepositories.some((repo) => (appSettings.include_forks_in_totals || !(repo.is_fork ?? repo.isFork)) && !repoLocAvailable(repo)), [activeRepositories, appSettings.include_forks_in_totals])
  const selectedRepoId = useMemo(() => selectedRepo ? repositoryId(selectedRepo) : null, [selectedRepo])
  const lastSync = dashboard.last_sync_at

  const applyDashboard = useCallback((raw: DashboardData, reloadFeeds = true, preserveFeedCache = true) => {
    const next = toDashboard(raw)
    setDashboard((current) => ({
      ...next,
      pull_requests: preserveFeedCache && current.pull_requests.length ? current.pull_requests : next.pull_requests,
      issues: preserveFeedCache && current.issues.length ? current.issues : next.issues
    }))
    if (reloadFeeds) setFeedRevision((revision) => revision + 1)
    setSelectedRepo((previous) => previous ? next.repositories.find((repo) => String(repositoryId(repo)) === String(repositoryId(previous))) ?? previous : previous)
    if (next.user?.login) setLogin(next.user.login)
    const repositoryErrors = next.repositories.filter((repo) => repo.last_error).map((repo) => `${repositoryLabel(repo)}: ${repo.last_error}`).slice(0, 3)
    const errors = [...new Set([...next.errors, ...repositoryErrors])]
    setError(errors.length ? errors.join(' ') : null)
  }, [])

  const loadData = useCallback(async (showBusy = false, preserveFeedCache = true): Promise<boolean> => {
    if (showBusy) setBusy(true)
    const selectionRevision = selectionRevisionRef.current
    const dashboardVersion = cachedRefreshVersionRef.current
    const dashboardResult = await Promise.allSettled([getDashboard()])
    let hasCachedRepositories = false
    if (dashboardResult[0].status === 'fulfilled') {
      const next = dashboardResult[0].value
      const historyResult = await Promise.allSettled([getLocHistory({ range: 'ALL' })])
      const history = historyResult[0].status === 'fulfilled' ? historyResult[0].value : []
      if (selectionRevision !== selectionRevisionRef.current || dashboardVersion !== cachedRefreshVersionRef.current || settingsRefreshRef.current) { setBusy(false); return false }
      applyDashboard({ ...next, loc_history: next.loc_history ?? next.locHistory ?? next.history ?? history }, true, preserveFeedCache)
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

  const checkSetupConnection = useCallback(async () => {
    if (checkingDependencies) return
    setCheckingDependencies(true)
    setError(null)
    try {
      const next = await checkDependencies()
      setDeps(next)
      if (next.login) setLogin(next.login)
    } catch (reason) {
      const message = reason instanceof Error ? reason.message : String(reason)
      setDeps((current) => ({ ...current, error: message }))
      setError(`Could not check GitHub connection: ${message}`)
    } finally {
      setCheckingDependencies(false)
    }
  }, [checkingDependencies])

  const loadFinalData = useCallback(async (preserveFeedCache = true) => {
    finalRefreshRef.current = true
    cachedRefreshVersionRef.current += 1
    try {
      await loadData(false, preserveFeedCache)
    } finally {
      finalRefreshRef.current = false
    }
  }, [loadData])

  const refreshCachedData = useCallback(async () => {
    if (cachedRefreshRef.current || finalRefreshRef.current) return
    cachedRefreshRef.current = true
    const selectionRevision = selectionRevisionRef.current
    const requestVersion = ++cachedRefreshVersionRef.current
    try {
      const [dashboardResult, historyResult] = await Promise.allSettled([getDashboard(), getLocHistory({ range: 'ALL' })])
      if (selectionRevision !== selectionRevisionRef.current || settingsRefreshRef.current || finalRefreshRef.current || requestVersion !== cachedRefreshVersionRef.current || dashboardResult.status !== 'fulfilled') return
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
      await loadFinalData()
      if (failure) setError(failure)
    } catch (reason) {
      if (!automatic) await loadFinalData()
      setError(reason instanceof Error ? reason.message : String(reason))
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

  const loadSettings = useCallback(async () => {
    setSettingsError(null)
    try {
      const settings = await getAppSettings()
      if (settingsRevisionRef.current !== 0) return
      const initial = normalizeAppSettings(settings)
      settingsIntentRef.current = initial
      settingsPersistedRef.current = initial
      setAppSettingsState(initial)
      setSettingsLoaded(true)
    } catch (reason) {
      setSettingsError(`Could not load preferences: ${reason instanceof Error ? reason.message : String(reason)}`)
    }
  }, [])

  useEffect(() => { void loadSettings() }, [loadSettings])

  // Scheduling belongs to the native process, which keeps running with the window hidden.
  useEffect(() => {
    let alive = true
    let polling = false
    let wasRunning = false
    const poll = async () => {
      if (polling || syncingRef.current || importingRef.current) return
      polling = true
      try {
        const progress = await getSyncProgress()
        if (!alive || syncingRef.current || importingRef.current) return
        setSyncProgress(progress)
        if (progress.running) {
          wasRunning = true
          await refreshCachedData()
        } else if (wasRunning) {
          wasRunning = false
          await loadFinalData()
        }
      } catch { /* Keep the cached dashboard available. */ }
      finally { polling = false }
    }
    const reloadCached = () => { void refreshCachedData(); setFeedRevision((revision) => revision + 1); void poll() }
    let unlisten: (() => void) | undefined
    void listen('background-sync-completed', () => { wasRunning = false; void loadFinalData(); void poll() }).then((stop) => {
      if (alive) unlisten = stop
      else stop()
    }).catch(() => undefined)
    const onFocus = reloadCached
    const timer = window.setInterval(() => void poll(), 2000)
    window.addEventListener('focus', onFocus)
    return () => { alive = false; unlisten?.(); window.clearInterval(timer); window.removeEventListener('focus', onFocus) }
  }, [loadFinalData, refreshCachedData])

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
      setImportStep('Loading local line history')
      await loadFinalData()
      if (failure) setError(failure)
    } catch (reason) {
      await loadFinalData()
      setError(reason instanceof Error ? reason.message : String(reason))
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
    if (screen === 'detail' && selectedRepo) setFeedRepo(String(repositoryId(selectedRepo)))
  }, [screen, selectedRepo])

  const handleFeedState = useCallback((state: FeedState) => {
    setFeedState(state)
  }, [])

  const handleFeedRepository = useCallback((repository: string) => {
    if (screen === 'detail' && selectedRepo) return
    setFeedRepo(repository)
  }, [screen, selectedRepo])

  const scopedFeedRepositories = useMemo(() => activityScopeRepositories(feedRepo, activeRepositories, login), [feedRepo, activeRepositories, login])
  const feedRepositoryIdsKey = JSON.stringify(scopedFeedRepositories.map((repo) => backendRepositoryId(String(repositoryId(repo)))).filter((id): id is number => id !== null))
  const groupedFeedScope = feedRepo === 'personal' || feedRepo === 'companies' || feedRepo.startsWith('owner:')
  useEffect(() => {
    const requestId = ++feedRequestRef.current
    let alive = true
    void getActivityFeed({ kind: feedKind, state: feedState, repositoryId: backendRepositoryId(feedRepo), ...(groupedFeedScope ? { repositoryIds: JSON.parse(feedRepositoryIdsKey) as number[] } : {}) }).then((items) => {
      if (!alive || requestId !== feedRequestRef.current) return
      if (feedKind === 'prs') setDashboard((current) => ({ ...current, pull_requests: items as PullRequest[] }))
      else setDashboard((current) => ({ ...current, issues: items as Issue[] }))
    }).catch((reason) => {
      if (!alive || requestId !== feedRequestRef.current) return
      const message = reason instanceof Error ? reason.message : String(reason)
      setError(`Unable to load ${feedKind === 'prs' ? 'pull requests' : 'issues'}: ${message}`)
    })
    return () => { alive = false }
  }, [feedKind, feedRepo, feedRevision, feedState, groupedFeedScope, feedRepositoryIdsKey])

  const visibleHistory = useMemo(() => aggregateHistory(
    screen === 'detail' && detailHistory !== null ? detailHistory : dashboard.loc_history,
    screen === 'detail' && detailHistory !== null ? null : screen === 'detail' && selectedRepo ? repositoryId(selectedRepo) : null,
    range
  ), [dashboard.loc_history, detailHistory, range, screen, selectedRepo])

  const metrics = dashboard.metrics
  const sortedRepositories = useMemo(() => sortRepositories(activeRepositories, repoSort, repoSortDirection), [activeRepositories, repoSort, repoSortDirection])
  const filteredPrs = useMemo(() => filterPullRequests(dashboard.pull_requests, feedState, groupedFeedScope ? 'all' : feedRepo).filter((item) => !groupedFeedScope || scopedFeedRepositories.some((repo) => isRepoActivity(item, repo))), [dashboard.pull_requests, feedRepo, feedState, groupedFeedScope, scopedFeedRepositories])
  const filteredIssues = useMemo(() => filterIssues(dashboard.issues, feedState === 'merged' ? 'all' : feedState, groupedFeedScope ? 'all' : feedRepo).filter((item) => !groupedFeedScope || scopedFeedRepositories.some((repo) => isRepoActivity(item, repo))), [dashboard.issues, feedRepo, feedState, groupedFeedScope, scopedFeedRepositories])
  const hasSavedRepositorySelection = !appSettings.include_personal_repositories || !appSettings.include_company_repositories || appSettings.excluded_repository_ids.length > 0
  const isSetup = activeRepositories.length === 0 && !busy && settingsLoaded && !hasSavedRepositorySelection
  const syncActive = syncing || importing || Boolean(syncProgress?.running)
  const syncDetailsAvailable = syncActive || Boolean(syncProgress)
  const syncPopoverProgress = syncProgress ?? { running: true, phase: importing ? 'importing' : 'starting', current: 0, total: 0, message: importing ? importStep : 'Starting sync' }
  const flushSettings = useCallback(async () => {
    if (settingsWorkerRef.current) return
    settingsWorkerRef.current = true
    setSettingsSaving(true)
    setSettingsError(null)
    try {
      while (settingsPendingRef.current) {
        settingsPendingRef.current = false
        const next = settingsIntentRef.current
        const previous = settingsPersistedRef.current
        const persisted = await setAppSettings(next)
        const saved = normalizeAppSettings({ ...next, ...(persisted ?? {}) })
        settingsPersistedRef.current = saved
        const oldExcluded = new Set(previous.excluded_repository_ids.map(String))
        const newExcluded = new Set(saved.excluded_repository_ids.map(String))
        settingsRefreshRef.current ||= previous.include_personal_repositories !== saved.include_personal_repositories || previous.include_company_repositories !== saved.include_company_repositories || previous.include_forks_in_totals !== saved.include_forks_in_totals || oldExcluded.size !== newExcluded.size || [...oldExcluded].some((id) => !newExcluded.has(id))
        // Acknowledgements never replace optimistic intent. A newer click may have
        // arrived while this write was in flight; only flush its latest snapshot.
        if (settingsPendingRef.current || !settingsRefreshRef.current) continue
        const revision = settingsRevisionRef.current
        cachedRefreshVersionRef.current += 1
        const [raw, history] = await Promise.all([getDashboard(), getLocHistory({ range: 'ALL' })])
        if (revision !== settingsRevisionRef.current) continue
        settingsRefreshRef.current = false
        cachedRefreshVersionRef.current += 1
        setScreen('dashboard')
        setSelectedRepo(null)
        setDetailHistory(null)
        setDetailPrs([])
        setDetailIssues([])
        setFeedRepo('all')
        setFeedState('open')
        feedRequestRef.current += 1
        applyDashboard({ ...raw, loc_history: raw.loc_history ?? raw.locHistory ?? raw.history ?? history }, true, false)
      }
    } catch (reason) {
      // Keep the newest complete intent across errors and drawer unmounts. Retry
      // repeats it, including a cache reload that failed after a successful write.
      settingsPendingRef.current = true
      setSettingsError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      settingsWorkerRef.current = false
      setSettingsSaving(false)
    }
  }, [applyDashboard])

  const changeSettings = useCallback((update: (current: AppSettings) => AppSettings) => {
    const previous = settingsIntentRef.current
    const next = update(previous)
    const oldExcluded = new Set(previous.excluded_repository_ids.map(String))
    const newExcluded = new Set(next.excluded_repository_ids.map(String))
    const selectionChanged = previous.include_personal_repositories !== next.include_personal_repositories || previous.include_company_repositories !== next.include_company_repositories || previous.include_forks_in_totals !== next.include_forks_in_totals || oldExcluded.size !== newExcluded.size || [...oldExcluded].some((id) => !newExcluded.has(id))
    if (selectionChanged) {
      selectionRevisionRef.current += 1
      cachedRefreshVersionRef.current += 1
      settingsRefreshRef.current = true
    }
    settingsIntentRef.current = next
    settingsRevisionRef.current += 1
    settingsPendingRef.current = true
    setAppSettingsState(next)
    void flushSettings()
  }, [flushSettings])

  return (
    <div className={lightMode ? 'app-shell light' : 'app-shell'}>
      <header className="topbar">
        <div className="brand-mark"><img src={codetallyMark} alt="" aria-hidden="true" /><span>CodeTally</span></div>
        <div className="topbar-status">
          {login ? <span className="identity"><span className="status-dot" />{login}</span> : <span className="muted">Local desktop dashboard</span>}
          <button className="icon-button" title="Settings" onClick={() => setSettingsOpen(true)}><Settings size={17} /></button>
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

      {busy && !dashboard.repositories.length ? <LoadingScreen /> : isSetup ? <SetupScreen deps={deps} login={login} error={error} importing={importing} importStep={importStep} checking={checkingDependencies} onImport={() => void importRepositories()} onRetry={checkSetupConnection} /> : (
        <div className="content-shell">
          {screen === 'dashboard' ? (
            <main className="main-column">
              {deps.gh && !dependenciesAuthenticated(deps) && <GitHubConnection deps={deps} login={login} compact checking={checkingDependencies} onRetry={checkSetupConnection} />}
              {activeRepositories.length === 0 ? <EmptySelectionState onOpenSettings={() => setSettingsOpen(true)} /> : <>
                <div className="page-heading"><div><p className="eyebrow">Portfolio overview</p><h1>Code at a glance</h1></div><span className="small-note"><CircleDot size={13} /> {activeRepositories.length} active repositories</span></div>
                <SummaryStrip metrics={metrics} partialLoc={partialLoc} />
                <LocChart history={visibleHistory} metric={metric} range={range} onMetric={setMetric} onRange={setRange} />
                <RepositoryTable repositories={sortedRepositories} login={login} sort={repoSort} direction={repoSortDirection} onSort={(next) => { if (next === repoSort) setRepoSortDirection((current) => current === 'asc' ? 'desc' : 'asc'); else { setRepoSort(next); setRepoSortDirection(next === 'name' ? 'asc' : 'desc') } }} onSelect={(repo) => void openRepository(repo)} />
              </>}
            </main>
          ) : selectedRepo ? (
            <RepositoryDetails repo={selectedRepo} history={visibleHistory} historyLoading={detailLoading} metric={metric} range={range} onMetric={setMetric} onRange={setRange} onBack={backToDashboard} pullRequests={detailPrs} issues={detailIssues} onOpenUrl={(url) => void openUrl(url, setError)} />
          ) : null}
          <ActivitySidebar login={login} kind={feedKind} state={feedState} repository={feedRepo} lockedRepository={screen === 'detail' && selectedRepo ? String(repositoryId(selectedRepo)) : null} repositories={activeRepositories} prs={filteredPrs} issues={filteredIssues} onKind={handleFeedKind} onState={handleFeedState} onRepository={handleFeedRepository} onOpenUrl={(url) => void openUrl(url, setError)} />
        </div>
      )}

      {settingsOpen && <SettingsDrawer settings={appSettings} loaded={settingsLoaded} metrics={dashboard.metrics} saving={settingsSaving} error={settingsError} onChange={changeSettings} onRetry={() => void (settingsLoaded ? flushSettings() : loadSettings())} onClose={() => setSettingsOpen(false)} />}
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

function SetupScreen({ deps, login, error, importing, importStep, checking, onImport, onRetry }: { deps: DependencyStatus; login: string; error: string | null; importing: boolean; importStep: string; checking: boolean; onImport: () => void; onRetry: () => void | Promise<void> }) {
  const authenticated = dependenciesAuthenticated(deps)
  const checks = [
    { label: 'Git installed', ok: deps.git, icon: GitBranch },
    { label: 'GitHub CLI installed', ok: deps.gh, icon: Terminal },
    { label: authenticated ? `Logged in as ${login || deps.login}` : 'GitHub authentication', ok: authenticated, icon: Github },
    { label: 'Tokei installed', ok: deps.tokei, icon: Zap }
  ]
  const ready = checks.every((check) => check.ok)
  const missingTools = [!deps.git ? 'Git' : null, !deps.tokei ? 'Tokei' : null].filter((tool): tool is string => Boolean(tool))
  return <div className="setup-wrap"><div className="setup-card"><div className="setup-icon"><img src={codetallyMark} alt="" aria-hidden="true" /></div><p className="eyebrow">First run</p><GitHubConnection deps={deps} login={login} checking={checking} onRetry={onRetry} /><div className="setup-checks">{checks.map(({ label, ok, icon: Icon }) => <div className={ok ? 'setup-check ok' : 'setup-check'} key={label}><span className="check-icon">{ok ? <Check size={15} /> : <Icon size={15} />}</span><span>{label}</span>{ok ? <span className="check-state">Ready</span> : <span className="check-state missing">Missing</span>}</div>)}</div>{missingTools.length ? <p className="github-missing-tools">Missing local dependencies: {missingTools.join(' and ')}. Install them, then use Check connection to verify again.</p> : null}{error && <div className="setup-error"><AlertCircle size={15} /> {error}</div>}{importing ? <div className="setup-import-note" role="status"><LoaderCircle size={15} className="spin" /><span>{importStep}</span><small>Follow progress from the Importing status in the header.</small></div> : ready ? <button className="button primary wide" onClick={onImport}><HardDrive size={16} /> {error ? 'Retry import' : 'Import repositories'}</button> : null}</div></div>
}

function EmptySelectionState({ onOpenSettings }: { onOpenSettings: () => void }) {
  return <section className="empty-selection" aria-labelledby="empty-selection-heading"><div className="empty-selection-icon"><ListFilter size={20} /></div><p className="eyebrow">Portfolio paused</p><h1 id="empty-selection-heading">No repositories selected</h1><p>Choose repositories in Settings to bring them back to the dashboard. Nothing was deleted: your local history and repository cache are retained, and any in-flight work may finish.</p><button className="button primary" type="button" onClick={onOpenSettings}><Settings size={15} /> Open Settings</button></section>
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

function RepositoryTable({ repositories, login, sort, direction, onSort, onSelect }: { repositories: Repository[]; login: string; sort: RepoSort; direction: 'asc' | 'desc'; onSort: (sort: RepoSort) => void; onSelect: (repo: Repository) => void }) {
  const [hiddenIds, setHiddenIds] = useState<Set<string>>(() => new Set())
  const visibleRepositories = repositories.filter((repo) => !hiddenIds.has(String(repositoryId(repo))))
  const setVisible = (ids: string[], visible: boolean) => setHiddenIds((current) => {
    const next = new Set(current)
    for (const id of ids) { if (visible) next.delete(id); else next.add(id) }
    return next
  })
  const columns: Array<{ key: RepoSort; label: string; title: string }> = [{ key: 'name', label: 'Repository', title: 'Repository' }, { key: 'loc', label: 'Total', title: 'Total lines' }, { key: 'source', label: 'Source', title: 'Source lines' }, { key: 'tests', label: 'Tests', title: 'Test lines' }, { key: 'growth', label: '30d', title: '30-day change' }, { key: 'prs', label: 'PRs', title: 'Open pull requests' }, { key: 'issues', label: 'Issues', title: 'Open issues' }, { key: 'stars', label: 'Stars', title: 'GitHub stars' }, { key: 'forks', label: 'Forks', title: 'GitHub forks' }, { key: 'activity', label: 'Activity', title: 'Last activity' }]
  return <section className="repos-panel"><div className="panel-heading table-heading"><div><p className="eyebrow">Inventory</p><h2>Repositories</h2></div><TrackedRepositoryMenu repositories={repositories} login={login} hiddenIds={hiddenIds} onVisibilityChange={setVisible} onShowAll={() => setHiddenIds(new Set())} /></div>{visibleRepositories.length ? <div className="table-scroll"><table><thead><tr>{columns.map((column) => <th key={column.key}><button className={sort === column.key ? 'sort-button active' : 'sort-button'} title={`Sort by ${column.title}`} onClick={() => onSort(column.key)}>{column.label}{sort === column.key && (direction === 'asc' ? <ChevronUp size={13} /> : <ChevronDown size={13} />)}</button></th>)}</tr></thead><tbody>{visibleRepositories.map((repo) => { const isPrivate = repo.is_private ?? repo.isPrivate ?? false; return <tr key={String(repositoryId(repo))} onClick={() => onSelect(repo)} tabIndex={0} onKeyDown={(event) => { if (event.key === 'Enter') onSelect(repo) }}><td><div className="repo-cell"><span className="repo-glyph"><GitBranch size={14} /></span><span><strong>{repoName(repo)}</strong><small><span className="repo-meta-text">{repo.owner ?? repositoryLabel(repo).split('/')[0]}{repo.primary_language ?? repo.primaryLanguage ? ` · ${repo.primary_language ?? repo.primaryLanguage}` : ''}</span><em className={`repo-visibility ${isPrivate ? 'private' : 'public'}`}>{isPrivate ? 'Private' : 'Public'}</em></small></span></div></td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoTotal(repo))}</td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoSource(repo))}</td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoTests(repo))}</td><td className={repoGrowthClass(repo)} title={repoGrowthTitle(repo)}>{repoChangeDisplay(repo)}</td><td className="number-cell">{formatCount(repoOpenPrs(repo))}</td><td className="number-cell">{formatCount(repoOpenIssues(repo))}</td><td className="number-cell">{formatCount(repoStars(repo))}</td><td className="number-cell">{formatCount(repoForks(repo))}</td><td><span className="relative" title={exactDate(repoActivity(repo))}>{relativeTime(repoActivity(repo))}</span></td></tr> })}</tbody></table></div> : <div className="empty-state"><GitBranch size={22} /><p>{repositories.length ? 'All repositories hidden from this table' : 'No repositories in the portfolio'}</p></div>}</section>
}

function SelectionCheckbox({ label, checked, mixed = false, disabled = false, onChange }: { label: string; checked: boolean; mixed?: boolean; disabled?: boolean; onChange: (checked: boolean) => void }) {
  const ref = useRef<HTMLInputElement>(null)
  useEffect(() => { if (ref.current) ref.current.indeterminate = mixed }, [mixed])
  return <input ref={ref} className="mac-checkbox" type="checkbox" aria-label={label} aria-checked={mixed ? 'mixed' : checked} checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
}

function TrackedRepositoryMenu({ repositories, login, hiddenIds, onVisibilityChange, onShowAll }: { repositories: Repository[]; login: string; hiddenIds: Set<string>; onVisibilityChange: (ids: string[], visible: boolean) => void; onShowAll: () => void }) {
  const [open, setOpen] = useState(false)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const containerRef = useRef<HTMLDivElement>(null)
  const triggerRef = useRef<HTMLButtonElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  const [panelStyle, setPanelStyle] = useState<CSSProperties>({ position: 'fixed' })
  useLayoutEffect(() => {
    if (!open || !triggerRef.current) return
    const anchor = triggerRef.current.getBoundingClientRect()
    const margin = 12
    const gap = 8
    const width = Math.min(320, Math.max(0, window.innerWidth - margin * 2))
    const below = Math.max(0, window.innerHeight - anchor.bottom - gap - margin)
    const above = Math.max(0, anchor.top - gap - margin)
    const upward = below < 420 && above > below
    setPanelStyle({
      position: 'fixed',
      width,
      left: Math.max(margin, Math.min(anchor.right - width, window.innerWidth - width - margin)),
      right: 'auto',
      top: upward ? 'auto' : anchor.bottom + gap,
      bottom: upward ? window.innerHeight - anchor.top + gap : 'auto',
      maxHeight: Math.min(420, upward ? above : below)
    })
  }, [open])
  useEffect(() => {
    if (!open) return
    panelRef.current?.focus({ preventScroll: true })
    const dismissOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !containerRef.current?.contains(event.target)) setOpen(false)
    }
    const dismissOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') { event.preventDefault(); setOpen(false); triggerRef.current?.focus() }
    }
    const dismissOnViewportChange = () => setOpen(false)
    const dismissOnScroll = (event: Event) => {
      if (event.target instanceof Node && panelRef.current?.contains(event.target)) return
      setOpen(false)
    }
    window.addEventListener('resize', dismissOnViewportChange)
    document.addEventListener('scroll', dismissOnScroll, true)
    document.addEventListener('pointerdown', dismissOutside)
    document.addEventListener('keydown', dismissOnEscape)
    return () => { window.removeEventListener('resize', dismissOnViewportChange); document.removeEventListener('scroll', dismissOnScroll, true); document.removeEventListener('pointerdown', dismissOutside); document.removeEventListener('keydown', dismissOnEscape) }
  }, [open])
  const ownerOf = (repo: Repository) => repo.owner ?? repositoryLabel(repo).split('/')[0] ?? 'Unknown'
  const personal = repositories.filter((repo) => ownerOf(repo).toLowerCase() === login.toLowerCase())
  const companies = new Map<string, { owner: string; repositories: Repository[] }>()
  for (const repo of repositories) {
    const owner = ownerOf(repo)
    if (owner.toLowerCase() === login.toLowerCase()) continue
    const key = owner.toLowerCase()
    const group = companies.get(key) ?? { owner, repositories: [] }
    group.repositories.push(repo)
    companies.set(key, group)
  }
  const groups = [
    { key: 'personal', label: 'Personal repositories', repositories: personal },
    ...[...companies.entries()].sort(([left], [right]) => left.localeCompare(right)).map(([key, group]) => ({ key: `company-${key}`, label: `${group.owner} repositories`, repositories: group.repositories }))
  ].filter((group) => group.repositories.length)
  const visibleCount = repositories.filter((repo) => !hiddenIds.has(String(repositoryId(repo)))).length
  return <div ref={containerRef} className={open ? 'tracked-repository-menu open' : 'tracked-repository-menu'} onBlur={(event) => { if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) setOpen(false) }}>
    <button ref={triggerRef} type="button" aria-expanded={open} aria-haspopup="dialog" aria-controls="repository-view-picker" onClick={() => { if (!open) setExpanded({}); setOpen((current) => !current) }}><CircleDot size={13} /> {repositories.length} tracked <ChevronDown size={14} /></button>
    {open && <div ref={panelRef} style={panelStyle} tabIndex={-1} role="dialog" aria-label="Show repositories in table" id="repository-view-picker" className="repo-picker tracked-repository-groups">
      <div className="repo-picker-heading"><strong>Show in table</strong><span>{visibleCount} of {repositories.length} visible</span><button type="button" onClick={onShowAll} disabled={visibleCount === repositories.length}>Show all</button></div>
      <p className="repo-picker-note">Tracking continues for hidden repositories.</p>
      {groups.map((group) => {
        const groupOpen = expanded[group.key] ?? false
        const ids = group.repositories.map((repo) => String(repositoryId(repo)))
        const selected = ids.filter((id) => !hiddenIds.has(id)).length
        return <section className={groupOpen ? 'tracked-repository-group open' : 'tracked-repository-group'} key={group.key}>
          <div className="repo-picker-group-header"><label><SelectionCheckbox label={`Show ${group.label} in table`} checked={selected === ids.length} mixed={selected > 0 && selected < ids.length} onChange={(visible) => onVisibilityChange(ids, visible)} /><span>{group.label}</span></label>
            <button type="button" aria-label={`${groupOpen ? 'Collapse' : 'Expand'} ${group.label}`} aria-expanded={groupOpen} aria-controls={`view-${group.key}`} onClick={() => setExpanded((current) => ({ ...current, [group.key]: !groupOpen }))}><small>{selected}/{ids.length}</small><ChevronDown size={13} /></button></div>
          {groupOpen && <div className="repo-picker-options" id={`view-${group.key}`}>{[...group.repositories].sort((left, right) => repoName(left).localeCompare(repoName(right))).map((repo) => <label className="repo-picker-option" key={String(repositoryId(repo))}><SelectionCheckbox label={`Show ${repositoryLabel(repo)} in table`} checked={!hiddenIds.has(String(repositoryId(repo)))} onChange={(visible) => onVisibilityChange([String(repositoryId(repo))], visible)} /><span>{repoName(repo)}</span></label>)}</div>}
        </section>
      })}
    </div>}
  </div>
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

function ActivitySidebar({ login, kind, state, repository, lockedRepository, repositories, prs, issues, onKind, onState, onRepository, onOpenUrl }: { login: string; kind: FeedKind; state: FeedState; repository: string; lockedRepository?: string | null; repositories: Repository[]; prs: PullRequest[]; issues: Issue[]; onKind: (kind: FeedKind) => void; onState: (state: FeedState) => void; onRepository: (repository: string) => void; onOpenUrl: (url: string | undefined) => void }) {
  const feed = kind === 'prs' ? prs : issues
  const selectedRepository = lockedRepository ?? repository
  return <aside className="activity-sidebar"><div className="sidebar-sticky"><div className="sidebar-heading"><div><p className="eyebrow">Live feed</p><h2>Recent activity</h2></div><span className="feed-count">{feed.length}</span></div><div className="feed-tabs" role="tablist"><button className={kind === 'prs' ? 'active' : ''} onClick={() => onKind('prs')}>Pull requests</button><button className={kind === 'issues' ? 'active' : ''} onClick={() => onKind('issues')}>Issues</button></div><div className="feed-filters"><div className="filter-pills">{(kind === 'prs' ? (['all', 'open', 'merged'] as FeedState[]) : (['all', 'open', 'closed'] as FeedState[])).map((key) => <button key={key} className={state === key ? 'active' : ''} onClick={() => onState(key)}>{key[0].toUpperCase() + key.slice(1)}</button>)}</div><ActivityRepositoryMenu repositories={repositories} login={login} value={selectedRepository} disabled={Boolean(lockedRepository)} onChange={onRepository} /></div></div><div className="feed-list">{feed.length ? feed.map((item) => kind === 'prs' ? <PullRequestItem key={`${activityRepo(item)}-${item.number}`} item={item} onOpen={() => onOpenUrl(item.url)} /> : <IssueItem key={`${activityRepo(item)}-${item.number}`} item={item} onOpen={() => onOpenUrl(item.url)} />) : <div className="feed-empty"><CircleDot size={21} /><p>No {kind === 'prs' ? 'pull requests' : 'issues'} match these filters</p><span>Activity will appear here after the next sync.</span></div>}</div></aside>
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


export default App
