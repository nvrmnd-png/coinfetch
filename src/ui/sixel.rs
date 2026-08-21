
use std::collections::HashSet;

use icy_sixel::dither::sixel_dither;
use icy_sixel::output::sixel_output;
use icy_sixel::{DiffusionMethod, PixelFormat, Quality};
use image::RgbaImage;

const OPAQUE_AT: u8 = 128;

const MAX_COLORS: usize = 256;

const OPAQUE_HEADER: &str = "\x1bPq";
const TRANSPARENT_HEADER: &str = "\x1bP0;1;0q";

pub fn encode(image: &RgbaImage, lines: &[[u8; 3]]) -> Option<String> {
    let (width, height) = image.dimensions();
    if width == 0 || height == 0 {
        return None;
    }

    let mut colors = vec![key_color(image, lines)?];
    for line in lines {
        if !colors.contains(line) {
            colors.push(*line);
        }
    }
    if colors.len() > MAX_COLORS {
        return None;
    }

    let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
    for pixel in image.pixels() {
        let source = if pixel.0[3] >= OPAQUE_AT {
            nearest(&colors[1..], [pixel.0[0], pixel.0[1], pixel.0[2]]).unwrap_or(colors[0])
        } else {
            colors[0]
        };
        rgb.extend_from_slice(&source);
    }

    let mut dither = sixel_dither::new(colors.len() as i32).ok()?;
    dither.set_pixelformat(PixelFormat::RGB888);
    dither.set_palette(colors.iter().flatten().copied().collect());
    dither.ncolors = colors.len() as i32;

    dither.optimized = true;
    dither.set_quality_mode(Quality::HIGH);

    dither.set_optimize_palette(false);

    dither.set_diffusion_type(DiffusionMethod::None);
    dither.set_transparent(0);

    dither.set_body_only(true);

    let mut out: Vec<u8> = Vec::new();
    {
        let mut output = sixel_output::new(&mut out);
        output
            .encode(&mut rgb, width as i32, height as i32, 0, &mut dither)
            .ok()?;
    }

    if dither.palette.get(..3) != Some(&colors[0][..]) {
        return None;
    }
    let palette = palette_definitions(&dither.palette, dither.ncolors);

    let raster = format!("\"1;1;{width};{height}");
    let sequence = String::from_utf8(out).ok()?;
    let body = sequence.strip_prefix(&format!("{OPAQUE_HEADER}{raster}"))?;
    Some(format!("{TRANSPARENT_HEADER}{raster}{palette}{body}"))
}

fn palette_definitions(palette: &[u8], ncolors: i32) -> String {
    let percent = |value: u8| (u32::from(value) * 100 + 127) / 255;

    (0..ncolors.max(0) as usize)
        .filter(|index| palette.len() >= (index + 1) * 3)
        .map(|index| {
            let at = index * 3;
            format!(
                "#{index};2;{};{};{}",
                percent(palette[at]),
                percent(palette[at + 1]),
                percent(palette[at + 2])
            )
        })
        .collect()
}

fn key_color(image: &RgbaImage, lines: &[[u8; 3]]) -> Option<[u8; 3]> {

    let mut drawn: HashSet<[u8; 3]> = lines.iter().copied().collect();
    drawn.extend(
        image
            .pixels()
            .filter(|pixel| pixel.0[3] >= OPAQUE_AT)
            .map(|pixel| [pixel.0[0], pixel.0[1], pixel.0[2]]),
    );

    const STEPS: [u8; 6] = [0, 51, 102, 153, 204, 255];
    STEPS
        .iter()
        .flat_map(|r| {
            STEPS
                .iter()
                .flat_map(move |g| STEPS.iter().map(move |b| [*r, *g, *b]))
        })
        .find(|candidate| !drawn.contains(candidate))
}

