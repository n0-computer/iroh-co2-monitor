//! The ratatui screen. Mirrors the browser GUI: a headline, CO2 as an oversized
//! number colored like the device's LED (blue → green → yellow → red) that fades to
//! gray as the reading ages, temperature + humidity underneath, and a status line.
//! Below that, what the browser can't show: a connection panel with the selected
//! path (direct or relay), its RTT, and every open path.

use std::time::{Duration, Instant};

use co2_proto::Reading;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use iroh::EndpointId;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;
use tokio::sync::mpsc;

use crate::net::{ConnInfo, ConnState, NetEvent, PathKind};

/// Redraw cadence. Drives the fade-to-gray and the "Ns ago" counter.
const FRAME: Duration = Duration::from_millis(100);
/// A reading fades fully to gray over this long (same as the web GUI).
const FADE: Duration = Duration::from_secs(30);

/// CO2 → color stops, mirroring the device's LED and the web GUI's `CO2_STOPS`.
const CO2_STOPS: [(f32, (u8, u8, u8)); 4] = [
    (420.0, (59, 130, 246)), // blue
    (650.0, (34, 197, 94)),  // green
    (1000.0, (234, 179, 8)), // yellow
    (1500.0, (239, 68, 68)), // red
];
/// Settled gray: before the first reading, and what a reading fades toward.
const STALE: (u8, u8, u8) = (138, 143, 152);
const MUTED: Color = Color::Rgb(150, 155, 165);
const DIM: Color = Color::Rgb(110, 115, 125);

/// Everything the screen needs.
pub struct App {
    me: EndpointId,
    remote: EndpointId,
    reading: Option<(Reading, Instant)>,
    status: String,
    conn: ConnInfo,
}

impl App {
    pub fn new(me: EndpointId, remote: EndpointId) -> Self {
        Self {
            me,
            remote,
            reading: None,
            status: "starting…".to_string(),
            conn: ConnInfo::default(),
        }
    }

    fn apply(&mut self, ev: NetEvent) {
        match ev {
            NetEvent::Reading(r) => self.reading = Some((r, Instant::now())),
            // Once we've had a reading, the status line shows its age instead of raw
            // rpc/connection errors — the grayed-out number already signals stale.
            NetEvent::Status(s) => self.status = s,
            NetEvent::Conn(c) => self.conn = c,
        }
    }
}

/// Event loop: draw, then wait for a key, a network event, or the frame tick.
pub async fn run(mut app: App, mut net_rx: mpsc::Receiver<NetEvent>) -> anyhow::Result<()> {
    let mut terminal = ratatui::init();
    let (key_tx, mut key_rx) = mpsc::channel::<Event>(16);
    // crossterm's reader blocks, so it lives on its own thread.
    std::thread::spawn(move || loop {
        match event::read() {
            Ok(ev) => {
                if key_tx.blocking_send(ev).is_err() {
                    return;
                }
            }
            Err(_) => return,
        }
    });

    let mut tick = tokio::time::interval(FRAME);
    let result = loop {
        if let Err(e) = terminal.draw(|f| draw(f, &app)) {
            break Err(e.into());
        }
        tokio::select! {
            Some(ev) = key_rx.recv() => {
                if is_quit(&ev) {
                    break Ok(());
                }
            }
            ev = net_rx.recv() => match ev {
                Some(ev) => app.apply(ev),
                None => break Ok(()),
            },
            _ = tick.tick() => {}
        }
    };
    ratatui::restore();
    result
}

