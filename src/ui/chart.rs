use std::collections::BTreeMap;

use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::symbols;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType};
use ratatui::Frame;

use crate::app::AppState;
use crate::ui::palette;

pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Evolution ");

    if state.series.is_empty() {
        f.render_widget(block, area);
        return;
    }

    // Bucket series by group label.
    let mut by_group: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_max: f64 = 0.0;

    for p in &state.series {
        let x = p.bucket as f64;
        let y = p.value as f64;
        by_group.entry(p.group.clone()).or_default().push((x, y));
        x_min = x_min.min(x);
        x_max = x_max.max(x);
        y_max = y_max.max(y);
    }

    // Own the point vecs so Dataset can borrow them.
    let points: Vec<(String, Vec<(f64, f64)>)> = by_group.into_iter().collect();

    let datasets: Vec<Dataset> = points
        .iter()
        .map(|(name, pts)| {
            Dataset::default()
                .name(name.clone())
                .marker(symbols::Marker::Braille)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(palette::color_for(name)))
                .data(pts)
        })
        .collect();

    let chart = Chart::new(datasets)
        .block(block)
        .x_axis(Axis::default().bounds([x_min, x_max.max(x_min + 1.0)]))
        .y_axis(Axis::default().bounds([0.0, y_max.max(1.0)]));

    f.render_widget(chart, area);
}
