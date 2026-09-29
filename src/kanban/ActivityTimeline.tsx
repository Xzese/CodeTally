import { CircleDot, ExternalLink, GitCommitHorizontal, MessageSquare, RefreshCw } from 'lucide-react'
import type { KanbanDiscussion, KanbanItem } from '../types'
import { relativeTime } from '../utils'
import { isGitHubCommentUrl, isGitHubCommitUrl, isGitHubWorkUrl, isSafeHttpsUrl, sourceStateLabel } from './model'
import { commentSegments, namedCommentLinks } from './commentLinks'

type Entry = { kind: 'comment' | 'commit' | 'state'; id: string; at: string; index: number }

function checkLabel(value: string): string {
  return value.replaceAll('_', ' ').toLowerCase().replace(/^\w/, (letter) => letter.toUpperCase())
}

export default function ActivityTimeline({ item, discussion, error, loadingMore, refreshing, onRefresh, onLoadMore, onOpenUrl }: {
  item: KanbanItem
  discussion: KanbanDiscussion | null
  error: string
  loadingMore: 'comments' | 'checks' | 'commits' | null
  refreshing: boolean
  onRefresh: () => void
  onLoadMore: (section: 'comments' | 'checks' | 'commits') => void
  onOpenUrl: (url: string) => void
}) {
  const entries: Entry[] = [{ kind: 'state', id: 'source-state', at: item.updated_at, index: 0 }]
  discussion?.comments.forEach((comment, index) => entries.push({ kind: 'comment', id: comment.id, at: comment.created_at, index }))
  discussion?.commits.forEach((commit, index) => entries.push({ kind: 'commit', id: commit.oid, at: commit.committed_at, index }))
  entries.sort((a, b) => b.at.localeCompare(a.at) || a.kind.localeCompare(b.kind) || a.id.localeCompare(b.id))
  const latestCommit = discussion?.commits.at(-1)?.oid
  const showChecksSeparately = item.kind === 'pr' && !latestCommit
  const checks = item.kind === 'pr' && discussion && <div className="kanban-timeline-checks">
    <strong>Checks for latest commit {discussion.refreshed_at || discussion.checks.length || discussion.check_count ? `· ${discussion.check_count}` : '· not loaded'}</strong>
    <p>Passing checks alone does not prove the PR can merge.</p>
    {!discussion.checks.length && <p>{discussion.partial && !discussion.refreshed_at ? 'Checks are not loaded yet.' : 'No checks reported for the current commit.'}</p>}
    <ul className="kanban-checks">{discussion.checks.map((check) => <li key={check.id}><span className={`kanban-check-status check-${check.status.toLowerCase()}`}>{checkLabel(check.status)}</span><strong>{check.name}</strong>{check.details_url && isSafeHttpsUrl(check.details_url) && <button type="button" className="kanban-text-link" onClick={() => onOpenUrl(check.details_url!)}>Details <ExternalLink size={12} aria-hidden="true" /></button>}</li>)}</ul>
    {discussion.checks_has_more && <button className="button secondary compact" type="button" disabled={loadingMore !== null} onClick={() => onLoadMore('checks')}>{loadingMore === 'checks' ? 'Loading…' : `Load more checks (${discussion.checks.length} of ${discussion.check_count})`}</button>}
  </div>

  return <section className="kanban-detail-section kanban-activity-section" aria-label="Activity timeline">
    <div className="kanban-detail-section-heading"><h3>Activity timeline {discussion && (discussion.refreshed_at || discussion.commits.length || discussion.comments.length) && <span>{item.kind === 'pr' ? `${discussion.commit_count} commits · ` : ''}{discussion.comment_count} comments</span>}</h3><div className="kanban-detail-section-actions"><button className="button secondary compact" type="button" disabled={refreshing} onClick={onRefresh}><RefreshCw size={13} aria-hidden="true" /> {refreshing ? 'Refreshing…' : 'Refresh activity'}</button><button className="button secondary compact" type="button" disabled={!isGitHubWorkUrl(item.url)} onClick={() => onOpenUrl(item.url)}>Add comment on GitHub <ExternalLink size={13} aria-hidden="true" /></button></div></div>
    <p className="kanban-section-hint">Read-only GitHub history. PR line review comments remain on GitHub.{discussion?.refreshed_at ? ` Last loaded ${relativeTime(discussion.refreshed_at)}.` : ''}</p>
    {discussion?.partial && <p className="kanban-detail-alert" role="status">{discussion.message ?? 'Activity may be incomplete.'}</p>}
    {error && <p className="kanban-detail-alert" role="alert">{error}</p>}
    {!discussion && !error && <p>Loading ticket activity…</p>}
    <ol className="kanban-timeline">
      {showChecksSeparately && <li className="kanban-timeline-event"><span className="kanban-timeline-marker"><CircleDot size={13} aria-hidden="true" /></span><div className="kanban-timeline-content">{checks ?? <p>Loading current PR checks…</p>}</div></li>}
      {entries.map((entry) => {
        if (entry.kind === 'state') return <li className="kanban-timeline-event" key={entry.id}><span className="kanban-timeline-marker"><CircleDot size={13} aria-hidden="true" /></span><div className="kanban-timeline-content"><div className="kanban-timeline-heading"><strong>GitHub state: {sourceStateLabel(item)}</strong><time dateTime={entry.at}>{relativeTime(entry.at)}</time></div><p>Latest ticket update recorded by GitHub.</p></div></li>
        if (entry.kind === 'comment') {
          const comment = discussion!.comments[entry.index]
          const links = namedCommentLinks(comment.body_markdown, comment.body_text)
          return <li className="kanban-timeline-event" key={entry.id}><span className="kanban-timeline-marker"><MessageSquare size={13} aria-hidden="true" /></span><div className="kanban-timeline-content"><div className="kanban-timeline-heading"><strong>{comment.author ? `@${comment.author}` : 'Unknown author'} commented</strong><time dateTime={comment.created_at}>{relativeTime(comment.created_at)}</time></div><p className="kanban-timeline-comment">{comment.body_text ? commentSegments(comment.body_text).map((segment, index) => segment.url ? <button className="kanban-comment-url" type="button" key={index} title={segment.url} onClick={() => onOpenUrl(segment.url!)}>{segment.text}</button> : segment.text) : '(No text)'}</p>{links.length > 0 && <div className="kanban-comment-links" aria-label="Links in comment">{links.map((link) => <button className="kanban-text-link" type="button" key={link.url} title={link.url} onClick={() => onOpenUrl(link.url)}>{link.label} <ExternalLink size={12} aria-hidden="true" /></button>)}</div>}{isGitHubCommentUrl(comment.url, item.url) && <button type="button" className="kanban-text-link" onClick={() => onOpenUrl(comment.url)}>View comment on GitHub <ExternalLink size={12} aria-hidden="true" /></button>}</div></li>
        }
        const commit = discussion!.commits[entry.index]
        return <li className="kanban-timeline-event" key={entry.id}><span className="kanban-timeline-marker"><GitCommitHorizontal size={13} aria-hidden="true" /></span><div className="kanban-timeline-content"><div className="kanban-timeline-heading"><strong>Commit <code>{commit.oid.slice(0, 7)}</code></strong><time dateTime={commit.committed_at}>{relativeTime(commit.committed_at)}</time></div><p className="kanban-timeline-headline">{commit.headline}</p>{commit.author && <p className="kanban-timeline-byline">by {commit.author}</p>}{isGitHubCommitUrl(commit.url, item.url) && <button type="button" className="kanban-text-link" onClick={() => onOpenUrl(commit.url)}>View commit on GitHub <ExternalLink size={12} aria-hidden="true" /></button>}{entry.id === latestCommit && checks}</div></li>
      })}
    </ol>
    {discussion && <div className="kanban-timeline-pages">{discussion.comments_has_more && <button className="button secondary compact" type="button" disabled={loadingMore !== null} onClick={() => onLoadMore('comments')}>{loadingMore === 'comments' ? 'Loading…' : `Load earlier comments (${discussion.comments.length} of ${discussion.comment_count})`}</button>}{discussion.commits_has_more && <button className="button secondary compact" type="button" disabled={loadingMore !== null} onClick={() => onLoadMore('commits')}>{loadingMore === 'commits' ? 'Loading…' : `Load earlier commits (${discussion.commits.length} of ${discussion.commit_count})`}</button>}</div>}
  </section>
}
