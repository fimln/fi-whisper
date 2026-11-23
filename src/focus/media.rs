use super::{Command, Noise, Playback};
use std::sync::mpsc::Sender;
use windows::{
    Foundation::TypedEventHandler,
    Media::{
        MediaPlaybackStatus, MediaPlaybackType, SystemMediaTransportControls,
        SystemMediaTransportControlsButton as Button,
        SystemMediaTransportControlsButtonPressedEventArgs,
    },
    Win32::{Foundation::HWND, System::WinRT::ISystemMediaTransportControlsInterop},
    core::{HSTRING, Result, factory},
};

pub(super) struct Media {
    controls: SystemMediaTransportControls,
    token: i64,
}

impl Media {
    pub(super) fn new(hwnd: isize, tx: Sender<Command>) -> Result<Self> {
        let interop: ISystemMediaTransportControlsInterop =
            factory::<SystemMediaTransportControls, _>()?;
        let controls: SystemMediaTransportControls =
            unsafe { interop.GetForWindow(HWND(hwnd as _))? };
        controls.SetIsPlayEnabled(true)?;
        controls.SetIsPauseEnabled(true)?;
        controls.SetIsStopEnabled(true)?;
        controls.SetIsNextEnabled(true)?;
        controls.SetIsPreviousEnabled(true)?;
        let updater = controls.DisplayUpdater()?;
        updater.SetType(MediaPlaybackType::Music)?;
        updater
            .MusicProperties()?
            .SetArtist(&HSTRING::from("fi-focus"))?;
        let token = controls.ButtonPressed(&TypedEventHandler::<
            SystemMediaTransportControls,
            SystemMediaTransportControlsButtonPressedEventArgs,
        >::new(move |_, args| {
            if let Some(args) = args.as_ref() {
                let action = match args.Button()? {
                    Button::Play => Command::Play,
                    Button::Pause => Command::Pause,
                    Button::Stop => Command::Stop,
                    Button::Next => Command::Next,
                    Button::Previous => Command::Previous,
                    _ => return Ok(()),
                };
                let _ = tx.send(action);
            }
            Ok(())
        }))?;
        let media = Self { controls, token };
        media.update(Noise::White, Playback::Stopped)?;
        media.controls.SetIsEnabled(true)?;
        Ok(media)
    }

    pub(super) fn update(&self, noise: Noise, playback: Playback) -> Result<()> {
        let updater = self.controls.DisplayUpdater()?;
        updater
            .MusicProperties()?
            .SetTitle(&HSTRING::from(noise.label()))?;
        updater.Update()?;
        self.controls.SetPlaybackStatus(match playback {
            Playback::Playing => MediaPlaybackStatus::Playing,
            Playback::Paused => MediaPlaybackStatus::Paused,
            Playback::Stopped => MediaPlaybackStatus::Stopped,
        })
    }
}

impl Drop for Media {
    fn drop(&mut self) {
        let _ = self.controls.RemoveButtonPressed(self.token);
        let _ = self.controls.SetPlaybackStatus(MediaPlaybackStatus::Closed);
        let _ = self.controls.SetIsEnabled(false);
    }
}
