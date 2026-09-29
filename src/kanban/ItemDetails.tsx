import { Check, CircleDot, ExternalLink, GitPullRequest, MessageSquare, X } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { getKanbanDiscussion, getKanbanLinks, openExternalUrl } from '../api'
import type { KanbanDiscussion, KanbanItem, KanbanLinks, KanbanMetadata } from '../types'
import { relativeTime } from '../utils'
import { columnFor, isGitHubWorkUrl, sourceStateLabel } from './model'
import ActivityTimeline from './ActivityTimeline'

const PRIORITIES = ['None', 'Low', 'Medium', 'High'] as const

function readableCheckStatus(value: string): string {
  return value.replaceAll('_', ' ').toLowerCase().replace(/^\w/, (letter) => letter.toUpperCase())
}

export default function ItemDetails({ item, saving, syncBusy, revision, onClose, onSave }: {
  item: KanbanItem
  saving: boolean
  syncBusy: boolean
  revision: number
  onClose: () => void
  onSave: (changes: Partial<KanbanMetadata>) => Promise<boolean>
}) {
  const titleId = useId()
  const titleRef = useRef<HTMLHeadingElement>(null)
  const dialog = useRef<HTMLElement>(null)
  const discussionRef = useRef<KanbanDiscussion | null>(null)
  const activationClickRef = useRef(false)
  const [notes, setNotes] = useState(item.notes)
  const [priority, setPriority] = useState(item.priority)
  const [links, setLinks] = useState<KanbanLinks | null>(null)
  const [discussion, setDiscussion] = useState<KanbanDiscussion | null>(null)
  const [loadingMore, setLoadingMore] = useState<'comments' | 'checks' | 'commits' | null>(null)
  const [refreshingDiscussion, setRefreshingDiscussion] = useState(false)
  const [sourceError, setSourceError] = useState('')
  const [linksError, setLinksError] = useState('')
  const [saveError, setSaveError] = useState('')
  const [saved, setSaved] = useState(false)
  const [confirmClose, setConfirmClose] = useState(false)
  const dirty = notes !== item.notes || priority !== item.priority
  const githubUrlValid = isGitHubWorkUrl(item.url)

  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    titleRef.current?.focus()
    return () => previous?.focus()
  }, [])
  useEffect(() => {
    let focusTimer: number | undefined
    const onBlur = () => { if (focusTimer !== undefined) window.clearTimeout(focusTimer); activationClickRef.current = true }
    const onFocus = () => { if (focusTimer !== undefined) window.clearTimeout(focusTimer); focusTimer = window.setTimeout(() => { activationClickRef.current = false }, 250) }
    window.addEventListener('blur', onBlur)
    window.addEventListener('focus', onFocus)
    return () => { if (focusTimer !== undefined) window.clearTimeout(focusTimer); window.removeEventListener('blur', onBlur); window.removeEventListener('focus', onFocus) }
  }, [])
  useEffect(() => {
    if (syncBusy && discussionRef.current && !discussionRef.current.partial) return
    let alive = true
    const load = async () => {
      // Both reads use the same GitHub job lock. Load discussion first so an
      // uncached ticket does not lose its checks to a simultaneous links read.
      try {
        const result = await getKanbanDiscussion(item.item_key)
        if (alive) { discussionRef.current = result; setDiscussion(result); setSourceError('') }
      } catch (reason) { if (alive) setSourceError(String(reason)) }
      if (!alive) return
      try { const result = await getKanbanLinks(item.item_key); if (alive) { setLinks(result); setLinksError('') } }
      catch (reason) { if (alive) setLinksError(String(reason)) }
    }
    void load()
    return () => { alive = false }
  }, [item.item_key, revision, syncBusy])

  const requestClose = () => {
    if (saving) return
    if (dirty) { setConfirmClose(true); return }
    onClose()
  }
  const openUrl = (url: string) => void openExternalUrl(url).catch((reason) => setSourceError(String(reason)))
  const persist = async (changes: Partial<KanbanMetadata>) => {
    setSaveError('')
    const ok = await onSave(changes)
    if (ok) { setSaved(true); setConfirmClose(false) }
    else { setSaved(false); setSaveError('Could not save these local changes. Your draft is still here; retry after checking the error above.') }
  }
  const loadMore = async (section: 'comments' | 'checks' | 'commits') => {
    setLoadingMore(section)
    setSourceError('')
    try { const result = await getKanbanDiscussion(item.item_key, section); discussionRef.current = result; setDiscussion(result) }
    catch (reason) { setSourceError(String(reason)) }
    finally { setLoadingMore(null) }
  }
  const refreshDiscussion = async () => {
    setRefreshingDiscussion(true)
    setSourceError('')
    try { const result = await getKanbanDiscussion(item.item_key, undefined, true); discussionRef.current = result; setDiscussion(result) }
    catch (reason) { setSourceError(String(reason)) }
    finally { setRefreshingDiscussion(false) }
  }

  return <div className="kanban-detail-backdrop" onClick={(event) => { if (event.target === event.currentTarget && !activationClickRef.current) requestClose() }}><section ref={dialog} className="kanban-detail" role="dialog" aria-modal="true" aria-labelledby={titleId} onKeyDown={(event) => {
    if (event.key === 'Escape') { event.preventDefault(); requestClose(); return }
    if (event.key !== 'Tab') return
    const targets = dialog.current?.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled), select:not(:disabled), textarea:not(:disabled), summary')
    if (!targets?.length) return
    const first = targets[0], last = targets[targets.length - 1]
    if (event.shiftKey && (document.activeElement === first || document.activeElement === titleRef.current)) { event.preventDefault(); last.focus() }
    else if (!event.shiftKey && document.activeElement === last) { event.preventDefault(); first.focus() }
  }}>
    <header className="kanban-detail-header">
      <div className="kanban-detail-overline">{item.kind === 'pr' ? <GitPullRequest size={15} aria-hidden="true" /> : <CircleDot size={15} aria-hidden="true" />}<span>{item.kind === 'pr' ? 'Pull request' : 'Issue'} · {item.repository} #{item.number}</span></div>
      <div className="kanban-detail-header-actions"><button className="button secondary compact" type="button" disabled={!githubUrlValid} title={!githubUrlValid ? 'Invalid GitHub work item URL' : undefined} onClick={() => openUrl(item.url)}>Open in GitHub <ExternalLink size={13} aria-hidden="true" /></button><button className="icon-button" type="button" aria-label="Close details" title="Close details" onClick={requestClose}><X size={18} /></button></div>
    </header>
    <div className="kanban-detail-body">
      <h2 id={titleId} ref={titleRef} tabIndex={-1}>{item.title}</h2>
      <div className="kanban-detail-source-actions"><span className="kanban-source-state">{sourceStateLabel(item)}</span></div>
      {item.unavailable && <p className="kanban-detail-alert" role="status">Repository unavailable. Showing the last cached GitHub state.</p>}

      <section className="kanban-detail-section" aria-label="GitHub details">
        <h3>GitHub details <span>Read only</span></h3>
        <dl className="kanban-detail-facts"><div><dt>Author</dt><dd>{item.author ? `@${item.author}` : 'Unknown'}</dd></div><div><dt>Assignees</dt><dd>{item.assignees.length ? item.assignees.map((name) => `@${name}`).join(', ') : 'None'}</dd></div><div><dt>Updated</dt><dd>{relativeTime(item.updated_at)}</dd></div>{item.kind === 'pr' && <div><dt>CI summary</dt><dd>{item.ci_state ? readableCheckStatus(item.ci_state) : 'Not reported'}</dd></div>}</dl>
      </section>

      <section className="kanban-detail-section kanban-local-section" aria-label="Your planning">
        <h3>Your planning <span>Saved on this Mac</span></h3>
        <div className="kanban-column-setting"><div><strong>Column</strong><p>{columnFor(item)} · {item.manual_column ? 'Local placement' : 'Automatic placement'}</p></div>{columnFor(item) !== 'Done' && <button className="button secondary compact" type="button" disabled={saving || item.manual_column === null} onClick={() => void persist({ manual_column: null, priority, notes })}>Reset to automatic</button>}</div>
        {columnFor(item) === 'Done' && <p className="kanban-local-explanation">Done follows GitHub’s closed or merged state. Local planning cannot reopen it.</p>}
        <fieldset className="kanban-priority"><legend>Local priority</legend><div className="kanban-priority-options">{PRIORITIES.map((value) => <label key={value} className={priority === value ? `selected priority-${value.toLowerCase()}` : ''}><input type="radio" name="kanban-priority" value={value} checked={priority === value} onChange={() => { setPriority(value); setSaved(false); setSaveError('') }} /><span>{value}</span></label>)}</div></fieldset>
        <label className="kanban-notes-label" htmlFor="kanban-local-notes">Local notes</label><textarea id="kanban-local-notes" maxLength={4000} value={notes} onChange={(event) => { setNotes(event.target.value); setSaved(false); setSaveError('') }} placeholder="Notes for your own planning. These are not posted to GitHub." /><div className="kanban-notes-hint">Only on this computer <span>{notes.length}/4000</span></div>
      </section>

      <ActivityTimeline item={item} discussion={discussion} error={sourceError} loadingMore={loadingMore} refreshing={refreshingDiscussion} onRefresh={() => void refreshDiscussion()} onLoadMore={(section) => void loadMore(section)} onOpenUrl={openUrl} />

      <details className="kanban-detail-section kanban-linked-work"><summary><span><MessageSquare size={15} aria-hidden="true" /> Confirmed closing links {links && `(${links.items.length})`}</span></summary>{links ? <>{links.partial && <p className="kanban-detail-alert">{links.message ?? 'Linked work is incomplete.'}</p>}{!links.items.length && <p>No explicit closing relationships found.</p>}{links.items.map((link) => <button className="kanban-link" key={`${link.repository}:${link.kind}:${link.number}`} disabled={!isGitHubWorkUrl(link.url)} title={!isGitHubWorkUrl(link.url) ? 'Invalid GitHub work item URL' : undefined} onClick={() => openUrl(link.url)}>{link.repository} #{link.number} · {link.title} · {link.state} ↗</button>)}</> : <p role={linksError ? 'alert' : undefined}>{linksError ? `Linked work is unavailable: ${linksError}` : 'Loading linked work…'}</p>}</details>
    </div>
    <footer className="kanban-detail-footer">
      {confirmClose && <div className="kanban-discard-prompt" role="alert"><span>Discard unsaved local changes?</span><button className="button secondary compact" type="button" onClick={() => setConfirmClose(false)}>Keep editing</button><button className="button secondary compact" type="button" onClick={onClose}>Discard changes</button></div>}
      {saveError && <p className="kanban-save-error" role="alert">{saveError}</p>}
      <div className="kanban-save-row"><span className="kanban-save-status" role="status">{saving ? 'Saving…' : dirty ? 'Unsaved local changes' : saved ? <><Check size={14} /> Saved locally</> : 'Local edits stay on this computer'}</span><button className="button primary" type="button" disabled={saving || !dirty} onClick={() => void persist({ priority, notes })}>{saving ? 'Saving…' : 'Save changes'}</button></div>
    </footer>
  </section></div>
}
