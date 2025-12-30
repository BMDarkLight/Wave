use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub struct AudioPlayer {
    handle: &'static OutputStreamHandle,
    sink: Arc<Mutex<Option<Sink>>>,
    current_path: Arc<Mutex<Option<PathBuf>>>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self, String> {
        let (_stream, handle) = OutputStream::try_default()
            .map_err(|e| format!("Failed to create output stream: {}", e))?;
        
        // Leak the stream to keep it alive for the program lifetime
        // The handle is Send + Sync, so we can safely store a static reference
        Box::leak(Box::new(_stream));
        let handle = Box::leak(Box::new(handle));
        
        Ok(Self {
            handle,
            sink: Arc::new(Mutex::new(None)),
            current_path: Arc::new(Mutex::new(None)),
        })
    }

    pub fn play(&self, path: &str) -> Result<(), String> {
        // Stop current playback if any
        self.stop()?;

        let file = File::open(path)
            .map_err(|e| format!("Failed to open file: {}", e))?;
        
        let source = Decoder::new(BufReader::new(file))
            .map_err(|e| format!("Failed to decode audio: {}", e))?;

        let sink = Sink::try_new(self.handle)
            .map_err(|e| format!("Failed to create sink: {}", e))?;

        sink.append(source);
        sink.play();

        *self.sink.lock().unwrap() = Some(sink);
        *self.current_path.lock().unwrap() = Some(PathBuf::from(path));

        Ok(())
    }

    pub fn pause(&self) -> Result<(), String> {
        if let Some(ref sink) = *self.sink.lock().unwrap() {
            sink.pause();
            Ok(())
        } else {
            Err("No track is currently playing".to_string())
        }
    }

    pub fn resume(&self) -> Result<(), String> {
        if let Some(ref sink) = *self.sink.lock().unwrap() {
            sink.play();
            Ok(())
        } else {
            Err("No track is currently paused".to_string())
        }
    }

    pub fn stop(&self) -> Result<(), String> {
        if let Some(sink) = self.sink.lock().unwrap().take() {
            sink.stop();
        }
        *self.current_path.lock().unwrap() = None;
        Ok(())
    }

    pub fn is_playing(&self) -> bool {
        if let Some(ref sink) = *self.sink.lock().unwrap() {
            !sink.is_paused() && sink.len() > 0
        } else {
            false
        }
    }

    pub fn is_paused(&self) -> bool {
        if let Some(ref sink) = *self.sink.lock().unwrap() {
            sink.is_paused()
        } else {
            false
        }
    }

    pub fn get_current_path(&self) -> Option<PathBuf> {
        self.current_path.lock().unwrap().clone()
    }
}