import { useMemo } from 'react'
import { aggregateHistory } from './model'
import type { LocSnapshot, SyncProgress, TimeRange } from './types'
import type { NormalizedMetrics } from './utils'

export type CombinedMenuPreviewData = {
  history?: LocSnapshot[]
  lastFullRefreshAt?: string | null
  lastActivityRefreshAt?: string | null
  lastPersonalRefreshAt?: string | null
  partialLoc?: boolean
  progress?: SyncProgress | null
}

type Props = {
  metrics: NormalizedMetrics
  range: TimeRange
  data?: CombinedMenuPreviewData
}

type ChartPoint = { date: string; value: number }

const RANGE_LABELS: Record<TimeRange, string> = {
  '30D': '30 days',
  '3M': '3 months',
  '1Y': '1 year',
  '3Y': '3 years',
  ALL: 'all time'
}

const formatNumber = (value: number) => Math.abs(value).toLocaleString('en-US')
const formatSigned = (value: number) => `${value >= 0 ? '+' : '-'}${formatNumber(value)}`

function localRefreshTime(value?: string | null): string | null {
  if (!value) return null
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return null
  const monthDay = new Intl.DateTimeFormat('en-US', { month: 'short', day: 'numeric' }).format(date)
  const time = new Intl.DateTimeFormat('en-GB', { hour: '2-digit', minute: '2-digit', hourCycle: 'h23' }).format(date)
  return `${monthDay}, ${time}`
}

function utcDay(value: string): string | null {
  // Rust's chart parser consumes YYYY-MM-DD saved days. Dashboard fixtures and
  // older snapshots may carry an ISO timestamp, so use its UTC calendar day.
  const day = value.slice(0, 10)
  if (!/^\d{4}-\d{2}-\d{2}$/.test(day)) return null
  const parsed = new Date(`${day}T00:00:00.000Z`)
  return Number.isNaN(parsed.getTime()) || parsed.toISOString().slice(0, 10) !== day ? null : day
}

function shiftUtcMonth(day: string, months: number): string {
  const [year, month, date] = day.split('-').map(Number)
  const monthIndex = year * 12 + month - 1 - months
  const targetYear = Math.floor(monthIndex / 12)
  const targetMonth = ((monthIndex % 12) + 12) % 12
  const lastDay = new Date(Date.UTC(targetYear, targetMonth + 1, 0)).getUTCDate()
  return new Date(Date.UTC(targetYear, targetMonth, Math.min(date, lastDay))).toISOString().slice(0, 10)
}

function addUtcDays(day: string, days: number): string {
  const date = new Date(`${day}T00:00:00.000Z`)
  date.setUTCDate(date.getUTCDate() + days)
  return date.toISOString().slice(0, 10)
}

function getRangeStart(range: TimeRange, today: string, history: ChartPoint[]): string {
  if (range === '30D') return addUtcDays(today, -29)
  if (range === '3M') return shiftUtcMonth(today, 3)
  if (range === '1Y') return shiftUtcMonth(today, 12)
  if (range === '3Y') return shiftUtcMonth(today, 36)
  return history[0]?.date ?? today
}

function rangeChart(history: LocSnapshot[], range: TimeRange, today: string) {
  const byDay = new Map<string, number>()
  aggregateHistory(history).forEach((point) => {
    const date = utcDay(point.date)
    if (date) byDay.set(date, (byDay.get(date) ?? 0) + point.total)
  })
  const points = [...byDay].map(([date, value]) => ({ date, value })).sort((left, right) => left.date.localeCompare(right.date))
  const start = getRangeStart(range, today, points)
  const visible = points.filter((point) => point.date >= start && point.date <= today)
  const bins = range === '30D' ? 30 : 48
  const values: Array<ChartPoint | null> = Array.from({ length: bins }, () => null)
  const span = Math.max(1, Math.round((Date.parse(`${today}T00:00:00Z`) - Date.parse(`${start}T00:00:00Z`)) / 86_400_000))

  for (const point of visible) {
    const offset = Math.round((Date.parse(`${point.date}T00:00:00Z`) - Date.parse(`${start}T00:00:00Z`)) / 86_400_000)
    const index = Math.floor(offset * (bins - 1) / span)
    const existing = values[index]
    if (!existing || point.date >= existing.date) values[index] = point
  }

  return {
    values,
    first: visible[0] ?? null,
    latest: visible[visible.length - 1] ?? null,
    measuredDays: new Set(visible.map((point) => point.date)).size
  }
}

