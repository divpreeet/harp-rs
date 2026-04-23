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
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::{process::Command, sync::mpsc, task};

#[tokio::main]
async fn main() -> Result<()> {
    let api = Api::new();

    let _sel_track: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
    let (player_tx, mut player_rx) = mpsc::channel::<String>(5);

    task::spawn(async move {
        while let Some(url) = player_rx.recv().await {
            println!("Playing track URL: {}", url);

            let status = Command::new("mpv").arg("--no-video").arg(&url).status().await;

            if let Err(err) = status {
                eprintln!("Failed to play audio {}", err);
            }
        }
    });

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let app = App::new(api, player_tx);
    let res = app.run(&mut terminal).await;

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    res
}

struct App {
    api: Api,
    query: String,
    results: Vec<(String, String)>,
    selected: usize,
    player_tx: mpsc::Sender<String>,
}

impl App {
    fn new(api: Api, player_tx: mpsc::Sender<String>) -> Self {
        Self {
            api,
            query: String::new(),
            results: Vec::new(),
            selected: 0,
            player_tx,
        }
    }
    async fn run<B>(mut self, terminal: &mut Terminal<B>) -> Result<()>
    where
        B: Backend + Send + Sync,
        B::Error: std::error::Error + Send + Sync + 'static,
    {
        loop {
            terminal.draw(|f| {
                let layout = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints(
                        [
                            Constraint::Length(3),
                            Constraint::Min(5),
                            Constraint::Length(3),
                        ]
                        .as_ref(),
                    )
                    .split(f.area());

                let search_bar = Paragraph::new(self.query.as_str()).block(Block::default().borders(Borders::ALL).title("search"));
                f.render_widget(search_bar, layout[0]);

                let items: Vec<ListItem> = self
                    .results
                    .iter()
                    .enumerate()
                    .map(|(i, (title, _))| {
                        let style = if i == self.selected {
                            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
                        } else {
                            Style::default()
                        };

                        ListItem::new(Span::styled(title, style))
                    })
                    .collect();

                let results_list = List::new(items)
                    .block(Block::default().borders(Borders::ALL).title("results"));
                f.render_widget(results_list, layout[1]);

                let now_playing = Paragraph::new("arrow keys to navigage | enter - play | / - search | q - quit")
                    .block(Block::default().borders(Borders::ALL).title("controls"));
                f.render_widget(now_playing, layout[2]);
            })?;

            if event::poll(Duration::from_millis(100))? {
                if let Event::Key(KeyEvent { code, .. }) = event::read()? {
                    match code {
                        KeyCode::Char('q') => break,
                        KeyCode::Char('/') => {
                            self.query.clear();
                        }
                        KeyCode::Enter => {
                            if let Some((_title, track_id)) = self.results.get(self.selected).cloned() {
                                let track_id = track_id.clone();
                                let player_tx = self.player_tx.clone();
                                let api = self.api.clone();
                                tokio::spawn(async move {
                                    if let Ok(url) = api.get_url(track_id.parse::<i64>().unwrap()).await {
                                        let _ = player_tx.send(url).await;
                                    } else {
                                        println!("could not query url")
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
                                self.results = self.search().await?;
                                self.selected = 0;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        Ok(())
    }

    async fn search(&self) -> Result<Vec<(String, String)>> {
        let tracks = self.api.search(self.query.trim()).await?;
        let results: Vec<(String, String)> = tracks
            .into_iter()
            .map(|track| {
                let title = format!(
                    "{} - {}",
                    track.title,
                    track
                        .artist
                        .and_then(|a| a.name.clone())
                        .unwrap_or_else(|| "Unknown Artist".to_string())
                );
                (title, track.id.to_string())
            })
            .collect();

        Ok(results)
    }
}