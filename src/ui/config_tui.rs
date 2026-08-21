
use std::sync::Arc;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Clear, List, ListItem, ListState, Paragraph, StatefulWidget, Widget,
};
use tokio::sync::mpsc;

use crate::api::coingecko::{self, CoinGecko, MAX_COINS, SearchHit};
use crate::config::{Config, MIN_CACHE_TTL_SECS};
use crate::error::{Error, Result};
use crate::secret::{self, KeyStore};

const ACCENT: Color = Color::Rgb(255, 176, 0);
const IDLE: Color = Color::DarkGray;

const DEBOUNCE: Duration = Duration::from_millis(300);

const NAME_WIDTH: usize = 22;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Pane {
    Search,
    Results,
    Coins,
    Wallets,
    Settings,
    Palette,
}

const PANES: [Pane; 6] = [
    Pane::Search,
    Pane::Results,
    Pane::Coins,
    Pane::Wallets,
    Pane::Settings,
    Pane::Palette,
];

impl Pane {
    fn title(self) -> &'static str {
        match self {
            Pane::Search => "Search",
            Pane::Results => "Results",
            Pane::Coins => "Selected coins",
            Pane::Wallets => "Wallets",
            Pane::Settings => "Settings",
            Pane::Palette => "Palette",
        }
    }

    fn index(self) -> usize {
        PANES.iter().position(|p| *p == self).unwrap_or(0)
    }

    fn next(self) -> Pane {
        PANES[(self.index() + 1) % PANES.len()]
    }

    fn prev(self) -> Pane {
        PANES[(self.index() + PANES.len() - 1) % PANES.len()]
    }

    fn hints(self) -> &'static str {
        match self {
            Pane::Search => "type to search   ↓ results   Esc clear",
            Pane::Results => "↑↓ move   Enter add coin",
            Pane::Coins => "↑↓ move   Del remove",
            Pane::Wallets => "↑↓ move   Enter add   Del remove",
            Pane::Settings => "↑↓ move   Enter edit/cycle   ←→ cycle",
            Pane::Palette => "↑↓ move   ←→ cycle color   Enter hex",
        }
    }
}

const NAMED_COLORS: [&str; 16] = [
    "black",
    "red",
    "green",
    "yellow",
    "blue",
    "magenta",
    "cyan",
    "gray",
    "darkgray",
    "lightred",
    "lightgreen",
    "lightyellow",
    "lightblue",
    "lightmagenta",
    "lightcyan",
    "white",
];

