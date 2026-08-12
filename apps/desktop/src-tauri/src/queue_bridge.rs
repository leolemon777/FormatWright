use serde::Serialize;

pub const DEFAULT_BENCHMARK_JOBS: u32 = 10_000;
pub const DEFAULT_BATCH_JOBS: u32 = 250;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum JobViewState {
    Queued,
    Running,
    Validating,
    Completed,
    Warning,
    Failed,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct JobView {
    pub id: String,
    pub state: JobViewState,
    pub progress_basis_points: u16,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct QueueDeltaBatch {
    pub schema_version: u32,
    pub batch_sequence: u32,
    pub total_batches: u32,
    pub total_jobs: u32,
    pub jobs: Vec<JobView>,
}

#[derive(Clone, Debug)]
pub struct QueueBatchIter {
    total_jobs: u32,
    batch_jobs: u32,
    next_index: u32,
    batch_sequence: u32,
    total_batches: u32,
}

impl QueueBatchIter {
    pub fn new(total_jobs: u32, batch_jobs: u32) -> Result<Self, &'static str> {
        if total_jobs == 0 || total_jobs > DEFAULT_BENCHMARK_JOBS {
            return Err("job count must be between 1 and 10,000");
        }
        if batch_jobs == 0 || batch_jobs > 1_000 {
            return Err("batch size must be between 1 and 1,000");
        }
        let total_batches = total_jobs.div_ceil(batch_jobs);
        Ok(Self {
            total_jobs,
            batch_jobs,
            next_index: 0,
            batch_sequence: 0,
            total_batches,
        })
    }
}

impl Iterator for QueueBatchIter {
    type Item = QueueDeltaBatch;

    fn next(&mut self) -> Option<Self::Item> {
        if self.next_index >= self.total_jobs {
            return None;
        }
        let remaining = self.total_jobs - self.next_index;
        let count = remaining.min(self.batch_jobs);
        let start = self.next_index;
        let jobs = (start..start + count)
            .map(|index| {
                let state = match index % 10 {
                    0 => JobViewState::Completed,
                    1 => JobViewState::Running,
                    2 => JobViewState::Validating,
                    3 => JobViewState::Warning,
                    4 => JobViewState::Failed,
                    _ => JobViewState::Queued,
                };
                let progress_basis_points = match state {
                    JobViewState::Completed | JobViewState::Warning => 10_000,
                    JobViewState::Validating => 9_000,
                    JobViewState::Running => 5_000,
                    JobViewState::Failed => 3_000,
                    JobViewState::Queued => 0,
                };
                JobView {
                    id: format!("bench-{index:05}"),
                    progress_basis_points,
                    state,
                }
            })
            .collect();
        let batch = QueueDeltaBatch {
            schema_version: 1,
            batch_sequence: self.batch_sequence,
            total_batches: self.total_batches,
            total_jobs: self.total_jobs,
            jobs,
        };
        self.next_index += count;
        self.batch_sequence += 1;
        Some(batch)
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let remaining = self.total_batches.saturating_sub(self.batch_sequence);
        let remaining = usize::try_from(remaining).unwrap_or(usize::MAX);
        (remaining, Some(remaining))
    }
}

impl ExactSizeIterator for QueueBatchIter {}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::{DEFAULT_BATCH_JOBS, DEFAULT_BENCHMARK_JOBS, QueueBatchIter};

    #[test]
    fn ten_thousand_jobs_become_forty_bounded_batches() {
        let batches = QueueBatchIter::new(DEFAULT_BENCHMARK_JOBS, DEFAULT_BATCH_JOBS)
            .expect("valid benchmark")
            .collect::<Vec<_>>();
        assert_eq!(batches.len(), 40);
        assert!(batches.iter().all(|batch| batch.jobs.len() <= 250));
        assert_eq!(batches[0].batch_sequence, 0);
        assert_eq!(batches[39].batch_sequence, 39);
        let ids = batches
            .iter()
            .flat_map(|batch| batch.jobs.iter().map(|job| job.id.as_str()))
            .collect::<BTreeSet<_>>();
        assert_eq!(ids.len(), 10_000);
    }

    #[test]
    fn rejects_unbounded_requests() {
        assert!(QueueBatchIter::new(0, 250).is_err());
        assert!(QueueBatchIter::new(10_001, 250).is_err());
        assert!(QueueBatchIter::new(10_000, 0).is_err());
    }
}
