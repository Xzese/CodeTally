import type { ActivityRelationship, AppSettings, MenuBarMetric } from './types'

const METRIC_ORDER: MenuBarMetric[] = ['total_lines', 'source_lines', 'test_lines', 'open_prs', 'open_issues']

export const DEFAULT_APP_SETTINGS: AppSettings = {
  activity_refresh_minutes: 30,
  activity_relationship: 'author',
  lines_refresh_minutes: 120,
  refresh_lines_on_change: true,
  include_forks_in_totals: false,
  run_in_background: true,
  menu_bar_metric: 'total_lines',
  menu_bar_metrics: ['total_lines'],
  show_menu_bar: true,
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
  return {
    ...DEFAULT_APP_SETTINGS,
    ...settings,
    theme_mode: settings.theme_mode ?? 'system',
    activity_relationship: activityRelationship,
    menu_bar_metrics: menuMetrics,
    menu_bar_metric: menuMetrics[0],
    excluded_repository_ids: settings.excluded_repository_ids ?? []
  }
}
