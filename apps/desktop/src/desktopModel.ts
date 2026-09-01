export type DesktopError = {
  code?: string;
  stage?: string;
  message: string;
  recovery?: string;
};

export const JOB_PAGE_SIZE = 100;

export type JobProgressUpdate = {
  schema_version: number;
  job_id: string;
  job_sequence: number;
  state: string;
  wait_reason: string | null;
  occurred_unix_ms: number;
  eta_milliseconds: number | null;
};

export function latestJobProgress(
  current: JobProgressUpdate | undefined,
  candidate: JobProgressUpdate,
): JobProgressUpdate {
  return current && (
    current.job_sequence > candidate.job_sequence ||
    (current.job_sequence === candidate.job_sequence && current.occurred_unix_ms > candidate.occurred_unix_ms)
  )
    ? current
    : candidate;
}

export function progressForJob(
  progress: JobProgressUpdate | undefined,
  durableSequence: number,
): JobProgressUpdate | undefined {
  return progress && progress.job_sequence >= durableSequence ? progress : undefined;
}

export function elapsedProgressSeconds(progress: JobProgressUpdate, nowUnixMs: number): number {
  return Math.max(0, Math.floor((nowUnixMs - progress.occurred_unix_ms) / 1_000));
}

export function jobListAriaAttributes(offset: number, index: number, total: number) {
  const position = Math.max(1, Math.trunc(offset) + Math.trunc(index) + 1);
  return {
    "aria-posinset": position,
    "aria-setsize": Math.max(position, Math.trunc(total)),
  } as const;
}

export function recommendedTargets(path: string): string[] {
  const extension = path.split(/[\\/]/).pop()?.split(".").pop()?.toLowerCase() ?? "";
  if (["heic", "heif"].includes(extension)) return ["jpg", "png"];
  if (["png", "jpg", "jpeg"].includes(extension)) return ["webp", "avif"];
  if (["mov", "mkv", "avi", "webm"].includes(extension)) return ["mp4", "gif", "mp3"];
  if (["wav", "flac", "aac", "m4a", "ogg", "opus", "mp3"].includes(extension)) {
    return ["m4a", "mp3", "wav"];
  }
  if (["docx", "pptx", "xlsx"].includes(extension)) return ["pdf"];
  if (["xls", "xlsm", "xlsb"].includes(extension)) return [];
  if (["md", "markdown", "html", "htm"].includes(extension)) return ["pdf", "docx"];
  if (extension === "pdf") return ["png", "jpg"];
  if (["csv", "json", "yaml", "yml", "xml"].includes(extension)) {
    return ["json", "csv", "yaml", "xml"];
  }
  return [];
}

export function suggestedOutput(input: string, target: string): string {
  if (!input || !target) return "";
  const normalized = target === "jpeg" ? "jpg" : target;
  const separator = Math.max(input.lastIndexOf("/"), input.lastIndexOf("\\"));
  const directory = separator >= 0 ? input.slice(0, separator + 1) : "";
  const filename = separator >= 0 ? input.slice(separator + 1) : input;
  const dot = filename.lastIndexOf(".");
  const stem = dot > 0 ? filename.slice(0, dot) : filename;
  if (isDirectoryOutput(input, target)) {
    return `${directory}${stem}.converted-${normalized}-pages`;
  }
  return `${directory}${stem}.converted.${normalized}`;
}

export function isDirectoryOutput(input: string, target: string): boolean {
  const extension = input.split(/[\\/]/).pop()?.split(".").pop()?.toLowerCase() ?? "";
  return extension === "pdf" && ["png", "jpg", "jpeg"].includes(target.toLowerCase());
}

export const SUPPORTED_TARGET_FORMATS: readonly string[] = [
  "jpg", "png", "webp", "avif", "mp4", "mp3", "m4a", "wav", "gif", "pdf", "docx", "json", "csv", "yaml", "xml",
];

export type TargetRouteAvailability = {
  available: boolean;
  missing_engines?: readonly string[];
};

export type TargetOptionView = { value: string; label: string; disabled: boolean };

