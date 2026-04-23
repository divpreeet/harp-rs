use hifi_core::api::Api;
use std::io::{self, Write};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let api = Api::new();

    print!("enter query: ");
    io::stdout().flush()?;

    let mut q = String::new();
    io::stdin().read_line(&mut q)?;
    let q = q.trim();

    let tracks = api.search(q).await?;
    if tracks.is_empty() {
        println!("no results");
        return Ok(());
    }

    println!("\nresults:");
    for (i, t) in tracks.iter().take(10).enumerate() {
        let artist = t
            .artist
            .as_ref()
            .and_then(|a| a.name.clone())
            .unwrap_or_else(|| "unknown artist".to_string());

        println!("[{}] {} - {} - id={}", i, t.title, artist, t.id);
    }

    print!("\npick index to resolve url: ");
    io::stdout().flush()?;

    let mut idx = String::new();
    io::stdin().read_line(&mut idx)?;
    let idx: usize = idx.trim().parse().unwrap_or(0);

    if let Some(track) = tracks.get(idx) {
        let url = api.get_url(track.id).await?;
        println!("playback url: {}", url);
    } else {
        println!("invalid index");
    }

    Ok(())
}