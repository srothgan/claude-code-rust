// Copyright 2025 Simon Peter Rothgang
// SPDX-License-Identifier: Apache-2.0

use crate::app::App;
use anyhow::anyhow;
use ratatui::Terminal;
use ratatui::backend::{Backend, CrosstermBackend};
use std::io::Stdout;

type StdoutTerminal = Terminal<CrosstermBackend<Stdout>>;

pub(super) struct FullscreenTerminalSession {
    terminal: StdoutTerminal,
}

impl FullscreenTerminalSession {
    pub(super) fn new() -> anyhow::Result<Self> {
        Terminal::new(CrosstermBackend::new(std::io::stdout()))
            .map(|terminal| Self { terminal })
            .map_err(|err| anyhow!("failed to construct fullscreen terminal session: {err}"))
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
    terminal
        .draw(|frame| crate::ui::render_fullscreen_surface(frame, app))
        .map(|_| ())
        .map_err(|err| anyhow!("failed to draw fullscreen surface: {err}"))?;
    Ok(())
}