fn nearest(palette: &[[u8; 3]], color: [u8; 3]) -> Option<[u8; 3]> {
    palette.iter().copied().min_by_key(|candidate| {
        (0..3)
            .map(|c| {
                let delta = i32::from(candidate[c]) - i32::from(color[c]);
                delta * delta
            })
            .sum::<i32>()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn one_line(color: [u8; 3]) -> RgbaImage {
        let mut image = RgbaImage::new(40, 20);
        for x in 0..40 {
            for (y, alpha) in [(9, 90u8), (10, 255), (11, 255), (12, 90)] {
                image.put_pixel(x, y, Rgba([color[0], color[1], color[2], alpha]));
            }
        }
        image
    }

    #[test]
    fn the_sequence_says_to_leave_the_unset_pixels_alone() {

        let sequence = encode(&one_line([0, 205, 205]), &[[0, 205, 205]]).expect("a sequence");
        assert!(
            sequence.starts_with(TRANSPARENT_HEADER),
            "header is {:?}",
            &sequence[..12.min(sequence.len())]
        );
        assert!(!sequence.starts_with(OPAQUE_HEADER));
        assert!(sequence.ends_with("\x1b\\"), "unterminated sequence");
    }

    #[test]
    fn the_line_colour_is_in_the_palette_the_sequence_declares() {

        let sequence = encode(&one_line([0, 205, 205]), &[[0, 205, 205]]).expect("a sequence");
        assert!(
            sequence.contains(";2;0;80;80"),
            "no cyan in the palette: {}",
            &sequence[..300.min(sequence.len())]
        );
    }

    #[test]
    fn the_key_is_never_a_colour_the_chart_draws_in() {

        for color in [
            [255u8, 0, 255],
            [0, 0, 0],
            [255, 255, 255],
            [51, 102, 153],
            [0, 205, 205],
        ] {
            let image = one_line(color);
            let key = key_color(&image, &[color]).expect("a key");
            assert_ne!(key, color, "the key landed on the line's own color");
        }
    }

    #[test]
    fn a_canvas_with_nothing_drawn_on_it_still_encodes() {

        let sequence = encode(&RgbaImage::new(20, 12), &[[0, 205, 205]]).expect("a sequence");
        assert!(sequence.starts_with(TRANSPARENT_HEADER));
    }

    #[test]
    fn several_colours_all_reach_the_palette() {

        let mut image = RgbaImage::new(60, 24);
        for x in 0..60 {
            for y in 4..6 {
                image.put_pixel(x, y, Rgba([0, 205, 205, 255]));
            }
            for y in 16..18 {
                image.put_pixel(x, y, Rgba([205, 0, 205, 255]));
            }
        }

        let sequence = encode(&image, &[[0, 205, 205], [205, 0, 205]]).expect("a sequence");
        assert!(sequence.contains(";2;0;80;80"), "no cyan");
        assert!(sequence.contains(";2;80;0;80"), "no magenta");
    }

    #[test]
    fn a_real_chart_does_not_run_the_palette_out() {

        use crate::model::{ChartData, NormalizedSeries};
        use crate::ui::plot_image;
        use ratatui::style::Color;

        let series = |id: &str, swing: f64, period: f64| NormalizedSeries {
            id: id.to_string(),
            points: (0..168)
                .map(|hour| {
                    let t = f64::from(hour);
                    (t, swing * (t / period).sin())
                })
                .collect(),
            last_value: 0.0,
        };
        let data = ChartData {
            series: vec![
                series("bitcoin", 6.0, 12.0),
                series("ethereum", 7.0, 15.0),
                series("solana", 9.0, 20.0),
            ],
            x_bounds: [0.0, 167.0],
            y_bounds: [-9.0, 9.0],
        };
        let lines = plot_image::palette(&[Color::Cyan, Color::Magenta, Color::Yellow]);
        let image = plot_image::draw(
            &data,
            &[Color::Cyan, Color::Magenta, Color::Yellow],
            1300,
            230,
        )
        .expect("an image");

        let sequence = encode(&image, &lines).expect("a sequence");

        assert_eq!(
            sequence.matches(";2;").count(),
            4,
            "unexpected palette size"
        );
        for entry in [";2;0;80;80", ";2;80;0;80", ";2;80;80;0"] {
            assert!(sequence.contains(entry), "missing {entry}");
        }
        assert!(
            sequence.len() > 5_000,
            "suspiciously short: {}",
            sequence.len()
        );
    }

    #[test]
    fn an_empty_canvas_is_refused_rather_than_encoded() {
        assert!(encode(&RgbaImage::new(0, 10), &[[0, 205, 205]]).is_none());
        assert!(encode(&RgbaImage::new(10, 0), &[[0, 205, 205]]).is_none());
    }
}
