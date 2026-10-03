import type { ActivityRelationship, AppSettings, MenuBarMetric, UpdateCheckInterval } from './types'

const METRIC_ORDER: MenuBarMetric[] = ['total_lines', 'source_lines', 'test_lines', 'open_prs', 'open_issues']
const REPO_REFRESH_INTERVALS = [60, 1440, 10080, 43200]

export const DEFAULT_APP_SETTINGS: AppSettings = {
  kanban_enabled: true,
  activity_refresh_minutes: 1440,
  personal_refresh_minutes: 5,
  activity_relationship: 'author',
  update_check_interval: 'daily',
  lines_refresh_minutes: 1440,
  refresh_lines_on_change: true,
  include_forks_in_totals: false,
  run_in_background: true,
  menu_bar_metric: 'total_lines',
  loc_chart_range: 'ALL',
  menu_bar_metrics: ['total_lines'],
  menu_bar_compact_metrics: [],
  show_menu_bar: true,
  menu_bar_combined: false,
  theme_mode: 'system',
  include_personal_repositories: true,
  include_company_repositories: true,
  excluded_repository_ids: []
}

export function normalizeAppSettings(settings: Partial<AppSettings>): AppSettings {
  const candidates = settings.menu_bar_metrics?.length ? settings.menu_bar_metrics : [settings.menu_bar_metric ?? 'total_lines']
  const selected = METRIC_ORDER.filter((metric) => candidates.includes(metric))
  const menuMetrics = selected.length ? selected : ['total_lines' as const]
  const activityRelationship: ActivityRelationship = settings.activity_relationship === 'everyone' || settings.activity_relationship === 'author' || settings.activity_relationship === 'assignee' || settings.activity_relationship === 'author_or_assignee'
    ? settings.activity_relationship
    : DEFAULT_APP_SETTINGS.activity_relationship
  const updateCheckInterval: UpdateCheckInterval = settings.update_check_interval === 'daily' || settings.update_check_interval === 'weekly' || settings.update_check_interval === 'monthly' || settings.update_check_interval === 'never'
    ? settings.update_check_interval
    : DEFAULT_APP_SETTINGS.update_check_interval
  return {
    ...DEFAULT_APP_SETTINGS,
    ...settings,
    activity_refresh_minutes: REPO_REFRESH_INTERVALS.includes(settings.activity_refresh_minutes ?? NaN)
      ? settings.activity_refresh_minutes!
      : DEFAULT_APP_SETTINGS.activity_refresh_minutes,
    theme_mode: settings.theme_mode ?? 'system',
    activity_relationship: activityRelationship,
    update_check_interval: updateCheckInterval,
    menu_bar_metrics: menuMetrics,
    menu_bar_metric: menuMetrics[0],
    loc_chart_range: ['30D', '3M', '1Y', '3Y', 'ALL'].includes(settings.loc_chart_range ?? '') ? settings.loc_chart_range! : 'ALL',
    menu_bar_compact_metrics: METRIC_ORDER.filter((metric) => settings.menu_bar_compact_metrics?.includes(metric)),
    excluded_repository_ids: settings.excluded_repository_ids ?? []
  }
}
