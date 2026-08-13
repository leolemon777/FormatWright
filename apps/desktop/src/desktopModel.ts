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
  if (["md", "markdown", "html", "htm"].includes(extension)) return ["pdf", "docx"];
  if (extension === "pdf") return ["png", "jpg"];
  if (["csv", "json", "yaml", "yml", "xml"].includes(extension)) {
    return ["json", "csv", "yaml", "xml"];
  }
  return ["mp4", "webp", "pdf"];
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
