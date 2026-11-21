mod media;
mod noise;

use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread,
    time::Duration,
};
use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize, RoUninitialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(u8)]
pub enum Noise {
    #[default]
    White = 1,
    Pink,
    Brown,
}

impl Noise {
    pub const ALL: [Self; 3] = [Self::White, Self::Pink, Self::Brown];
    pub fn label(self) -> &'static str {
        match self {
            Self::White => "White noise",
            Self::Pink => "Pink noise",
            Self::Brown => "Brown noise",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Playback {
    #[default]
    Stopped,
    Playing,
    Paused,
}

#[derive(Clone, Debug, Default)]
pub struct Status {
    pub noise: Noise,
    pub playback: Playback,
    pub audio_error: Option<String>,
    pub media_error: Option<String>,
}

pub enum Command {
    Select(Noise),
    Play,
    Pause,
    Stop,
    Next,
    Previous,
    OutputFailed(u64),
    Quit,
}

pub struct Focus {
    tx: Sender<Command>,
    state: Arc<Mutex<Status>>,
    worker: Option<thread::JoinHandle<()>>,
}

impl Focus {
    pub fn new(hwnd: isize, repaint: impl Fn() + Send + 'static) -> std::io::Result<Self> {
        let (tx, rx) = mpsc::channel();
        let state = Arc::new(Mutex::new(Status::default()));
        let shared = state.clone();
        let sender = tx.clone();
        let worker = thread::Builder::new()
            .name("fi-focus".into())
            .spawn(move || {
                let apartment = unsafe { RoInitialize(RO_INIT_MULTITHREADED) };
                let initialized = apartment.is_ok();
                let controls = apartment.and_then(|()| media::Media::new(hwnd, sender.clone()));
                playback_loop(controls, rx, &sender, &shared, repaint);
                if initialized {
                    unsafe { RoUninitialize() };
                }
            })?;
        Ok(Self {
            tx,
            state,
            worker: Some(worker),
        })
    }

    pub fn status(&self) -> Status {
        self.state.lock().unwrap().clone()
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }
}

fn playback_loop(
    controls: windows::core::Result<media::Media>,
    rx: mpsc::Receiver<Command>,
    sender: &Sender<Command>,
    shared: &Mutex<Status>,
    repaint: impl Fn(),
) {
    let mut status = Status::default();
    if let Err(error) = &controls {
        status.media_error = Some(error.to_string());
    }
    let media = controls.ok();
    let mode = Arc::new(AtomicU8::new(0));
    let mut stream = None;
    let mut generation = 0;
    loop {
        *shared.lock().unwrap() = status.clone();
        repaint();
        // Lanjutkan fade-out di callback, lalu lepaskan perangkat saat jeda.
        let cmd = if stream.is_some() && status.playback != Playback::Playing {
            match rx.recv_timeout(Duration::from_millis(60)) {
                Ok(cmd) => cmd,
                Err(RecvTimeoutError::Timeout) => {
                    stream.take();
                    continue;
                }
                Err(RecvTimeoutError::Disconnected) => break,
            }
        } else {
            let Ok(cmd) = rx.recv() else { break };
            cmd
        };
        match cmd {
            Command::Quit => break,
            Command::OutputFailed(id) if id != generation || stream.is_none() => {
                continue;
            }
            Command::OutputFailed(_) => {
                stream.take();
                status.playback = Playback::Stopped;
                status.audio_error = Some("Output device unavailable".into());
            }
            Command::Pause => status.playback = Playback::Paused,
            Command::Stop => status.playback = Playback::Stopped,
            action => {
                status.noise = match action {
                    Command::Select(noise) => noise,
                    Command::Next => Noise::ALL[status.noise as usize % 3],
                    Command::Previous => Noise::ALL[(status.noise as usize + 1) % 3],
                    _ => status.noise,
                };
                status.audio_error = None;
                status.playback = Playback::Playing;
                if stream.is_none() {
                    generation += 1;
                    let id = generation;
                    let sender = sender.clone();
                    match noise::open(mode.clone(), move || {
                        let _ = sender.send(Command::OutputFailed(id));
                    }) {
                        Ok(output) => stream = Some(output),
                        Err(error) => {
                            status.audio_error = Some(error);
                            status.playback = Playback::Stopped;
                        }
                    }
                }
            }
        }
        mode.store(
            if status.playback == Playback::Playing {
                status.noise as u8
            } else {
                0
            },
            Ordering::Relaxed,
        );
        if let Some(media) = &media {
            status.media_error = media
                .update(status.noise, status.playback)
                .err()
                .map(|e| e.to_string());
        }
    }
}

impl Drop for Focus {
    fn drop(&mut self) {
        self.send(Command::Quit);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
