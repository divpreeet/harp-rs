use hifi_core::api::Api;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    Terminal, backend::{Backend, CrosstermBackend}, layout::{Alignment, Constraint, Direction, Layout}, style::{Color, Modifier, Style}, text::{Line, Span}, widgets::{Block, Borders, List, ListItem, Paragraph}
};
use std::{
    io::{self}, process::Stdio, sync::Arc, time::{Duration, Instant}
};
use tokio::{process::{Child, Command}, sync::{Mutex, mpsc::{self, Receiver, Sender}}, task};
use hifi_core::models::Track;
use image::imageops::{self, FilterType};
#[tokio::main]

async fn main() -> Result<()> {
    let api = Api::new();

    let _sel_track: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let current: Arc<Mutex<Option<Child>>> = Arc::new(Mutex::new(None));
    let (player_tx, mut player_rx) = mpsc::channel::<String>(5);

    let active = current.clone();
    task::spawn(async move {
        while let Some(url) = player_rx.recv().await {
            {
                let mut maybe_child = active.lock().await;
                if let Some(child) = maybe_child.as_mut() {
                    let _ = child.kill().await;
                }
            }

            let  child = Command::new("mpv").arg("--no-video").arg(&url).stderr(Stdio::null()).stdout(Stdio::null()).spawn().expect("failed to spawn");

            {
                let mut maybe_child = active.lock().await;
                *maybe_child = Some(child);
            }
        }
    });

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = App::new(api, player_tx, current.clone());
    let res = app.run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

#[derive(Clone, Copy, PartialEq)]
enum ViewMode{
    Search,
    Player,
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

struct App {
    api: Api,
    query: String,
    results: Vec<Track>,
    selected: usize,
    player_tx: mpsc::Sender<String>,
    search_active: bool,
    status: String,
    search_result_rx: Receiver<Vec<Track>>,
    search_result_tx: Sender<Vec<Track>>,
    current: Arc<Mutex<Option<Child>>>,
    view_mode: ViewMode,
    now_playing: Option<Track>,
    started_at: Option<Instant>,
    track_duration: Option<f32>,
    update_rx: Receiver<NowPlaying>,
    update_tx: Sender<NowPlaying>,
    art: Option<Vec<Line<'static>>>
}



impl App {
    fn new(api: Api, player_tx: mpsc::Sender<String>, current: Arc<Mutex<Option<Child>>>) -> Self {
        let (search_result_tx, search_result_rx) = mpsc::channel(1);
        let (update_tx, update_rx)= mpsc::channel(1);
        Self {
            api,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            player_tx,
            search_active: false,
            status: String::new(),
            search_result_rx,
            search_result_tx,
            current,
            view_mode: ViewMode::Search,
            now_playing: None,
            started_at: None,
            track_duration: None,
            update_rx,
            update_tx,
            art: None
        }
    }

    fn progress(&self) -> (f32, f32, f32) {
        if let (Some(start), Some(duration)) = (self.started_at, self.track_duration) {
            let elapsed = start.elapsed().as_secs_f32();
            let progress = (elapsed / duration).min(1.0);
            (elapsed, duration, progress)
        } else {
            (0.0, 0.0, 0.0)
        }
    }
    async fn run<B>(mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B: Backend + Send + Sync,
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        loop {
            if let Ok(update) = self.update_rx.try_recv() {
                self.track_duration = Some(update.duration);
                self.started_at = Some(Instant::now());
                self.art = update.art
            } 

            if let Ok(new_results) = self.search_result_rx.try_recv() {
                self.results = new_results;
                self.selected = 0;
                self.status = "ready".to_string();
            }

            terminal.draw(|f| {
                let layout = Layout::default().direction(Direction::Vertical).margin(1).constraints([
                    Constraint::Length(3),
                    Constraint::Min(5),
                    Constraint::Length(3)
                ]).split(f.area());

                let main_split = Layout::default().direction(Direction::Horizontal).constraints([
                        Constraint::Length(45),
                        Constraint::Min(10)
                    ]).split(layout[1]);

                let left_split = Layout::default().direction(
                    Direction::Vertical).constraints([
                        Constraint::Length(22),
                        Constraint::Min(6),
                    ]).split(main_split[0]);

                let mut input = self.query.clone();
                if self.search_active {
                    input.push('|')
                }
                
                let search = Paragraph::new(self.query.clone()).block(Block::default().borders(Borders::ALL).title("search"));
                f.render_widget(search, layout[0]);

                // left panel                
                let (elapsed, total, progress) = self.progress();
                
                let title = self.now_playing.as_ref().map(|t| t.title.clone()).unwrap_or_else(|| "nothing playing".to_string());

                let artist = self.now_playing.as_ref().and_then(|t| t.artist.clone()).unwrap_or_else(|| "-".to_string());
                
                // rpogress bar
                let width = 24;
                let filled = (progress * width as f32) as usize;

                let bar: String = (0..width).map(|i| if i < filled { '█' } else { '─' }).collect();

                let time = format!("{} / {}", format_t(elapsed), format_t(total));

                let art = self.art.clone().unwrap_or_else(|| vec![Line::from("loading artwotk")]);
                
                let art_w = Paragraph::new(art).alignment(Alignment::Center).block(Block::default().borders((Borders::ALL)));

                f.render_widget(art_w, left_split[0]);

                let info = vec![
                    Line::from(title),
                    Line::from(artist),
                    Line::from(""),
                    Line::from(format!("[{}]", bar)),
                    Line::from(time),
                ];

                let info_widget = Paragraph::new(info)
                    .block(Block::default().borders(Borders::ALL).title("player"));

                f.render_widget(info_widget, left_split[1]);

                let items: Vec<ListItem> = self.results.iter().enumerate().map(|(i, track)| {
                    let style = if i == self.selected {
                        Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                    } else {
                        Style::default()
                    };
                    
                    let display = format!("{} - {}", track.artist.as_deref().unwrap_or("unknown artist"), track.title);
                    
                    ListItem::new(Span::styled(display, style))
                }).collect();
                

                let right = List::new(items).block(Block::default().borders(Borders::ALL).title("results"));
                f.render_widget(right, main_split[1]);

                let footer = Paragraph::new("controls").block(Block::default().borders(Borders::ALL));
                f.render_widget(footer, layout[2]);



            })?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(KeyEvent { code, .. }) = event::read()? {
                    match code {
                        KeyCode::Char('q')  => {
                            let mut guard = self.current.lock().await;
                            if let Some(child) = guard.as_mut() {
                                let _ = child.kill().await;
                            }
                            break;
                        }
                        KeyCode::Char('/') => {
                            self.query.clear();
                            self.search_active = true;
                        }
                        KeyCode::Enter => {
                            if self.search_active {
                                self.status = "searching".to_string();
                                let query = self.query.trim().to_string();
                                let api = self.api.clone();
                                let search_result_tx = self.search_result_tx.clone();
                                tokio::spawn(async move {
                                    let tracks = api.search(&query).await.unwrap_or_default();
                                    let _ = search_result_tx.send(tracks).await;
                                });
                                self.search_active = false;
                            } else if let Some(track) = self.results.get(self.selected).cloned() {
                                self.status = "playing".to_string();
                                self.now_playing = Some(track.clone());
                                let player_tx = self.player_tx.clone();
                                let api = self.api.clone();
                                let artist = track.artist.unwrap_or_else(|| "Unknown Artist".to_string());
                                let title = track.title.clone();
                                let artwork = track.artwork.clone();
                                let update_tx = self.update_tx.clone();

                                tokio::spawn(async move {
                                    if let Ok((url, duration)) = api.get_url(&artist, &title).await {
                                        let _ = player_tx.send(url).await;
                                        
                                        let art = if let Some(art_url) = artwork {
                                            if let Ok(img_bytes) = api.artwork(&art_url).await {
                                                let path = "/tmp/art.jpg";
                                                let _ = std::fs::write(path, &img_bytes);
                                                img_unicode(path, 24)
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
                        KeyCode::Char(ch) => {
                            self.query.push(ch);
                        }
                        KeyCode::Backspace => {
                            self.query.pop();
                        }
                        KeyCode::Esc => {
                            if !self.query.is_empty() {
                                self.status = "searching".to_string();
                                let query = self.query.trim().to_string();
                                let api = self.api.clone();
                                let search_result_tx = self.search_result_tx.clone();
                                tokio::spawn(async move {
                                    let tracks = api.search(&query).await.unwrap_or_default();
                                    let _ = search_result_tx.send(tracks).await;
                                });
                                self.search_active = false;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }
}
