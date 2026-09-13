import { createPortal } from 'react-dom'
import { useCallback, useEffect, useId, useLayoutEffect, useRef, useState, type MouseEvent, type ReactNode } from 'react'
import { AlertCircle, Check, ChevronDown, ChevronUp, HardDrive, LoaderCircle, ChartNoAxesColumnIncreasing, CodeXml, FlaskConical, GitPullRequest, CircleDot, Sun, Moon, Monitor, X } from 'lucide-react'
import { getDatabaseLocation, getRepositorySelection, revealDatabase } from './api'
import type { AppSettings, MenuBarMetric, RepositorySelection, ThemeMode } from './types'
import type { NormalizedMetrics } from './utils'

const METRICS: { key: MenuBarMetric; label: string; field: keyof NormalizedMetrics; suffix: string }[] = [
  { key: 'total_lines', label: 'Total lines', field: 'total_loc', suffix: 'lines' },
  { key: 'source_lines', label: 'Source lines', field: 'source_loc', suffix: 'source' },
  { key: 'test_lines', label: 'Test lines', field: 'test_loc', suffix: 'tests' },
  { key: 'open_prs', label: 'Open PRs', field: 'open_prs', suffix: 'PRs' },
  { key: 'open_issues', label: 'Open issues', field: 'open_issues', suffix: 'issues' }
]

export function menuBarTitle(metric: MenuBarMetric, metrics: NormalizedMetrics): string {
  const option = METRICS.find((item) => item.key === metric) ?? METRICS[0]
  const value = metrics[option.field]
  const number = value >= 1_000_000_000 ? `${(value / 1_000_000_000).toFixed(1)}B` : value >= 1_000_000 ? `${(value / 1_000_000).toFixed(1)}M` : value >= 1_000 ? `${(value / 1_000).toFixed(1)}K` : String(value)
  return `${number} ${option.suffix}`
}

const THEME_OPTIONS: { key: ThemeMode; label: string; icon: typeof Sun }[] = [{ key: 'light', label: 'Light', icon: Sun }, { key: 'dark', label: 'Dark', icon: Moon }, { key: 'system', label: 'Follow system', icon: Monitor }]
const METRIC_ICONS = { total_lines: ChartNoAxesColumnIncreasing, source_lines: CodeXml, test_lines: FlaskConical, open_prs: GitPullRequest, open_issues: CircleDot }

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

function Interval({ label, value, options, onChange }: { label: string; value: number; options: number[]; onChange: (value: number) => void }) {
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
    if (!/^\d+$/.test(draft) || Number(draft) < 1 || Number(draft) > 1440) {
      setError('Enter a whole number from 1 to 1440 minutes.')
      return
    }
    setError(null)
    const minutes = Number(draft)
    setDraft(String(minutes))
    if (minutes !== value) onChange(minutes)
  }
  const customId = `${errorId}-custom`
  const customLabel = `Custom ${label === 'Activity refresh' ? 'activity refresh' : 'line count refresh'} minutes`
  const selectCustom = () => {
    setCustom(true)
    setDraft(String(value))
    setError(null)
    requestAnimationFrame(() => {
      inputRef.current?.focus()
      inputRef.current?.select()
    })
  }
  return <div className="settings-row settings-interval-row"><div className="settings-row-label">{label} <span className="settings-row-unit">(min)</span><Help label={label}>{label === 'Activity refresh' ? 'Checks pull requests and issues while CodeTally is running.' : 'Recounts source and test lines. Longer intervals use fewer resources.'} Choose Custom for any whole number from 1 to 1440; changes apply when you leave the field or press Enter.</Help></div><div className="settings-interval-control"><div className="settings-interval-options" role="radiogroup" aria-label={label}>{options.map((minutes) => <label key={minutes} className={!custom && value === minutes ? 'selected' : ''}><input type="radio" name={label} aria-label={`${minutes} ${minutes === 1 ? 'minute' : 'minutes'}`} checked={!custom && value === minutes} onChange={() => { setCustom(false); setError(null); setDraft(String(minutes)); if (minutes !== value) onChange(minutes) }} /><span>{minutes}</span></label>)}<div className={`settings-custom-option${custom ? ' selected' : ''}`} onClick={() => { if (!custom) selectCustom() }}><input id={customId} type="radio" name={label} aria-label="Custom" checked={custom} onChange={selectCustom} />{custom ? <input ref={inputRef} className="settings-inline-custom" type="text" inputMode="numeric" aria-label={customLabel} aria-invalid={!!error} aria-describedby={error ? errorId : undefined} value={draft} onClick={(event) => event.stopPropagation()} onChange={(event) => { setDraft(event.target.value); setError(null) }} onBlur={commit} onKeyDown={(event) => { if (event.key === 'Enter') { event.preventDefault(); commit() } }} /> : <label htmlFor={customId}>Custom</label>}</div></div>{error && <span id={errorId} role="alert" className="settings-interval-error">{error}</span>}</div></div>
}

