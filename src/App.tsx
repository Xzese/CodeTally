import { listen } from '@tauri-apps/api/event'
import { lazy, memo, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from 'react'
import { AlertCircle, ArrowDown, ArrowLeft, ArrowUp, CodeXml, Database, FlaskConical, Home, PanelsTopLeft, Check, ChevronDown, ChevronUp, CircleCheck, CircleDot, ExternalLink, GitBranch, GitPullRequest, Github, HardDrive, ListFilter, LoaderCircle, PanelRightClose, PanelRightOpen, Settings, Terminal, X, Zap } from 'lucide-react'
import { checkDependencies, getActivityFeed, getActivityRefreshAt, getPersonalRefreshAt, getAppSettings, getDashboard, getGithubUser, getLocHistory, getSyncProgress, openExternalUrl, setAppSettings, syncActivity, syncGithubData, syncPersonalWorkItems, syncWorkItems } from './api'
import type { ActivityRelationship, AppSettings, DashboardData, DependencyStatus, FeedKind, Issue, LocSnapshot, PullRequest, Repository, SyncProgress, TimeRange } from './types'
import { aggregateHistory, filterIssues, filterPullRequests, isDraft, isMerged, reuseUnchangedRecords, sameFields, sortRepositories, type FeedState } from './model'
import { activityRepo, exactDate, formatCompact, formatCount, formatSigned, issueUpdated, normalizeDashboard, normalizeLabels, prUpdated, relativeTime, repoActivity, repoBaseline30Available, repoChange, repoChangePercent, repoForks, repoLocAvailable, repoName, repoOpenIssues, repoOpenPrs, repoSource, repoStars, repoTests, repoTotal, repositoryId, repositoryLabel } from './utils'
import codetallyMark from './assets/codetally-mark.webp'
import codetallyAppIcon from '../src-tauri/icons/icon.png'
import SettingsDrawer from './SettingsDrawer'
import KanbanBoard from './kanban/KanbanBoard'
import UpdateStatus from './UpdateStatus'
import { DEFAULT_APP_SETTINGS, normalizeAppSettings } from './settings'
import GitHubConnection from './GitHubConnection'
import ActivityRepositoryMenu, { activityScopeRepositories } from './ActivityRepositoryMenu'
import ActivityInvolvementFilter from './ActivityInvolvementFilter'
import './styles.css'

const LocChart = lazy(() => import('./LocChart'))

function HistoryChart(props: React.ComponentProps<typeof LocChart>) {
  return <Suspense fallback={<section className="chart-panel"><div className="chart-wrap" role="status">Loading line history chart…</div></section>}><LocChart {...props} /></Suspense>
}

type Screen = 'dashboard' | 'detail' | 'kanban'
type ActivityView = FeedKind | 'all'
type RepoSort = 'name' | 'loc' | 'source' | 'tests' | 'growth' | 'prs' | 'issues' | 'stars' | 'forks' | 'activity'
const NARROW_FEED_WIDTH = 1000

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
  const [allTicketsRefreshing, setAllTicketsRefreshing] = useState(false)
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
  const [feedKind, setFeedKind] = useState<ActivityView>('prs')
  const [feedState, setFeedState] = useState<FeedState>('open')
  const [feedRepo, setFeedRepo] = useState('all')
  const [feedRevision, setFeedRevision] = useState(0)
  const [metric, setMetric] = useState<'total' | 'source' | 'tests'>('total')
  const [repoSort, setRepoSort] = useState<RepoSort>('loc')
  const [repoSortDirection, setRepoSortDirection] = useState<'asc' | 'desc'>('desc')
  const [settingsOpen, setSettingsOpen] = useState(false)
  const [appSettings, setAppSettingsState] = useState<AppSettings>(DEFAULT_APP_SETTINGS)
  const range = appSettings.loc_chart_range
  const [settingsLoaded, setSettingsLoaded] = useState(false)
  const [settingsSaving, setSettingsSaving] = useState(false)
  const [settingsError, setSettingsError] = useState<string | null>(null)
  const [narrowFeed, setNarrowFeed] = useState(() => window.innerWidth < NARROW_FEED_WIDTH)
  const [wideFeedOpen, setWideFeedOpen] = useState(true)
  const [narrowFeedOpen, setNarrowFeedOpen] = useState(false)
  const feedOpen = narrowFeed ? narrowFeedOpen : wideFeedOpen
  useEffect(() => {
    const updateWidth = () => setNarrowFeed(window.innerWidth < NARROW_FEED_WIDTH)
    window.addEventListener('resize', updateWidth)
    return () => window.removeEventListener('resize', updateWidth)
  }, [])
  useEffect(() => { if (narrowFeed) setNarrowFeedOpen(false) }, [narrowFeed])
  useEffect(() => { setNarrowFeedOpen(false) }, [screen])
  useEffect(() => {
    if (!narrowFeed || !narrowFeedOpen) return
    const closeOnEscape = (event: KeyboardEvent) => { if (event.key === 'Escape') setNarrowFeedOpen(false) }
    window.addEventListener('keydown', closeOnEscape)
    return () => window.removeEventListener('keydown', closeOnEscape)
  }, [narrowFeed, narrowFeedOpen])
  const toggleFeed = () => narrowFeed ? setNarrowFeedOpen((open) => !open) : setWideFeedOpen((open) => !open)
  useEffect(() => { if (!appSettings.kanban_enabled && screen === 'kanban') setScreen('dashboard') }, [appSettings.kanban_enabled, screen])
  const feedRelationship = appSettings.activity_relationship
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
  const feedLoadingRef = useRef({ prs: false, issues: false })
  const lastActiveProgressRef = useRef<SyncProgress | null>(null)
  const cachedRefreshRef = useRef(false)
  const cachedRefreshVersionRef = useRef(0)
  const finalRefreshRef = useRef(false)

  const activeRepositories = useMemo(() => dashboard.repositories.filter((repo) => !(repo.is_archived ?? repo.isArchived)), [dashboard.repositories])
  const partialLoc = useMemo(() => activeRepositories.some((repo) => (appSettings.include_forks_in_totals || !(repo.is_fork ?? repo.isFork)) && !repoLocAvailable(repo)), [activeRepositories, appSettings.include_forks_in_totals])
  const selectedRepoId = useMemo(() => selectedRepo ? repositoryId(selectedRepo) : null, [selectedRepo])

  const applyDashboard = useCallback((raw: DashboardData, reloadFeeds = true, preserveFeedCache = true) => {
    const next = toDashboard(raw)
    const prsRequestPending = feedLoadingRef.current.prs
    const issuesRequestPending = feedLoadingRef.current.issues
    setDashboard((current) => ({
      ...next,
      repositories: reuseUnchangedRecords(current.repositories, next.repositories),
      loc_history: reuseUnchangedRecords(current.loc_history, next.loc_history),
      metrics: sameFields(current.metrics, next.metrics) ? current.metrics : next.metrics,
      pull_requests: preserveFeedCache && (prsRequestPending || current.pull_requests.length) ? current.pull_requests : next.pull_requests,
      issues: preserveFeedCache && (issuesRequestPending || current.issues.length) ? current.issues : next.issues
    }))
    if (reloadFeeds) setFeedRevision((revision) => revision + 1)
    setSelectedRepo((previous) => {
      if (!previous) return previous
      const refreshed = next.repositories.find((repo) => String(repositoryId(repo)) === String(repositoryId(previous)))
      return refreshed && !sameFields(previous, refreshed) ? refreshed : previous
    })
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
      const embeddedHistory = next.loc_history ?? next.locHistory ?? next.history
      const historyResult = await Promise.allSettled([embeddedHistory !== undefined ? Promise.resolve(embeddedHistory) : getLocHistory({ range: 'ALL' })])
      const history = historyResult[0].status === 'fulfilled' ? historyResult[0].value : []
      if (selectionRevision !== selectionRevisionRef.current || dashboardVersion !== cachedRefreshVersionRef.current || settingsRefreshRef.current) { setBusy(false); return false }
      applyDashboard({ ...next, loc_history: next.loc_history ?? next.locHistory ?? next.history ?? history }, true, preserveFeedCache)
      hasCachedRepositories = next.repositories?.some((repo) => !(repo.is_archived ?? repo.isArchived)) ?? false
      if (historyResult[0].status === 'rejected' && !next.loc_history && !next.locHistory && !next.history) setError('Line history isn\'t available yet. Import a repository to start tracking it.')
    } else {
      const reason = dashboardResult[0].reason
      setError(reason instanceof Error ? reason.message : String(reason))
    }
    setBusy(false)
    if (showBusy && !hasCachedRepositories) void checkDependencies().then(setDeps).catch((reason) => {
      if (!hasCachedRepositories) setError(reason instanceof Error ? reason.message : String(reason))
    })
    if (!hasCachedRepositories && showBusy) void getGithubUser().then((user) => { if (user.login) setLogin(user.login) }).catch(() => undefined)
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
      setError(`Couldn't check GitHub sign-in: ${message}`)
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
      const [dashboardResult] = await Promise.allSettled([getDashboard()])
      if (selectionRevision !== selectionRevisionRef.current || settingsRefreshRef.current || finalRefreshRef.current || requestVersion !== cachedRefreshVersionRef.current || dashboardResult.status !== 'fulfilled') return
      const next = dashboardResult.value
      const history = next.loc_history ?? next.locHistory ?? next.history ?? await getLocHistory({ range: 'ALL' }).catch(() => [])
      if (selectionRevision !== selectionRevisionRef.current || settingsRefreshRef.current || finalRefreshRef.current || requestVersion !== cachedRefreshVersionRef.current) return
      applyDashboard({ ...next, loc_history: next.loc_history ?? next.locHistory ?? next.history ?? history }, false)
      if (screen === 'detail' && selectedRepoId !== null) {
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

  const refreshActivityTimestamp = useCallback(() => {
    void getActivityRefreshAt().then((timestamp) => {
      setDashboard((current) => {
        const existing = current.last_activity_refresh_at
        if (existing && (!timestamp || new Date(existing).getTime() > new Date(timestamp).getTime())) return current
        return { ...current, last_activity_refresh_at: timestamp }
      })
    }).catch(() => undefined)
  }, [])

  const refreshActivityCache = useCallback(() => {
    setFeedRevision((current) => current + 1)
    refreshActivityTimestamp()
  }, [refreshActivityTimestamp])

  const refreshPersonalTimestamp = useCallback(() => {
    void getPersonalRefreshAt().then((timestamp) => {
      setDashboard((current) => {
        const existing = current.last_personal_refresh_at
        if (existing && (!timestamp || new Date(existing).getTime() > new Date(timestamp).getTime())) return current
        return { ...current, last_personal_refresh_at: timestamp }
      })
    }).catch(() => undefined)
  }, [])

  const refreshPersonalCache = useCallback(() => {
    setFeedRevision((current) => current + 1)
    refreshPersonalTimestamp()
    if (screen === 'detail' && selectedRepoId !== null) {
      void Promise.all([getActivityFeed({ kind: 'prs', repositoryId: selectedRepoId }), getActivityFeed({ kind: 'issues', repositoryId: selectedRepoId })]).then(([prs, issues]) => {
        setDetailPrs(prs as PullRequest[])
        setDetailIssues(issues as Issue[])
      }).catch(() => undefined)
    }
  }, [refreshPersonalTimestamp, screen, selectedRepoId])

  const refreshAllTickets = useCallback(async () => {
    if (syncingRef.current || importingRef.current) return
    setAllTicketsRefreshing(true)
    setError(null)
    try {
      const result = await syncWorkItems()
      refreshActivityCache()
      const failure = syncFailure(result)
      if (failure) setError(failure)
    } catch (reason) {
      setError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setAllTicketsRefreshing(false)
    }
  }, [refreshActivityCache])

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
        return authoritative ? { ...authoritative, running: false, message: completionMessage || authoritative.message || 'Refresh complete', error: completionError ?? authoritative.error } : previous
      })
      syncingRef.current = false
      setSyncing(false)
    }
  }, [loadFinalData])

  useEffect(() => {
    if (startupRef.current) return
    startupRef.current = true
    void loadData(true).then((hasCachedRepositories) => { if (hasCachedRepositories && import.meta.env.VITE_CODETALLY_SCREENSHOT_MODE !== '1') void refreshData(true) })
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
      setSettingsError(`Couldn't load settings: ${reason instanceof Error ? reason.message : String(reason)}`)
    }
  }, [])

  useEffect(() => { void loadSettings() }, [loadSettings])

  // Personal activity events update the activity cache without touching line history.
  useEffect(() => {
    let alive = true
    let stopListening: (() => void) | undefined
    void listen<{ refreshed_at: string | null; complete: boolean }>('background-personal-sync-completed', (event) => {
      if (!alive) return
      if (event.payload.complete && event.payload.refreshed_at) {
        const timestamp = event.payload.refreshed_at
        setDashboard((current) => ({ ...current, last_personal_refresh_at: timestamp }))
      }
      setFeedRevision((current) => current + 1)
      if (screen === 'detail' && selectedRepo) {
        const repositoryIdValue = repositoryId(selectedRepo)
        void Promise.all([getActivityFeed({ kind: 'prs', repositoryId: repositoryIdValue }), getActivityFeed({ kind: 'issues', repositoryId: repositoryIdValue })]).then(([prs, issues]) => {
          if (alive) { setDetailPrs(prs as PullRequest[]); setDetailIssues(issues as Issue[]) }
        }).catch(() => undefined)
      }
    }).then((stop) => { if (alive) stopListening = stop; else stop() }).catch(() => undefined)
    return () => { alive = false; stopListening?.() }
  }, [screen, selectedRepo])

  useEffect(() => {
    let alive = true
    let stop: (() => void) | undefined
    void listen('open-settings', () => { if (alive) setSettingsOpen(true) }).then((unlisten) => { if (alive) stop = unlisten; else unlisten() }).catch(() => undefined)
    return () => { alive = false; stop?.() }
  }, [])

  // Scheduling belongs to the native process, which keeps running with the window hidden.
  useEffect(() => {
    let alive = true
    let polling = false
    let wasRunning = false
    let workItemsOnly = false
    let personalOnly = false
    const poll = async () => {
      if (polling || syncingRef.current || importingRef.current) return
      polling = true
      try {
        const progress = await getSyncProgress()
        if (!alive || syncingRef.current || importingRef.current) return
        setSyncProgress(progress)
        if (progress.running) {
          personalOnly = progress.phase === 'syncing_personal_work_items'
          workItemsOnly = personalOnly || progress.phase === 'discovering_work_items' || progress.phase === 'syncing_work_items'
          wasRunning = true
          if (!workItemsOnly) await refreshCachedData()
        } else if (wasRunning) {
          wasRunning = false
          if (personalOnly || progress.phase === 'personal_work_items_complete') {
            refreshPersonalCache()
          } else if (workItemsOnly || progress.phase === 'work_items_complete') {
            refreshActivityCache()
          } else await loadFinalData()
          workItemsOnly = false
          personalOnly = false
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
  }, [loadFinalData, refreshCachedData, refreshActivityCache, refreshPersonalCache])

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
          setSyncProgress({ ...lastActiveProgressRef.current, phase: progress.phase, message: progress.message || 'Updated', error: progress.error })
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
      setImportStep('Importing repositories and GitHub activity')
      const result = await syncGithubData()
      completionProgress = await getSyncProgress().catch(() => null)
      completionMessage = result.message
      completionError = result.errors?.[0] ?? null
      const failure = syncFailure(result)
      setImportStep('Loading line history')
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

  const openRepository = useCallback((repo: Repository) => {
    feedRepoBeforeDetailRef.current = feedRepo
    setFeedRepo(String(repositoryId(repo)))
    setSelectedRepo(repo)
    setScreen('detail')
    setDetailLoading(true)
    setDetailHistory(null)
    setDetailPrs([])
    setDetailIssues([])
  }, [feedRepo])

  useEffect(() => {
    if (screen !== 'detail' || selectedRepoId === null) return
    let alive = true
    setDetailLoading(true)
    void Promise.all([getLocHistory({ repositoryId: selectedRepoId, range: 'ALL' }), getActivityFeed({ kind: 'prs', repositoryId: selectedRepoId }), getActivityFeed({ kind: 'issues', repositoryId: selectedRepoId })]).then(([history, prs, issues]) => {
      if (!alive) return
      setDetailHistory(history)
      setDetailPrs(prs as PullRequest[])
      setDetailIssues(issues as Issue[])
    }).catch((reason) => {
      if (alive) setError(reason instanceof Error ? reason.message : String(reason))
    }).finally(() => { if (alive) setDetailLoading(false) })
    return () => { alive = false }
  }, [feedRevision, screen, selectedRepoId])

  const backToDashboard = useCallback(() => {
    setScreen('dashboard')
    setFeedRepo(feedRepoBeforeDetailRef.current)
    setSelectedRepo(null)
    setDetailHistory(null)
    setDetailPrs([])
    setDetailIssues([])
  }, [])

  const handleFeedKind = useCallback((kind: ActivityView) => {
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
    if (!settingsLoaded || busy) return
    const requestId = ++feedRequestRef.current
    let alive = true
    const kinds: FeedKind[] = feedKind === 'all' ? ['prs', 'issues'] : [feedKind]
    for (const kind of kinds) feedLoadingRef.current[kind] = true
    setDashboard((current) => ({ ...current, ...(kinds.includes('prs') ? { pull_requests: [] } : {}), ...(kinds.includes('issues') ? { issues: [] } : {}) }))
    void Promise.allSettled(kinds.map((kind) => getActivityFeed({ kind, state: feedState, repositoryId: backendRepositoryId(feedRepo), relationship: feedRelationship, ...(groupedFeedScope ? { repositoryIds: JSON.parse(feedRepositoryIdsKey) as number[] } : {}) }))).then((results) => {
      if (!alive || requestId !== feedRequestRef.current) return
      const updates: { pull_requests?: PullRequest[]; issues?: Issue[] } = {}
      const errors: string[] = []
      results.forEach((result, index) => {
        const kind = kinds[index]
        feedLoadingRef.current[kind] = false
        if (kind === 'prs') updates.pull_requests = result.status === 'fulfilled' ? result.value as PullRequest[] : []
        else updates.issues = result.status === 'fulfilled' ? result.value as Issue[] : []
        if (result.status === 'rejected') errors.push(`Couldn't load ${kind === 'prs' ? 'pull requests' : 'issues'}: ${result.reason instanceof Error ? result.reason.message : String(result.reason)}`)
      })
      setDashboard((current) => ({ ...current, ...updates }))
      if (errors.length) setError(errors.join(' '))
    })
    return () => {
      alive = false
      if (requestId === feedRequestRef.current) for (const kind of kinds) feedLoadingRef.current[kind] = false
    }
  }, [busy, feedKind, feedRelationship, feedRepo, feedRevision, feedState, groupedFeedScope, feedRepositoryIdsKey, settingsLoaded, login])

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
  const syncActive = syncing || importing || allTicketsRefreshing || Boolean(syncProgress?.running)
  const personalActivityNewest = Boolean(dashboard.last_personal_refresh_at && (!dashboard.last_activity_refresh_at || new Date(dashboard.last_personal_refresh_at).getTime() > new Date(dashboard.last_activity_refresh_at).getTime()))
  const latestActivityRefreshAt = personalActivityNewest ? dashboard.last_personal_refresh_at : dashboard.last_activity_refresh_at
  const activityRefreshDescription = personalActivityNewest ? 'Personal tickets were refreshed' : 'All tracked repositories were refreshed'
  const syncDetailsAvailable = true
  const syncPopoverProgress = syncProgress ?? { running: true, phase: importing ? 'importing' : 'starting', current: 0, total: 0, message: importing ? importStep : 'Starting refresh' }
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

  const handleFeedRelationship = useCallback((relationship: ActivityRelationship) => {
    changeSettings((current) => ({ ...current, activity_relationship: relationship }))
  }, [changeSettings])

  const setRange = useCallback((nextRange: TimeRange) => {
    changeSettings((current) => ({ ...current, loc_chart_range: nextRange }))
  }, [changeSettings])

  const handleRepositorySort = useCallback((next: RepoSort) => {
    if (next === repoSort) setRepoSortDirection((current) => current === 'asc' ? 'desc' : 'asc')
    else { setRepoSort(next); setRepoSortDirection(next === 'name' ? 'asc' : 'desc') }
  }, [repoSort])
  const handleOpenUrl = useCallback((url: string | undefined) => { void openUrl(url, setError) }, [])

  const navigate = (destination: string) => {
    if (destination === 'kanban') { setScreen('kanban'); return }
    backToDashboard()
    requestAnimationFrame(() => {
      document.querySelector('.page-heading')?.scrollIntoView?.({ behavior: 'smooth', block: 'start' })
    })
  }
  const navItems = [
    { key: 'overview', label: 'Overview', icon: Home, aria: screen === 'kanban' ? 'Back to dashboard' : 'Overview' },
    ...(appSettings.kanban_enabled ? [{ key: 'kanban', label: 'Kanban', icon: PanelsTopLeft, aria: 'Kanban Board' }] : [])
  ]
  const activeNavigation = screen === 'kanban' ? 'kanban' : 'overview'

  return (
    <div className={lightMode ? 'app-shell light' : 'app-shell'}>
      <aside className="navigation-rail" aria-label="Application controls">
        <div className="rail-brand" title="CodeTally"><img src={codetallyAppIcon} alt="CodeTally" /></div>
        <nav aria-label="Main navigation">{navItems.map(({ key, label, icon: Icon, aria }) => <button key={key} type="button" aria-label={aria} title={label} aria-current={activeNavigation === key ? 'page' : undefined} className={activeNavigation === key ? 'active' : ''} disabled={isSetup || (busy && !dashboard.repositories.length)} onClick={() => navigate(key)}><Icon size={17} aria-hidden="true" /><span>{label}</span></button>)}</nav>
        <div className="rail-footer"><UpdateStatus placement="rail" updateCheckInterval={settingsLoaded ? appSettings.update_check_interval : null} /><button className="rail-settings" title="Settings" aria-label="Settings" onClick={() => setSettingsOpen(true)}><Settings size={17} /></button></div>
      </aside>
      <div className="workspace-shell">
      <header className="topbar">
        <div className="topbar-navigation"><span className="workspace-label">Your workspace</span><span className="workspace-divider">/</span><strong>{screen === 'detail' ? 'Repository' : screen === 'kanban' ? 'Kanban' : 'Overview'}</strong></div>
        <div className="topbar-status">
          <div className="topbar-refresh-group">
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
            <div className="refresh-timestamps" tabIndex={0} aria-label="Refresh times">
              <div className="refresh-timestamp-list">
                <span title={latestActivityRefreshAt ? `${activityRefreshDescription} ${exactDate(latestActivityRefreshAt)}` : 'No successful PR and issue refresh has completed.'} aria-label={`Activities refreshed ${latestActivityRefreshAt ? relativeTime(latestActivityRefreshAt) : 'Not recorded'}`}><span className="refresh-label-full">Activities refreshed</span><span className="refresh-label-compact">Activities</span><strong>{latestActivityRefreshAt ? relativeTime(latestActivityRefreshAt, true) : '—'}</strong></span>
                <span title={dashboard.last_full_refresh_at ? exactDate(dashboard.last_full_refresh_at) : 'No successful Repo Refresh or Force Refresh has completed. Partial or failed attempts do not set this time.'} aria-label={`Repositories refreshed ${dashboard.last_full_refresh_at ? relativeTime(dashboard.last_full_refresh_at) : 'Not recorded'}`}><span className="refresh-label-full">Repositories refreshed</span><span className="refresh-label-compact">Repos</span><strong>{dashboard.last_full_refresh_at ? relativeTime(dashboard.last_full_refresh_at, true) : '—'}</strong></span>
              </div>
              {syncActive && <span className="refresh-running" role="status" aria-label={importing ? 'Importing' : 'Refreshing'}><LoaderCircle size={13} className="spin" aria-hidden="true" /></span>}
            </div>
            {syncDetailsAvailable && syncPopoverOpen && <div className="sync-popover" role="dialog" aria-label={syncActive ? 'Refresh details' : 'Last refresh'}>
              <div className="sync-popover-heading"><span>{syncActive ? 'Refresh details' : 'Last refresh'}</span><button className="icon-button subtle" type="button" aria-label="Close refresh details" onClick={() => { syncPopoverDismissedRef.current = true; setSyncPopoverOpen(false); setSyncPopoverPinned(false) }}><X size={14} /></button></div>
              <div className="sync-popover-timestamps">
                <div><span>Activities refreshed</span><time dateTime={latestActivityRefreshAt ?? undefined} title={activityRefreshDescription}>{latestActivityRefreshAt ? exactDate(latestActivityRefreshAt) : 'Not recorded'}</time></div>
                <div><span>Repositories refreshed</span><time dateTime={dashboard.last_full_refresh_at ?? undefined}>{dashboard.last_full_refresh_at ? exactDate(dashboard.last_full_refresh_at) : 'Not recorded'}</time></div>
              </div>
              {syncActive && <SyncProgressPanel progress={syncPopoverProgress} />}
            </div>}
            </div>
          </div>
          {!isSetup && screen !== 'kanban' && <button className="icon-button feed-toggle" type="button" aria-label={feedOpen ? 'Hide activity sidebar' : 'Show activity sidebar'} aria-controls="activity-sidebar" aria-expanded={feedOpen} title={feedOpen ? 'Hide Live Feed' : 'Show Live Feed'} onClick={toggleFeed}>{feedOpen ? <PanelRightClose size={17} /> : <PanelRightOpen size={17} />}</button>}
        </div>
      </header>

      {error && <div className="error-banner"><AlertCircle size={16} /><span>{error}</span><button className="icon-button subtle" onClick={() => setError(null)} aria-label="Dismiss error"><X size={15} /></button></div>}

      {busy && !dashboard.repositories.length ? <LoadingScreen /> : isSetup ? <SetupScreen deps={deps} login={login} error={error} importing={importing} importStep={importStep} checking={checkingDependencies} onImport={() => void importRepositories()} onRetry={checkSetupConnection} /> : (
        <div className={screen === 'kanban' && appSettings.kanban_enabled ? 'content-shell kanban-shell' : `content-shell${feedOpen ? '' : ' feed-collapsed'}`}>
          {screen === 'kanban' && appSettings.kanban_enabled ? <KanbanBoard repositories={activeRepositories} login={login} relationship={feedRelationship} onRelationship={handleFeedRelationship} onBack={backToDashboard} onActivityRefreshed={refreshPersonalCache} syncBusy={syncActive} revision={feedRevision} /> : screen === 'dashboard' ? (
            <main className="main-column">
              {deps.gh && !dependenciesAuthenticated(deps) && <GitHubConnection deps={deps} login={login} compact checking={checkingDependencies} onRetry={checkSetupConnection} />}
              {activeRepositories.length === 0 ? <EmptySelectionState onOpenSettings={() => setSettingsOpen(true)} /> : <>
                <div className="page-heading"><div><p className="eyebrow">Your repositories</p><h1>Code at a glance</h1><p className="page-subtitle">Here’s what’s happening across your code.</p></div><span className="overview-period"><CircleDot size={12} /> Local overview</span></div>
                <SummaryStrip metrics={metrics} partialLoc={partialLoc} />
                <HistoryChart history={visibleHistory} metric={metric} range={range} onMetric={setMetric} onRange={setRange} />
                <RepositoryTable repositories={sortedRepositories} login={login} sort={repoSort} direction={repoSortDirection} onSort={handleRepositorySort} onSelect={openRepository} />
              </>}
            </main>
          ) : selectedRepo ? (
            <RepositoryDetails repo={selectedRepo} history={visibleHistory} historyLoading={detailLoading} metric={metric} range={range} onMetric={setMetric} onRange={setRange} onBack={backToDashboard} pullRequests={detailPrs} issues={detailIssues} onOpenUrl={handleOpenUrl} />
          ) : null}
          {screen !== 'kanban' && narrowFeed && <button className="activity-backdrop" type="button" aria-label="Close activity sidebar" aria-hidden={!feedOpen} tabIndex={feedOpen ? 0 : -1} onClick={() => setNarrowFeedOpen(false)} />}
          {screen !== 'kanban' && <ActivitySidebar hidden={!feedOpen} asOf={feedRelationship === 'everyone' ? dashboard.last_activity_refresh_at : dashboard.last_personal_refresh_at} onActivityRefreshed={refreshPersonalCache} syncBusy={syncActive} activityProgress={syncProgress} login={login} kind={feedKind} state={feedState} relationship={feedRelationship} repository={feedRepo} lockedRepository={screen === 'detail' && selectedRepo ? String(repositoryId(selectedRepo)) : null} repositories={activeRepositories} prs={filteredPrs} issues={filteredIssues} onKind={handleFeedKind} onState={handleFeedState} onRelationship={handleFeedRelationship} onRepository={handleFeedRepository} onOpenUrl={handleOpenUrl} />}
        </div>
      )}

      </div>
      {settingsOpen && <SettingsDrawer login={login} onForceRefresh={() => void refreshData()} onRefreshAllTickets={() => void refreshAllTickets()} refreshing={syncActive} settings={appSettings} loaded={settingsLoaded} metrics={dashboard.metrics} menuPreview={{ history: dashboard.loc_history, lastFullRefreshAt: dashboard.last_full_refresh_at, lastActivityRefreshAt: dashboard.last_activity_refresh_at, lastPersonalRefreshAt: dashboard.last_personal_refresh_at, partialLoc, progress: syncProgress }} saving={settingsSaving} error={settingsError} onChange={changeSettings} onRetry={() => void (settingsLoaded ? flushSettings() : loadSettings())} onClose={() => setSettingsOpen(false)} />}
    </div>
  )
}

async function openUrl(url: string | undefined, setError: (error: string | null) => void) {
  if (!url) return
  try { await openExternalUrl(url) } catch (reason) { setError(reason instanceof Error ? reason.message : String(reason)) }
}

function LoadingScreen() {
  return <div className="center-state"><LoaderCircle size={27} className="spin" /><p>Opening your dashboard</p><span>Loading saved GitHub data…</span></div>
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
  if (phase === 'syncing_personal_work_items') return 'Searching your tickets'
  if (phase === 'personal_work_items_complete') return 'Tickets updated'
  if (phase === 'discovering' || phase === 'discovering_activity') return 'Finding repositories'
  if (phase === 'syncing' || phase === 'syncing_repository' || phase === 'syncing_activity') return 'Updating'
  if (phase === 'backfilling') return 'Building line history'
  if (phase === 'scanning_current') return 'Checking line counts'
  if (phase === 'complete') return 'Updated'
  if (phase === 'idle') return 'Ready'
  return phase.replaceAll('_', ' ').replace(/\b\w/g, (character) => character.toUpperCase())
}

function SyncProgressPanel({ progress }: { progress: SyncProgress }) {
  const counts = progressCounts(progress)
  const overallLabel = counts.repositoriesTotal > 0 ? `Repositories ${counts.repositoriesCurrent} / ${counts.repositoriesTotal}` : 'Waiting for repository count'
  const samplesLabel = counts.snapshotsTotal > 0 ? `Lines scanned ${counts.snapshotsCurrent} / ${counts.snapshotsTotal}` : null
  const activityOnly = progress.phase.includes('work_items')
  const personalOnly = progress.phase.includes('personal_work_items')
  return <section className="sync-progress-panel" aria-live="polite"><div className="sync-progress-heading"><div><p className="eyebrow">{progress.running ? readablePhase(progress.phase) : 'Last refresh'}</p><strong>{progress.message || readablePhase(progress.phase)}</strong>{progress.repository_name && !personalOnly && <span>{progress.repository_name}</span>}</div><span className={progress.running ? 'sync-progress-state active' : 'sync-progress-state'}>{progress.running ? 'Updating' : 'Updated'}</span>{progress.error && <div className="sync-progress-error"><AlertCircle size={13} /> {progress.error}</div>}</div>{personalOnly ? <p className="sync-progress-personal">Searching your authored and assigned tickets in tracked repositories.</p> : <div className="sync-progress-bars"><div className="sync-progress-row"><div><span>Repository progress</span><b>{overallLabel}</b></div><ProgressMeter value={counts.repositoriesCurrent} max={counts.repositoriesTotal} label="Repository progress" /></div>{!activityOnly && <div className="sync-progress-row samples"><div><span>Current line scan</span><b>{samplesLabel ?? 'Waiting to scan lines'}</b></div><ProgressMeter value={counts.snapshotsCurrent} max={counts.snapshotsTotal} label="Current line scan progress" /></div>}</div>}</section>
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
  return <div className="setup-wrap"><div className="setup-card"><div className="setup-icon"><img src={codetallyMark} alt="" aria-hidden="true" /></div><p className="eyebrow">Let’s get started</p><GitHubConnection deps={deps} login={login} checking={checking} onRetry={onRetry} /><div className="setup-checks">{checks.map(({ label, ok, icon: Icon }) => <div className={ok ? 'setup-check ok' : 'setup-check'} key={label}><span className="check-icon">{ok ? <Check size={15} /> : <Icon size={15} />}</span><span>{label}</span>{ok ? <span className="check-state">Ready</span> : <span className="check-state missing">Missing</span>}</div>)}</div>{missingTools.length ? <p className="github-missing-tools">Missing local tools: {missingTools.join(' and ')}. Install them, then check again.</p> : null}{error && <div className="setup-error"><AlertCircle size={15} /> {error}</div>}{importing ? <div className="setup-import-note" role="status"><LoaderCircle size={15} className="spin" /><span>{importStep}</span><small>You can follow progress from the header.</small></div> : ready ? <button className="button primary wide" onClick={onImport}><HardDrive size={16} /> {error ? 'Try again' : 'Add repositories'}</button> : null}</div></div>
}

function EmptySelectionState({ onOpenSettings }: { onOpenSettings: () => void }) {
  return <section className="empty-selection" aria-labelledby="empty-selection-heading"><div className="empty-selection-icon"><ListFilter size={20} /></div><p className="eyebrow">Dashboard paused</p><h1 id="empty-selection-heading">No repositories selected</h1><p>Choose repositories in Settings to bring them back to the dashboard. Your saved history stays here, and any update already running may finish.</p><button className="button primary" type="button" onClick={onOpenSettings}><Settings size={15} /> Open Settings</button></section>
}

const SummaryStrip = memo(function SummaryStrip({ metrics, partialLoc }: { metrics: ReturnType<typeof toDashboard>['metrics']; partialLoc: boolean }) {
  const cards = [
    { label: 'Total Lines', value: metrics.total_loc, icon: CodeXml, tone: 'blue', hint: 'Lines of code' },
    { label: 'Repositories', value: metrics.repositories, icon: Database, tone: 'purple', hint: 'Tracked repositories' },
    { label: 'Open PRs', value: metrics.open_prs, icon: GitPullRequest, tone: 'purple', hint: 'Pull requests' },
    { label: 'Open issues', value: metrics.open_issues, icon: CircleDot, tone: 'red', hint: 'Issues to follow up' }
  ]
  return <>
    <div className="summary-strip">{cards.map(({ label, value, icon: Icon, tone, hint }) => <div className={`summary-item ${tone}`} key={label}><span className="summary-icon"><Icon size={22} /></span><strong>{formatCount(value)}</strong><span className="summary-label">{label}</span><small>{hint}</small></div>)}</div>
    <div className="summary-breakdown"><span><CodeXml size={14} /> Source Lines <strong>{formatCount(metrics.source_loc)}</strong></span><span><FlaskConical size={14} /> Test Lines <strong>{formatCount(metrics.test_loc)}</strong></span><span><ArrowUp size={14} /> 30-day change <strong className={metrics.loc_30d_change >= 0 ? 'positive' : 'negative'}>{formatSigned(metrics.loc_30d_change, true)}</strong></span></div>
    {partialLoc && <p className="summary-partial-notice" role="status"><AlertCircle size={13} /><span>Partial data: line totals and change exclude repositories whose line counts are not available yet.</span></p>}
  </>
})

const RepositoryTable = memo(function RepositoryTable({ repositories, login, sort, direction, onSort, onSelect }: { repositories: Repository[]; login: string; sort: RepoSort; direction: 'asc' | 'desc'; onSort: (sort: RepoSort) => void; onSelect: (repo: Repository) => void }) {
  const [hiddenIds, setHiddenIds] = useState<Set<string>>(() => new Set())
  const [excludeForks, setExcludeForks] = useState(false)
  const visibleRepositories = repositories.filter((repo) => !hiddenIds.has(String(repositoryId(repo))) && (!excludeForks || !(repo.is_fork ?? repo.isFork)))
  const setVisible = (ids: string[], visible: boolean) => setHiddenIds((current) => {
    const next = new Set(current)
    for (const id of ids) { if (visible) next.delete(id); else next.add(id) }
    return next
  })
  const columns: Array<{ key: RepoSort; label: string; title: string }> = [{ key: 'name', label: 'Repository', title: 'Repository' }, { key: 'loc', label: 'Total', title: 'Total lines' }, { key: 'source', label: 'Source', title: 'Source lines' }, { key: 'tests', label: 'Tests', title: 'Test lines' }, { key: 'growth', label: '30d', title: '30-day change' }, { key: 'prs', label: 'PRs', title: 'Open pull requests' }, { key: 'issues', label: 'Issues', title: 'Open issues' }, { key: 'stars', label: 'Stars', title: 'GitHub stars' }, { key: 'forks', label: 'Forks', title: 'GitHub forks' }, { key: 'activity', label: 'Activity', title: 'Last activity' }]
  return <section className="repos-panel"><div className="panel-heading table-heading"><div><p className="eyebrow">Your repositories</p><h2>Repositories</h2></div><TrackedRepositoryMenu repositories={repositories} login={login} hiddenIds={hiddenIds} excludeForks={excludeForks} onExcludeForks={setExcludeForks} onVisibilityChange={setVisible} onShowAll={() => { setHiddenIds(new Set()); setExcludeForks(false) }} /></div>{visibleRepositories.length ? <div className="table-scroll" role="region" tabIndex={0} aria-label="Repository table"><table><thead><tr>{columns.map((column) => <th key={column.key}><button className={sort === column.key ? 'sort-button active' : 'sort-button'} title={`Sort by ${column.title}`} onClick={() => onSort(column.key)}>{column.label}{sort === column.key && (direction === 'asc' ? <ChevronUp size={13} /> : <ChevronDown size={13} />)}</button></th>)}</tr></thead><tbody>{visibleRepositories.map((repo) => { const isPrivate = repo.is_private ?? repo.isPrivate ?? false; return <tr key={String(repositoryId(repo))} onClick={() => onSelect(repo)} tabIndex={0} onKeyDown={(event) => { if (event.key === 'Enter') onSelect(repo) }}><td><div className="repo-cell"><span className="repo-glyph"><GitBranch size={14} /></span><span><strong>{repoName(repo)}</strong><small><span className="repo-meta-text">{repo.owner ?? repositoryLabel(repo).split('/')[0]}{repo.primary_language ?? repo.primaryLanguage ? ` · ${repo.primary_language ?? repo.primaryLanguage}` : ''}</span><em className={`repo-visibility ${isPrivate ? 'private' : 'public'}`}>{isPrivate ? 'Private' : 'Public'}</em></small></span></div></td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoTotal(repo))}</td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoSource(repo))}</td><td className="number-cell" title={!repoLocAvailable(repo) ? 'Lines are not available for this repository' : undefined}>{repoLocDisplay(repo, repoTests(repo))}</td><td className={repoGrowthClass(repo)} title={repoGrowthTitle(repo)}>{repoChangeDisplay(repo)}</td><td className="number-cell">{formatCount(repoOpenPrs(repo))}</td><td className="number-cell">{formatCount(repoOpenIssues(repo))}</td><td className="number-cell">{formatCount(repoStars(repo))}</td><td className="number-cell">{formatCount(repoForks(repo))}</td><td><span className="relative" title={exactDate(repoActivity(repo))}>{relativeTime(repoActivity(repo))}</span></td></tr> })}</tbody></table></div> : <div className="empty-state"><GitBranch size={22} /><p>{excludeForks ? 'No non-fork repositories match this view' : repositories.length ? 'All repositories are hidden' : 'No repositories selected'}</p></div>}</section>
})

