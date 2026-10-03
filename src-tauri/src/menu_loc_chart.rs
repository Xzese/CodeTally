//! A measured-day LOC chart embedded in the existing macOS NSMenu.
//! Tauri's text items cannot show a readable graph; AppKit allows an NSView
//! on one menu row while leaving the other menu actions native.
use crate::models::{HistoryPoint, LocChartRange, MenuBarMetric};
use chrono::{Duration, Months, NaiveDate};
use objc2::{AnyThread, MainThreadMarker, MainThreadOnly};
use objc2_app_kit::{NSImage, NSImageView, NSView};
use objc2_foundation::{NSData, NSPoint, NSRect, NSSize, NSString};
use tauri::tray::TrayIcon;

const DAYS: usize = 30;
const LONG_RANGE_BINS: usize = 48;
const WIDTH: usize = 600;
const HEIGHT: usize = 144;
const DISPLAY_WIDTH: f64 = 300.0;
const DISPLAY_HEIGHT: f64 = 72.0;
const CHART_ROW_HORIZONTAL_INSET: f64 = 16.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartData {
    pub values: Vec<Option<i64>>,
    pub first: Option<(NaiveDate, i64)>,
    pub latest: Option<(NaiveDate, i64)>,
    pub measured_days: usize,
}

fn value(point: &HistoryPoint, metric: MenuBarMetric) -> i64 {
    match metric {
        MenuBarMetric::TotalLines => point.total_loc,
        MenuBarMetric::SourceLines => point.source_loc,
        MenuBarMetric::TestLines => point.test_loc,
        _ => 0,
    }
}

pub fn chart_data(history: &[HistoryPoint], metric: MenuBarMetric, range: LocChartRange, today: NaiveDate) -> ChartData {
    let earliest = history.iter().filter_map(|point| NaiveDate::parse_from_str(&point.snapshot_date, "%Y-%m-%d").ok()).min();
    let (start, bins) = match range {
        LocChartRange::ThirtyDays => (today - Duration::days((DAYS - 1) as i64), DAYS),
        LocChartRange::ThreeMonths => (today.checked_sub_months(Months::new(3)).unwrap_or(today), LONG_RANGE_BINS),
        LocChartRange::OneYear => (today.checked_sub_months(Months::new(12)).unwrap_or(today), LONG_RANGE_BINS),
        LocChartRange::ThreeYears => (today.checked_sub_months(Months::new(36)).unwrap_or(today), LONG_RANGE_BINS),
        LocChartRange::All => (earliest.unwrap_or(today), LONG_RANGE_BINS),
    };
    let mut values = vec![None; bins];
    let mut binned_dates = vec![None; bins];
    let mut first = None;
    let mut latest = None;
    let mut measured_days = std::collections::BTreeSet::new();
    let span = (today - start).num_days().max(1);
    for point in history {
        let Ok(date) = NaiveDate::parse_from_str(&point.snapshot_date, "%Y-%m-%d") else {
            continue;
        };
        if date < start || date > today {
            continue;
        }
        let sample = value(point, metric);
        let index = ((date - start).num_days() * (bins - 1) as i64 / span) as usize;
        // Use the latest observation in a bin. Keep distinct observed dates
        // for the summary; empty bins do not imply zero lines.
        if binned_dates[index].is_none_or(|existing_date| date >= existing_date) {
            binned_dates[index] = Some(date);
            values[index] = Some(sample);
        }
        measured_days.insert(date);
        if first.is_none_or(|(first_date, _)| date < first_date) { first = Some((date, sample)); }
        if latest.is_none_or(|(last_date, _)| date >= last_date) { latest = Some((date, sample)); }
    }
    ChartData {
        values,
        first,
        latest,
        measured_days: measured_days.len(),
    }
}

fn rectangle(pixels: &mut [u8], x: usize, y: usize, width: usize, height: usize, color: [u8; 4]) {
    for row in y..(y + height).min(HEIGHT) {
        for col in x..(x + width).min(WIDTH) {
            pixels[(row * WIDTH + col) * 4..(row * WIDTH + col + 1) * 4].copy_from_slice(&color);
        }
    }
}

