import { GitPullRequest, CircleDot } from 'lucide-react'
import { useCallback, useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import { getKanbanLinks, getKanbanPage, getKanbanPreferences, setKanbanPreferences, openExternalUrl, setKanbanMetadata, syncWorkItems } from '../api'
import ActivityRepositoryMenu, { activityScopeRepositories } from '../ActivityRepositoryMenu'
import ActivityInvolvementFilter from '../ActivityInvolvementFilter'
import type { ActivityRelationship, KanbanItem, KanbanKind, KanbanLinks, KanbanMetadata, KanbanPage, Repository } from '../types'
import { relativeTime, repositoryId } from '../utils'
import { COLUMNS, columnFor, isGitHubWorkUrl, type Column } from './model'

export default function KanbanBoard({ repositories, login, relationship: defaultRelationship, revision }: { repositories: Repository[]; login: string; relationship: ActivityRelationship; onRelationship: (value: ActivityRelationship) => void; onBack: () => void; revision: number }) {
  const [kind, setKind] = useState<KanbanKind>('both')
  const [scope, setScope] = useState('all')
  const [relationship, setRelationship] = useState(defaultRelationship)
  const [preferencesReady, setPreferencesReady] = useState(false)
  const preferenceQueue = useRef(Promise.resolve())
  const [search, setSearch] = useState('')
  const [completed, setCompleted] = useState(false)
  const [page, setPage] = useState<KanbanPage | null>(null)
  const [loading, setLoading] = useState(false)
  const [refreshing, setRefreshing] = useState(false)
  const [ordering, setOrdering] = useState(false)
  const [draggedKey, setDraggedKey] = useState<string | null>(null)
  const [dropTarget, setDropTarget] = useState<{ column: Column; index: number } | null>(null)
  const dropTargetRef = useRef<{ column: Column; index: number } | null>(null)
  const suppressClickRef = useRef<string | null>(null)
  const [blockedMove, setBlockedMove] = useState<KanbanItem | null>(null)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState<string | null>(null)
  const [pending, setPending] = useState<Set<string>>(new Set())
  const pendingRef = useRef(new Set<string>())
  const generation = useRef(0)
  const ids = JSON.stringify(activityScopeRepositories(scope, repositories, login).map((repo) => Number(repositoryId(repo))).filter(Number.isFinite))
  useEffect(() => {
    let alive = true
    void getKanbanPreferences().then((preferences) => {
      if (!alive) return
      setKind(preferences.kind); setScope(preferences.repository_scope)
      setRelationship(preferences.relationship); setSearch(preferences.search); setCompleted(preferences.show_completed)
    }).catch((reason) => { if (alive) setError(`Could not load board preferences: ${String(reason)}`) }).finally(() => { if (alive) setPreferencesReady(true) })
    return () => { alive = false }
  }, [])
  useEffect(() => {
    if (!preferencesReady) return
    let saved = false
    const persist = () => {
      if (saved) return
      saved = true
      preferenceQueue.current = preferenceQueue.current.then(async () => {
        await setKanbanPreferences({ kind, repository_scope: scope, relationship, search, show_completed: completed })
      }).catch((reason) => setError(`Could not save board preferences: ${String(reason)}`))
    }
    const timer = window.setTimeout(persist, 300)
    return () => { window.clearTimeout(timer); persist() }
  }, [preferencesReady, kind, scope, relationship, search, completed])
  const load = useCallback(async (append = false, offset = 0) => {
    const token = ++generation.current
    setLoading(true)
    setError('')
    try {
      const next = await getKanbanPage({ kind, repository_ids: JSON.parse(ids) as number[], relationship, search, show_completed: completed, offset, limit: 200 })
      if (token === generation.current) setPage((current) => ({ ...next, items: append && current ? [...current.items, ...next.items.filter((item) => !current.items.some((existing) => existing.item_key === item.item_key))] : next.items }))
    } catch (reason) { if (token === generation.current) setError(String(reason)) }
    finally { if (token === generation.current) setLoading(false) }
  }, [kind, ids, relationship, search, completed])
  useEffect(() => { if (!preferencesReady) return; setPage(null); setSelected(null); void load(); return () => { generation.current++ } }, [load, revision, preferencesReady])
  const save = async (item: KanbanItem, changes: Partial<KanbanMetadata>) => {
    if (pendingRef.current.has(item.item_key)) return false
    pendingRef.current.add(item.item_key); setPending(new Set(pendingRef.current))
    const optimistic = { ...item, ...changes }
    setPage((current) => current && ({ ...current, items: current.items.map((row) => row.item_key === item.item_key ? optimistic : row) }))
    try {
      const metadata = await setKanbanMetadata({ item_key: item.item_key, manual_column: optimistic.manual_column, priority: optimistic.priority, notes: optimistic.notes, sort_rank: optimistic.sort_rank, expected_revision: item.revision })
      setPage((current) => current && ({ ...current, items: current.items.map((row) => row.item_key === item.item_key && row.revision <= metadata.revision ? { ...row, ...metadata } : row) }))
      return true
    } catch (reason) {
      setPage((current) => current && ({ ...current, items: current.items.map((row) => row.item_key === item.item_key && row.revision === item.revision ? { ...row, manual_column: item.manual_column, priority: item.priority, notes: item.notes, sort_rank: item.sort_rank } : row) }))
      setError(`Could not save local changes. ${String(reason)} Reload the board before retrying.`)
      return false
    } finally { pendingRef.current.delete(item.item_key); setPending(new Set(pendingRef.current)) }
  }
  const cardsIn = (column: Column) => (page?.items ?? []).filter((row) => columnFor(row) === column).sort((a, b) => a.sort_rank - b.sort_rank || b.updated_at.localeCompare(a.updated_at) || a.item_key.localeCompare(b.item_key))
  const clearDrag = () => { dropTargetRef.current = null; setDraggedKey(null); setDropTarget(null) }
  const place = async (item: KanbanItem, column: Column, index: number) => {
    if (column === 'Done' || columnFor(item) === 'Done') { setBlockedMove(item); return }
    const currentColumn = columnFor(item)
    const sourceIndex = cardsIn(currentColumn).findIndex((row) => row.item_key === item.item_key)
    if (currentColumn === column && sourceIndex === index) return
    const ordered = cardsIn(column).filter((row) => row.item_key !== item.item_key)
    ordered.splice(index, 0, item)
    const before = ordered[index - 1]?.sort_rank
    const after = ordered[index + 1]?.sort_rank
    const rank = before === undefined ? after === undefined ? 0 : after - 1024 : after === undefined ? before + 1024 : Math.floor((before + after) / 2)
    setOrdering(true)
    try {
      if (Math.abs(rank) <= 1_000_000_000 && (before === undefined || rank > before) && (after === undefined || rank < after)) {
        await save(item, { manual_column: column, sort_rank: rank })
      } else {
        // Rebalance only when adjacent integer ranks have no room between them.
        for (let position = 0; position < ordered.length; position++) {
          const row = ordered[position], nextRank = -100_000_000 + position * 1024
          if ((row.sort_rank !== nextRank || row.item_key === item.item_key || currentColumn !== column) && !await save(row, { sort_rank: nextRank, ...(row.item_key === item.item_key ? { manual_column: column } : {}) })) break
        }
      }
    } finally { setOrdering(false); void load() }
  }
  const insertionIndex = (element: HTMLElement, column: Column, dragged: KanbanItem, clientY: number) => {
    const visible = cardsIn(column).filter((row) => row.item_key !== dragged.item_key)
    const nodes = Array.from(element.querySelectorAll<HTMLElement>('.kanban-card')).filter((node) => node.dataset.itemKey !== dragged.item_key)
    const index = nodes.findIndex((node) => clientY < node.getBoundingClientRect().top + node.getBoundingClientRect().height / 2)
    return index < 0 ? visible.length : index
  }
  const columnAt = (clientX: number, clientY: number) => {
    const element = document.elementFromPoint(clientX, clientY)?.closest<HTMLElement>('.kanban-column')
    const column = element?.dataset.kanbanColumn as Column | undefined
    return column && COLUMNS.includes(column) ? { element: element!, column } : null
  }
  const updateDropTarget = (dragged: KanbanItem, clientX: number, clientY: number) => {
    const hit = columnAt(clientX, clientY)
    if (!hit || hit.column === 'Done') { dropTargetRef.current = null; setDropTarget(null); return hit?.column ?? null }
    const index = insertionIndex(hit.element, hit.column, dragged, clientY)
    const sourceIndex = cardsIn(hit.column).findIndex((row) => row.item_key === dragged.item_key)
    const next = sourceIndex === index ? null : { column: hit.column, index }
    dropTargetRef.current = next
    setDropTarget((current) => current?.column === next?.column && current?.index === next?.index ? current : next)
    return hit.column
  }
  const startPointerDrag = (event: ReactPointerEvent<HTMLElement>, item: KanbanItem) => {
    if (event.button !== 0 || ordering || pendingRef.current.has(item.item_key) || columnFor(item) === 'Done') return
    const { pointerId, clientX, clientY } = event
    const originalUserSelect = document.body.style.userSelect
    let started = false
    const cleanup = () => {
      window.removeEventListener('pointermove', move)
      window.removeEventListener('pointerup', end)
      window.removeEventListener('pointercancel', cancel)
      document.body.style.userSelect = originalUserSelect
    }
    const move = (pointer: PointerEvent) => {
      if (pointer.pointerId !== pointerId) return
      if (!started && Math.hypot(pointer.clientX - clientX, pointer.clientY - clientY) < 6) return
      if (!started) { started = true; document.body.style.userSelect = 'none'; setDraggedKey(item.item_key) }
      pointer.preventDefault()
      updateDropTarget(item, pointer.clientX, pointer.clientY)
    }
    const end = (pointer: PointerEvent) => {
      if (pointer.pointerId !== pointerId) return
      if (started) {
        pointer.preventDefault()
        const column = updateDropTarget(item, pointer.clientX, pointer.clientY)
        const target = dropTargetRef.current
        suppressClickRef.current = item.item_key
        window.setTimeout(() => { if (suppressClickRef.current === item.item_key) suppressClickRef.current = null }, 0)
        clearDrag()
        if (column === 'Done') setBlockedMove(item)
        else if (target) void place(item, target.column, target.index)
      }
      cleanup()
    }
    const cancel = (pointer: PointerEvent) => { if (pointer.pointerId === pointerId) { clearDrag(); cleanup() } }
    window.addEventListener('pointermove', move, { passive: false })
    window.addEventListener('pointerup', end)
    window.addEventListener('pointercancel', cancel)
  }
  const refresh = async () => {
    setRefreshing(true); setError('')
    let failure = ''
    try { const result = await syncWorkItems(); if (!result.ok || result.errors.length) failure = [result.message, ...result.errors].join(' ') }
    catch (reason) { failure = String(reason) }
    finally { await load(); if (failure) setError(failure); setRefreshing(false) }
  }
  const item = page?.items.find((row) => row.item_key === selected)
  return <main className="kanban-main">
    <div className="page-heading"><div><p className="eyebrow">Local workflow</p><h1>Kanban</h1></div><button className="button primary" disabled={refreshing || loading || pending.size > 0} onClick={() => void refresh()}>{refreshing ? 'Refreshing…' : 'Refresh Tickets'}</button></div>
    <p className="small-note">Last successful activity refresh: {relativeTime(page?.last_successful_refresh)} · Refresh fetches PRs and issues from all tracked repositories. Local changes do not update GitHub.</p>
    <div className="kanban-filters"><div className="kanban-kind-buttons" role="group" aria-label="Ticket types"><button className={kind === 'prs' || kind === 'both' ? 'active' : ''} aria-pressed={kind === 'prs' || kind === 'both'} onClick={() => setKind(kind === 'both' ? 'issues' : kind === 'prs' ? 'none' : kind === 'issues' ? 'both' : 'prs')}><GitPullRequest size={14} aria-hidden="true" /> PRs</button><button className={kind === 'issues' || kind === 'both' ? 'active' : ''} aria-pressed={kind === 'issues' || kind === 'both'} onClick={() => setKind(kind === 'both' ? 'prs' : kind === 'issues' ? 'none' : kind === 'prs' ? 'both' : 'issues')}><CircleDot size={14} aria-hidden="true" /> Issues</button></div>
      <ActivityRepositoryMenu repositories={repositories} login={login} value={scope} disabled={false} onChange={setScope} />
      <ActivityInvolvementFilter id="kanban-involvement-select" value={relationship} login={login} onChange={setRelationship} />
      <input aria-label="Search board" placeholder="Search work items" value={search} onChange={(event) => setSearch(event.target.value)} />
      <button className={completed ? 'button secondary compact kanban-completed active' : 'button secondary compact kanban-completed'} aria-pressed={completed} onClick={() => setCompleted(!completed)}>Show completed</button>
    </div>
    <div className="kanban-summary" aria-live="polite"><span>{page?.active_count ?? 0} cached active · {page?.completed_count ?? 0} cached completed</span>{loading && <span>Loading board…</span>}</div>
    {blockedMove && <div role="alert" className="kanban-notice">Done reflects GitHub completion. Close or reopen this item on GitHub.<button className="button secondary compact" disabled={!isGitHubWorkUrl(blockedMove.url)} onClick={() => void openExternalUrl(blockedMove.url).catch((reason) => setError(String(reason)))}>Open in GitHub</button><button className="button secondary compact" onClick={() => setBlockedMove(null)}>Dismiss</button></div>}
    {error && <div role="alert" className="error-banner">{error}</div>}
    {page?.partial && <div role="status" className="kanban-notice">Activity data is partial. {page.errors.join(' ')}</div>}
    {!loading && page && !page.items.length && <p className="kanban-notice">{kind === 'none' ? 'Select PRs or Issues to show work items.' : 'No work items match these filters. Refresh activity to fetch the latest GitHub data.'}</p>}
    <div className="kanban-columns">{COLUMNS.filter((column) => completed || column !== 'Done').map((column) => {
      const cards = cardsIn(column)
      const rendered: Array<KanbanItem | null> = [...cards]
      if (dropTarget?.column === column) {
        const sourceIndex = cards.findIndex((card) => card.item_key === draggedKey)
        rendered.splice(dropTarget.index + (sourceIndex >= 0 && sourceIndex <= dropTarget.index ? 1 : 0), 0, null)
      }
      return <section className="kanban-column" data-kanban-column={column} key={column} aria-label={`${column} column`}>
        <h2>{column} <small>{cards.length}{page && page.items.length < page.total ? ' loaded' : ''}</small></h2>{rendered.map((card) => card === null ? <div key="drop-placeholder" className="kanban-drop-placeholder" role="status" aria-label={`Drop in ${column}`} /> : <article className={card.item_key === draggedKey ? 'kanban-card kanban-card-dragging' : 'kanban-card'} data-item-key={card.item_key} key={card.item_key} tabIndex={0} aria-keyshortcuts="Alt+ArrowUp Alt+ArrowDown Alt+ArrowLeft Alt+ArrowRight" title="Drag to move. With keyboard focus, use Alt and arrow keys to move." onPointerDown={(event) => startPointerDrag(event, card)} onClickCapture={(event) => { if (suppressClickRef.current === card.item_key) { event.preventDefault(); event.stopPropagation(); suppressClickRef.current = null } }} onKeyDown={(event) => {
          if (!event.altKey || ordering || pending.size || column === 'Done') return
          const direction = event.key === 'ArrowUp' ? -1 : event.key === 'ArrowDown' ? 1 : 0
          if (direction) {
            event.preventDefault()
            const index = cards.findIndex((row) => row.item_key === card.item_key) + direction
            if (index >= 0 && index < cards.length) void place(card, column, index)
          } else if (event.key === 'ArrowLeft' || event.key === 'ArrowRight') {
            event.preventDefault()
            const nextColumn = COLUMNS[COLUMNS.indexOf(column) + (event.key === 'ArrowLeft' ? -1 : 1)]
            if (nextColumn) void place(card, nextColumn, cardsIn(nextColumn).length)
          }
        }}>
          <button className="kanban-card-title" onClick={() => setSelected(card.item_key)}>{card.title}</button><span className="small-note">{card.kind === 'pr' ? <GitPullRequest size={13} aria-label="Pull request" /> : <CircleDot size={13} aria-label="Issue" />}{card.kind === 'pr' ? 'PR' : 'Issue'} · {card.repository} #{card.number}</span><div className="kanban-badges">{card.unavailable && <span>Unavailable · cached state</span>}<span>{card.merged_at || card.state.toLowerCase() === 'merged' ? 'Merged' : columnFor(card) === 'Done' ? card.kind === 'pr' ? 'Closed without merge' : card.completion_reason ?? 'Closed' : card.state}{card.is_draft ? ' · Draft' : ''}</span>{card.priority !== 'None' && <span>{card.priority}</span>}{card.ci_state && <span>CI: {card.ci_state}</span>}{card.notes && <span>Local notes</span>}</div><span className="small-note">{card.author ? `@${card.author} · ` : ''}{relativeTime(card.updated_at)}</span>
        </article>)}
      </section>
    })}</div>
    {page && page.items.length < page.total && <button className="button secondary" disabled={loading} onClick={() => void load(true, page.items.length)}>Load more ({page.items.length} of {page.total})</button>}
    {item && <ItemDetails key={item.item_key} item={item} saving={pending.has(item.item_key)} onClose={() => setSelected(null)} onSave={(changes) => void save(item, changes)} />}
  </main>
}

function ItemDetails({ item, saving, onClose, onSave }: { item: KanbanItem; saving: boolean; onClose: () => void; onSave: (changes: Partial<KanbanMetadata>) => void }) {
  const dialog = useRef<HTMLElement>(null)
  useEffect(() => { const previous = document.activeElement as HTMLElement | null; dialog.current?.focus(); return () => previous?.focus() }, [])
  const [notes, setNotes] = useState(item.notes)
  const [priority, setPriority] = useState(item.priority)
  const [links, setLinks] = useState<KanbanLinks | null>(null)
  const [error, setError] = useState('')
  useEffect(() => { let alive = true; void getKanbanLinks(item.item_key).then((result) => { if (alive) setLinks(result) }).catch((reason) => { if (alive) setError(String(reason)) }); return () => { alive = false } }, [item.item_key])
  useEffect(() => { const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') onClose() }; window.addEventListener('keydown', escape); return () => window.removeEventListener('keydown', escape) }, [onClose])
  return <div className="kanban-detail-backdrop"><section ref={dialog} tabIndex={-1} onKeyDown={(event) => {
    if (event.key !== 'Tab') return
    const targets = dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), select:not(:disabled), textarea:not(:disabled)')
    if (!targets?.length) return
    const first = targets[0], last = targets[targets.length - 1]
    if (event.shiftKey && (document.activeElement === first || document.activeElement === dialog.current)) { event.preventDefault(); last.focus() }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
  }} className="kanban-detail" role="dialog" aria-modal="true" aria-label="Work item details"><button className="button secondary compact" onClick={onClose}>Close details</button><h2>{item.title}</h2>{item.unavailable && <p role="status">Repository unavailable. Showing the last cached state.</p>}<p>{item.repository} #{item.number} · {item.state}{item.completion_reason ? ` · ${item.completion_reason}` : ''}</p><p>Author: {item.author ?? 'Unknown'} · Assignees: {item.assignees.join(', ') || 'None'}</p><button className="button secondary" disabled={!isGitHubWorkUrl(item.url)} title={!isGitHubWorkUrl(item.url) ? 'Invalid GitHub work item URL' : undefined} onClick={() => void openExternalUrl(item.url).catch((reason) => setError(String(reason)))}>Open on GitHub ↗</button>
    {columnFor(item) !== 'Done' && <div className="kanban-automatic"><p>Column: {columnFor(item)} · {item.manual_column ? 'Local override' : 'Automatic'}</p><button className="button secondary" disabled={saving || item.manual_column === null} onClick={() => onSave({ manual_column: null })}>Reset to automatic</button></div>}
    <label>Local priority<select value={priority} onChange={(event) => setPriority(event.target.value)}>{['None', 'Low', 'Medium', 'High'].map((value) => <option key={value}>{value}</option>)}</select></label><label>Local notes<textarea maxLength={4000} value={notes} onChange={(event) => setNotes(event.target.value)} /></label><button className="button primary" disabled={saving} onClick={() => onSave({ notes, priority })}>{saving ? 'Saving…' : 'Save local details'}</button>
    <h3>Explicit closing relationships</h3>{error && <p role="alert">{error}</p>}{links ? <>{links.partial && <p>Linked work is partial. {links.message}</p>}{!links.items.length && <p>No explicit closing relationships found.</p>}{links.items.map((link) => <button className="kanban-link" key={`${link.repository}:${link.kind}:${link.number}`} disabled={!isGitHubWorkUrl(link.url)} title={!isGitHubWorkUrl(link.url) ? 'Invalid GitHub work item URL' : undefined} onClick={() => void openExternalUrl(link.url).catch((reason) => setError(String(reason)))}>{link.repository} #{link.number} · {link.title} ↗</button>)}</> : <p>Loading linked work…</p>}
  </section></div>
}