export type TargetOptionScope = "convert-file" | "convert-folder" | "preset";

export type TargetUnavailableLabels = {
  missing: string;
  unsupported: string;
};

function isRelevantConvertTarget(
  route: TargetRouteAvailability | undefined,
): boolean {
  return route?.available === true || (route?.missing_engines?.length ?? 0) > 0;
}

// HowToConvert-style picker: once capabilities are known, hide pairs this
// input cannot use. Keep missing-engine routes so the user can see the pack gap.
export function targetOptionViews(
  recommendations: readonly string[],
  routes: Readonly<Record<string, TargetRouteAvailability>> | null,
  scope: TargetOptionScope,
  unavailableLabels: TargetUnavailableLabels,
): TargetOptionView[] {
  const candidates = Array.from(new Set([...recommendations, ...SUPPORTED_TARGET_FORMATS]));
  const values = scope === "convert-file" && routes !== null
    ? candidates.filter((value) => isRelevantConvertTarget(routes[value]))
    : candidates;
  return values.map((value) => {
    const route = routes?.[value];
    const unavailable = scope !== "convert-folder" && routes !== null && route?.available !== true;
    const reason = (route?.missing_engines?.length ?? 0) > 0
      ? unavailableLabels.missing
      : unavailableLabels.unsupported;
    return {
      value,
      label: unavailable && scope === "convert-file" ? `${value} — ${reason}` : value,
      disabled: unavailable,
    };
  });
}

export function qualityFieldApplies(target: string): boolean {
  return ["jpg", "jpeg", "webp", "avif", "mp3", "m4a", "gif"].includes(target.toLowerCase());
}

export type PlanConstraintSnapshot = {
  quality: number | null;
  width: number | null;
  dpi: number | null;
  colorMode: string | null;
  videoCrf: number | null;
  videoPreset: string | null;
  audioBitrateKbps: number | null;
  preserveAllStreams: boolean;
};

export function videoKnobsApply(target: string): boolean {
  return target.toLowerCase() === "mp4";
}

export function audioBitrateApplies(target: string): boolean {
  return [
    "mp4", "mp3", "m4a", "wav", "flac", "ogg", "opus", "aac",
  ].includes(target.toLowerCase());
}

// CLI `convert INPUT --to T` has no leftover GUI flags. Explorer convert and a
// fresh selectInput must start from this snapshot, not a hot Convert form.
export function defaultPlanConstraints(_target: string): PlanConstraintSnapshot {
  return {
    quality: null,
    width: null,
    dpi: null,
    colorMode: null,
    videoCrf: null,
    videoPreset: null,
    audioBitrateKbps: null,
    preserveAllStreams: true,
  };
}

export function normalizeTargetFormat(target: string): string {
  const normalized = target.trim().replace(/^\./, "").toLowerCase();
  if (normalized === "jpeg") return "jpg";
  if (normalized === "yml") return "yaml";
  return normalized;
}

export type PendingCapabilityDecision = {
  target: string | null;
  clearPending: boolean;
};

// T-UI-06 / T-UI-12: while Explorer convert is pending, keep the approved
// target. If that route is missing, fail honestly instead of jumping.
export function resolvePendingCapabilityTarget(args: {
  pendingWanted: string | null;
  currentTarget: string;
  inputPath: string;
  routes: Readonly<Record<string, TargetRouteAvailability & { target_format?: string }>>;
}): PendingCapabilityDecision {
  if (args.pendingWanted) {
    const wanted = normalizeTargetFormat(args.pendingWanted);
    if (args.routes[wanted]?.available) {
      return { target: wanted, clearPending: false };
    }
    return { target: null, clearPending: true };
  }
  const current = normalizeTargetFormat(args.currentTarget);
  if (args.routes[current]?.available) {
    return { target: null, clearPending: false };
  }
  const firstRecommended = recommendedTargets(args.inputPath).find(
    (candidate) => args.routes[candidate]?.available,
  );
  const firstAvailable = Object.values(args.routes).find((route) => route.available)?.target_format;
  return { target: firstRecommended ?? firstAvailable ?? null, clearPending: false };
}

