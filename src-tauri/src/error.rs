// Wave
// Copyright (C) 2025 BMDarkLight
//
// Licensed under the GNU Affero General Public License v3.0 (AGPL-3.0).
// See the LICENSE file in the project root for the full license text
// and additional terms (attribution and fork-marking requirements).
// https://github.com/BMDarkLight/Wave

use std::io;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum AppError {
    #[error("Audio error: {0}")]
    Audio(#[from] AudioError),

    #[error("Database error: {0}")]
    Database(#[from] DatabaseError),

    #[error("Metadata error: {0}")]
    Metadata(#[from] MetadataError),

    #[error("IO error: {0}")]
    Io(#[from] io::Error),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("Tauri error: {0}")]
    Tauri(#[from] tauri::Error),

    #[error("Invalid argument: {0}")]
    InvalidArgument(String),

    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Operation failed: {0}")]
    OperationFailed(String),

    #[error("Configuration error: {0}")]
    Config(String),
}

#[derive(Error, Debug)]
pub enum AudioError {
    #[error("Failed to create output stream: {0}")]
    StreamCreation(String),

    #[error("Failed to create sink: {0}")]
    SinkCreation(String),

    #[error("Failed to open audio file: {0}")]
    FileOpen(String),

    #[error("Failed to decode audio: {0}")]
    Decode(String),

    #[error("Unsupported audio format: {0}")]
    UnsupportedFormat(String),

    #[error("No track currently loaded")]
    NoTrackLoaded,

    #[error("Track already playing")]
    AlreadyPlaying,

    #[error("Seek position out of bounds")]
    SeekOutOfBounds,

    #[error("Volume must be between 0.0 and 1.0")]
    InvalidVolume,

    #[error("Audio device not available: {0}")]
    DeviceUnavailable(String),

}

#[derive(Error, Debug)]
pub enum DatabaseError {
    #[error("Connection error: {0}")]
    Connection(String),

    #[error("Query error: {0}")]
    Query(String),

    #[error("Migration error: {0}")]
    Migration(String),

    #[error("Constraint violation: {0}")]
    Constraint(String),

    #[error("Record not found: {0}")]
    NotFound(String),

}

#[derive(Error, Debug)]
pub enum MetadataError {
    #[error("File not found: {0}")]
    FileNotFound(String),

    #[error("Unsupported file extension: {0}")]
    UnsupportedExtension(String),

    #[error("Failed to read file metadata: {0}")]
    ReadMetadata(String),

    #[error("Failed to probe audio format: {0}")]
    ProbeFormat(String),

    #[error("No audio tracks found in file")]
    NoAudioTracks,

    #[error("Invalid tag data: {0}")]
    InvalidTag(String),
}

impl From<AppError> for String {
    fn from(err: AppError) -> Self {
        err.to_string()
    }
}

impl From<AudioError> for String {
    fn from(err: AudioError) -> Self {
        err.to_string()
    }
}

impl From<DatabaseError> for String {
    fn from(err: DatabaseError) -> Self {
        err.to_string()
    }
}

impl From<MetadataError> for String {
    fn from(err: MetadataError) -> Self {
        err.to_string()
    }
}
