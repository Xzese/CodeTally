//! Native lifecycle and scheduling remain active when the dashboard is hidden.
use crate::models::{AppSettings, DashboardTotals, HistoryPoint, MenuBarMetric};
use crate::sync::{self, AppState};
use chrono::{Duration as ChronoDuration, NaiveDate, Utc};
use std::time::{Duration, Instant};
use tauri::{menu::{Menu, MenuBuilder}, tray::TrayIconBuilder, AppHandle, Emitter, Manager};

const TRAY_ID: &str = "codetally";
const GITHUB_MENU_PREFIX: &str = "open-github:";
const MENU_ACTIVITY_LIMIT: usize = 25;
const METRIC_TRAYS: [(MenuBarMetric, &str); 5] = [
    (MenuBarMetric::TotalLines, "codetally-total-lines"),
    (MenuBarMetric::SourceLines, "codetally-source-lines"),
    (MenuBarMetric::TestLines, "codetally-test-lines"),
    (MenuBarMetric::OpenPrs, "codetally-open-prs"),
    (MenuBarMetric::OpenIssues, "codetally-open-issues"),
];

pub fn hides_dock_icon(settings: &AppSettings) -> bool {
    settings.run_in_background && settings.show_menu_bar
}

pub fn apply_activation_policy(app: &AppHandle, settings: &AppSettings) -> tauri::Result<()> {
    #[cfg(target_os = "macos")]
    app.set_activation_policy(if hides_dock_icon(settings) {
        tauri::ActivationPolicy::Accessory
    } else {
        tauri::ActivationPolicy::Regular
    })?;
    #[cfg(not(target_os = "macos"))]
    let _ = (app, settings);
    Ok(())
}

pub fn metric_title(metric: MenuBarMetric, totals: &DashboardTotals) -> String {
    let (value, label) = match metric {
        MenuBarMetric::TotalLines => (totals.total_loc, "lines"),
        MenuBarMetric::TestLines => (totals.test_loc, "tests"),
        MenuBarMetric::SourceLines => (totals.source_loc, "source"),
        MenuBarMetric::OpenPrs => (totals.open_prs, "PRs"),
        MenuBarMetric::OpenIssues => (totals.open_issues, "issues"),
    };
    let number = if value >= 1_000_000_000 { format!("{:.1}B", value as f64 / 1_000_000_000.0) }
        else if value >= 1_000_000 { format!("{:.1}M", value as f64 / 1_000_000.0) }
        else if value >= 1_000 { format!("{:.1}K", value as f64 / 1_000.0) }
        else { value.to_string() };
    format!("{number} {label}")
}

pub fn menu_titles(metrics: &[MenuBarMetric], totals: &DashboardTotals) -> Vec<String> {
    metrics.iter().map(|metric| metric_title(*metric, totals)).collect()
}

/// Transparent monochrome bitmaps are rendered as templates by macOS.
fn metric_icon(metric: MenuBarMetric) -> tauri::image::Image<'static> {
    let rows: [&str; 16] = match metric {
        MenuBarMetric::TotalLines => [
            "................", "............###.", "............###.", "............###.",
            "............###.", ".......###..###.", ".......###..###.", ".......###..###.",
            ".......###..###.", "..###..###..###.", "..###..###..###.", "..###..###..###.",
            "..###..###..###.", "..###..###..###.", "..###..###..###.", "................",
        ],
        MenuBarMetric::SourceLines => [
            "................", ".........##.....", ".........##.....", "........##......",
            "....##..##.##...", "...##...##..##..", "..##...##....##.", ".##....##.....##",
            "..##...##....##.", "...##.##....##..", "....####...##...", "......##........",
            ".....##.........", ".....##.........", "................", "................",
        ],
        MenuBarMetric::TestLines => [
            ".....######.....", ".....######.....", "......#..#......", "......#..#......",
            "......#..#......", ".....##..##.....", ".....#....#.....", "....##....##....",
            "....########....", "...##......##...", "...#..##....#...", "..##.....#..##..",
            "..#..........#..", ".##..........##.", ".##############.", "................",
        ],
        MenuBarMetric::OpenPrs => [
            "..###...........", ".#...#...##.....", ".#...#..####....", "..###..##..##...",
            "...#....##......", "...#....#####...", "...#........##..", "...#.........#..",
            "...#.........#..", "...#.........#..", "...#........###.", "..###......#...#",
            ".#...#.....#...#", ".#...#......###.", "..###...........", "................",
        ],
        MenuBarMetric::OpenIssues => [
            ".....######.....", "...###....###...", "..##........##..", ".##..........##.",
            ".#............#.", "##............##", "#......##......#", "#.....####.....#",
            "#.....####.....#", "#......##......#", "##............##", ".#............#.",
            ".##..........##.", "..##........##..", "...###....###...", ".....######.....",
        ],
    };
    let mut rgba = vec![0; 18 * 18 * 4];
    for (y, row) in rows.iter().enumerate() {
        for (x, pixel) in row.bytes().enumerate() {
            if pixel == b'#' { rgba[((y + 1) * 18 + x + 1) * 4 + 3] = 255; }
        }
    }
    tauri::image::Image::new_owned(rgba, 18, 18)
}

