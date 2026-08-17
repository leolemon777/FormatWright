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
  | "preserve-all-streams";

// Every preset field except the display name feeds conversion plan arguments;
// changing one makes a previously rendered plan preview stale.
export function presetFieldChangeInvalidatesPreview(field: PresetFormField): boolean {
  return field !== "preset-name";
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
    const parsed = JSON.parse(raw) as Partial<DesktopError>;
    if (typeof parsed.message === "string") return { ...parsed, message: parsed.message };
  } catch {
    // The IPC layer may return a plain string for transport/setup failures.
  }
  return { message: raw };
}
