import { memo } from 'react'
import { CartesianGrid, Line, LineChart, ResponsiveContainer, Tooltip, XAxis, YAxis } from 'recharts'
import { BarChart3 } from 'lucide-react'
import type { LocSeries, TimeRange } from './types'
import type { ChartPoint } from './model'
import { exactDate, formatCompact, formatCount } from './utils'

const axisDateFormatter = new Intl.DateTimeFormat(undefined, { month: 'short', year: '2-digit' })
const series: { key: LocSeries; name: string; label: string; color: string }[] = [
  { key: 'source', name: 'Source', label: 'Source Lines', color: '#52d2b1' },
  { key: 'tests', name: 'Tests', label: 'Test Lines', color: '#d3a9ff' },
  { key: 'docs', name: 'Docs', label: 'Docs Lines', color: '#e8b568' }
]

function LocChart({ history, visibleSeries, range, onSeries, onRange }: { history: ChartPoint[]; visibleSeries: LocSeries[]; range: TimeRange; onSeries: (series: LocSeries[]) => void; onRange: (range: TimeRange) => void }) {
  const visible = series.filter((entry) => visibleSeries.includes(entry.key))
  const label = visible.length === 1 ? visible[0].label : visibleSeries.includes('docs') ? 'Selected Total Lines' : 'Total Lines'
  const points = history.map((point) => ({ ...point, selectedTotal: visible.reduce((sum, entry) => sum + point[entry.key], 0) }))
  const toggle = (key: LocSeries) => {
    if (visibleSeries.includes(key)) {
      if (visibleSeries.length > 1) onSeries(visibleSeries.filter((entry) => entry !== key))
    } else onSeries([...visibleSeries, key])
  }
  return <section className="chart-panel" aria-label="Line history chart">
    <div className="panel-heading">
      <div><p className="eyebrow">Growth over time</p><h2>{label}</h2></div>
      <div className="chart-controls">
        <div className="segmented" role="group" aria-label="Visible line categories">
          {series.map(({ key, name }) => <button key={key} aria-pressed={visibleSeries.includes(key)} disabled={visibleSeries.length === 1 && visibleSeries.includes(key)} className={visibleSeries.includes(key) ? 'active' : ''} onClick={() => toggle(key)} title={visibleSeries.length === 1 && visibleSeries.includes(key) ? 'Keep at least one category visible' : `Toggle ${name.toLowerCase()} lines`}>{name}</button>)}
        </div>
        <div className="range-controls" aria-label="Time range">{(['30D', '3M', '1Y', '3Y', 'ALL'] as const).map((key) => <button key={key} className={range === key ? 'active' : ''} onClick={() => onRange(key)}>{key}</button>)}</div>
      </div>
    </div>
    {history.length ? <div className="chart-wrap">
      <ResponsiveContainer width="100%" height="100%">
        <LineChart data={points} margin={{ top: 18, right: 18, left: 2, bottom: 4 }}>
          <CartesianGrid stroke="rgba(148, 163, 184, .11)" vertical={false} />
          <XAxis type="number" dataKey="timestamp" domain={['dataMin', 'dataMax']} tickFormatter={(value) => { const parsed = new Date(Number(value)); return Number.isNaN(parsed.getTime()) ? '' : axisDateFormatter.format(parsed) }} tick={{ fill: '#8491a7', fontSize: 11 }} tickLine={false} axisLine={false} minTickGap={38} />
          <YAxis tickFormatter={formatCompact} tick={{ fill: '#8491a7', fontSize: 11 }} tickLine={false} axisLine={false} width={43} />
          <Tooltip shared={true} isAnimationActive={false} position={{ y: 8 }} content={<LocTooltip />} cursor={{ stroke: 'rgba(137,162,203,.35)', strokeDasharray: '4 4' }} />
          {visible.length > 1 && <Line isAnimationActive={false} type="monotone" dataKey="selectedTotal" name="Selected total" stroke="#68a6ff" strokeWidth={2.7} dot={false} activeDot={{ r: 4 }} />}
          {visible.map(({ key, name, color }) => <Line key={key} isAnimationActive={false} type="monotone" dataKey={key} name={name} stroke={color} strokeWidth={visible.length === 1 ? 2.7 : 1.6} dot={false} activeDot={{ r: 4 }} />)}
        </LineChart>
      </ResponsiveContainer>
      <div className="chart-legend">{visible.length > 1 && <span style={{ color: '#68a6ff' }}><i />Selected total</span>}{visible.map(({ key, name, color }) => <span key={key} style={{ color }}><i />{name}</span>)}<span className="chart-hint">Total follows selected categories</span></div>
    </div> : <div className="empty-chart"><BarChart3 size={24} /><p>No line history for this range</p><span>Import a repository to create the first monthly snapshot.</span></div>}
  </section>
}

function LocTooltip({ active, payload, label }: { active?: boolean; payload?: Array<{ name?: string; value?: number; color?: string }>; label?: string | number }) {
  if (!active || !payload?.length) return null
  return <div className="chart-tooltip"><strong>{exactDate(label)}</strong>{payload.map((entry) => <div key={entry.name} className="tooltip-row"><span><i style={{ background: entry.color }} />{entry.name}</span><b>{formatCount(Number(entry.value ?? 0))}</b></div>)}</div>
}

export default memo(LocChart)
