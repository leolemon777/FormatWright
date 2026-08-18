import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open, save } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  EMPTY_STATE_CARDS,
  JOB_PAGE_SIZE,
  certificationLabel,
  defaultPlanConstraints,
  elapsedProgressSeconds,
  emptyStateCardAvailability,
  engineRecoveryNotices,
  engineRecoveryState,
  inputHasRunnableFamily,
  isDirectoryOutput,
  jobListAriaAttributes,
  latestJobProgress,
  packBadgeKind,
  parseDesktopError,
  localizeDesktopError,
  pdfPageCountFromReport,
  plainLossSummary,
  basicModeFailureCopy,
  presetFieldChangeInvalidatesPreview,
  qualityFieldApplies,
  progressForJob,
  recommendedTargets,
  resolvePendingCapabilityTarget,
  suggestedOutput,
  targetOptionViews,
  type EmptyStateCardId,
  type EngineRecoveryOutcome,
  type JobProgressUpdate,
} from "./desktopModel";
import { messages, type Language } from "./i18n";
import {
  QueueProjection,
  type QueueDeltaBatch,
  type QueueSnapshot,
} from "./queueProjection";
import "./styles.css";

type Tab = "convert" | "jobs" | "presets" | "engines" | "reports" | "maintenance" | "settings";
type JsonMap = Record<string, unknown>;

type Probe = {
  format: { id: string; kind: string; confidence: number; extension_matches?: boolean };
  artifact: { display_path: string; size_bytes: number };
  streams: Array<{ width?: number; height?: number; codec?: string; properties: JsonMap }>;
  warnings: Array<{ code: string; message: string }>;
};

type PlanStep = {
  step_id: string;
  capability_id: string;
  engine: { engine_id: string; version: string; certification: string };
  operation: string;
  loss_class: string;
  arguments: Record<string, string>;
};

type Plan = {
  plan_hash: string;
  target_format: string;
  network_policy: string;
  steps: PlanStep[];
  changes: { preserved: string[]; changed: string[]; dropped: string[]; unknown: string[] };
  constraints: JsonMap;
};

type Preview = { probe: Probe; plan: Plan };

type ValidationCheck = {
  code: string;
  status: string;
  required: boolean;
  message: string;
  expected: unknown;
  observed: unknown;
};

type ValidationReport = {
  job_id: string;
  plan_hash: string;
  status: string;
  output: { display_path?: string; format_id: string; size_bytes: number };
  checks: ValidationCheck[];
  engines: Array<{ engine_id: string; version: string; certification: string }>;
  intentional_changes: string[];
  redaction: { paths_redacted: boolean; metadata_values_redacted: boolean };
};

type JobRecord = {
  id: string;
  state: string;
  input_path: string;
  output_path: string;
  sequence: number;
  updated_unix_ms: number;
};

type ShellOpen = { path: string; directory: boolean; convert_to?: string | null };
type DropClassification = { kind: "file" | "directory" | "rejected"; path?: string | null };
type ShellConvertBatch = { target: string; paths: string[] };
type IngestResult = {
  ran_immediately: boolean;
  batch_id?: string | null;
  queued: number;
  job?: JobRecord | null;
  report?: ValidationReport | null;
  skipped_conflict: number;
  skipped_disk: number;
  rejected: number;
};

type JobQueryPage = {
  jobs: JobRecord[];
  total: number;
  limit: number;
  offset: number;
};

type StagedCleanupReport = {
  job: JobRecord;
  removed: boolean;
};

type BatchRecord = {
  id: string;
  name: string;
  job_count: number;
  updated_unix_ms: number;
};

type JobStateCount = { state: string; count: number };

type RecoverySummary = {
  recovered_after_restart: number;
  removed_staged_outputs: number;
  restored_bundle_id?: string;
  restore_error?: string;
  engine_recovery?: EngineRecoveryOutcome[];
  state_counts: JobStateCount[];
};

type MaintenanceStatus = {
  database_path: string;
  size_bytes: number;
  schema_version: number;
  supported_schema_version: number;
  journal_mode: string;
  job_count: number;
  active_job_count: number;
  integrity_ok: boolean;
};

type IntegrityReport = {
  ok: boolean;
  sqlite_messages: string[];
  foreign_key_violations: string[];
  application_issues: string[];
};

type StateBundleBackupReport = {
  bundle_path: string;
  bundle_id: string;
  size_bytes: number;
  entry_count: number;
  reports_included: boolean;
};

type StateBundlePreflightReport = {
  bundle_path: string;
  bundle_id: string;
  application_version: string;
  entry_count: number;
  total_uncompressed_bytes: number;
  reports_included: boolean;
  database: {
    source_schema_version: number;
    restored_schema_version: number;
    migration_required: boolean;
    integrity: IntegrityReport;
  };
  warnings: string[];
};

type CompactReport = {
  size_before_bytes: number;
  size_after_bytes: number;
  reclaimed_bytes: number;
  integrity: IntegrityReport;
};

type FolderMappingEntry = {
  input_path: string;
  relative_input_path: string;
  output_path: string;
};

type FolderPreview = {
  preview_id: string;
  expires_unix_ms: number;
  input_root: string;
  output_root: string;
  target_format: string;
  discovered: number;
  planned: number;
  skipped: number;
  sample: FolderMappingEntry[];
  truncated: boolean;
  disk_budget: {
    estimated_output_bytes: number;
    peak_temporary_bytes: number;
    safety_margin_bytes: number;
    required_bytes: number;
    available_bytes: number;
    sufficient: boolean;
  };
};

type FolderQueueResult = {
  batch: BatchRecord;
  queued: number;
};

type DoctorReport = {
  engines: Record<
    string,
    {
      available: boolean;
      message: string;
      identity?: { version: string; certification: string };
      signature_trust?: { status: string; key_id?: string } | null;
      review_status?: string | null;
    }
  >;
};

type RouteAvailability = {
  target_format: string;
  available: boolean;
  required_engines: string[];
  missing_engines: string[];
  message: string;
};

type CapabilitySnapshot = {
  input_extension?: string;
  routes: Record<string, RouteAvailability>;
};

type EnginePackSummary = {
  manifest_path: string;
  engine_id?: string;
  version?: string;
  manifest_sha256?: string;
  executable_names: string[];
  signature_present: boolean;
  signature_trust?: { status: string; key_id?: string } | null;
  review_status?: string | null;
  certification?: string | null;
  valid: boolean;
  message: string;
};

type BenchmarkResult = {
  total_jobs: number;
  emitted_batches: number;
  maximum_batch_jobs: number;
  elapsed_milliseconds: number;
};

type QueueRunReport = {
  schema_version: number;
  selected: number;
  completed: number;
  warning: number;
  blocked: number;
  failed: number;
  cancelled: number;
  contended: number;
  stopped: boolean;
  parallelism: number;
  peak_active: number;
};

type SelectionSnapshot = {
  id: string;
  member_count: number;
};

type BulkActionReport = {
  action_id: string;
  selection_id: string;
  action: "cancel" | "resume" | "retry";
  matched: number;
  transitioned: number;
  skipped_state: number;
  skipped_conflict: number;
};

type ConversionPreset = {
  schema_version: number;
  preset_id: string;
  name: string;
  target_format: string;
  quality?: number;
  width?: number;
  dpi?: number;
  color_mode?: string;
  preserve_all_streams: boolean;
};

type PresetImportResult = { imported: number; total: number };
type ApplicationSettings = {
  schema_version: number;
  language: Language;
  expert_mode: boolean;
};

const emptySnapshot: QueueSnapshot = {
  totalJobs: 0,
  completed: 0,
  active: 0,
  failed: 0,
  lastBatchSequence: -1,
  visibleJobs: [],
};

const jobStateOptions = [
  "queued",
  "inspecting",
  "planned",
  "blocked",
  "running",
  "validating",
  "completed",
  "warning",
  "failed",
  "cancelled",
  "interrupted",
] as const;

