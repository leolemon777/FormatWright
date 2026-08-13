import { describe, expect, it } from "vitest";

import {
  JOB_PAGE_SIZE,
  elapsedProgressSeconds,
  isDirectoryOutput,
  jobListAriaAttributes,
  latestJobProgress,
  parseDesktopError,
  progressForJob,
  recommendedTargets,
  suggestedOutput,
} from "./desktopModel";

describe("desktop workflow model", () => {
  it("recommends content-family targets from a dropped path", () => {
    expect(recommendedTargets("C:\\photos\\image.heic")).toEqual(["jpg", "png"]);
    expect(recommendedTargets("report.pdf")).toEqual(["png", "jpg"]);
    expect(recommendedTargets("report.docx")).toEqual(["pdf"]);
    expect(recommendedTargets("notes.md")).toEqual(["pdf", "docx"]);
  });

  it("suggests a directory for multi-page PDF rendering", () => {
    expect(isDirectoryOutput("C:\\in\\report.pdf", "png")).toBe(true);
    expect(suggestedOutput("C:\\in\\report.pdf", "png")).toBe(
      "C:\\in\\report.converted-png-pages",
    );
  });

  it("builds a non-overwriting suggested output", () => {
    expect(suggestedOutput("C:\\in\\photo.png", "webp")).toBe(
      "C:\\in\\photo.converted.webp",
    );
  });

  it("preserves typed backend error details", () => {
    expect(
      parseDesktopError('{"code":"OUTPUT_CONFLICT","stage":"commit","message":"Exists"}'),
    ).toMatchObject({ code: "OUTPUT_CONFLICT", stage: "commit", message: "Exists" });
  });

  it("keeps large job histories bounded while exposing global list positions", () => {
    expect(JOB_PAGE_SIZE).toBe(100);
    expect(jobListAriaAttributes(9_900, 99, 10_000)).toEqual({
      "aria-posinset": 10_000,
      "aria-setsize": 10_000,
    });
  });

  it("keeps truthful progress monotonic and derives elapsed time without inventing ETA", () => {
    const running = {
      schema_version: 1,
      job_id: "job-1",
      job_sequence: 4,
      state: "running",
      wait_reason: null,
      occurred_unix_ms: 4_000,
      eta_milliseconds: null,
    };
    const stale = { ...running, job_sequence: 3, state: "inspecting", occurred_unix_ms: 5_000 };
    expect(latestJobProgress(running, stale)).toBe(running);
    expect(progressForJob(running, 5)).toBeUndefined();
    expect(progressForJob(running, 4)).toBe(running);
    expect(elapsedProgressSeconds(running, 6_750)).toBe(2);
    expect(running.eta_milliseconds).toBeNull();
  });
});
