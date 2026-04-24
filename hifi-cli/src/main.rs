use hifi_core::api::Api;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEvent},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    layout::{Constraint, Direction, Layout},
    style::{Modifier, Style},
    text::Span,
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Terminal,
};
use std::{
    io::{self},
    sync::Arc,
    time::Duration,
    process::Stdio
};
use tokio::{process::{Command, Child}, sync::{Mutex, mpsc, mpsc::{Sender, Receiver}}, task};
use hifi_core::models::Track;
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
}

impl App {
    fn new(api: Api, player_tx: mpsc::Sender<String>, current: Arc<Mutex<Option<Child>>>) -> Self {
        let (search_result_tx, search_result_rx) = mpsc::channel(1);
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
        }
    }
    async fn run<B>(mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B: Backend + Send + Sync,
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        loop {
            if let Ok(new_results) = self.search_result_rx.try_recv() {
                self.results = new_results;
                self.selected = 0;
                self.status = "ready".to_string();
            }

            terminal.draw(|f| {
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints(
                        [
                            Constraint::Length(3),
                            Constraint::Min(5),
                            Constraint::Length(3),
                            Constraint::Length(3),
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                let mut input = self.query.clone();
                if self.search_active {
                    input.push('|')
                }
                let search_bar = Paragraph::new(input).block(Block::default().borders(Borders::ALL).title("search"));

                let items: Vec<ListItem> = self
                    .results
                    .iter()
                    .enumerate()
                    .map(|(i, track)| {
                        let style = if i == self.selected {
                            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                        } else {
                            Style::default()
                        };
                        let display = format!(
                            "{} - {}",
                            track.artist.as_deref().unwrap_or("Unknown Artist"),
                            track.title
                        );
                        ListItem::new(Span::styled(display, style))
                    })
                    .collect();

                f.render_widget(search_bar, layout[0]);

                let status_bar = Paragraph::new(format!("status: {}", self.status))
                    .block(Block::default().borders(Borders::ALL));
                let results_list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title("results"));
                f.render_widget(results_list, layout[1]);

                let now_playing = Paragraph::new("arrow keys to navigage | enter - play | / - search | q - quit")
                    .block(Block::default().borders(Borders::ALL).title("controls"));
                f.render_widget(now_playing, layout[2]);
                f.render_widget(status_bar, layout[3]);
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
                                let player_tx = self.player_tx.clone();
                                let api = self.api.clone();
                                let artist = track.artist.unwrap_or_else(|| "Unknown Artist".to_string());
                                let title = track.title.clone();
                                let artwork = track.artwork.clone();

                                tokio::spawn(async move {
                                    if let Ok(url) = api.get_url(&artist, &title).await {
                                        let _ = player_tx.send(url).await;
                                    } else {
                                        eprintln!("coudl not query url");
                                    }

                                    if let Some(art_url) = artwork {
                                        if let Ok(img_bytes) = api.artwork(&art_url).await {
                                            let path = "/tmp/art.jpg";
                                            let _ = std::fs::write(path, &img_bytes);
                                        }
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
