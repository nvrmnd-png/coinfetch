
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::Marker;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Block, Chart, Dataset, GraphType, Paragraph, Widget};

use crate::api::coingecko::MarketData;
use crate::config::ChartRender;
use crate::model::{self, ChartData};

pub const CHART_HEIGHT: u16 = 16;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ChartStyle {

    pub render: ChartRender,

    pub minimal: bool,
}

impl ChartStyle {

    fn defers_to_image(self) -> bool {
        self.render == ChartRender::Lines
    }
}

pub struct LegendRow {
    pub label: String,
    pub color: Color,
    pub price: Option<f64>,
    pub change_pct: f64,
}

pub struct PriceView {
    pub chart: Option<ChartData>,
    pub style: ChartStyle,
    pub legend: Vec<LegendRow>,
    pub notes: Vec<String>,
}

pub fn build(data: &MarketData, colors: &[Color], style: ChartStyle) -> PriceView {
    let normalized = model::normalize(&data.series);
    let mut legend = Vec::new();
    let mut notes = Vec::new();

    if let Some(chart) = &normalized.chart {
        for (index, series) in chart.series.iter().enumerate() {
            legend.push(LegendRow {
                label: series.id.clone(),
                color: colors[index % colors.len()],
                price: data.quote(&series.id).map(|q| q.price),
                change_pct: series.last_value,
            });
        }
    }

    for (id, err) in &data.failed {
        notes.push(format!("{id}: {err}, skipped"));
    }
    for (id, err) in &normalized.skipped {
        notes.push(format!("{id}: {err}, skipped"));
    }
    notes.extend(data.notes.iter().cloned());

    PriceView {
        chart: normalized.chart,
        style,
        legend,
        notes,
    }
}

impl PriceView {

    pub fn has_chart(&self) -> bool {
        self.chart.is_some()
    }

    pub fn height(&self) -> u16 {
        let chart = if self.chart.is_some() {
            CHART_HEIGHT
        } else {
            0
        };

        if self.style.minimal && self.chart.is_some() {
            return chart;
        }
        if self.style.minimal {
            return self.notes.len() as u16;
        }
        chart + self.legend.len() as u16 + self.notes.len() as u16
    }

    fn slices(&self, area: Rect) -> [Rect; 3] {
        let chart_height = if self.chart.is_some() {
            CHART_HEIGHT.min(area.height)
        } else {
            0
        };
        Layout::vertical([
            Constraint::Length(chart_height),
            Constraint::Length(self.legend.len() as u16),
            Constraint::Length(self.notes.len() as u16),
        ])
        .areas(area)
    }

    pub fn image_area(&self, area: Rect) -> Option<Rect> {
        if !self.style.defers_to_image() {
            return None;
        }
        let data = self.chart.as_ref()?;

        let chart_area = if self.style.minimal {
            area
        } else {
            self.slices(area)[0]
        };

        let rect = plot_area(chart_area, self.style, &y_labels(data));
        (rect.width > 0 && rect.height > 0).then_some(rect)
    }

    pub fn series_colors(&self) -> Vec<Color> {
        self.legend.iter().map(|row| row.color).collect()
    }

    fn legend_lines(&self) -> Vec<Line<'_>> {
        let name_width = self
            .legend
            .iter()
            .map(|r| r.label.chars().count())
            .max()
            .unwrap_or(0);

        self.legend
            .iter()
            .map(|row| {
                let price = row
                    .price
                    .map(model::format_price)
                    .unwrap_or_else(|| "—".to_string());
                let change = model::format_pct(row.change_pct);
                let change_color = if !row.change_pct.is_finite() {
                    Color::DarkGray
                } else if row.change_pct >= 0.0 {
                    Color::Green
                } else {
                    Color::Red
                };

                Line::from(vec![
                    Span::styled("● ", Style::default().fg(row.color)),
                    Span::raw(format!("{:<width$}", row.label, width = name_width)),
                    Span::raw(format!("{price:>14}")),
                    Span::styled(format!("{change:>10}"), Style::default().fg(change_color)),
                ])
            })
            .collect()
    }

    fn note_lines(&self) -> Vec<Line<'_>> {
        self.notes
            .iter()
            .map(|note| {
                Line::from(Span::styled(
                    format!("! {note}"),
                    Style::default().fg(Color::Yellow),
                ))
            })
            .collect()
    }

    pub fn plain_text(&self) -> String {
        let mut out = String::new();
        if self.chart.is_some() {
            out.push_str("7d change, percent since the start of the window\n");
        }

        let name_width = self
            .legend
            .iter()
            .map(|r| r.label.chars().count())
            .max()
            .unwrap_or(0);

        for row in &self.legend {
            let price = row
                .price
                .map(model::format_price)
                .unwrap_or_else(|| "—".to_string());
            out.push_str(&format!(
                "{:<width$}  {:>14}  {:>9}\n",
                row.label,
                price,
                model::format_pct(row.change_pct),
                width = name_width
            ));
        }
        for note in &self.notes {
            out.push_str(&format!("! {note}\n"));
        }
        out.trim_end().to_string()
    }
}