fn is_quit(ev: &Event) -> bool {
    match ev {
        Event::Key(k) if k.kind != KeyEventKind::Release => {
            matches!(k.code, KeyCode::Char('q') | KeyCode::Esc)
                || (k.code == KeyCode::Char('c') && k.modifiers.contains(KeyModifiers::CONTROL))
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Drawing
// ---------------------------------------------------------------------------

fn draw(f: &mut Frame, app: &App) {
    let outer = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(
            Line::from(" CO₂ monitor ")
                .style(Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
        )
        .title_alignment(Alignment::Center);
    let area = outer.inner(f.area());
    f.render_widget(outer, f.area());

    let hero_h = if area.width >= big_width(4) + 6 {
        BIG_ROWS as u16
    } else {
        1
    };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),                // top padding
            Constraint::Length(hero_h),           // CO2 hero
            Constraint::Length(1),                // gap
            Constraint::Length(1),                // temp + humidity
            Constraint::Length(1),                // gap
            Constraint::Length(1),                // status
            Constraint::Length(1),                // gap
            Constraint::Length(conn_height(app)), // connection panel
            Constraint::Min(0),                   // filler
            Constraint::Length(1),                // help
        ])
        .split(area);

    draw_hero(f, rows[1], app);
    draw_readings(f, rows[3], app);
    draw_status(f, rows[5], app);
    draw_conn(f, rows[7], app);
    f.render_widget(
        Paragraph::new(Line::from(vec![
            Span::styled("q", Style::default().fg(MUTED).add_modifier(Modifier::BOLD)),
            Span::styled(" quit", Style::default().fg(DIM)),
        ]))
        .alignment(Alignment::Center),
        rows[9],
    );
}

/// Rows the connection panel needs: borders, state/path/rtt, a gap, the path list,
/// a gap, and the two endpoint IDs.
fn conn_height(app: &App) -> u16 {
    let paths = app.conn.paths.len().max(1) as u16;
    2 + 3 + 1 + paths + 1 + 2
}

/// The big CO2 number, in the LED's color, fading to gray with age.
fn draw_hero(f: &mut Frame, area: Rect, app: &App) {
    let (text, color) = match app.reading {
        Some((r, at)) => (
            format!("{}", r.co2.round() as i64),
            co2_color(r.co2, at.elapsed()),
        ),
        None => ("—".to_string(), rgb(STALE)),
    };
    let unit = Span::styled(
        " ppm",
        Style::default().fg(MUTED).add_modifier(Modifier::BOLD),
    );

    if area.height >= BIG_ROWS as u16 {
        let glyphs = big_lines(&text);
        let width = big_width(text.chars().count()) + 4; // + " ppm"
        let x = area.x + area.width.saturating_sub(width) / 2;
        for (i, row) in glyphs.iter().enumerate() {
            let mut spans = vec![Span::styled(
                row.clone(),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            )];
            // The unit sits on the number's baseline, like the web GUI.
            if i == BIG_ROWS - 1 {
                spans.push(unit.clone());
            }
            let line_area = Rect::new(
                x,
                area.y + i as u16,
                area.width.saturating_sub(x - area.x),
                1,
            );
            f.render_widget(Paragraph::new(Line::from(spans)), line_area);
        }
    } else {
        // Too narrow for the block font: plain, but still bold and colored.
        let line = Line::from(vec![
            Span::styled(
                text,
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            unit,
        ]);
        f.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
    }
}

/// Temperature + humidity: secondary, side by side, muted.
fn draw_readings(f: &mut Frame, area: Rect, app: &App) {
    let (temp, hum) = match app.reading {
        Some((r, _)) => (
            format!("{:.1}", r.temperature),
            format!("{:.1}", r.humidity),
        ),
        None => ("—".to_string(), "—".to_string()),
    };
    let value = Style::default().fg(MUTED).add_modifier(Modifier::BOLD);
    let unit = Style::default().fg(DIM);
    let line = Line::from(vec![
        Span::styled(temp, value),
        Span::styled(" °C", unit),
        Span::raw("      "),
        Span::styled(hum, value),
        Span::styled(" %", unit),
    ]);
    f.render_widget(Paragraph::new(line).alignment(Alignment::Center), area);
}

fn draw_status(f: &mut Frame, area: Rect, app: &App) {
    let text = match app.reading {
        Some((_, at)) => format!("last reading {}", ago(at.elapsed())),
        None => app.status.clone(),
    };
    f.render_widget(
        Paragraph::new(Line::from(Span::styled(text, Style::default().fg(DIM))))
            .alignment(Alignment::Center),
        area,
    );
}

/// The part the browser can't show: which path the QUIC connection is on and how
/// fast it is, plus every open path (iroh keeps the relay path warm even once a
/// direct one wins).
fn draw_conn(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(DIM))
        .title(Line::from(" connection ").style(Style::default().fg(MUTED)));
    let inner = block.inner(area);
    f.render_widget(block, area);

    let label = Style::default().fg(DIM);
    let value = Style::default().fg(MUTED);
    let bold = Style::default()
        .fg(Color::Reset)
        .add_modifier(Modifier::BOLD);
    let row = |k: &str, v: Vec<Span<'static>>| {
        let mut spans = vec![Span::styled(format!("  {k:<8}"), label)];
        spans.extend(v);
        Line::from(spans)
    };

    let conn = &app.conn;
    let (state_text, state_style) = match conn.state {
        Some(ConnState::Connected) => ("connected", Style::default().fg(Color::Green)),
        Some(ConnState::Connecting) => ("connecting…", Style::default().fg(Color::Yellow)),
        Some(ConnState::Reconnecting) => ("reconnecting…", Style::default().fg(Color::Yellow)),
        None => ("—", value),
    };

    let mut state_spans = vec![Span::styled(state_text, state_style)];
    // While not connected, the status line up top may be showing the reading's age,
    // so repeat the latest network status (e.g. the error) here where it's useful.
    if conn.state != Some(ConnState::Connected) && app.reading.is_some() {
        state_spans.push(Span::styled(format!("  {}", app.status), label));
    }
    let mut lines = vec![row("state", state_spans)];

    match conn.selected() {
        Some(p) => {
            lines.push(row(
                "path",
                vec![
                    Span::styled(
                        p.kind.label(),
                        kind_style(p.kind).add_modifier(Modifier::BOLD),
                    ),
                    Span::styled(format!("  {}", p.remote), value),
                ],
            ));
            lines.push(row("rtt", vec![Span::styled(fmt_rtt(p.rtt), bold)]));
        }
        None => {
            lines.push(row("path", vec![Span::styled("—", value)]));
            lines.push(row("rtt", vec![Span::styled("—", value)]));
        }
    }

    lines.push(Line::default());
    if conn.paths.is_empty() {
        lines.push(row("paths", vec![Span::styled("none open", value)]));
    } else {
        for (i, p) in conn.paths.iter().enumerate() {
            let marker = if p.selected { "●" } else { "○" };
            let marker_style = if p.selected {
                kind_style(p.kind)
            } else {
                label
            };
            lines.push(row(
                if i == 0 { "paths" } else { "" },
                vec![
                    Span::styled(format!("{marker} "), marker_style),
                    Span::styled(format!("{:<7}", p.kind.label()), kind_style(p.kind)),
                    Span::styled(
                        format!("{:>9}  ", fmt_rtt(p.rtt)),
                        if p.selected { bold } else { value },
                    ),
                    Span::styled(p.remote.clone(), if p.selected { value } else { label }),
                ],
            ));
        }
    }

    lines.push(Line::default());
    lines.push(row(
        "remote",
        vec![Span::styled(app.remote.to_string(), value)],
    ));
    lines.push(row("me", vec![Span::styled(app.me.to_string(), label)]));

    f.render_widget(Paragraph::new(lines), inner);
}

