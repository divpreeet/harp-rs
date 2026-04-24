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
pub struct Track{
    pub id: String,
    pub title: String,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub year: Option<u16>,
    pub artwork: Option<String>
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