impl Widget for &PriceView {
    fn render(self, area: Rect, buf: &mut Buffer) {

        if self.style.minimal {
            match &self.chart {
                Some(data) => render_chart(data, &self.legend, self.style, area, buf),

                None => Paragraph::new(self.note_lines()).render(area, buf),
            }
            return;
        }

        let [chart_area, legend_area, notes_area] = self.slices(area);

        if let Some(data) = &self.chart {
            render_chart(data, &self.legend, self.style, chart_area, buf);
        }

        Paragraph::new(self.legend_lines()).render(legend_area, buf);
        Paragraph::new(self.note_lines()).render(notes_area, buf);
    }
}

fn render_chart(
    data: &ChartData,
    legend: &[LegendRow],
    style: ChartStyle,
    area: Rect,
    buf: &mut Buffer,
) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    let [x_min, x_max] = data.x_bounds;

    let axis_style = Style::default().fg(Color::DarkGray);
    let y_labels = y_labels(data);

    let mut x_axis = Axis::default().style(axis_style).bounds(data.x_bounds);
    let mut y_axis = Axis::default().style(axis_style).bounds(data.y_bounds);
    let mut block = Block::bordered().border_style(axis_style);

    if !style.minimal {
        x_axis = x_axis.labels(vec![
            model::format_day(x_min),
            model::format_day((x_min + x_max) / 2.0),
            model::format_day(x_max),
        ]);
        y_axis = y_axis.labels(y_labels.to_vec());
        block = block.title(Span::styled(
            " 7d — % change since day 1 ",
            Style::default().add_modifier(Modifier::BOLD),
        ));
    }

    let datasets: Vec<Dataset<'_>> = match style.render {
        ChartRender::Steps | ChartRender::Lines => Vec::new(),

        ChartRender::Blocks | ChartRender::Dots => {
            let (graph_type, marker) = match style.render {
                ChartRender::Blocks => (GraphType::Line, Marker::HalfBlock),
                _ => (GraphType::Scatter, Marker::Braille),
            };
            data.series
                .iter()
                .enumerate()
                .map(|(index, series)| {
                    Dataset::default()
                        .marker(marker)
                        .graph_type(graph_type)
                        .style(Style::default().fg(series_color(legend, index)))
                        .data(&series.points)
                })
                .collect()
        }
    };

    Chart::new(datasets)
        .block(block)
        .x_axis(x_axis)
        .y_axis(y_axis)
        .render(area, buf);

    if style.render == ChartRender::Steps {
        draw_step_plot(data, legend, plot_area(area, style, &y_labels), buf);
    }
}

fn y_labels(data: &ChartData) -> [String; 3] {
    let [y_min, y_max] = data.y_bounds;
    let y_mid = (y_min + y_max) / 2.0;
    [
        format!("{y_min:+.1}%"),
        format!("{y_mid:+.1}%"),
        format!("{y_max:+.1}%"),
    ]
}

fn series_color(legend: &[LegendRow], index: usize) -> Color {
    legend
        .get(index)
        .map(|row| row.color)
        .unwrap_or(Color::White)
}

fn plot_area(area: Rect, style: ChartStyle, y_labels: &[String]) -> Rect {
    let inner = Block::bordered().inner(area);

    if style.minimal {
        return inner;
    }

    let gutter = y_labels
        .iter()
        .map(|label| label.chars().count() as u16)
        .max()
        .unwrap_or(0)
        + 1;

    Rect {
        x: inner.x + gutter.min(inner.width),
        y: inner.y,
        width: inner.width.saturating_sub(gutter),

        height: inner.height.saturating_sub(2).max(inner.height.min(1)),
    }
}

const LINE_FLAT: char = '─';
const LINE_DROP: char = '│';
const TURN_DOWN_IN: char = '┐';
const TURN_DOWN_OUT: char = '└';
const TURN_UP_IN: char = '┘';
const TURN_UP_OUT: char = '┌';

fn draw_step_plot(data: &ChartData, legend: &[LegendRow], area: Rect, buf: &mut Buffer) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    for (index, series) in data.series.iter().enumerate() {
        let color = series_color(legend, index);
        let rows = series_rows(&series.points, data, area);

        let mut previous: Option<u16> = None;
        for (column, row) in rows.iter().enumerate() {
            let x = area.x + column as u16;
            match previous {
                Some(prev) if prev != *row => {
                    let falling = *row > prev;
                    let (turn_in, turn_out) = if falling {
                        (TURN_DOWN_IN, TURN_DOWN_OUT)
                    } else {
                        (TURN_UP_IN, TURN_UP_OUT)
                    };
                    put(buf, x, prev, turn_in, color);
                    put(buf, x, *row, turn_out, color);

                    for y in (prev.min(*row) + 1)..prev.max(*row) {
                        put(buf, x, y, LINE_DROP, color);
                    }
                }

                _ => put(buf, x, *row, LINE_FLAT, color),
            }
            previous = Some(*row);
        }
    }
}