pub fn show_window(app: &AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
}

fn menu_label(repository: &str, number: i64, title: &str) -> String {
    let prefix = format!("{repository} #{number} — ");
    let remaining = 74usize.saturating_sub(prefix.chars().count());
    let mut shortened: String = title.chars().take(remaining).collect();
    if title.chars().count() > remaining { shortened.push('…'); }
    format!("{prefix}{shortened}")
}

fn loc_value(point: &HistoryPoint, metric: MenuBarMetric) -> i64 {
    match metric {
        MenuBarMetric::TotalLines => point.total_loc,
        MenuBarMetric::SourceLines => point.source_loc,
        MenuBarMetric::TestLines => point.test_loc,
        _ => 0,
    }
}

/// A 30-day chart from saved snapshots. Days before the first sample are left
/// blank, while gaps after a sample retain its last measured value.
fn loc_sparkline(history: &[HistoryPoint], metric: MenuBarMetric, today: NaiveDate) -> Option<(String, String)> {
    let mut samples: Vec<_> = history.iter().filter_map(|point| {
        NaiveDate::parse_from_str(&point.snapshot_date, "%Y-%m-%d").ok().map(|date| (date, loc_value(point, metric)))
    }).collect();
    samples.sort_by_key(|(date, _)| *date);
    let last_recorded = samples.last()?.0;
    let mut current = None;
    let mut index = 0;
    let values: Vec<_> = (0..30).map(|offset| {
        let day = today - ChronoDuration::days(29 - offset);
        while index < samples.len() && samples[index].0 <= day {
            current = Some(samples[index].1);
            index += 1;
        }
        current
    }).collect();
    let (minimum, maximum) = values.iter().flatten().fold((i64::MAX, i64::MIN), |(low, high), value| (low.min(*value), high.max(*value)));
    if minimum == i64::MAX { return None; }
    let levels = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let graph = values.into_iter().map(|value| match value {
        None => '·',
        Some(_) if minimum == maximum => '▄',
        Some(value) => {
            let level = ((value - minimum) as f64 / (maximum - minimum) as f64 * 7.0).round() as usize;
            levels[level.min(7)]
        }
    }).collect();
    Some((graph, last_recorded.to_string()))
}

fn tray_menu(app: &AppHandle, state: Option<&AppState>, metric: MenuBarMetric, history: Option<&[HistoryPoint]>) -> tauri::Result<Menu<tauri::Wry>> {
    let mut builder = MenuBuilder::new(app);
    let mut activity_count = 0;
    if matches!(metric, MenuBarMetric::TotalLines | MenuBarMetric::SourceLines | MenuBarMetric::TestLines) {
        let label = match metric {
            MenuBarMetric::TotalLines => "Total lines",
            MenuBarMetric::SourceLines => "Source lines",
            _ => "Test lines",
        };
        builder = builder.text("loc-trend-label", format!("{label} · last 30 days"));
        if let Some((graph, last_recorded)) = history.and_then(|points| loc_sparkline(points, metric, Utc::now().date_naive())) {
            builder = builder.text("loc-trend-graph", graph).text("loc-trend-as-of", format!("Last LOC sample: {last_recorded}"));
        } else {
            builder = builder.text("loc-trend-empty", "No LOC history available");
        }
        builder = builder.separator();
    }
    if let Some(state) = state {
        let db = state.database();
        match metric {
            MenuBarMetric::OpenPrs => {
                for item in db.pull_requests(None, Some("open"), MENU_ACTIVITY_LIMIT).unwrap_or_default() {
                    activity_count += 1;
                    builder = builder.text(format!("{GITHUB_MENU_PREFIX}{}", item.url), menu_label(&item.repository, item.number, &item.title));
                }
            }
            MenuBarMetric::OpenIssues => {
                for item in db.issues(None, Some("open"), MENU_ACTIVITY_LIMIT).unwrap_or_default() {
                    activity_count += 1;
                    builder = builder.text(format!("{GITHUB_MENU_PREFIX}{}", item.url), menu_label(&item.repository, item.number, &item.title));
                }
            }
            _ => {}
        }
    }
    if matches!(metric, MenuBarMetric::OpenPrs | MenuBarMetric::OpenIssues) {
        if activity_count == 0 {
            builder = builder.text("no-open-activity", match metric {
                MenuBarMetric::OpenPrs => "No open pull requests",
                _ => "No open issues",
            });
        }
        builder = builder.separator();
    }
    builder.text("show", "Show CodeTally").text("quit", "Quit CodeTally").build()
}

