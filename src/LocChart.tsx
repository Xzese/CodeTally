import { memo } from 'react'
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { BarChart3 } from 'lucide-react'
import type { TimeRange } from './types'
import { exactDate, formatCompact, formatCount } from './utils'

const axisDateFormatter = new Intl.DateTimeFormat(undefined, { month: 'short', year: '2-digit' })

function LocChart({ history, metric, range, onMetric, onRange }: { history: { date: string; timestamp: number; total: number; source: number; tests: number }[]; metric: 'total' | 'source' | 'tests'; range: TimeRange; onMetric: (metric: 'total' | 'source' | 'tests') => void; onRange: (range: TimeRange) => void }) {
  const label = metric === 'total' ? 'Total Lines' : metric === 'source' ? 'Source Lines' : 'Test Lines'
  const color = metric === 'total' ? '#68a6ff' : metric === 'source' ? '#52d2b1' : '#d3a9ff'
  return <section className="chart-panel"><div className="panel-heading"><div><p className="eyebrow">Growth over time</p><h2>{label}</h2></div><div className="chart-controls"><div className="segmented" role="tablist" aria-label="Lines metric">{(['total', 'source', 'tests'] as const).map((key) => <button key={key} className={metric === key ? 'active' : ''} onClick={() => onMetric(key)}>{key === 'total' ? 'Total' : key === 'source' ? 'Source' : 'Tests'}</button>)}</div><div className="range-controls" aria-label="Time range">{(['3M', '1Y', '3Y', 'ALL'] as const).map((key) => <button key={key} className={range === key ? 'active' : ''} onClick={() => onRange(key)}>{key}</button>)}</div></div></div>{history.length ? <div className="chart-wrap"><ResponsiveContainer width="100%" height="100%"><LineChart data={history} margin={{ top: 18, right: 18, left: 2, bottom: 4 }}><CartesianGrid stroke="rgba(148, 163, 184, .11)" vertical={false} /><XAxis type="number" dataKey="timestamp" domain={['dataMin', 'dataMax']} tickFormatter={(value) => { const parsed = new Date(Number(value)); return Number.isNaN(parsed.getTime()) ? '' : axisDateFormatter.format(parsed) }} tick={{ fill: '#8491a7', fontSize: 11 }} tickLine={false} axisLine={false} minTickGap={38} /><YAxis tickFormatter={formatCompact} tick={{ fill: '#8491a7', fontSize: 11 }} tickLine={false} axisLine={false} width={43} /><Tooltip content={<LocTooltip />} cursor={{ stroke: 'rgba(137,162,203,.35)', strokeDasharray: '4 4' }} /><Line type="monotone" dataKey="total" name="Total" stroke="#68a6ff" strokeWidth={metric === 'total' ? 2.7 : 1.4} strokeOpacity={metric === 'total' ? 1 : .26} dot={false} activeDot={{ r: 4, strokeWidth: 2, fill: '#0d1421' }} /><Line type="monotone" dataKey="source" name="Source" stroke="#52d2b1" strokeWidth={metric === 'source' ? 2.7 : 1.4} strokeOpacity={metric === 'source' ? 1 : .26} dot={false} activeDot={{ r: 4, strokeWidth: 2, fill: '#0d1421' }} /><Line type="monotone" dataKey="tests" name="Tests" stroke="#d3a9ff" strokeWidth={metric === 'tests' ? 2.7 : 1.4} strokeOpacity={metric === 'tests' ? 1 : .26} dot={false} activeDot={{ r: 4, strokeWidth: 2, fill: '#0d1421' }} /></LineChart></ResponsiveContainer><div className="chart-legend"><span style={{ color }}><i /> {label}</span><span className="chart-hint">Hover for all series</span></div></div> : <div className="empty-chart"><BarChart3 size={24} /><p>No line history for this range</p><span>Import a repository to create the first monthly snapshot.</span></div>}</section>
}

function LocTooltip({ active, payload, label }: { active?: boolean; payload?: Array<{ name?: string; value?: number; color?: string }>; label?: string | number }) {
  if (!active || !payload?.length) return null
  return <div className="chart-tooltip"><strong>{exactDate(label)}</strong>{payload.map((entry) => <div key={entry.name} className="tooltip-row"><span><i style={{ background: entry.color }} />{entry.name}</span><b>{formatCount(Number(entry.value ?? 0))}</b></div>)}</div>
}


export default memo(LocChart)
