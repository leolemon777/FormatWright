export type JobState =
  | "queued"
  | "running"
  | "validating"
  | "completed"
  | "warning"
  | "failed";

export type JobView = {
  id: string;
  state: JobState;
  progress_basis_points: number;
};

export type QueueDeltaBatch = {
  schema_version: 1;
  batch_sequence: number;
  total_batches: number;
  total_jobs: number;
  jobs: JobView[];
};

export type QueueSnapshot = {
  totalJobs: number;
  completed: number;
  active: number;
  failed: number;
  lastBatchSequence: number;
  visibleJobs: JobView[];
};

export type FrameScheduler = (callback: () => void) => void;

export class QueueProjection {
  readonly #jobs = new Map<string, JobView>();
  #lastBatchSequence = -1;
  #renderPending = false;

  get size(): number {
    return this.#jobs.size;
  }

  reset(): void {
    this.#jobs.clear();
    this.#lastBatchSequence = -1;
    this.#renderPending = false;
  }

  apply(
    batch: QueueDeltaBatch,
    schedule: FrameScheduler,
    publish: (snapshot: QueueSnapshot) => void,
  ): void {
    if (batch.schema_version !== 1 || batch.batch_sequence <= this.#lastBatchSequence) {
      return;
    }
    this.#lastBatchSequence = batch.batch_sequence;
    for (const job of batch.jobs) {
      this.#jobs.set(job.id, job);
    }
    if (this.#renderPending) {
      return;
    }
    this.#renderPending = true;
    schedule(() => {
      this.#renderPending = false;
      publish(this.snapshot());
    });
  }

  snapshot(): QueueSnapshot {
    let completed = 0;
    let active = 0;
    let failed = 0;
    for (const job of this.#jobs.values()) {
      if (job.state === "completed") {
        completed += 1;
      } else if (job.state === "failed") {
        failed += 1;
      } else if (job.state === "running" || job.state === "validating") {
        active += 1;
      }
    }
    return {
      totalJobs: this.#jobs.size,
      completed,
      active,
      failed,
      lastBatchSequence: this.#lastBatchSequence,
      visibleJobs: Array.from(this.#jobs.values()).slice(0, 100),
    };
  }
}