fn kind_style(kind: PathKind) -> Style {
    match kind {
        PathKind::Direct => Style::default().fg(Color::Green),
        PathKind::Relay => Style::default().fg(Color::Yellow),
    }
}

// ---------------------------------------------------------------------------
// Formatting helpers
// ---------------------------------------------------------------------------

fn fmt_rtt(d: Duration) -> String {
    let ms = d.as_secs_f64() * 1000.0;
    if ms >= 100.0 {
        format!("{ms:.0} ms")
    } else {
        format!("{ms:.1} ms")
    }
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    if s < 1 {
        "just now".to_string()
    } else if s < 60 {
        format!("{s}s ago")
    } else if s < 3600 {
        format!("{}m {}s ago", s / 60, s % 60)
    } else {
        format!("{}h {}m ago", s / 3600, (s % 3600) / 60)
    }
}

fn rgb((r, g, b): (u8, u8, u8)) -> Color {
    Color::Rgb(r, g, b)
}

fn lerp(a: (u8, u8, u8), b: (u8, u8, u8), t: f32) -> (u8, u8, u8) {
    let t = t.clamp(0.0, 1.0);
    let ch = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    (ch(a.0, b.0), ch(a.1, b.1), ch(a.2, b.2))
}

/// CO2 → color (same stops as the LED and the web GUI), then faded toward gray by
/// how long ago the reading arrived.
fn co2_color(ppm: f32, age: Duration) -> Color {
    let base = co2_base_color(ppm);
    let t = age.as_secs_f32() / FADE.as_secs_f32();
    rgb(lerp(base, STALE, t))
}