function LocChart({ values }: { values: Array<ChartPoint | null> }) {
  const samples = values.filter((point): point is ChartPoint => point !== null)
  if (!samples.length) return null
  const minimum = Math.min(...samples.map((point) => point.value))
  const maximum = Math.max(...samples.map((point) => point.value))
  const firstIndex = values.findIndex((point) => point !== null)
  const lastIndex = values.length - 1 - [...values].reverse().findIndex((point) => point !== null)
  const plotted = values.flatMap((point, index) => {
    if (!point) return []
    const x = firstIndex === lastIndex ? 150 : 6 + (index - firstIndex) * 288 / (lastIndex - firstIndex)
    const y = minimum === maximum ? 36 : 59 - (point.value - minimum) / (maximum - minimum) * 48
    return [{ x, y }]
  })
  const path = plotted.map((point, index) => `${index ? 'L' : 'M'}${point.x.toFixed(2)} ${point.y.toFixed(2)}`).join(' ')
  return <svg className="native-menu-preview-chart" role="img" aria-label={`${samples.length} measured line count samples`} viewBox="0 0 300 72" preserveAspectRatio="none">
    {[10, 35, 63].map((y) => <path key={y} className="native-menu-preview-grid" d={`M4 ${y}h292`} />)}
    {plotted.length > 1 && <path className="native-menu-preview-line" d={path} />}
    {plotted.map((point, index) => {
      const size = index === plotted.length - 1 ? 4.5 : 3.5
      return <rect key={index} className={index === plotted.length - 1 ? 'latest' : undefined} x={point.x - size / 2} y={point.y - size / 2} width={size} height={size} />
    })}
  </svg>
}

function Separator() {
  return <div className="native-menu-preview-separator" aria-hidden="true" />
}

function SummaryRows({ metrics }: { metrics: NormalizedMetrics }) {
  const rows = [
    `${formatNumber(metrics.total_loc)} total lines`,
    `${formatNumber(metrics.repositories)} repositories`,
    `${formatNumber(metrics.source_loc)} source lines`,
    `${formatNumber(metrics.test_loc)} test lines`,
    `${formatSigned(metrics.loc_30d_change)} lines · 30-day change`,
    `${formatNumber(metrics.open_prs)} open PRs`,
    `${formatNumber(metrics.open_issues)} open issues`
  ]
  return <div className="native-menu-preview-summary">{rows.map((row) => <div className="native-menu-preview-info-row" key={row}>{row}</div>)}</div>
}

export function CombinedMenuPreview({ metrics, range, data }: Props) {
  const history = data?.history ?? []
  const today = new Date().toISOString().slice(0, 10)
  const chart = useMemo(() => rangeChart(history, range, today), [history, range, today])
  const latestActivity = [data?.lastActivityRefreshAt, data?.lastPersonalRefreshAt]
    .filter((value): value is string => Boolean(value))
    .reduce<string | undefined>((latest, current) => !latest || current > latest ? current : latest, undefined)
  const repositoryRefresh = localRefreshTime(data?.lastFullRefreshAt)
  const ticketsRefresh = localRefreshTime(latestActivity)
  const progress = data?.progress

  return <section className="native-menu-preview" aria-label="Combined menu bar preview">
    <div className="native-menu-preview-info-row">CodeTally</div>
    <div className="native-menu-preview-info-row">{progress?.running ? `Refreshing · ${progress.message}` : repositoryRefresh ? `Repositories refreshed ${repositoryRefresh}` : 'No full refresh recorded'}</div>
    {ticketsRefresh && <div className="native-menu-preview-info-row">Tickets refreshed {ticketsRefresh}</div>}
    {data?.partialLoc && <div className="native-menu-preview-info-row">Partial line counts</div>}
    <Separator />
    <SummaryRows metrics={metrics} />
    <Separator />
    <div className="native-menu-preview-section-label">Total lines · {RANGE_LABELS[range]}</div>
    {chart.latest ? <>
      <LocChart values={chart.values} />
      <div className="native-menu-preview-chart-copy">Latest: {formatNumber(chart.latest.value)} lines · {chart.latest.date}</div>
      {chart.first && chart.first.date !== chart.latest.date && <div className="native-menu-preview-chart-copy">Change since {chart.first.date}: {formatSigned(chart.latest.value - chart.first.value)} lines</div>}
      <div className="native-menu-preview-chart-copy">{chart.measuredDays} saved sample days</div>
    </> : <div className="native-menu-preview-muted-row">No LOC samples for {RANGE_LABELS[range]}</div>}
    <Separator />
    <div className="native-menu-preview-actions" aria-hidden="true">
      <div>Open CodeTally</div>
      <div>Settings…</div>
      <div>Quit CodeTally</div>
    </div>
  </section>
}
