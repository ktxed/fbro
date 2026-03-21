use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event, KeyCode, KeyModifiers};
use crossterm::terminal::{self, EnterAlternateScreen, LeaveAlternateScreen};
use crossterm::{execute, cursor};
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph};
use ratatui::Terminal;
use serde::{Deserialize, Serialize};

// ─── Protocol types ──────────────────────────────────────────────────────────

#[derive(Deserialize, Debug)]
struct SpyRequest {
    id: String,
    op: String,
    args: serde_json::Value,
    #[allow(dead_code)]
    ts: u64,
}

#[derive(Serialize)]
struct SpyResponse {
    id: String,
    action: String,
}

// ─── App state ───────────────────────────────────────────────────────────────

struct PendingOp {
    request: SpyRequest,
    reply_tx: mpsc::Sender<String>, // "approve" or "deny"
    arrived: Instant,
}

struct LogEntry {
    time: String,
    op: String,
    detail: String,
    result: String,
}

struct App {
    log: Vec<LogEntry>,
    pending: Option<PendingOp>,
    auto_delay_ms: u64,
    auto_approve: bool,
    input_mode: InputMode,
    input_buf: String,
}

enum InputMode {
    Normal,
    DelayInput,
}

impl App {
    fn new() -> Self {
        Self {
            log: Vec::new(),
            pending: None,
            auto_delay_ms: 0,
            auto_approve: false,
            input_mode: InputMode::Normal,
            input_buf: String::new(),
        }
    }

    fn resolve(&mut self, action: &str) {
        if let Some(pending) = self.pending.take() {
            let detail = format_args_short(&pending.request.op, &pending.request.args);
            self.log.push(LogEntry {
                time: now_str(),
                op: pending.request.op.clone(),
                detail,
                result: action.to_uppercase(),
            });
            let _ = pending.reply_tx.send(action.to_string());
        }
    }
}

fn format_args_short(op: &str, args: &serde_json::Value) -> String {
    match op {
        "try_rename" => {
            let src = args.get("src").and_then(|v| v.as_str()).unwrap_or("?");
            let dst = args.get("dst").and_then(|v| v.as_str()).unwrap_or("?");
            format!("{src} -> {dst}")
        }
        _ => args
            .get("path")
            .and_then(|v| v.as_str())
            .unwrap_or("?")
            .to_string(),
    }
}

fn now_str() -> String {
    let d = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    let secs = d.as_secs();
    let h = (secs / 3600) % 24;
    let m = (secs / 60) % 60;
    let s = secs % 60;
    format!("{h:02}:{m:02}:{s:02}")
}

// ─── Socket listener thread ─────────────────────────────────────────────────

fn handle_connection(
    stream: UnixStream,
    op_tx: mpsc::Sender<(SpyRequest, mpsc::Sender<String>)>,
) {
    let reader = BufReader::new(
        stream
            .try_clone()
            .expect("failed to clone stream"),
    );
    let mut writer = stream;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => break,
        };
        if line.trim().is_empty() {
            continue;
        }

        let request: SpyRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(_) => continue,
        };

        let (reply_tx, reply_rx) = mpsc::channel::<String>();
        let req_id = request.id.clone();

        if op_tx.send((request, reply_tx)).is_err() {
            break;
        }

        // Block until the TUI thread sends a verdict
        let action = reply_rx.recv().unwrap_or_else(|_| "deny".to_string());

        let resp = SpyResponse {
            id: req_id,
            action,
        };
        let mut msg = serde_json::to_string(&resp).unwrap_or_default();
        msg.push('\n');
        if writer.write_all(msg.as_bytes()).is_err() {
            break;
        }
        let _ = writer.flush();
    }
}

fn start_listener(
    sock_path: &str,
    op_tx: mpsc::Sender<(SpyRequest, mpsc::Sender<String>)>,
) {
    // Clean up stale socket
    let _ = std::fs::remove_file(sock_path);

    let listener = UnixListener::bind(sock_path).expect("failed to bind UDS");
    listener
        .set_nonblocking(false)
        .expect("failed to set blocking");

    for stream in listener.incoming() {
        match stream {
            Ok(s) => {
                let tx = op_tx.clone();
                thread::spawn(move || handle_connection(s, tx));
            }
            Err(e) => {
                eprintln!("accept error: {e}");
            }
        }
    }
}

// ─── TUI rendering ──────────────────────────────────────────────────────────

fn draw(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, app: &App) {
    let _ = terminal.draw(|f| {
        let chunks = Layout::vertical([
            Constraint::Length(3), // header
            Constraint::Length(5), // pending
            Constraint::Min(5),   // log
        ])
        .split(f.area());

        draw_header(f, chunks[0], app);
        draw_pending(f, chunks[1], app);
        draw_log(f, chunks[2], app);
    });
}