fn co2_base_color(ppm: f32) -> (u8, u8, u8) {
    if ppm <= CO2_STOPS[0].0 {
        return CO2_STOPS[0].1;
    }
    for w in CO2_STOPS.windows(2) {
        let (p0, c0) = w[0];
        let (p1, c1) = w[1];
        if ppm <= p1 {
            return lerp(c0, c1, (ppm - p0) / (p1 - p0));
        }
    }
    CO2_STOPS[CO2_STOPS.len() - 1].1
}

// ---------------------------------------------------------------------------
// Block-character digit font for the hero number
// ---------------------------------------------------------------------------

const BIG_ROWS: usize = 7;
/// Each glyph cell is rendered as two terminal cells so digits look roughly square.
const CELL: &str = "██";
const CELL_W: u16 = 2;
const GLYPH_COLS: u16 = 5;
const GLYPH_GAP: u16 = 2;

/// Rendered width of `n` glyphs, in terminal cells.
fn big_width(n: usize) -> u16 {
    let n = n as u16;
    n * GLYPH_COLS * CELL_W + n.saturating_sub(1) * GLYPH_GAP
}

/// 5×7 bitmap glyphs: `#` is a lit cell.
fn glyph(c: char) -> [&'static str; BIG_ROWS] {
    match c {
        '0' => [
            " ### ", "#   #", "#   #", "#   #", "#   #", "#   #", " ### ",
        ],
        '1' => [
            "  #  ", " ##  ", "  #  ", "  #  ", "  #  ", "  #  ", " ### ",
        ],
        '2' => [
            " ### ", "#   #", "    #", "   # ", "  #  ", " #   ", "#####",
        ],
        '3' => [
            " ### ", "#   #", "    #", "  ## ", "    #", "#   #", " ### ",
        ],
        '4' => [
            "   # ", "  ## ", " # # ", "#  # ", "#####", "   # ", "   # ",
        ],
        '5' => [
            "#####", "#    ", "#### ", "    #", "    #", "#   #", " ### ",
        ],
        '6' => [
            " ### ", "#    ", "#    ", "#### ", "#   #", "#   #", " ### ",
        ],
        '7' => [
            "#####", "    #", "   # ", "  #  ", " #   ", " #   ", " #   ",
        ],
        '8' => [
            " ### ", "#   #", "#   #", " ### ", "#   #", "#   #", " ### ",
        ],
        '9' => [
            " ### ", "#   #", "#   #", " ####", "    #", "    #", " ### ",
        ],
        '-' | '—' => [
            "     ", "     ", "     ", "#####", "     ", "     ", "     ",
        ],
        _ => [
            "     ", "     ", "     ", "     ", "     ", "     ", "     ",
        ],
    }
}

/// Render `text` as `BIG_ROWS` lines of block characters.
fn big_lines(text: &str) -> Vec<String> {
    let mut rows = vec![String::new(); BIG_ROWS];
    for (i, c) in text.chars().enumerate() {
        let g = glyph(c);
        for (r, row) in rows.iter_mut().enumerate() {
            if i > 0 {
                row.push_str(&" ".repeat(GLYPH_GAP as usize));
            }
            for cell in g[r].chars() {
                row.push_str(if cell == '#' { CELL } else { "  " });
            }
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn color_stops_match_web_gui() {
        assert_eq!(co2_base_color(300.0), (59, 130, 246));
        assert_eq!(co2_base_color(420.0), (59, 130, 246));
        assert_eq!(co2_base_color(650.0), (34, 197, 94));
        assert_eq!(co2_base_color(1000.0), (234, 179, 8));
        assert_eq!(co2_base_color(1500.0), (239, 68, 68));
        assert_eq!(co2_base_color(9000.0), (239, 68, 68));
    }

    #[test]
    fn fades_to_gray() {
        assert_eq!(co2_color(420.0, FADE), rgb(STALE));
        assert_eq!(co2_color(420.0, FADE * 2), rgb(STALE));
    }

    #[test]
    fn big_font_is_rectangular() {
        for row in big_lines("0123456789—") {
            assert_eq!(row.chars().count() as u16, big_width(11));
        }
    }
}