fn cycle_color(current: &str, delta: i32) -> String {
    let resolved = crate::config::parse_color(current);
    let index = resolved
        .and_then(|c| {
            NAMED_COLORS
                .iter()
                .position(|name| crate::config::parse_color(name) == Some(c))
        })
        .unwrap_or(0);

    let len = NAMED_COLORS.len() as i32;
    let next = (index as i32 + delta).rem_euclid(len) as usize;
    NAMED_COLORS[next].to_string()
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Setting {
    CacheTtl,
    ApiKey,
    Chart,
    Minimal,
}

const SETTINGS: [Setting; 4] = [
    Setting::CacheTtl,
    Setting::ApiKey,
    Setting::Chart,
    Setting::Minimal,
];

impl Setting {

    fn label(self) -> &'static str {
        match self {
            Setting::CacheTtl => "Cache TTL",
            Setting::ApiKey => "API key",
            Setting::Chart => "Chart",
            Setting::Minimal => "Minimal",
        }
    }

    fn secret(self) -> bool {
        matches!(self, Setting::ApiKey)
    }

    fn is_toggle(self) -> bool {
        matches!(self, Setting::Chart | Setting::Minimal)
    }

    fn raw(self, cfg: &Config) -> String {
        match self {
            Setting::CacheTtl => cfg.cache_ttl_secs.to_string(),
            Setting::ApiKey => cfg.api_key().unwrap_or_default().to_string(),
            Setting::Chart | Setting::Minimal => String::new(),
        }
    }

    fn display(self, cfg: &Config) -> String {
        match self {
            Setting::CacheTtl => format!("{}s", cfg.cache_ttl_secs),
            Setting::ApiKey => match cfg.api_key() {
                Some(key) => {
                    let len = key.chars().count();
                    format!("{} ({len})", "•".repeat(len))
                }
                None => "not set — free tier".to_string(),
            },
            Setting::Chart => cfg.chart_render.name().to_string(),
            Setting::Minimal => if cfg.chart_minimal { "on" } else { "off" }.to_string(),
        }
    }

    fn cycle(self, cfg: &mut Config, delta: i32) {
        match self {
            Setting::Chart => cfg.chart_render = cfg.chart_render.step(delta),
            Setting::Minimal => cfg.chart_minimal = !cfg.chart_minimal,
            Setting::CacheTtl | Setting::ApiKey => {}
        }
    }

    fn apply(self, cfg: &mut Config, input: &str) -> std::result::Result<(), String> {
        match self {
            Setting::ApiKey => {
                cfg.coingecko_api_key = Some(input.to_string());
                cfg.normalize_api_key();
            }
            Setting::CacheTtl => {
                let secs: u64 = input
                    .trim()
                    .parse()
                    .map_err(|_| format!("`{}` is not a whole number of seconds", input.trim()))?;
                if secs < MIN_CACHE_TTL_SECS {
                    return Err(format!("cache TTL must be at least {MIN_CACHE_TTL_SECS}s"));
                }
                cfg.cache_ttl_secs = secs;
            }

            Setting::Chart | Setting::Minimal => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum EditTarget {
    Setting(Setting),
    NewWallet,

    PaletteSlot(usize),
}

impl EditTarget {
    fn secret(self) -> bool {
        matches!(self, EditTarget::Setting(s) if s.secret())
    }

    fn title(self) -> String {
        match self {
            EditTarget::Setting(s) => format!("Edit {}", s.label().to_lowercase()),
            EditTarget::NewWallet => "New wallet address".to_string(),
            EditTarget::PaletteSlot(_) => "Edit color (name or #rrggbb)".to_string(),
        }
    }
}

struct Editor {
    target: EditTarget,
    buffer: String,
}

type SearchOutcome = std::result::Result<Vec<SearchHit>, String>;

struct App {
    config: Config,
    pane: Pane,

    query: String,
    results: Vec<SearchHit>,

    results_idx: usize,
    coins_idx: usize,
    wallets_idx: usize,
    settings_idx: usize,
    palette_idx: usize,

    editor: Option<Editor>,

    search_due: Option<Instant>,

    search_seq: u64,
    searching: bool,

    status: String,
    dirty: bool,
    should_quit: bool,

    graphics: bool,

    keys: Box<dyn KeyStore>,
}

impl App {

    fn new(config: Config, warnings: &[String], graphics: bool, keys: Box<dyn KeyStore>) -> Self {
        let status = match warnings.first() {
            Some(first) if warnings.len() > 1 => {
                format!("{first} (+{} more)", warnings.len() - 1)
            }
            Some(first) => first.clone(),
            None => "Type to look up a coin".to_string(),
        };

        App {
            config,
            pane: Pane::Search,
            query: String::new(),
            results: Vec::new(),
            results_idx: 0,
            coins_idx: 0,
            wallets_idx: 0,
            settings_idx: 0,
            palette_idx: 0,
            editor: None,
            search_due: None,
            search_seq: 0,
            searching: false,
            status,
            dirty: false,
            should_quit: false,
            graphics,
            keys,
        }
    }

    fn schedule_search(&mut self, now: Instant) {
        if self.query.trim().is_empty() {
            self.search_due = None;
            self.results.clear();
            self.results_idx = 0;
            self.searching = false;
            return;
        }
        self.search_due = Some(now + DEBOUNCE);
    }

    fn take_due_search(&mut self, now: Instant) -> Option<(u64, String)> {
        let due = self.search_due?;
        if now < due {
            return None;
        }
        self.search_due = None;
        self.search_seq += 1;
        self.searching = true;
        Some((self.search_seq, self.query.trim().to_string()))
    }

    fn apply_search(&mut self, seq: u64, outcome: SearchOutcome) {
        if seq != self.search_seq {
            return;
        }
        self.searching = false;

        match outcome {
            Ok(hits) => {
                self.status = if hits.is_empty() {
                    format!("nothing found for `{}`", self.query.trim())
                } else {
                    format!("{} results for `{}`", hits.len(), self.query.trim())
                };
                self.results = hits;
                self.results_idx = 0;
            }
            Err(err) => {
                self.status = format!("search failed: {err}");
                self.results.clear();
                self.results_idx = 0;
            }
        }
    }

    fn begin_edit(&mut self, target: EditTarget) {
        let buffer = match target {
            EditTarget::Setting(s) => s.raw(&self.config),
            EditTarget::NewWallet => String::new(),
            EditTarget::PaletteSlot(i) => self.config.palette.get(i).cloned().unwrap_or_default(),
        };
        self.editor = Some(Editor { target, buffer });
        self.status = "Enter confirm   Esc cancel".to_string();
    }

    fn commit_edit(&mut self, editor: Editor) {
        let outcome = match editor.target {
            EditTarget::Setting(s) => s.apply(&mut self.config, &editor.buffer),
            EditTarget::NewWallet => self.add_wallet(&editor.buffer),
            EditTarget::PaletteSlot(i) => self.apply_palette_slot(i, &editor.buffer),
        };

        match outcome {

            Ok(()) if editor.target == EditTarget::Setting(Setting::ApiKey) => {
                self.persist_api_key();
            }
            Ok(()) => {
                self.dirty = true;
                self.status = "changed — Ctrl+S to save".to_string();
            }
            Err(err) => {

                self.status = err;
                self.editor = Some(editor);
            }
        }
    }

    fn persist_api_key(&mut self) {
        let outcome = match self.config.api_key() {
            Some(key) => self
                .keys
                .store(key)
                .map(|()| format!("api key saved to the {} keyring", secret::SERVICE)),
            None => self
                .keys
                .clear()
                .map(|()| format!("api key removed from the {} keyring", secret::SERVICE)),
        };

        self.status = match outcome {
            Ok(done) => done,

            Err(err) => format!(
                "could not reach the system keyring ({err}) — the key applies to this session \
                 only and is gone on the next run"
            ),
        };
    }

    fn add_selected_coin(&mut self) {
        let Some(hit) = self.results.get(self.results_idx) else {
            return;
        };
        if self.config.default_coins.contains(&hit.id) {
            self.status = format!("{} is already selected", hit.id);
            return;
        }

        if self.config.default_coins.len() >= MAX_COINS {
            self.status = format!("at most {MAX_COINS} coins — remove one first");
            return;
        }

        self.status = format!("added {}", hit.id);
        self.config.default_coins.push(hit.id.clone());
        self.dirty = true;
    }

    fn remove_selected_coin(&mut self) {
        if self.coins_idx >= self.config.default_coins.len() {
            return;
        }
        let removed = self.config.default_coins.remove(self.coins_idx);
        self.coins_idx = clamp_index(self.coins_idx, self.config.default_coins.len());

        self.palette_idx = clamp_index(self.palette_idx, self.config.default_coins.len());
        self.status = format!("removed {removed}");
        self.dirty = true;
    }

    fn add_wallet(&mut self, input: &str) -> std::result::Result<(), String> {
        let address = input.trim();
        if address.is_empty() {
            return Err("a wallet address cannot be empty".to_string());
        }
        if self.config.wallet_addresses.iter().any(|a| a == address) {
            return Err(format!("{address} is already saved"));
        }
        self.config.wallet_addresses.push(address.to_string());
        Ok(())
    }

    fn remove_selected_wallet(&mut self) {
        if self.wallets_idx >= self.config.wallet_addresses.len() {
            return;
        }
        let removed = self.config.wallet_addresses.remove(self.wallets_idx);
        self.wallets_idx = clamp_index(self.wallets_idx, self.config.wallet_addresses.len());
        self.status = format!("removed {removed}");
        self.dirty = true;
    }

    fn save(&mut self) {
        match self.config.save() {
            Ok(path) => {
                self.dirty = false;
                self.status = format!("saved to {}", path.display());
            }
            Err(err) => self.status = format!("could not save: {err}"),
        }
    }

    fn sync_palette_len(&mut self) {
        let needed = self.config.default_coins.len();
        while self.config.palette.len() < needed {
            let next = cycle_color(
                self.config
                    .palette
                    .last()
                    .map(String::as_str)
                    .unwrap_or("white"),
                1,
            );
            self.config.palette.push(next);
        }
    }

    fn cycle_selected_palette_color(&mut self, delta: i32) {
        self.sync_palette_len();
        let Some(current) = self.config.palette.get(self.palette_idx).cloned() else {
            return;
        };
        self.config.palette[self.palette_idx] = cycle_color(&current, delta);
        self.dirty = true;
    }

    fn apply_palette_slot(&mut self, index: usize, input: &str) -> std::result::Result<(), String> {
        let color = input.trim();
        if crate::config::parse_color(color).is_none() {
            return Err(format!("`{color}` is not a valid color (name or #rrggbb)"));
        }

        let Some(slot) = self.config.palette.get_mut(index) else {
            return Err("that color slot no longer exists".to_string());
        };
        *slot = color.to_string();
        Ok(())
    }

    fn on_key(&mut self, key: KeyEvent, now: Instant) {
        if key.kind != KeyEventKind::Press {
            return;
        }

        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);

        if ctrl && matches!(key.code, KeyCode::Char('c')) {
            self.should_quit = true;
            return;
        }

        if ctrl && matches!(key.code, KeyCode::Char('s')) {
            self.save();
            return;
        }

        if let Some(editor) = self.editor.take() {
            self.on_key_editing(key, editor);
            return;
        }

        match key.code {
            KeyCode::Tab => {
                self.pane = self.pane.next();
                if self.pane == Pane::Palette {
                    self.sync_palette_len();
                }
                return;
            }
            KeyCode::BackTab => {
                self.pane = self.pane.prev();
                if self.pane == Pane::Palette {
                    self.sync_palette_len();
                }
                return;
            }
            _ => {}
        }

        match self.pane {
            Pane::Search => self.on_key_search(key, now),
            Pane::Results => self.on_key_results(key),
            Pane::Coins => self.on_key_coins(key),
            Pane::Wallets => self.on_key_wallets(key),
            Pane::Settings => self.on_key_settings(key),
            Pane::Palette => self.on_key_palette(key),
        }
    }

    fn on_key_editing(&mut self, key: KeyEvent, mut editor: Editor) {
        match key.code {
            KeyCode::Esc => self.status = "edit cancelled".to_string(),
            KeyCode::Enter => self.commit_edit(editor),
            KeyCode::Backspace => {
                editor.buffer.pop();
                self.editor = Some(editor);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                editor.buffer.push(c);
                self.editor = Some(editor);
            }
            _ => self.editor = Some(editor),
        }
    }

    fn on_key_search(&mut self, key: KeyEvent, now: Instant) {
        match key.code {

            KeyCode::Esc => {
                if self.query.is_empty() {
                    self.should_quit = true;
                } else {
                    self.query.clear();
                    self.schedule_search(now);
                    self.status = "search cleared".to_string();
                }
            }
            KeyCode::Down | KeyCode::Enter if !self.results.is_empty() => {
                self.pane = Pane::Results;
            }
            KeyCode::Backspace => {
                self.query.pop();
                self.schedule_search(now);
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.query.push(c);
                self.schedule_search(now);
            }
            _ => {}
        }
    }

    fn on_key_results(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up => {

                if self.results_idx == 0 {
                    self.pane = Pane::Search;
                } else {
                    self.results_idx -= 1;
                }
            }
            KeyCode::Down => {
                self.results_idx = step_down(self.results_idx, self.results.len());
            }
            KeyCode::Enter => self.add_selected_coin(),
            _ => {}
        }
    }

    fn on_key_coins(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up => self.coins_idx = self.coins_idx.saturating_sub(1),
            KeyCode::Down => {
                self.coins_idx = step_down(self.coins_idx, self.config.default_coins.len());
            }
            KeyCode::Delete | KeyCode::Backspace => self.remove_selected_coin(),
            _ => {}
        }
    }

    fn on_key_wallets(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up => self.wallets_idx = self.wallets_idx.saturating_sub(1),
            KeyCode::Down => {
                self.wallets_idx = step_down(self.wallets_idx, self.config.wallet_addresses.len());
            }
            KeyCode::Delete | KeyCode::Backspace => self.remove_selected_wallet(),
            KeyCode::Enter | KeyCode::Char('a') => self.begin_edit(EditTarget::NewWallet),
            _ => {}
        }
    }

    fn on_key_settings(&mut self, key: KeyEvent) {
        let setting = SETTINGS[self.settings_idx.min(SETTINGS.len() - 1)];

        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up => self.settings_idx = self.settings_idx.saturating_sub(1),
            KeyCode::Down => self.settings_idx = step_down(self.settings_idx, SETTINGS.len()),

            KeyCode::Enter => {
                if setting.is_toggle() {
                    self.cycle_setting(setting, 1);
                } else {
                    self.begin_edit(EditTarget::Setting(setting));
                }
            }

            KeyCode::Left if setting.is_toggle() => self.cycle_setting(setting, -1),
            KeyCode::Right if setting.is_toggle() => self.cycle_setting(setting, 1),
            _ => {}
        }
    }

    fn cycle_setting(&mut self, setting: Setting, delta: i32) {

        if setting == Setting::Chart
            && !self.graphics
            && self.config.chart_render.step(delta).needs_graphics()
        {
            self.status = "lines needs a terminal with kitty or sixel graphics \u{2014} \
                           this one has neither, so the chart style is unchanged \
                           (steps draws the same stroke in glyphs)"
                .to_string();
            return;
        }

        setting.cycle(&mut self.config, delta);
        self.dirty = true;
        self.status = format!(
            "{} → {}   — Ctrl+S to save",
            setting.label().to_lowercase(),
            setting.display(&self.config)
        );
    }

    fn on_key_palette(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Up => self.palette_idx = self.palette_idx.saturating_sub(1),
            KeyCode::Down => {
                self.palette_idx = step_down(self.palette_idx, self.config.default_coins.len());
            }
            KeyCode::Left => self.cycle_selected_palette_color(-1),
            KeyCode::Right => self.cycle_selected_palette_color(1),
            KeyCode::Enter => {
                self.sync_palette_len();
                self.begin_edit(EditTarget::PaletteSlot(self.palette_idx));
            }
            _ => {}
        }
    }
}

