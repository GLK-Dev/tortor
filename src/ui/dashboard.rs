use std::collections::VecDeque;
use std::io;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Gauge, List, ListItem, Paragraph},
    Terminal,
};
use tokio::sync::{broadcast, mpsc};

use crate::core::coordinator::CoordinatorMsg;
use crate::core::command::SessionEvent;
use crate::net::swarm;

pub enum CoreMessage {
    Status(String),
    Error(String),
    Progress(f32), // 0.0 to 1.0
}

struct AppState {
    logs: VecDeque<String>,
    progress: f32,
    magnet_input: String,
    input_mode: bool,
}

pub fn run_dashboard(
    torrent_path: Option<PathBuf>,
    magnet_link: Option<String>,
    listen_port: u16,
    output_dir: PathBuf,
) -> Result<()> {
    // Basic initialization
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // We need an async context to run the app loop since swarm is async
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
        
    let res = runtime.block_on(run_app(&mut terminal, torrent_path, magnet_link, listen_port, output_dir));

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    
    res
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    torrent_path: Option<PathBuf>,
    magnet_link: Option<String>,
    listen_port: u16,
    output_dir: PathBuf,
) -> Result<()> {
    let mut state = AppState {
        logs: VecDeque::with_capacity(100),
        progress: 0.0,
        magnet_input: String::new(),
        input_mode: false,
    };
    
    state.logs.push_back("Welcome to TorTor TUI".to_string());

    if let Some(magnet) = magnet_link {
        state.logs.push_back(format!("Loaded magnet from CLI: {}", magnet));
    }

    
    // We would launch swarm here if torrent_path is provided.
    // For simplicity, we just stub it in this boilerplate.
    
    let tick_rate = Duration::from_millis(250);
    let mut last_tick = Instant::now();
    
    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(
                    [
                        Constraint::Length(3), // Progress
                        Constraint::Min(10),   // Logs
                        Constraint::Length(3), // Input
                    ]
                    .as_ref(),
                )
                .split(f.size());

            // 1. Progress Bar
            let gauge = Gauge::default()
                .block(Block::default().title(" Download Progress ").borders(Borders::ALL))
                .gauge_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))
                .ratio(state.progress.clamp(0.0, 1.0) as f64);
            f.render_widget(gauge, chunks[0]);

            // 2. Logs
            let log_items: Vec<ListItem> = state.logs.iter().map(|msg| {
                ListItem::new(Line::from(vec![Span::raw(msg.clone())]))
            }).collect();
            
            let logs_list = List::new(log_items)
                .block(Block::default().title(" DHT & Swarm Activity ").borders(Borders::ALL))
                .style(Style::default().fg(Color::White));
            f.render_widget(logs_list, chunks[1]);
            
            // 3. Input
            let input_text = if state.input_mode {
                format!("> {}", state.magnet_input)
            } else {
                "Press 'a' to add Magnet link, 'q' to quit".to_string()
            };
            
            let input_para = Paragraph::new(input_text)
                .style(Style::default().fg(Color::Green))
                .block(Block::default().borders(Borders::ALL).title(" Command Line "));
            f.render_widget(input_para, chunks[2]);
        })?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if crossterm::event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if state.input_mode {
                    match key.code {
                        KeyCode::Enter => {
                            state.logs.push_back(format!("Parsing magnet: {}", state.magnet_input));
                            // TODO: Dispatch StartSearch
                            state.magnet_input.clear();
                            state.input_mode = false;
                        }
                        KeyCode::Char(c) => {
                            state.magnet_input.push(c);
                        }
                        KeyCode::Backspace => {
                            state.magnet_input.pop();
                        }
                        KeyCode::Esc => {
                            state.input_mode = false;
                        }
                        _ => {}
                    }
                } else {
                    match key.code {
                        KeyCode::Char('q') => return Ok(()),
                        KeyCode::Char('a') => state.input_mode = true,
                        _ => {}
                    }
                }
            }
        }
        
        if last_tick.elapsed() >= tick_rate {
            // Update logic here
            last_tick = Instant::now();
        }
    }
}
