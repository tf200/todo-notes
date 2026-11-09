//! Updates, through Velopack and the app's GitHub releases.
//!
//! An installed app checks for a newer release when it starts and every six hours after. It
//! downloads one in the background, then the sidebar offers to restart into it. A download the
//! user never applies is applied by Velopack the next time the app starts. Runs from `cargo run`
//! (not installed by Velopack) never check.
//!
//! `TODO_UPDATE_SOURCE` points the check elsewhere: a folder of `vpk pack` output, or another
//! URL. That's for testing an update before publishing it.

use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use rune::Waker;
use velopack::sources::AutoSource;
use velopack::{UpdateCheck, UpdateManager, VelopackAsset};

/// Where releases are published.
pub const RELEASES: &str = "https://github.com/tf200/todo-notes";
const CHECK_EVERY: Duration = Duration::from_secs(6 * 60 * 60);
/// After a failed check (offline, rate-limited): try again sooner than the usual interval.
const RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, PartialEq)]
pub enum Status {
    /// Not an installed copy: nothing to update.
    Off,
    /// Up to date, as far as the last check knows.
    Current,
    /// A newer version is downloading.
    Downloading(String),
    /// A newer version is downloaded and applies on restart.
    Ready(String),
}

pub struct Updates {
    status: Status,
    ready: Option<VelopackAsset>,
    found: Option<Receiver<Found>>,
}

enum Found {
    Downloading(String),
    Ready(VelopackAsset),
}

fn manager() -> Option<UpdateManager> {
    let source = std::env::var("TODO_UPDATE_SOURCE").unwrap_or_else(|_| RELEASES.to_string());
    UpdateManager::new(AutoSource::new(&source), None, None).ok()
}

impl Updates {
    /// Doesn't check: headless runs and tests.
    pub fn off() -> Updates {
        Updates { status: Status::Off, ready: None, found: None }
    }

    /// Shows `status` without checking: screenshot scenes.
    pub fn showing(status: Status) -> Updates {
        Updates { status, ready: None, found: None }
    }

    /// Starts checking in the background; `waker` asks for a frame when something is found.
    pub fn start(waker: Waker) -> Updates {
        if manager().is_none() {
            return Updates::off();
        }
        let (send, found) = channel();
        let spawned = std::thread::Builder::new().name("updates".into()).spawn(move || {
            loop {
                let wait = match check(&send, &waker) {
                    Some(true) => return, // downloaded: nothing more to do until a restart
                    Some(false) => CHECK_EVERY,
                    None => RETRY_AFTER,
                };
                std::thread::sleep(wait);
            }
        });
        match spawned {
            Ok(_) => Updates { status: Status::Current, ready: None, found: Some(found) },
            Err(_) => Updates::off(),
        }
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Takes in what the background check found. True when the status changed.
    pub fn poll(&mut self) -> bool {
        let Some(found) = &self.found else { return false };
        let mut changed = false;
        while let Ok(f) = found.try_recv() {
            match f {
                Found::Downloading(v) => self.status = Status::Downloading(v),
                Found::Ready(asset) => {
                    self.status = Status::Ready(asset.Version.clone());
                    self.ready = Some(asset);
                }
            }
            changed = true;
        }
        changed
    }

    /// Hands the downloaded version to Velopack, which installs it once this process has
    /// exited and starts the app again. The caller then closes the window. False when there's
    /// nothing to apply or Velopack couldn't be started (the app then just keeps running).
    pub fn restart_into_update(&self) -> bool {
        let (Some(asset), Some(m)) = (&self.ready, manager()) else { return false };
        m.wait_exit_then_apply_updates(asset, false, true, Vec::<String>::new()).is_ok()
    }
}

/// One check. `Some(true)`: an update is downloaded; `Some(false)`: nothing new; `None`: failed.
fn check(send: &std::sync::mpsc::Sender<Found>, waker: &Waker) -> Option<bool> {
    let m = manager()?;
    let info = match m.check_for_updates().ok()? {
        UpdateCheck::UpdateAvailable(info) => info,
        UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty => return Some(false),
    };
    let target = info.TargetFullRelease.clone();
    let _ = send.send(Found::Downloading(target.Version.clone()));
    waker();
    m.download_updates(&info, None).ok()?;
    let _ = send.send(Found::Ready(target));
    waker();
    Some(true)
}
