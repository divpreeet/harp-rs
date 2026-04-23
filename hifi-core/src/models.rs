use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SearchResponse {
    pub data: SearchData,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SearchData {
    pub items: Vec<Track>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Track {
    pub id: i64,
    pub title: String,
    pub artist: Option<Artist>,
    pub album: Option<Album>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Artist {
    pub name: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Album {
    pub title: Option<String>,
    pub artwork: Option<String>,
}
