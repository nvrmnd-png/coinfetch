
use crate::error::CoinError;

#[derive(Debug, Clone, PartialEq)]
pub struct RawSeries {
    pub id: String,
    pub points: Vec<(f64, f64)>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    pub id: String,
    pub symbol: String,
    pub price: f64,
    pub change_24h: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct NormalizedSeries {
    pub id: String,
    pub points: Vec<(f64, f64)>,
    pub last_value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChartData {
    pub series: Vec<NormalizedSeries>,
    pub x_bounds: [f64; 2],
    pub y_bounds: [f64; 2],
}

#[derive(Debug, Clone, PartialEq)]
pub struct Normalized {
    pub chart: Option<ChartData>,
    pub skipped: Vec<(String, CoinError)>,
}

fn sanitize(points: &[(f64, f64)]) -> Vec<(f64, f64)> {
    let mut clean: Vec<(f64, f64)> = points
        .iter()
        .copied()
        .filter(|(t, p)| t.is_finite() && p.is_finite())
        .collect();
    clean.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    clean
}

struct WindowedSeries {
    id: String,

    points: Vec<(f64, f64)>,
}

type WindowResult = (
    Option<[f64; 2]>,
    Vec<WindowedSeries>,
    Vec<(String, CoinError)>,
);

fn window(raw: &[RawSeries]) -> WindowResult {
    let mut skipped: Vec<(String, CoinError)> = Vec::new();
    let mut usable: Vec<(String, Vec<(f64, f64)>)> = Vec::new();

    for series in raw {
        let points = sanitize(&series.points);
        if points.len() < 2 {
            skipped.push((series.id.clone(), CoinError::NoData));
        } else {
            usable.push((series.id.clone(), points));
        }
    }

    if usable.is_empty() {
        return (None, Vec::new(), skipped);
    }

    let start = usable
        .iter()
        .map(|(_, p)| p[0].0)
        .fold(f64::NEG_INFINITY, f64::max);
    let end = usable
        .iter()
        .map(|(_, p)| p[p.len() - 1].0)
        .fold(f64::INFINITY, f64::min);

    if start >= end {
        for (id, _) in usable {
            skipped.push((id, CoinError::OutsideWindow));
        }
        return (None, Vec::new(), skipped);
    }

    let mut clipped = Vec::new();
    for (id, points) in usable {
        let within: Vec<(f64, f64)> = points
            .into_iter()
            .filter(|(t, _)| *t >= start && *t <= end)
            .collect();
        if within.len() < 2 {
            skipped.push((id, CoinError::OutsideWindow));
            continue;
        }
        clipped.push(WindowedSeries { id, points: within });
    }

    (Some([start, end]), clipped, skipped)
}

fn pad_range(mut lo: f64, mut hi: f64) -> [f64; 2] {
    let span = hi - lo;
    if span < 1e-9 {
        lo -= 1.0;
        hi += 1.0;
    } else {
        let pad = span * 0.05;
        lo -= pad;
        hi += pad;
    }
    [lo, hi]
}

pub fn normalize(raw: &[RawSeries]) -> Normalized {
    let (bounds, windowed, mut skipped) = window(raw);
    let Some([start, end]) = bounds else {
        return Normalized {
            chart: None,
            skipped,
        };
    };

    let mut series_out: Vec<NormalizedSeries> = Vec::new();
    for series in windowed {
        let baseline = series.points[0].1;
        if baseline <= 0.0 {
            skipped.push((series.id, CoinError::ZeroBaseline));
            continue;
        }

        let rebased: Vec<(f64, f64)> = series
            .points
            .iter()
            .map(|(t, p)| (*t, (p / baseline - 1.0) * 100.0))
            .collect();
        let last_value = rebased[rebased.len() - 1].1;
        series_out.push(NormalizedSeries {
            id: series.id,
            points: rebased,
            last_value,
        });
    }

    if series_out.is_empty() {
        return Normalized {
            chart: None,
            skipped,
        };
    }

    let mut y_min = 0.0_f64;
    let mut y_max = 0.0_f64;
    for s in &series_out {
        for (_, pct) in &s.points {
            y_min = y_min.min(*pct);
            y_max = y_max.max(*pct);
        }
    }

    Normalized {
        chart: Some(ChartData {
            series: series_out,
            x_bounds: [start, end],
            y_bounds: pad_range(y_min, y_max),
        }),
        skipped,
    }
}

fn group_thousands(digits: &str) -> String {
    let mut out = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub fn format_price(price: f64) -> String {
    if !price.is_finite() {
        return "$—".to_string();
    }
    let sign = if price < 0.0 { "-" } else { "" };
    let value = price.abs();

    let decimals = if value >= 1.0 {
        2
    } else if value >= 0.01 {
        4
    } else {
        8
    };

    let text = format!("{value:.decimals$}");
    let (int_part, frac_part) = text.split_once('.').unwrap_or((text.as_str(), ""));
    let grouped = group_thousands(int_part);

    if decimals == 8 {
        let trimmed = frac_part.trim_end_matches('0');
        if trimmed.is_empty() {
            return format!("{sign}${grouped}.00");
        }
        return format!("{sign}${grouped}.{trimmed}");
    }
    format!("{sign}${grouped}.{frac_part}")
}

pub fn format_pct(pct: f64) -> String {
    if !pct.is_finite() {
        return "—".to_string();
    }
    format!("{pct:+.2}%")
}

pub fn format_btc(sats: i64) -> String {
    let sign = if sats < 0 { "-" } else { "" };
    let abs = sats.unsigned_abs();
    let whole = abs / 100_000_000;
    let frac = abs % 100_000_000;
    let frac_text = format!("{frac:08}");
    let trimmed = frac_text.trim_end_matches('0');
    if trimmed.is_empty() {
        format!("{sign}{whole}.0")
    } else {
        format!("{sign}{whole}.{trimmed}")
    }
}

pub fn format_eth(wei: u128) -> String {
    const WEI_PER_ETH: u128 = 1_000_000_000_000_000_000;
    let whole = wei / WEI_PER_ETH;
    let frac = wei % WEI_PER_ETH;
    let frac_text = format!("{frac:018}");
    let trimmed = frac_text.trim_end_matches('0');
    if trimmed.is_empty() {
        format!("{whole}.0")
    } else {
        format!("{whole}.{trimmed}")
    }
}

pub fn format_day(unix_ms: f64) -> String {
    if !unix_ms.is_finite() {
        return "??-??".to_string();
    }
    let days = (unix_ms / 86_400_000.0).floor() as i64;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };

    format!("{month:02}-{day:02}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn series(id: &str, points: &[(f64, f64)]) -> RawSeries {
        RawSeries {
            id: id.to_string(),
            points: points.to_vec(),
        }
    }

    #[test]
    fn rebases_a_single_series_to_percent_since_the_start() {
        let out = normalize(&[series(
            "bitcoin",
            &[(1000.0, 100.0), (2000.0, 110.0), (3000.0, 120.0)],
        )]);
        let chart = out.chart.expect("chart");
        assert!(out.skipped.is_empty());

        let pcts: Vec<f64> = chart.series[0].points.iter().map(|(_, p)| *p).collect();
        assert!((pcts[0] - 0.0).abs() < 1e-9);
        assert!((pcts[1] - 10.0).abs() < 1e-9);
        assert!((pcts[2] - 20.0).abs() < 1e-9);
        assert!((chart.series[0].last_value - 20.0).abs() < 1e-9);
        assert_eq!(chart.x_bounds, [1000.0, 3000.0]);
    }

    #[test]
    fn makes_coins_of_very_different_price_levels_comparable() {

        let out = normalize(&[
            series("bitcoin", &[(1000.0, 64_000.0), (2000.0, 70_400.0)]),
            series("solana", &[(1000.0, 76.0), (2000.0, 83.6)]),
        ]);
        let chart = out.chart.expect("chart");
        assert_eq!(chart.series.len(), 2);
        assert!((chart.series[0].last_value - 10.0).abs() < 1e-9);
        assert!((chart.series[1].last_value - 10.0).abs() < 1e-9);
    }

    #[test]
    fn clips_to_the_shared_window_and_rebases_from_inside_it() {

        let out = normalize(&[
            series(
                "bitcoin",
                &[(1000.0, 50.0), (2000.0, 100.0), (3000.0, 150.0)],
            ),
            series("ethereum", &[(2000.0, 10.0), (3000.0, 12.0)]),
        ]);
        let chart = out.chart.expect("chart");
        assert_eq!(chart.x_bounds, [2000.0, 3000.0]);

        let btc = &chart.series[0];
        assert_eq!(btc.points.len(), 2);
        assert!((btc.points[0].1 - 0.0).abs() < 1e-9);
        assert!((btc.last_value - 50.0).abs() < 1e-9);
    }

    #[test]
    fn handles_series_of_unequal_length() {
        let mut long = Vec::new();
        for i in 0..169 {
            long.push((1000.0 + i as f64, 100.0 + i as f64));
        }
        let mut short = Vec::new();
        for i in 0..168 {
            short.push((1000.0 + i as f64, 50.0 + i as f64));
        }
        let out = normalize(&[series("a", &long), series("b", &short)]);
        let chart = out.chart.expect("chart");
        assert_eq!(chart.series.len(), 2);
        assert!(out.skipped.is_empty());
    }

    #[test]
    fn skips_a_coin_whose_baseline_is_zero_instead_of_emitting_nan() {
        let out = normalize(&[
            series("good", &[(1000.0, 10.0), (2000.0, 12.0)]),
            series("broken", &[(1000.0, 0.0), (2000.0, 5.0)]),
        ]);
        let chart = out.chart.expect("chart");
        assert_eq!(chart.series.len(), 1);
        assert_eq!(chart.series[0].id, "good");
        assert_eq!(
            out.skipped,
            vec![("broken".into(), CoinError::ZeroBaseline)]
        );
        assert!(chart.series[0].points.iter().all(|(_, p)| p.is_finite()));
    }

    #[test]
    fn skips_a_series_with_fewer_than_two_points() {
        let out = normalize(&[series("thin", &[(1000.0, 10.0)])]);
        assert!(out.chart.is_none());
        assert_eq!(out.skipped, vec![("thin".into(), CoinError::NoData)]);
    }

    #[test]
    fn reports_a_clean_failure_when_windows_do_not_overlap() {
        let out = normalize(&[
            series("early", &[(1000.0, 10.0), (2000.0, 11.0)]),
            series("late", &[(9000.0, 20.0), (9500.0, 21.0)]),
        ]);
        assert!(out.chart.is_none());
        assert_eq!(out.skipped.len(), 2);
        assert!(
            out.skipped
                .iter()
                .all(|(_, e)| *e == CoinError::OutsideWindow)
        );
    }

    #[test]
    fn y_bounds_always_contain_zero_and_are_padded() {

        let out = normalize(&[series("up", &[(1000.0, 100.0), (2000.0, 120.0)])]);
        let chart = out.chart.expect("chart");
        assert!(chart.y_bounds[0] < 0.0, "{:?}", chart.y_bounds);
        assert!(chart.y_bounds[1] > 20.0, "{:?}", chart.y_bounds);

        assert!((chart.y_bounds[0] + 1.0).abs() < 1e-9);
        assert!((chart.y_bounds[1] - 21.0).abs() < 1e-9);
    }

    #[test]
    fn a_perfectly_flat_series_still_gets_a_usable_y_range() {
        let out = normalize(&[series("flat", &[(1000.0, 42.0), (2000.0, 42.0)])]);
        let chart = out.chart.expect("chart");
        assert!(chart.y_bounds[1] > chart.y_bounds[0]);
    }

    #[test]
    fn drops_non_finite_samples_and_sorts_by_time() {
        let out = normalize(&[series(
            "messy",
            &[
                (3000.0, 120.0),
                (1000.0, 100.0),
                (2000.0, f64::NAN),
                (f64::INFINITY, 5.0),
            ],
        )]);
        let chart = out.chart.expect("chart");
        let times: Vec<f64> = chart.series[0].points.iter().map(|(t, _)| *t).collect();
        assert_eq!(times, vec![1000.0, 3000.0]);
    }

    #[test]
    fn formats_prices_across_magnitudes() {
        assert_eq!(format_price(64_098.0), "$64,098.00");
        assert_eq!(format_price(1_877.05), "$1,877.05");
        assert_eq!(format_price(75.87), "$75.87");
        assert_eq!(format_price(0.5), "$0.5000");
        assert_eq!(format_price(0.00001234), "$0.00001234");
        assert_eq!(format_price(0.0), "$0.00");
    }

    #[test]
    fn formats_percentages_with_an_explicit_sign() {
        assert_eq!(format_pct(4.123), "+4.12%");
        assert_eq!(format_pct(-1.8), "-1.80%");
    }

    #[test]
    fn formats_satoshis_as_btc() {
        assert_eq!(format_btc(13_001_007_902_736), "130010.07902736");
        assert_eq!(format_btc(100_000_000), "1.0");
        assert_eq!(format_btc(0), "0.0");
        assert_eq!(format_btc(1), "0.00000001");
    }

    #[test]
    fn formats_wei_as_eth() {
        assert_eq!(format_eth(0x1a055690d9db80000), "30.0");
        assert_eq!(format_eth(1_000_000_000_000_000_000), "1.0");
        assert_eq!(format_eth(0), "0.0");
        assert_eq!(format_eth(1), "0.000000000000000001");
    }

    #[test]
    fn formats_a_civil_day_from_unix_millis() {

        assert_eq!(format_day(0.0), "01-01");

        assert_eq!(format_day(951_868_800_000.0), "03-01");
    }
}
