use std::fs;
use std::path::PathBuf;

use directories::ProjectDirs;
use ratatui::style::Color;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

pub const MIN_CACHE_TTL_SECS: u64 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChartRender {

    #[default]
    Steps,

    Lines,
    Blocks,
    Dots,
}

impl ChartRender {

    pub const ALL: [Self; 4] = [Self::Steps, Self::Lines, Self::Blocks, Self::Dots];

    pub fn needs_graphics(self) -> bool {
        matches!(self, Self::Lines)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Steps => "steps",
            Self::Lines => "lines",
            Self::Blocks => "blocks",
            Self::Dots => "dots",
        }
    }

    pub fn step(self, delta: i32) -> Self {
        let index = Self::ALL.iter().position(|s| *s == self).unwrap_or(0) as i32;
        Self::ALL[(index + delta).rem_euclid(Self::ALL.len() as i32) as usize]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub default_coins: Vec<String>,

    #[serde(alias = "refresh_interval_secs")]
    pub cache_ttl_secs: u64,
    pub palette: Vec<String>,
    pub wallet_addresses: Vec<String>,

    pub chart_render: ChartRender,

    pub chart_minimal: bool,

    #[serde(alias = "coingecko_demo_key", skip_serializing)]
    pub coingecko_api_key: Option<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            default_coins: vec![
                "bitcoin".to_string(),
                "ethereum".to_string(),
                "solana".to_string(),
            ],
            cache_ttl_secs: 60,
            palette: ["cyan", "magenta", "yellow", "green", "blue", "red"]
                .iter()
                .map(|s| s.to_string())
                .collect(),
            wallet_addresses: Vec::new(),
            chart_render: ChartRender::default(),
            chart_minimal: false,
            coingecko_api_key: None,
        }
    }
}

pub fn config_path() -> Option<PathBuf> {
    ProjectDirs::from("", "", "coinfetch").map(|d| d.config_dir().join("config.toml"))
}

impl Config {

    pub fn load() -> (Config, Vec<String>) {
        let (mut config, mut warnings) = Config::load_file();
        warnings.extend(crate::secret::resolve_api_key(
            &mut config,
            &crate::secret::Keyring,
        ));
        (config, warnings)
    }

    fn load_file() -> (Config, Vec<String>) {
        let mut warnings = Vec::new();

        let Some(path) = config_path() else {
            return (Config::default(), warnings);
        };
        let Ok(text) = fs::read_to_string(&path) else {
            return (Config::default(), warnings);
        };

        match toml::from_str::<Config>(&text) {
            Ok(mut cfg) => {
                if cfg.cache_ttl_secs < MIN_CACHE_TTL_SECS {
                    warnings.push(format!(
                        "cache TTL raised to {MIN_CACHE_TTL_SECS}s (configured value was too low)"
                    ));
                    cfg.cache_ttl_secs = MIN_CACHE_TTL_SECS;
                }
                if cfg.palette.is_empty() {
                    warnings.push("empty palette, using defaults".to_string());
                    cfg.palette = Config::default().palette;
                }
                cfg.normalize_api_key();
                (cfg, warnings)
            }
            Err(err) => {
                warnings.push(format!(
                    "{} is not valid TOML ({err}), using defaults",
                    path.display()
                ));
                (Config::default(), warnings)
            }
        }
    }

    pub fn api_key(&self) -> Option<&str> {
        self.coingecko_api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
    }

    pub fn normalize_api_key(&mut self) {
        self.coingecko_api_key = self.api_key().map(str::to_string);
    }

    pub fn save(&self) -> Result<PathBuf> {
        let path = config_path()
            .ok_or_else(|| Error::msg("cannot determine a config directory on this system"))?;
        let dir = path
            .parent()
            .ok_or_else(|| Error::msg("config path has no parent directory"))?;
        fs::create_dir_all(dir)?;

        let text = toml::to_string_pretty(self)
            .map_err(|e| Error::msg(format!("could not serialize config: {e}")))?;

        let tmp = path.with_extension("toml.tmp");
        fs::write(&tmp, text)?;
        fs::rename(&tmp, &path)?;
        Ok(path)
    }

