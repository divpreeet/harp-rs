use std::fs::OpenOptions;

use anyhow::{Result, bail};
use reqwest::Client;
use serde_json::ser;
use tokio::process::Command;
use crate::models::Track;
use serde::Deserialize;


#[derive(Deserialize)]
struct DiscogsRelease {
    title: String,
    #[serde(default)]
    year: Option<String>,
    #[serde(default)]
    cover_image: Option<String>,
    #[serde(default)]
    thumb: Option<String>,
    #[serde(default)]
    id: Option<u32>,
    #[serde(default)]
    resource_url: Option<String>
}

#[derive(Deserialize)]
struct DiscogsResults {
    results: Vec<DiscogsRelease>
}

#[derive(Clone)]
pub struct Api {
    http: Client,
    // yt dlpe path
    pub ytdlp: String,
}

fn split_artist(s: &str) -> (String, String) {
    if let Some(idx) = s.find(" - ") {
        let (artist, title) = s.split_at(idx);
        (artist.trim().to_string(), title[3..].trim().to_string())
    } else {
        ("unknown artist".to_string(), s.trim().to_string())
    }
}

impl Api {
    pub fn new() -> Self {
        Self {
            http: Client::new(),
            ytdlp: "yt-dlp".to_string()
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }

        let url = "https://api.discogs.com/database/search";
        let params = [
            ("q", q),
            ("type", "release"),
            ("per_page", "10") 
        ];

        let res = self.http.get(url).query(&params).header("User-Agent", "harp-rs/0.1 +https://github.com/divpreeet").send().await?.error_for_status()?;

        let data: DiscogsResults = res.json().await?;
        let tracks = data.results.into_iter().map(|item| {
            let (title, artist) = split_artist(&item.title);
            Track {
                id: item.resource_url.clone().unwrap_or_else(|| item.id.map_or_else(|| format!("{}-{}", title, artist), |i| i.to_string())),
                title: title.to_string(),
                artist: Some(artist.to_string()),
                album: None,
                year: item.year.as_ref().and_then(|s| s.parse::<u16>().ok()),
                artwork: item.cover_image.or(item.thumb),
            }
        }).collect();
        Ok(tracks)
    }

    pub async fn get_url(&self, artist: &str, title: &str) -> Result<String> {
        let yt_q = format!("ytsearch1:{} - {} topic", artist, title);
        let output = Command::new(&self.ytdlp).arg(&yt_q).arg("--print").arg("%(id)s").output().await?;

        if !output.status.success() {
            bail!(
                "ytdlp failed to extract url {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let out = String::from_utf8_lossy(&output.stdout);
        let video_id = out.lines().next().unwrap_or("").trim();
        let watch_url = format!("https://www.youtube.com/watch?v={}", video_id);
        Ok(watch_url)
    }
}
        