export type EmptyStateCardId = "pdf-png" | "json-yaml" | "video-mp4";

export type EmptyStateCardSpec = {
  id: EmptyStateCardId;
  target: string;
  filters: { name: string; extensions: string[] };
};

export const EMPTY_STATE_CARDS: readonly EmptyStateCardSpec[] = [
  { id: "pdf-png", target: "png", filters: { name: "PDF", extensions: ["pdf"] } },
  { id: "json-yaml", target: "yaml", filters: { name: "JSON", extensions: ["json"] } },
  { id: "video-mp4", target: "mp4", filters: { name: "Video", extensions: ["mkv", "mov", "avi", "webm"] } },
];

export function emptyStateCardAvailability(
  card: EmptyStateCardId,
  routes: Readonly<Record<string, TargetRouteAvailability>> | null,
): { available: boolean; missingEngines: string[] } {
  if (card === "json-yaml") {
    return { available: true, missingEngines: [] };
  }
  const target = card === "pdf-png" ? "png" : "mp4";
  const route = routes?.[target];
  return {
    available: route?.available === true,
    missingEngines: [...(route?.missing_engines ?? [])],
  };
}

export function pdfPageCountFromReport(
  report: { checks: ReadonlyArray<{ code: string; observed: unknown }> },
): number | null {
  const check = report.checks.find((entry) => entry.code === "PDF_PAGE_COUNT");
  if (check == null) return null;
  const observed = Number(check.observed);
  return Number.isFinite(observed) ? observed : null;
}

export function pathStemAndExt(input: string): { directory: string; stem: string; ext: string } {
  const separator = Math.max(input.lastIndexOf("/"), input.lastIndexOf("\\"));
  const directory = separator >= 0 ? input.slice(0, separator + 1) : "";
  const filename = separator >= 0 ? input.slice(separator + 1) : input;
  const dot = filename.lastIndexOf(".");
  const stem = dot > 0 ? filename.slice(0, dot) : filename;
  const ext = (dot > 0 ? filename.slice(dot + 1) : "bin").toLowerCase();
  return { directory, stem, ext };
}

export function suggestedConvertedName(
  input: string,
  target: string,
  reserved: readonly string[],
): string {
  const normalized = target === "jpeg" ? "jpg" : target;
  const { directory, stem, ext } = pathStemAndExt(input);
  const first = isDirectoryOutput(input, normalized)
    ? `${directory}${stem}.converted-${normalized}-pages`
    : `${directory}${stem}.converted.${normalized}`;
  if (!reserved.includes(first)) return first;
  const second = isDirectoryOutput(input, normalized)
    ? `${directory}${stem}.from-${ext}.converted-${normalized}-pages`
    : `${directory}${stem}.from-${ext}.converted.${normalized}`;
  if (!reserved.includes(second)) return second;
  let index = 2;
  while (true) {
    const candidate = isDirectoryOutput(input, normalized)
      ? `${directory}${stem}.from-${ext}-${index}.converted-${normalized}-pages`
      : `${directory}${stem}.from-${ext}-${index}.converted.${normalized}`;
    if (!reserved.includes(candidate)) return candidate;
    index += 1;
  }
}

export type PlainLossBadge = "lossy" | "drop-tracks" | "unknown" | "container" | "lossless";

export function plainLossSummary(plan: {
  steps: ReadonlyArray<{ loss_class: string }>;
  changes: { dropped: readonly string[] };
}): PlainLossBadge {
  const classes = plan.steps.map((step) => step.loss_class.toLowerCase());
  if (classes.includes("lossy")) return "lossy";
  const droppedTracks = plan.changes.dropped.some((item) =>
    /track|stream|subtitle|chapter/i.test(item),
  );
  if (droppedTracks) return "drop-tracks";
  if (classes.includes("unknown")) return "unknown";
  if (classes.every((item) => item === "none" || item === "container-only")) return "container";
  return "lossless";
}

