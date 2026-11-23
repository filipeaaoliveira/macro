//! The in-memory stand-in for break records.

use chrono::Utc;
use factory_records::domain::breaks::{BreakRecord, BreakStatus, NewBreakRecord, RunsBreakRecords};
use factory_records::domain::models::{RunId, StepKey};

use super::FakeRepo;

impl FakeRepo {
    /// Keep `record` for the Step `key`, unless it keeps one for its test:
    /// the first stays, unless it was interrupted, as in Postgres.
    pub(crate) fn keep_break(&self, key: StepKey, record: &NewBreakRecord) {
        let mut breaks = self.breaks.lock().expect("breaks");
        if let Some(at) = breaks
            .iter()
            .position(|(step, kept)| *step == key && kept.test == record.test)
        {
            if breaks[at].1.status != BreakStatus::Interrupted {
                return;
            }
            breaks.remove(at);
        }
        breaks.push((
            key,
            BreakRecord {
                seq: key.seq,
                test: record.test.clone(),
                file: record.file.clone(),
                old_text: record.old_text.clone(),
                new_text: record.new_text.clone(),
                command: record.command.clone(),
                status: record.status,
                checked_commit: record.checked_commit.clone(),
                output: record.output.clone(),
                recorded_at: Utc::now(),
            },
        ));
    }

    /// The break records of the Runs `ids`, by Run, each Run's by Step.
    pub(crate) fn breaks_of(&self, ids: &[RunId]) -> RunsBreakRecords {
        let breaks = self.breaks.lock().expect("breaks");
        let mut records = RunsBreakRecords::new();
        for (key, record) in breaks.iter().filter(|(key, _)| ids.contains(&key.run_id)) {
            records.entry(key.run_id).or_default().push(record.clone());
        }
        for kept in records.values_mut() {
            kept.sort_by_key(|record| record.seq);
        }
        records
    }

    /// The break records the Step `key` kept, in order.
    pub(crate) fn step_breaks(&self, key: StepKey) -> Vec<BreakRecord> {
        self.breaks
            .lock()
            .expect("breaks")
            .iter()
            .filter(|(step, _)| *step == key)
            .map(|(_, record)| record.clone())
            .collect()
    }
}
