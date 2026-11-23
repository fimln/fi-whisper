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

    fn next(self) -> Self {
        match self {
            Self::White => Self::Pink,
            Self::Pink => Self::Brown,
            Self::Brown => Self::White,
        }
    }

    fn previous(self) -> Self {
        match self {
            Self::White => Self::Brown,
            Self::Brown => Self::Pink,
            Self::Pink => Self::White,
        }
    }

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
    pub audio_error: Option<OutputError>,
    pub media_error: Option<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OutputError {
    NoDevice,
    Configuration,
    UnsupportedFormat(cpal::SampleFormat),
    Open,
    Play,
    Unavailable,
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
                playback_loop(
                    controls,
                    rx,
                    |mode, id| {
                        let sender = sender.clone();
                        noise::open(mode, move || {
                            let _ = sender.send(Command::OutputFailed(id));
                        })
                    },
                    |status| {
                        *shared.lock().unwrap() = status.clone();
                        repaint();
                    },
                );
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

fn playback_loop<S>(
    controls: windows::core::Result<media::Media>,
    rx: mpsc::Receiver<Command>,
    mut open: impl FnMut(Arc<AtomicU8>, u64) -> Result<S, OutputError>,
    mut publish: impl FnMut(&Status),
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
        publish(&status);
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
                status.audio_error = Some(OutputError::Unavailable);
            }
            Command::Pause => status.playback = Playback::Paused,
            Command::Stop => status.playback = Playback::Stopped,
            action => {
                status.noise = match action {
                    Command::Select(noise) => noise,
                    Command::Next => status.noise.next(),
                    Command::Previous => status.noise.previous(),
                    _ => status.noise,
                };
                status.audio_error = None;
                status.playback = Playback::Playing;
                if stream.is_none() {
                    generation += 1;
                    match open(mode.clone(), generation) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_and_previous_wrap_in_display_order() {
        for (i, noise) in Noise::ALL.into_iter().enumerate() {
            let next = Noise::ALL[(i + 1) % Noise::ALL.len()];
            assert_eq!(noise.next(), next);
            assert_eq!(next.previous(), noise);
        }
    }

    #[test]
    fn worker_releases_paused_output_and_ignores_its_late_errors() {
        let (commands, rx) = mpsc::channel();
        let (updates, snapshots) = mpsc::channel();
        let (opened, generations) = mpsc::channel();
        let worker = thread::spawn(move || {
            let output = Arc::new(());
            playback_loop(
                Err(windows::core::Error::empty()),
                rx,
                |_, id| {
                    opened.send(id).unwrap();
                    Ok(output.clone())
                },
                |status| {
                    let active = Arc::strong_count(&output) > 1;
                    updates.send((status.clone(), active)).unwrap();
                },
            );
        });
        let timeout = Duration::from_secs(3);
        let snapshot = || snapshots.recv_timeout(timeout).unwrap();
        let send = |cmd| commands.send(cmd).unwrap();
        assert_eq!(snapshot().0.playback, Playback::Stopped);

        send(Command::Next);
        let (playing, active) = snapshot();
        assert!(active);
        assert_eq!(
            (playing.noise, playing.playback),
            (Noise::Pink, Playback::Playing)
        );
        let first = generations.recv_timeout(timeout).unwrap();
        send(Command::Pause);
        let (paused, fading) = snapshot();
        assert_eq!(paused.playback, Playback::Paused);
        assert!(fading);
        let (paused, active) = snapshot();
        assert_eq!(paused.playback, Playback::Paused);
        assert!(!active);

        send(Command::Previous);
        let (playing, active) = snapshot();
        assert!(active);
        assert_eq!(
            (playing.noise, playing.playback),
            (Noise::White, Playback::Playing)
        );
        let second = generations.recv_timeout(timeout).unwrap();
        assert_ne!(first, second);
        send(Command::OutputFailed(first));
        let (playing, active) = snapshot();
        assert!(active && playing.audio_error.is_none());
        assert_eq!(playing.playback, Playback::Playing);
        send(Command::OutputFailed(second));
        let (failed, active) = snapshot();
        assert!(!active);
        assert_eq!(failed.playback, Playback::Stopped);
        assert_eq!(failed.audio_error, Some(OutputError::Unavailable));
        send(Command::Quit);
        worker.join().unwrap();
    }
}