fn step_down(index: usize, len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (index + 1).min(len - 1)
    }
}

fn clamp_index(index: usize, len: usize) -> usize {
    if len == 0 { 0 } else { index.min(len - 1) }
}

fn truncate(text: &str, width: usize) -> String {
    if text.chars().count() <= width {
        return text.to_string();
    }
    text.chars()
        .take(width.saturating_sub(1))
        .collect::<String>()
        + "…"
}

enum Msg {
    Input(Event),
    Search { seq: u64, outcome: SearchOutcome },
}

pub async fn run(
    config: Config,
    gecko: CoinGecko,
    mut warnings: Vec<String>,
    graphics: bool,
) -> Result<()> {

    let keys = secret::Keyring;
    if let Err(err) = keys.load() {
        warnings.insert(
            0,
            format!(
                "the system keyring is unavailable ({err}) — an API key set here cannot be kept"
            ),
        );
    }

    let mut terminal = ratatui::try_init()
        .map_err(|e| Error::msg(format!("cannot start the config screen: {e}")))?;
    let outcome = event_loop(
        &mut terminal,
        config,
        gecko,
        &warnings,
        graphics,
        Box::new(keys),
    )
    .await;
    ratatui::restore();
    outcome
}

async fn event_loop(
    terminal: &mut ratatui::DefaultTerminal,
    config: Config,
    gecko: CoinGecko,
    warnings: &[String],
    graphics: bool,
    keys: Box<dyn KeyStore>,
) -> Result<()> {
    let mut client_key = config.api_key().map(str::to_string);
    let mut client_ttl = config.cache_ttl_secs;
    let mut gecko = Arc::new(gecko);
    let mut app = App::new(config, warnings, graphics, keys);

    let (tx, mut rx) = mpsc::unbounded_channel::<Msg>();

    {
        let tx = tx.clone();
        std::thread::spawn(move || {
            while let Ok(event) = event::read() {
                if tx.send(Msg::Input(event)).is_err() {
                    break;
                }
            }
        });
    }

    while !app.should_quit {
        terminal.draw(|frame| render(&app, frame.area(), frame.buffer_mut()))?;

        let due = app.search_due;
        tokio::select! {
            message = rx.recv() => {
                let Some(message) = message else { break };
                match message {
                    Msg::Input(Event::Key(key)) => app.on_key(key, Instant::now()),
                    Msg::Input(_) => {}
                    Msg::Search { seq, outcome } => app.apply_search(seq, outcome),
                }
            }
            () = sleep_until(due), if due.is_some() => {
                if let Some((seq, query)) = app.take_due_search(Instant::now()) {
                    let gecko = Arc::clone(&gecko);
                    let tx = tx.clone();
                    tokio::spawn(async move {
                        let outcome = gecko.search(&query).await.map_err(|e| e.to_string());
                        let _ = tx.send(Msg::Search { seq, outcome });
                    });
                }
            }
        }

        let current_key = app.config.api_key().map(str::to_string);
        let current_ttl = app.config.cache_ttl_secs;
        if (current_key != client_key || current_ttl != client_ttl)
            && let Ok(fresh) = coingecko::client(current_key.as_deref(), current_ttl)
        {
            gecko = Arc::new(fresh);
            client_key = current_key;
            client_ttl = current_ttl;
        }
    }

    Ok(())
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(tokio::time::Instant::from_std(at)).await,
        None => std::future::pending().await,
    }
}

