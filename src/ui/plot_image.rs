
use image::{Rgba, RgbaImage};
use plotters::prelude::*;
use ratatui::style::Color;

use crate::model::ChartData;

const SUPERSAMPLE: u32 = 3;

const STROKE_PX: u32 = 2;

const MAX_SIDE: u32 = 4_000;

pub fn draw(data: &ChartData, colors: &[Color], width: u32, height: u32) -> Option<RgbaImage> {
    if data.series.is_empty() || colors.is_empty() || width == 0 || height == 0 {
        return None;
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }

    let (w, h) = (width * SUPERSAMPLE, height * SUPERSAMPLE);

    let over_black = render_pass(data, colors, w, h, 0)?;
    let over_white = render_pass(data, colors, w, h, 255)?;

    let mut large = RgbaImage::new(w, h);
    for (index, pixel) in large.pixels_mut().enumerate() {
        let at = index * 3;

        let uncovered: u32 = (0..3)
            .map(|c| u32::from(over_white[at + c].saturating_sub(over_black[at + c])))
            .sum::<u32>()
            / 3;
        let alpha = 255u32.saturating_sub(uncovered) as u8;
        *pixel = Rgba([
            over_black[at],
            over_black[at + 1],
            over_black[at + 2],
            alpha,
        ]);
    }

    let mut small =
        image::imageops::resize(&large, width, height, image::imageops::FilterType::Triangle);
    unpremultiply(&mut small);
    Some(small)
}

fn render_pass(
    data: &ChartData,
    colors: &[Color],
    width: u32,
    height: u32,
    background: u8,
) -> Option<Vec<u8>> {
    let mut buffer = vec![background; (width as usize) * (height as usize) * 3];

    {
        let root = BitMapBackend::with_buffer(&mut buffer, (width, height)).into_drawing_area();

        let mut chart = ChartBuilder::on(&root)
            .margin(0)
            .build_cartesian_2d(span(data.x_bounds), span(data.y_bounds))
            .ok()?;

        for (index, series) in data.series.iter().enumerate() {
            let color = rgb_of(colors[index % colors.len()]);
            chart
                .draw_series(LineSeries::new(
                    series.points.iter().copied(),
                    ShapeStyle {
                        color: color.into(),
                        filled: true,
                        stroke_width: STROKE_PX * SUPERSAMPLE,
                    },
                ))
                .ok()?;
        }

        root.present().ok()?;
    }

    Some(buffer)
}

fn span([low, high]: [f64; 2]) -> std::ops::Range<f64> {
    if !(low.is_finite() && high.is_finite()) {
        return -1.0..1.0;
    }
    if high > low {
        return low..high;
    }
    let pad = low.abs().max(1.0) * 0.01;
    (low - pad)..(high + pad)
}

fn unpremultiply(image: &mut RgbaImage) {
    for pixel in image.pixels_mut() {
        let alpha = pixel.0[3];
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in &mut pixel.0[..3] {
            *channel = ((u32::from(*channel) * 255) / u32::from(alpha)).min(255) as u8;
        }
    }
}

fn rgb_of(color: Color) -> RGBColor {
    let [r, g, b] = rgb_bytes(color);
    RGBColor(r, g, b)
}

pub fn palette(colors: &[Color]) -> Vec<[u8; 3]> {
    colors.iter().copied().map(rgb_bytes).collect()
}

fn rgb_bytes(color: Color) -> [u8; 3] {
    let (r, g, b) = match color {
        Color::Black => (0, 0, 0),
        Color::Red => (205, 0, 0),
        Color::Green => (0, 205, 0),
        Color::Yellow => (205, 205, 0),
        Color::Blue => (0, 0, 238),
        Color::Magenta => (205, 0, 205),
        Color::Cyan => (0, 205, 205),
        Color::Gray => (229, 229, 229),
        Color::DarkGray => (127, 127, 127),
        Color::LightRed => (255, 0, 0),
        Color::LightGreen => (0, 255, 0),
        Color::LightYellow => (255, 255, 0),
        Color::LightBlue => (92, 92, 255),
        Color::LightMagenta => (255, 0, 255),
        Color::LightCyan => (0, 255, 255),
        Color::White => (255, 255, 255),
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Indexed(index) => indexed_rgb(index),

        Color::Reset => (229, 229, 229),
    };
    [r, g, b]
}

