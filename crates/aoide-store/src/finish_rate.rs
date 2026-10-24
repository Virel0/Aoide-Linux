//! How often a listen actually reaches the end — shared arithmetic with the phone and
//! the desktop, so one library shows one figure.

use std::collections::HashMap;
use std::ops::Add;

use rusqlite::params_from_iter;

use crate::error::Result;
use crate::history::MAX_PARAMETERS;
use crate::record::placeholders;
use crate::store::Store;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FinishCounts {
    /// Listens that reached a verdict — played through, or skipped.
    pub starts: i64,
    /// Of those, the ones that reached the end.
    pub completed: i64,
}

impl FinishCounts {
    pub fn new(starts: i64, completed: i64) -> Self {
        Self { starts, completed }
    }
}

impl Add for FinishCounts {
    type Output = FinishCounts;

    fn add(self, other: FinishCounts) -> FinishCounts {
        FinishCounts::new(self.starts + other.starts, self.completed + other.completed)
    }
}

pub struct FinishRate;

impl FinishRate {
    /// Below this many decided listens the figure is noise.
    pub const MINIMUM_SAMPLE: i64 = 3;

    pub fn rate(counts: FinishCounts) -> Option<f64> {
        (counts.starts >= Self::MINIMUM_SAMPLE)
            .then(|| counts.completed as f64 / counts.starts as f64)
    }

    /// Sums and then divides, rather than averaging each track's rate: an album is one
    /// thing rather than twelve equal ones.
    pub fn rate_over(counts: impl IntoIterator<Item = FinishCounts>) -> Option<f64> {
        Self::rate(counts.into_iter().fold(FinishCounts::default(), Add::add))
    }

    /// "78%", or `None` when there is not enough history.
    pub fn percent_description(counts: FinishCounts) -> Option<String> {
        Self::rate(counts).map(|rate| format!("{}%", (rate * 100.0).round() as i64))
    }
}

impl Store {
    /// Finish counts per track. Every requested id comes back, at zero if it has no
    /// history, so a caller can sum without checking for gaps.
    pub fn finish_counts(&self, jellyfin_ids: &[String]) -> Result<HashMap<String, FinishCounts>> {
        let mut found: HashMap<String, FinishCounts> = jellyfin_ids
            .iter()
            .map(|id| (id.clone(), FinishCounts::default()))
            .collect();
        if jellyfin_ids.is_empty() {
            return Ok(found);
        }
        let measured: Vec<(String, FinishCounts)> = self.read(|conn| {
            let mut rows = Vec::new();
            for chunk in jellyfin_ids.chunks(MAX_PARAMETERS) {
                let mut statement = conn.prepare(&format!(
                    "SELECT jellyfinId, COUNT(*) AS starts, \
                            SUM(CASE WHEN completed = 1 THEN 1 ELSE 0 END) AS completed \
                     FROM playEvent \
                     WHERE endedAt IS NOT NULL AND (completed = 1 OR skipped = 1) \
                       AND jellyfinId IN ({}) \
                     GROUP BY jellyfinId",
                    placeholders(chunk.len())
                ))?;
                let mapped = statement.query_map(params_from_iter(chunk), |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        FinishCounts::new(row.get(1)?, row.get(2)?),
                    ))
                })?;
                for row in mapped {
                    rows.push(row?);
                }
            }
            Ok(rows)
        })?;
        found.extend(measured);
        Ok(found)
    }

    /// The finish rate over a set of tracks, in one pass.
    pub fn finish_rate(&self, jellyfin_ids: &[String]) -> Result<Option<f64>> {
        Ok(FinishRate::rate_over(
            self.finish_counts(jellyfin_ids)?.into_values(),
        ))
    }
}