fn build_metric_tray(app: &AppHandle, state: Option<&AppState>, id: &str, metric: MenuBarMetric, title: &str, tooltip: &str, history: Option<&[HistoryPoint]>) -> tauri::Result<()> {
    TrayIconBuilder::with_id(id)
        .icon(metric_icon(metric))
        .icon_as_template(true)
        .title(title)
        .tooltip(tooltip)
        .menu(&tray_menu(app, state, metric, history)?)
        .build(app)?;
    Ok(())
}

pub fn refresh_menu(app: &AppHandle, state: &AppState) {
    let db = state.database();
    let Ok(settings) = sync::app_settings(&db) else { return; };
    if !settings.show_menu_bar {
        drop(app.remove_tray_by_id(TRAY_ID));
        for (_, id) in METRIC_TRAYS { drop(app.remove_tray_by_id(id)); }
        return;
    }
    let Ok(summaries) = db.summaries() else { return; };
    let Ok(totals) = db.totals(&summaries) else { return; };
    let metrics = settings.effective_menu_bar_metrics();
    let history = if metrics.iter().any(|metric| matches!(metric, MenuBarMetric::TotalLines | MenuBarMetric::SourceLines | MenuBarMetric::TestLines)) {
        db.history(None).unwrap_or_default()
    } else { Vec::new() };
    let titles = menu_titles(&metrics, &totals);
    let combined_title = titles.join(" · ");
    let tooltip = format!("CodeTally — {combined_title}");
    if app.tray_by_id(TRAY_ID).is_none() {
        let _ = build_metric_tray(app, Some(state), TRAY_ID, metrics[0], &titles[0], &tooltip, Some(&history));
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return; };
    let _ = tray.set_icon(Some(metric_icon(metrics[0])));
    let _ = tray.set_icon_as_template(true);
    let _ = tray.set_title(Some(&titles[0]));
    let _ = tray.set_tooltip(Some(&tooltip));
    if let Ok(menu) = tray_menu(app, Some(state), metrics[0], Some(&history)) { let _ = tray.set_menu(Some(menu)); }

    for (metric, id) in METRIC_TRAYS {
        if let Some(index) = metrics.iter().position(|selected| *selected == metric).filter(|index| *index > 0) {
            if let Some(metric_tray) = app.tray_by_id(id) {
                let _ = metric_tray.set_icon(Some(metric_icon(metric)));
                let _ = metric_tray.set_icon_as_template(true);
                let _ = metric_tray.set_title(Some(&titles[index]));
                let _ = metric_tray.set_tooltip(Some(&tooltip));
                if let Ok(menu) = tray_menu(app, Some(state), metric, Some(&history)) { let _ = metric_tray.set_menu(Some(menu)); }
            } else {
                let _ = build_metric_tray(app, Some(state), id, metric, &titles[index], &tooltip, Some(&history));
            }
        } else {
            drop(app.remove_tray_by_id(id));
        }
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(TRAY_ID).is_some() { return Ok(()); }
    let menu = tray_menu(app, None, MenuBarMetric::TotalLines, None)?;
    app.on_menu_event(|app, event| match event.id.as_ref() {
        "show" => show_window(app),
        "quit" => app.exit(0),
        id if id.starts_with(GITHUB_MENU_PREFIX) => {
            let url = &id[GITHUB_MENU_PREFIX.len()..];
            if url.starts_with("https://github.com/") { let _ = open::that(url); }
        }
        _ => {}
    });
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(metric_icon(MenuBarMetric::TotalLines))
        .icon_as_template(true)
        .title("CodeTally")
        .tooltip("CodeTally")
        .menu(&menu)
        .build(app)?;
    let app = app.clone();
    let state = app.state::<AppState>().inner().clone();
    if crate::screenshot_mode() {
        refresh_menu(&app, &state);
        return Ok(());
    }
    std::thread::spawn(move || {
        refresh_menu(&app, &state);
        let mut last_repo_attempt = Instant::now();
        let mut last_personal_attempt = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let db = state.database();
            let Ok(settings) = sync::app_settings(&db) else { continue; };
            let repo_due = refresh_due(last_repo_attempt.elapsed(), settings.activity_refresh_minutes);
            let personal_due = refresh_due(last_personal_attempt.elapsed(), settings.personal_refresh_minutes);
            if !repo_due && !personal_due { continue; }
            // Never queue an automatic refresh behind an active manual job.
            let Ok(_job) = state.job_lock.try_lock() else {
                continue;
            };
            // First import remains an explicit user action.
            if !db.repositories().is_ok_and(|repos| !repos.is_empty()) {
                last_repo_attempt = Instant::now();
                last_personal_attempt = Instant::now();
                continue;
            }
            // One scheduler coordinates the broad repository pass and the fast
            // personal search; neither runs behind a manual job.
            let outcome = if repo_due { sync::sync_activity(&state) } else { sync::sync_personal_work_items(&state) };
            if let Err(error) = &outcome {
                let mut progress = state.progress();
                progress.running = false;
                progress.error = Some(error.to_string());
                state.set_progress(progress);
            }
            if repo_due {
                last_repo_attempt = Instant::now();
                if outcome.as_ref().is_ok_and(|result| result.ok) { last_personal_attempt = Instant::now(); }
            } else {
                last_personal_attempt = Instant::now();
            }
            drop(_job);
            refresh_menu(&app, &state);
            if repo_due {
                let _ = app.emit("background-sync-completed", ());
            } else {
                let _ = app.emit("background-personal-sync-completed", serde_json::json!({
                    "refreshed_at": db.metadata(sync::LAST_PERSONAL_REFRESH_METADATA_KEY).ok().flatten(),
                    "complete": outcome.as_ref().is_ok_and(|result| result.ok),
                }));
            }
        }
    });
    Ok(())
}

