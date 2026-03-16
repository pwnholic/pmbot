//! Event recording and replay for backtesting.
//!
//! [`EventRecorder`] writes market events to JSONL files.
//! [`EventReader`] reads them back as an iterator.

use std::fs::File;
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::Path;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};

use pmbot_core::types::Level;

/// A recorded market event for replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum RecordedEvent {
    /// Orderbook snapshot update.
    Book {
        /// Timestamp of the event.
        ts: DateTime<Utc>,
        /// Market identifier.
        market: String,
        /// Token identifier.
        token: String,
        /// Bid levels.
        bids: Vec<Level>,
        /// Ask levels.
        asks: Vec<Level>,
    },
    /// External spot price update.
    Spot {
        /// Timestamp of the event.
        ts: DateTime<Utc>,
        /// Symbol name.
        symbol: String,
        /// Spot price.
        price: Decimal,
    },
}

impl RecordedEvent {
    /// Returns the timestamp of this event.
    pub fn timestamp(&self) -> DateTime<Utc> {
        match self {
            RecordedEvent::Book { ts, .. } | RecordedEvent::Spot { ts, .. } => *ts,
        }
    }
}

/// Writes recorded events to a JSONL file (one JSON object per line).
pub struct EventRecorder {
    writer: BufWriter<File>,
    events_written: u64,
}

impl EventRecorder {
    /// Create a new recorder writing to the given path.
    pub fn new(path: &Path) -> anyhow::Result<Self> {
        let file = File::create(path)?;
        Ok(Self {
            writer: BufWriter::new(file),
            events_written: 0,
        })
    }

    /// Record a single event as a JSONL line.
    pub fn record(&mut self, event: &RecordedEvent) -> anyhow::Result<()> {
        let line = serde_json::to_string(event)?;
        writeln!(self.writer, "{line}")?;
        self.events_written += 1;
        Ok(())
    }

    /// Flush the underlying writer.
    pub fn flush(&mut self) -> anyhow::Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Number of events written so far.
    pub fn events_written(&self) -> u64 {
        self.events_written
    }
}

/// Reads recorded events from a JSONL file as an iterator.
pub struct EventReader {
    reader: BufReader<File>,
    line_buf: String,
}

impl EventReader {
    /// Open a JSONL file for reading.
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let file = File::open(path)?;
        Ok(Self {
            reader: BufReader::new(file),
            line_buf: String::new(),
        })
    }
}

impl Iterator for EventReader {
    type Item = anyhow::Result<RecordedEvent>;

    fn next(&mut self) -> Option<Self::Item> {
        self.line_buf.clear();
        match self.reader.read_line(&mut self.line_buf) {
            Ok(0) => None, // EOF
            Ok(_) => {
                let trimmed = self.line_buf.trim();
                if trimmed.is_empty() {
                    return self.next(); // skip blank lines
                }
                Some(
                    serde_json::from_str(trimmed)
                        .map_err(|e| anyhow::anyhow!("Failed to parse event: {e}")),
                )
            }
            Err(e) => Some(Err(e.into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use std::fs;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("pmbot_test_{name}_{}", std::process::id()))
    }

    fn sample_book_event(ts: DateTime<Utc>) -> RecordedEvent {
        RecordedEvent::Book {
            ts,
            market: "market1".into(),
            token: "token1".into(),
            bids: vec![Level {
                price: dec!(0.50),
                size: dec!(100),
            }],
            asks: vec![Level {
                price: dec!(0.55),
                size: dec!(80),
            }],
        }
    }

    fn sample_spot_event(ts: DateTime<Utc>) -> RecordedEvent {
        RecordedEvent::Spot {
            ts,
            symbol: "BTCUSDT".into(),
            price: dec!(65000),
        }
    }

    #[test]
    fn test_write_events_to_file() {
        let path = temp_path("write_events.jsonl");
        let ts = Utc::now();

        let mut recorder = EventRecorder::new(&path).unwrap();
        recorder.record(&sample_book_event(ts)).unwrap();
        recorder.record(&sample_spot_event(ts)).unwrap();
        recorder.flush().unwrap();

        assert_eq!(recorder.events_written(), 2);

        let content = fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = content.trim().lines().collect();
        assert_eq!(lines.len(), 2);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_read_events_from_file() {
        let path = temp_path("read_events.jsonl");
        let ts = Utc::now();

        let mut recorder = EventRecorder::new(&path).unwrap();
        recorder.record(&sample_book_event(ts)).unwrap();
        recorder.record(&sample_spot_event(ts)).unwrap();
        recorder.flush().unwrap();

        let reader = EventReader::open(&path).unwrap();
        let events: Vec<RecordedEvent> = reader.map(|r| r.unwrap()).collect();
        assert_eq!(events.len(), 2);

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_roundtrip_book_event() {
        let path = temp_path("roundtrip_book.jsonl");
        let ts = Utc::now();
        let original = sample_book_event(ts);

        let mut recorder = EventRecorder::new(&path).unwrap();
        recorder.record(&original).unwrap();
        recorder.flush().unwrap();

        let mut reader = EventReader::open(&path).unwrap();
        let read_back = reader.next().unwrap().unwrap();

        match (&original, &read_back) {
            (
                RecordedEvent::Book {
                    market: m1,
                    token: t1,
                    bids: b1,
                    asks: a1,
                    ..
                },
                RecordedEvent::Book {
                    market: m2,
                    token: t2,
                    bids: b2,
                    asks: a2,
                    ..
                },
            ) => {
                assert_eq!(m1, m2);
                assert_eq!(t1, t2);
                assert_eq!(b1.len(), b2.len());
                assert_eq!(a1.len(), a2.len());
                assert_eq!(b1[0].price, b2[0].price);
                assert_eq!(a1[0].price, a2[0].price);
            }
            _ => panic!("Expected Book events"),
        }

        let _ = fs::remove_file(&path);
    }

    #[test]
    fn test_roundtrip_spot_event() {
        let path = temp_path("roundtrip_spot.jsonl");
        let ts = Utc::now();
        let original = sample_spot_event(ts);

        let mut recorder = EventRecorder::new(&path).unwrap();
        recorder.record(&original).unwrap();
        recorder.flush().unwrap();

        let mut reader = EventReader::open(&path).unwrap();
        let read_back = reader.next().unwrap().unwrap();

        match (&original, &read_back) {
            (
                RecordedEvent::Spot {
                    symbol: s1,
                    price: p1,
                    ..
                },
                RecordedEvent::Spot {
                    symbol: s2,
                    price: p2,
                    ..
                },
            ) => {
                assert_eq!(s1, s2);
                assert_eq!(p1, p2);
            }
            _ => panic!("Expected Spot events"),
        }

        let _ = fs::remove_file(&path);
    }
}
