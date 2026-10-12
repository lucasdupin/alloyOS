use alloc::format;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use metalos_proto::DirtyRect;

use crate::font::{
    draw_cursor, draw_markdown, draw_str, draw_str_styled, CHAR_ADVANCE_X, CHAR_LINE_HEIGHT,
};
use crate::framebuffer::{rect_union, MercuryFramebuffer};
use crate::theme::{
    ACCENT_LIQUID_SILVER, BG_OBSIDIAN, BORDER_CHROME, BORDER_SILVER_DIM, SURFACE_CARBON,
    TEXT_MERCURY_BRIGHT,
};

/// Canonical model selector badge displayed in the Mercury AI Console header.
pub const MODEL_BADGE_BAR: &str = "[Gemini | Claude | ChatGPT]";

/// Stateful representation of the primary Mercury AI Console window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiConsoleView {
    pub history: Vec<String>,
    pub streaming_line: String,
    pub active_model: String,
    pub input_prompt: String,
    pub cursor_visible: bool,
    pub scroll_offset: usize,
}

impl Default for AiConsoleView {
    fn default() -> Self {
        Self::new("Gemini")
    }
}

impl AiConsoleView {
    pub fn new(active_model: &str) -> Self {
        let mut history = Vec::new();
        history.push("**metalOS Mercury AI Console** initialized.".to_string());
        history.push("Type an intent or query below. Developer apps run as headless `MCP` tools.".to_string());
        Self {
            history,
            streaming_line: String::new(),
            active_model: active_model.to_string(),
            input_prompt: String::new(),
            cursor_visible: true,
            scroll_offset: 0,
        }
    }

    /// Formats the active model selector badge `[Gemini | Claude | ChatGPT]`.
    pub fn model_badge_text(&self) -> String {
        format!(
            "[Gemini | Claude | ChatGPT] (active: {})",
            self.active_model
        )
    }

    /// Appends a streaming token from the AI agent into the active response line.
    pub fn push_streaming_token(&mut self, token: &str) {
        for ch in token.chars() {
            if ch == '\n' {
                let completed = core::mem::take(&mut self.streaming_line);
                self.history.push(completed);
            } else {
                self.streaming_line.push(ch);
            }
        }
    }

    /// Commits any buffered streaming tokens as a finished chat line.
    pub fn finish_streaming_turn(&mut self) {
        if !self.streaming_line.is_empty() {
            let line = core::mem::take(&mut self.streaming_line);
            self.history.push(line);
        }
    }

    /// Handles an ASCII character typed by the user.
    pub fn push_input_char(&mut self, ch: char) {
        match ch {
            '\n' | '\r' => {
                let submitted = core::mem::take(&mut self.input_prompt);
                self.history.push(format!("> {}", submitted));
            }
            '\x08' | '\x7f' => {
                self.input_prompt.pop();
            }
            c if (' '..='~').contains(&c) => {
                self.input_prompt.push(c);
            }
            _ => {}
        }
    }