fn indexed_rgb(index: u8) -> (u8, u8, u8) {
    const NAMED: [(u8, u8, u8); 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];

    match index {
        0..=15 => NAMED[index as usize],
        16..=231 => {
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            let c = index - 16;
            (step(c / 36), step((c / 6) % 6), step(c % 6))
        }
        232..=255 => {
            let level = 8 + (index - 232) * 10;
            (level, level, level)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::NormalizedSeries;

    fn wavy(id: &str, swing: f64, period: f64) -> NormalizedSeries {
        NormalizedSeries {
            id: id.to_string(),
            points: (0..168)
                .map(|hour| {
                    let t = f64::from(hour);
                    (t, swing * (t / period).sin())
                })
                .collect(),
            last_value: 0.0,
        }
    }

    fn chart_data(series: Vec<NormalizedSeries>) -> ChartData {
        let (low, high) = series
            .iter()
            .flat_map(|s| s.points.iter().map(|(_, y)| *y))
            .fold((f64::MAX, f64::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
        ChartData {
            series,
            x_bounds: [0.0, 167.0],
            y_bounds: [low, high],
        }
    }

    fn opaque_colors(image: &RgbaImage) -> std::collections::BTreeMap<(u8, u8, u8), usize> {
        let mut seen = std::collections::BTreeMap::new();
        for pixel in image.pixels() {
            if pixel.0[3] != 255 {
                continue;
            }
            *seen
                .entry((pixel.0[0], pixel.0[1], pixel.0[2]))
                .or_insert(0) += 1;
        }
        seen
    }

    #[test]
    fn the_canvas_is_exactly_the_size_it_was_asked_for() {

        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0)]);
        let image = draw(&data, &[Color::Cyan], 240, 96).expect("an image");
        assert_eq!(image.dimensions(), (240, 96));
    }

    #[test]
    fn everything_that_is_not_a_line_is_transparent() {

        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0)]);
        let image = draw(&data, &[Color::Cyan], 240, 96).expect("an image");

        let clear = image.pixels().filter(|p| p.0[3] == 0).count();
        let painted = image.pixels().filter(|p| p.0[3] > 0).count();
        assert!(painted > 0, "nothing was drawn");
        assert!(
            clear > painted * 3,
            "a line should leave most of the canvas clear: {painted} painted, {clear} clear"
        );
    }

    #[test]
    fn each_coin_gets_its_own_color_in_the_one_image() {

        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0), wavy("solana", 9.0, 20.0)]);
        let image = draw(&data, &[Color::Cyan, Color::Magenta], 300, 120).expect("an image");

        let colors = opaque_colors(&image);
        assert!(
            colors.contains_key(&(0, 205, 205)),
            "no cyan line: {colors:?}"
        );
        assert!(
            colors.contains_key(&(205, 0, 205)),
            "no magenta line: {colors:?}"
        );
    }

    #[test]
    fn the_palette_cycles_the_same_way_the_legend_does() {

        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0), wavy("solana", 9.0, 20.0)]);
        let image = draw(&data, &[Color::Cyan], 300, 120).expect("an image");

        let colors = opaque_colors(&image);
        assert!(
            colors.keys().all(|c| *c == (0, 205, 205)),
            "a one-color palette should draw one color: {colors:?}"
        );
    }

    #[test]
    fn a_hex_palette_entry_is_drawn_as_the_exact_color_it_names() {
        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0)]);
        let image = draw(&data, &[Color::Rgb(255, 136, 0)], 240, 96).expect("an image");
        assert!(
            opaque_colors(&image).contains_key(&(255, 136, 0)),
            "the configured color should survive to the pixels"
        );
    }

    #[test]
    fn a_flat_coin_still_draws_a_line() {

        let flat = NormalizedSeries {
            id: "usd-coin".into(),
            points: (0..168).map(|hour| (f64::from(hour), 0.0)).collect(),
            last_value: 0.0,
        };
        let data = ChartData {
            series: vec![flat],
            x_bounds: [0.0, 167.0],
            y_bounds: [0.0, 0.0],
        };

        let image = draw(&data, &[Color::Cyan], 240, 96).expect("an image");
        assert!(image.pixels().any(|p| p.0[3] > 0), "nothing was drawn");
    }

    #[test]
    fn the_line_spans_the_whole_width_of_the_canvas() {

        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0)]);
        let image = draw(&data, &[Color::Cyan], 240, 96).expect("an image");

        let painted_column = |x: u32| (0..image.height()).any(|y| image.get_pixel(x, y).0[3] > 0);
        assert!(painted_column(0), "nothing at the left edge");
        assert!(
            painted_column(image.width() - 1),
            "nothing at the right edge"
        );
    }

    #[test]
    fn nothing_is_drawn_without_a_series_or_without_a_canvas() {
        let empty = ChartData {
            series: Vec::new(),
            x_bounds: [0.0, 1.0],
            y_bounds: [0.0, 1.0],
        };
        assert!(draw(&empty, &[Color::Cyan], 100, 100).is_none());

        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0)]);
        assert!(draw(&data, &[Color::Cyan], 0, 100).is_none());
        assert!(draw(&data, &[Color::Cyan], 100, 0).is_none());
        assert!(draw(&data, &[], 100, 100).is_none());
    }

    #[test]
    fn an_absurd_canvas_is_refused_rather_than_allocated() {

        let data = chart_data(vec![wavy("bitcoin", 6.0, 12.0)]);
        assert!(draw(&data, &[Color::Cyan], MAX_SIDE + 1, 100).is_none());
    }

    #[test]
    #[ignore = "timing measurement, not a correctness check"]
    fn how_long_one_frame_takes() {

        let data = chart_data(vec![
            wavy("bitcoin", 6.0, 12.0),
            wavy("ethereum", 7.0, 15.0),
            wavy("solana", 9.0, 20.0),
        ]);
        let colors = [Color::Cyan, Color::Magenta, Color::Yellow];

        for (w, h) in [(600, 280), (1200, 320), (2400, 400)] {
            let start = std::time::Instant::now();
            let runs = 20;
            for _ in 0..runs {
                draw(&data, &colors, w, h).expect("an image");
            }
            println!("{w}x{h}: {:?} per frame", start.elapsed() / runs);
        }
    }

    #[test]
    fn the_indexed_palette_matches_the_xterm_cube() {
        assert_eq!(indexed_rgb(0), (0, 0, 0));
        assert_eq!(indexed_rgb(16), (0, 0, 0));
        assert_eq!(indexed_rgb(21), (0, 0, 255));
        assert_eq!(indexed_rgb(231), (255, 255, 255));
        assert_eq!(indexed_rgb(232), (8, 8, 8));
        assert_eq!(indexed_rgb(255), (238, 238, 238));
    }
}