export function basicModeFailureCopy(
  inputPath: string,
  error: { code?: string; message: string },
  missingEngines: readonly string[],
  labels: {
    oldExcel: string;
    unsupported: string;
    engineMissing: string;
    outputConflict: string;
    policyBlocked: string;
  },
): string {
  const ext = pathStemAndExt(inputPath).ext;
  if (["xls", "xlsm", "xlsb"].includes(ext)) return labels.oldExcel;
  const code = (error.code ?? "").toUpperCase();
  if (code === "UNSUPPORTED") return labels.unsupported;
  if (code === "ENGINE_MISSING" || missingEngines.length > 0) {
    return labels.engineMissing.replace("{names}", missingEngines.join(", ") || "soffice");
  }
  if (code === "OUTPUT_CONFLICT") return labels.outputConflict;
  if (code === "POLICY_BLOCKED") return labels.policyBlocked;
  return error.message;
}

export type DesktopDropKind = "file" | "directory" | "rejected";

export const SHELL_CONVERT_TARGETS: readonly string[] = [
  "jpg", "png", "webp", "avif", "mp4", "mp3", "m4a", "wav", "gif", "pdf", "docx", "json", "csv", "yaml", "xml",
];

export function normalizeShellTarget(value: string | null | undefined): string | null {
  const normalized = (value ?? "").trim().replace(/^\./, "").toLowerCase();
  if (normalized === "jpeg") return "jpg";
  if (normalized === "yml") return "yaml";
  return SHELL_CONVERT_TARGETS.includes(normalized) ? normalized : null;
}

export function inputHasRunnableFamily(
  routes: Readonly<Record<string, TargetRouteAvailability>> | null | undefined,
): boolean {
  return Object.values(routes ?? {}).some(
    (route) => route.available || (route.missing_engines?.length ?? 0) > 0,
  );
}

export type EngineRecoveryOutcome = {
  outcome: "activated" | "fell_back" | "failed";
  engine_id: string;
  version?: string;
  manifest_sha256?: string;
  fallback?: {
    failed_version: string;
    failed_manifest_sha256: string;
    reason: string;
    fallback_version: string;
  };
  failed_version?: string;
  reason?: string;
};

export type EngineRecoveryState = "fell-back" | "failed";

// Only degraded engine states belong on the recovery banner; a plain
// activated outcome is the normal startup path and must not raise a notice.
export function engineRecoveryNotices(
  recovery: { engine_recovery?: readonly EngineRecoveryOutcome[] } | null | undefined,
  labels: {
    engineFallbackNotice: (engine: string, version: string) => string;
    engineFailedNotice: (engine: string, reason: string) => string;
  },
): string[] {
  return (recovery?.engine_recovery ?? [])
    .filter((outcome) => outcome.outcome !== "activated")
    .map((outcome) =>
      outcome.outcome === "fell_back"
        ? labels.engineFallbackNotice(
            outcome.engine_id,
            outcome.fallback?.fallback_version ?? "?",
          )
        : labels.engineFailedNotice(outcome.engine_id, outcome.reason ?? ""),
    );
}

export function engineRecoveryState(
  outcomes: readonly EngineRecoveryOutcome[] | undefined,
  engineId: string | null | undefined,
): EngineRecoveryState | null {
  if (!engineId) return null;
  const match = (outcomes ?? []).find((outcome) => outcome.engine_id === engineId);
  if (!match) return null;
  if (match.outcome === "fell_back") return "fell-back";
  if (match.outcome === "failed") return "failed";
  return null;
}

export type PresetFormField =
  | "preset-name"
  | "target"
  | "quality"
  | "width"
  | "dpi"
  | "color-mode"
  | "video-crf"
  | "video-preset"
  | "audio-bitrate"
  | "preserve-all-streams";

// Every preset field except the display name feeds conversion plan arguments;
// changing one makes a previously rendered plan preview stale.
export function presetFieldChangeInvalidatesPreview(
  field: PresetFormField,
  target?: string,
): boolean {
  if (field === "preset-name") return false;
  if (field === "quality" && target != null && !qualityFieldApplies(target)) return false;
  return true;
}

export type SignatureTrustView = {
  status: string;
  key_id?: string;
};

