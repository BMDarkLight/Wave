use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Track {
    pub path: String,
    pub name: String,
}

impl Track {
    pub fn from_path(path: String) -> Self {
        let name = PathBuf::from(&path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("Unknown")
            .to_string();
        
        Self { path, name }
    }
}

pub struct Playlist {
    tracks: Mutex<Vec<Track>>,
}

impl Playlist {
    pub fn new() -> Self {
        Self {
            tracks: Mutex::new(Vec::new()),
        }
    }

    pub fn add_track(&self, path: String) -> Track {
        let track = Track::from_path(path);
        self.tracks.lock().unwrap().push(track.clone());
        track
    }

    pub fn remove_track(&self, index: usize) -> Result<(), String> {
        let mut tracks = self.tracks.lock().unwrap();
        if index < tracks.len() {
            tracks.remove(index);
            Ok(())
        } else {
            Err("Index out of bounds".to_string())
        }
    }

    pub fn get_tracks(&self) -> Vec<Track> {
        self.tracks.lock().unwrap().clone()
    }

    pub fn clear(&self) {
        self.tracks.lock().unwrap().clear();
    }

    pub fn get_track(&self, index: usize) -> Option<Track> {
        self.tracks.lock().unwrap().get(index).cloned()
    }
}

