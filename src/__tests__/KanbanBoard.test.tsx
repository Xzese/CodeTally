import { createEvent, fireEvent, render, screen, waitFor, within } from '@testing-library/react'
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
  it('filters ticket types independently and keeps the completed control as a button', async () => {
    invoke.mockImplementation(async (command, args) => command === 'get_kanban_preferences' ? preferences : command === 'get_kanban_page' ? { ...page, items: args.kind === 'none' || args.kind === 'issues' ? [] : [item], total: args.kind === 'none' || args.kind === 'issues' ? 0 : 1 } : undefined)
    render(<KanbanBoard {...props} />)
    await screen.findByText(item.title)
    const prs = screen.getByRole('button', { name: 'PRs' }), issues = screen.getByRole('button', { name: 'Issues' })
    expect(prs).toHaveAttribute('aria-pressed', 'true')
    expect(issues).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(prs)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('get_kanban_page', expect.objectContaining({ kind: 'issues' })))
    await waitFor(() => expect(prs).toHaveAttribute('aria-pressed', 'false'))
    fireEvent.click(issues)
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('get_kanban_page', expect.objectContaining({ kind: 'none' })))
    expect(prs).toHaveAttribute('aria-pressed', 'false')
    expect(issues).toHaveAttribute('aria-pressed', 'false')
    fireEvent.click(prs)
    await waitFor(() => expect(prs).toHaveAttribute('aria-pressed', 'true'))
    fireEvent.click(issues)
    await waitFor(() => expect(issues).toHaveAttribute('aria-pressed', 'true'))
    expect(prs).toHaveAttribute('aria-pressed', 'true')
    expect(issues).toHaveAttribute('aria-pressed', 'true')
    fireEvent.click(screen.getByRole('button', { name: 'Show completed' }))
    expect(screen.getByRole('button', { name: 'Show completed' })).toHaveAttribute('aria-pressed', 'true')
    await screen.findByRole('region', { name: 'Done column' })
  })

  it('shows a dashed insertion position and persists a drag within a column', async () => {
    let cached = [item, { ...item, item_key: 'second', title: 'Second work', sort_rank: 1 }]
    invoke.mockImplementation(async (command, args) => {
      if (command === 'get_kanban_preferences') return preferences
      if (command === 'get_kanban_page') return { ...page, items: cached, total: 2 }
      if (command === 'set_kanban_metadata') { const metadata = { ...args, revision: args.expected_revision + 1 }; cached = cached.map((row) => row.item_key === args.item_key ? { ...row, ...metadata } : row); return metadata }
    })
    render(<KanbanBoard {...props} />)
    await screen.findByText('Second work')
    const review = screen.getByRole('region', { name: 'Review column' })
    const transfer = { setData: vi.fn(), getData: () => 'second', effectAllowed: '', dropEffect: '' }
    Object.defineProperty(within(review).getAllByRole('article')[0], 'getBoundingClientRect', { value: () => ({ top: 0, height: 100 }) })
    fireEvent.dragStart(within(review).getAllByRole('article')[1], { dataTransfer: transfer })
    const unchanged = createEvent.dragOver(review, { dataTransfer: transfer })
    Object.defineProperty(unchanged, 'clientY', { value: 1000 })
    fireEvent(review, unchanged)
    expect(transfer.dropEffect).toBe('none')
    expect(within(review).queryByRole('status', { name: 'Drop in Review' })).not.toBeInTheDocument()
    const over = createEvent.dragOver(review, { dataTransfer: transfer })
    Object.defineProperty(over, 'clientY', { value: 10 })
    fireEvent(review, over)
    expect(within(review).getByRole('status', { name: 'Drop in Review' })).toHaveClass('kanban-drop-placeholder')
    expect(review.querySelectorAll('.kanban-card:not(.kanban-card-dragging)')).toHaveLength(1)
    expect(transfer.effectAllowed).toBe('move')
    expect(transfer.dropEffect).toBe('move')
    fireEvent.drop(review, { dataTransfer: transfer })
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('set_kanban_metadata', expect.objectContaining({ item_key: 'second', manual_column: 'Review', sort_rank: -1024 })))
    await waitFor(() => expect(within(review).getAllByRole('article')[0]).toHaveTextContent('Second work'))
    fireEvent.keyDown(within(review).getAllByRole('article')[0], { key: 'ArrowDown', altKey: true })
    await waitFor(() => expect(within(review).getAllByRole('article')[1]).toHaveTextContent('Second work'))
    expect(screen.queryByText('Move Second work up')).not.toBeInTheDocument()
  })

  it('moves between columns and explains why Done cannot receive local moves', async () => {
    let cached = [item]
    invoke.mockImplementation(async (command, args) => {
      if (command === 'get_kanban_preferences') return preferences
      if (command === 'get_kanban_page') return { ...page, items: cached }
      if (command === 'set_kanban_metadata') { const metadata = { ...args, revision: args.expected_revision + 1 }; cached = cached.map((row) => ({ ...row, ...metadata })); return metadata }
    })
    render(<KanbanBoard {...props} />)
    await screen.findByText(item.title)
    const transfer = { setData: vi.fn(), getData: () => item.item_key, effectAllowed: '', dropEffect: '' }
    fireEvent.dragStart(within(screen.getByRole('region', { name: 'Review column' })).getByRole('article'), { dataTransfer: transfer })
    const blocked = screen.getByRole('region', { name: 'Blocked column' })
    fireEvent.dragOver(blocked, { dataTransfer: transfer, clientY: 100 })
    expect(within(blocked).getByRole('status', { name: 'Drop in Blocked' })).toBeInTheDocument()
    fireEvent.drop(blocked, { dataTransfer: transfer })
    await waitFor(() => expect(within(blocked).getByText(item.title)).toBeInTheDocument())
    fireEvent.click(screen.getByRole('button', { name: 'Show completed' }))
    const done = await screen.findByRole('region', { name: 'Done column' })
    fireEvent.dragStart(within(blocked).getByRole('article'), { dataTransfer: transfer })
    fireEvent.dragOver(done, { dataTransfer: transfer })
    expect(transfer.dropEffect).toBe('none')
    expect(within(done).queryByRole('status', { name: 'Drop in Done' })).not.toBeInTheDocument()
    fireEvent.drop(done, { dataTransfer: transfer })
    expect(screen.getByRole('alert')).toHaveTextContent('Done reflects GitHub completion')
    expect(screen.getByRole('button', { name: 'Open in GitHub' })).toBeInTheDocument()
  })

  it('resets a local column override and Refresh Tickets reloads the board', async () => {
    let cached = { ...item, manual_column: 'Blocked' as string | null }
    invoke.mockImplementation(async (command, args) => {
      if (command === 'get_kanban_preferences') return preferences
      if (command === 'get_kanban_page') return { ...page, items: [cached] }
      if (command === 'get_kanban_links') return { items: [], partial: false }
      if (command === 'sync_work_items') return { ok: true, errors: [], message: 'Updated' }
      if (command === 'set_kanban_metadata') { cached = { ...cached, ...args, revision: args.expected_revision + 1 }; return cached }
    })
    render(<KanbanBoard {...props} />)
    await screen.findByText(item.title)
    fireEvent.click(screen.getByRole('button', { name: item.title }))
    fireEvent.click(screen.getByRole('button', { name: 'Reset to automatic' }))
    await waitFor(() => expect(invoke).toHaveBeenCalledWith('set_kanban_metadata', expect.objectContaining({ manual_column: null, expected_revision: 0 })))
    expect(within(screen.getByRole('region', { name: 'Review column' })).getByText(item.title)).toBeInTheDocument()
    fireEvent.click(screen.getByRole('button', { name: 'Close details' }))
    const reads = invoke.mock.calls.filter(([command]) => command === 'get_kanban_page').length
    fireEvent.click(screen.getByRole('button', { name: 'Refresh Tickets' }))
    await waitFor(() => expect(invoke.mock.calls.filter(([command]) => command === 'get_kanban_page').length).toBeGreaterThan(reads))
    expect(cached.manual_column).toBeNull()
    expect(screen.queryByRole('button', { name: 'Reload board' })).not.toBeInTheDocument()
  })

  it('rolls back a rejected drag save', async () => {
    let rejectSave!: (reason: Error) => void
    invoke.mockImplementation((command) => command === 'get_kanban_preferences' ? Promise.resolve(preferences) : command === 'get_kanban_page' ? Promise.resolve(page) : new Promise((_resolve, reject) => { rejectSave = reject }))
    render(<KanbanBoard {...props} />)
    await screen.findByText(item.title)
    const review = screen.getByRole('region', { name: 'Review column' }), blocked = screen.getByRole('region', { name: 'Blocked column' })
    const transfer = { setData: vi.fn(), getData: () => item.item_key, effectAllowed: '', dropEffect: '' }
    fireEvent.dragStart(within(review).getByRole('article'), { dataTransfer: transfer })
    fireEvent.dragOver(blocked, { dataTransfer: transfer, clientY: 100 })
    fireEvent.drop(blocked, { dataTransfer: transfer })
    expect(within(blocked).getByText(item.title)).toBeInTheDocument()
    rejectSave(new Error('Revision conflict'))
    await screen.findByRole('alert')
    await waitFor(() => expect(within(review).getByText(item.title)).toBeInTheDocument())
  })
})
