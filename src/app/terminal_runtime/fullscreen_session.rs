// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use anyhow::{Context, anyhow};
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use std::io::Stdout;

pub(super) struct FullscreenTerminalSession<B: Backend = CrosstermBackend<Stdout>> {
    terminal: Terminal<B>,
}

impl<B: Backend<Error = std::io::Error>> FullscreenTerminalSession<B> {
    pub(super) fn new(backend: B) -> anyhow::Result<Self> {
        Terminal::new(backend)
            .map(|terminal| Self { terminal })
            .context("failed to construct fullscreen terminal session")
    }

    pub(super) fn draw(&mut self, app: &mut App) -> anyhow::Result<()> {
        let result = draw_fullscreen_surface_frame(&mut self.terminal, app);
        if result.is_ok() {
            app.surface_dirty.fullscreen.redraw = false;
        }
        result
    }
}

pub(super) fn draw_fullscreen_surface_frame<B: Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> anyhow::Result<()> {
    let area = terminal
        .draw(|frame| crate::ui::render_fullscreen_surface(frame, app))
        .map(|frame| frame.area)
        .map_err(|err| anyhow!("failed to draw fullscreen surface: {err}"))?;
    tracing::debug!(
        target: crate::logging::targets::APP_RENDER,
        event_name = "fullscreen_surface_draw",
        message = "fullscreen surface repainted",
        outcome = "success",
        terminal_width = area.width,
        terminal_height = area.height,
    );
    Ok(())
}
