import { useEffect, useLayoutEffect, useRef, useState, type CSSProperties } from 'react'
import { Check, ChevronDown, ListFilter } from 'lucide-react'
import type { Repository } from './types'
import { repoName, repositoryId, repositoryLabel } from './utils'

const ownerOf = (repo: Repository) => repo.owner ?? repositoryLabel(repo).split('/')[0]

export function activityScopeRepositories(scope: string, repositories: Repository[], login: string): Repository[] {
  if (scope === 'all') return repositories
  if (scope === 'personal') return repositories.filter((repo) => ownerOf(repo).toLowerCase() === login.toLowerCase())
  if (scope === 'companies') return repositories.filter((repo) => ownerOf(repo).toLowerCase() !== login.toLowerCase())
  if (scope.startsWith('owner:')) return repositories.filter((repo) => ownerOf(repo).toLowerCase() === scope.slice(6))
  return repositories.filter((repo) => String(repositoryId(repo)) === scope)
}

export default function ActivityRepositoryMenu({ repositories, login, value, disabled, onChange }: { repositories: Repository[]; login: string; value: string; disabled: boolean; onChange: (value: string) => void }) {
  const [open, setOpen] = useState(false)
  const [expanded, setExpanded] = useState<Record<string, boolean>>({})
  const container = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const panel = useRef<HTMLDivElement>(null)
  const [style, setStyle] = useState<CSSProperties>({ position: 'fixed' })
  const owners = [...new Set(repositories.map(ownerOf).filter((owner) => owner.toLowerCase() !== login.toLowerCase()))].sort((a, b) => a.localeCompare(b))
  const groups = [
    { key: 'personal', label: 'Personal repositories' },
    ...owners.map((owner) => ({ key: `owner:${owner.toLowerCase()}`, label: owner }))
  ].map((group) => ({ ...group, repos: activityScopeRepositories(group.key, repositories, login) })).filter((group) => group.repos.length)
  const selectedLabel = value === 'all' ? 'All repositories' : value === 'personal' ? 'Personal repositories' : value === 'companies' ? 'All company repositories' : groups.find((group) => group.key === value)?.label ?? repoName(repositories.find((repo) => String(repositoryId(repo)) === value) ?? { id: value, name: 'Repository' })
  useLayoutEffect(() => {
    if (!open || !trigger.current) return
    const anchor = trigger.current.getBoundingClientRect()
    const width = Math.min(320, window.innerWidth - 24)
    const below = window.innerHeight - anchor.bottom - 20
    const above = anchor.top - 20
    const upward = below < 360 && above > below
    setStyle({ position: 'fixed', width, left: Math.max(12, Math.min(anchor.right - width, window.innerWidth - width - 12)), top: upward ? 'auto' : anchor.bottom + 8, bottom: upward ? window.innerHeight - anchor.top + 8 : 'auto', maxHeight: Math.min(420, Math.max(0, upward ? above : below)) })
    panel.current?.focus({ preventScroll: true })
  }, [open])
  useEffect(() => {
    if (!open) return
    const outside = (event: PointerEvent) => { if (event.target instanceof Node && !container.current?.contains(event.target)) setOpen(false) }
    const escape = (event: KeyboardEvent) => { if (event.key === 'Escape') { event.preventDefault(); setOpen(false); trigger.current?.focus() } }
    const resize = () => setOpen(false)
    const scroll = (event: Event) => { if (!(event.target instanceof Node && panel.current?.contains(event.target))) setOpen(false) }
    document.addEventListener('pointerdown', outside)
    document.addEventListener('keydown', escape)
    document.addEventListener('scroll', scroll, true)
    window.addEventListener('resize', resize)
    return () => { document.removeEventListener('pointerdown', outside); document.removeEventListener('keydown', escape); document.removeEventListener('scroll', scroll, true); window.removeEventListener('resize', resize) }
  }, [open])
  const choose = (scope: string) => { onChange(scope); setOpen(false); trigger.current?.focus() }
  const option = (scope: string, label: string) => <button type="button" className={`activity-scope-option${value === scope ? ' selected' : ''}`} aria-pressed={value === scope} onClick={() => choose(scope)}><span>{label}</span>{value === scope && <Check size={14} />}</button>
  return <div className="activity-repository-menu" ref={container} onBlur={(event) => { if (event.relatedTarget instanceof Node && !event.currentTarget.contains(event.relatedTarget)) setOpen(false) }}>
    <button ref={trigger} className="activity-repository-trigger" type="button" aria-label={`Filter by repository: ${selectedLabel}`} aria-haspopup="dialog" aria-expanded={open} disabled={disabled} onClick={() => { setExpanded({}); setOpen(!open) }}><ListFilter size={14} /><span>{selectedLabel}</span><ChevronDown size={14} /></button>
    {open && <div ref={panel} style={style} className="repo-picker tracked-repository-groups activity-scope-picker" tabIndex={-1} role="dialog" aria-label="Filter activity by repository">
      <div className="repo-picker-heading"><strong>Show activity from</strong></div>
      {option('all', 'All repositories')}
      {owners.length > 0 && option('companies', 'All company repositories')}
      {groups.map((group) => <section className={expanded[group.key] ? 'tracked-repository-group open' : 'tracked-repository-group'} key={group.key}>
        <div className="activity-scope-group"><button type="button" aria-expanded={Boolean(expanded[group.key])} aria-label={`${expanded[group.key] ? 'Collapse' : 'Expand'} ${group.label}`} onClick={() => setExpanded((current) => ({ ...current, [group.key]: !current[group.key] }))}><ChevronDown size={13} /><span>{group.label}</span><small>{group.repos.length}</small></button></div>
        {expanded[group.key] && <div className="activity-scope-repos">{option(group.key, group.key === 'personal' ? 'All personal repositories' : `All ${group.label} repositories`)}{[...group.repos].sort((a, b) => repoName(a).localeCompare(repoName(b))).map((repo) => <div key={String(repositoryId(repo))}>{option(String(repositoryId(repo)), repoName(repo))}</div>)}</div>}
      </section>)}
    </div>}
  </div>
}