fn pane_block(title: &str, focused: bool) -> Block<'static> {
    let color = if focused { ACCENT } else { IDLE };
    let title_style = if focused {
        Style::default().fg(ACCENT).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(IDLE)
    };

    Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(format!(" {title} "), title_style))
}

fn render(app: &App, area: Rect, buf: &mut Buffer) {

    let [search_area, body, status_area] = Layout::vertical([
        Constraint::Length(3),
        Constraint::Min(0),
        Constraint::Length(2),
    ])
    .areas(area);

    let [left, right] =
        Layout::horizontal([Constraint::Percentage(50), Constraint::Percentage(50)]).areas(body);

    let settings_height = SETTINGS.len() as u16 + 2;

    let [coins_area, wallets_area, settings_area, palette_area] = Layout::vertical([
        Constraint::Min(3),
        Constraint::Min(3),
        Constraint::Length(settings_height),
        Constraint::Min(3),
    ])
    .areas(right);

    render_search(app, search_area, buf);
    render_results(app, left, buf);
    render_coins(app, coins_area, buf);
    render_wallets(app, wallets_area, buf);
    render_settings(app, settings_area, buf);
    render_palette(app, palette_area, buf);
    render_status(app, status_area, buf);

    if let Some(editor) = &app.editor {
        render_editor(editor, body, buf);
    }
}

fn render_search(app: &App, area: Rect, buf: &mut Buffer) {
    let focused = app.pane == Pane::Search && app.editor.is_none();

    let title = if app.searching {
        format!("{} — searching…", Pane::Search.title())
    } else {
        Pane::Search.title().to_string()
    };

    let spans = if focused {
        vec![
            Span::raw(app.query.clone()),
            Span::styled("█", Style::default().fg(ACCENT)),
        ]
    } else if app.query.is_empty() {
        vec![Span::styled(
            "Tab here and type a coin name",
            Style::default().fg(IDLE),
        )]
    } else {
        vec![Span::raw(app.query.clone())]
    };

    Paragraph::new(Line::from(spans))
        .block(pane_block(&title, focused))
        .render(area, buf);
}

fn render_list(
    items: Vec<ListItem<'static>>,
    title: &str,
    focused: bool,
    selected: usize,
    area: Rect,
    buf: &mut Buffer,
) {
    let empty = items.is_empty();
    let items = if empty {
        vec![ListItem::new(Line::from(Span::styled(
            "— empty —",
            Style::default().fg(IDLE),
        )))]
    } else {
        items
    };

    let highlight = if focused {
        Style::default()
            .fg(Color::Black)
            .bg(ACCENT)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().add_modifier(Modifier::BOLD)
    };

    let mut state = ListState::default();
    if !empty {
        state.select(Some(selected));
    }

    StatefulWidget::render(
        List::new(items)
            .block(pane_block(title, focused))
            .highlight_style(highlight)
            .highlight_symbol(if focused { "› " } else { "  " }),
        area,
        buf,
        &mut state,
    );
}

fn render_results(app: &App, area: Rect, buf: &mut Buffer) {
    let items: Vec<ListItem<'static>> = app
        .results
        .iter()
        .map(|hit| {
            let rank = hit
                .market_cap_rank
                .map(|r| format!("#{r}"))
                .unwrap_or_else(|| "—".to_string());

            ListItem::new(Line::from(vec![
                Span::raw(format!(
                    "{:<width$}",
                    truncate(&hit.name, NAME_WIDTH),
                    width = NAME_WIDTH
                )),
                Span::styled(format!("{:<8}", hit.symbol), Style::default().fg(ACCENT)),
                Span::styled(format!("{rank:>5}"), Style::default().fg(IDLE)),
            ]))
        })
        .collect();

    render_list(
        items,
        Pane::Results.title(),
        app.pane == Pane::Results && app.editor.is_none(),
        app.results_idx,
        area,
        buf,
    );
}

fn render_coins(app: &App, area: Rect, buf: &mut Buffer) {
    let items: Vec<ListItem<'static>> = app
        .config
        .default_coins
        .iter()
        .map(|id| ListItem::new(Line::raw(id.clone())))
        .collect();

    let title = format!(
        "{} ({}/{})",
        Pane::Coins.title(),
        app.config.default_coins.len(),
        MAX_COINS
    );

    render_list(
        items,
        &title,
        app.pane == Pane::Coins && app.editor.is_none(),
        app.coins_idx,
        area,
        buf,
    );
}

fn render_wallets(app: &App, area: Rect, buf: &mut Buffer) {
    let items: Vec<ListItem<'static>> = app
        .config
        .wallet_addresses
        .iter()
        .map(|address| ListItem::new(Line::raw(address.clone())))
        .collect();

    render_list(
        items,
        Pane::Wallets.title(),
        app.pane == Pane::Wallets && app.editor.is_none(),
        app.wallets_idx,
        area,
        buf,
    );
}

