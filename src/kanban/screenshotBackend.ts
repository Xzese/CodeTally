// Browser-only screenshot fixture. Native screenshot builds use the isolated
// SQLite fixture through real Tauri commands instead.
import type { AppSettings, KanbanItem, KanbanPreferences, SyncProgress } from '../types'
import { DEFAULT_APP_SETTINGS } from '../settings'

const now = new Date('2026-09-27T09:30:00Z').toISOString()
const repositories = [
  { id: 1, github_id: 'repo-story-planner', owner: 'storyforge', name: 'planner', name_with_owner: 'storyforge/planner', url: 'https://github.com/storyforge/planner', primary_language: 'TypeScript', is_archived: false, is_fork: false, loc_available: true, total_loc: 18420, source_loc: 14800, test_loc: 3620, open_prs: 1, open_issues: 1 },
  { id: 2, github_id: 'repo-story-mobile', owner: 'storyforge', name: 'mobile', name_with_owner: 'storyforge/mobile', url: 'https://github.com/storyforge/mobile', primary_language: 'TypeScript', is_archived: false, is_fork: false, loc_available: true, total_loc: 24110, source_loc: 19740, test_loc: 4370, open_prs: 1, open_issues: 0 },
  { id: 3, github_id: 'repo-orbit-api', owner: 'orbit-labs', name: 'api', name_with_owner: 'orbit-labs/api', url: 'https://github.com/orbit-labs/api', primary_language: 'Rust', is_archived: false, is_fork: false, loc_available: true, total_loc: 11970, source_loc: 9150, test_loc: 2820, open_prs: 1, open_issues: 1 }
]
const item = (kind: 'pr' | 'issue', repository_id: number, number: number, title: string, state: string, extras: Partial<KanbanItem> = {}): KanbanItem => {
  const repo = repositories[repository_id - 1]
  return { item_key: `github.com:node:${kind.toUpperCase()}_${repository_id}_${number}`, kind, repository_id, repository: repo.name_with_owner, number, title, state, is_draft: false, updated_at: now, url: `${repo.url}/${kind === 'pr' ? 'pull' : 'issues'}/${number}`, author: 'demo', assignees: ['alex'], ci_state: kind === 'pr' ? 'success' : null, closed_at: null, merged_at: null, completion_reason: null, manual_column: null, priority: 'None', notes: '', sort_rank: 0, revision: 0, ...extras }
}
const items: KanbanItem[] = [
  item('pr', 1, 42, 'Add accessible planning timeline', 'OPEN', { manual_column: 'In progress', priority: 'High', notes: 'Check the keyboard journey before release.', sort_rank: -2048 }),
  item('pr', 2, 87, 'Polish offline sign-in flow', 'OPEN', { is_draft: true, manual_column: 'Blocked', priority: 'Medium', notes: 'Waiting for the copy review.', ci_state: 'pending', sort_rank: -1024 }),
  item('pr', 3, 16, 'Reduce API response latency', 'OPEN', { manual_column: 'Review', priority: 'High', notes: 'Review retry behavior on a slow connection.', ci_state: 'failure' }),
  item('issue', 1, 105, 'Clarify weekly planning view', 'OPEN', { manual_column: 'Todo', priority: 'Low', notes: 'Draft clear acceptance criteria.', author: 'alex', assignees: ['demo'] }),
  item('issue', 3, 204, 'Document API timeout behavior', 'OPEN', { author: 'alex', assignees: ['demo'] }),
  item('pr', 1, 31, 'Ship keyboard navigation', 'MERGED', { closed_at: now, merged_at: now }),
  item('pr', 2, 72, 'Retire legacy notification', 'CLOSED', { closed_at: now, ci_state: 'failure' }),
  item('issue', 1, 96, 'Improve screen reader labels', 'CLOSED', { closed_at: now, completion_reason: 'COMPLETED', author: 'alex', assignees: ['demo'] })
]
let settings: AppSettings = { ...DEFAULT_APP_SETTINGS, kanban_enabled: true, run_in_background: false }
let preferences: KanbanPreferences = { kind: 'both', repository_scope: 'all', relationship: 'everyone', search: '', show_completed: false }
let progress: SyncProgress = { running: false, phase: 'idle', current: 0, total: 0, message: 'Ready' }

