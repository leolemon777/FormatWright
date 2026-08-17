import { describe, expect, it } from "vitest";

import {
  JOB_PAGE_SIZE,
  SUPPORTED_TARGET_FORMATS,
  certificationLabel,
  elapsedProgressSeconds,
  engineRecoveryNotices,
  engineRecoveryState,
  isDirectoryOutput,
  jobListAriaAttributes,
  latestJobProgress,
  normalizeShellTarget,
  packBadgeKind,
  parseDesktopError,
  qualityFieldApplies,
  presetFieldChangeInvalidatesPreview,
  progressForJob,
  recommendedTargets,
  suggestedOutput,
  targetOptionViews,
  type EngineRecoveryOutcome,
} from "./desktopModel";

describe("desktop workflow model", () => {
  it("recommends content-family targets from a dropped path", () => {
    expect(recommendedTargets("C:\\photos\\image.heic")).toEqual(["jpg", "png"]);
    expect(recommendedTargets("report.pdf")).toEqual(["png", "jpg"]);
    expect(recommendedTargets("report.docx")).toEqual(["pdf"]);
    expect(recommendedTargets("notes.md")).toEqual(["pdf", "docx"]);
    expect(recommendedTargets("C:\\\\桌面\\\\新建 XLS 工作表.xls")).toEqual([]);
    expect(recommendedTargets("unknown.bin")).toEqual([]);
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

describe("target option views", () => {
  it("keeps the submitted option value separate from the localized unavailable label", () => {
    const routes = { png: { available: false, missing_engines: ["ffmpeg"] }, jpg: { available: true } };
    const views = targetOptionViews(["png", "jpg"], routes, "convert-file", { missing: "Missing", unsupported: "Unsupported" });
    const png = views.find((option) => option.value === "png");
    expect(png).toEqual({ value: "png", label: "png — Missing", disabled: true });
    expect(views.find((option) => option.value === "jpg")).toEqual({ value: "jpg", label: "jpg", disabled: false });
  });

  it("hides unsupported pairs once capabilities are known", () => {
    const routes = {
      png: { available: true },
      jpg: { available: true },
      webp: { available: false, missing_engines: ["ffmpeg"] },
      mp4: { available: false, missing_engines: [] },
    };
    const views = targetOptionViews(["png", "jpg"], routes, "convert-file", { missing: "Missing", unsupported: "Unsupported" });
    expect(views.map((option) => option.value)).toEqual(["png", "jpg", "webp"]);
    expect(views.find((option) => option.value === "webp")).toEqual({
      value: "webp",
      label: "webp — Missing",
      disabled: true,
    });
  });

  it("treats a right-click convert target as an explicit allowed format", () => {
    expect(normalizeShellTarget("PNG")).toBe("png");
    expect(normalizeShellTarget(".jpeg")).toBe("jpg");
    expect(normalizeShellTarget("yml")).toBe("yaml");
    expect(normalizeShellTarget("exe")).toBeNull();
    expect(qualityFieldApplies("jpg")).toBe(true);
    expect(qualityFieldApplies("png")).toBe(false);
    expect(qualityFieldApplies("yaml")).toBe(false);
  });

  it("does not gate targets before capabilities load or in folder mode", () => {
    const labels = { missing: "Missing", unsupported: "Unsupported" };
    expect(targetOptionViews(["png"], null, "convert-file", labels).find((option) => option.value === "png")?.disabled).toBe(false);
    expect(targetOptionViews(["png"], { png: { available: false } }, "convert-folder", labels).find((option) => option.value === "png")?.disabled).toBe(false);
    expect(targetOptionViews(["png"], { png: { available: false } }, "convert-folder", labels).find((option) => option.value === "png")?.label).toBe("png");
  });

  it("gates preset targets without relabeling them", () => {
    const view = targetOptionViews(["png"], { png: { available: false } }, "preset", { missing: "Missing", unsupported: "Unsupported" }).find((option) => option.value === "png");
    expect(view).toEqual({ value: "png", label: "png", disabled: true });
  });

  it("merges recommendations with the supported target list deterministically", () => {
    const labels = { missing: "Missing", unsupported: "Unsupported" };
    const merged = targetOptionViews(["png"], null, "convert-file", labels).map((option) => option.value);
    expect(merged).toEqual(["png", ...SUPPORTED_TARGET_FORMATS.filter((value) => value !== "png")]);
    expect(targetOptionViews(["csv"], null, "preset", labels).map((option) => option.value)[0]).toBe("csv");
  });
});

describe("preset preview invalidation", () => {
  it("invalidates a stale preview for every conversion-affecting preset field except the name", () => {
    expect(presetFieldChangeInvalidatesPreview("target")).toBe(true);
    expect(presetFieldChangeInvalidatesPreview("quality")).toBe(true);
    expect(presetFieldChangeInvalidatesPreview("width")).toBe(true);
    expect(presetFieldChangeInvalidatesPreview("dpi")).toBe(true);
    expect(presetFieldChangeInvalidatesPreview("color-mode")).toBe(true);
    expect(presetFieldChangeInvalidatesPreview("preserve-all-streams")).toBe(true);
    expect(presetFieldChangeInvalidatesPreview("preset-name")).toBe(false);
  });
});

describe("engine recovery notices", () => {
  const labels = {
    engineFallbackNotice: (engine: string, version: string) => `${engine} -> ${version}`,
    engineFailedNotice: (engine: string, reason: string) => `${engine} failed: ${reason}`,
  };

  it("raises notices only for degraded engines", () => {
    const recovery: { engine_recovery: EngineRecoveryOutcome[] } = {
      engine_recovery: [
        { outcome: "activated", engine_id: "formatwright-pdf", version: "26.02.0-0" },
        {
          outcome: "fell_back",
          engine_id: "formatwright-media",
          fallback: {
            failed_version: "9.0",
            failed_manifest_sha256: "ab".repeat(32),
            reason: "hash mismatch",
            fallback_version: "8.0",
          },
        },
        { outcome: "failed", engine_id: "formatwright-image", failed_version: "1.0.0", reason: "no verifiable copy" },
      ],
    };
    expect(engineRecoveryNotices(recovery, labels)).toEqual([
      "formatwright-media -> 8.0",
      "formatwright-image failed: no verifiable copy",
    ]);
    expect(engineRecoveryNotices(null, labels)).toEqual([]);
    expect(engineRecoveryNotices({}, labels)).toEqual([]);
  });

  it("keeps missing fallback details honest instead of crashing", () => {
    expect(
      engineRecoveryNotices({ engine_recovery: [{ outcome: "fell_back", engine_id: "x" }] }, labels),
    ).toEqual(["x -> ?"]);
    expect(
      engineRecoveryNotices({ engine_recovery: [{ outcome: "failed", engine_id: "y" }] }, labels),
    ).toEqual(["y failed: "]);
  });

  it("maps per-engine badge state for the engines page", () => {
    const outcomes: EngineRecoveryOutcome[] = [
      { outcome: "activated", engine_id: "formatwright-pdf" },
      { outcome: "fell_back", engine_id: "formatwright-media" },
      { outcome: "failed", engine_id: "formatwright-image" },
    ];
    expect(engineRecoveryState(outcomes, "formatwright-pdf")).toBeNull();
    expect(engineRecoveryState(outcomes, "formatwright-media")).toBe("fell-back");
    expect(engineRecoveryState(outcomes, "formatwright-image")).toBe("failed");
    expect(engineRecoveryState(outcomes, null)).toBeNull();
    expect(engineRecoveryState(undefined, "formatwright-pdf")).toBeNull();
  });
});

describe("engine certification display", () => {
  it("never treats a trusted signature or a present signature as certified", () => {
    expect(
      packBadgeKind({
        valid: true,
        signature_present: true,
        signature_trust: { status: "trusted", key_id: "release-2026h2" },
        review_status: "incomplete",
        certification: "unverified",
      }),
    ).toBe("trusted-incomplete");
    expect(
      packBadgeKind({
        valid: true,
        signature_present: true,
        signature_trust: { status: "unsigned" },
        review_status: "complete",
        certification: "unverified",
      }),
    ).toBe("unsigned");
    expect(
      packBadgeKind({
        valid: true,
        signature_present: true,
        signature_trust: { status: "trusted", key_id: "release-2026h2" },
        review_status: "complete",
        certification: "certified",
      }),
    ).toBe("certified");
    expect(certificationLabel("experimental", {
      certified: "Certified",
      experimental: "Experimental",
      unverified: "Unverified",
    })).toBe("Experimental");
  });
});
