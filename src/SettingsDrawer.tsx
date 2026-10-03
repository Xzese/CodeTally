import { createPortal } from 'react-dom'
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type MouseEvent, type ReactNode } from 'react'
import { disable as disableAutostart, enable as enableAutostart, isEnabled as isAutostartEnabled } from '@tauri-apps/plugin-autostart'
import { AlertCircle, ArrowRight, Check, ChevronDown, ChevronUp, Coffee, ExternalLink, Github, Globe2, HardDrive, LoaderCircle, ChartNoAxesColumnIncreasing, CodeXml, FlaskConical, GitPullRequest, CircleDot, Sun, Moon, Monitor, X } from 'lucide-react'
import { checkForUpdates, getAppInfo, getDatabaseLocation, getRepositorySelection, installAppUpdate, openExternalUrl, revealDatabase, type AppInfo, type AppUpdate } from './api'
import type { AppSettings, MenuBarMetric, RepositorySelection, ThemeMode, UpdateCheckInterval } from './types'
import type { NormalizedMetrics } from './utils'
import codetallyMark from './assets/codetally-mark.webp'

const METRICS: { key: MenuBarMetric; label: string; field: keyof NormalizedMetrics; suffix: string }[] = [
  { key: 'total_lines', label: 'Total lines', field: 'total_loc', suffix: 'lines' },
  { key: 'source_lines', label: 'Source lines', field: 'source_loc', suffix: 'source' },
  { key: 'test_lines', label: 'Test lines', field: 'test_loc', suffix: 'tests' },
  { key: 'open_prs', label: 'Open PRs', field: 'open_prs', suffix: 'PRs' },
  { key: 'open_issues', label: 'Open issues', field: 'open_issues', suffix: 'issues' }
]

export function menuBarTitle(metric: MenuBarMetric, metrics: NormalizedMetrics, compact = false): string {
  const option = METRICS.find((item) => item.key === metric) ?? METRICS[0]
  const value = metrics[option.field]
  const number = value >= 1_000_000_000 ? `${(value / 1_000_000_000).toFixed(1)}B` : value >= 1_000_000 ? `${(value / 1_000_000).toFixed(1)}M` : value >= 1_000 ? `${(value / 1_000).toFixed(1)}K` : String(value)
  return compact ? number : `${number} ${option.suffix}`
}

const THEME_OPTIONS: { key: ThemeMode; label: string; icon: typeof Sun }[] = [{ key: 'light', label: 'Light', icon: Sun }, { key: 'dark', label: 'Dark', icon: Moon }, { key: 'system', label: 'Follow system', icon: Monitor }]
const UPDATE_CHECK_OPTIONS: { key: UpdateCheckInterval; label: string }[] = [
  { key: 'daily', label: 'Daily' },
  { key: 'weekly', label: 'Weekly' },
  { key: 'monthly', label: 'Monthly' },
  { key: 'never', label: 'Never' }
]
const METRIC_ICONS = { total_lines: ChartNoAxesColumnIncreasing, source_lines: CodeXml, test_lines: FlaskConical, open_prs: GitPullRequest, open_issues: CircleDot }
const WEBSITE_URL = 'https://www.samfaid.com/'
const BUY_ME_A_COFFEE_URL = 'https://www.buymeacoffee.com/samfaid'