fn render_settings(app: &App, area: Rect, buf: &mut Buffer) {
    let items: Vec<ListItem<'static>> = SETTINGS
        .iter()
        .map(|setting| {
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{:<10}", setting.label()),
                    Style::default().fg(IDLE),
                ),
                Span::raw(setting.display(&app.config)),

                Span::styled(
                    match setting {
                        Setting::Chart
                            if !app.graphics && app.config.chart_render.needs_graphics() =>
                        {
                            "  (no graphics — drawn as steps)"
                        }
                        _ => "",
                    },
                    Style::default().fg(IDLE),
                ),
            ]))
        })
        .collect();

    render_list(
        items,
        Pane::Settings.title(),
        app.pane == Pane::Settings && app.editor.is_none(),
        app.settings_idx,
        area,
        buf,
    );
}

fn render_palette(app: &App, area: Rect, buf: &mut Buffer) {
    let items: Vec<ListItem<'static>> = app
        .config
        .default_coins
        .iter()
        .enumerate()
        .map(|(i, id)| {
            let name = app
                .config
                .palette
                .get(i)
                .cloned()
                .unwrap_or_else(|| "white".to_string());
            let color = crate::config::parse_color(&name).unwrap_or(Color::White);
            ListItem::new(Line::from(vec![
                Span::styled("● ", Style::default().fg(color)),
                Span::raw(id.clone()),
                Span::styled(format!("  {name}"), Style::default().fg(IDLE)),
            ]))
        })
        .collect();

    render_list(
        items,
        Pane::Palette.title(),
        app.pane == Pane::Palette && app.editor.is_none(),
        app.palette_idx,
        area,
        buf,
    );
}

fn render_editor(editor: &Editor, area: Rect, buf: &mut Buffer) {

    let [_, middle, _] = Layout::vertical([
        Constraint::Min(0),
        Constraint::Length(3),
        Constraint::Min(0),
    ])
    .areas(area);
    let [_, box_area, _] = Layout::horizontal([
        Constraint::Percentage(10),
        Constraint::Percentage(80),
        Constraint::Percentage(10),
    ])
    .areas(middle);

    let shown = if editor.target.secret() {
        "•".repeat(editor.buffer.chars().count())
    } else {
        editor.buffer.clone()
    };

    Clear.render(box_area, buf);
    Paragraph::new(Line::from(vec![
        Span::raw(shown),
        Span::styled("█", Style::default().fg(ACCENT)),
    ]))
    .block(pane_block(&editor.target.title(), true))
    .render(box_area, buf);
}

