use anyhow::{Result, anyhow, bail};
use base64::Engine;
use reqwest::Client;
use serde_json::Value;

use crate::models::{SearchResponse, Track};

#[derive(Clone)]
pub struct Api {
    client: Client,
    search_base: &'static str,
    playback_bases: [&'static str; 4],
}

impl Api {
    pub fn new() -> Self {
        Self {
            client: Client::new(),
            search_base: "https://api.monochrome.tf",
            playback_bases: ["https://hund.qqdl.site", "https://frankfurt-2.monochrome.tf", "https://wolf.qqdl.site", "https://api.monochrome.tf"],
        }
    }

    pub async fn search(&self, query: &str) -> Result<Vec<Track>> {
        let q = query.trim();
        if q.is_empty() {
            return Ok(vec![]);
        }

        let url = format!("{}/search/", self.search_base);
        let response = self.client.get(url).query(&[("s", q)]).send().await?;
        let status = response.status();
        let body = response.text().await?;

        if !status.is_success() {
            bail!("http {}: {}", status.as_u16(), body);
        }

        let parsed: SearchResponse = serde_json::from_str(&body)?;
        Ok(parsed.data.items)
    }

    pub async fn get_url(&self, id: i64) -> Result<String> {
        let qualities = ["LOSSLESS", "HIGH", "LOW", "HI_RES_LOSSLESS"];

        for base in self.playback_bases {
            for quality in qualities {
                if let Some(url) = self.try_playback(base, id, quality).await? {
                    println!("playback resolved {}, quality={}", base, quality);
                    return Ok(url);
                }
            }
        }

        Err(anyhow!("playback unavailable"))
    }

    async fn try_playback(
        &self,
        base: &str,
        track_id: i64,
        quality: &str,
    ) -> Result<Option<String>> {
        let url = format!("{}/track/", base);
        let response = self
            .client
            .get(url)
            .query(&[
                ("id", track_id.to_string()),
                ("quality", quality.to_string()),
            ])
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        println!(
            "track status={}, quality={}, host={}",
            status.as_u16(),
            quality,
            base
        );
        println!("track raw {}", body.chars().take(1000).collect::<String>());
        if !status.is_success() {
            return Ok(None);
        }

        let root: Value = match serde_json::from_str(&body) {
            Ok(v) => v,
            Err(_) => return Ok(None),
        };

        let data_obj = match root.get("data") {
            Some(v) => v,
            None => return Ok(None),
        };

        let direct_candidates = extract_urls(data_obj);
        if let Some(best) = direct_candidates
            .into_iter()
            .find(|u| u.starts_with("http"))
        {
            return Ok(Some(best));
        }

        if let Some(manifest_b64) = data_obj.get("manifest").and_then(|v| v.as_str()) {
            if let Ok(manifest_data) =
                base64::engine::general_purpose::STANDARD.decode(manifest_b64)
            {
                if let Ok(manifest_json) = serde_json::from_slice::<Value>(&manifest_data) {
                    if let Some(urls) = manifest_json.get("urls").and_then(|u| u.as_array()) {
                        if let Some(first) = urls
                            .iter()
                            .filter_map(|x| x.as_str())
                            .find(|u| u.starts_with("http"))
                        {
                            return Ok(Some(first.to_string()));
                        }
                    }

                    let nested = extract_urls(&manifest_json);
                    if let Some(any) = nested.into_iter().find(|u| u.starts_with("http")) {
                        return Ok(Some(any));
                    }
                }
            }
        }

        Ok(None)
    }
}

fn extract_urls(v: &Value) -> Vec<String> {
    let mut out = Vec::new();
    extract_urls_inner(v, &mut out);
    out.sort();
    out.dedup();
    out
}

fn extract_urls_inner(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::String(s) => {
            if s.starts_with("http://") || s.starts_with("https://") {
                out.push(s.clone());
            }
        }
        Value::Array(arr) => {
            for item in arr {
                extract_urls_inner(item, out);
            }
        }
        Value::Object(map) => {
            for value in map.values() {
                extract_urls_inner(value, out);
            }
        }
        _ => {}
    }
}