type Props = { settings: AppSettings; loaded: boolean; metrics: NormalizedMetrics; saving: boolean; error: string | null; onChange: (update: (current: AppSettings) => AppSettings) => void; onRetry: () => void; onClose: () => void }

export default function SettingsDrawer({ settings, loaded, metrics, saving, error, onChange, onRetry, onClose }: Props) {
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

  const handleRevealDatabase = async (event: MouseEvent<HTMLAnchorElement>) => {
    event.preventDefault()
    setDatabaseRevealError(null)
    try {
      if (!await revealDatabase()) throw new Error('The database could not be revealed.')
    } catch (reason) {
      setDatabaseRevealError(reason instanceof Error ? reason.message : String(reason))
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
        </label>) : <span className="repository-options-empty">No repositories discovered in this group.</span>}
      </div>}
    </section>
  }

  const selectedMetrics = settings.menu_bar_metrics?.length ? settings.menu_bar_metrics : [settings.menu_bar_metric]
  const exploratoryMetric = hoverMetric ?? focusMetric
  const previewMetrics = METRICS.filter((option) => selectedMetrics.includes(option.key) || option.key === exploratoryMetric)
  const toggleMetric = (metric: MenuBarMetric) => {
    if (selectedMetrics.length === 1 && selectedMetrics.includes(metric)) return
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
  return <div className="drawer-backdrop" onClick={onClose}><aside ref={drawerRef} tabIndex={-1} role="dialog" aria-modal="true" aria-label="Settings" className="settings-drawer" onClick={(event) => event.stopPropagation()} onKeyDown={(event) => {
    if (event.key === 'Escape') onClose()
    if (event.key === 'Tab') {
      const controls = [...event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), a[href], summary')].filter((element) => element.getClientRects().length > 0)
      const first = controls[0]; const last = controls[controls.length - 1]
      if (event.shiftKey && (document.activeElement === first || document.activeElement === event.currentTarget)) { event.preventDefault(); last?.focus() }
      else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first?.focus() }
    }
  }}>
    <div className="drawer-heading"><div><p className="eyebrow">Make it yours</p><h2>Settings</h2><span className="settings-status" role="status">{!loaded ? error ? 'Preferences unavailable' : 'Loading preferences…' : saving ? <><LoaderCircle size={12} className="spin" /> Saving…</> : error ? 'Changes not saved' : <><Check size={12} /> Changes save automatically</>}</span></div><button className="icon-button" type="button" aria-label="Close settings" onClick={onClose}><X size={17} /></button></div>
    {error && <div className="settings-error" role="alert"><AlertCircle size={14} /><span>{error}</span><button className="button secondary compact" type="button" onClick={onRetry}>Retry settings</button></div>}
    <fieldset className="settings-controls" disabled={!loaded}>
    <section className="settings-section" id="settings-appearance"><SectionHeading title="Appearance & menu bar" help="Choose one or more portfolio metrics for the macOS menu bar. Each has its own icon. Hover or focus an option to preview adding it without saving. Values come from your locally cached dashboard." />
      <div className="settings-theme-options" role="radiogroup" aria-label="Appearance">{THEME_OPTIONS.map((option) => { const Icon = option.icon; return <label key={option.key} className={`settings-theme-option${settings.theme_mode === option.key ? ' selected' : ''}`}><input type="radio" name="appearance" aria-label={option.label} checked={settings.theme_mode === option.key} onChange={() => onChange((current) => ({ ...current, theme_mode: option.key }))} /><Icon size={18} aria-hidden="true" /><span>{option.label}</span></label> })}</div>
      <div className="settings-row settings-menu-visibility"><div className="settings-row-label">Show in menu bar<Help label="Show in menu bar">Show selected metrics in the macOS menu bar. If hidden, reopen CodeTally from Applications or the Dock.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Show CodeTally in the menu bar" checked={settings.show_menu_bar} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, show_menu_bar: checked })) }} /></div>
      <div className="settings-menu-preview"><div className="settings-menu-preview-strip"><span className="settings-preview-value" aria-label="Menu bar preview">{previewMetrics.map((option, index) => { const Icon = METRIC_ICONS[option.key]; return <span className="settings-preview-metric" key={option.key}>{index > 0 && <span aria-hidden="true"> · </span>}<Icon size={16} aria-hidden="true" />{menuBarTitle(option.key, metrics)}</span> })}</span></div><span className="settings-preview-caption">{exploratoryMetric && !selectedMetrics.includes(exploratoryMetric) ? 'Preview · select to add' : 'Your menu bar'}</span></div>
      <div className="settings-metric-options" role="group" aria-label="Menu bar metrics">{METRICS.map((option) => { const selected = selectedMetrics.includes(option.key); const Icon = METRIC_ICONS[option.key]; return <label key={option.key} className={`settings-metric-option${selected ? ' selected' : ''}`} onMouseEnter={() => setHoverMetric(option.key)} onMouseLeave={() => setHoverMetric(null)}><input type="checkbox" aria-label={option.label} checked={selected} aria-disabled={selected && selectedMetrics.length === 1} onFocus={() => setFocusMetric(option.key)} onBlur={() => setFocusMetric(null)} onChange={() => toggleMetric(option.key)} /><Icon className="settings-metric-icon" size={18} aria-hidden="true" /><span className="settings-metric-copy"><strong>{option.label}</strong><small>{menuBarTitle(option.key, metrics)}</small></span><span className="settings-metric-indicator" aria-hidden="true">{selected && <Check size={12} />}</span></label> })}</div>
      <p className="settings-metric-hint">Select one or more metrics. Keep at least one selected.</p>
    </section>
    <section className="settings-section" id="settings-refresh"><SectionHeading title="Background & refresh" help="Scheduled refreshes continue while CodeTally is running. Background operation keeps the app running when its window closes; it does not launch at login or keep your Mac awake." />
      <div className="settings-row"><div className="settings-row-label">Run in background<Help label="Run in background">Close the window and keep refreshing. Reopen or quit CodeTally from the menu bar.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Keep running in the background when the window closes" checked={settings.run_in_background} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, run_in_background: checked })) }} /></div>
      <Interval label="Activity refresh" value={settings.activity_refresh_minutes} options={[15, 30, 60, 120]} onChange={(value) => onChange((current) => ({ ...current, activity_refresh_minutes: value }))} />
      <Interval label="Line count refresh" value={settings.lines_refresh_minutes} options={[60, 120, 240, 480]} onChange={(value) => onChange((current) => ({ ...current, lines_refresh_minutes: value }))} />
      <div className="settings-row"><div className="settings-row-label">Refresh on code changes<Help label="Refresh on code changes">Also recount a repository when GitHub reports new pushed code during an activity refresh.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Refresh line counts when code changes" checked={settings.refresh_lines_on_change} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, refresh_lines_on_change: checked })) }} /></div>
    </section>
    <section className="settings-section repository-selection-block" id="settings-repositories"><SectionHeading title="Repositories" help="Deselected repositories disappear from the dashboard and future refreshes. Their local cache and history stay on this device. Work already in progress may finish." />
      <div className="settings-row"><div className="settings-row-label">Include forks in totals<Help label="Include forks in totals">Count selected forks in portfolio line totals and history. Off by default.</Help></div><input className="mac-switch" role="switch" type="checkbox" aria-label="Include forks in line totals" checked={settings.include_forks_in_totals} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, include_forks_in_totals: checked })) }} /></div>
      {inventoryLoading ? <div className="repository-inventory-state" role="status"><LoaderCircle size={14} className="spin" /> Loading repositories…</div> : inventoryError ? <div className="repository-inventory-state error" role="alert"><AlertCircle size={14} /><span>{inventoryError}</span><button className="button secondary compact" type="button" onClick={() => void loadInventory()}>Retry</button></div> : <div className="repository-groups">{renderGroup('personal', 'Personal repositories', personalRepositories, true)}<label className="setting-checkbox repository-company-switch"><input className="mac-switch" role="switch" type="checkbox" checked={settings.include_company_repositories} onChange={(event) => { const checked = event.target.checked; onChange((current) => ({ ...current, include_company_repositories: checked })) }} /><span>Track company repositories</span></label>{[...organizations.entries()].sort(([left], [right]) => left.localeCompare(right)).map(([key, group]) => renderGroup(`company-${key}`, `${group.owner} repositories`, group.repositories))}</div>}
    </section>
    </fieldset>
    <section className="settings-section" id="settings-storage"><SectionHeading title="Storage & connection" help="Repository data, history, and preferences stay in this local SQLite database. Authentication uses your existing GitHub CLI session; CodeTally never stores a token." />
      <span className="setting-label">SQLite database</span>{databaseLocationLoading ? <div className="setting-value setting-status" role="status"><LoaderCircle size={14} className="spin" /> Loading SQLite location…</div> : databaseLocationError ? <div className="settings-error storage-error" role="alert"><AlertCircle size={14} /><span>{databaseLocationError}</span><button className="button secondary compact" type="button" onClick={() => void loadDatabaseLocation()}>Retry</button></div> : databaseLocation ? <a className="setting-value storage-link" href={databaseLocation} title="Reveal SQLite database in Finder" onClick={handleRevealDatabase}><HardDrive size={14} /><span>{databaseLocation}</span></a> : <div className="setting-value setting-status"><AlertCircle size={14} /> SQLite location unavailable</div>}{databaseRevealError && <div className="settings-error storage-error" role="alert"><AlertCircle size={14} /><span>{databaseRevealError}</span></div>}
      <details className="settings-connection"><summary>GitHub connection</summary><div className="github-command-list"><div className="github-command"><code>gh --version</code><span>Check the GitHub CLI installation.</span></div><div className="github-command"><code>gh auth status</code><span>Check the authentication session.</span></div><div className="github-command"><code>gh api user --jq .login</code><span>Show the signed-in username.</span></div></div></details>
    </section>
  </aside></div>
}
