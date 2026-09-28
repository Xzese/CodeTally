import { ChevronDown } from 'lucide-react'
import type { ActivityRelationship } from './types'

const OPTIONS: Array<{ value: ActivityRelationship; label: string }> = [
  { value: 'everyone', label: 'Everyone' },
  { value: 'author', label: 'Authored by me' },
  { value: 'assignee', label: 'Assigned to me' },
  { value: 'author_or_assignee', label: 'Authored or assigned to me' }
]

export default function ActivityInvolvementFilter({ value, login, onChange, id }: { value: ActivityRelationship; login: string; onChange: (value: ActivityRelationship) => void; id: string }) {
  return <div className="activity-involvement-filter"><label htmlFor={id}>My involvement</label><div className="select-wrap"><select id={id} aria-label="My involvement" value={value} onChange={(event) => onChange(event.target.value as ActivityRelationship)}>{OPTIONS.map((option) => <option key={option.value} value={option.value}>{option.label}</option>)}</select><ChevronDown size={14} /></div>{login && <span className="activity-involvement-note">Signed in as {login}</span>}</div>
}