fn refresh_due(elapsed: Duration, interval_minutes: i64) -> bool {
    elapsed >= Duration::from_secs(interval_minutes.max(1) as u64 * 60)
}

#[cfg(test)]
mod tests {
    use super::{hides_dock_icon, loc_sparkline, refresh_due};
    use crate::models::{AppSettings, HistoryPoint, MenuBarMetric};
    use chrono::NaiveDate;
    use std::time::Duration;

    #[test]
    fn dock_icon_is_hidden_only_when_background_and_menu_bar_are_enabled() {
        let mut settings = AppSettings::default();
        assert!(hides_dock_icon(&settings));

        settings.run_in_background = false;
        assert!(!hides_dock_icon(&settings));

        settings.run_in_background = true;
        settings.show_menu_bar = false;
        assert!(!hides_dock_icon(&settings));
    }

    #[test]
    fn personal_search_and_repo_refresh_have_independent_due_times() {
        let settings = AppSettings::default();
        assert!(!refresh_due(Duration::from_secs(4 * 60), settings.personal_refresh_minutes));
        assert!(refresh_due(Duration::from_secs(5 * 60), settings.personal_refresh_minutes));
        assert!(!refresh_due(Duration::from_secs(5 * 60), settings.activity_refresh_minutes));
        assert!(refresh_due(Duration::from_secs(24 * 60 * 60), settings.activity_refresh_minutes));
    }

    #[test]
    fn loc_trend_uses_cached_daily_values_for_the_selected_metric() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 28).unwrap();
        let points = vec![
            HistoryPoint { snapshot_date: "2026-09-20".into(), total_loc: 100, source_loc: 80, test_loc: 20 },
            HistoryPoint { snapshot_date: "2026-09-27".into(), total_loc: 200, source_loc: 80, test_loc: 120 },
        ];
        let (total, recorded) = loc_sparkline(&points, MenuBarMetric::TotalLines, today).unwrap();
        assert_eq!(total.chars().count(), 30);
        assert!(total.starts_with("····"));
        assert!(total.ends_with("██"));
        assert_eq!(recorded, "2026-09-27");
        let (source, _) = loc_sparkline(&points, MenuBarMetric::SourceLines, today).unwrap();
        assert!(source.ends_with("▄▄"));
        assert_ne!(total, source);
        assert!(loc_sparkline(&[], MenuBarMetric::TestLines, today).is_none());
    }
}