export type EnginePackTrustView = {
  valid: boolean;
  signature_present: boolean;
  signature_trust?: SignatureTrustView | null;
  review_status?: string | null;
  certification?: string | null;
};

export type PackBadgeKind =
  | "certified"
  | "trusted-incomplete"
  | "untrusted"
  | "unsigned"
  | "invalid";

export function packBadgeKind(pack: EnginePackTrustView): PackBadgeKind {
  if (!pack.valid) return "invalid";
  if (pack.certification === "certified") return "certified";
  const trust = pack.signature_trust?.status;
  if (trust === "trusted") return "trusted-incomplete";
  if (
    trust === "revoked" ||
    trust === "expired" ||
    trust === "invalid_signature" ||
    trust === "unknown_key"
  ) {
    return "untrusted";
  }
  return "unsigned";
}

export function certificationLabel(
  certification: string | undefined,
  labels: { certified: string; experimental: string; unverified: string },
): string {
  if (certification === "certified") return labels.certified;
  if (certification === "experimental") return labels.experimental;
  return labels.unverified;
}

export function parseDesktopError(reason: unknown): DesktopError {
  const raw = reason instanceof Error ? reason.message : String(reason);
  try {
    const parsed = JSON.parse(raw) as Partial<DesktopError> & { user_action?: string };
    if (typeof parsed.message === "string") {
      return {
        code: parsed.code,
        stage: parsed.stage,
        message: parsed.message,
        recovery: parsed.recovery ?? parsed.user_action,
      };
    }
  } catch {
    // The IPC layer may return a plain string for transport/setup failures.
  }
  return { message: raw };
}

export type LocalizedDesktopError = {
  title: string;
  message: string;
  recovery?: string;
};

type ErrorCopy = {
  stageError: string;
  errorTitlePolicyBlocked: string;
  errorTitleUnsupported: string;
  errorTitleEngineMissing: string;
  errorTitleOutputConflict: string;
  errorTitleInputInvalid: string;
  errorTitleInputChanged: string;
  errorTitleEngineIncompatible: string;
  errorTitleResourceExhausted: string;
  errorTitleExecutionFailed: string;
  errorTitleCancelled: string;
  errorTitleValidationFailed: string;
  errorTitleStorageFailed: string;
  errorTitleInternal: string;
  stageInspect: string;
  stagePlan: string;
  stageExecute: string;
  stageValidate: string;
  stageCommit: string;
  stageStore: string;
  stageDoctor: string;
  oldExcel: string;
  pairUnsupported: string;
  engineMissingPack: string;
  outputExists: string;
  policyBlocked: string;
  errorFolderOverlap: string;
  errorFolderOverlapHint: string;
  errorFolderTooMany: string;
  errorFolderNotLocal: string;
  errorFolderNotDir: string;
  errorFolderInvalidTarget: string;
  errorFolderDisk: string;
  errorFolderOutputExists: string;
  errorFolderNoRoute: string;
  errorFolderNoRouteHint: string;
  errorOutputGone: string;
  errorExportExists: string;
  errorExportLimit: string;
  errorGenericPolicy: string;
  errorGenericInput: string;
  errorRevealFailed: string;
};

function looksEnglish(text: string): boolean {
  const letters = text.replace(/[^A-Za-z]/g, "");
  return letters.length >= 8 && letters.length >= text.replace(/\s/g, "").length * 0.5;
}