export async function screenshotCall(command: string, args: Record<string, unknown> = {}): Promise<unknown> {
  switch (command) {
    case 'check_dependencies': return { gh: true, git: true, tokei: true, gh_authenticated: true, login: 'demo' }
    case 'get_github_user': return { login: 'demo' }
    case 'get_app_settings': return settings
    case 'set_app_settings': settings = args.settings as AppSettings; return settings
    case 'get_dashboard': return { user: { login: 'demo' }, repositories, totals: { repositories: 3, total_loc: 54500, source_loc: 43690, test_loc: 10810, loc_change_30d: 720, open_prs: 3, open_issues: 2 }, history: [], last_sync_at: now, last_lines_refresh_at: now, last_full_refresh_at: now, last_activity_refresh_at: now, last_personal_refresh_at: now, errors: [] }
    case 'get_activity_refresh_at': return now
    case 'get_loc_history': return []
    case 'get_repository_selection': return repositories.map((repo) => ({ github_id: repo.github_id, name_with_owner: repo.name_with_owner, owner: repo.owner, group: 'company' }))
    case 'get_activity_feed': {
      const kind = args.kind === 'issues' ? 'issue' : 'pr'
      const scope = Array.isArray(args.repository_ids) ? args.repository_ids as number[] : repositories.map((repo) => repo.id)
      const state = String(args.state ?? 'all').toUpperCase()
      return { kind: args.kind, items: items.filter((row) => row.kind === kind && scope.includes(row.repository_id) && (!args.repository_id || row.repository_id === args.repository_id) && (state === 'ALL' || (state === 'OPEN' ? row.state === 'OPEN' : row.state === state))).map((row) => ({ ...row, kind: row.kind === 'pr' ? 'pull_request' : 'issue' })) }
    }
    case 'get_kanban_preferences': return preferences
    case 'set_kanban_preferences': preferences = args.preferences as KanbanPreferences; return preferences
    case 'get_kanban_page': {
      const ids = Array.isArray(args.repository_ids) ? args.repository_ids as number[] : repositories.map((repo) => repo.id)
      const relationship = String(args.relationship ?? 'everyone')
      const login = 'demo'
      const search = String(args.search ?? '').toLowerCase()
      const filtered = items.filter((row) => ids.includes(row.repository_id) && (args.kind === 'both' || (args.kind !== 'none' && row.kind === (args.kind === 'issues' ? 'issue' : 'pr'))) && (!search || `${row.title} ${row.repository} ${row.number}`.toLowerCase().includes(search)) && (relationship === 'everyone' || (relationship === 'author' ? row.author === login : relationship === 'assignee' ? row.assignees.includes(login) : row.author === login || row.assignees.includes(login))))
      const active = filtered.filter((row) => row.state === 'OPEN').length
      const completed = filtered.length - active
      const visible = filtered.filter((row) => args.show_completed || row.state === 'OPEN')
      const offset = Number(args.offset ?? 0), limit = Number(args.limit ?? 200)
      return { items: visible.slice(offset, offset + limit), total: visible.length, active_count: active, completed_count: completed, last_successful_refresh: '2026-09-27T12:00:00Z', partial: false, errors: [] }
    }
    case 'set_kanban_metadata': {
      const row = items.find((candidate) => candidate.item_key === args.item_key)
      if (!row) throw new Error('Item not found')
      if (row.revision !== args.expected_revision) throw new Error('Stale board edit')
      Object.assign(row, { manual_column: args.manual_column, priority: args.priority, notes: args.notes, sort_rank: args.sort_rank, revision: row.revision + 1 })
      return { item_key: row.item_key, manual_column: row.manual_column, priority: row.priority, notes: row.notes, sort_rank: row.sort_rank, revision: row.revision }
    }
    case 'get_kanban_links': return { items: args.item_key === 'github.com:node:PR_1_42' ? [{ kind: 'issue', repository: 'storyforge/planner', number: 105, title: 'Clarify weekly planning view', state: 'OPEN', url: 'https://github.com/storyforge/planner/issues/105' }] : [], partial: false, message: null }
    case 'get_kanban_discussion': {
      const key = String(args.item_key)
      const ticket = items.find((candidate) => candidate.item_key === key)
      return {
        comments: ticket ? [{ id: `comment-${key}`, author: 'alex', body_text: key === 'github.com:node:PR_1_42' ? 'The keyboard flow looks good. Preview URL: https://preview.storyforge.example/build-42.\nView deployment logs ↗' : 'The keyboard flow looks good. Please check the narrow window layout too.', body_markdown: key === 'github.com:node:PR_1_42' ? 'The keyboard flow looks good. Preview URL: https://preview.storyforge.example/build-42.\n[View deployment logs ↗](https://logs.storyforge.example/build-42)' : null, created_at: now, updated_at: now, url: `${ticket.url}#issuecomment-101` }] : [],
        comment_count: ticket ? 1 : 0,
        comments_has_more: false,
        checks: ticket?.kind === 'pr' ? ['Frontend checks', 'Desktop checks (Apple Silicon)', 'Desktop checks (Intel)'].map((name, index) => ({ id: `check-${key}-${index}`, name, status: ticket.ci_state === 'failure' && index === 1 ? 'FAILURE' : ticket.ci_state === 'pending' ? 'IN_PROGRESS' : 'SUCCESS', details_url: 'https://github.com/storyforge/planner/actions/runs/101' })) : [],
        check_count: ticket?.kind === 'pr' ? 3 : 0,
        checks_has_more: false,
        commits: ticket?.kind === 'pr' ? [{ oid: 'a1b2c3d4e5f6789012345678901234567890abcd', headline: 'Add keyboard navigation to the planning timeline', committed_at: '2026-09-26T08:10:00Z', author: 'demo', url: 'https://github.com/storyforge/planner/commit/a1b2c3d4e5f6789012345678901234567890abcd' }, { oid: 'f1e2d3c4b5a6987012345678901234567890abcd', headline: 'Improve focus order and small-window layout', committed_at: now, author: 'demo', url: 'https://github.com/storyforge/planner/commit/f1e2d3c4b5a6987012345678901234567890abcd' }] : [],
        commit_count: ticket?.kind === 'pr' ? 2 : 0,
        commits_has_more: false,
        refreshed_at: now,
        partial: false,
        message: null
      }
    }
    case 'get_sync_progress': return progress
    case 'get_personal_refresh_at': return now
    case 'sync_personal_work_items': {
      progress = { running: true, phase: 'syncing_personal_work_items', current: 0, total: 0, repository_current: 0, repository_total: 0, message: 'Searching your authored and assigned PRs and issues' }
      await new Promise((resolve) => window.setTimeout(resolve, 8000))
      progress = { ...progress, running: false, phase: 'personal_work_items_complete', message: 'Your PRs and issues refreshed' }
      return { ok: true, message: 'Your PRs and issues refreshed', repositories_synced: 0, pull_requests_synced: 3, issues_synced: 2, snapshots_created: 0, errors: [] }
    }
    case 'sync_work_items': {
      for (let i = 0; i < 3; i++) {
        progress = { running: true, phase: 'syncing_work_items', current: i, total: 3, repository_current: i, repository_total: 3, repository_name: repositories[i].name_with_owner, message: `Refreshing PRs and issues: repository ${i + 1} of 3` }
        await new Promise((resolve) => window.setTimeout(resolve, 5000))
      }
      progress = { ...progress, running: false, phase: 'work_items_complete', current: 3, repository_current: 3, message: 'PRs and issues refreshed' }
      return { ok: true, message: 'PRs and issues refreshed', repositories_synced: 3, pull_requests_synced: 5, issues_synced: 3, snapshots_created: 0, errors: [] }
    }
    case 'get_app_info': return { name: 'CodeTally', version: '0.1.2', identifier: 'com.samfaid.codetally', repository_url: 'https://github.com/Xzese/CodeTally' }
    case 'check_for_updates': return { current_version: '0.1.2', latest_version: window.location.search.includes('update-available') ? '0.1.8' : '0.1.2', release_url: '', update_available: window.location.search.includes('update-available') }
    case 'get_database_location': return 'file:///private/tmp/codetally-screenshot-fixture.sqlite3'
    case 'open_external_url': return true
    default: throw new Error(`Screenshot fixture does not implement ${command}`)
  }
}
