import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import KanbanBoard from '../kanban/KanbanBoard'
import { columnFor, isGitHubWorkUrl } from '../kanban/model'
import type { KanbanItem } from '../types'
const invoke = vi.hoisted(() => vi.fn())
vi.mock('@tauri-apps/api/core', () => ({ invoke }))
const item: KanbanItem = { item_key: 'pr:1:2', kind: 'pr', repository_id: 1, repository: 'sam/repo', number: 2, title: 'Review changes', state: 'open', is_draft: false, updated_at: '2026-09-27', url: 'https://github.com/sam/repo/pull/2', author: 'sam', assignees: [], ci_state: 'SUCCESS', closed_at: null, merged_at: null, completion_reason: null, manual_column: null, priority: 'None', notes: '', sort_rank: 0, revision: 0 }
const preferences = { kind: 'both', repository_id: null, repository_scope: 'all', relationship: 'author', search: '', show_completed: false }
const page = { items: [item], total: 1, active_count: 1, completed_count: 7, partial: false, errors: [] }
const props = { repositories: [{ id: 1, name: 'repo', owner: 'sam' }], login: 'sam', relationship: 'author' as const, onRelationship: vi.fn(), onBack: vi.fn(), revision: 0 }
beforeEach(() => { invoke.mockReset(); invoke.mockImplementation(async (command) => command === 'get_kanban_preferences' ? preferences : command === 'get_kanban_page' ? page : command === 'get_kanban_links' ? { items: [], partial: false } : undefined) })
describe('local Kanban workflow', () => {
  it('accepts only canonical HTTPS GitHub work item URLs', () => {
    expect(isGitHubWorkUrl(item.url)).toBe(true)
    for (const url of ['https://example.com/sam/repo/pull/2', 'javascript:alert(1)', 'https://github.com/settings', 'https://github.com.evil.test/sam/repo/issues/1', 'https://user:secret@github.com/sam/repo/pull/2']) expect(isGitHubWorkUrl(url)).toBe(false)
  })
  it('places terminal work in Done while preserving active overrides on reopen', () => {
    expect(columnFor(item)).toBe('Review')
    expect(columnFor({ ...item, is_draft: true })).toBe('In progress')
    expect(columnFor({ ...item, kind: 'issue' })).toBe('Todo')
    expect(columnFor({ ...item, manual_column: 'Blocked', state: 'closed' })).toBe('Done')
    expect(columnFor({ ...item, manual_column: 'Blocked' })).toBe('Blocked')
  })
  it('starts with both PRs and issues and hides completed cards, loads subsequent pages past 1000, and refreshes only activity', async () => {
    invoke.mockImplementation(async (command, args) => {
      if (command === 'get_kanban_preferences') return preferences
      if (command === 'sync_work_items') return { ok: true, errors: [], message: 'Updated' }
      if (command === 'get_kanban_page') return args.offset === 0 ? { ...page, items: Array.from({ length: 1000 }, (_, i) => ({ ...item, item_key: `pr:${i}`, title: `Work ${i}` })), total: 1001 } : { ...page, items: [{ ...item, title: 'Beyond initial thousand' }], total: 1001 }
    })
    render(<KanbanBoard {...props} />)
    await screen.findByText('Work 999')
    expect(invoke).toHaveBeenCalledWith('get_kanban_page', expect.objectContaining({ kind: 'both', show_completed: false, limit: 200 }))
    expect(screen.queryByRole('region', { name: 'Done column' })).not.toBeInTheDocument()
    expect(screen.getByText('1 cached active · 7 cached completed')).toBeInTheDocument()
    fireEvent.click(screen.getByText('Load more (1000 of 1001)'))
    await screen.findByText('Beyond initial thousand')
    expect(invoke).toHaveBeenCalledWith('get_kanban_page', expect.objectContaining({ offset: 1000 }))
    fireEvent.click(screen.getByText('Refresh Tickets'))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('sync_work_items', undefined))
    expect(invoke.mock.calls.some(([command]) => command === 'get_loc_history' || command === 'sync_github_data')).toBe(false)
  }, 15000)
  it('persists keyboard ordering and explains why Done cannot receive local moves', async () => {
    let cached = [item, { ...item, item_key: 'second', title: 'Second work', sort_rank: 1 }]
    invoke.mockImplementation(async (command, args) => {
      if (command === 'get_kanban_preferences') return preferences
      if (command === 'get_kanban_page') return { ...page, items: cached, total: 2 }
      if (command === 'set_kanban_metadata') { const metadata = { ...args, revision: args.expected_revision + 1 }; cached = cached.map((row) => row.item_key === args.item_key ? { ...row, ...metadata } : row); return metadata }
    })
    render(<KanbanBoard {...props} />)
    await screen.findByText('Second work')
    fireEvent.click(screen.getByRole('button', { name: 'Move Second work up' }))
    await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === 'set_kanban_metadata')).toHaveLength(2))
    const review = screen.getByRole('region', { name: 'Review column' })
    expect(within(review).getAllByRole('article')[0]).toHaveTextContent('Second work')
    fireEvent.click(screen.getByLabelText('Show completed'))
    const done = await screen.findByRole('region', { name: 'Done column' })
    fireEvent.drop(done, { dataTransfer: { getData: () => item.item_key } })
    expect(screen.getByRole('alert')).toHaveTextContent('Done reflects GitHub completion')
    expect(screen.getByRole('button', { name: 'Open in GitHub' })).toBeInTheDocument()
  })

  it('restarts pagination after a move changes the cached order', async () => {
    let records = Array.from({ length: 201 }, (_, index) => ({ ...item, item_key: `pr:${index}`, title: `Work ${index}`, sort_rank: 0 }))
    invoke.mockImplementation(async (command, args) => {
      if (command === 'get_kanban_preferences') return preferences
      if (command === 'get_kanban_page') return { ...page, items: [...records].sort((a, b) => a.sort_rank - b.sort_rank || a.item_key.localeCompare(b.item_key)).slice(args.offset, args.offset + args.limit), total: records.length, active_count: records.length }
      if (command === 'set_kanban_metadata') {
        records = records.map((row) => row.item_key === args.item_key ? { ...row, sort_rank: args.sort_rank, manual_column: args.manual_column } : row)
        return { ...args, revision: 1 }
      }
    })
    render(<KanbanBoard {...props} />)
    await screen.findByText('Work 0')
    fireEvent.change(screen.getByLabelText('Move Work 0'), { target: { value: 'Blocked' } })
    await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === 'get_kanban_page').length).toBeGreaterThan(1))
    await waitFor(() => expect(screen.queryByText('Work 0')).not.toBeInTheDocument())
    fireEvent.click(await screen.findByText('Load more (200 of 201)'))
    await screen.findByText('Work 0')
    expect(screen.queryByText('Load more (200 of 201)')).not.toBeInTheDocument()
    expect(document.querySelectorAll('.kanban-card')).toHaveLength(201)
  })

  it('resets a local column override to automatic and persists it across reload', async () => {
    let cached = { ...item, manual_column: 'Blocked' as string | null }
    invoke.mockImplementation(async (command, args) => {
      if (command === 'get_kanban_preferences') return preferences
      if (command === 'get_kanban_page') return { ...page, items: [cached] }
      if (command === 'get_kanban_links') return { items: [], partial: false }
      if (command === 'set_kanban_metadata') { cached = { ...cached, ...args, revision: args.expected_revision + 1 }; return cached }
    })
    render(<KanbanBoard {...props} />)
    await screen.findByText(item.title)
    fireEvent.click(screen.getByRole('button', { name: item.title }))
    fireEvent.click(screen.getByRole('button', { name: 'Reset to automatic' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('set_kanban_metadata', expect.objectContaining({ manual_column: null, expected_revision: 0 })))
    expect(within(screen.getByRole('region', { name: 'Review column' })).getByText(item.title)).toBeInTheDocument()
    await waitFor(() => expect(screen.getByRole('button', { name: 'Reset to automatic' })).toBeDisabled())
    fireEvent.click(screen.getByRole('button', { name: 'Close details' }))
    fireEvent.click(screen.getByRole('button', { name: 'Reload board' }))
    await waitFor(() => expect(within(screen.getByRole('region', { name: 'Review column' })).getByText(item.title)).toBeInTheDocument())
    expect(cached.manual_column).toBeNull()
  })

  it('moves optimistically and rolls back rejected stale saves; closed work cannot move', async () => {
    let rejectSave!: (reason: Error) => void
    invoke.mockImplementation((command) => command === 'get_kanban_preferences' ? Promise.resolve(preferences) : command === 'get_kanban_page' ? Promise.resolve({ ...page, items: [item, { ...item, item_key: 'closed', title: 'Closed work', state: 'closed' }], total: 2 }) : new Promise((_resolve, reject) => { rejectSave = reject }))
    render(<KanbanBoard {...props} />)
    await screen.findByText(item.title)
    fireEvent.change(screen.getByLabelText(`Move ${item.title}`), { target: { value: 'Blocked' } })
    expect(within(screen.getByRole('region', { name: 'Blocked column' })).getByText(item.title)).toBeInTheDocument()
    expect(invoke).toHaveBeenCalledWith('set_kanban_metadata', expect.objectContaining({ manual_column: 'Blocked', expected_revision: 0 }))
    rejectSave(new Error('Revision conflict'))
    await screen.findByRole('alert')
    expect(within(screen.getByRole('region', { name: 'Review column' })).getByText(item.title)).toBeInTheDocument()
    fireEvent.click(screen.getByLabelText('Show completed'))
    await screen.findByText('Closed work')
    expect(screen.getByLabelText('Move Closed work')).toBeDisabled()
  })
})
