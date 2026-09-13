//! Native lifecycle and scheduling remain active when the dashboard is hidden.
use crate::models::{DashboardTotals, MenuBarMetric};
use crate::sync::{self, AppState};
use std::time::{Duration, Instant};
use tauri::{menu::{Menu, MenuBuilder}, tray::TrayIconBuilder, AppHandle, Emitter, Manager};

const TRAY_ID: &str = "codetally";
const GITHUB_MENU_PREFIX: &str = "open-github:";
const METRIC_TRAYS: [(MenuBarMetric, &str); 5] = [
    (MenuBarMetric::TotalLines, "codetally-total-lines"),
    (MenuBarMetric::SourceLines, "codetally-source-lines"),
    (MenuBarMetric::TestLines, "codetally-test-lines"),
    (MenuBarMetric::OpenPrs, "codetally-open-prs"),
    (MenuBarMetric::OpenIssues, "codetally-open-issues"),
];

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

fn tray_menu(app: &AppHandle, state: Option<&AppState>, metric: MenuBarMetric) -> tauri::Result<Menu<tauri::Wry>> {
    let mut builder = MenuBuilder::new(app);
    let mut activity_count = 0;
    if let Some(state) = state {
        let db = state.database();
        match metric {
            MenuBarMetric::OpenPrs => {
                for item in db.pull_requests(None, Some("open"), 12).unwrap_or_default() {
                    activity_count += 1;
                    builder = builder.text(format!("{GITHUB_MENU_PREFIX}{}", item.url), menu_label(&item.repository, item.number, &item.title));
                }
            }
            MenuBarMetric::OpenIssues => {
                for item in db.issues(None, Some("open"), 12).unwrap_or_default() {
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

fn build_metric_tray(app: &AppHandle, state: Option<&AppState>, id: &str, metric: MenuBarMetric, title: &str, tooltip: &str) -> tauri::Result<()> {
    TrayIconBuilder::with_id(id)
        .icon(metric_icon(metric))
        .icon_as_template(true)
        .title(title)
        .tooltip(tooltip)
        .menu(&tray_menu(app, state, metric)?)
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
    let titles = menu_titles(&metrics, &totals);
    let combined_title = titles.join(" · ");
    let tooltip = format!("CodeTally — {combined_title}");
    if app.tray_by_id(TRAY_ID).is_none() {
        let _ = build_metric_tray(app, Some(state), TRAY_ID, metrics[0], &titles[0], &tooltip);
    }
    let Some(tray) = app.tray_by_id(TRAY_ID) else { return; };
    let _ = tray.set_icon(Some(metric_icon(metrics[0])));
    let _ = tray.set_icon_as_template(true);
    let _ = tray.set_title(Some(&titles[0]));
    let _ = tray.set_tooltip(Some(&tooltip));
    if let Ok(menu) = tray_menu(app, Some(state), metrics[0]) { let _ = tray.set_menu(Some(menu)); }

    for (metric, id) in METRIC_TRAYS {
        if let Some(index) = metrics.iter().position(|selected| *selected == metric).filter(|index| *index > 0) {
            if let Some(metric_tray) = app.tray_by_id(id) {
                let _ = metric_tray.set_icon(Some(metric_icon(metric)));
                let _ = metric_tray.set_icon_as_template(true);
                let _ = metric_tray.set_title(Some(&titles[index]));
                let _ = metric_tray.set_tooltip(Some(&tooltip));
                if let Ok(menu) = tray_menu(app, Some(state), metric) { let _ = metric_tray.set_menu(Some(menu)); }
            } else {
                let _ = build_metric_tray(app, Some(state), id, metric, &titles[index], &tooltip);
            }
        } else {
            drop(app.remove_tray_by_id(id));
        }
    }
}

pub fn setup(app: &AppHandle) -> tauri::Result<()> {
    if app.tray_by_id(TRAY_ID).is_some() { return Ok(()); }
    let menu = tray_menu(app, None, MenuBarMetric::TotalLines)?;
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
    std::thread::spawn(move || {
        refresh_menu(&app, &state);
        let mut last_attempt = Instant::now();
        loop {
            std::thread::sleep(Duration::from_secs(5));
            let db = state.database();
            let Ok(settings) = sync::app_settings(&db) else { continue; };
            if last_attempt.elapsed() < Duration::from_secs(settings.activity_refresh_minutes.max(1) as u64 * 60) { continue; }
            // Never queue an automatic refresh behind an active manual job.
            let Ok(_job) = state.job_lock.try_lock() else {
                last_attempt = Instant::now();
                continue;
            };
            last_attempt = Instant::now();
            // First import remains an explicit user action.
            if !db.repositories().is_ok_and(|repos| !repos.is_empty()) { continue; }
            // Existing activity sync owns discovery cadence, LOC sweeps and persisted rate-limit pauses.
            if let Err(error) = sync::sync_activity(&state) {
                let mut progress = state.progress();
                progress.running = false;
                progress.error = Some(error.to_string());
                state.set_progress(progress);
            }
            drop(_job);
            refresh_menu(&app, &state);
            let _ = app.emit("background-sync-completed", ());
        }
    });
    Ok(())
}
