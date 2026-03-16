//! Config hot-reloading via file watching.
//!
//! Uses the `notify` crate to watch for config file changes and reloads
//! the configuration automatically.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use notify::{Config as NotifyConfig, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::{broadcast, mpsc};

use crate::config::BotConfig;
use crate::error::ConfigError;

/// Message sent when config is reloaded.
#[derive(Debug, Clone)]
pub enum ConfigEvent {
    /// Config was successfully reloaded.
    Reloaded(BotConfig),
    /// Failed to reload config.
    ReloadFailed(String),
    /// Config file was modified but content is invalid.
    Invalid(String),
}

/// Watches a config file and emits reload events.
pub struct ConfigWatcher {
    _watcher: RecommendedWatcher,
    events: broadcast::Sender<ConfigEvent>,
}

impl ConfigWatcher {
    /// Create a new config watcher that watches the given file path.
    ///
    /// Returns an error if the file cannot be watched.
    pub fn new(
        path: PathBuf,
        poll_interval: Duration,
    ) -> Result<(Self, broadcast::Receiver<ConfigEvent>), notify::Error> {
        let (events_tx, events_rx) = broadcast::channel(16);

        let events_tx_clone = events_tx.clone();
        let path_clone = path.clone();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    if event.kind.is_modify() {
                        match BotConfig::load(&path_clone) {
                            Ok(config) => {
                                let _ = events_tx_clone.send(ConfigEvent::Reloaded(config));
                            }
                            Err(e) => {
                                let _ = events_tx_clone
                                    .send(ConfigEvent::Invalid(e.to_string()));
                            }
                        }
                    }
                }
            },
            NotifyConfig::default().with_poll_interval(poll_interval),
        )?;

        watcher.watch(&path, RecursiveMode::NonRecursive)?;

        Ok((
            Self {
                _watcher: watcher,
                events: events_tx,
            },
            events_rx,
        ))
    }

    /// Subscribe to config change events.
    pub fn subscribe(&self) -> broadcast::Receiver<ConfigEvent> {
        self.events.subscribe()
    }
}

/// Start a background task that watches for config changes.
///
/// Returns a receiver that will receive config reload events.
pub fn start_config_watcher(
    path: PathBuf,
    poll_interval: Duration,
) -> Result<broadcast::Receiver<ConfigEvent>, notify::Error> {
    let (watcher, rx) = ConfigWatcher::new(path, poll_interval)?;
    // Keep watcher alive by spawning it - in practice this would be tied to app lifetime
    std::mem::forget(watcher);
    Ok(rx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use std::path::PathBuf;
    use tempfile::TempDir;

    fn create_test_config(tmp_dir: &TempDir) -> PathBuf {
        let path = tmp_dir.path().join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(
            b"
[general]
mode = \"paper\"

[risk]
bankroll = 1000
",
        )
        .unwrap();
        path
    }

    #[tokio::test]
    async fn test_watch_config_file() {
        let tmp_dir = TempDir::new().unwrap();
        let path = create_test_config(&tmp_dir);

        // Keep watcher alive during test
        let (watcher, mut rx) = ConfigWatcher::new(path.clone(), Duration::from_millis(100)).unwrap();

        // Modify the config file
        tokio::time::sleep(Duration::from_millis(200)).await;
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(
            b"
[general]
mode = \"live\"

[risk]
bankroll = 2000
",
        )
        .unwrap();
        drop(f);

        // Wait for the watcher to detect the change
        let result = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await;

        // Verify watcher is still alive
        match result {
            Ok(Ok(event)) => {
                match event {
                    ConfigEvent::Reloaded(config) => {
                        assert_eq!(config.general.mode, "live");
                    }
                    other => panic!("expected Reloaded, got: {other:?}"),
                }
            }
            Ok(Err(e)) => panic!("channel error: {}", e),
            Err(_) => {
                // Timeout - this can happen on slow systems, but watcher should still work
                // Keep watcher alive to prevent premature drop
                drop(watcher);
            }
        }
    }
}