fn series_rows(points: &[(f64, f64)], data: &ChartData, area: Rect) -> Vec<u16> {
    let [x_min, x_max] = data.x_bounds;
    let [y_min, y_max] = data.y_bounds;
    let x_span = x_max - x_min;
    let y_span = y_max - y_min;
    let last_column = f64::from(area.width.saturating_sub(1)).max(1.0);
    let last_row = f64::from(area.height - 1);

    (0..area.width)
        .map(|column| {
            let x = x_min + x_span * f64::from(column) / last_column;

            let height = if y_span > 0.0 {
                (sample(points, x) - y_min) / y_span
            } else {
                1.0
            };

            let offset = ((1.0 - height) * f64::from(area.height))
                .floor()
                .clamp(0.0, last_row);
            area.y + offset as u16
        })
        .collect()
}

fn sample(points: &[(f64, f64)], x: f64) -> f64 {
    match points.iter().position(|(px, _)| *px >= x) {
        None => points.last().map(|(_, y)| *y).unwrap_or(0.0),
        Some(0) => points[0].1,
        Some(after) => {
            let (x0, y0) = points[after - 1];
            let (x1, y1) = points[after];
            if x1 == x0 {
                y1
            } else {
                y0 + (y1 - y0) * (x - x0) / (x1 - x0)
            }
        }
    }
}

