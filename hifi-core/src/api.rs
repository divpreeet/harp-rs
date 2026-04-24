use anyhow::{bail, Result};
use reqwest::Client;
use serde::Deserialize;
use tokio::process::Command;

use crate::models::Track;

#[derive(Clone)]
pub struct Api {
    http: Client,
    pub ytdlp: String,
}

#[derive(Debug, Deserialize)]
struct DeezerSearchResponse {
    data: Vec<DeezerTrack>,
}

#[derive(Debug, Deserialize, Clone)]
struct DeezerTrack {
    id: u64,
    title: String,
    artist: DeezerArtist,
    album: DeezerAlbum,
}

#[derive(Debug, Deserialize, Clone)]
struct DeezerArtist {
    name: String,
}

#[derive(Debug, Deserialize, Clone)]
struct DeezerAlbum {
    title: String,
    #[serde(default)]
    release_date: Option<String>,
    #[serde(default)]
    cover_xl: Option<String>,
    #[serde(default)]
    cover_big: Option<String>,
    #[serde(default)]
    cover_medium: Option<String>,
    #[serde(default)]
    cover: Option<String>,
}

fn non_empty(opt: Option<String>) -> Option<String> {
    opt.and_then(|s| {
        let s = s.trim().to_string();
        if s.is_empty() {
            None
        } else {
            Some(s)
        }
    })
}

fn pick_artwork(album: &DeezerAlbum) -> Option<String> {
    non_empty(album.cover_xl.clone())
        .or_else(|| non_empty(album.cover_big.clone()))
        .or_else(|| non_empty(album.cover_medium.clone()))
        .or_else(|| non_empty(album.cover.clone()))
}

fn parse_year(release_date: &Option<String>) -> Option<u16> {
    let date = release_date.as_deref()?.trim();
    if date.len() >= 4 {
        date[..4].parse::<u16>().ok()
    } else {
        None
    }
}

impl Api {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
            ytdlp: "yt-dlp".to_string(),
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }

        let res = self
            .http
            .get("https://api.deezer.com/search")
            .query(&[("q", q), ("limit", "10")])
            .header("User-Agent", "hifi-cli/0.1 (+https://github.com/divpreeet)")
            .send()
            .await?
            .error_for_status()?;

        let data: DeezerSearchResponse = res.json().await?;

        Ok(data
            .data
            .into_iter()
            .map(|item| {
                let album = item.album.clone();

                Track {
                    id: item.id.to_string(),
                    title: item.title,
                    artist: Some(item.artist.name),
                    album: Some(album.title.clone()),
                    year: parse_year(&album.release_date),
                    artwork: pick_artwork(&album),
                }
            })
            .collect())
    }

    pub async fn artwork(&self, artwork_url: &str) -> Result<Vec<u8>> {
        let url = artwork_url.trim();
        if url.is_empty() {
            bail!("empty artwork url");
        }

        let response = self.http.get(url).send().await?.error_for_status()?;
        Ok(response.bytes().await?.to_vec())
    }

    pub async fn get_url(&self, artist: &str, title: &str) -> Result<String> {
        let yt_q = format!("ytsearch1:{} - {} topic", artist.trim(), title.trim());

        let output = Command::new(&self.ytdlp)
            .arg(&yt_q)
            .arg("--print")
            .arg("%(id)s")
            .output()
            .await?;

        if !output.status.success() {
            bail!(
                "ytdlp failed to extract url: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        let out = String::from_utf8_lossy(&output.stdout);
        let video_id = out.lines().next().unwrap_or("").trim();

        if video_id.is_empty() {
            bail!("yt-dlp returned an empty video id");
        }

        Ok(format!("https://www.youtube.com/watch?v={}", video_id))
    }
}