function Help({ label, children }: { label: string; children: ReactNode }) {
  const id = useId()
  const [open, setOpen] = useState(false)
  const anchor = useRef<HTMLButtonElement>(null)
  const [position, setPosition] = useState({ left: 12, top: 12 })
  useLayoutEffect(() => {
    if (!open) return
    const place = () => {
      const rect = anchor.current?.getBoundingClientRect()
      if (rect) setPosition({ left: Math.max(12, Math.min(rect.left, window.innerWidth - 272)), top: Math.max(12, Math.min(rect.bottom + 8, window.innerHeight - 150)) })
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => { window.removeEventListener('resize', place); window.removeEventListener('scroll', place, true) }
  }, [open])
  return <span className="settings-help" onMouseEnter={() => setOpen(true)} onMouseLeave={() => setOpen(false)}>
    <button ref={anchor} type="button" aria-label={`About ${label}`} aria-expanded={open} aria-describedby={open ? id : undefined} onFocus={() => setOpen(true)} onBlur={() => setOpen(false)} onClick={() => setOpen(true)} onKeyDown={(event) => { if (event.key === 'Escape') { event.stopPropagation(); setOpen(false) } }}>?</button>
    {open && createPortal(<span id={id} role="tooltip" className="settings-help-tooltip" style={{ position: 'fixed', left: position.left, top: position.top, right: 'auto', bottom: 'auto', width: 'min(260px, calc(100vw - 24px))' }}>{children}</span>, anchor.current?.closest('.app-shell') ?? document.body)}
  </span>
}

function SectionHeading({ title, help }: { title: string; help: string }) {
  return <div className="settings-section-heading"><h3>{title}</h3><Help label={title}>{help}</Help></div>
}

function Interval({ label, value, options, minMinutes = 1, help, onChange }: { label: string; value: number; options: number[]; minMinutes?: number; help: string; onChange: (value: number) => void }) {
  const [custom, setCustom] = useState(!options.includes(value))
  const [draft, setDraft] = useState(String(value))
  const [error, setError] = useState<string | null>(null)
  const inputRef = useRef<HTMLInputElement>(null)
  const errorId = useId()
  useEffect(() => {
    setCustom(!options.includes(value))
    setDraft(String(value))
    setError(null)
  }, [value])
  const commit = () => {
    if (!/^\d+$/.test(draft) || Number(draft) < minMinutes || Number(draft) > 1440) {
      setError(`Enter a whole number between ${minMinutes} and 1440 minutes.`)
      return
    }
    setError(null)
    const minutes = Number(draft)
    setDraft(String(minutes))
    if (minutes !== value) onChange(minutes)
  }
  const customId = `${errorId}-custom`
  const customLabel = `Custom minutes for ${label.toLowerCase()}`
  const selectCustom = () => {
    setCustom(true)
    setDraft(String(value))
    setError(null)
    requestAnimationFrame(() => {
      inputRef.current?.focus()
      inputRef.current?.select()
    })
  }
  return <div className="settings-row settings-interval-row"><div className="settings-row-label">{label}<Help label={label}>{help} Choose Custom for any whole number from {minMinutes} to 1440 minutes. Your change saves when you leave the field or press Enter.</Help></div><div className="settings-interval-control"><div className="settings-interval-options" role="radiogroup" aria-label={label}>{options.map((minutes) => <label key={minutes} className={!custom && value === minutes ? 'selected' : ''}><input type="radio" name={label} aria-label={`${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`} checked={!custom && value === minutes} onChange={() => { setCustom(false); setError(null); setDraft(String(minutes)); if (minutes !== value) onChange(minutes) }} /><span>{minutes}</span></label>)}<div className={`settings-custom-option${custom ? ' selected' : ''}`} onClick={() => { if (!custom) selectCustom() }}><input id={customId} type="radio" name={label} aria-label="Custom" checked={custom} onChange={selectCustom} />{custom ? <input ref={inputRef} className="settings-inline-custom" type="text" inputMode="numeric" aria-label={customLabel} aria-invalid={!!error} aria-describedby={error ? errorId : undefined} value={draft} onClick={(event) => event.stopPropagation()} onChange={(event) => { setDraft(event.target.value); setError(null) }} onBlur={commit} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); commit() } }} /> : <label htmlFor={customId}>Custom</label>}</div></div>{error && <span id={errorId} role="alert" className="settings-interval-error">{error}</span>}</div></div>
}

function UpdateCheckIntervalControl({ value, onChange }: { value: UpdateCheckInterval; onChange: (value: UpdateCheckInterval) => void }) {
  return <div className="settings-row settings-update-interval-row settings-update-checks"><div className="settings-row-label">Check for Updates<Help label="Check for Updates">Check for new app versions automatically. Choose Never to check only when you select Check for Updates.</Help></div><div className="settings-update-interval-options" role="radiogroup" aria-label="Check for Updates">{UPDATE_CHECK_OPTIONS.map((option) => <label key={option.key} className={value === option.key ? 'selected' : ''}><input type="radio" name="app-update-check-interval" aria-label={option.label} checked={value === option.key} onChange={() => onChange(option.key)} /><span>{option.label}</span></label>)}</div></div>
}