function SelectionCheckbox({ label, checked, mixed = false, disabled = false, onChange }: { label: string; checked: boolean; mixed?: boolean; disabled?: boolean; onChange: (checked: boolean) => void }) {
  const ref = useRef<HTMLInputElement>(null)
  useEffect(() => { if (ref.current) ref.current.indeterminate = mixed }, [mixed])
  return <input ref={ref} className="mac-checkbox" type="checkbox" aria-label={label} aria-checked={mixed ? 'mixed' : checked} checked={checked} disabled={disabled} onChange={(event) => onChange(event.target.checked)} />
}

function TrackedRepositoryMenu({ repositories, login, hiddenIds, excludeForks, onExcludeForks, onVisibilityChange, onShowAll }: { repositories: Repository[]; login: string; hiddenIds: Set<string>; excludeForks: boolean; onExcludeForks: (exclude: boolean) => void; onVisibilityChange: (ids: string[], visible: boolean) => void; onShowAll: () => void }) {
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
  const filterableRepositories = repositories.filter((repo) => !excludeForks || !(repo.is_fork ?? repo.isFork))
  const personal = filterableRepositories.filter((repo) => ownerOf(repo).toLowerCase() === login.toLowerCase())
  const companies = new Map<string, { owner: string; repositories: Repository[] }>()
  for (const repo of filterableRepositories) {
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
  const visibleCount = filterableRepositories.filter((repo) => !hiddenIds.has(String(repositoryId(repo)))).length
  return <div ref={containerRef} className={open ? 'tracked-repository-menu open' : 'tracked-repository-menu'} onBlur={(event) => { if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) setOpen(false) }}>
    <button ref={triggerRef} type="button" aria-expanded={open} aria-haspopup="dialog" aria-controls="repository-view-picker" onClick={() => { if (!open) setExpanded({}); setOpen((current) => !current) }}><CircleDot size={13} /> {repositories.length} tracked <ChevronDown size={14} /></button>
    {open && <div ref={panelRef} style={panelStyle} tabIndex={-1} role="dialog" aria-label="Show repositories in table" id="repository-view-picker" className="repo-picker tracked-repository-groups">
      <div className="repo-picker-heading"><strong>Show in table</strong><span>{visibleCount} of {repositories.length} visible</span><button type="button" onClick={onShowAll} disabled={visibleCount === repositories.length && !excludeForks}>Show all</button></div>
      <p className="repo-picker-note">Tracking continues for hidden repositories.</p>
      <label className="repo-picker-option repo-picker-fork-filter"><SelectionCheckbox label="Exclude forks from table" checked={excludeForks} onChange={onExcludeForks} /><span>Exclude forks</span></label>
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

const ActivitySidebar = memo(function ActivitySidebar({ hidden, asOf, onActivityRefreshed, syncBusy, activityProgress, login, kind, state, relationship, repository, lockedRepository, repositories, prs, issues, onKind, onState, onRelationship, onRepository, onOpenUrl }: { hidden: boolean; asOf?: string | null; onActivityRefreshed: () => void; syncBusy: boolean; activityProgress: SyncProgress | null; login: string; kind: ActivityView; state: FeedState; relationship: ActivityRelationship; repository: string; lockedRepository?: string | null; repositories: Repository[]; prs: PullRequest[]; issues: Issue[]; onKind: (kind: ActivityView) => void; onState: (state: FeedState) => void; onRelationship: (relationship: ActivityRelationship) => void; onRepository: (repository: string) => void; onOpenUrl: (url: string | undefined) => void }) {
  const sidebarRef = useRef<HTMLElement>(null)
  useEffect(() => { if (sidebarRef.current) sidebarRef.current.inert = hidden }, [hidden])
  const [compactPullRequestLabel, setCompactPullRequestLabel] = useState(true)
  useLayoutEffect(() => {
    const sidebar = sidebarRef.current
    if (!sidebar) return
    const updateLabel = () => setCompactPullRequestLabel(sidebar.clientWidth <= 360)
    updateLabel()
    if (typeof ResizeObserver === 'undefined') {
      window.addEventListener('resize', updateLabel)
      return () => window.removeEventListener('resize', updateLabel)
    }
    const observer = new ResizeObserver(updateLabel)
    observer.observe(sidebar)
    return () => observer.disconnect()
  }, [])
  const [refreshing, setRefreshing] = useState(false)
  const [refreshMessage, setRefreshMessage] = useState('')
  const [refreshedAt, setRefreshedAt] = useState<string | null>(null)
  const [refreshError, setRefreshError] = useState('')
  const refreshActivity = async () => {
    setRefreshing(true); setRefreshMessage('Searching your authored and assigned PRs and issues in tracked repositories…'); setRefreshError('')
    try {
      const result = await syncPersonalWorkItems()
      onActivityRefreshed()
      setRefreshMessage(result.message)
      if (result.ok && !result.errors.length) setRefreshedAt(new Date().toISOString())
      if (!result.ok || result.errors.length) setRefreshError(result.errors.join(' ') || result.message)
    } catch (reason) { setRefreshError(String(reason)); setRefreshMessage('') }
    finally { setRefreshing(false) }
  }
  const effectiveAsOf = relationship !== 'everyone' && refreshedAt && (!asOf || new Date(refreshedAt).getTime() > new Date(asOf).getTime()) ? refreshedAt : asOf
  const feed = useMemo(() => {
    const pullRequests = prs.map((item) => ({ kind: 'prs' as const, item }))
    const issueItems = issues.map((item) => ({ kind: 'issues' as const, item }))
    if (kind === 'prs') return pullRequests
    if (kind === 'issues') return issueItems
    return [...pullRequests, ...issueItems].sort((left, right) => {
      const updated = (entry: typeof left) => new Date(entry.kind === 'prs' ? prUpdated(entry.item) : issueUpdated(entry.item)).getTime() || 0
      return updated(right) - updated(left)
    })
  }, [kind, prs, issues])
  const selectedRepository = lockedRepository ?? repository
  return <aside id="activity-sidebar" ref={sidebarRef} className="activity-sidebar" aria-hidden={hidden}><div className="sidebar-sticky"><div className="sidebar-heading"><div><p className="eyebrow">Live feed <span className="feed-count">{feed.length}</span></p><h2>Recent activity</h2></div><button className="button primary compact activity-refresh-button" disabled={syncBusy || refreshing} title="Refresh your authored and assigned PRs and issues in tracked repositories without scanning lines of code" onClick={() => void refreshActivity()}>{refreshing ? 'Refreshing…' : 'Refresh Tickets'}</button></div><p className="activity-as-of" title={exactDate(effectiveAsOf)}>{relationship === 'everyone' ? 'All tracked activity' : 'Personal activity'} · As of {effectiveAsOf ? relativeTime(effectiveAsOf) : 'Not recorded'}</p>{refreshing || refreshMessage ? <p className="small-note" role="status">{refreshing && activityProgress?.phase.includes('work_items') ? activityProgress.message : refreshMessage}</p> : null}{refreshError && <p className="activity-refresh-error" role="alert">{refreshError}</p>}<div className="feed-tabs" role="tablist" aria-label="Activity type"><button className={kind === 'all' ? 'active' : ''} aria-label="Pull requests and issues" aria-pressed={kind === 'all'} onClick={() => onKind('all')}>All</button><button className={kind === 'prs' ? 'active' : ''} aria-label="Pull requests" title="Pull requests" onClick={() => onKind('prs')}><GitPullRequest size={14} aria-hidden="true" /><span aria-hidden="true">{compactPullRequestLabel ? 'PRs' : 'Pull requests'}</span></button><button className={kind === 'issues' ? 'active' : ''} onClick={() => onKind('issues')}><CircleDot size={14} aria-hidden="true" />Issues</button></div><div className="feed-filters"><div className="filter-pills">{(['all', 'open', 'closed'] as FeedState[]).map((key) => <button key={key} className={state === key ? 'active' : ''} onClick={() => onState(key)}>{key[0].toUpperCase() + key.slice(1)}</button>)}</div><ActivityInvolvementFilter id="activity-involvement-select" value={relationship} onChange={onRelationship} /><ActivityRepositoryMenu repositories={repositories} login={login} value={selectedRepository} disabled={Boolean(lockedRepository)} onChange={onRepository} /></div></div><div className="feed-list">{feed.length ? feed.map((entry) => entry.kind === 'prs' ? <PullRequestItem key={`pr-${activityRepo(entry.item)}-${entry.item.number}`} item={entry.item} onOpen={() => onOpenUrl(entry.item.url)} /> : <IssueItem key={`issue-${activityRepo(entry.item)}-${entry.item.number}`} item={entry.item} onOpen={() => onOpenUrl(entry.item.url)} />) : <div className="feed-empty"><CircleDot size={21} /><p>No {kind === 'all' ? 'pull requests or issues' : kind === 'prs' ? 'pull requests' : 'issues'} match these filters</p><span>Activity will appear here after the next refresh.</span></div>}</div></aside>
})

function PullRequestItem({ item, onOpen }: { item: PullRequest; onOpen: () => void }) {
  const state = statusText(item)
  const date = item.updated_at ?? item.updatedAt ?? item.created_at ?? item.createdAt
  const ciState = String(item.ci_state ?? item.ciState ?? '').toLowerCase()
  const CiIcon = ciState === 'success' ? Check : ciState === 'failure' ? X : ciState === 'pending' ? LoaderCircle : null
  const ActivityIcon = state === 'MERGED' ? CircleCheck : GitPullRequest
  return <button className="feed-item" onClick={onOpen}><span className={`feed-item-icon pr-${state.toLowerCase()}`}><ActivityIcon size={24} aria-hidden="true" /></span><div className="feed-item-content"><div className="feed-item-top"><span className="feed-repo">{activityRepo(item) || 'Repository'} <b>#{item.number}</b></span><ExternalLink size={13} /></div><strong className="feed-title">{item.title}</strong><div className="feed-meta"><span className={`badge ${state.toLowerCase()}`}>{state}</span><span title={exactDate(date)}>{relativeTime(date)}</span></div><div className="feed-detail"><span className="diff positive">+{formatCount(item.additions ?? 0)}</span><span className="diff negative">−{formatCount(item.deletions ?? 0)}</span><span>{item.changed_files ?? item.changedFiles ?? 0} files</span>{CiIcon ? <span className={`ci-state ${ciState}`}><CiIcon size={12} className={ciState === 'pending' ? 'spin' : ''} /> Checks {ciState}</span> : null}</div></div></button>
}

function IssueItem({ item, onOpen }: { item: Issue; onOpen: () => void }) {
  const state = String(item.state || 'open').toUpperCase()
  const date = item.updated_at ?? item.updatedAt ?? item.created_at ?? item.createdAt
  const labels = normalizeLabels(item.labels, item.labels_json ?? item.labelsJson)
  const ActivityIcon = state === 'CLOSED' ? CircleCheck : CircleDot
  return <button className="feed-item issue-item" onClick={onOpen}><span className={`feed-item-icon issue-${state.toLowerCase()}`}><ActivityIcon size={24} aria-hidden="true" /></span><div className="feed-item-content"><div className="feed-item-top"><span className="feed-repo">{activityRepo(item) || 'Repository'} <b>#{item.number}</b></span><ExternalLink size={13} /></div><strong className="feed-title">{item.title}</strong><div className="feed-meta"><span className={`badge ${state.toLowerCase()}`}>{state}</span><span title={exactDate(date)}>Updated {relativeTime(date)}</span></div>{labels.length ? <div className="label-row">{labels.slice(0, 4).map((label) => <span key={label}>{label}</span>)}</div> : null}</div></button>
}

const RepositoryDetails = memo(function RepositoryDetails({ repo, history, historyLoading, metric, range, onMetric, onRange, onBack, pullRequests, issues, onOpenUrl }: { repo: Repository; history: { date: string; timestamp: number; total: number; source: number; tests: number }[]; historyLoading: boolean; metric: 'total' | 'source' | 'tests'; range: TimeRange; onMetric: (metric: 'total' | 'source' | 'tests') => void; onRange: (range: TimeRange) => void; onBack: () => void; pullRequests: PullRequest[]; issues: Issue[]; onOpenUrl: (url: string | undefined) => void }) {
  const isPrivate = repo.is_private ?? repo.isPrivate ?? false
  return <main className="main-column detail-column"><button className="back-button" onClick={onBack}><ArrowLeft size={15} /> Back to repositories</button><div className="detail-heading"><div><p className="eyebrow">Repository details</p><h1>{repoName(repo)}</h1><p className="detail-subtitle">{repositoryLabel(repo)} {repo.primary_language ?? repo.primaryLanguage ? <><span>·</span> {repo.primary_language ?? repo.primaryLanguage}</> : null} <em className={`repo-visibility ${isPrivate ? 'private' : 'public'}`}>{isPrivate ? 'Private' : 'Public'}</em></p></div>{repo.url && <button className="button secondary" onClick={() => onOpenUrl(repo.url)}><Github size={15} /> Open on GitHub <ExternalLink size={14} /></button>}</div><div className="detail-stats"><DetailStat label="Total Lines" value={detailLocValue(repo, repoTotal(repo))} /><DetailStat label="Source Lines" value={detailLocValue(repo, repoSource(repo))} /><DetailStat label="Test Lines" value={detailLocValue(repo, repoTests(repo))} /><DetailStat label="30-day change" value={detailChangeValue(repo)} tone={repoLocAvailable(repo) && repoBaseline30Available(repo) ? (repoChange(repo) >= 0 ? 'positive' : 'negative') : undefined} title={repoGrowthTitle(repo)} /><DetailStat label="Open PRs" value={formatCount(repoOpenPrs(repo))} /><DetailStat label="Open issues" value={formatCount(repoOpenIssues(repo))} /><DetailStat label="Stars" value={formatCount(repoStars(repo))} /><DetailStat label="Forks" value={formatCount(repoForks(repo))} /><DetailStat label="Last activity" value={relativeTime(repoActivity(repo))} title={exactDate(repoActivity(repo))} /></div>{historyLoading ? <div className="detail-loading"><LoaderCircle size={16} className="spin" /> Loading repository history…</div> : <HistoryChart history={history} metric={metric} range={range} onMetric={onMetric} onRange={onRange} />}<div className="detail-activity"><ActivityList title="Recent pull requests" items={pullRequests.slice(0, 5)} kind="prs" onOpenUrl={onOpenUrl} /><ActivityList title="Recent issues" items={issues.slice(0, 5)} kind="issues" onOpenUrl={onOpenUrl} /></div></main>
})

function DetailStat({ label, value, tone, title }: { label: string; value: string; tone?: string; title?: string }) {
  return <div className="detail-stat"><span>{label}</span><strong className={tone ?? ''} title={title}>{value}</strong></div>
}

function ActivityList({ title, items, kind, onOpenUrl }: { title: string; items: Array<PullRequest | Issue>; kind: FeedKind; onOpenUrl: (url: string | undefined) => void }) {
  return <section className="detail-list"><div className="list-title"><h3>{title}</h3><span>{items.length}</span></div>{items.length ? items.map((item) => kind === 'prs' ? <PullRequestItem item={item as PullRequest} key={item.number} onOpen={() => onOpenUrl(item.url)} /> : <IssueItem item={item as Issue} key={item.number} onOpen={() => onOpenUrl(item.url)} />) : <div className="detail-empty">No recent activity</div>}</section>
}


export default App