fn stroke_segment(pixels: &mut [u8], from: (usize, usize), to: (usize, usize)) {
    let dx = to.0 as i64 - from.0 as i64;
    let dy = to.1 as i64 - from.1 as i64;
    let steps = dx.abs().max(dy.abs()).max(1) as usize;
    for step in 0..=steps {
        let x = (from.0 as f64 + dx as f64 * step as f64 / steps as f64).round() as usize;
        let y = (from.1 as f64 + dy as f64 * step as f64 / steps as f64).round() as usize;
        rectangle(pixels, x.saturating_sub(1), y.saturating_sub(1), 3, 3, [72, 181, 216, 240]);
    }
}

fn chart_png(data: &ChartData) -> Option<Vec<u8>> {
    let samples: Vec<_> = data.values.iter().flatten().copied().collect();
    let minimum = *samples.iter().min()?;
    let maximum = *samples.iter().max()?;
    let mut pixels = vec![0u8; WIDTH * HEIGHT * 4];
    for y in [20, 70, 126] {
        for x in (8..WIDTH - 8).step_by(8) {
            rectangle(&mut pixels, x, y, 3, 1, [119, 145, 168, 75]);
        }
    }
    // The selected range filters saved days; draw from the first measured bin
    // to the last so missing days at either edge do not push the line aside.
    let first_index = data.values.iter().position(Option::is_some)?;
    let last_index = data.values.iter().rposition(Option::is_some)?;
    let points: Vec<_> = data.values.iter().enumerate().filter_map(|(index, sample)| {
        let sample = (*sample)?;
        let x = if first_index == last_index { WIDTH / 2 } else {
            12 + ((index - first_index) as f64 * (WIDTH - 24) as f64 / (last_index - first_index) as f64).round() as usize
        };
        // Scale to the observed range so small changes remain visible.
        let y = if minimum == maximum { 72 } else {
            118 - (((sample - minimum) as f64 / (maximum - minimum) as f64) * 96.0).round() as usize
        };
        Some((index, x, y))
    }).collect();
    for pair in points.windows(2) {
        let from = pair[0];
        let to = pair[1];
        stroke_segment(&mut pixels, (from.1, from.2), (to.1, to.2));
    }
    for (index, (_, x, y)) in points.iter().enumerate() {
        let latest = index + 1 == points.len();
        let radius = if latest { 4 } else { 3 };
        rectangle(&mut pixels, x.saturating_sub(radius), y.saturating_sub(radius), radius * 2 + 1, radius * 2 + 1,
            if latest { [55, 163, 226, 255] } else { [89, 185, 205, 245] });
    }
    let mut output = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut output, WIDTH as u32, HEIGHT as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&pixels).ok()?;
    }
    Some(output)
}

/// Replace the named chart placeholder with an image
/// view. `with_inner_tray_icon` runs this closure on AppKit's main thread.
pub fn attach(tray: &TrayIcon<tauri::Wry>, data: &ChartData) {
    let Some(bytes) = chart_png(data) else {
        return;
    };
    let _ = tray.with_inner_tray_icon(move |inner| {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Some(status_item) = inner.ns_status_item() else {
            return;
        };
        let Some(menu) = status_item.menu(mtm) else {
            return;
        };
        menu.update();
        let Some(menu_item) = menu.itemWithTitle(&NSString::from_str("LOC chart")) else {
            return;
        };
        let Some(image) = NSImage::initWithData(NSImage::alloc(), &NSData::with_bytes(&bytes))
        else {
            return;
        };
        image.setSize(NSSize::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
        let image_view = NSImageView::imageViewWithImage(&image, mtm);
        // Give the chart row the menu's measured content width so it follows
        // wider summary/status text, then center the fixed-size chart inside it.
        let row_width = menu.size().width.max(DISPLAY_WIDTH + 2.0 * CHART_ROW_HORIZONTAL_INSET);
        let row = NSView::initWithFrame(
            NSView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(row_width, DISPLAY_HEIGHT)),
        );
        image_view.setFrameOrigin(NSPoint::new((row_width - DISPLAY_WIDTH) / 2.0, 0.0));
        image_view.setFrameSize(NSSize::new(DISPLAY_WIDTH, DISPLAY_HEIGHT));
        row.addSubview(&image_view);
        menu_item.setView(Some(&row));
    });
}