fn render_status(app: &App, area: Rect, buf: &mut Buffer) {
    let [message_area, hint_area] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(area);
    let [message_area, dirty_area] =
        Layout::horizontal([Constraint::Min(0), Constraint::Length(10)]).areas(message_area);

    Paragraph::new(Line::from(Span::styled(
        app.status.clone(),
        Style::default().fg(ACCENT),
    )))
    .render(message_area, buf);

    if app.dirty {
        Paragraph::new(Line::from(Span::styled(
            "● unsaved",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        )))
        .render(dirty_area, buf);
    }

    let hints = match &app.editor {
        Some(_) => "Enter confirm   Esc cancel".to_string(),
        None => format!("Tab pane   {}   Ctrl+S save   q quit", app.pane.hints()),
    };
    Paragraph::new(Line::from(Span::styled(hints, Style::default().fg(IDLE))))
        .render(hint_area, buf);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ChartRender;
    use ratatui::Terminal;
    use ratatui::backend::TestBackend;

    fn app() -> App {
        App::new(
            Config::default(),
            &[],
            true,
            Box::new(secret::MemoryStore::empty()),
        )
    }

    #[test]
    fn a_complaint_from_loading_the_config_greets_the_user() {

        let app = App::new(
            Config::default(),
            &["config.toml is not valid TOML, using defaults".to_string()],
            true,
            Box::new(secret::MemoryStore::empty()),
        );
        assert!(app.status.contains("not valid TOML"), "{}", app.status);

        let app = App::new(
            Config::default(),
            &["first".into(), "second".into()],
            true,
            Box::new(secret::MemoryStore::empty()),
        );
        assert!(app.status.contains("first"), "{}", app.status);
        assert!(app.status.contains("+1 more"), "{}", app.status);
    }

    fn press(app: &mut App, code: KeyCode) {
        app.on_key(KeyEvent::from(code), Instant::now());
    }

    fn hit(id: &str, name: &str, rank: Option<u32>) -> SearchHit {
        SearchHit {
            id: id.to_string(),
            name: name.to_string(),
            symbol: id.chars().take(3).collect::<String>().to_uppercase(),
            market_cap_rank: rank,
        }
    }

    fn render_to_string(app: &App, width: u16, height: u16) -> String {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        terminal
            .draw(|frame| render(app, frame.area(), frame.buffer_mut()))
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
    fn tab_cycles_forward_and_shift_tab_backward_through_every_pane() {
        let mut app = app();
        for expected in [
            Pane::Results,
            Pane::Coins,
            Pane::Wallets,
            Pane::Settings,
            Pane::Palette,
            Pane::Search,
        ] {
            press(&mut app, KeyCode::Tab);
            assert_eq!(app.pane, expected);
        }

        press(&mut app, KeyCode::BackTab);
        assert_eq!(app.pane, Pane::Palette);
    }

    #[test]
    fn arrow_keys_move_inside_a_pane_without_running_off_the_end() {
        let mut app = app();
        app.pane = Pane::Coins;
        assert_eq!(app.config.default_coins.len(), 3);

        for _ in 0..5 {
            press(&mut app, KeyCode::Down);
        }
        assert_eq!(app.coins_idx, 2, "must stop at the last row");

        for _ in 0..5 {
            press(&mut app, KeyCode::Up);
        }
        assert_eq!(app.coins_idx, 0);
    }

    #[test]
    fn arrowing_up_off_the_results_list_returns_to_the_search_box() {
        let mut app = app();
        app.results = vec![hit("bitcoin", "Bitcoin", Some(1))];
        app.pane = Pane::Results;

        press(&mut app, KeyCode::Up);
        assert_eq!(app.pane, Pane::Search);
    }

    #[test]
    fn a_burst_of_keystrokes_schedules_exactly_one_search() {
        let mut app = app();
        let start = Instant::now();

        for c in "bit".chars() {
            app.on_key(KeyEvent::from(KeyCode::Char(c)), start);
        }
        assert_eq!(app.query, "bit");

        assert!(
            app.take_due_search(start + Duration::from_millis(299))
                .is_none()
        );

        let (seq, query) = app
            .take_due_search(start + DEBOUNCE)
            .expect("the search is due");
        assert_eq!(query, "bit");
        assert_eq!(seq, 1);

        assert!(
            app.take_due_search(start + Duration::from_secs(5))
                .is_none()
        );
    }

    #[test]
    fn each_keystroke_pushes_the_deadline_out() {
        let mut app = app();
        let start = Instant::now();

        app.on_key(KeyEvent::from(KeyCode::Char('b')), start);
        let later = start + Duration::from_millis(200);
        app.on_key(KeyEvent::from(KeyCode::Char('i')), later);

        assert!(app.take_due_search(start + DEBOUNCE).is_none());
        assert!(app.take_due_search(later + DEBOUNCE).is_some());
    }

    #[test]
    fn emptying_the_query_cancels_the_search_and_clears_the_results() {
        let mut app = app();
        let now = Instant::now();

        app.on_key(KeyEvent::from(KeyCode::Char('b')), now);
        app.results = vec![hit("bitcoin", "Bitcoin", Some(1))];

        app.on_key(KeyEvent::from(KeyCode::Backspace), now);
        assert!(app.query.is_empty());
        assert!(app.search_due.is_none(), "no request for an empty query");
        assert!(app.results.is_empty());
    }

    #[test]
    fn a_late_result_for_a_superseded_query_is_ignored() {
        let mut app = app();
        let start = Instant::now();

        app.on_key(KeyEvent::from(KeyCode::Char('b')), start);
        let (first, _) = app.take_due_search(start + DEBOUNCE).expect("first search");

        app.on_key(KeyEvent::from(KeyCode::Char('t')), start + DEBOUNCE);
        let (second, query) = app
            .take_due_search(start + DEBOUNCE * 2)
            .expect("second search");
        assert_eq!(query, "bt");
        assert!(second > first);

        app.apply_search(second, Ok(vec![hit("bitcoin", "Bitcoin", Some(1))]));
        app.apply_search(first, Ok(vec![hit("wrong", "Wrong Coin", Some(9))]));

        assert_eq!(app.results.len(), 1);
        assert_eq!(app.results[0].id, "bitcoin");
    }

    #[test]
    fn a_failed_search_is_reported_rather_than_swallowed() {
        let mut app = app();
        app.search_seq = 3;
        app.searching = true;
        app.apply_search(3, Err("rate limited by CoinGecko".to_string()));

        assert!(app.status.contains("rate limited"), "{}", app.status);
        assert!(!app.searching);
    }

    #[test]
    fn enter_on_a_result_adds_the_coin_to_the_config() {
        let mut app = app();
        app.results = vec![hit("cardano", "Cardano", Some(10))];
        app.pane = Pane::Results;

        press(&mut app, KeyCode::Enter);

        assert!(app.config.default_coins.contains(&"cardano".to_string()));
        assert!(app.dirty);
    }

    #[test]
    fn adding_a_coin_that_is_already_selected_changes_nothing() {
        let mut app = app();
        app.results = vec![hit("bitcoin", "Bitcoin", Some(1))];
        app.pane = Pane::Results;

        press(&mut app, KeyCode::Enter);

        assert_eq!(app.config.default_coins.len(), 3, "no duplicate row");
        assert!(app.status.contains("already"), "{}", app.status);
        assert!(!app.dirty);
    }

    #[test]
    fn refuses_to_grow_the_list_past_what_a_run_would_accept() {
        let mut app = app();
        app.config.default_coins = (0..MAX_COINS).map(|i| format!("coin{i}")).collect();
        app.results = vec![hit("cardano", "Cardano", Some(10))];
        app.pane = Pane::Results;

        press(&mut app, KeyCode::Enter);

        assert_eq!(app.config.default_coins.len(), MAX_COINS);
        assert!(
            app.status.contains(&MAX_COINS.to_string()),
            "{}",
            app.status
        );
    }

    #[test]
    fn delete_and_backspace_both_remove_the_selected_coin() {
        let mut app = app();
        app.pane = Pane::Coins;

        press(&mut app, KeyCode::Delete);
        assert_eq!(app.config.default_coins, vec!["ethereum", "solana"]);

        press(&mut app, KeyCode::Backspace);
        assert_eq!(app.config.default_coins, vec!["solana"]);
        assert!(app.dirty);
    }

    #[test]
    fn removing_the_last_row_pulls_the_selection_back_into_range() {
        let mut app = app();
        app.pane = Pane::Coins;
        app.coins_idx = 2;

        press(&mut app, KeyCode::Delete);
        assert_eq!(app.config.default_coins.len(), 2);
        assert_eq!(app.coins_idx, 1);

        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Delete);
        assert!(app.config.default_coins.is_empty());
        assert_eq!(app.coins_idx, 0);
    }

    #[test]
    fn a_wallet_can_be_added_and_removed() {
        let mut app = app();
        app.pane = Pane::Wallets;

        press(&mut app, KeyCode::Enter);
        for c in "bc1qexample".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.config.wallet_addresses, vec!["bc1qexample"]);
        assert!(app.editor.is_none());

        press(&mut app, KeyCode::Delete);
        assert!(app.config.wallet_addresses.is_empty());
    }

    #[test]
    fn an_empty_wallet_address_is_rejected_and_the_editor_stays_open() {
        let mut app = app();
        app.pane = Pane::Wallets;

        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Enter);

        assert!(app.config.wallet_addresses.is_empty());
        assert!(app.editor.is_some(), "the user keeps their cursor");
    }

    #[test]
    fn the_cache_ttl_rejects_junk_and_values_below_the_floor() {
        let mut cfg = Config::default();
        assert!(Setting::CacheTtl.apply(&mut cfg, "soon").is_err());
        assert!(Setting::CacheTtl.apply(&mut cfg, "1").is_err());
        assert_eq!(cfg.cache_ttl_secs, 60, "config left alone");

        Setting::CacheTtl.apply(&mut cfg, " 120 ").expect("applies");
        assert_eq!(cfg.cache_ttl_secs, 120);
    }

    fn app_on_setting(setting: Setting) -> App {
        let mut app = app();
        app.pane = Pane::Settings;
        app.settings_idx = SETTINGS
            .iter()
            .position(|s| *s == setting)
            .expect("setting is listed");
        app
    }

    #[test]
    fn enter_steps_the_chart_settings_instead_of_opening_an_editor() {
        let mut app = app_on_setting(Setting::Chart);
        assert_eq!(app.config.chart_render, ChartRender::Steps);

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.chart_render, ChartRender::Lines);
        assert!(app.editor.is_none(), "a toggle must not open the editor");
        assert!(app.dirty);

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.chart_render, ChartRender::Blocks);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.chart_render, ChartRender::Dots);
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.chart_render, ChartRender::Steps);
    }

    #[test]
    fn left_and_right_step_the_chart_style_in_opposite_directions() {
        let mut app = app_on_setting(Setting::Chart);

        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.chart_render, ChartRender::Lines);
        assert!(app.dirty);

        press(&mut app, KeyCode::Left);
        assert_eq!(app.config.chart_render, ChartRender::Steps);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.config.chart_render, ChartRender::Dots);
    }

    #[test]
    fn left_and_right_toggle_the_chart_settings_too() {
        let mut app = app_on_setting(Setting::Minimal);
        assert!(!app.config.chart_minimal);

        press(&mut app, KeyCode::Right);
        assert!(app.config.chart_minimal);
        assert!(app.dirty);

        press(&mut app, KeyCode::Left);
        assert!(!app.config.chart_minimal);
    }

    #[test]
    fn the_two_chart_settings_are_independent_of_each_other() {
        let mut app = app_on_setting(Setting::Chart);
        press(&mut app, KeyCode::Enter);

        app.settings_idx = SETTINGS
            .iter()
            .position(|s| *s == Setting::Minimal)
            .expect("listed");
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.config.chart_render, ChartRender::Lines);
        assert!(app.config.chart_minimal);
    }

    fn app_on_setting_without_graphics(setting: Setting, render: ChartRender) -> App {
        let mut app = App::new(
            Config {
                chart_render: render,
                ..Config::default()
            },
            &[],
            false,
            Box::new(secret::MemoryStore::empty()),
        );
        app.pane = Pane::Settings;
        app.settings_idx = SETTINGS
            .iter()
            .position(|s| *s == setting)
            .expect("setting is listed");
        app
    }

    #[test]
    fn lines_is_refused_on_a_terminal_that_cannot_draw_it() {

        let mut app = app_on_setting_without_graphics(Setting::Chart, ChartRender::Steps);

        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.chart_render, ChartRender::Steps, "value changed");
        assert!(!app.dirty, "a refused selection is not an unsaved change");
        assert!(app.status.contains("graphics"), "{}", app.status);
        assert!(app.status.contains("lines"), "{}", app.status);

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.chart_render, ChartRender::Steps);
        assert!(app.editor.is_none(), "a toggle must not open the editor");
        assert!(!app.dirty);
    }

    #[test]
    fn stepping_backwards_onto_lines_is_refused_too() {

        let mut app = app_on_setting_without_graphics(Setting::Chart, ChartRender::Blocks);
        press(&mut app, KeyCode::Left);
        assert_eq!(app.config.chart_render, ChartRender::Blocks);
        assert!(!app.dirty);
    }

    #[test]
    fn steps_never_needs_a_check_of_its_own() {

        let mut app = app_on_setting_without_graphics(Setting::Chart, ChartRender::Dots);

        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.chart_render, ChartRender::Steps);
        assert!(app.dirty);
        assert!(!app.status.contains("graphics"), "{}", app.status);
    }

    #[test]
    fn the_drawable_styles_still_cycle_where_lines_cannot() {

        let mut app = app_on_setting_without_graphics(Setting::Chart, ChartRender::Steps);

        press(&mut app, KeyCode::Left);
        assert_eq!(app.config.chart_render, ChartRender::Dots);
        assert!(app.dirty);

        press(&mut app, KeyCode::Left);
        assert_eq!(app.config.chart_render, ChartRender::Blocks);

        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.chart_render, ChartRender::Dots);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.chart_render, ChartRender::Steps);
    }

    #[test]
    fn lines_is_selectable_where_it_can_be_drawn() {

        let mut app = app_on_setting(Setting::Chart);
        app.config.chart_render = ChartRender::Steps;

        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.chart_render, ChartRender::Lines);
        assert!(app.dirty);
    }

    #[test]
    fn a_lines_config_written_elsewhere_is_kept_rather_than_reset() {

        let app = app_on_setting_without_graphics(Setting::Chart, ChartRender::Lines);
        assert_eq!(app.config.chart_render, ChartRender::Lines);
        assert!(!app.dirty, "opening the screen is not an edit");

        let rendered = render_to_string(&app, 100, 30);
        assert!(rendered.contains("lines"), "{rendered}");
        assert!(rendered.contains("drawn as steps"), "{rendered}");
    }

    #[test]
    fn a_terminal_with_graphics_says_nothing_extra_about_the_chart_style() {

        let mut app = app_on_setting(Setting::Chart);
        app.config.chart_render = ChartRender::Lines;

        let rendered = render_to_string(&app, 100, 30);
        assert!(rendered.contains("lines"), "{rendered}");
        assert!(!rendered.contains("drawn as steps"), "{rendered}");
    }

    #[test]
    fn the_glyph_styles_say_nothing_extra_either() {

        let app = app_on_setting_without_graphics(Setting::Chart, ChartRender::Steps);
        let rendered = render_to_string(&app, 100, 30);
        assert!(rendered.contains("steps"), "{rendered}");
        assert!(!rendered.contains("drawn as steps"), "{rendered}");
    }

    #[test]
    fn arrow_keys_do_not_toggle_a_setting_that_is_edited_as_text() {
        let mut app = app_on_setting(Setting::CacheTtl);
        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.cache_ttl_secs, 60);
        assert!(!app.dirty, "nothing changed, so nothing is unsaved");
    }

    #[test]
    fn every_setting_row_fits_in_the_pane_the_layout_gives_it() {

        let app = app();
        let rendered = render_to_string(&app, 100, 30);
        for setting in SETTINGS {
            assert!(
                rendered.contains(setting.label()),
                "missing {} in:\n{rendered}",
                setting.label()
            );
        }
    }

    fn app_on_api_key(keys: Box<dyn KeyStore>) -> App {
        let mut app = App::new(Config::default(), &[], true, keys);
        app.pane = Pane::Settings;
        app.settings_idx = SETTINGS
            .iter()
            .position(|s| *s == Setting::ApiKey)
            .expect("setting is listed");
        app
    }

    #[test]
    fn editing_the_api_key_stores_it_and_blanking_it_clears_it() {
        let mut app = app_on_api_key(Box::new(secret::MemoryStore::empty()));

        press(&mut app, KeyCode::Enter);
        for c in "CG-abc".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.api_key(), Some("CG-abc"));

        press(&mut app, KeyCode::Enter);
        for _ in 0..6 {
            press(&mut app, KeyCode::Backspace);
        }
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.config.coingecko_api_key, None);
    }

    #[test]
    fn confirming_the_api_key_puts_it_straight_into_the_keyring() {

        let mut app = app_on_api_key(Box::new(secret::MemoryStore::empty()));

        press(&mut app, KeyCode::Enter);
        for c in "CG-abc".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.keys.load(), Ok(Some("CG-abc".to_string())));
        assert!(!app.dirty, "the key is not an unsaved change to the file");
        assert!(app.status.contains("keyring"), "{}", app.status);
    }

    #[test]
    fn blanking_the_api_key_takes_it_out_of_the_keyring_too() {

        let mut app = app_on_api_key(Box::new(secret::MemoryStore::holding("CG-old")));
        app.config.coingecko_api_key = Some("CG-old".to_string());

        press(&mut app, KeyCode::Enter);
        for _ in 0..6 {
            press(&mut app, KeyCode::Backspace);
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.keys.load(), Ok(None));
        assert_eq!(app.config.coingecko_api_key, None);
    }

    #[test]
    fn a_keyring_that_cannot_be_written_says_so_and_keeps_the_key_for_now() {

        let mut app = app_on_api_key(Box::new(secret::MemoryStore::broken()));

        press(&mut app, KeyCode::Enter);
        for c in "CG-abc".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.config.api_key(), Some("CG-abc"), "usable right now");
        assert!(app.status.contains("keyring"), "{}", app.status);
        assert!(app.status.contains("session only"), "{}", app.status);
        assert!(app.editor.is_none(), "the editor is done, not stuck open");
    }

    #[test]
    fn the_api_key_is_never_echoed_back_to_the_screen() {
        let cfg = Config {
            coingecko_api_key: Some("CG-supersecret".to_string()),
            ..Config::default()
        };

        let shown = Setting::ApiKey.display(&cfg);
        assert!(!shown.contains("supersecret"), "{shown}");
        assert!(shown.starts_with('•'), "{shown}");

        assert!(shown.contains("14"), "{shown}");

        assert_eq!(
            Setting::ApiKey.display(&Config::default()),
            "not set — free tier"
        );
    }

    #[test]
    fn the_api_key_stays_masked_while_it_is_being_typed() {
        let mut app = app();
        app.pane = Pane::Settings;
        app.settings_idx = 1;
        press(&mut app, KeyCode::Enter);
        for c in "CG-secret".chars() {
            press(&mut app, KeyCode::Char(c));
        }

        let rendered = render_to_string(&app, 90, 24);
        assert!(!rendered.contains("CG-secret"), "{rendered}");
        assert!(rendered.contains("•••"), "{rendered}");
    }

    #[test]
    fn q_quits_from_a_list_but_is_literal_text_in_the_search_box() {
        let mut app = app();
        press(&mut app, KeyCode::Char('q'));
        assert!(!app.should_quit);
        assert_eq!(app.query, "q");

        app.pane = Pane::Coins;
        press(&mut app, KeyCode::Char('q'));
        assert!(app.should_quit);
    }

    #[test]
    fn esc_clears_the_query_first_and_only_then_quits() {
        let mut app = app();
        press(&mut app, KeyCode::Char('b'));

        press(&mut app, KeyCode::Esc);
        assert!(app.query.is_empty());
        assert!(!app.should_quit, "the first Esc only clears");

        press(&mut app, KeyCode::Esc);
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_c_quits_even_in_the_middle_of_an_edit() {
        let mut app = app();
        app.pane = Pane::Settings;
        press(&mut app, KeyCode::Enter);
        assert!(app.editor.is_some());

        app.on_key(
            KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL),
            Instant::now(),
        );
        assert!(app.should_quit);
    }

    #[test]
    fn ctrl_s_is_a_save_rather_than_a_typed_character() {
        let mut app = app();
        app.on_key(
            KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL),
            Instant::now(),
        );
        assert!(app.query.is_empty(), "Ctrl+S must not reach the search box");
    }

    #[test]
    fn draws_every_pane_with_its_title() {
        let mut app = app();
        app.results = vec![hit("bitcoin", "Bitcoin", Some(1))];
        app.config.wallet_addresses = vec!["bc1qexample".to_string()];

        let rendered = render_to_string(&app, 100, 26);
        for title in [
            "Search",
            "Results",
            "Selected coins",
            "Wallets",
            "Settings",
            "Palette",
        ] {
            assert!(rendered.contains(title), "missing {title} in:\n{rendered}");
        }

        assert!(rendered.contains("Ctrl+S save"), "{rendered}");
    }

    #[test]
    fn a_search_result_shows_its_name_symbol_and_rank() {
        let mut app = app();
        app.results = vec![hit("bitcoin", "Bitcoin", Some(1))];
        app.pane = Pane::Results;

        let rendered = render_to_string(&app, 100, 26);
        assert!(rendered.contains("Bitcoin"), "{rendered}");
        assert!(rendered.contains("BIT"), "{rendered}");
        assert!(rendered.contains("#1"), "{rendered}");
    }

    #[test]
    fn an_unranked_result_renders_without_a_rank_rather_than_being_dropped() {
        let mut app = app();
        app.results = vec![hit("obscure", "Obscure Coin", None)];
        app.pane = Pane::Results;

        let rendered = render_to_string(&app, 100, 26);
        assert!(rendered.contains("Obscure Coin"), "{rendered}");
    }

    #[test]
    fn renders_without_panicking_in_a_cramped_terminal() {
        let mut app = app();
        app.results = vec![hit("bitcoin", "Bitcoin", Some(1))];
        assert!(!render_to_string(&app, 40, 14).is_empty());
        assert!(!render_to_string(&app, 20, 10).is_empty());
    }

    #[test]
    fn long_names_are_cut_to_fit_instead_of_overflowing_the_column() {
        assert_eq!(truncate("Bitcoin", 22), "Bitcoin");
        let long = truncate(&"x".repeat(40), 22);
        assert_eq!(long.chars().count(), 22);
        assert!(long.ends_with('…'));
    }

    #[test]
    fn left_and_right_cycle_the_selected_coin_through_the_named_colors() {
        let mut app = app();
        app.pane = Pane::Palette;

        press(&mut app, KeyCode::Right);
        assert_eq!(app.config.palette[0], "gray");
        assert!(app.dirty);

        press(&mut app, KeyCode::Left);
        assert_eq!(app.config.palette[0], "cyan");
    }

    #[test]
    fn removing_coins_pulls_the_palette_selection_back_into_range() {
        let mut app = app();
        app.palette_idx = 2;
        app.pane = Pane::Coins;

        press(&mut app, KeyCode::Delete);
        press(&mut app, KeyCode::Delete);

        assert_eq!(app.config.default_coins.len(), 1);
        assert_eq!(app.palette_idx, 0);
    }

    #[test]
    fn entering_the_palette_pane_gives_every_coin_its_own_slot() {
        let mut app = app();
        app.config.default_coins = vec!["a".into(), "b".into(), "c".into(), "d".into(), "e".into()];
        app.config.palette = vec!["red".into(), "green".into()];

        app.sync_palette_len();

        assert_eq!(
            app.config.palette,
            vec!["red", "green", "yellow", "blue", "magenta"]
        );
    }

    #[test]
    fn enter_opens_a_hex_editor_that_validates_the_color() {
        let mut app = app();
        app.pane = Pane::Palette;

        press(&mut app, KeyCode::Enter);
        assert_eq!(app.editor.as_ref().unwrap().buffer, "cyan");

        for _ in 0..4 {
            press(&mut app, KeyCode::Backspace);
        }
        for c in "not-a-color".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.editor.is_some(), "invalid color keeps the editor open");
        assert!(app.status.contains("not a valid color"), "{}", app.status);

        for _ in 0.."not-a-color".chars().count() {
            press(&mut app, KeyCode::Backspace);
        }
        for c in "#ff8800".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(app.editor.is_none());
        assert_eq!(app.config.palette[0], "#ff8800");
    }
}