    pub fn colors(&self) -> (Vec<Color>, Vec<String>) {
        let mut colors = Vec::new();
        let mut warnings = Vec::new();
        for name in &self.palette {
            match parse_color(name) {
                Some(c) => colors.push(c),
                None => {
                    warnings.push(format!("unknown color `{name}`, falling back to white"));
                    colors.push(Color::White);
                }
            }
        }
        if colors.is_empty() {
            colors.push(Color::White);
        }
        (colors, warnings)
    }
}

pub fn parse_color(name: &str) -> Option<Color> {
    let key = name.trim().to_ascii_lowercase();

    if let Some(hex) = key.strip_prefix('#') {
        if hex.len() == 6 {
            let r = u8::from_str_radix(&hex[0..2], 16).ok()?;
            let g = u8::from_str_radix(&hex[2..4], 16).ok()?;
            let b = u8::from_str_radix(&hex[4..6], 16).ok()?;
            return Some(Color::Rgb(r, g, b));
        }
        return None;
    }

    let color = match key.as_str() {
        "black" => Color::Black,
        "red" => Color::Red,
        "green" => Color::Green,
        "yellow" => Color::Yellow,
        "blue" => Color::Blue,
        "magenta" => Color::Magenta,
        "cyan" => Color::Cyan,
        "gray" | "grey" => Color::Gray,
        "darkgray" | "darkgrey" => Color::DarkGray,
        "lightred" => Color::LightRed,
        "lightgreen" => Color::LightGreen,
        "lightyellow" => Color::LightYellow,
        "lightblue" => Color::LightBlue,
        "lightmagenta" => Color::LightMagenta,
        "lightcyan" => Color::LightCyan,
        "white" => Color::White,
        _ => return None,
    };
    Some(color)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_survive_a_toml_round_trip() {
        let original = Config::default();
        let text = toml::to_string_pretty(&original).expect("serialize");
        let parsed: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(original, parsed);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        let parsed: Config = toml::from_str("default_coins = [\"bitcoin\"]").expect("parse");
        assert_eq!(parsed.default_coins, vec!["bitcoin"]);
        assert_eq!(parsed.cache_ttl_secs, 60);
        assert_eq!(parsed.palette, Config::default().palette);
    }

    #[test]
    fn a_config_predating_the_chart_settings_gets_the_previous_look() {

        let parsed: Config = toml::from_str("default_coins = [\"bitcoin\"]").expect("parse");
        assert_eq!(parsed.chart_render, ChartRender::Steps);
        assert!(!parsed.chart_minimal);
    }

    #[test]
    fn a_steps_config_from_the_combined_era_still_loads_as_steps() {

        let parsed: Config = toml::from_str("chart_render = \"steps\"").expect("parse");
        assert_eq!(parsed.chart_render, ChartRender::Steps);
        assert!(
            !parsed.chart_render.needs_graphics(),
            "steps must never depend on the terminal again"
        );
    }

    #[test]
    fn lines_is_a_value_of_its_own_and_is_the_one_that_wants_a_picture() {
        let parsed: Config = toml::from_str("chart_render = \"lines\"").expect("parse");
        assert_eq!(parsed.chart_render, ChartRender::Lines);
        assert!(parsed.chart_render.needs_graphics());

        let picky: Vec<ChartRender> = ChartRender::ALL
            .into_iter()
            .filter(|s| s.needs_graphics())
            .collect();
        assert_eq!(picky, vec![ChartRender::Lines]);
    }

    #[test]
    fn the_chart_settings_survive_a_toml_round_trip() {
        let original = Config {
            chart_render: ChartRender::Dots,
            chart_minimal: true,
            ..Config::default()
        };
        let text = toml::to_string_pretty(&original).expect("serialize");
        assert!(text.contains("chart_render = \"dots\""), "{text}");

        let parsed: Config = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, original);
    }

    #[test]
    fn every_render_style_has_its_own_name_in_the_file() {

        for style in ChartRender::ALL {
            let name = style.name();
            let text = toml::to_string_pretty(&Config {
                chart_render: style,
                ..Config::default()
            })
            .expect("serialize");
            assert!(
                text.contains(&format!("chart_render = \"{name}\"")),
                "{text}"
            );

            let parsed: Config =
                toml::from_str(&format!("chart_render = \"{name}\"")).expect("parse");
            assert_eq!(parsed.chart_render, style);
        }

        let mut names: Vec<&str> = ChartRender::ALL.iter().map(|s| s.name()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), ChartRender::ALL.len());
    }

    #[test]
    fn stepping_the_render_style_cycles_through_all_four_in_both_directions() {
        let forward: Vec<ChartRender> = (1..=4)
            .scan(ChartRender::Steps, |style, _| {
                *style = style.step(1);
                Some(*style)
            })
            .collect();
        assert_eq!(
            forward,
            vec![
                ChartRender::Lines,
                ChartRender::Blocks,
                ChartRender::Dots,
                ChartRender::Steps,
            ],
            "a full cycle must visit every style and come back"
        );

        assert_eq!(ChartRender::Steps.step(-1), ChartRender::Dots);
        assert_eq!(ChartRender::Lines.step(-1), ChartRender::Steps);
    }

    #[test]
    fn reads_the_cache_ttl_written_under_the_old_refresh_interval_name() {
        let parsed: Config = toml::from_str("refresh_interval_secs = 120").expect("parse");
        assert_eq!(parsed.cache_ttl_secs, 120);
    }

    #[test]
    fn a_config_without_a_key_stays_on_the_free_tier() {
        let parsed: Config = toml::from_str("default_coins = [\"bitcoin\"]").expect("parse");
        assert_eq!(parsed.coingecko_api_key, None);
        assert_eq!(parsed.api_key(), None);
    }

    #[test]
    fn reads_a_key_written_under_the_old_field_name() {
        let parsed: Config = toml::from_str("coingecko_demo_key = \"CG-abc123\"").expect("parse");
        assert_eq!(parsed.api_key(), Some("CG-abc123"));
    }

    #[test]
    fn a_blank_key_counts_as_no_key_at_all() {
        let mut cfg: Config = toml::from_str("coingecko_api_key = \"   \"").expect("parse");
        assert_eq!(cfg.api_key(), None);

        cfg.normalize_api_key();
        assert_eq!(cfg.coingecko_api_key, None);
        let text = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(!text.contains("coingecko_api_key"), "{text}");
    }

    #[test]
    fn a_key_is_read_from_a_file_but_never_written_back_to_one() {

        let mut cfg: Config =
            toml::from_str("coingecko_api_key = \"  CG-abc123  \"").expect("parse");
        cfg.normalize_api_key();
        assert_eq!(cfg.api_key(), Some("CG-abc123"));

        let text = toml::to_string_pretty(&cfg).expect("serialize");
        assert!(!text.contains("coingecko_api_key"), "{text}");
        assert!(!text.contains("CG-abc123"), "{text}");
    }

    #[test]
    fn parses_named_and_hex_colors() {
        assert_eq!(parse_color("cyan"), Some(Color::Cyan));
        assert_eq!(parse_color("  LightBlue "), Some(Color::LightBlue));
        assert_eq!(parse_color("grey"), Some(Color::Gray));
        assert_eq!(parse_color("#ff8800"), Some(Color::Rgb(255, 136, 0)));
    }

    #[test]
    fn rejects_malformed_colors() {
        assert_eq!(parse_color("chartreuse"), None);
        assert_eq!(parse_color("#fff"), None);
        assert_eq!(parse_color("#gggggg"), None);
    }

    #[test]
    fn unknown_palette_entry_degrades_with_a_warning() {
        let cfg = Config {
            palette: vec!["cyan".into(), "chartreuse".into()],
            ..Config::default()
        };
        let (colors, warnings) = cfg.colors();
        assert_eq!(colors, vec![Color::Cyan, Color::White]);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("chartreuse"));
    }

    #[test]
    fn colors_never_returns_an_empty_palette() {
        let cfg = Config {
            palette: Vec::new(),
            ..Config::default()
        };
        let (colors, _) = cfg.colors();
        assert!(!colors.is_empty());
    }
}