function OpenAtLoginControl() {
  const [enabled, setEnabled] = useState(false)
  const [loading, setLoading] = useState(true)
  const [changing, setChanging] = useState(false)
  const [error, setError] = useState<string | null>(null)

  const load = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      if (import.meta.env.VITE_CODETALLY_SCREENSHOT_MODE === '1') {
        setEnabled(false)
        return
      }
      setEnabled(await isAutostartEnabled())
    } catch {
      setError('Couldn’t read the login setting.')
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => { void load() }, [load])

  const change = async (next: boolean) => {
    setChanging(true)
    setError(null)
    try {
      if (import.meta.env.VITE_CODETALLY_SCREENSHOT_MODE === '1') { setEnabled(next); return }
      if (next) await enableAutostart()
      else await disableAutostart()
      setEnabled(next)
    } catch {
      setError('Couldn’t change the login setting.')
    } finally {
      setChanging(false)
    }
  }

  return <>
    <div className="settings-row"><div className="settings-row-label">Open at login<Help label="Open at login">Start CodeTally automatically when you sign in to your Mac. You can also manage this in macOS System Settings.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Open CodeTally at login" checked={enabled} disabled={loading || changing || !!error} onChange={(event) => void change(event.target.checked)} /></div>
    {loading && <div className="setting-status" role="status"><LoaderCircle size={12} className="spin" /> Checking login setting…</div>}
    {error && <div className="settings-error startup-error" role="alert"><AlertCircle size={14} /><span>{error}</span><button className="button secondary compact" type="button" onClick={() => void load()}>Retry</button></div>}
  </>
}

type Props = { login: string; settings: AppSettings; loaded: boolean; metrics: NormalizedMetrics; saving: boolean; error: string | null; onChange: (update: (current: AppSettings) => AppSettings) => void; onRetry: () => void; onClose: () => void; onForceRefresh: () => void; onRefreshAllTickets: () => void; refreshing: boolean }

