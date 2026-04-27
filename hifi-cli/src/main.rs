use hifi_core::api::Api;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal,
    backend::{Backend, CrosstermBackend},
    layout::{Alignment, Constraint, Direction, Layout},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use std::{
    io::{self},
    process::Stdio,
    sync::{Arc, Mutex as StdMutex},
    time::{Duration},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::UnixStream,
    process::{Child, Command},
    sync::{
        mpsc::{self, Receiver, Sender},
        Mutex as AsyncMutex,
    },
    task,
    time::sleep,
};
use hifi_core::models::Track;
use image::imageops::{self, FilterType};

#[tokio::main]
async fn main() -> Result<()> {
    let api = Api::new();

    let current: Arc<AsyncMutex<Option<Child>>> = Arc::new(AsyncMutex::new(None));
    let playback = Arc::new(StdMutex::new(PlaybackState::default()));
    let (player_tx, mut player_rx) = mpsc::channel::<PlayerCommand>(8);

    let active = current.clone();
    let playback_for_player = playback.clone();

    task::spawn(async move {
        let socket_path = "/tmp/harp.rs-mpv.sock";

        while let Some(cmd) = player_rx.recv().await {
            match cmd {
                PlayerCommand::Load { url, duration } => {
                    {
                        let mut guard = active.lock().await;
                        if let Some(child) = guard.as_mut() {
                            let _ = child.kill().await;
                        }
                        *guard = None;
                    }

                    let _ = std::fs::remove_file(socket_path);

                    if let Ok(mut state) = playback_for_player.lock() {
                        state.position = 0.0;
                        state.duration = Some(duration);
                        state.paused = false;
                        state.loaded = true;
                    }

                    let child = Command::new("mpv")
                        .arg("--no-video")
                        .arg(format!("--input-ipc-server={}", socket_path))
                        .arg(&url)
                        .stderr(Stdio::null())
                        .stdout(Stdio::null())
                        .spawn()
                        .expect("failed to spawn");

                    let mut guard = active.lock().await;
                    *guard = Some(child);
                }

                PlayerCommand::TogglePause => {
                    let cmd = r#"{"command":["cycle","pause"]}"#;
                    let _ = send_mpv_ipc(socket_path, cmd).await;
                }

                PlayerCommand::Seek(offset) => {
                    let cmd = format!(r#"{{"command":["seek",{},"relative"]}}"#, offset);
                    let _ = send_mpv_ipc(socket_path, &cmd).await;
                }

                PlayerCommand::Stop => {
                    let mut guard = active.lock().await;
                    if let Some(child) = guard.as_mut() {
                        let _ = child.kill().await;
                    }
                    *guard = None;

                    if let Ok(mut state) = playback_for_player.lock() {
                        state.position = 0.0;
                        state.duration = None;
                        state.paused = false;
                        state.loaded = false;
                    }
                }
            }
        }
    });

    let playback_poll = playback.clone();
    task::spawn(async move {
        let socket_path = "/tmp/harp.rs-mpv.sock";

        loop {
            sleep(Duration::from_millis(250)).await;

            if let Some(pos) = query_mpv_f32(socket_path, "time-pos").await {
                if let Ok(mut state) = playback_poll.lock() {
                    state.position = pos.max(0.0);
                }
            }

            if let Some(duration) = query_mpv_f32(socket_path, "duration").await {
                if let Ok(mut state) = playback_poll.lock() {
                    if duration.is_finite() && duration > 0.0 {
                        state.duration = Some(duration);
                    }
                }
            }

            if let Some(paused) = query_mpv_bool(socket_path, "pause").await {
                if let Ok(mut state) = playback_poll.lock() {
                    state.paused = paused;
                }
            }
        }
    });

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = App::new(api, player_tx, playback);
    let res = app.run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

async fn send_mpv_ipc(socket_path: &str, command: &str) -> Result<()> {
    if let Ok(mut stream) = UnixStream::connect(socket_path).await {
        stream.write_all(command.as_bytes()).await?;
        stream.write_all(b"\n").await?;
        let _ = stream.shutdown().await;
    }
    Ok(())
}

async fn query_mpv_value(socket_path: &str, property: &str) -> Option<String> {
    let stream = UnixStream::connect(socket_path).await.ok()?;
    let mut stream = stream;

    let command = format!(r#"{{"command":["get_property","{}"]}}"#, property);
    stream.write_all(command.as_bytes()).await.ok()?;
    stream.write_all(b"\n").await.ok()?;
    let _ = stream.shutdown().await;

    let mut reader = BufReader::new(stream);
    let mut response = String::new();
    reader.read_line(&mut response).await.ok()?;

    extract_mpv_data_value(&response)
}

async fn query_mpv_f32(socket_path: &str, property: &str) -> Option<f32> {
    query_mpv_value(socket_path, property)
        .await?
        .parse::<f32>()
        .ok()
}

async fn query_mpv_bool(socket_path: &str, property: &str) -> Option<bool> {
    match query_mpv_value(socket_path, property).await?.as_str() {
        "true" => Some(true),
        "false" => Some(false),
        _ => None,
    }
}

fn extract_mpv_data_value(response: &str) -> Option<String> {
    let idx = response.find("\"data\":")? + "\"data\":".len();
    let s = response[idx..].trim_start();

    if s.starts_with("null") {
        return None;
    }
    if s.starts_with("true") {
        return Some("true".to_string());
    }
    if s.starts_with("false") {
        return Some("false".to_string());
    }

    let mut end = s.len();
    for (i, ch) in s.char_indices() {
        if ch == ',' || ch == '}' || ch == '\n' {
            end = i;
            break;
        }
    }

    let value = s[..end].trim().trim_matches('"').to_string();
    if value.is_empty() {
        None
    } else {
        Some(value)
    }
}

enum PlayerCommand {
    Load { url: String, duration: f32 },
    TogglePause,
    Seek(i64),
    Stop,
}

#[derive(Default)]
struct PlaybackState {
    position: f32,
    duration: Option<f32>,
    paused: bool,
    loaded: bool,
}

struct NowPlaying {
    duration: f32,
    art: Option<Vec<Line<'static>>>,
}

fn format_t(secs: f32) -> String {
    let secs = secs as u64;
    format!("{:02}:{:02}", secs / 60, secs % 60)
}

fn img_unicode(path: &str, cells: u32) -> Option<Vec<Line<'static>>> {
    let img = image::open(path).ok()?.to_rgb8();
    let (w, h) = img.dimensions();

    let side = w.min(h);
    let x0 = (w - side) / 2;
    let y0 = (h - side) / 2;

    let cropped = imageops::crop_imm(&img, x0, y0, side, side).to_image();
    let img = imageops::resize(&cropped, cells * 2, cells * 2, FilterType::Triangle);

    let (w, h) = img.dimensions();
    let mut lines = Vec::new();

    for y in (0..h).step_by(2) {
        let mut spans = Vec::new();

        for x in 0..w {
            let top = img.get_pixel(x, y);
            let bottom = if y + 1 < h { img.get_pixel(x, y + 1) } else { top };

            spans.push(Span::styled(
                "▀",
                Style::default()
                    .fg(Color::Rgb(top[0], top[1], top[2]))
                    .bg(Color::Rgb(bottom[0], bottom[1], bottom[2])),
            ));
        }

        lines.push(Line::from(spans));
    }

    Some(lines)
}

fn result(selected: bool) -> Style {
    if selected {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(Color::DarkGray)
            .add_modifier(Modifier::ITALIC)
    }
}

struct App {
    api: Api,
    query: String,
    results: Vec<Track>,
    selected: usize,
    player_tx: Sender<PlayerCommand>,
    search_active: bool,
    search_result_rx: Receiver<Vec<Track>>,
    search_result_tx: Sender<Vec<Track>>,
    now_playing: Option<Track>,
    update_rx: Receiver<NowPlaying>,
    update_tx: Sender<NowPlaying>,
    art: Option<Vec<Line<'static>>>,
    playback: Arc<StdMutex<PlaybackState>>,
}

impl App {
    fn new(api: Api, player_tx: Sender<PlayerCommand>, playback: Arc<StdMutex<PlaybackState>>) -> Self {
        let (search_result_tx, search_result_rx) = mpsc::channel(1);
        let (update_tx, update_rx) = mpsc::channel(1);

        Self {
            api,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            player_tx,
            search_active: false,
            search_result_rx,
            search_result_tx,
            now_playing: None,
            update_rx,
            update_tx,
            art: None,
            playback,
        }
    }

    fn progress(&self) -> (f32, f32, f32, bool) {
        if let Ok(state) = self.playback.lock() {
            let duration = state.duration.unwrap_or(0.0);
            let elapsed = state.position.max(0.0).min(duration.max(0.0));
            let progress = if duration > 0.0 {
                (elapsed / duration).min(1.0)
            } else {
                0.0
            };
            (elapsed, duration, progress, state.paused)
        } else {
            (0.0, 0.0, 0.0, false)
        }
    }

    async fn run<B>(mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B: Backend + Send + Sync,
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        loop {
            if let Ok(update) = self.update_rx.try_recv() {
                if let Ok(mut state) = self.playback.lock() {
                    state.duration = Some(update.duration);
                    state.position = 0.0;
                    state.paused = false;
                    state.loaded = true;
                }
                self.art = update.art;
            }

            if let Ok(new_results) = self.search_result_rx.try_recv() {
                self.results = new_results;
                self.selected = 0;
            }

            terminal.draw(|f| {
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints([
                        Constraint::Length(3),
                        Constraint::Min(5),
                        Constraint::Length(3),
                    ])
                    .split(f.area());

                let main_split = Layout::default()
                    .direction(Direction::Horizontal)
                    .constraints([
                        Constraint::Length(45),
                        Constraint::Min(10),
                    ])
                    .split(layout[1]);

                let left_split = Layout::default()
                    .direction(Direction::Vertical)
                    .constraints([
                        Constraint::Length(22),
                        Constraint::Min(6),
                    ])
                    .split(main_split[0]);

                let search_text = if self.search_active {
                    format!("{}|", self.query)
                } else {
                    self.query.clone()
                };

                let search = Paragraph::new(search_text)
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("search"));
                f.render_widget(search, layout[0]);

                let (elapsed, total, progress, paused) = self.progress();

                let title = self
                    .now_playing
                    .as_ref()
                    .map(|t| t.title.clone())
                    .unwrap_or_else(|| "nothing playing".to_string());

                let artist = self
                    .now_playing
                    .as_ref()
                    .and_then(|t| t.artist.clone())
                    .unwrap_or_else(|| "-".to_string());

                let width = 24;
                let filled = (progress * width as f32) as usize;

                let bar: String = (0..width)
                    .map(|i| if i < filled { '█' } else { '─' })
                    .collect();

                let time = format!("{} / {}", format_t(elapsed), format_t(total));

                let art = self
                    .art
                    .clone()
                    .unwrap_or_else(|| vec![Line::from("")]);

                let art_w = Paragraph::new(art)
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("player"));

                f.render_widget(art_w, left_split[0]);

                let mut info = vec![
                    Line::from(title),
                    Line::from(artist),
                    Line::from(""),
                    Line::from(format!("[{}]", bar)),
                    Line::from(time),
                ];

                if paused {
                    info.insert(0, Line::from(""));
                    info.insert(0, Line::from("paused"));
                }

                let info_widget = Paragraph::new(info)
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL));

                f.render_widget(info_widget, left_split[1]);

                let mut result_lines: Vec<Line<'static>> = Vec::new();

                if self.results.is_empty() {
                    result_lines.push(Line::from(""));
                    result_lines.push(Line::from(Span::styled(
                        "no results",
                        Style::default()
                            .fg(Color::DarkGray)
                            .add_modifier(Modifier::ITALIC),
                    )));
                    result_lines.push(Line::from(""));
                } else {
                    result_lines.push(Line::from(""));

                    for (i, track) in self.results.iter().enumerate() {
                        if i != 0 {
                            result_lines.push(Line::from(""));
                        }

                        let display = format!(
                            "{} - {}",
                            track.artist.as_deref().unwrap_or("unknown artist"),
                            track.title
                        );

                        let padded = format!("  {}  ", display);
                        result_lines.push(Line::from(Span::styled(
                            padded,
                            result(i == self.selected),
                        )));
                    }

                    result_lines.push(Line::from(""));
                }

                let right = Paragraph::new(result_lines)
                    .alignment(Alignment::Center)
                    .block(Block::default().borders(Borders::ALL).title("results"));

                f.render_widget(right, main_split[1]);

                let footer = Paragraph::new(
                    "q - quit | / - search | enter - play | space - pause | ←/→ - seek ±5s",
                )
                .alignment(Alignment::Center)
                .block(Block::default().borders(Borders::ALL));

                f.render_widget(footer, layout[2]);
            })?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(KeyEvent { code, .. }) = event::read()? {
                    if self.search_active {
                        match code {
                            KeyCode::Esc => self.search_active = false,
                            KeyCode::Enter => {
                                let query = self.query.trim().to_string();
                                let api = self.api.clone();
                                let tx = self.search_result_tx.clone();

                                tokio::spawn(async move {
                                    let tracks = api.search(&query).await.unwrap_or_default();
                                    let _ = tx.send(tracks).await;
                                });

                                self.search_active = false;
                            }
                            KeyCode::Backspace => {
                                self.query.pop();
                            }
                            KeyCode::Char(ch) => {
                                self.query.push(ch);
                            }
                            _ => {}
                        }
                    } else {
                        match code {
                            KeyCode::Char('q') => {
                                let _ = self.player_tx.send(PlayerCommand::Stop).await;
                                break;
                            }
                            KeyCode::Char('/') => {
                                self.query.clear();
                                self.search_active = true;
                            }
                            KeyCode::Char(' ') => {
                                let _ = self.player_tx.send(PlayerCommand::TogglePause).await;
                            }
                            KeyCode::Left => {
                                let _ = self.player_tx.send(PlayerCommand::Seek(-5)).await;
                            }
                            KeyCode::Right => {
                                let _ = self.player_tx.send(PlayerCommand::Seek(5)).await;
                            }
                            KeyCode::Enter => {
                                if let Some(track) = self.results.get(self.selected).cloned() {
                                    self.now_playing = Some(track.clone());

                                    let player_tx = self.player_tx.clone();
                                    let api = self.api.clone();
                                    let artist = track
                                        .artist
                                        .unwrap_or_else(|| "Unknown Artist".to_string());
                                    let title = track.title.clone();
                                    let artwork = track.artwork.clone();
                                    let update_tx = self.update_tx.clone();

                                    tokio::spawn(async move {
                                        if let Ok((url, duration)) = api.get_url(&artist, &title).await {
                                            let _ = player_tx
                                                .send(PlayerCommand::Load { url, duration })
                                                .await;

                                            let art = if let Some(art_url) = artwork {
                                                if let Ok(img_bytes) = api.artwork(&art_url).await {
                                                    let path = "/tmp/art.jpg";
                                                    let _ = std::fs::write(path, &img_bytes);
                                                    img_unicode(path, 22)
                                                } else {
                                                    None
                                                }
                                            } else {
                                                None
                                            };

                                            let _ = update_tx.send(NowPlaying { duration, art }).await;
                                        }
                                    });
                                }
                            }
                            KeyCode::Down => {
                                if self.selected < self.results.len().saturating_sub(1) {
                                    self.selected += 1;
                                }
                            }
                            KeyCode::Up => {
                                if self.selected > 0 {
                                    self.selected -= 1;
                                }
                            }
                            KeyCode::Backspace => {
                                self.query.pop();
                            }
                            KeyCode::Char(ch) => {
                                self.query.push(ch);
                            }
                            _ => {}
                        }
                    }
                }
            }
        }

        Ok(())
    }
}