export default function App() {
  const [language, setLanguage] = useState<Language>(() =>
    navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en",
  );
  const [expert, setExpert] = useState(false);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [tab, setTab] = useState<Tab>("convert");
  const [inputPath, setInputPath] = useState("");
  const [convertMode, setConvertMode] = useState<"file" | "folder">("file");
  const [folderInputRoot, setFolderInputRoot] = useState("");
  const [folderOutputRoot, setFolderOutputRoot] = useState("");
  const [folderPreview, setFolderPreview] = useState<FolderPreview | null>(null);
  const [folderBusy, setFolderBusy] = useState<"preview" | "queue" | null>(null);
  const [outputPath, setOutputPath] = useState("");
  const [target, setTarget] = useState("webp");
  const [quality, setQuality] = useState("85");
  const [width, setWidth] = useState("");
  const [dpi, setDpi] = useState("144");
  const [colorMode, setColorMode] = useState("rgb");
  const [preserveAllStreams, setPreserveAllStreams] = useState(true);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [report, setReport] = useState<ValidationReport | null>(null);
  const [reportBusy, setReportBusy] = useState<"report" | "recipe" | "reveal" | "revalidate" | null>(null);
  const [reportNotice, setReportNotice] = useState<string | null>(null);
  const [redactReportPaths, setRedactReportPaths] = useState(true);
  const [jobs, setJobs] = useState<JobRecord[]>([]);
  const [doctor, setDoctor] = useState<DoctorReport | null>(null);
  const [capabilities, setCapabilities] = useState<CapabilitySnapshot | null>(null);
  const [capabilityBusy, setCapabilityBusy] = useState(false);
  const [enginePacks, setEnginePacks] = useState<EnginePackSummary[]>([]);
  const [engineBusy, setEngineBusy] = useState(false);
  const [busy, setBusy] = useState<"plan" | "run" | "queue" | "queue-run" | null>(null);
  const [activeJobId, setActiveJobId] = useState<string | null>(null);
  const [error, setError] = useState<ReturnType<typeof parseDesktopError> | null>(null);
  const [dragging, setDragging] = useState(false);
  const projection = useMemo(() => new QueueProjection(), []);
  const [queueSnapshot, setQueueSnapshot] = useState(emptySnapshot);
  const [benchmark, setBenchmark] = useState<BenchmarkResult | null>(null);
  const [queueReport, setQueueReport] = useState<QueueRunReport | null>(null);
  const [presets, setPresets] = useState<ConversionPreset[]>([]);
  const [presetName, setPresetName] = useState("");
  const [editingPresetId, setEditingPresetId] = useState<string | null>(null);
  const [pendingDeleteId, setPendingDeleteId] = useState<string | null>(null);
  const [presetNotice, setPresetNotice] = useState<string | null>(null);
  const [presetBusy, setPresetBusy] = useState(false);
  const [jobActionBusy, setJobActionBusy] = useState<string | null>(null);
  const [jobCleanupBusy, setJobCleanupBusy] = useState<string | null>(null);
  const [pendingCleanupId, setPendingCleanupId] = useState<string | null>(null);
  const [jobActionNotice, setJobActionNotice] = useState<string | null>(null);
  const [jobSearch, setJobSearch] = useState("");
  const [jobStateFilter, setJobStateFilter] = useState("");
  const [jobBatchId, setJobBatchId] = useState("");
  const [jobOffset, setJobOffset] = useState(0);
  const [jobTotal, setJobTotal] = useState(0);
  const [jobBatches, setJobBatches] = useState<BatchRecord[]>([]);
  const [jobProgress, setJobProgress] = useState<Record<string, JobProgressUpdate>>({});
  const [progressClock, setProgressClock] = useState(() => Date.now());
  const [recovery, setRecovery] = useState<RecoverySummary | null>(null);
  const [recoveryDismissed, setRecoveryDismissed] = useState(false);
  const [maintenanceStatus, setMaintenanceStatus] = useState<MaintenanceStatus | null>(null);
  const [maintenanceResult, setMaintenanceResult] = useState<string | null>(null);
  const [restorePreflight, setRestorePreflight] = useState<StateBundlePreflightReport | null>(null);
  const [maintenanceBusy, setMaintenanceBusy] = useState<"status" | "check" | "backup" | "compact" | "preflight" | "restore" | null>(null);
  const [includeReports, setIncludeReports] = useState(false);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkReport, setBulkReport] = useState<BulkActionReport | null>(null);
  const queueSubmission = useRef<{ intent: string; key: string } | null>(null);
  const jobQuery = useRef({ batchId: "", state: "", search: "", offset: 0 });
  const jobRefreshSequence = useRef(0);
  const mounted = useRef(true);
  const pendingShellConvert = useRef<string | null>(null);
  const shellConvertRunning = useRef(false);
  const [probePdfRoutes, setProbePdfRoutes] = useState<CapabilitySnapshot["routes"] | null>(null);
  const [probeVideoRoutes, setProbeVideoRoutes] = useState<CapabilitySnapshot["routes"] | null>(null);
  const copy = messages[language];
  jobQuery.current = {
    batchId: jobBatchId,
    state: jobStateFilter,
    search: jobSearch,
    offset: jobOffset,
  };

  useEffect(() => {
    document.documentElement.lang = language;
    if (!settingsLoaded || !("__TAURI_INTERNALS__" in window)) return;
    void invoke<ApplicationSettings>("save_desktop_settings", {
      settings: { schema_version: 1, language, expert_mode: expert },
    }).catch((reason) => setError(parseDesktopError(reason)));
  }, [expert, language, settingsLoaded]);

  useEffect(() => {
    mounted.current = true;
    const disposers: Array<() => void> = [];
    void listen<QueueDeltaBatch>("formatwright://queue-delta", (event) => {
      projection.apply(event.payload, requestAnimationFrame, (next) => {
        if (mounted.current) setQueueSnapshot(next);
      });
    }).then((dispose) => disposers.push(dispose));
    void listen<JobRecord>("formatwright://job-updated", (event) => {
      if (event.payload.state === "running") setActiveJobId(event.payload.id);
      void refreshJobs();
    }).then((dispose) => disposers.push(dispose));
    void listen<JobProgressUpdate>("formatwright://job-progress", (event) => {
      if (!mounted.current) return;
      setJobProgress((current) => ({
        ...current,
        [event.payload.job_id]: latestJobProgress(current[event.payload.job_id], event.payload),
      }));
      setProgressClock(Date.now());
    }).then((dispose) => disposers.push(dispose));
    void listen<QueueRunReport>("formatwright://queue-window-finished", (event) => {
      if (mounted.current) setQueueReport(event.payload);
      void refreshJobs();
    }).then((dispose) => disposers.push(dispose));
    if ("__TAURI_INTERNALS__" in window) {
      let consumingShellOpen = false;
      let shellOpenRequested = false;
      const consumeShellOpens = async () => {
        shellOpenRequested = true;
        if (consumingShellOpen) return;
        consumingShellOpen = true;
        try {
          do {
            shellOpenRequested = false;
            while (mounted.current) {
              const shellOpen = await invoke<ShellOpen | null>("get_desktop_shell_open");
              if (!shellOpen) break;
              applyShellOpen(shellOpen);
            }
          } while (mounted.current && shellOpenRequested);
        } catch {
          // A normal app launch has no shell-selected path.
        } finally {
          consumingShellOpen = false;
        }
      };
      void listen<void>("formatwright://shell-open-requested", () => void consumeShellOpens())
        .then((dispose) => {
          if (!mounted.current) {
            dispose();
            return;
          }
          disposers.push(dispose);
          void consumeShellOpens();
        });
      let consumingConvert = false;
      let convertRequested = false;
      const consumeConvertBatches = async () => {
        convertRequested = true;
        if (consumingConvert) return;
        consumingConvert = true;
        try {
          do {
            convertRequested = false;
            while (mounted.current) {
              const batch = await invoke<ShellConvertBatch | null>("take_desktop_shell_convert_batch");
              if (!batch) break;
              await handleIngestBatch(batch);
            }
          } while (mounted.current && convertRequested);
        } catch (reason) {
          if (mounted.current) setError(parseDesktopError(reason));
        } finally {
          consumingConvert = false;
        }
      };
      void listen<void>("formatwright://shell-convert-batch", () => void consumeConvertBatches())
        .then((dispose) => {
          if (!mounted.current) {
            dispose();
            return;
          }
          disposers.push(dispose);
          void consumeConvertBatches();
        });
      void invoke<ApplicationSettings | null>("get_desktop_settings")
        .then(async (settings) => {
          if (!mounted.current) return;
          const migrated: ApplicationSettings = settings ?? {
            schema_version: 1,
            language: localStorage.getItem("fw-language") === "zh-CN"
              ? "zh-CN"
              : localStorage.getItem("fw-language") === "en"
                ? "en"
                : navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en",
            expert_mode: localStorage.getItem("fw-expert") === "true",
          };
          setLanguage(migrated.language);
          setExpert(migrated.expert_mode);
          if (!settings) {
            try {
              await invoke<ApplicationSettings>("save_desktop_settings", { settings: migrated });
              localStorage.removeItem("fw-language");
              localStorage.removeItem("fw-expert");
            } catch (reason) {
              if (mounted.current) setError(parseDesktopError(reason));
            }
          }
          if (mounted.current) setSettingsLoaded(true);
        })
        .catch((reason) => {
          if (mounted.current) setError(parseDesktopError(reason));
        });
      void getCurrentWebviewWindow()
        .onDragDropEvent((event) => {
          if (event.payload.type === "enter" || event.payload.type === "over") setDragging(true);
          if (event.payload.type === "leave") setDragging(false);
          if (event.payload.type === "drop") {
            setDragging(false);
            const first = event.payload.paths[0];
            if (first) void classifyAndSelectDrop(first);
          }
        })
        .then((dispose) => disposers.push(dispose));
    }
    void refreshJobs();
    void refreshJobBatches();
    void refreshRecovery();
    void refreshMaintenanceStatus();
    void refreshEngines();
    void refreshPresets();
    void loadStarterProbes();
    return () => {
      mounted.current = false;
      for (const dispose of disposers) dispose();
    };
  }, [projection]);

  useEffect(() => {
    if (busy !== "queue-run" && busy !== "run") return;
    const timer = window.setInterval(() => setProgressClock(Date.now()), 1_000);
    return () => window.clearInterval(timer);
  }, [busy]);

  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) return;
    const timer = window.setTimeout(() => void refreshJobs(0), 200);
    return () => window.clearTimeout(timer);
  }, [jobBatchId, jobSearch, jobStateFilter]);

  useEffect(() => {
    if (!inputPath || !("__TAURI_INTERNALS__" in window)) {
      setCapabilities(null);
      setCapabilityBusy(false);
      return;
    }
    let current = true;
    setCapabilityBusy(true);
    void invoke<CapabilitySnapshot>("desktop_capability_snapshot", { inputPath })
      .then((snapshot) => {
        if (!current) return;
        setCapabilities(snapshot);
        const decision = resolvePendingCapabilityTarget({
          pendingWanted: pendingShellConvert.current,
          currentTarget: target,
          inputPath,
          routes: snapshot.routes,
        });
        if (decision.clearPending) {
          pendingShellConvert.current = null;
        }
        if (decision.target) {
          setTarget(decision.target);
          setOutputPath(suggestedOutput(inputPath, decision.target));
        }
      })
      .catch((reason) => {
        if (current) setError(parseDesktopError(reason));
      })
      .finally(() => {
        if (current) setCapabilityBusy(false);
      });
    return () => {
      current = false;
    };
  }, [inputPath]);

  function applyDefaultPlanConstraints(nextTarget: string) {
    const defaults = defaultPlanConstraints(nextTarget);
    setQuality(defaults.quality == null ? "" : String(defaults.quality));
    setWidth(defaults.width == null ? "" : String(defaults.width));
    setDpi(defaults.dpi == null ? "" : String(defaults.dpi));
    setColorMode(defaults.colorMode ?? "");
    setPreserveAllStreams(defaults.preserveAllStreams);
  }

  function selectInput(path: string) {
    const recommended = recommendedTargets(path)[0] ?? "";
    applyDefaultPlanConstraints(recommended);
    setInputPath(path);
    setTarget(recommended);
    setOutputPath(recommended ? suggestedOutput(path, recommended) : "");
    setPreview(null);
    setReport(null);
    setError(null);
  }

  function applyShellOpen(shellOpen: ShellOpen) {
    if (shellOpen.directory) {
      pendingShellConvert.current = null;
      setFolderInputRoot(shellOpen.path);
      setFolderPreview(null);
      setConvertMode("folder");
    } else {
      selectInput(shellOpen.path);
      setConvertMode("file");
      if (shellOpen.convert_to) {
        applyDefaultPlanConstraints(shellOpen.convert_to);
        setTarget(shellOpen.convert_to);
        setOutputPath(suggestedOutput(shellOpen.path, shellOpen.convert_to));
        pendingShellConvert.current = shellOpen.convert_to;
      } else {
        pendingShellConvert.current = null;
      }
    }
    setTab("convert");
  }

  async function classifyAndSelectDrop(path: string) {
    try {
      const classified = await invoke<DropClassification>("classify_desktop_drop_path", { path });
      if (classified.kind === "directory" && classified.path) {
        pendingShellConvert.current = null;
        setConvertMode("folder");
        setFolderInputRoot(classified.path);
        setFolderPreview(null);
        setError(null);
        return;
      }
      if (classified.kind === "file" && classified.path) {
        selectInput(classified.path);
        setConvertMode("file");
      }
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function loadStarterProbes() {
    if (!("__TAURI_INTERNALS__" in window)) return;
    try {
      const [pdf, video] = await Promise.all([
        invoke<CapabilitySnapshot>("desktop_capability_snapshot", {
          inputPath: "C:\\formatwright-probe.pdf",
        }),
        invoke<CapabilitySnapshot>("desktop_capability_snapshot", {
          inputPath: "C:\\formatwright-probe.mkv",
        }),
      ]);
      if (!mounted.current) return;
      setProbePdfRoutes(pdf.routes);
      setProbeVideoRoutes(video.routes);
    } catch (reason) {
      if (mounted.current) setError(parseDesktopError(reason));
    }
  }

  function changeTarget(next: string) {
    pendingShellConvert.current = null;
    setTarget(next);
    setOutputPath(suggestedOutput(inputPath, next));
    setPreview(null);
    setFolderPreview(null);
  }

  async function chooseInput() {
    try {
      const selected = await open({ directory: false, multiple: false, title: copy.chooseFile });
      if (typeof selected === "string") selectInput(selected);
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function activateEmptyStateCard(cardId: EmptyStateCardId) {
    const spec = EMPTY_STATE_CARDS.find((card) => card.id === cardId);
    if (!spec) return;
    const routes = cardId === "pdf-png" ? probePdfRoutes : cardId === "video-mp4" ? probeVideoRoutes : null;
    const availability = emptyStateCardAvailability(cardId, routes);
    if (!availability.available) {
      setTab("engines");
      return;
    }
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: copy.chooseFile,
        filters: [spec.filters],
      });
      if (typeof selected !== "string") return;
      selectInput(selected);
      setConvertMode("file");
      applyDefaultPlanConstraints(spec.target);
      setTarget(spec.target);
      setOutputPath(suggestedOutput(selected, spec.target));
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function chooseOutput() {
    try {
      const selected = isDirectoryOutput(inputPath, target)
        ? await open({ directory: true, multiple: false, title: copy.chooseOutput })
        : await save({ defaultPath: outputPath || undefined, title: copy.chooseOutput });
      if (typeof selected === "string") {
        pendingShellConvert.current = null;
        setOutputPath(selected);
        setPreview(null);
      }
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function chooseFolderRoot(kind: "input" | "output") {
    try {
      const selected = await open({
        directory: true,
        multiple: false,
        title: kind === "input" ? copy.chooseInputFolder : copy.chooseOutputFolder,
      });
      if (typeof selected !== "string") return;
      if (kind === "input") setFolderInputRoot(selected);
      else setFolderOutputRoot(selected);
      setFolderPreview(null);
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function previewFolderBatch() {
    setFolderBusy("preview");
    setFolderPreview(null);
    setError(null);
    try {
      setFolderPreview(await invoke<FolderPreview>("preview_desktop_folder_batch", {
        request: {
          inputRoot: folderInputRoot,
          outputRoot: folderOutputRoot,
          targetFormat: target,
          quality: target === "png" || !quality ? null : Number(quality),
          width: width ? Number(width) : null,
          dpi: target === "png" || target === "jpg" || target === "jpeg" ? Number(dpi) : null,
          colorMode: target === "png" || target === "jpg" || target === "jpeg" ? colorMode : null,
          preserveAllStreams,
        },
      }));
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setFolderBusy(null);
    }
  }

  async function queueFolderBatch() {
    if (!folderPreview) return;
    setFolderBusy("queue");
    setError(null);
    try {
      const result = await invoke<FolderQueueResult>("queue_desktop_folder_batch", {
        previewId: folderPreview.preview_id,
        batchName: null,
      });
      setFolderPreview(null);
      setJobBatchId(result.batch.id);
      setTab("jobs");
      await Promise.all([refreshJobs(0), refreshJobBatches()]);
    } catch (reason) {
      setFolderPreview(null);
      setError(parseDesktopError(reason));
    } finally {
      setFolderBusy(null);
    }
  }

  function request(
    approvedPlanHash: string | null = null,
    idempotencyKey: string | null = null,
  ) {
    return {
      inputPath,
      outputPath,
      targetFormat: target,
      quality: target === "png" || !quality ? null : Number(quality),
      width: width ? Number(width) : null,
      dpi: (target === "png" || target === "jpg" || target === "jpeg") && dpi ? Number(dpi) : null,
      colorMode: (target === "png" || target === "jpg" || target === "jpeg") && colorMode ? colorMode : null,
      preserveAllStreams,
      approvedPlanHash,
      idempotencyKey,
    };
  }

  async function previewPlan() {
    pendingShellConvert.current = null;
    setBusy("plan");
    setError(null);
    setReport(null);
    try {
      setPreview(await invoke<Preview>("preview_conversion", { request: request() }));
    } catch (reason) {
      setPreview(null);
      setError(parseDesktopError(reason));
    } finally {
      setBusy(null);
    }
  }

  async function runConversion() {
    if (!preview) return;
    setBusy("run");
    setJobProgress({});
    setProgressClock(Date.now());
    setError(null);
    try {
      const result = await invoke<{ job: JobRecord; report: ValidationReport }>(
        "run_desktop_conversion",
        { request: request(preview.plan.plan_hash) },
      );
      setReport(result.report);
      setActiveJobId(null);
      await notifyToast(copy.toastSuccess, result.report.output.display_path ?? outputPath);
      await refreshJobs();
    } catch (reason) {
      const parsed = parseDesktopError(reason);
      setPreview(null);
      setError({
        ...parsed,
        message: basicModeFailureCopy(inputPath, parsed, [], {
          oldExcel: copy.oldExcel,
          unsupported: copy.pairUnsupported,
          engineMissing: copy.engineMissingPack,
          outputConflict: copy.outputExists,
          policyBlocked: copy.policyBlocked,
        }),
      });
      setActiveJobId(null);
      await notifyToast(copy.toastFailed, parsed.message);
      await refreshJobs();
    } finally {
      setBusy(null);
    }
  }

  async function notifyToast(title: string, body: string) {
    try {
      await invoke("show_desktop_toast", { title, body });
    } catch {
      // Toast is best-effort; conversion result still shows in the window.
    }
  }

  async function handleIngestBatch(batch: ShellConvertBatch) {
    pendingShellConvert.current = null;
    const first = batch.paths[0] ?? "";
    applyDefaultPlanConstraints(batch.target);
    if (first) {
      setInputPath(first);
      setTarget(batch.target);
      setOutputPath(suggestedOutput(first, batch.target));
      setConvertMode("file");
      setTab("convert");
    }
    setBusy("run");
    setError(null);
    try {
      const result = await invoke<IngestResult>("ingest_shell_convert_paths", {
        paths: batch.paths,
        target: batch.target,
      });
      if (result.ran_immediately && result.report) {
        setReport(result.report);
        setActiveJobId(null);
        await notifyToast(copy.toastSuccess, result.report.output.display_path ?? first);
      } else {
        if (result.batch_id) {
          setJobBatchId(result.batch_id);
          setTab("jobs");
        }
        const summary = [
          copy.queuedCount.replace("{count}", String(result.queued)),
          result.skipped_conflict ? copy.skippedConflictCount.replace("{count}", String(result.skipped_conflict)) : "",
        ].filter(Boolean).join(" · ");
        await notifyToast(result.queued > 0 ? copy.toastQueued : copy.toastFailed, summary);
      }
      await refreshJobs();
      await refreshJobBatches();
    } catch (reason) {
      const parsed = parseDesktopError(reason);
      setError({
        ...parsed,
        message: basicModeFailureCopy(first, parsed, [], {
          oldExcel: copy.oldExcel,
          unsupported: copy.pairUnsupported,
          engineMissing: copy.engineMissingPack,
          outputConflict: copy.outputExists,
          policyBlocked: copy.policyBlocked,
        }),
      });
      await notifyToast(copy.toastFailed, parsed.message);
    } finally {
      setBusy(null);
    }
  }

  async function cancel() {
    if (activeJobId) await invoke("cancel_desktop_job", { jobId: activeJobId });
  }

  async function queueConversion() {
    if (!preview) return;
    setBusy("queue");
    setError(null);
    const intent = JSON.stringify([inputPath, outputPath, preview.plan.plan_hash]);
    const submission =
      queueSubmission.current?.intent === intent
        ? queueSubmission.current
        : { intent, key: crypto.randomUUID() };
    queueSubmission.current = submission;
    try {
      await invoke<JobRecord>("queue_desktop_conversion", {
        request: request(preview.plan.plan_hash, submission.key),
      });
      queueSubmission.current = null;
      setTab("jobs");
      await refreshJobs();
    } catch (reason) {
      setError(parseDesktopError(reason));
      await refreshJobs();
    } finally {
      setBusy(null);
    }
  }

  async function runQueueWindow() {
    setBusy("queue-run");
    setJobProgress({});
    setProgressClock(Date.now());
    setError(null);
    try {
      const report = await invoke<QueueRunReport>("run_desktop_queue_window", {
        limit: 100,
        parallel: 4,
      });
      setQueueReport(report);
      await refreshJobs();
    } catch (reason) {
      setError(parseDesktopError(reason));
      await refreshJobs();
    } finally {
      setBusy(null);
      setProgressClock(Date.now());
    }
  }

  async function pauseQueueFinishCurrent() {
    try {
      await invoke<boolean>("pause_desktop_queue_window", { mode: "finish-current" });
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function stopQueueWindow() {
    try {
      await invoke<boolean>("pause_desktop_queue_window", { mode: "immediate" });
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function refreshJobs(nextOffset = jobQuery.current.offset) {
    const sequence = ++jobRefreshSequence.current;
    const query = jobQuery.current;
    try {
      const page = await invoke<JobQueryPage>("query_desktop_jobs", {
        batchId: query.batchId || null,
        states: query.state ? [query.state] : [],
        search: query.search.trim() || null,
        limit: JOB_PAGE_SIZE,
        offset: nextOffset,
      });
      if (sequence !== jobRefreshSequence.current || !mounted.current) return;
      setJobs(page.jobs);
      setJobTotal(page.total);
      setJobOffset(page.offset);
      void refreshRecovery();
    } catch {
      // Browser-only development and first setup can render without an IPC backend.
    }
  }

  async function refreshJobBatches() {
    try {
      const batches = await invoke<BatchRecord[]>("list_desktop_batches", {
        limit: 500,
        offset: 0,
      });
      if (mounted.current) setJobBatches(batches);
    } catch {
      // Browser-only development and first setup can render without an IPC backend.
    }
  }

  async function refreshRecovery() {
    try {
      const summary = await invoke<RecoverySummary>("get_desktop_recovery_summary");
      if (mounted.current) setRecovery(summary);
    } catch {
      // Browser-only development and first setup can render without an IPC backend.
    }
  }

  async function refreshMaintenanceStatus() {
    if (!("__TAURI_INTERNALS__" in window)) return;
    try {
      const status = await invoke<MaintenanceStatus>("get_desktop_maintenance_status");
      if (mounted.current) setMaintenanceStatus(status);
    } catch (reason) {
      if (mounted.current) setError(parseDesktopError(reason));
    }
  }

  async function checkIntegrity() {
    setMaintenanceBusy("check");
    setMaintenanceResult(null);
    setError(null);
    try {
      const result = await invoke<IntegrityReport>("check_desktop_integrity");
      setMaintenanceResult(result.ok ? copy.integrityPassed : copy.integrityFailed);
      await refreshMaintenanceStatus();
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setMaintenanceBusy(null);
    }
  }

  async function backupState() {
    setMaintenanceBusy("backup");
    setMaintenanceResult(null);
    setError(null);
    try {
      const selected = await save({
        defaultPath: `formatwright-state-${new Date().toISOString().slice(0, 10)}.fwstate`,
        title: copy.backupState,
        filters: [{ name: "FormatWright state bundle", extensions: ["fwstate"] }],
      });
      if (typeof selected !== "string") return;
      const result = await invoke<StateBundleBackupReport>("backup_desktop_state", {
        destinationPath: selected,
        includeReports,
      });
      setMaintenanceResult(`${copy.backupCreated}: ${result.bundle_path} · ${formatBytes(result.size_bytes)} · ${result.entry_count} ${copy.entries}`);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setMaintenanceBusy(null);
    }
  }

  async function compactDatabase() {
    setMaintenanceBusy("compact");
    setMaintenanceResult(null);
    setError(null);
    try {
      const result = await invoke<CompactReport>("compact_desktop_database");
      setMaintenanceResult(`${copy.compactCompleted}: ${formatBytes(result.reclaimed_bytes)} ${copy.reclaimed}`);
      await Promise.all([refreshMaintenanceStatus(), refreshJobs()]);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setMaintenanceBusy(null);
    }
  }

  async function preflightRestore() {
    setMaintenanceBusy("preflight");
    setMaintenanceResult(null);
    setRestorePreflight(null);
    setError(null);
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: copy.restoreState,
        filters: [{ name: "FormatWright state bundle", extensions: ["fwstate"] }],
      });
      if (typeof selected !== "string") return;
      const result = await invoke<StateBundlePreflightReport>("preflight_desktop_state_restore", {
        bundlePath: selected,
      });
      setRestorePreflight(result);
      setMaintenanceResult(copy.restorePreflightPassed);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setMaintenanceBusy(null);
    }
  }

  async function scheduleRestore() {
    if (!restorePreflight) return;
    setMaintenanceBusy("restore");
    setMaintenanceResult(null);
    setError(null);
    try {
      await invoke("schedule_desktop_state_restore", {
        bundlePath: restorePreflight.bundle_path,
        expectedBundleId: restorePreflight.bundle_id,
      });
      setRestorePreflight(null);
      setMaintenanceResult(copy.restoreScheduled);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setMaintenanceBusy(null);
    }
  }

  async function requeueJob(job: JobRecord) {
    setJobActionBusy(job.id);
    setPendingCleanupId(null);
    setJobActionNotice(null);
    setError(null);
    try {
      await invoke<JobRecord>("requeue_desktop_job", { jobId: job.id });
      setJobProgress((current) => {
        const next = { ...current };
        delete next[job.id];
        return next;
      });
      setQueueReport(null);
      await refreshJobs();
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setJobActionBusy(null);
    }
  }

  async function cleanupJobStaging(job: JobRecord) {
    if (pendingCleanupId !== job.id) {
      setPendingCleanupId(job.id);
      setJobActionNotice(copy.cleanupConfirmationHint);
      return;
    }
    setJobCleanupBusy(job.id);
    setJobActionNotice(null);
    setError(null);
    try {
      const result = await invoke<StagedCleanupReport>("cleanup_desktop_job_staging", {
        jobId: job.id,
      });
      setJobActionNotice(result.removed ? copy.stagingCleaned : copy.noStagingFound);
      await refreshJobs();
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setJobCleanupBusy(null);
      setPendingCleanupId(null);
    }
  }

  async function runBulkAction(
    action: "cancel" | "resume" | "retry",
    states: string[],
  ) {
    setBulkBusy(true);
    setBulkReport(null);
    setError(null);
    try {
      const selection = await invoke<SelectionSnapshot>("capture_desktop_job_selection", {
        batchId: jobBatchId || null,
        states,
        search: jobSearch.trim() || null,
      });
      const result = await invoke<BulkActionReport>("run_desktop_bulk_action", {
        selectionId: selection.id,
        action,
      });
      setBulkReport(result);
      setQueueReport(null);
      setJobProgress({});
      await refreshJobs();
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setBulkBusy(false);
    }
  }

  async function refreshPresets() {
    try {
      setPresets(await invoke<ConversionPreset[]>("list_desktop_presets"));
    } catch {
      // Browser-only development can render the empty preset library.
    }
  }

  function applyPreset(preset: ConversionPreset, destination: Tab = "convert") {
    setTarget(preset.target_format);
    setQuality(preset.quality == null ? "" : String(preset.quality));
    setWidth(preset.width == null ? "" : String(preset.width));
    setDpi(preset.dpi == null ? "144" : String(preset.dpi));
    setColorMode(preset.color_mode ?? "rgb");
    setPreserveAllStreams(preset.preserve_all_streams);
    setOutputPath(suggestedOutput(inputPath, preset.target_format));
    setPreview(null);
    setReport(null);
    setTab(destination);
  }

  function editPreset(preset: ConversionPreset) {
    applyPreset(preset, "presets");
    setPresetName(preset.name);
    setEditingPresetId(preset.preset_id);
    setPendingDeleteId(null);
    setPresetNotice(null);
  }

  function resetPresetEditor() {
    setPresetName("");
    setEditingPresetId(null);
    setPendingDeleteId(null);
    setPresetNotice(null);
  }

  async function savePreset() {
    setPresetBusy(true);
    setPresetNotice(null);
    setError(null);
    try {
      await invoke<ConversionPreset>("save_desktop_preset", {
        request: {
          presetId: editingPresetId,
          name: presetName,
          targetFormat: target,
          quality: target === "png" || !quality ? null : Number(quality),
          width: width ? Number(width) : null,
          dpi: dpi ? Number(dpi) : null,
          colorMode: colorMode || null,
          preserveAllStreams,
        },
      });
      await refreshPresets();
      setPresetNotice(copy.presetSaved);
      setEditingPresetId(null);
      setPresetName("");
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setPresetBusy(false);
    }
  }

  async function deletePreset(presetId: string) {
    if (pendingDeleteId !== presetId) {
      setPendingDeleteId(presetId);
      return;
    }
    setPresetBusy(true);
    setError(null);
    try {
      await invoke<boolean>("delete_desktop_preset", { presetId });
      await refreshPresets();
      resetPresetEditor();
      setPresetNotice(copy.presetDeleted);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setPresetBusy(false);
    }
  }

  async function importPresets() {
    setPresetBusy(true);
    setError(null);
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: copy.importPresets,
        filters: [{ name: "FormatWright preset library", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      const result = await invoke<PresetImportResult>("import_desktop_presets", {
        sourcePath: selected,
      });
      await refreshPresets();
      setPresetNotice(`${copy.importedPresets}: ${result.imported} / ${result.total}`);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setPresetBusy(false);
    }
  }

  async function exportPresets() {
    setPresetBusy(true);
    setError(null);
    try {
      const selected = await save({
        defaultPath: "formatwright-presets.json",
        title: copy.exportPresets,
        filters: [{ name: "FormatWright preset library", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      const count = await invoke<number>("export_desktop_presets", {
        destinationPath: selected,
      });
      setPresetNotice(`${copy.exportedPresets}: ${count}`);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setPresetBusy(false);
    }
  }

  async function loadReport(jobId: string) {
    setError(null);
    setReportNotice(null);
    try {
      setReport(await invoke<ValidationReport | null>("get_desktop_report", { jobId }));
      setTab("reports");
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function exportReport() {
    if (!report) return;
    setError(null);
    setReportNotice(null);
    setReportBusy("report");
    try {
      const selected = await save({
        defaultPath: `formatwright-report-${report.job_id}.json`,
        title: copy.exportReport,
        filters: [{ name: "FormatWright ValidationReport", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      await invoke<number>("export_desktop_report", {
        jobId: report.job_id,
        destinationPath: selected,
        redactPaths: redactReportPaths,
      });
      setReportNotice(copy.reportExported);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setReportBusy(null);
    }
  }

  async function exportRecipe() {
    if (!report) return;
    setError(null);
    setReportNotice(null);
    setReportBusy("recipe");
    try {
      const selected = await save({
        defaultPath: `formatwright-recipe-${report.job_id}.json`,
        title: copy.exportRecipe,
        filters: [{ name: "FormatWright job recipe", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      await invoke<number>("export_desktop_recipe", {
        jobId: report.job_id,
        destinationPath: selected,
      });
      setReportNotice(copy.recipeExported);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setReportBusy(null);
    }
  }

  async function revealOutput() {
    if (!report) return;
    setError(null);
    setReportNotice(null);
    setReportBusy("reveal");
    try {
      await invoke("reveal_desktop_job_output", { jobId: report.job_id });
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setReportBusy(null);
    }
  }

  async function revalidateJob() {
    if (!report) return;
    setError(null);
    setReportNotice(null);
    setReportBusy("revalidate");
    try {
      setReport(await invoke<ValidationReport>("revalidate_desktop_job", { jobId: report.job_id }));
      await refreshJobs();
      setReportNotice(copy.revalidatedReport);
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setReportBusy(null);
    }
  }

  async function refreshEngines() {
    if (!("__TAURI_INTERNALS__" in window)) return;
    setError(null);
    try {
      const [nextDoctor, nextPacks] = await Promise.all([
        invoke<DoctorReport>("desktop_doctor"),
        invoke<EnginePackSummary[]>("list_imported_engine_packs"),
      ]);
      setDoctor(nextDoctor);
      setEnginePacks(nextPacks);
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  async function importEnginePack() {
    setError(null);
    try {
      const selected = await open({
        directory: false,
        multiple: false,
        title: copy.importEnginePack,
        filters: [{ name: "FormatWright engine manifest", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      setEngineBusy(true);
      await invoke<EnginePackSummary>("import_desktop_engine_pack", {
        manifestPath: selected,
      });
      await refreshEngines();
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setEngineBusy(false);
    }
  }

  async function runBenchmark() {
    projection.reset();
    setQueueSnapshot(emptySnapshot);
    setBenchmark(null);
    setError(null);
    try {
      setBenchmark(
        await invoke<BenchmarkResult>("run_queue_bridge_benchmark", { jobCount: 10_000 }),
      );
    } catch (reason) {
      setError(parseDesktopError(reason));
    }
  }

  const recommendations = recommendedTargets(inputPath);
  const normalizedTarget = target === "jpeg" ? "jpg" : target === "yml" ? "yaml" : target;
  const route = capabilities?.routes[normalizedTarget];
  const routeAvailable = !capabilities || route?.available === true;
  const unavailableLabels = { missing: copy.unavailable, unsupported: copy.unsupported };
  const targetOptions = targetOptionViews(recommendations, capabilities ? capabilities.routes : null, convertMode === "file" ? "convert-file" : "convert-folder", unavailableLabels);
  const presetTargetOptions = targetOptionViews(recommendations, capabilities ? capabilities.routes : null, "preset", unavailableLabels);
  const applyPresetField = (field: Parameters<typeof presetFieldChangeInvalidatesPreview>[0]) => {
    if (presetFieldChangeInvalidatesPreview(field, target)) setPreview(null);
  };
  const tabs: Tab[] = ["convert", "jobs", "presets", "engines", "reports", "maintenance", "settings"];
  const recoveryCounts = Object.fromEntries(
    (recovery?.state_counts ?? []).map((entry) => [entry.state, entry.count]),
  );
  const recoverableCount = ["blocked", "interrupted", "failed", "cancelled"].reduce(
    (total, state) => total + (recoveryCounts[state] ?? 0),
    0,
  );
  const engineNotices = engineRecoveryNotices(recovery, {
    engineFallbackNotice: (engine, version) => copy.engineFallbackNotice.replace("{engine}", engine).replace("{version}", version),
    engineFailedNotice: (engine, reason) => copy.engineFailedNotice.replace("{engine}", engine).replace("{reason}", reason),
  });
  const showRecovery = !recoveryDismissed && recovery != null &&
    (recovery.recovered_after_restart > 0 || recovery.restored_bundle_id != null || recovery.restore_error != null || recoverableCount > 0 || engineNotices.length > 0);
  const pageStart = jobTotal === 0 ? 0 : jobOffset + 1;
  const pageEnd = Math.min(jobOffset + jobs.length, jobTotal);
  const activeProgress = activeJobId ? jobProgress[activeJobId] : undefined;
  const windowChrome = getCurrentWebviewWindow();

  return (
    <div className="c95-desktop fw-app">
      <a className="skip-link" href="#main-content">{copy.skipToContent}</a>
      <article className="c95-window fw-main-window">
        <header className="c95-window__titlebar">
          <span className="c95-window__title" data-tauri-drag-region>
            <svg className="c95-window__title-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
              <path d="M14 3v4a1 1 0 0 0 1 1h4" />
              <path d="M17 21h-10a2 2 0 0 1 -2 -2v-14a2 2 0 0 1 2 -2h7l5 5v11a2 2 0 0 1 -2 2z" />
            </svg>
            {copy.product} — {copy[tab]}
          </span>
          <span className="c95-window__controls">
            <button type="button" className="c95-window__ctrl" aria-label="Minimize" onClick={() => void windowChrome.minimize()}>_</button>
            <button type="button" className="c95-window__ctrl" aria-label="Maximize" onClick={() => void windowChrome.toggleMaximize()}>□</button>
            <button type="button" className="c95-window__ctrl" aria-label="Close" onClick={() => void windowChrome.close()}>×</button>
          </span>
        </header>
        <div className="c95-tabs fw-tabs">
          <div className="c95-tabs__strip" role="tablist" aria-label={copy.primaryNavigation}>
            {tabs.map((item) => (
              <button
                key={item}
                type="button"
                role="tab"
                className="c95-tabs__tab"
                aria-selected={tab === item}
                onClick={() => setTab(item)}
              >
                {copy[item]}
              </button>
            ))}
          </div>
          <div className="c95-tabs__panel c95-scroll fw-tabs-panel" id="main-content" tabIndex={-1} role="tabpanel">
      {error && (() => {
        const localized = localizeDesktopError(error, copy);
        return (
          <section className="error-banner" role="alert">
            <strong>{localized.title}</strong>
            <span>{localized.message}</span>
            {localized.recovery && <small>{localized.recovery}</small>}
          </section>
        );
      })()}

      {showRecovery && (
        <section className="recovery-banner" role="status" aria-live="polite">
          <div>
            <strong>{copy.recoveryTitle}</strong>
            <span>{copy.recoveryBody}: {recovery.recovered_after_restart} {copy.recoveredOnStartup} · {recovery.removed_staged_outputs} {copy.partialsCleaned} · {recoverableCount} {copy.recoverableJobs}</span>
            {recovery.restored_bundle_id && <span>{copy.restoreCompleted}: {recovery.restored_bundle_id}</span>}
            {recovery.restore_error && <span className="restore-error">{copy.restoreFailed}: {recovery.restore_error}</span>}
            {engineNotices.length > 0 && <ul className="engine-recovery-list">{engineNotices.map((notice) => <li key={notice}>{notice}</li>)}</ul>}
          </div>
          <span className="heading-actions">
            <button className="primary" type="button" onClick={() => setTab("jobs")}>{copy.reviewRecovery}</button>
            <button type="button" onClick={() => setRecoveryDismissed(true)}>{copy.dismiss}</button>
          </span>
        </section>
      )}

      {tab === "convert" && (
        <section className="workspace">
          <div className="workspace-main">
            <div className="convert-mode" role="group" aria-label={copy.convertMode}><button type="button" className={convertMode === "file" ? "selected" : ""} aria-pressed={convertMode === "file"} onClick={() => setConvertMode("file")}>{copy.singleFile}</button><button type="button" className={convertMode === "folder" ? "selected" : ""} aria-pressed={convertMode === "folder"} onClick={() => setConvertMode("folder")}>{copy.folderBatch}</button></div>
            {convertMode === "file" && <div className={`drop-zone ${dragging ? "is-dragging" : ""}`}>
              <span className="drop-icon" aria-hidden="true">↓</span>
              <div><h1>{copy.dropTitle}</h1><p>{copy.dropBody}</p></div>
              <button className="secondary choose-file" type="button" onClick={chooseInput}>{copy.chooseFile}</button>
            </div>}
            {convertMode === "file" && !inputPath && (
              <div className="empty-cards" aria-label={copy.recommended}>
                {EMPTY_STATE_CARDS.map((card) => {
                  const routes = card.id === "pdf-png" ? probePdfRoutes : card.id === "video-mp4" ? probeVideoRoutes : null;
                  const availability = emptyStateCardAvailability(card.id, routes);
                  const title = card.id === "pdf-png" ? copy.emptyCardPdf : card.id === "json-yaml" ? copy.emptyCardJson : copy.emptyCardVideo;
                  const body = card.id === "pdf-png" ? copy.emptyCardPdfBody : card.id === "json-yaml" ? copy.emptyCardJsonBody : copy.emptyCardVideoBody;
                  return (
                    <button
                      key={card.id}
                      type="button"
                      className={`empty-card ${availability.available ? "" : "is-unavailable"}`}
                      onClick={() => void activateEmptyStateCard(card.id)}
                    >
                      <strong>{title}</strong>
                      <span>{body}</span>
                      {!availability.available && (
                        <small>
                          {availability.missingEngines.length > 0
                            ? copy.emptyCardMissing.replace("{names}", availability.missingEngines.join(", "))
                            : copy.emptyCardUnavailable}
                        </small>
                      )}
                    </button>
                  );
                })}
              </div>
            )}
            {convertMode === "folder" && <section className="folder-intro"><p className="section-label">BOUNDED BATCH</p><h1>{copy.folderBatch}</h1><p>{copy.folderBatchHint}</p></section>}

            <div className="form-grid">
              {convertMode === "file" && <div className="form-field wide"><label htmlFor="input-path">{copy.inputPath}</label><span className="path-control"><input id="input-path" dir="auto" spellCheck={false} value={inputPath} onChange={(event) => selectInput(event.target.value)} placeholder="C:\\…\\input.ext" /><button className="secondary" type="button" onClick={chooseInput}>{copy.chooseFile}</button></span></div>}
              {convertMode === "file" && <div className="form-field wide"><label htmlFor="output-path">{copy.outputPath}</label><span className="path-control"><input id="output-path" dir="auto" spellCheck={false} value={outputPath} onChange={(event) => { pendingShellConvert.current = null; setOutputPath(event.target.value); setPreview(null); }} placeholder="C:\\…\\output.ext" /><button className="secondary" type="button" disabled={!inputPath} onClick={chooseOutput}>{copy.chooseOutput}</button></span></div>}
              {convertMode === "folder" && <div className="form-field wide"><label htmlFor="input-folder">{copy.inputFolder}</label><span className="path-control"><input id="input-folder" dir="auto" spellCheck={false} value={folderInputRoot} onChange={(event) => { setFolderInputRoot(event.target.value); setFolderPreview(null); }} placeholder="C:\\…\\source-folder" /><button className="secondary" type="button" onClick={() => chooseFolderRoot("input")}>{copy.chooseInputFolder}</button></span></div>}
              {convertMode === "folder" && <div className="form-field wide"><label htmlFor="output-folder">{copy.outputFolder}</label><span className="path-control"><input id="output-folder" dir="auto" spellCheck={false} value={folderOutputRoot} onChange={(event) => { setFolderOutputRoot(event.target.value); setFolderPreview(null); }} placeholder="C:\\…\\output-folder" /><button className="secondary" type="button" onClick={() => chooseFolderRoot("output")}>{copy.chooseOutputFolder}</button></span></div>}
              <label>{copy.target}<select value={target} onChange={(event) => changeTarget(event.target.value)} disabled={convertMode === "file" && capabilityBusy}>
                {targetOptions.map((option) => <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>)}
              </select></label>
              {qualityFieldApplies(target) && <label>{copy.quality}<input type="number" min="1" max="100" value={quality} onChange={(event) => { setQuality(event.target.value); setPreview(null); }} /></label>}
              {expert && <label>{copy.width}<input type="number" min="1" max="16384" value={width} onChange={(event) => { setWidth(event.target.value); setPreview(null); }} /></label>}
              {expert && <label>{copy.dpi}<input type="number" min="36" max="600" value={dpi} onChange={(event) => { setDpi(event.target.value); setPreview(null); }} /></label>}
              {expert && <label>{copy.colorMode}<select value={colorMode} onChange={(event) => { setColorMode(event.target.value); setPreview(null); }}><option value="rgb">RGB</option><option value="gray">Gray</option></select></label>}
              {expert && <label className="checkbox-control"><input type="checkbox" checked={preserveAllStreams} onChange={(event) => { setPreserveAllStreams(event.target.checked); setPreview(null); }} />{copy.preserveAllStreams}</label>}
            </div>

            {inputPath && (capabilityBusy ? <p className="capability-notice" role="status">{copy.capabilityLoading}</p> : route && !route.available ? <p className="capability-notice capability-blocked" role="status"><strong>{copy.routeUnavailable}</strong> {basicModeFailureCopy(inputPath, { code: route.missing_engines.length > 0 ? "ENGINE_MISSING" : "UNSUPPORTED", message: "" }, route.missing_engines, { oldExcel: copy.oldExcel, unsupported: copy.pairUnsupported, engineMissing: copy.engineMissingPack, outputConflict: copy.outputExists, policyBlocked: copy.policyBlocked })}</p> : capabilities && !Object.values(capabilities.routes).some((candidate) => candidate.available) ? <p className="capability-notice capability-blocked" role="status">{["xls", "xlsm", "xlsb"].includes(inputPath.split(/[\\/]/).pop()?.split(".").pop()?.toLowerCase() ?? "") ? copy.oldExcel : inputHasRunnableFamily(capabilities.routes) ? copy.noAvailableTargets : copy.inputNotSupported}</p> : null)}

            <div className="preset-row" aria-label="Presets">
              <button type="button" disabled={capabilities ? !capabilities.routes.webp?.available : false} onClick={() => { changeTarget("webp"); setQuality("78"); }}>{copy.presetImage}</button>
              <button type="button" disabled={capabilities ? !capabilities.routes.png?.available : false} onClick={() => changeTarget("png")}>{copy.presetArchive}</button>
              <button type="button" disabled={capabilities ? !capabilities.routes.pdf?.available : false} onClick={() => changeTarget("pdf")}>{copy.presetPdf}</button>
              {presets.map((preset) => <button type="button" key={preset.preset_id} disabled={capabilities ? !capabilities.routes[preset.target_format]?.available : false} onClick={() => applyPreset(preset)}>{preset.name}</button>)}
              <button type="button" onClick={() => setTab("presets")}>+ {copy.savePreset}</button>
            </div>

            <div className="action-row">
              {convertMode === "file" && <button className="secondary" type="button" disabled={!inputPath || !outputPath || busy !== null || capabilityBusy || !routeAvailable} onClick={previewPlan}>{busy === "plan" ? copy.planning : copy.inspectPlan}</button>}
              {convertMode === "file" && <button className="primary" type="button" disabled={!preview || busy !== null || !routeAvailable} onClick={runConversion}>{busy === "run" ? copy.running : copy.run}</button>}
              {convertMode === "file" && <button className="secondary" type="button" disabled={!preview || busy !== null || !routeAvailable} onClick={queueConversion}>{busy === "queue" ? copy.queueing : copy.queueOnly}</button>}
              {convertMode === "file" && busy === "run" && <button className="danger" type="button" onClick={cancel}>{copy.cancel}</button>}
              {convertMode === "folder" && <button className="secondary" type="button" disabled={!folderInputRoot || !folderOutputRoot || folderBusy !== null} onClick={previewFolderBatch}>{folderBusy === "preview" ? copy.planningFolder : copy.previewFolderMapping}</button>}
              {convertMode === "folder" && <button className="primary" type="button" disabled={!folderPreview || !folderPreview.disk_budget.sufficient || folderBusy !== null} onClick={queueFolderBatch}>{folderBusy === "queue" ? copy.queueingFolder : copy.queueFolderBatch}</button>}
            </div>

            {convertMode === "file" && busy === "run" && activeProgress && <p className="execution-progress" role="status"><span>{copy.stageLabel}: {activeProgress.state}</span><span>{copy.elapsedLabel}: {elapsedProgressSeconds(activeProgress, progressClock)}s</span><span>{copy.rateEtaUnavailable}</span></p>}

            {convertMode === "file" && report && (report.status === "pass" || report.status === "warning") && (
              <section className="success-notice convert-success" role="status">
                <div>
                  <strong>{report.status === "warning" ? copy.conversionWarning : copy.conversionComplete}</strong>
                  {pdfPageCountFromReport(report) != null
                    ? <span>{copy.conversionPages.replace("{count}", String(pdfPageCountFromReport(report)))}</span>
                    : report.output.display_path
                      ? <span>{report.output.display_path}</span>
                      : null}
                </div>
                <span className="heading-actions">
                  <button className="primary" type="button" disabled={reportBusy === "reveal"} onClick={() => void revealOutput()}>
                    {reportBusy === "reveal" ? copy.openingOutput : copy.openOutputLocation}
                  </button>
                  <button type="button" onClick={() => setTab("reports")}>{copy.selectJob}</button>
                </span>
              </section>
            )}

            {convertMode === "file" && preview && <PlanView preview={preview} expert={expert} copy={copy} />}
            {convertMode === "folder" && folderPreview && <section className="folder-preview"><div className="plan-heading"><div><p className="section-label">MAPPING PREVIEW</p><h2>{folderPreview.planned.toLocaleString()} {copy.filesReady}</h2></div><span className={`loss ${folderPreview.disk_budget.sufficient ? "loss-safe" : "loss-lossy"}`}>{folderPreview.disk_budget.sufficient ? copy.diskReady : copy.diskInsufficient}</span></div><p>{copy.folderPreviewSummary}: {folderPreview.discovered.toLocaleString()} {copy.discovered} · {folderPreview.planned.toLocaleString()} {copy.planned} · {folderPreview.skipped.toLocaleString()} {copy.skipped} · {copy.diskRequired} {formatBytes(folderPreview.disk_budget.required_bytes)} / {copy.diskAvailable} {formatBytes(folderPreview.disk_budget.available_bytes)}</p><div className="mapping-list">{folderPreview.sample.map((entry) => <div key={entry.input_path}><bdi>{entry.relative_input_path}</bdi><strong>→</strong><bdi>{entry.output_path}</bdi></div>)}</div>{folderPreview.truncated && <p className="typed-note">{copy.mappingTruncated}</p>}<p className="typed-note">{copy.previewExpires}: {new Date(folderPreview.expires_unix_ms).toLocaleTimeString()}</p></section>}
          </div>

          <aside className="side-panel">
            <div className="mode-switch" role="group" aria-label={copy.mode}>
              <button type="button" className={!expert ? "selected" : ""} aria-pressed={!expert} onClick={() => setExpert(false)}>{copy.basic}</button>
              <button type="button" className={expert ? "selected" : ""} aria-pressed={expert} onClick={() => setExpert(true)}>{copy.expert}</button>
            </div>
            <p className="section-label">{copy.recommended}</p>
            <div className="recommendations">{recommendations.map((value, index) => { const candidate = capabilities?.routes[value]; return <button type="button" key={value} aria-pressed={target === value} disabled={capabilities ? !candidate?.available : false} title={candidate?.message} onClick={() => changeTarget(value)}><span>{index + 1}</span>{value.toUpperCase()}{candidate && !candidate.available ? ` · ${copy.unavailable}` : ""}</button>; })}</div>
            <div className="privacy-card"><strong>LOCAL</strong><p>{copy.privacy}</p></div>
          </aside>
        </section>
      )}

      {tab === "jobs" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">SQLITE</p><h1>{copy.jobs}</h1><p>{copy.queueHint}</p></div><div className="heading-actions"><button className="primary" type="button" disabled={busy !== null} onClick={runQueueWindow}>{busy === "queue-run" ? copy.runningQueue : copy.runQueue}</button>{busy === "queue-run" && <button className="secondary" type="button" onClick={pauseQueueFinishCurrent}>{copy.pauseFinishCurrent}</button>}{busy === "queue-run" && <button className="danger" type="button" onClick={stopQueueWindow}>{copy.stopQueue}</button>}<button type="button" onClick={() => void refreshJobs()}>{copy.refresh}</button></div></div>
          {queueReport && (
            <p className="success-notice" role="status" aria-live="polite">
              {copy.queueReport}: selected {queueReport.selected} · completed {queueReport.completed} · warning {queueReport.warning} · blocked {queueReport.blocked} · failed {queueReport.failed} · cancelled {queueReport.cancelled} · contended {queueReport.contended} · peak {queueReport.peak_active}/{queueReport.parallelism}{queueReport.stopped ? " · stopped" : ""}
            </p>
          )}
          <div className="state-summary" aria-label={copy.stateSummary}>
            {jobStateOptions.map((state) => <button type="button" key={state} className={jobStateFilter === state ? "selected" : ""} aria-pressed={jobStateFilter === state} onClick={() => setJobStateFilter(jobStateFilter === state ? "" : state)}>{state} <strong>{recoveryCounts[state] ?? 0}</strong></button>)}
          </div>
          <div className="bulk-toolbar">
            <div className="job-filters">
              <label>{copy.filterJobs}<input value={jobSearch} maxLength={200} onChange={(event) => setJobSearch(event.target.value)} placeholder={copy.filterJobsHint} /></label>
              <label>{copy.stateFilter}<select value={jobStateFilter} onChange={(event) => setJobStateFilter(event.target.value)}><option value="">{copy.allStates}</option>{jobStateOptions.map((state) => <option key={state} value={state}>{state}</option>)}</select></label>
              <label>{copy.batchFilter}<select value={jobBatchId} onChange={(event) => setJobBatchId(event.target.value)}><option value="">{copy.allBatches}</option>{jobBatches.map((batch) => <option key={batch.id} value={batch.id}>{batch.name} ({batch.job_count})</option>)}</select></label>
            </div>
            <div className="heading-actions">
              <button className="primary" type="button" disabled={bulkBusy || busy === "queue-run"} onClick={() => runBulkAction("retry", ["failed", "cancelled", "interrupted"])}>{copy.retryMatching}</button>
              <button type="button" disabled={bulkBusy || busy === "queue-run"} onClick={() => runBulkAction("resume", ["blocked", "interrupted"])}>{copy.resumeMatching}</button>
              <button className="danger" type="button" disabled={bulkBusy || busy === "queue-run"} onClick={() => runBulkAction("cancel", ["planned", "queued", "blocked", "interrupted"])}>{copy.cancelMatching}</button>
            </div>
          </div>
          {bulkReport && <p className="success-notice" role="status" aria-live="polite">{copy.bulkReport}: {bulkReport.transitioned} / {bulkReport.matched} · {copy.skippedState} {bulkReport.skipped_state} · {copy.skippedConflict} {bulkReport.skipped_conflict}</p>}
          {jobActionNotice && <p className={pendingCleanupId ? "typed-note" : "success-notice"} role="status" aria-live="polite">{jobActionNotice}</p>}
          {jobs.length === 0 ? <p className="empty">{copy.historyEmpty}</p> : (
            <div className="job-list" role="list" aria-label={copy.jobListLabel}>
              {jobs.map((job, index) => {
                const liveProgress = progressForJob(jobProgress[job.id], job.sequence);
                const liveState = liveProgress?.state ?? job.state;
                const resumable = liveState === "interrupted" || liveState === "blocked";
                const retryable = liveState === "failed" || liveState === "cancelled";
                const cleanupAllowed = ["blocked", "failed", "cancelled", "interrupted"].includes(liveState);
                const waitReason = liveProgress?.wait_reason
                  ? copy.waitReasons[liveProgress.wait_reason as keyof typeof copy.waitReasons]
                  : null;
                return (
                  <article key={job.id} role="listitem" {...jobListAriaAttributes(jobOffset, index, jobTotal)}>
                    <div>
                      <strong title={job.output_path}><bdi>{job.output_path}</bdi></strong>
                      <small title={job.input_path}><bdi>{job.input_path}</bdi></small>
                      {liveProgress && <span className="job-progress"><span>{copy.stageLabel}: {liveState}</span>{waitReason && <span>{copy.waitingFor}: {waitReason}</span>}<span>{copy.elapsedLabel}: {elapsedProgressSeconds(liveProgress, progressClock)}s</span>{liveProgress.eta_milliseconds == null && <span>{copy.rateEtaUnavailable}</span>}</span>}
                    </div>
                    <span className={`status status-${liveState}`}>{liveState}</span>
                    <span className="job-actions">
                      {(resumable || retryable) && <button className="primary" type="button" aria-label={`${resumable ? copy.resumeJob : copy.retryJob}: ${job.id}`} disabled={jobActionBusy !== null || jobCleanupBusy !== null || busy === "queue-run" || bulkBusy} onClick={() => requeueJob(job)}>{jobActionBusy === job.id ? (resumable ? copy.resumingJob : copy.retryingJob) : (resumable ? copy.resumeJob : copy.retryJob)}</button>}
                      {cleanupAllowed && <button className={pendingCleanupId === job.id ? "danger" : "secondary"} type="button" aria-label={`${pendingCleanupId === job.id ? copy.confirmCleanStaging : copy.cleanStaging}: ${job.id}`} disabled={jobActionBusy !== null || jobCleanupBusy !== null || busy === "queue-run" || bulkBusy} onClick={() => cleanupJobStaging(job)}>{jobCleanupBusy === job.id ? copy.cleaningStaging : pendingCleanupId === job.id ? copy.confirmCleanStaging : copy.cleanStaging}</button>}
                      <button type="button" aria-label={`${copy.selectJob}: ${job.id}`} onClick={() => loadReport(job.id)}>{copy.selectJob}</button>
                    </span>
                  </article>
                );
              })}
            </div>
          )}
          <p className="typed-note">{copy.boundedJobListHint}</p>
          <nav className="job-pagination" aria-label={copy.pagination}>
            <button type="button" disabled={jobOffset === 0} onClick={() => void refreshJobs(Math.max(0, jobOffset - JOB_PAGE_SIZE))}>{copy.previousPage}</button>
            <span>{pageStart.toLocaleString()}–{pageEnd.toLocaleString()} / {jobTotal.toLocaleString()}</span>
            <button type="button" disabled={jobOffset + jobs.length >= jobTotal} onClick={() => void refreshJobs(jobOffset + JOB_PAGE_SIZE)}>{copy.nextPage}</button>
          </nav>
          <details className="benchmark"><summary>{copy.benchmark}</summary><button type="button" onClick={runBenchmark}>{copy.benchmark}</button>{benchmark && <p>{benchmark.total_jobs.toLocaleString()} / {benchmark.emitted_batches} batches / {benchmark.elapsed_milliseconds} ms</p>}<p>{queueSnapshot.totalJobs.toLocaleString()} projected · {queueSnapshot.completed.toLocaleString()} completed</p></details>
        </section>
      )}

      {tab === "presets" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">PORTABLE JSON</p><h1>{copy.presets}</h1><p>{copy.presetsHint}</p></div><div className="heading-actions"><button className="secondary" type="button" disabled={presetBusy} onClick={importPresets}>{copy.importPresets}</button><button type="button" disabled={presetBusy || presets.length === 0} onClick={exportPresets}>{copy.exportPresets}</button></div></div>
          {presetNotice && <p className="success-notice" role="status" aria-live="polite">{presetNotice}</p>}
          <section className="preset-editor" aria-label={copy.presetEditor}>
            <div><p className="section-label">{editingPresetId ? copy.editPreset : copy.newPreset}</p><h2>{editingPresetId ? copy.editPreset : copy.saveCurrentSettings}</h2></div>
            <div className="preset-fields"><label>{copy.presetName}<input maxLength={80} value={presetName} onChange={(event) => setPresetName(event.target.value)} /></label><label>{copy.target}<select value={target} onChange={(event) => changeTarget(event.target.value)}>{presetTargetOptions.map((option) => <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>)}</select></label>{qualityFieldApplies(target) && <label>{copy.quality}<input type="number" min="1" max="100" value={quality} onChange={(event) => { setQuality(event.target.value); applyPresetField("quality"); }} /></label>}<label>{copy.width}<input type="number" min="1" max="16384" value={width} onChange={(event) => { setWidth(event.target.value); applyPresetField("width"); }} /></label><label>{copy.dpi}<input type="number" min="36" max="600" value={dpi} onChange={(event) => { setDpi(event.target.value); applyPresetField("dpi"); }} /></label><label>{copy.colorMode}<select value={colorMode} onChange={(event) => { setColorMode(event.target.value); applyPresetField("color-mode"); }}><option value="rgb">RGB</option><option value="gray">Gray</option></select></label><label className="checkbox-control"><input type="checkbox" checked={preserveAllStreams} onChange={(event) => { setPreserveAllStreams(event.target.checked); applyPresetField("preserve-all-streams"); }} />{copy.preserveAllStreams}</label></div>
            <div className="action-row"><button className="primary" type="button" disabled={presetBusy || presetName.trim().length === 0} onClick={savePreset}>{presetBusy ? copy.savingPreset : copy.savePreset}</button>{editingPresetId && <button className="secondary" type="button" onClick={resetPresetEditor}>{copy.cancelEdit}</button>}</div>
          </section>
          <div className="preset-list">{presets.length === 0 ? <p className="empty">{copy.noPresets}</p> : presets.map((preset) => <article key={preset.preset_id}><div><strong>{preset.name}</strong><small>{preset.target_format.toUpperCase()} · Q {preset.quality ?? "—"} · {preset.width ? `${preset.width}px` : copy.originalSize}</small></div><div className="preset-actions"><button type="button" onClick={() => applyPreset(preset)}>{copy.applyPreset}</button><button type="button" onClick={() => editPreset(preset)}>{copy.editPreset}</button><button className={pendingDeleteId === preset.preset_id ? "danger" : "secondary"} type="button" disabled={presetBusy} onClick={() => deletePreset(preset.preset_id)}>{pendingDeleteId === preset.preset_id ? copy.confirmDelete : copy.deletePreset}</button></div></article>)}</div>
        </section>
      )}

      {tab === "engines" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">LOCAL INVENTORY</p><h1>{copy.doctor}</h1><p>{copy.doctorHint}</p></div><div className="heading-actions"><button className="secondary" type="button" disabled={engineBusy} onClick={importEnginePack}>{engineBusy ? copy.verifyingEnginePack : copy.importEnginePack}</button><button type="button" onClick={refreshEngines}>{copy.refresh}</button></div></div>
          {!doctor ? <p className="empty">{copy.importHint}</p> : <div className="engine-grid">{Object.entries(doctor.engines).map(([name, health]) => <article key={name}><strong>{name}</strong><span className={`status ${health.available ? "status-completed" : "status-failed"}`}>{health.available ? `✓ ${copy.available}` : `× ${copy.unavailable}`}</span><small>{health.identity?.version ?? health.message}</small>{health.identity && <small>{certificationLabel(health.identity.certification, copy)}</small>}</article>)}</div>}
          <div className="pack-section"><p className="section-label">{copy.importedPacks}</p>{enginePacks.length === 0 ? <p className="empty">{copy.noImportedPacks}</p> : <div className="pack-list">{enginePacks.map((pack) => <article key={pack.manifest_sha256 ?? pack.manifest_path}><div><strong>{pack.engine_id ?? copy.invalidPack} {pack.version ?? ""}</strong><small><bdi>{pack.manifest_path}</bdi></small><small>{pack.executable_names.join(", ")}</small><small>{pack.valid ? packReviewText(pack, copy) : pack.message}</small></div><div className="pack-status">{engineRecoveryState(recovery?.engine_recovery, pack.engine_id) === "fell-back" && <span className="status status-warning">{copy.engineRolledBackBadge}</span>}{engineRecoveryState(recovery?.engine_recovery, pack.engine_id) === "failed" && <span className="status status-failed">{copy.engineRecoveryFailedBadge}</span>}<span className={`status ${packBadgeStatusClass(packBadgeKind(pack))}`}>{packBadgeText(pack, copy)}</span></div></article>)}</div>}</div>
        </section>
      )}

      {tab === "reports" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">VALIDATION</p><h1>{copy.report}</h1></div>{report && <div className="heading-actions"><button className="secondary" type="button" disabled={reportBusy !== null} onClick={revealOutput}>{reportBusy === "reveal" ? copy.openingOutput : copy.openOutput}</button><button type="button" disabled={reportBusy !== null} onClick={revalidateJob}>{reportBusy === "revalidate" ? copy.revalidatingReport : copy.revalidateReport}</button><button type="button" disabled={reportBusy !== null} onClick={exportRecipe}>{reportBusy === "recipe" ? copy.exportingRecipe : copy.exportRecipe}</button><button className="primary" type="button" disabled={reportBusy !== null} onClick={exportReport}>{reportBusy === "report" ? copy.exportingReport : copy.exportReport}</button><span className={`report-status report-${report.status}`}>{report.status}</span></div>}</div>
          {report && <label className="checkbox-control report-export-option"><input type="checkbox" checked={redactReportPaths} onChange={(event) => setRedactReportPaths(event.target.checked)} />{copy.redactReportPaths}</label>}
          {reportNotice && <p className="success-notice" role="status" aria-live="polite">{reportNotice}</p>}
          {!report ? <p className="empty">{copy.noReport}</p> : <ReportView report={report} copy={copy} />}
        </section>
      )}

      {tab === "maintenance" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">LONG-TERM STATE</p><h1>{copy.maintenance}</h1><p>{copy.maintenanceHint}</p></div><div className="heading-actions"><button type="button" disabled={maintenanceBusy !== null} onClick={() => void refreshMaintenanceStatus()}>{copy.refresh}</button></div></div>
          {maintenanceResult && <p className="success-notice" role="status" aria-live="polite">{maintenanceResult}</p>}
          <div className="maintenance-grid">
            <article>
              <p className="section-label">SQLITE</p><h2>{copy.databaseHealth}</h2>
              {!maintenanceStatus ? <p>{copy.statusUnavailable}</p> : <dl><div><dt>{copy.databasePath}</dt><dd>{maintenanceStatus.database_path}</dd></div><div><dt>{copy.databaseSize}</dt><dd>{formatBytes(maintenanceStatus.size_bytes)}</dd></div><div><dt>{copy.schemaVersion}</dt><dd>v{maintenanceStatus.schema_version} / v{maintenanceStatus.supported_schema_version}</dd></div><div><dt>{copy.journalMode}</dt><dd>{maintenanceStatus.journal_mode}</dd></div><div><dt>{copy.totalJobs}</dt><dd>{maintenanceStatus.job_count.toLocaleString()}</dd></div><div><dt>{copy.activeJobs}</dt><dd>{maintenanceStatus.active_job_count.toLocaleString()}</dd></div><div><dt>{copy.integrity}</dt><dd className={maintenanceStatus.integrity_ok ? "healthy" : "unhealthy"}>{maintenanceStatus.integrity_ok ? copy.healthy : copy.integrityFailed}</dd></div></dl>}
              <div className="action-row"><button className="primary" type="button" disabled={maintenanceBusy !== null} onClick={checkIntegrity}>{maintenanceBusy === "check" ? copy.checking : copy.checkIntegrity}</button><button type="button" disabled={maintenanceBusy !== null} onClick={compactDatabase}>{maintenanceBusy === "compact" ? copy.compacting : copy.compactDatabase}</button></div>
            </article>
            <article>
              <p className="section-label">VERIFIED BUNDLE</p><h2>{copy.backupAndRestore}</h2><p>{copy.backupHint}</p>
              <label className="checkbox-control"><input type="checkbox" checked={includeReports} onChange={(event) => setIncludeReports(event.target.checked)} />{copy.includeReports}</label>
              <div className="action-row"><button className="primary" type="button" disabled={maintenanceBusy !== null} onClick={backupState}>{maintenanceBusy === "backup" ? copy.backingUp : copy.backupState}</button><button type="button" disabled={maintenanceBusy !== null} onClick={preflightRestore}>{maintenanceBusy === "preflight" ? copy.preflighting : copy.restoreState}</button></div>
              {restorePreflight && <section className="restore-confirm"><strong>{copy.restorePreflightPassed}</strong><span>{restorePreflight.bundle_path}</span><span>{copy.bundleId}: {restorePreflight.bundle_id}</span><span>{restorePreflight.entry_count} {copy.entries} · {formatBytes(restorePreflight.total_uncompressed_bytes)} · DB v{restorePreflight.database.source_schema_version} → v{restorePreflight.database.restored_schema_version}</span>{restorePreflight.warnings.map((warning) => <small key={warning}>{copy.warning}: {warning}</small>)}<p>{copy.restoreRestartWarning}</p><button className="danger" type="button" disabled={maintenanceBusy !== null} onClick={scheduleRestore}>{maintenanceBusy === "restore" ? copy.schedulingRestore : copy.confirmRestoreOnRestart}</button></section>}
            </article>
          </div>
        </section>
      )}

      {tab === "settings" && (
        <section className="page-card settings-grid">
          <div><p className="section-label">PREFERENCES</p><h1>{copy.settings}</h1></div>
          <label>{copy.language}<select value={language} onChange={(event) => setLanguage(event.target.value as Language)}><option value="zh-CN">简体中文</option><option value="en">English</option></select></label>
          <label>{copy.mode}<select value={expert ? "expert" : "basic"} onChange={(event) => setExpert(event.target.value === "expert")}><option value="basic">{copy.basic}</option><option value="expert">{copy.expert}</option></select></label>
          <p>{copy.privacy}</p><p>{copy.accessibility}</p>
        </section>
      )}
          </div>
        </div>
        <footer className="c95-window__statusbar">
          <span className="c95-window__statusbar-cell">{copy.localOnly}</span>
          <span className="c95-window__statusbar-cell">{jobTotal.toLocaleString()} {copy.jobs}</span>
          <span className="c95-window__statusbar-cell" style={{ marginLeft: "auto" }}>{busy ?? "Ready"}</span>
        </footer>
      </article>
    </div>
  );
}

function PlanView({ preview, expert, copy }: { preview: Preview; expert: boolean; copy: (typeof messages)[Language] }) {
  const badge = plainLossSummary(preview.plan);
  const badgeLabel = badge === "lossy" ? copy.lossySummary : badge === "drop-tracks" ? copy.dropTracksSummary : badge === "unknown" ? copy.unknownLossSummary : badge === "container" ? copy.containerSummary : copy.losslessSummary;
  return <section className="plan-card" aria-live="polite"><div className="plan-heading"><div><p className="section-label">{copy.detected}</p><h2>{preview.probe.format.id.toUpperCase()} → {preview.plan.target_format.toUpperCase()}</h2></div><span className={`loss loss-${badge === "lossy" || badge === "drop-tracks" ? "lossy" : "safe"}`}>{badgeLabel}</span></div><div className="change-grid"><ChangeList title={copy.preserved} values={preview.plan.changes.preserved} symbol="✓" /><ChangeList title={copy.changed} values={preview.plan.changes.changed} symbol="△" /><ChangeList title={copy.dropped} values={preview.plan.changes.dropped} symbol="−" /><ChangeList title={copy.unknown} values={preview.plan.changes.unknown} symbol="?" /></div><h3>{copy.engineSteps}</h3><ol className="steps">{preview.plan.steps.map((step) => <li key={step.step_id}><div><strong>{step.engine.engine_id}</strong><small>{step.operation} · {certificationLabel(step.engine.certification, copy)}</small></div><code>{step.capability_id}</code>{expert && <pre>{step.loss_class}{'\n'}{JSON.stringify(step.arguments, null, 2)}</pre>}</li>)}</ol>{expert && <p className="typed-note">{copy.commandBoundary}<br /><code>{preview.plan.plan_hash}</code></p>}</section>;
}

function ChangeList({ title, values, symbol }: { title: string; values: string[]; symbol: string }) {
  return <div><h3>{symbol} {title}</h3>{values.length === 0 ? <span>—</span> : <ul>{values.map((value) => <li key={value}>{value}</li>)}</ul>}</div>;
}

function ReportView({ report, copy }: { report: ValidationReport; copy: (typeof messages)[Language] }) {
  const passed = report.checks.filter((check) => check.required && check.status === "pass").length;
  const required = report.checks.filter((check) => check.required).length;
  return <div className="report-body"><div className="report-summary"><div><span>{copy.requiredChecks}</span><strong>{passed}/{required}</strong></div><div><span>{copy.openPathHint}</span><strong><bdi>{report.output.display_path ?? "—"}</bdi></strong></div></div>{report.engines.length > 0 && <div className="engine-used"><p className="section-label">{copy.usedEngines}</p><ul>{report.engines.map((engine) => <li key={`${engine.engine_id}-${engine.version}`}><strong>{engine.engine_id}</strong> {engine.version} · {certificationLabel(engine.certification, copy)}</li>)}</ul></div>}<div className="check-list">{report.checks.map((check) => <article key={check.code}><span aria-hidden="true">{check.status === "pass" ? "✓" : check.status === "fail" ? "×" : "!"}</span><div><strong>{check.code}</strong><small>{check.message}</small></div><em>{check.status}</em></article>)}</div></div>;
}

function packBadgeStatusClass(kind: ReturnType<typeof packBadgeKind>) {
  switch (kind) {
    case "certified":
      return "status-completed";
    case "trusted-incomplete":
    case "unsigned":
      return "status-warning";
    case "untrusted":
    case "invalid":
      return "status-failed";
  }
}

function packReviewText(pack: EnginePackSummary, copy: (typeof messages)[Language]) {
  switch (pack.review_status) {
    case "complete":
      return copy.reviewComplete;
    case "incomplete":
      return copy.reviewIncomplete;
    default:
      return copy.reviewMissing;
  }
}

function packBadgeText(pack: EnginePackSummary, copy: (typeof messages)[Language]) {
  switch (packBadgeKind(pack)) {
    case "certified":
      return copy.certified;
    case "trusted-incomplete":
      return copy.signatureTrustedIncomplete;
    case "untrusted":
      return copy.signatureUntrusted;
    case "unsigned":
      return pack.signature_present ? copy.signaturePending : copy.unverified;
    case "invalid":
      return copy.invalidPack;
  }
}

function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KiB", "MiB", "GiB", "TiB"];
  let value = bytes / 1024;
  let unit = units[0];
  for (const candidate of units.slice(1)) {
    if (value < 1024) break;
    value /= 1024;
    unit = candidate;
  }
  return `${value.toFixed(value >= 100 ? 0 : value >= 10 ? 1 : 2)} ${unit}`;
}
