import { GitPullRequest, CircleDot } from 'lucide-react'
import { useCallback, useEffect, useRef, useState } from 'react'
import { getKanbanLinks, getKanbanPage, getKanbanPreferences, setKanbanPreferences, openExternalUrl, setKanbanMetadata, syncWorkItems } from '../api'
import ActivityRepositoryMenu, { activityScopeRepositories } from '../ActivityRepositoryMenu'
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
  const move = (item: KanbanItem, column: Column) => {
    if (column === 'Done' || columnFor(item) === 'Done') { setBlockedMove(item); return }
    void save(item, { manual_column: column, sort_rank: Math.min(1_000_000_000, Math.max(0, ...(page?.items ?? []).map((row) => row.sort_rank)) + 1) }).then((saved) => { if (saved) void load() })
  }
  const reorder = async (cards: KanbanItem[], index: number, direction: number) => {
    const target = index + direction
    if (target < 0 || target >= cards.length) return
    const ordered = [...cards]
    ;[ordered[index], ordered[target]] = [ordered[target], ordered[index]]
    setOrdering(true)
    try {
      // Integer ranks need spacing when imported items all start at zero.
      // Stop on a conflict so a newer local edit is never overwritten.
      for (let position = 0; position < ordered.length; position++) {
        const row = ordered[position], rank = -1_000_000_000 + position * 1024
        if (row.sort_rank !== rank && !await save(row, { sort_rank: rank })) break
      }
    } finally { setOrdering(false); void load() }
  }
  const refresh = async () => {
    setRefreshing(true); setError('')
    try { const result = await syncWorkItems(); await load(); if (!result.ok || result.errors.length) setError([result.message, ...result.errors].join(' ')) }
    catch (reason) { setError(String(reason)) }
    finally { setRefreshing(false) }
  }
  const item = page?.items.find((row) => row.item_key === selected)
  return <main className="kanban-main">
    <div className="page-heading"><div><p className="eyebrow">Local workflow</p><h1>Kanban</h1></div><button className="button primary" disabled={refreshing || loading || pending.size > 0} onClick={() => void refresh()}>{refreshing ? 'Refreshing…' : 'Refresh Tickets'}</button></div>
    <p className="small-note">Last successful activity refresh: {relativeTime(page?.last_successful_refresh)} · Refresh fetches PRs and issues from all tracked repositories. Local changes do not update GitHub.</p>
    <div className="kanban-filters"><label>Items <select aria-label="Board item type" value={kind} onChange={(event) => setKind(event.target.value as KanbanKind)}><option value="prs">Pull requests</option><option value="issues">Issues</option><option value="both">Both</option></select></label>
      <ActivityRepositoryMenu repositories={repositories} login={login} value={scope} disabled={false} onChange={setScope} />
      <label>My involvement <select aria-label="Board involvement" value={relationship} onChange={(event) => setRelationship(event.target.value as ActivityRelationship)}><option value="everyone">Everyone</option><option value="author">Authored by me</option><option value="assignee">Assigned to me</option><option value="author_or_assignee">Authored or assigned to me</option></select></label>
      <input aria-label="Search board" placeholder="Search work items" value={search} onChange={(event) => setSearch(event.target.value)} />
      <label><input type="checkbox" checked={completed} onChange={(event) => setCompleted(event.target.checked)} /> Show completed</label>
    </div>
    <div className="kanban-summary" aria-live="polite"><span>{page?.active_count ?? 0} cached active · {page?.completed_count ?? 0} cached completed</span>{loading && <span>Loading board…</span>}<button className="button secondary compact" disabled={loading || pending.size > 0} onClick={() => void load()}>Reload board</button></div>
    {blockedMove && <div role="alert" className="kanban-notice">Done reflects GitHub completion. Close or reopen this item on GitHub.<button className="button secondary compact" disabled={!isGitHubWorkUrl(blockedMove.url)} onClick={() => void openExternalUrl(blockedMove.url).catch((reason) => setError(String(reason)))}>Open in GitHub</button><button className="button secondary compact" onClick={() => setBlockedMove(null)}>Dismiss</button></div>}
    {error && <div role="alert" className="error-banner">{error}</div>}
    {page?.partial && <div role="status" className="kanban-notice">Activity data is partial. {page.errors.join(' ')}</div>}
    {!loading && page && !page.items.length && <p className="kanban-notice">No work items match these filters. Refresh activity to fetch the latest GitHub data.</p>}
    <div className="kanban-columns">{COLUMNS.filter((column) => completed || column !== 'Done').map((column) => {
      const cards = (page?.items ?? []).filter((row) => columnFor(row) === column).sort((a, b) => a.sort_rank - b.sort_rank || b.updated_at.localeCompare(a.updated_at) || a.item_key.localeCompare(b.item_key))
      return <section className="kanban-column" key={column} aria-label={`${column} column`} onDragOver={(event) => event.preventDefault()} onDrop={(event) => { event.preventDefault(); const dragged = page?.items.find((row) => row.item_key === event.dataTransfer.getData('text/plain')); if (dragged) move(dragged, column) }}>
        <h2>{column} <small>{cards.length}{page && page.items.length < page.total ? ' loaded' : ''}</small></h2>{cards.map((card, cardIndex) => <article className="kanban-card" key={card.item_key} draggable={column !== 'Done' && !ordering && !pending.has(card.item_key)} onDragStart={(event) => event.dataTransfer.setData('text/plain', card.item_key)}>
          <button className="kanban-card-title" onClick={() => setSelected(card.item_key)}>{card.title}</button><span className="small-note">{card.kind === 'pr' ? <GitPullRequest size={13} aria-label="Pull request" /> : <CircleDot size={13} aria-label="Issue" />}{card.kind === 'pr' ? 'PR' : 'Issue'} · {card.repository} #{card.number}</span><div className="kanban-badges">{card.unavailable && <span>Unavailable · cached state</span>}<span>{card.merged_at || card.state.toLowerCase() === 'merged' ? 'Merged' : columnFor(card) === 'Done' ? card.kind === 'pr' ? 'Closed without merge' : card.completion_reason ?? 'Closed' : card.state}{card.is_draft ? ' · Draft' : ''}</span>{card.priority !== 'None' && <span>{card.priority}</span>}{card.ci_state && <span>CI: {card.ci_state}</span>}{card.notes && <span>Local notes</span>}</div><span className="small-note">{card.author ? `@${card.author} · ` : ''}{relativeTime(card.updated_at)}</span>
          <label className="kanban-move">Move to <select aria-label={`Move ${card.title}`} value={column} disabled={column === 'Done' || ordering || pending.has(card.item_key)} onChange={(event) => move(card, event.target.value as Column)}>{COLUMNS.filter((value) => value !== 'Done' || column === 'Done').map((value) => <option key={value}>{value}</option>)}</select></label>
          {column !== 'Done' && <div className="kanban-order"><button aria-label={`Move ${card.title} up`} disabled={ordering || pending.size > 0 || cardIndex === 0} onClick={() => void reorder(cards, cardIndex, -1)}>↑ Move up</button><button aria-label={`Move ${card.title} down`} disabled={ordering || pending.size > 0 || cardIndex === cards.length - 1} onClick={() => void reorder(cards, cardIndex, 1)}>↓ Move down</button></div>}
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