fn put(buf: &mut Buffer, x: u16, y: u16, glyph: char, color: Color) {
    if let Some(cell) = buf.cell_mut((x, y)) {
        cell.set_char(glyph);
        cell.set_fg(color);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::coingecko::MarketData;
    use crate::error::CoinError;
    use crate::model::{Quote, RawSeries};
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn sample_data() -> MarketData {
        MarketData {
            series: vec![
                RawSeries {
                    id: "bitcoin".into(),
                    points: vec![(1_000.0, 64_000.0), (2_000.0, 66_560.0)],
                },
                RawSeries {
                    id: "solana".into(),
                    points: vec![(1_000.0, 76.0), (2_000.0, 72.2)],
                },
            ],
            quotes: vec![
                Quote {
                    id: "bitcoin".into(),
                    symbol: "BTC".into(),
                    price: 66_560.0,
                    change_24h: Some(1.2),
                },
                Quote {
                    id: "solana".into(),
                    symbol: "SOL".into(),
                    price: 72.2,
                    change_24h: Some(-3.0),
                },
            ],
            failed: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn render_to_string(view: &PriceView, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| frame.render_widget(view, frame.area()))
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        (0..buffer.area.height)
            .map(|y| {
                (0..buffer.area.width)
                    .map(|x| buffer[(x, y)].symbol().to_string())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn assigns_a_distinct_palette_color_per_coin() {
        let colors = [Color::Cyan, Color::Magenta];
        let view = build(&sample_data(), &colors, ChartStyle::default());

        assert_eq!(view.legend.len(), 2);
        assert_eq!(view.legend[0].color, Color::Cyan);
        assert_eq!(view.legend[1].color, Color::Magenta);
    }

    #[test]
    fn cycles_the_palette_when_there_are_more_coins_than_colors() {
        let colors = [Color::Cyan];
        let view = build(&sample_data(), &colors, ChartStyle::default());
        assert_eq!(view.legend[0].color, Color::Cyan);
        assert_eq!(view.legend[1].color, Color::Cyan);
    }

    #[test]
    fn draws_the_chart_and_a_legend_row_per_coin() {
        let colors = [Color::Cyan, Color::Magenta];
        let view = build(&sample_data(), &colors, ChartStyle::default());
        let rendered = render_to_string(&view, 70, view.height());

        assert!(rendered.contains("bitcoin"), "{rendered}");
        assert!(rendered.contains("solana"), "{rendered}");
        assert!(rendered.contains("$66,560.00"), "{rendered}");
        assert!(rendered.contains("7d"), "{rendered}");

        assert!(rendered.contains("+4.00%"), "{rendered}");
        assert!(rendered.contains("-5.00%"), "{rendered}");
    }

    #[test]
    fn surfaces_a_failed_coin_as_a_note_while_still_charting_the_rest() {
        let mut data = sample_data();
        data.failed
            .push(("dogecoin".into(), CoinError::RateLimited));

        let view = build(&data, &[Color::Cyan, Color::Magenta], ChartStyle::default());
        assert_eq!(view.legend.len(), 2);
        assert_eq!(view.notes.len(), 1);
        assert!(view.notes[0].contains("dogecoin"), "{:?}", view.notes);

        let rendered = render_to_string(&view, 70, view.height());
        assert!(rendered.contains("dogecoin"), "{rendered}");
        assert!(rendered.contains("bitcoin"), "{rendered}");
    }

    #[test]
    fn produces_a_view_with_no_chart_when_every_coin_failed() {
        let data = MarketData {
            series: Vec::new(),
            quotes: Vec::new(),
            failed: vec![("notacoin123".into(), CoinError::NotFound)],
            notes: Vec::new(),
        };
        let view = build(&data, &[Color::Cyan], ChartStyle::default());

        assert!(!view.has_chart());
        assert!(view.legend.is_empty());
        assert_eq!(view.notes.len(), 1);
    }

    #[test]
    fn plain_text_output_carries_the_same_numbers_without_a_chart() {
        let view = build(
            &sample_data(),
            &[Color::Cyan, Color::Magenta],
            ChartStyle::default(),
        );
        let text = view.plain_text();

        assert!(text.contains("bitcoin"));
        assert!(text.contains("$66,560.00"));
        assert!(text.contains("+4.00%"));
        assert!(!text.contains('│'));
    }

    #[test]
    fn shows_a_dash_when_the_current_price_is_unavailable() {
        let mut data = sample_data();
        data.quotes.clear();
        let view = build(&data, &[Color::Cyan, Color::Magenta], ChartStyle::default());
        assert!(view.plain_text().contains('—'));
    }

    #[test]
    fn joins_the_points_into_a_line_instead_of_plotting_them_as_dots() {

        let view = build(
            &sample_data(),
            &[Color::Cyan, Color::Magenta],
            ChartStyle::default(),
        );
        let rendered = render_to_string(&view, 70, view.height());

        let painted = painted_cells(&rendered);

        assert!(
            painted > 20,
            "expected a drawn line, got {painted} plotted cells:\n{rendered}"
        );
    }

    #[test]
    fn renders_without_panicking_in_a_narrow_terminal() {
        let view = build(
            &sample_data(),
            &[Color::Cyan, Color::Magenta],
            ChartStyle::default(),
        );
        let rendered = render_to_string(&view, 24, view.height());
        assert!(!rendered.is_empty());
    }

    #[test]
    fn height_accounts_for_chart_legend_and_notes() {
        let mut view = build(&sample_data(), &[Color::Cyan], ChartStyle::default());
        assert_eq!(view.height(), CHART_HEIGHT + 2);

        view.notes.push("something".into());
        assert_eq!(view.height(), CHART_HEIGHT + 2 + 1);
    }

    fn is_braille(c: char) -> bool {
        matches!(c, '\u{2801}'..='\u{28FF}')
    }

    fn is_half_block(c: char) -> bool {
        matches!(c, '▀' | '▄' | '█')
    }

    fn is_box_line(c: char) -> bool {
        matches!(c, '─' | '│' | '┌' | '┐' | '└' | '┘')
    }

    fn is_plot_cell(c: char) -> bool {
        is_braille(c) || is_half_block(c) || is_box_line(c)
    }

    fn painted_cells(plot: &str) -> usize {
        plot.chars().filter(|c| is_plot_cell(*c)).count()
    }

    fn styled(render: ChartRender, minimal: bool) -> PriceView {
        build(
            &sample_data(),
            &[Color::Cyan, Color::Magenta],
            ChartStyle { render, minimal },
        )
    }

    fn grid(rendered: &str) -> Vec<Vec<char>> {
        rendered.lines().map(|l| l.chars().collect()).collect()
    }

    fn plot_bounds(rendered: &str, minimal: bool) -> (usize, usize, usize, usize) {
        let rows = grid(rendered);
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        let corner = (!minimal)
            .then(|| {
                rows.iter().enumerate().rev().find_map(|(y, row)| {
                    row.iter()
                        .position(|c| *c == '└')
                        .filter(|x| *x > 0)
                        .map(|x| (x, y))
                })
            })
            .flatten();

        match corner {
            Some((x, y)) => (x + 1, width.saturating_sub(1), 1, y),
            None => (1, width.saturating_sub(1), 1, rows.len().saturating_sub(1)),
        }
    }

    fn plot_text(rendered: &str, minimal: bool) -> String {
        let (x0, x1, y0, y1) = plot_bounds(rendered, minimal);
        grid(rendered)[y0..y1]
            .iter()
            .map(|row| row[x0.min(row.len())..x1.min(row.len())].iter().collect())
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn frame_text(rendered: &str, minimal: bool) -> String {
        let (x0, x1, y0, y1) = plot_bounds(rendered, minimal);
        grid(rendered)
            .iter()
            .enumerate()
            .map(|(y, row)| {
                row.iter()
                    .enumerate()
                    .map(|(x, c)| {
                        let inside = (y0..y1).contains(&y) && (x0..x1).contains(&x);
                        if inside { ' ' } else { *c }
                    })
                    .collect()
            })
            .collect::<Vec<String>>()
            .join("\n")
    }

    fn wavy_data() -> MarketData {
        let series = |id: &str, base: f64, swing: f64, period: f64| RawSeries {
            id: id.into(),
            points: (0..168)
                .map(|hour| {
                    let t = f64::from(hour);
                    (
                        1_700_000_000.0 + t * 3_600.0,
                        base * (1.0 + swing * (t / period).sin()),
                    )
                })
                .collect(),
        };

        MarketData {
            series: vec![
                series("bitcoin", 64_000.0, 0.06, 12.0),
                series("solana", 76.0, 0.09, 20.0),
            ],
            quotes: Vec::new(),
            failed: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn wavy_view(render: ChartRender, minimal: bool) -> PriceView {
        build(
            &wavy_data(),
            &[Color::Cyan, Color::Magenta],
            ChartStyle { render, minimal },
        )
    }

    fn plot_of(render: ChartRender) -> String {
        plot_text(
            &render_to_string(&wavy_view(render, false), 70, CHART_HEIGHT),
            false,
        )
    }

    fn unpainted_columns(plot: &str) -> Vec<usize> {
        let rows = grid(plot);
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        (0..width)
            .filter(|x| {
                !rows
                    .iter()
                    .any(|row| row.get(*x).copied().is_some_and(is_plot_cell))
            })
            .collect()
    }

    #[test]
    fn dots_plot_the_samples_where_lines_join_them_up() {

        let lines = styled(ChartRender::Steps, false);
        let dots = styled(ChartRender::Dots, false);

        let line_cells = painted_cells(&plot_text(
            &render_to_string(&lines, 70, lines.height()),
            false,
        ));
        let dot_cells = painted_cells(&plot_text(
            &render_to_string(&dots, 70, dots.height()),
            false,
        ));

        assert!(dot_cells > 0, "dots must still plot something");
        assert!(
            line_cells > dot_cells * 2,
            "expected a line to paint far more than {dot_cells} scattered cells, got {line_cells}"
        );
    }

    #[test]
    fn a_stroke_is_drawn_with_box_drawing_and_covers_every_column() {

        let plot = plot_of(ChartRender::Steps);

        let stray: Vec<char> = plot
            .chars()
            .filter(|c| *c != ' ' && *c != '\n' && !is_box_line(*c))
            .collect();
        assert!(stray.is_empty(), "unexpected glyphs {stray:?} in:\n{plot}");
        assert!(
            plot.chars().any(|c| matches!(c, '┌' | '┐' | '└' | '┘')),
            "a line that never turns is not being drawn:\n{plot}"
        );

        let blank = unpainted_columns(&plot);
        assert!(blank.is_empty(), "columns {blank:?} are unpainted:\n{plot}");
        let seams = uncrossed_boundaries(&plot);
        assert!(seams.is_empty(), "the stroke breaks at {seams:?}:\n{plot}");
    }

    #[test]
    fn a_step_of_one_row_turns_squarely_like_any_other() {

        let plot = plot_of(ChartRender::Steps);

        let stray: Vec<char> = plot
            .chars()
            .filter(|c| *c != ' ' && *c != '\n' && !is_box_line(*c))
            .collect();
        assert!(
            stray.is_empty(),
            "a step was drawn with {stray:?} instead of a corner:\n{plot}"
        );
        assert!(
            plot.contains('┌') && plot.contains('┐'),
            "the wave turns both ways, so both corners should appear:\n{plot}"
        );
    }

    fn calm_data() -> MarketData {
        MarketData {
            series: vec![RawSeries {
                id: "usd-coin".into(),
                points: (0..168)
                    .map(|hour| {
                        let t = f64::from(hour);

                        let jitter = (t * 2.4).sin() + (t * 0.7).sin();
                        (1_700_000_000.0 + t * 3_600.0, 1.0 + 0.0004 * jitter)
                    })
                    .collect(),
            }],
            quotes: Vec::new(),
            failed: Vec::new(),
            notes: Vec::new(),
        }
    }

    fn touches_left(c: char) -> bool {
        matches!(c, '─' | '┐' | '┘')
    }

    fn touches_right(c: char) -> bool {
        matches!(c, '─' | '┌' | '└')
    }

    fn uncrossed_boundaries(plot: &str) -> Vec<usize> {
        let rows = grid(plot);
        let width = rows.iter().map(Vec::len).max().unwrap_or(0);
        (1..width)
            .filter(|x| {
                !rows.iter().any(|row| {
                    let left = row.get(x - 1).copied().unwrap_or(' ');
                    let right = row.get(*x).copied().unwrap_or(' ');
                    touches_right(left) && touches_left(right)
                })
            })
            .collect()
    }

    #[test]
    fn a_flat_coin_is_drawn_as_one_connected_stroke() {

        let view = build(
            &calm_data(),
            &[Color::Cyan],
            ChartStyle {
                render: ChartRender::Steps,
                minimal: false,
            },
        );
        let plot = plot_text(&render_to_string(&view, 70, CHART_HEIGHT), false);

        let seams = uncrossed_boundaries(&plot);
        assert!(
            seams.is_empty(),
            "the stroke breaks at column boundaries {seams:?}:\n{plot}"
        );
    }

    fn steep_data() -> MarketData {
        MarketData {
            series: vec![RawSeries {
                id: "bitcoin".into(),
                points: (0..168)
                    .map(|hour| {
                        let price = if hour < 84 { 64_000.0 } else { 40_000.0 };
                        (1_700_000_000.0 + f64::from(hour) * 3_600.0, price)
                    })
                    .collect(),
            }],
            quotes: Vec::new(),
            failed: Vec::new(),
            notes: Vec::new(),
        }
    }

    #[test]
    fn a_turn_in_the_line_is_filled_in_rather_than_jumped() {

        let view = build(
            &steep_data(),
            &[Color::Cyan],
            ChartStyle {
                render: ChartRender::Steps,
                minimal: false,
            },
        );
        let plot = plot_text(&render_to_string(&view, 70, CHART_HEIGHT), false);
        let rows = grid(&plot);

        let mut bridged = 0;
        for (y, row) in rows.iter().enumerate() {
            for (x, c) in row.iter().enumerate() {
                if *c != '│' {
                    continue;
                }
                bridged += 1;
                let above = rows.get(y.wrapping_sub(1)).and_then(|r| r.get(x));
                let below = rows.get(y + 1).and_then(|r| r.get(x));
                for neighbour in [above, below].into_iter().flatten() {
                    assert!(
                        is_box_line(*neighbour),
                        "the vertical at {x},{y} runs into `{neighbour}`:\n{plot}"
                    );
                }
            }
        }
        assert!(bridged > 0, "this data should need bridging:\n{plot}");
    }

    fn image_view(minimal: bool) -> PriceView {
        build(
            &wavy_data(),
            &[Color::Cyan, Color::Magenta],
            ChartStyle {
                render: ChartRender::Lines,
                minimal,
            },
        )
    }

    #[test]
    fn the_image_style_leaves_the_plot_empty_and_the_frame_exactly_as_it_was() {

        let rendered = render_to_string(&image_view(false), 70, CHART_HEIGHT);

        let plot = plot_text(&rendered, false);
        assert_eq!(
            painted_cells(&plot),
            0,
            "the cells the image covers should be left empty:\n{plot}"
        );

        let blocks = render_to_string(&wavy_view(ChartRender::Blocks, false), 70, CHART_HEIGHT);
        assert_eq!(
            frame_text(&rendered, false),
            frame_text(&blocks, false),
            "the frame moved when the stroke became an image"
        );
    }

    #[test]
    fn the_image_is_asked_for_exactly_the_cells_the_glyph_stroke_would_fill() {

        let area = Rect::new(0, 0, 70, CHART_HEIGHT);
        let target = image_view(false).image_area(area).expect("a target rect");

        let glyphs = render_to_string(&wavy_view(ChartRender::Steps, false), 70, CHART_HEIGHT);
        let rows = grid(&glyphs);
        let painted: Vec<(u16, u16)> = rows
            .iter()
            .enumerate()
            .flat_map(|(y, row)| {
                row.iter()
                    .enumerate()
                    .filter(|(_, c)| is_box_line(**c))
                    .map(move |(x, _)| (x as u16, y as u16))
            })
            .collect();

        let inside: Vec<(u16, u16)> = painted
            .iter()
            .copied()
            .filter(|(x, y)| {
                *x >= target.x && *x < target.right() && *y >= target.y && *y < target.bottom()
            })
            .collect();
        assert!(!inside.is_empty(), "the glyph stroke drew nothing");

        let left = inside.iter().map(|(x, _)| *x).min().expect("a column");
        let right = inside.iter().map(|(x, _)| *x).max().expect("a column");
        let top = inside.iter().map(|(_, y)| *y).min().expect("a row");
        let bottom = inside.iter().map(|(_, y)| *y).max().expect("a row");

        assert_eq!(
            (left, right + 1, top, bottom + 1),
            (target.x, target.right(), target.y, target.bottom()),
            "the image rect {target:?} is not the rect the stroke fills"
        );
    }

    #[test]
    fn minimal_gives_the_image_the_rows_the_labels_would_have_taken() {

        let area = Rect::new(0, 0, 70, CHART_HEIGHT);
        let full = image_view(false).image_area(area).expect("a rect");
        let minimal = image_view(true).image_area(area).expect("a rect");

        assert!(minimal.width > full.width, "{minimal:?} vs {full:?}");
        assert!(minimal.height > full.height, "{minimal:?} vs {full:?}");
    }

    #[test]
    fn only_the_lines_style_asks_for_an_image() {
        let area = Rect::new(0, 0, 70, CHART_HEIGHT);

        for render in [ChartRender::Steps, ChartRender::Blocks, ChartRender::Dots] {
            let view = build(
                &wavy_data(),
                &[Color::Cyan],
                ChartStyle {
                    render,
                    minimal: false,
                },
            );
            assert!(view.image_area(area).is_none(), "{render:?} asked for one");
        }

        assert!(
            image_view(false).image_area(area).is_some(),
            "lines is the style that is a picture"
        );
    }

    #[test]
    fn steps_draws_its_stroke_whatever_the_terminal_can_do() {

        let plot = plot_of(ChartRender::Steps);
        assert!(painted_cells(&plot) > 0, "the plot came out empty:\n{plot}");
        assert!(
            plot.chars().filter(|c| is_box_line(*c)).count() > 0,
            "steps draws in box-drawing glyphs, not braille or blocks:\n{plot}"
        );

        let empty = plot_text(
            &render_to_string(&image_view(false), 70, CHART_HEIGHT),
            false,
        );
        assert_eq!(painted_cells(&empty), 0, "{empty}");
    }

    #[test]
    fn a_run_with_nothing_to_chart_asks_for_no_image() {
        let data = MarketData {
            series: Vec::new(),
            quotes: Vec::new(),
            failed: vec![("notacoin123".into(), CoinError::NotFound)],
            notes: Vec::new(),
        };
        let view = build(
            &data,
            &[Color::Cyan],
            ChartStyle {
                render: ChartRender::Lines,
                minimal: false,
            },
        );
        assert!(view.image_area(Rect::new(0, 0, 70, CHART_HEIGHT)).is_none());
    }

    #[test]
    fn a_terminal_too_small_to_frame_asks_for_no_image_rather_than_an_empty_one() {

        for (width, height) in [(1, 1), (3, 2), (8, 3), (24, 6), (70, 16)] {
            let area = Rect::new(0, 0, width, height);
            for minimal in [false, true] {
                if let Some(rect) = image_view(minimal).image_area(area) {
                    assert!(rect.width > 0 && rect.height > 0, "{rect:?}");
                    assert!(
                        rect.right() <= area.right() && rect.bottom() <= area.bottom(),
                        "{rect:?} leaves the {width}x{height} view"
                    );
                }
            }
        }
    }

    #[test]
    fn the_lines_are_coloured_the_way_the_legend_is() {

        let view = image_view(false);
        assert_eq!(view.series_colors(), vec![Color::Cyan, Color::Magenta]);
    }

    #[test]
    fn frame_is_identical_across_the_styles() {

        let lines = render_to_string(&wavy_view(ChartRender::Steps, false), 70, CHART_HEIGHT);
        let blocks = render_to_string(&wavy_view(ChartRender::Blocks, false), 70, CHART_HEIGHT);

        assert_eq!(
            frame_text(&lines, false),
            frame_text(&blocks, false),
            "the frame moved between the styles"
        );
        assert_ne!(
            plot_text(&lines, false),
            plot_text(&blocks, false),
            "the two styles should not draw the same plot"
        );
    }

    #[test]
    fn blocks_draw_the_line_in_solid_cells() {

        let plot = plot_of(ChartRender::Blocks);

        assert!(
            plot.chars().any(is_half_block),
            "blocks should be drawn in half blocks:\n{plot}"
        );
        let blank = unpainted_columns(&plot);
        assert!(blank.is_empty(), "columns {blank:?} are unpainted:\n{plot}");
    }

    #[test]
    fn dots_keep_the_braille_marker() {

        let plot = plot_of(ChartRender::Dots);
        assert!(
            plot.chars().any(is_braille),
            "dots should still be braille:\n{plot}"
        );
    }

    #[test]
    fn the_three_styles_share_no_glyphs() {

        let styles = [
            (ChartRender::Steps, plot_of(ChartRender::Steps)),
            (ChartRender::Blocks, plot_of(ChartRender::Blocks)),
            (ChartRender::Dots, plot_of(ChartRender::Dots)),
        ];

        for (style, plot) in &styles {
            let glyphs: std::collections::BTreeSet<char> =
                plot.chars().filter(|c| *c != ' ' && *c != '\n').collect();
            assert!(!glyphs.is_empty(), "{style:?} drew nothing");

            for (other, other_plot) in &styles {
                if other == style {
                    continue;
                }
                let shared: Vec<char> = other_plot
                    .chars()
                    .filter(|c| *c != ' ' && *c != '\n' && glyphs.contains(c))
                    .collect();
                assert!(
                    shared.is_empty(),
                    "{style:?} and {other:?} share {shared:?}"
                );
            }
        }
    }

    #[test]
    fn minimal_drops_everything_except_the_plot() {
        let view = styled(ChartRender::Steps, true);
        assert_eq!(view.height(), CHART_HEIGHT, "no rows for legend or notes");

        let rendered = render_to_string(&view, 70, view.height());
        assert!(!rendered.contains("bitcoin"), "{rendered}");
        assert!(!rendered.contains("7d"), "no title:\n{rendered}");
        assert!(!rendered.contains('%'), "no axis labels:\n{rendered}");
        assert!(!rendered.contains("$66,560.00"), "{rendered}");

        assert!(rendered.contains('┌'), "the border stays:\n{rendered}");
        assert!(
            painted_cells(&plot_text(&rendered, true)) > 20,
            "{rendered}"
        );
    }

    #[test]
    fn minimal_gives_the_plot_the_rows_the_axis_labels_would_have_taken() {

        let full = plot_text(
            &render_to_string(&wavy_view(ChartRender::Steps, false), 70, CHART_HEIGHT),
            false,
        );
        let minimal = plot_text(
            &render_to_string(&wavy_view(ChartRender::Steps, true), 70, CHART_HEIGHT),
            true,
        );

        assert!(
            minimal.lines().count() > full.lines().count(),
            "minimal should plot into more rows"
        );
        assert!(
            minimal.lines().next().unwrap().chars().count()
                > full.lines().next().unwrap().chars().count(),
            "minimal should plot into more columns"
        );
    }

    #[test]
    fn minimal_suppresses_the_notes_a_normal_chart_would_show() {
        let mut data = sample_data();
        data.failed
            .push(("dogecoin".into(), CoinError::RateLimited));

        let view = build(
            &data,
            &[Color::Cyan, Color::Magenta],
            ChartStyle {
                render: ChartRender::Steps,
                minimal: true,
            },
        );

        assert_eq!(view.notes.len(), 1);

        let rendered = render_to_string(&view, 70, view.height());
        assert!(!rendered.contains("dogecoin"), "{rendered}");
    }

    #[test]
    fn minimal_still_explains_itself_when_there_is_no_plot_to_protect() {

        let data = MarketData {
            series: Vec::new(),
            quotes: Vec::new(),
            failed: vec![("notacoin123".into(), CoinError::NotFound)],
            notes: Vec::new(),
        };
        let view = build(
            &data,
            &[Color::Cyan],
            ChartStyle {
                render: ChartRender::Steps,
                minimal: true,
            },
        );

        assert_eq!(view.height(), 1, "one row for the one note");
        let rendered = render_to_string(&view, 70, view.height());
        assert!(rendered.contains("notacoin123"), "{rendered}");
    }

    #[test]
    fn piped_output_ignores_minimal_rather_than_printing_nothing() {
        let view = styled(ChartRender::Dots, true);
        let text = view.plain_text();

        assert!(text.contains("bitcoin"), "{text}");
        assert!(text.contains("$66,560.00"), "{text}");
    }

    #[test]
    fn every_style_survives_a_terminal_too_small_to_frame() {

        for render in ChartRender::ALL {
            for minimal in [false, true] {
                let view = wavy_view(render, minimal);
                for (width, height) in [(1, 1), (3, 2), (8, 3), (24, 6)] {
                    let rendered = render_to_string(&view, width, height);
                    assert!(!rendered.is_empty(), "{render:?} at {width}x{height}");
                }
            }
        }
    }

    #[test]
    fn a_cramped_chart_still_gets_a_line_if_it_gets_a_plot_at_all() {

        for (width, height) in [(70, 16), (40, 12), (24, 10), (16, 8), (12, 6)] {
            let blocks = plot_text(
                &render_to_string(&wavy_view(ChartRender::Blocks, false), width, height),
                false,
            );
            if painted_cells(&blocks) == 0 {
                continue;
            }

            let lines = plot_text(
                &render_to_string(&wavy_view(ChartRender::Steps, false), width, height),
                false,
            );
            assert!(
                painted_cells(&lines) > 0,
                "blocks drew a plot at {width}x{height} but lines drew nothing:\n{lines}"
            );
        }
    }

    fn painted_row_span(plot: &str) -> (Option<usize>, Option<usize>) {
        let rows: Vec<usize> = plot
            .lines()
            .enumerate()
            .filter(|(_, line)| line.chars().any(is_plot_cell))
            .map(|(index, _)| index)
            .collect();
        (rows.first().copied(), rows.last().copied())
    }

    #[test]
    fn the_line_reaches_the_same_rows_the_blocks_do() {

        let lines = plot_text(
            &render_to_string(&wavy_view(ChartRender::Steps, true), 70, CHART_HEIGHT),
            true,
        );
        let blocks = plot_text(
            &render_to_string(&wavy_view(ChartRender::Blocks, true), 70, CHART_HEIGHT),
            true,
        );

        assert_eq!(
            painted_row_span(&lines),
            painted_row_span(&blocks),
            "the line and the blocks should span the same rows"
        );
    }
}