fn draw_header(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let auto_str = if app.auto_approve { "ON" } else { "OFF" };
    let status = match &app.input_mode {
        InputMode::Normal => format!(
            " Auto-delay: {}ms  |  Auto-approve: {}  |  [y]Approve [n]Deny [a]Auto [d]Delay [q]Quit",
            app.auto_delay_ms, auto_str
        ),
        InputMode::DelayInput => format!(
            " Set delay (ms): {}_ [Enter to confirm, Esc to cancel]",
            app.input_buf
        ),
    };
    let block = Block::default()
        .title(" lfs-spy ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Cyan));
    let paragraph = Paragraph::new(status).block(block);
    f.render_widget(paragraph, area);
}

fn draw_pending(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(" Pending ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::Yellow));

    let content = if let Some(ref p) = app.pending {
        let detail = format_args_short(&p.request.op, &p.request.args);
        let elapsed = p.arrived.elapsed().as_secs();
        vec![
            Line::from(vec![
                Span::styled(
                    format!("  {} ", p.request.op),
                    Style::default()
                        .fg(Color::White)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled(detail, Style::default().fg(Color::Gray)),
            ]),
            Line::from(Span::styled(
                format!("  waiting {elapsed}s..."),
                Style::default().fg(Color::DarkGray),
            )),
        ]
    } else {
        vec![Line::from(Span::styled(
            "  (none)",
            Style::default().fg(Color::DarkGray),
        ))]
    };

    let paragraph = Paragraph::new(content).block(block);
    f.render_widget(paragraph, area);
}

fn draw_log(f: &mut ratatui::Frame, area: Rect, app: &App) {
    let block = Block::default()
        .title(" Log ")
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    // Show most recent entries that fit
    let visible = (area.height.saturating_sub(2)) as usize;
    let start = app.log.len().saturating_sub(visible);

    let items: Vec<ListItem> = app.log[start..]
        .iter()
        .map(|e| {
            let result_color = match e.result.as_str() {
                "APPROVED" => Color::Green,
                "DENIED" => Color::Red,
                _ => Color::Gray,
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!(" {} ", e.time),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{:<15} ", e.op),
                    Style::default().fg(Color::Cyan),
                ),
                Span::styled(
                    format!("{:<40} ", e.detail),
                    Style::default().fg(Color::White),
                ),
                Span::styled(&e.result, Style::default().fg(result_color)),
            ]))
        })
        .collect();

    let list = List::new(items).block(block);
    f.render_widget(list, area);
}

// ─── Main ────────────────────────────────────────────────────────────────────

fn main() -> io::Result<()> {
    let sock_path = std::env::var("FBRO_SPY_SOCK")
        .unwrap_or_else(|_| "/tmp/fbro-spy.sock".to_string());

    let (op_tx, op_rx) = mpsc::channel::<(SpyRequest, mpsc::Sender<String>)>();

    // Start listener thread
    let sock_path_clone = sock_path.clone();
    thread::spawn(move || start_listener(&sock_path_clone, op_tx));

    // Setup terminal
    terminal::enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, cursor::Hide)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();

    // Main loop
    loop {
        // Check for new pending operations (non-blocking)
        if app.pending.is_none() {
            if let Ok((request, reply_tx)) = op_rx.try_recv() {
                app.pending = Some(PendingOp {
                    request,
                    reply_tx,
                    arrived: Instant::now(),
                });
            }
        }

        // Auto-approve logic
        if app.auto_approve {
            if let Some(ref p) = app.pending {
                if p.arrived.elapsed() >= Duration::from_millis(app.auto_delay_ms) {
                    app.resolve("approve");
                }
            }
        }

        draw(&mut terminal, &app);

        // Poll for keyboard input (100ms timeout for responsive auto-approve)
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                match &app.input_mode {
                    InputMode::Normal => match key.code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                            break
                        }
                        KeyCode::Char('y') | KeyCode::Enter => {
                            if app.pending.is_some() {
                                app.resolve("approve");
                            }
                        }
                        KeyCode::Char('n') => {
                            if app.pending.is_some() {
                                app.resolve("deny");
                            }
                        }
                        KeyCode::Char('a') => {
                            app.auto_approve = !app.auto_approve;
                        }
                        KeyCode::Char('d') => {
                            app.input_mode = InputMode::DelayInput;
                            app.input_buf.clear();
                        }
                        _ => {}
                    },
                    InputMode::DelayInput => match key.code {
                        KeyCode::Enter => {
                            if let Ok(ms) = app.input_buf.parse::<u64>() {
                                app.auto_delay_ms = ms;
                            }
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Esc => {
                            app.input_mode = InputMode::Normal;
                        }
                        KeyCode::Char(c) if c.is_ascii_digit() => {
                            app.input_buf.push(c);
                        }
                        KeyCode::Backspace => {
                            app.input_buf.pop();
                        }
                        _ => {}
                    },
                }
            }
        }
    }

    // Cleanup
    terminal::disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        cursor::Show
    )?;

    // Remove socket file
    let _ = std::fs::remove_file(&sock_path);

    Ok(())
}