export function localizeDesktopError(error: DesktopError, copy: ErrorCopy): LocalizedDesktopError {
  const code = (error.code ?? "").toUpperCase();
  const stage = (error.stage ?? "").toLowerCase();
  const blob = `${error.message}\n${error.recovery ?? ""}`;
  const title = errorTitleForCode(code, copy);
  const stageLabel = stageLabelFor(stage, copy);
  const heading = stageLabel ? `${title} · ${stageLabel}` : title;

  if (/input and output roots must not overlap|choose two separate local folders/i.test(blob)) {
    return { title: heading, message: copy.errorFolderOverlap, recovery: copy.errorFolderOverlapHint };
  }
  if (/100,000-file limit|10,000-Plan limit/i.test(blob)) {
    return { title: heading, message: copy.errorFolderTooMany };
  }
  if (/must be a local disk path|network/i.test(blob) && /folder batch/i.test(blob)) {
    return { title: heading, message: copy.errorFolderNotLocal };
  }
  if (/not a local directory/i.test(blob)) {
    return { title: heading, message: copy.errorFolderNotDir };
  }
  if (/target extension is invalid/i.test(blob)) {
    return { title: heading, message: copy.errorFolderInvalidTarget };
  }
  if (/requires .* bytes|insufficient disk|disk space/i.test(blob)) {
    return { title: heading, message: copy.errorFolderDisk };
  }
  if (/folder batch output already exists|output appeared after preview/i.test(blob)) {
    return { title: heading, message: copy.errorFolderOutputExists };
  }
  if (/no file in the selected folder can use this conversion route/i.test(blob)) {
    return { title: heading, message: copy.errorFolderNoRoute, recovery: copy.errorFolderNoRouteHint };
  }
  if (/output is no longer available/i.test(blob)) {
    return { title: heading, message: copy.errorOutputGone };
  }
  if (/will not overwrite|could not be committed without overwriting/i.test(blob)) {
    return { title: heading, message: copy.errorExportExists };
  }
  if (/16 MiB limit|exceeds the 1 MiB/i.test(blob)) {
    return { title: heading, message: copy.errorExportLimit };
  }
  if (/file browser could not be opened/i.test(blob)) {
    return { title: heading, message: copy.errorRevealFailed };
  }

  const byCode = fallbackMessageForCode(code, copy);
  if (!looksEnglish(error.message)) {
    return { title: heading, message: error.message, recovery: error.recovery };
  }
  return {
    title: heading,
    message: byCode,
    recovery: error.recovery && !looksEnglish(error.recovery) ? error.recovery : undefined,
  };
}

function errorTitleForCode(code: string, copy: ErrorCopy): string {
  switch (code) {
    case "POLICY_BLOCKED":
      return copy.errorTitlePolicyBlocked;
    case "UNSUPPORTED":
      return copy.errorTitleUnsupported;
    case "ENGINE_MISSING":
      return copy.errorTitleEngineMissing;
    case "OUTPUT_CONFLICT":
      return copy.errorTitleOutputConflict;
    case "INPUT_INVALID":
      return copy.errorTitleInputInvalid;
    case "INPUT_CHANGED":
      return copy.errorTitleInputChanged;
    case "ENGINE_INCOMPATIBLE":
      return copy.errorTitleEngineIncompatible;
    case "RESOURCE_EXHAUSTED":
      return copy.errorTitleResourceExhausted;
    case "EXECUTION_FAILED":
      return copy.errorTitleExecutionFailed;
    case "CANCELLED":
      return copy.errorTitleCancelled;
    case "VALIDATION_FAILED":
      return copy.errorTitleValidationFailed;
    case "STORAGE_FAILED":
      return copy.errorTitleStorageFailed;
    case "INTERNAL":
      return copy.errorTitleInternal;
    default:
      return copy.stageError;
  }
}

function stageLabelFor(stage: string, copy: ErrorCopy): string | null {
  switch (stage) {
    case "inspect":
      return copy.stageInspect;
    case "plan":
      return copy.stagePlan;
    case "execute":
      return copy.stageExecute;
    case "validate":
      return copy.stageValidate;
    case "commit":
      return copy.stageCommit;
    case "store":
      return copy.stageStore;
    case "doctor":
      return copy.stageDoctor;
    default:
      return null;
  }
}

function fallbackMessageForCode(code: string, copy: ErrorCopy): string {
  switch (code) {
    case "POLICY_BLOCKED":
      return copy.errorGenericPolicy;
    case "UNSUPPORTED":
      return copy.pairUnsupported;
    case "ENGINE_MISSING":
      return copy.engineMissingPack.replace("{names}", "");
    case "OUTPUT_CONFLICT":
      return copy.outputExists;
    case "INPUT_INVALID":
      return copy.errorGenericInput;
    default:
      return copy.stageError;
  }
}
