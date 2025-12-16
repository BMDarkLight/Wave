use rodio::{OutputStream, Sink};
use std::fs::File;
use std::io::BufReader;

pub struct AudioPlayer {
    _stream: OutputStream,
    sink: Sink,
}

impl AudioPlayer {
    pub fn new() -> Self {
        let (_stream, handle) = OutputStream::try_default().unwrap();
        let sink = Sink::try_new(&handle).unwrap();
        Self { _stream, sink }
    }

    pub fn play(&self, path: &str) {
        let file = File::open(path).unwrap();
        let source = rodio::Decoder::new(BufReader::new(file)).unwrap();
        self.sink.append(source);
        self.sink.play();
    }

    pub fn pause(&self) {
        self.sink.pause();
    }
}