export default function SettingsDrawer({ login, settings, loaded, metrics, saving, error, onChange, onRetry, onClose, onForceRefresh, onRefreshAllTickets, refreshing }: Props) {
  const [hoverMetric, setHoverMetric] = useState<MenuBarMetric | null>(null)
  const [focusMetric, setFocusMetric] = useState<MenuBarMetric | null>(null)
  const drawerRef = useRef<HTMLElement>(null)
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    drawerRef.current?.focus()
    return () => previous?.focus()
  }, [])
  const [inventory, setInventory] = useState<RepositorySelection[]>([])
  const [inventoryLoading, setInventoryLoading] = useState(true)
  const [inventoryError, setInventoryError] = useState<string | null>(null)
  const [expandedGroups, setExpandedGroups] = useState<Record<string, boolean>>({})
  const [databaseLocation, setDatabaseLocation] = useState<string | null>(null)
  const [databaseLocationLoading, setDatabaseLocationLoading] = useState(true)
  const [databaseLocationError, setDatabaseLocationError] = useState<string | null>(null)
  const [databaseRevealError, setDatabaseRevealError] = useState<string | null>(null)
  const [appInfo, setAppInfo] = useState<AppInfo | null>(null)
  const [appInfoLoading, setAppInfoLoading] = useState(true)
  const [appInfoError, setAppInfoError] = useState<string | null>(null)
  const [aboutLinkError, setAboutLinkError] = useState<string | null>(null)
  const [appUpdate, setAppUpdate] = useState<AppUpdate | null>(null)
  const [appUpdateLoading, setAppUpdateLoading] = useState(true)
  const [appUpdateError, setAppUpdateError] = useState<string | null>(null)
  const [appUpdateInstalling, setAppUpdateInstalling] = useState(false)
  const [releaseLinkError, setReleaseLinkError] = useState<string | null>(null)
  const appUpdateCheckStarted = useRef(false)

  const loadInventory = useCallback(async () => {
    setInventoryLoading(true)
    setInventoryError(null)
    try {
      const repositories = await getRepositorySelection()
      setInventory(repositories)
    } catch (reason) {
      setInventoryError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setInventoryLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadInventory()
  }, [loadInventory])

  const loadDatabaseLocation = useCallback(async () => {
    setDatabaseLocationLoading(true)
    setDatabaseLocationError(null)
    try {
      setDatabaseLocation(await getDatabaseLocation())
    } catch (reason) {
      setDatabaseLocationError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setDatabaseLocationLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadDatabaseLocation()
  }, [loadDatabaseLocation])

  const loadAppInfo = useCallback(async () => {
    setAppInfoLoading(true)
    setAppInfoError(null)
    try {
      setAppInfo(await getAppInfo())
    } catch (reason) {
      setAppInfoError(reason instanceof Error ? reason.message : String(reason))
    } finally {
      setAppInfoLoading(false)
    }
  }, [])

  useEffect(() => {
    void loadAppInfo()
  }, [loadAppInfo])

  const checkAppUpdate = useCallback(async () => {
    setAppUpdateLoading(true)
    setAppUpdateError(null)
    setReleaseLinkError(null)
    setAppUpdate(null)
    try {
      setAppUpdate(await checkForUpdates())
    } catch {
      setAppUpdateError('Couldn’t check for updates. Check your connection and GitHub sign-in, then try again.')
    } finally {
      setAppUpdateLoading(false)
    }
  }, [])

  useEffect(() => {
    if (appUpdateCheckStarted.current) return
    appUpdateCheckStarted.current = true
    void checkAppUpdate()
  }, [checkAppUpdate])

  const handleRevealDatabase = async (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault()
    setDatabaseRevealError(null)
    try {
      if (!await revealDatabase()) throw new Error('The database could not be revealed.')
    } catch (reason) {
      setDatabaseRevealError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  const handleOpenAboutLink = async (event: MouseEvent<HTMLAnchorElement>, url: string) => {
    event.preventDefault()
    setAboutLinkError(null)
    try {
      await openExternalUrl(url)
    } catch (reason) {
      setAboutLinkError(reason instanceof Error ? reason.message : String(reason))
    }
  }

  const handleInstallUpdate = async () => {
    setAppUpdateInstalling(true)
    setReleaseLinkError(null)
    try {
      await installAppUpdate()
    } catch (reason) {
      setReleaseLinkError(reason instanceof Error ? reason.message : String(reason))
      setAppUpdateInstalling(false)
    }
  }

  const personalRepositories = inventory.filter((repository) => repository.group === 'personal')
  const organizations = new Map<string, { owner: string; repositories: RepositorySelection[] }>()
  for (const repository of inventory.filter((repository) => repository.group === 'company')) {
    const key = repository.owner.toLowerCase()
    const group = organizations.get(key) ?? { owner: repository.owner, repositories: [] }
    group.repositories.push(repository)
    organizations.set(key, group)
  }

  const isRepositoryIncluded = (repository: RepositorySelection) => !settings.excluded_repository_ids.map(String).includes(String(repository.github_id))
  const setRepositoryIncluded = (repository: RepositorySelection, included: boolean) => {
    const repositoryId = String(repository.github_id)
    onChange((current) => {
      const excluded = new Set(current.excluded_repository_ids.map(String))
      if (included) excluded.delete(repositoryId)
      else excluded.add(repositoryId)
      return { ...current, excluded_repository_ids: [...excluded] }
    })
  }

  const setRepositoriesIncluded = (repositories: RepositorySelection[], included: boolean) => {
    onChange((current) => {
      const excluded = new Set(current.excluded_repository_ids.map(String))
      for (const repository of repositories) {
        if (included) excluded.delete(String(repository.github_id))
        else excluded.add(String(repository.github_id))
      }
      return { ...current, excluded_repository_ids: [...excluded] }
    })
  }
  const renderGroup = (key: string, label: string, repositories: RepositorySelection[], personal = false) => {
    const enabled = personal ? settings.include_personal_repositories : settings.include_company_repositories
    const selected = repositories.filter(isRepositoryIncluded).length
    const groupId = `repository-group-${key}`
    const expanded = expandedGroups[key] ?? false
    return <section className="repository-group" aria-labelledby={`${groupId}-label`} key={key}>
      <div className="repository-group-header">
        <label className="repository-group-toggle">
          {personal ? <input className="mac-switch" role="switch" type="checkbox" aria-label="Track personal repositories" checked={enabled} onChange={(event) => onChange((current) => ({ ...current, include_personal_repositories: event.target.checked }))} /> : <input className="mac-switch" role="switch" type="checkbox" aria-label={`Track ${label}`} checked={selected > 0} disabled={!enabled} onChange={(event) => setRepositoriesIncluded(repositories, event.target.checked)} />}
          <span id={`${groupId}-label`}>{label}</span>
        </label>
        <button className="repository-group-expander" type="button" aria-expanded={expanded} aria-controls={groupId} aria-label={`${expanded ? 'Collapse' : 'Expand'} ${label}`} onClick={() => setExpandedGroups((current) => ({ ...current, [key]: !expanded }))}>
          <span>{enabled ? selected : 0} selected</span>
          {expanded ? <ChevronUp size={15} /> : <ChevronDown size={15} />}
        </button>
      </div>
      {expanded && <div className="repository-options" id={groupId}>
        {repositories.length ? [...repositories].sort((left, right) => left.name_with_owner.localeCompare(right.name_with_owner)).map((repository) => <label className="repository-option" key={repository.github_id}>
          <input className="mac-checkbox" type="checkbox" aria-label={repository.name_with_owner} checked={isRepositoryIncluded(repository)} disabled={!enabled} onChange={(event) => setRepositoryIncluded(repository, event.target.checked)} />
          <span><small>{repository.owner}</small><strong>{repository.name_with_owner.split('/').slice(1).join('/') || repository.name_with_owner}</strong></span>
        </label>) : <span className="repository-options-empty">No repositories found here.</span>}
      </div>}
    </section>
  }

  const selectedMetrics = settings.menu_bar_metrics?.length ? settings.menu_bar_metrics : [settings.menu_bar_metric]
  const compactMetrics = settings.menu_bar_compact_metrics ?? []
  const exploratoryMetric = hoverMetric ?? focusMetric
  const previewMetrics = METRICS.filter((option) => selectedMetrics.includes(option.key) || option.key === exploratoryMetric)
  const toggleMetric = (metric: MenuBarMetric) => {
    if (selectedMetrics.length === 1 && selectedMetrics.includes(metric)) return
    if (selectedMetrics.includes(metric)) {
      setHoverMetric((current) => current === metric ? null : current)
      setFocusMetric((current) => current === metric ? null : current)
    }
    onChange((current) => {
    const selected = new Set(current.menu_bar_metrics?.length ? current.menu_bar_metrics : [current.menu_bar_metric])
    if (selected.has(metric)) {
      if (selected.size === 1) return current
      selected.delete(metric)
    } else selected.add(metric)
    const next = METRICS.filter((option) => selected.has(option.key)).map((option) => option.key)
    return { ...current, menu_bar_metrics: next, menu_bar_metric: next[0] }
    })
  }
  const toggleCompactMetric = (metric: MenuBarMetric) => onChange((current) => {
    const compact = new Set(current.menu_bar_compact_metrics ?? [])
    if (compact.has(metric)) compact.delete(metric)
    else compact.add(metric)
    return { ...current, menu_bar_compact_metrics: METRICS.filter((option) => compact.has(option.key)).map((option) => option.key) }
  })
  return <div className="drawer-backdrop" onClick={onClose}><aside ref={drawerRef} tabIndex={-1} role="dialog" aria-modal="true" aria-label="Settings" className="settings-drawer" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => {
    if (event.key === 'Escape') onClose()
    if (event.key === 'Tab') {
      const controls = [...event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), a[href], summary')].filter((element) => element.getClientRects().length > 0)
      const first = controls[0]; const last = controls[controls.length - 1]
      if (event.shiftKey && (document.activeElement === first || document.activeElement === event.currentTarget)) { event.preventDefault(); last?.focus() }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus() }
    }
  }}>
    <div className="drawer-heading"><div><p className="eyebrow">Make it yours</p><h2>Settings</h2><span className="settings-status" role="status">{!loaded ? error ? 'Settings unavailable' : 'Loading settings…' : saving ? <><LoaderCircle size={12} className="spin" /> Saving…</> : error ? 'Changes couldn’t be saved' : <><Check size={12} /> Changes save automatically</>}</span></div><button className="icon-button" type="button" aria-label="Close settings" onClick={onClose}><X size={17} /></button></div>
    {error && <div className="settings-error" role="alert"><AlertCircle size={14} /><span>{error}</span><button className="button secondary compact" type="button" onClick={onRetry}>Retry</button></div>}
    <fieldset className="settings-controls" disabled={!loaded}>
    <section className="settings-section" id="settings-appearance"><SectionHeading title="Appearance & menu bar" help="Choose the metrics you want to see in the macOS menu bar. Hover or focus a metric to preview it. Values come from your saved dashboard data." />
      <div className="settings-theme-options" role="radiogroup" aria-label="Appearance">{THEME_OPTIONS.map((option) => { const Icon = option.icon; return <label key={option.key} className={`settings-theme-option${settings.theme_mode === option.key ? ' selected' : ''}`}><input type="radio" name="appearance" aria-label={option.label} checked={settings.theme_mode === option.key} onChange={() => onChange((current) => ({ ...current, theme_mode: option.key }))} /><Icon size={18} aria-hidden="true" /><span>{option.label}</span></label> })}</div>
      <div className="settings-row settings-menu-visibility"><div className="settings-row-label">Show in menu bar<Help label="Show in menu bar">Show your selected metrics in the macOS menu bar. If you hide them, open CodeTally from Applications or the Dock.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Show CodeTally in the menu bar" checked={settings.show_menu_bar} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, show_menu_bar: checked })) }} /></div>
      <div className="settings-row"><div className="settings-row-label">Combined menu bar item<Help label="Combined menu bar item">Use one CodeTally icon. Open it for all dashboard metrics and the lines-of-code trend. Turn this off to show your selected metrics as separate items.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Combined menu bar item" checked={settings.menu_bar_combined} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, menu_bar_combined: checked })) }} /></div>
      {settings.menu_bar_combined ? <div className="combined-menu-preview" aria-label="Combined menu bar preview"><div className="combined-preview-heading"><strong>CodeTally</strong><ChartNoAxesColumnIncreasing size={16} /></div><span className="combined-preview-status"><i /> Dashboard summary</span>{[{ label: 'Total lines', value: metrics.total_loc, icon: CodeXml }, { label: 'Repositories', value: metrics.repositories, icon: HardDrive }, { label: 'Source lines', value: metrics.source_loc, icon: CodeXml }, { label: 'Test lines', value: metrics.test_loc, icon: FlaskConical }, { label: '30-day change', value: metrics.loc_30d_change, icon: ChartNoAxesColumnIncreasing }, { label: 'Open PRs', value: metrics.open_prs, icon: GitPullRequest }, { label: 'Open issues', value: metrics.open_issues, icon: CircleDot }].map(({ label, value, icon: Icon }) => <div className="combined-preview-row" key={label}><Icon size={15} /><strong>{label === '30-day change' && value >= 0 ? '+' : ''}{value.toLocaleString()}</strong><span>{label}</span></div>)}<div className="combined-preview-footer">Open CodeTally <ArrowRight size={14} /></div></div> : <>
      <div className="settings-menu-preview"><div className="settings-menu-preview-strip" tabIndex={0} aria-label="Scroll menu bar preview horizontally"><span className="settings-preview-value" aria-label="Menu bar preview">{previewMetrics.map((option) => { const Icon = METRIC_ICONS[option.key]; const fullTitle = menuBarTitle(option.key, metrics); const compact = compactMetrics.includes(option.key); return <span className={`settings-preview-metric${selectedMetrics.includes(option.key) ? '' : ' is-exploratory'}`} key={option.key} aria-label={compact ? `${fullTitle}, compact` : fullTitle} title={fullTitle}><Icon size={13} aria-hidden="true" /><span>{menuBarTitle(option.key, metrics, compact)}</span></span> })}</span></div><span className="settings-preview-caption">{exploratoryMetric && !selectedMetrics.includes(exploratoryMetric) ? 'Preview · select to add' : 'Your menu bar'}</span></div>
      <div className="settings-metric-options" role="group" aria-label="Menu bar metrics">{METRICS.map((option) => { const selected = selectedMetrics.includes(option.key); const Icon = METRIC_ICONS[option.key]; return <div key={option.key} className="settings-metric-choice" onMouseEnter={() => setHoverMetric(option.key)} onMouseLeave={() => setHoverMetric(null)} onFocusCapture={() => setFocusMetric(option.key)} onBlurCapture={(event) => { if (!event.currentTarget.contains(event.relatedTarget as Node | null)) setFocusMetric(null) }}><label className="settings-metric-compact-control"><span>Compact</span><input className="mac-switch" role="switch" type="checkbox" aria-label={`Compact ${option.label} in menu bar`} checked={compactMetrics.includes(option.key)} onChange={() => toggleCompactMetric(option.key)} /></label><label className={`settings-metric-option${selected ? ' selected' : ''}`}><input type="checkbox" aria-label={option.label} checked={selected} aria-disabled={selected && selectedMetrics.length === 1} onChange={() => toggleMetric(option.key)} /><Icon className="settings-metric-icon" size={18} aria-hidden="true" /><span className="settings-metric-copy"><strong>{option.label}</strong><small>{menuBarTitle(option.key, metrics)}</small></span><span className="settings-metric-indicator" aria-hidden="true">{selected && <Check size={12} />}</span></label></div> })}</div>
      <p className="settings-metric-hint">Choose the metrics you want to see. Keep at least one selected.</p></>}
    </section>
    <section className="settings-section"><SectionHeading title="Kanban" help="Combine pull requests and issues from your tracked repositories on one board. Manual columns, priorities, and notes stay on this Mac and do not update GitHub." />
      <div className="settings-row"><label htmlFor="kanban-enabled">Enable Kanban board</label><input id="kanban-enabled" className="mac-switch" role="switch" type="checkbox" checked={settings.kanban_enabled} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, kanban_enabled: checked })) }} /></div>
      <p className="settings-metric-hint">See tracked GitHub pull requests and issues together. Columns, priorities, and notes stay on this computer.</p>
    </section>
    <section className="settings-section" id="settings-refresh"><SectionHeading title="Background & refresh" help="Choose when CodeTally refreshes your GitHub activity and line counts." />
      <OpenAtLoginControl />
      <div className="settings-row"><div className="settings-row-label">Run in background<Help label="Run in background">Keep CodeTally running from the menu bar without a Dock icon.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Keep running in the background when the window closes" checked={settings.run_in_background} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, run_in_background: checked })) }} /></div>
      <div className="settings-row settings-interval-row"><div className="settings-row-label">Repo Refresh<Help label="Repo Refresh">Choose how often CodeTally refreshes repositories, tickets, and line counts. Daily is the default.</Help></div><div className="settings-interval-control"><div className="settings-update-interval-options" role="radiogroup" aria-label="Repo Refresh">{[{ value: 60, label: 'Hourly' }, { value: 1440, label: 'Daily' }, { value: 10080, label: 'Weekly' }, { value: 43200, label: 'Monthly' }].map((option) => <label key={option.value} className={settings.activity_refresh_minutes === option.value ? 'selected' : ''}><input type="radio" name="repo-refresh" aria-label={option.label} checked={settings.activity_refresh_minutes === option.value} onChange={() => onChange((current) => ({ ...current, activity_refresh_minutes: option.value }))} /><span>{option.label}</span></label>)}</div>{settings.activity_refresh_minutes === 60 && <p className="settings-interval-warning" role="status">Hourly repository refreshes can use more GitHub API requests and may reach API limits. Daily is recommended for most repositories.</p>}</div></div>
      <Interval label="PR & Issue Refresh" value={settings.personal_refresh_minutes} options={[5, 15, 30, 60]} minMinutes={5} help="Refresh pull requests and issues involving you, plus the menu bar PR and issue counts across tracked repositories. The default is every 5 minutes. This does not scan lines of code." onChange={(value) => onChange((current) => ({ ...current, personal_refresh_minutes: value }))} />
      <div className="settings-row"><div className="settings-row-label">Repo Refresh now<Help label="Force Repo Refresh">Refresh tracked repositories, pull requests, issues, and line counts now.</Help></div><button className="button primary" disabled={refreshing || !loaded} onClick={onForceRefresh}>{refreshing ? 'Refreshing…' : 'Force Refresh'}</button></div>
      <div className="settings-row"><div className="settings-row-label">All tracked tickets<Help label="All tracked tickets">Refresh PRs and issues across every tracked repository without scanning lines of code. This may take longer than the personal Refresh Tickets action.</Help></div><button className="button secondary" disabled={refreshing || !loaded} onClick={onRefreshAllTickets}>Refresh All Tickets</button></div>
      <div className="settings-row"><div className="settings-row-label">Refresh on code changes<Help label="Refresh on code changes">Update line counts when a repository changes.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Refresh Lines of Code when code changes" checked={settings.refresh_lines_on_change} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, refresh_lines_on_change: checked })) }} /></div>
    </section>
    <section className="settings-section repository-selection-block" id="settings-repositories"><SectionHeading title="Repositories" help="Choose which repositories to include. Repositories you leave out won’t appear in the dashboard or refresh, but their saved data stays on this Mac." />
      <div className="settings-row"><div className="settings-row-label">Include forks in totals<Help label="Include forks in totals">Include selected forks when counting Lines of Code. This is off by default.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Include forks in line totals" checked={settings.include_forks_in_totals} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, include_forks_in_totals: checked })) }} /></div>
      {inventoryLoading ? <div className="repository-inventory-state" role="status"><LoaderCircle size={14} className="spin" /> Loading repositories…</div> : inventoryError ? <div className="repository-inventory-state error" role="alert"><AlertCircle size={14} /><span>{inventoryError}</span><button className="button secondary compact" type="button" onClick={() => void loadInventory()}>Retry</button></div> : <div className="repository-groups">{renderGroup('personal', 'Personal repositories', personalRepositories, true)}<label className="setting-checkbox repository-company-switch"><input className="mac-switch" role="switch" type="checkbox" checked={settings.include_company_repositories} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, include_company_repositories: checked })) }} /><span>Include company repositories</span></label>{[...organizations.entries()].sort(([left], [right]) => left.localeCompare(right)).map(([key, group]) => renderGroup(`company-${key}`, `${group.owner} repositories`, group.repositories))}</div>}
    </section>
    </fieldset>
    <section className="settings-section" id="settings-storage"><SectionHeading title="Storage & connection" help="Your repository data, history, and preferences are stored on this Mac. CodeTally uses your existing GitHub sign-in and never stores your token." />
      <div className="settings-row"><span className="settings-row-label">GitHub account</span><span className="settings-account-name">{login || 'Not signed in'}</span></div>
      <span className="setting-label">Data location</span>{databaseLocationLoading ? <div className="setting-value setting-status" role="status"><LoaderCircle size={14} className="spin" /> Loading location…</div> : databaseLocationError ? <div className="settings-error storage-error" role="alert"><AlertCircle size={14} /><span>{databaseLocationError}</span><button className="button secondary compact" type="button" onClick={() => void loadDatabaseLocation()}>Retry</button></div> : databaseLocation ? <a className="setting-value storage-link" href={databaseLocation} title="Show data location in Finder" onClick={handleRevealDatabase}><HardDrive size={14} /><span>{databaseLocation}</span></a> : <div className="setting-value setting-status"><AlertCircle size={14} /> Data location unavailable</div>}{databaseRevealError && <div className="settings-error storage-error" role="alert"><AlertCircle size={14} /><span>{databaseRevealError}</span></div>}
      <details className="settings-connection"><summary>GitHub connection details</summary><div className="github-command-list"><div className="github-command"><code>gh --version</code><span>Check that GitHub CLI is installed.</span></div><div className="github-command"><code>gh auth status</code><span>Check your GitHub sign-in.</span></div><div className="github-command"><code>gh api user --jq .login</code><span>Show the GitHub account in use.</span></div></div></details>
    </section>
    <section className="settings-section settings-about" id="settings-about" aria-labelledby="settings-about-heading">
      <div className="settings-section-heading"><h3 id="settings-about-heading">About CodeTally</h3></div>
      {appInfoLoading ? <div className="settings-about-state" role="status"><LoaderCircle size={14} className="spin" /> Loading app details…</div> : appInfoError ? <div className="settings-about-state error" role="alert"><AlertCircle size={14} /><span>{appInfoError}</span><button className="button secondary compact" type="button" onClick={() => void loadAppInfo()}>Retry</button></div> : appInfo ? <div className="settings-about-content">
        <div className="settings-about-intro"><img src={codetallyMark} alt="" aria-hidden="true" /><div><strong>{appInfo.name}</strong><p>See your GitHub repositories, activity, and code history in one place.</p></div></div>
        <dl className="settings-about-details"><div><dt>Version</dt><dd className="settings-about-version-cell"><span className="settings-about-version"><span>{appInfo.version}</span>{appUpdate?.update_available && <><ArrowRight size={13} aria-hidden="true" /><span className="update-available">{appUpdate.latest_version}</span></>}</span>{appUpdate?.update_available && <button type="button" className="button primary compact" disabled={appUpdateInstalling} onClick={() => void handleInstallUpdate()}>{appUpdateInstalling ? <LoaderCircle size={14} className="spin" /> : <ExternalLink size={14} />} {appUpdateInstalling ? 'Installing…' : 'Download update'}</button>}</dd></div><div><dt>App identifier</dt><dd>{appInfo.identifier}</dd></div></dl>
        {appUpdateError && <div className="settings-about-state error" role="alert"><AlertCircle size={14} /><span>{appUpdateError}</span><button className="button secondary compact" type="button" onClick={() => void checkAppUpdate()}>Try again</button></div>}
        {releaseLinkError && <div className="settings-error settings-about-link-error" role="alert"><AlertCircle size={14} /><span>{releaseLinkError}</span></div>}
        <div className="settings-about-links">
          <button type="button" className="button secondary compact" disabled={appUpdateInstalling || appUpdateLoading} onClick={() => void checkAppUpdate()}>Check for Updates</button>
          {appInfo.repository_url && <a className="button secondary compact settings-about-link" href={appInfo.repository_url} onClick={(event) => void handleOpenAboutLink(event, appInfo.repository_url)}><Github size={14} /> View on GitHub <ExternalLink size={12} /></a>}
        </div>
        <UpdateCheckIntervalControl value={settings.update_check_interval} onChange={(value) => onChange((current) => ({ ...current, update_check_interval: value }))} />
        {aboutLinkError && <div className="settings-error settings-about-link-error" role="alert"><AlertCircle size={14} /><span>{aboutLinkError}</span></div>}
        <div className="settings-about-me">
          <h4>About me</h4>
          <p>I’m Sam, an independent UK software developer. I turn ideas into privacy-first apps and other useful products.</p>
          <div className="settings-about-me-actions"><a className="button secondary compact settings-about-link" href={WEBSITE_URL} onClick={(event) => void handleOpenAboutLink(event, WEBSITE_URL)}><Globe2 size={14} /> Visit www.samfaid.com <ExternalLink size={12} /></a><a className="settings-coffee-button" href={BUY_ME_A_COFFEE_URL} onClick={(event) => void handleOpenAboutLink(event, BUY_ME_A_COFFEE_URL)}><Coffee size={19} /><span>Buy me a coffee</span><ExternalLink size={13} /></a></div>
        </div>
      </div> : null}
    </section>
  </aside></div>
}
