
use std::io::{self, IsTerminal, Write};

use crossterm::cursor::{MoveTo, RestorePosition, SavePosition};
use crossterm::execute;
use ratatui::layout::Rect;
use ratatui::{TerminalOptions, Viewport};

use crate::config::ChartRender;
use crate::error::Result;
use crate::ui::chart::PriceView;
use crate::ui::graphics::{self, ImageProtocol};
use crate::ui::{plot_image, sixel};

const FALLBACK_CELL: (u32, u32) = (10, 20);

pub fn show(view: &mut PriceView) -> Result<()> {
    if !io::stdout().is_terminal() {
        return write_plain(view);
    }

    let mut terminal = match ratatui::try_init_with_options(TerminalOptions {
        viewport: Viewport::Inline(1),
    }) {
        Ok(terminal) => terminal,
        Err(_) => return write_plain(view),
    };

    let size = terminal.size().unwrap_or(ratatui::layout::Size {
        width: 80,
        height: u16::MAX,
    });
    let height = view.height().clamp(1, size.height.saturating_sub(1).max(1));
    let inserted = Rect::new(0, 0, size.width, height);

    let mut image = None;
    if let Some(target) = view.image_area(inserted) {
        image = render_lines(view, target).map(|rgba| (rgba, target));
    }
    if image.is_none() {

        view.style.render = ChartRender::Steps;
    }

    let view = &*view;
    let result = terminal.insert_before(height, |buf| {
        ratatui::widgets::Widget::render(view, buf.area, buf);
    });

    let top = terminal.get_frame().area().y.saturating_sub(height);

    let _ = terminal.clear();
    ratatui::restore();

    result?;

    if let Some((rgba, target)) = image {
        let on_screen = Rect {
            y: target.y.saturating_add(top),
            ..target
        };

        let _ = place(
            &rgba,
            &plot_image::palette(&view.series_colors()),
            on_screen,
        );
    }
    Ok(())
}

fn render_lines(view: &PriceView, target: Rect) -> Option<image::RgbaImage> {
    let data = view.chart.as_ref()?;
    graphics::protocol()?;
    let (cell_w, cell_h) = cell_size();
    plot_image::draw(
        data,
        &view.series_colors(),
        u32::from(target.width) * cell_w,
        u32::from(target.height) * cell_h,
    )
}

fn cell_size() -> (u32, u32) {

    let Ok(window) = crossterm::terminal::window_size() else {
        return FALLBACK_CELL;
    };
    if window.width == 0 || window.height == 0 || window.columns == 0 || window.rows == 0 {
        return FALLBACK_CELL;
    }
    (
        u32::from(window.width) / u32::from(window.columns),
        u32::from(window.height) / u32::from(window.rows),
    )
}

fn place(
    rgba: &image::RgbaImage,
    lines: &[[u8; 3]],
    area: Rect,
) -> std::result::Result<(), viuer::ViuError> {
    let Some(protocol) = graphics::protocol() else {
        return Ok(());
    };
    let Ok(y) = i16::try_from(area.y) else {
        return Ok(());
    };

    if protocol == ImageProtocol::Sixel {
        let Some(sequence) = sixel::encode(rgba, lines) else {
            return Ok(());
        };
        let mut stdout = io::stdout();

        execute!(stdout, SavePosition, MoveTo(area.x, area.y))?;
        write!(stdout, "{sequence}")?;
        execute!(stdout, RestorePosition)?;
        stdout.flush()?;
        return Ok(());
    }

    let config = viuer::Config {
        absolute_offset: true,
        x: area.x,
        y,
        width: Some(u32::from(area.width)),
        height: Some(u32::from(area.height)),
        restore_cursor: true,
        transparent: true,
        truecolor: true,

        use_kitty: protocol == ImageProtocol::Kitty,
        use_iterm: protocol == ImageProtocol::Iterm2,
        use_sixel: false,
        ..Default::default()
    };

    viuer::print(&image::DynamicImage::ImageRgba8(rgba.clone()), &config).map(|_| ())
}

fn write_plain(view: &PriceView) -> Result<()> {
    let mut out = io::stdout().lock();
    writeln!(out, "{}", view.plain_text())?;
    Ok(())
}