    /// Renders the `AiConsoleView` into `(x, y, width, height)` on `fb`.
    pub fn render(
        &self,
        fb: &mut MercuryFramebuffer,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> DirtyRect {
        let mut dirty = DirtyRect::new(x, y, 0, 0);
        if width < 32 || height < 32 {
            return dirty;
        }

        // Background fill and liquid-mercury border
        if let Some(r) = fb.fill_rect(x, y, width, height, BG_OBSIDIAN) {
            dirty = rect_union(dirty, r);
        }
        if let Some(r) = fb.draw_liquid_mercury_border(x, y, width, height) {
            dirty = rect_union(dirty, r);
        }

        // Header bar with title and `[Gemini | Claude | ChatGPT]` model badge
        let header_h = 20u32.min(height / 3);
        if let Some(r) = fb.fill_rect(x + 1, y + 1, width - 2, header_h, SURFACE_CARBON) {
            dirty = rect_union(dirty, r);
        }
        if let Some(r) = fb.draw_horizontal_line(x + 1, y + header_h, width - 2, BORDER_CHROME) {
            dirty = rect_union(dirty, r);
        }

        let title_rect = draw_str_styled(
            fb,
            x + 8,
            y + 6,
            "MERCURY // AI CONSOLE",
            TEXT_MERCURY_BRIGHT,
            true,
        );
        dirty = rect_union(dirty, title_rect);

        let badge = self.model_badge_text();
        let badge_w = (badge.len() as u32) * CHAR_ADVANCE_X;
        let badge_x = if width > badge_w + 12 {
            x + width - badge_w - 8
        } else {
            x + 8
        };
        let badge_rect = draw_str(fb, badge_x, y + 6, &badge, ACCENT_LIQUID_SILVER);
        dirty = rect_union(dirty, badge_rect);

        // Input prompt bar at bottom
        let prompt_h = 18u32;
        let prompt_y = (y + height).saturating_sub(prompt_h + 1);
        if let Some(r) = fb.fill_rect(x + 1, prompt_y, width - 2, prompt_h, SURFACE_CARBON) {
            dirty = rect_union(dirty, r);
        }
        if let Some(r) = fb.draw_horizontal_line(x + 1, prompt_y, width - 2, BORDER_SILVER_DIM) {
            dirty = rect_union(dirty, r);
        }

        let prompt_prefix = format!("agent> {}", self.input_prompt);
        let p_rect = draw_str(fb, x + 8, prompt_y + 5, &prompt_prefix, TEXT_MERCURY_BRIGHT);
        dirty = rect_union(dirty, p_rect);

        let cursor_x = x + 8 + (prompt_prefix.len() as u32) * CHAR_ADVANCE_X;
        if cursor_x + 8 < x + width {
            let c_rect = draw_cursor(fb, cursor_x, prompt_y + 5, self.cursor_visible);
            dirty = rect_union(dirty, c_rect);
        }

        // Scrollback chat history + streaming line in the middle viewport
        let body_y = y + header_h + 4;
        let body_h = prompt_y.saturating_sub(body_y + 2);
        let max_lines = (body_h / CHAR_LINE_HEIGHT).max(1) as usize;

        let mut combined = String::new();
        for line in &self.history {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(line);
        }
        if !self.streaming_line.is_empty() {
            if !combined.is_empty() {
                combined.push('\n');
            }
            combined.push_str(&self.streaming_line);
        }

        let md_rect = draw_markdown(
            fb,
            x + 8,
            body_y,
            width.saturating_sub(16),
            &combined,
            self.scroll_offset,
            max_lines,
        );
        dirty = rect_union(dirty, md_rect);

        dirty
    }
}

/// Agent-synthesized generative UI widgets rendered by `mercuryd`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MercuryWidget {
    /// Full interactive AI Console view with streaming chat history, model badge
    /// `[Gemini | Claude | ChatGPT]`, input prompt line, and cursor.
    AiConsoleView(AiConsoleView),
    /// High-contrast metric card displaying a primary telemetry or synthesis value.
    MetricCard {
        title: String,
        value: String,
        unit: String,
        subtitle: String,
    },
    /// Structured monochrome data table with chrome header divider and cell grid.
    DataTable {
        headers: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    /// Liquid-silver sparkline chart plotting sequential samples.
    Sparkline {
        label: String,
        samples: Vec<u32>,
    },
    /// Top or bottom status banner summarizing system health, active headless MCP
    /// apps, and scheduled automation scripts.
    StatusBanner {
        status: String,
        active_mcp_apps: usize,
        scheduled_scripts: usize,
    },
}

impl MercuryWidget {
    /// Renders this generative UI widget into the specified bounding rectangle
    /// `(x, y, width, height)` in `fb` and returns the damaged `DirtyRect`.
    pub fn render(
        &self,
        fb: &mut MercuryFramebuffer,
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    ) -> DirtyRect {
        match self {
            MercuryWidget::AiConsoleView(console) => console.render(fb, x, y, width, height),
            MercuryWidget::MetricCard {
                title,
                value,
                unit,
                subtitle,
            } => render_metric_card(fb, x, y, width, height, title, value, unit, subtitle),
            MercuryWidget::DataTable { headers, rows } => {
                render_data_table(fb, x, y, width, height, headers, rows)
            }
            MercuryWidget::Sparkline { label, samples } => {
                render_sparkline(fb, x, y, width, height, label, samples)
            }
            MercuryWidget::StatusBanner {
                status,
                active_mcp_apps,
                scheduled_scripts,
            } => render_status_banner(
                fb,
                x,
                y,
                width,
                height,
                status,
                *active_mcp_apps,
                *scheduled_scripts,
            ),
        }
    }
}

fn render_metric_card(
    fb: &mut MercuryFramebuffer,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    title: &str,
    value: &str,
    unit: &str,
    subtitle: &str,
) -> DirtyRect {
    let mut dirty = DirtyRect::new(x, y, 0, 0);
    if let Some(r) = fb.fill_rect(x, y, width, height, SURFACE_CARBON) {
        dirty = rect_union(dirty, r);
    }
    if let Some(r) = fb.draw_rect_border(x, y, width, height, 1, BORDER_CHROME) {
        dirty = rect_union(dirty, r);
    }
    // Top liquid silver accent bar
    if let Some(r) = fb.fill_rect(x + 1, y + 1, width.saturating_sub(2), 2, ACCENT_LIQUID_SILVER) {
        dirty = rect_union(dirty, r);
    }

    let r1 = draw_str(fb, x + 6, y + 6, title, ACCENT_LIQUID_SILVER);
    dirty = rect_union(dirty, r1);

    let val_line = if unit.is_empty() {
        value.to_string()
    } else {
        format!("{} {}", value, unit)
    };
    let r2 = draw_str_styled(fb, x + 6, y + 18, &val_line, TEXT_MERCURY_BRIGHT, true);
    dirty = rect_union(dirty, r2);

    if !subtitle.is_empty() && height >= 40 {
        let r3 = draw_str(fb, x + 6, y + 30, subtitle, BORDER_SILVER_DIM);
        dirty = rect_union(dirty, r3);
    }

    dirty
}

fn render_data_table(
    fb: &mut MercuryFramebuffer,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    headers: &[String],
    rows: &[Vec<String>],
) -> DirtyRect {
    let mut dirty = DirtyRect::new(x, y, 0, 0);
    if let Some(r) = fb.fill_rect(x, y, width, height, SURFACE_CARBON) {
        dirty = rect_union(dirty, r);
    }
    if let Some(r) = fb.draw_rect_border(x, y, width, height, 1, BORDER_CHROME) {
        dirty = rect_union(dirty, r);
    }

    let cols = headers.len().max(1) as u32;
    let col_w = (width.saturating_sub(8) / cols).max(24);

    // Header row
    let header_y = y + 4;
    for (col_idx, h) in headers.iter().enumerate() {
        let cx = x + 4 + (col_idx as u32) * col_w;
        let r = draw_str_styled(fb, cx, header_y, h, TEXT_MERCURY_BRIGHT, true);
        dirty = rect_union(dirty, r);
    }

    let sep_y = y + 14;
    if sep_y < y + height {
        if let Some(r) = fb.draw_horizontal_line(x + 2, sep_y, width.saturating_sub(4), BORDER_CHROME)
        {
            dirty = rect_union(dirty, r);
        }
    }

    // Data rows
    for (row_idx, row) in rows.iter().enumerate() {
        let ry = y + 18 + (row_idx as u32) * CHAR_LINE_HEIGHT;
        if ry + 8 >= y + height {
            break;
        }
        for (col_idx, cell) in row.iter().enumerate() {
            let cx = x + 4 + (col_idx as u32) * col_w;
            let r = draw_str(fb, cx, ry, cell, ACCENT_LIQUID_SILVER);
            dirty = rect_union(dirty, r);
        }
    }

    dirty
}

fn render_sparkline(
    fb: &mut MercuryFramebuffer,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    label: &str,
    samples: &[u32],
) -> DirtyRect {
    let mut dirty = DirtyRect::new(x, y, 0, 0);
    if let Some(r) = fb.fill_rect(x, y, width, height, SURFACE_CARBON) {
        dirty = rect_union(dirty, r);
    }
    if let Some(r) = fb.draw_rect_border(x, y, width, height, 1, BORDER_SILVER_DIM) {
        dirty = rect_union(dirty, r);
    }

    let r_label = draw_str(fb, x + 6, y + 4, label, TEXT_MERCURY_BRIGHT);
    dirty = rect_union(dirty, r_label);

    if samples.is_empty() || width <= 12 || height <= 20 {
        return dirty;
    }

    let chart_x = x + 6;
    let chart_y = y + 16;
    let chart_w = width - 12;
    let chart_h = height - 20;

    let min_val = *samples.iter().min().unwrap_or(&0);
    let max_val = (*samples.iter().max().unwrap_or(&1)).max(min_val + 1);
    let range = (max_val - min_val).max(1);

    let count = samples.len() as u32;
    let step_x = (chart_w / count.max(1)).max(2);

    for (i, &sample) in samples.iter().enumerate() {
        let bar_x = chart_x + (i as u32) * step_x;
        if bar_x >= chart_x + chart_w {
            break;
        }
        let norm_h = (((sample - min_val) * (chart_h - 1)) / range).max(1);
        let bar_y = chart_y + chart_h - norm_h;

        // Draw vertical bar in dimmed silver and top peak pixel in bright mercury
        if let Some(r) = fb.fill_rect(bar_x, bar_y, (step_x - 1).max(1), norm_h, ACCENT_LIQUID_SILVER)
        {
            dirty = rect_union(dirty, r);
        }
        if let Some(r) = fb.fill_rect(bar_x, bar_y, (step_x - 1).max(1), 1, TEXT_MERCURY_BRIGHT) {
            dirty = rect_union(dirty, r);
        }
    }

    dirty
}

fn render_status_banner(
    fb: &mut MercuryFramebuffer,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    status: &str,
    active_mcp_apps: usize,
    scheduled_scripts: usize,
) -> DirtyRect {
    let mut dirty = DirtyRect::new(x, y, 0, 0);
    if let Some(r) = fb.fill_rect(x, y, width, height, SURFACE_CARBON) {
        dirty = rect_union(dirty, r);
    }
    if let Some(r) = fb.draw_rect_border(x, y, width, height, 1, BORDER_SILVER_DIM) {
        dirty = rect_union(dirty, r);
    }

    let text = format!(
        "STATUS: {} | Headless MCP Apps: {} | Scheduled Scripts: {}",
        status, active_mcp_apps, scheduled_scripts
    );
    let text_y = y + (height.saturating_sub(8)) / 2;
    let r = draw_str(fb, x + 6, text_y, &text, ACCENT_LIQUID_SILVER);
    dirty = rect_union(dirty, r);
    dirty
}