#[cfg(test)]
mod tests {
    use super::{chart_data, chart_png, ChartData, WIDTH};
    use crate::models::{HistoryPoint, LocChartRange, MenuBarMetric};
    use chrono::NaiveDate;

    #[test]
    fn rendered_line_centers_sparse_history_and_single_samples() {
        for single_sample in [false, true] {
            let mut values = vec![None; 30];
            values[25] = Some(120);
            if !single_sample { values[8] = Some(100); }
            let bytes = chart_png(&ChartData { values, first: None, latest: None, measured_days: if single_sample { 1 } else { 2 } }).unwrap();
            let mut reader = png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
            let mut pixels = vec![0; reader.output_buffer_size()];
            let frame = reader.next_frame(&mut pixels).unwrap();
            let ink: Vec<_> = pixels[..frame.buffer_size()].chunks_exact(4).enumerate()
                .filter(|(_, rgba)| rgba[0] < 100 && rgba[1] > 100 && rgba[2] > 100 && rgba[3] > 200)
                .map(|(index, _)| index % WIDTH).collect();
            let left = *ink.iter().min().unwrap();
            let right = *ink.iter().max().unwrap();
            assert!(left.abs_diff(WIDTH - 1 - right) <= 3, "unequal plot margins: {left}..{right}");
            if single_sample {
                assert!(left >= WIDTH / 2 - 5 && right <= WIDTH / 2 + 5);
            } else {
                assert!(left < 20 && right > WIDTH - 20, "measured days should use the plot width");
            }
        }
    }

    #[test]
    fn chart_keeps_only_measured_days_and_selects_the_requested_metric() {
        let today = NaiveDate::from_ymd_opt(2026, 9, 29).unwrap();
        let history = vec![
            HistoryPoint {
                snapshot_date: "2026-08-30".into(),
                total_loc: 10,
                source_loc: 8,
                test_loc: 2,
            },
            HistoryPoint {
                snapshot_date: "2026-09-01".into(),
                total_loc: 100,
                source_loc: 80,
                test_loc: 20,
            },
            HistoryPoint {
                snapshot_date: "2026-09-28".into(),
                total_loc: 120,
                source_loc: 95,
                test_loc: 25,
            },
        ];
        let total = chart_data(&history, MenuBarMetric::TotalLines, LocChartRange::ThirtyDays, today);
        assert_eq!(total.measured_days, 2);
        assert_eq!(total.values[1], Some(100));
        assert_eq!(total.values[2], None);
        assert_eq!(total.values[28], Some(120));
        assert_eq!(total.values[29], None);
        assert_eq!(total.first.map(|(_, value)| value), Some(100));
        assert_eq!(total.latest.map(|(_, value)| value), Some(120));
        let tests = chart_data(&history, MenuBarMetric::TestLines, LocChartRange::ThirtyDays, today);
        assert_eq!(tests.latest.map(|(_, value)| value), Some(25));
        let all = chart_data(&history, MenuBarMetric::TotalLines, LocChartRange::All, today);
        assert_eq!(all.measured_days, 3);
        assert_eq!(all.first, Some((NaiveDate::from_ymd_opt(2026, 8, 30).unwrap(), 10)));
        assert_eq!(all.values.len(), 48);
        let three_months = chart_data(&history, MenuBarMetric::TotalLines, LocChartRange::ThreeMonths, today);
        assert_eq!(three_months.measured_days, 3);
        assert_eq!(three_months.first.map(|(_, value)| value), Some(10));
    }
}
