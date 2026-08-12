import { describe, expect, it } from "vitest";

import { QueueProjection, type QueueDeltaBatch } from "./queueProjection";

function batch(sequence: number, batchSize = 250): QueueDeltaBatch {
  const start = sequence * batchSize;
  return {
    schema_version: 1,
    batch_sequence: sequence,
    total_batches: 40,
    total_jobs: 10_000,
    jobs: Array.from({ length: batchSize }, (_, offset) => ({
      id: `bench-${String(start + offset).padStart(5, "0")}`,
      state: (start + offset) % 5 === 0 ? "completed" : "queued",
      progress_basis_points: (start + offset) % 5 === 0 ? 10_000 : 0,
    })),
  };
}

describe("QueueProjection", () => {
  it("coalesces 10,000 jobs into one render frame", () => {
    const projection = new QueueProjection();
    const frames: Array<() => void> = [];
    const snapshots: ReturnType<QueueProjection["snapshot"]>[] = [];

    for (let sequence = 0; sequence < 40; sequence += 1) {
      projection.apply(
        batch(sequence),
        (callback) => frames.push(callback),
        (snapshot) => snapshots.push(snapshot),
      );
    }

    expect(projection.size).toBe(10_000);
    expect(frames).toHaveLength(1);
    expect(snapshots).toHaveLength(0);
    frames[0]();
    expect(snapshots).toHaveLength(1);
    expect(snapshots[0]).toMatchObject({
      totalJobs: 10_000,
      completed: 2_000,
      lastBatchSequence: 39,
    });
    expect(snapshots[0].visibleJobs).toHaveLength(100);
  });

  it("ignores duplicate and out-of-order batches", () => {
    const projection = new QueueProjection();
    const frames: Array<() => void> = [];
    projection.apply(batch(2), (callback) => frames.push(callback), () => undefined);
    projection.apply(batch(1), (callback) => frames.push(callback), () => undefined);
    projection.apply(batch(2), (callback) => frames.push(callback), () => undefined);
    expect(projection.size).toBe(250);
    expect(frames).toHaveLength(1);
  });
});
