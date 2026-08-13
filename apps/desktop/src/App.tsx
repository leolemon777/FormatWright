import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWebviewWindow } from "@tauri-apps/api/webviewWindow";
import { open, save } from "@tauri-apps/plugin-dialog";
import { useEffect, useMemo, useRef, useState } from "react";

import {
  isDirectoryOutput,
  parseDesktopError,
  recommendedTargets,
  suggestedOutput,
} from "./desktopModel";
import { messages, type Language } from "./i18n";
import {
  QueueProjection,
  type QueueDeltaBatch,
  type QueueSnapshot,
} from "./queueProjection";
import "./styles.css";

type Tab = "convert" | "jobs" | "presets" | "engines" | "reports" | "settings";
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
};

type JobRecord = {
  id: string;
  state: string;
  input_path: string;
  output_path: string;
  updated_unix_ms: number;
};

type DoctorReport = {
  engines: Record<
    string,
    { available: boolean; message: string; identity?: { version: string; certification: string } }
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

export default function App() {
  const [language, setLanguage] = useState<Language>(() =>
    navigator.language.toLowerCase().startsWith("zh") ? "zh-CN" : "en",
  );
  const [expert, setExpert] = useState(false);
  const [settingsLoaded, setSettingsLoaded] = useState(false);
  const [tab, setTab] = useState<Tab>("convert");
  const [inputPath, setInputPath] = useState("");
  const [outputPath, setOutputPath] = useState("");
  const [target, setTarget] = useState("webp");
  const [quality, setQuality] = useState("85");
  const [width, setWidth] = useState("");
  const [dpi, setDpi] = useState("144");
  const [colorMode, setColorMode] = useState("rgb");
  const [preserveAllStreams, setPreserveAllStreams] = useState(true);
  const [preview, setPreview] = useState<Preview | null>(null);
  const [report, setReport] = useState<ValidationReport | null>(null);
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
  const [jobSearch, setJobSearch] = useState("");
  const [bulkBusy, setBulkBusy] = useState(false);
  const [bulkReport, setBulkReport] = useState<BulkActionReport | null>(null);
  const queueSubmission = useRef<{ intent: string; key: string } | null>(null);
  const mounted = useRef(true);
  const copy = messages[language];

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
    void listen<QueueRunReport>("formatwright://queue-window-finished", (event) => {
      if (mounted.current) setQueueReport(event.payload);
      void refreshJobs();
    }).then((dispose) => disposers.push(dispose));
    if ("__TAURI_INTERNALS__" in window) {
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
            if (first) selectInput(first);
          }
        })
        .then((dispose) => disposers.push(dispose));
    }
    void refreshJobs();
    void refreshEngines();
    void refreshPresets();
    return () => {
      mounted.current = false;
      for (const dispose of disposers) dispose();
    };
  }, [projection]);

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
        const normalizedTarget = target === "jpeg" ? "jpg" : target === "yml" ? "yaml" : target;
        if (!snapshot.routes[normalizedTarget]?.available) {
          const firstRecommended = recommendedTargets(inputPath).find(
            (candidate) => snapshot.routes[candidate]?.available,
          );
          const firstAvailable = Object.values(snapshot.routes).find((candidate) => candidate.available)
            ?.target_format;
          const next = firstRecommended ?? firstAvailable;
          if (next) {
            setTarget(next);
            setOutputPath(suggestedOutput(inputPath, next));
          }
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

  function selectInput(path: string) {
    const recommended = recommendedTargets(path)[0];
    setInputPath(path);
    setTarget(recommended);
    setOutputPath(suggestedOutput(path, recommended));
    setPreview(null);
    setReport(null);
    setError(null);
  }

  function changeTarget(next: string) {
    setTarget(next);
    setOutputPath(suggestedOutput(inputPath, next));
    setPreview(null);
  }

  async function chooseInput() {
    try {
      const selected = await open({ directory: false, multiple: false, title: copy.chooseFile });
      if (typeof selected === "string") selectInput(selected);
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
        setOutputPath(selected);
        setPreview(null);
      }
    } catch (reason) {
      setError(parseDesktopError(reason));
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
      dpi: target === "png" || target === "jpg" || target === "jpeg" ? Number(dpi) : null,
      colorMode: target === "png" || target === "jpg" || target === "jpeg" ? colorMode : null,
      preserveAllStreams,
      approvedPlanHash,
      idempotencyKey,
    };
  }

  async function previewPlan() {
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
    setError(null);
    try {
      const result = await invoke<{ job: JobRecord; report: ValidationReport }>(
        "run_desktop_conversion",
        { request: request(preview.plan.plan_hash) },
      );
      setReport(result.report);
      setActiveJobId(null);
      setTab("reports");
      await refreshJobs();
    } catch (reason) {
      setPreview(null);
      setError(parseDesktopError(reason));
      setActiveJobId(null);
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
    } finally {
      setBusy(null);
    }
  }

  async function runQueueWindow() {
    setBusy("queue-run");
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
    } finally {
      setBusy(null);
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

  async function refreshJobs() {
    try {
      setJobs(await invoke<JobRecord[]>("list_desktop_jobs", { limit: 100 }));
    } catch {
      // Browser-only development and first setup can render without an IPC backend.
    }
  }

  async function requeueJob(job: JobRecord) {
    setJobActionBusy(job.id);
    setError(null);
    try {
      await invoke<JobRecord>("requeue_desktop_job", { jobId: job.id });
      setQueueReport(null);
      await refreshJobs();
    } catch (reason) {
      setError(parseDesktopError(reason));
    } finally {
      setJobActionBusy(null);
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
        batchId: null,
        states,
        search: jobSearch.trim() || null,
      });
      const result = await invoke<BulkActionReport>("run_desktop_bulk_action", {
        selectionId: selection.id,
        action,
      });
      setBulkReport(result);
      setQueueReport(null);
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
    try {
      setReport(await invoke<ValidationReport | null>("get_desktop_report", { jobId }));
      setTab("reports");
    } catch (reason) {
      setError(parseDesktopError(reason));
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
  const targetOptions = Array.from(new Set([...recommendations, "jpg", "png", "webp", "avif", "mp4", "mp3", "m4a", "wav", "gif", "pdf", "docx", "json", "csv", "yaml", "xml"]));
  const tabs: Tab[] = ["convert", "jobs", "presets", "engines", "reports", "settings"];
  const normalizedJobSearch = jobSearch.trim().toLocaleLowerCase();
  const visibleJobs = normalizedJobSearch
    ? jobs.filter((job) =>
        `${job.input_path}\n${job.output_path}`.toLocaleLowerCase().includes(normalizedJobSearch),
      )
    : jobs;

  return (
    <main className="shell">
      <header className="topbar">
        <button className="brandmark" type="button" onClick={() => setTab("convert")} aria-label={copy.product}>FW</button>
        <div className="brandcopy"><strong>{copy.product}</strong><span>{copy.tagline}</span></div>
        <nav aria-label="Primary">
          {tabs.map((item) => (
            <button key={item} type="button" className={tab === item ? "nav-active" : ""} onClick={() => setTab(item)}>
              {copy[item]}
            </button>
          ))}
        </nav>
        <span className="local-badge">● {copy.localOnly}</span>
      </header>

      {error && (
        <section className="error-banner" role="alert">
          <strong>{error.code ?? copy.stageError}{error.stage ? ` · ${error.stage}` : ""}</strong>
          <span>{error.message}</span>
          {error.recovery && <small>{error.recovery}</small>}
        </section>
      )}

      {tab === "convert" && (
        <section className="workspace">
          <div className="workspace-main">
            <div className={`drop-zone ${dragging ? "is-dragging" : ""}`}>
              <span className="drop-icon" aria-hidden="true">↓</span>
              <div><h1>{copy.dropTitle}</h1><p>{copy.dropBody}</p></div>
              <button className="secondary choose-file" type="button" onClick={chooseInput}>{copy.chooseFile}</button>
            </div>

            <div className="form-grid">
              <label className="wide">{copy.inputPath}<span className="path-control"><input value={inputPath} onChange={(event) => selectInput(event.target.value)} placeholder="C:\\…\\input.ext" /><button className="secondary" type="button" onClick={chooseInput}>{copy.chooseFile}</button></span></label>
              <label className="wide">{copy.outputPath}<span className="path-control"><input value={outputPath} onChange={(event) => { setOutputPath(event.target.value); setPreview(null); }} placeholder="C:\\…\\output.ext" /><button className="secondary" type="button" disabled={!inputPath} onClick={chooseOutput}>{copy.chooseOutput}</button></span></label>
              <label>{copy.target}<select value={target} onChange={(event) => changeTarget(event.target.value)} disabled={capabilityBusy}>
                {targetOptions.map((value) => <option key={value} disabled={capabilities ? !capabilities.routes[value]?.available : false}>{value}{capabilities && !capabilities.routes[value]?.available ? ` — ${copy.unavailable}` : ""}</option>)}
              </select></label>
              <label>{copy.quality}<input type="number" min="1" max="100" value={quality} onChange={(event) => { setQuality(event.target.value); setPreview(null); }} disabled={target === "png"} /></label>
              {expert && <label>{copy.width}<input type="number" min="1" max="16384" value={width} onChange={(event) => { setWidth(event.target.value); setPreview(null); }} /></label>}
              {expert && <label>{copy.dpi}<input type="number" min="36" max="600" value={dpi} onChange={(event) => { setDpi(event.target.value); setPreview(null); }} /></label>}
              {expert && <label>{copy.colorMode}<select value={colorMode} onChange={(event) => { setColorMode(event.target.value); setPreview(null); }}><option value="rgb">RGB</option><option value="gray">Gray</option></select></label>}
              {expert && <label className="checkbox-control"><input type="checkbox" checked={preserveAllStreams} onChange={(event) => { setPreserveAllStreams(event.target.checked); setPreview(null); }} />{copy.preserveAllStreams}</label>}
            </div>

            {inputPath && (capabilityBusy ? <p className="capability-notice" role="status">{copy.capabilityLoading}</p> : route && !route.available ? <p className="capability-notice capability-blocked" role="status"><strong>{copy.routeUnavailable}</strong>{route.missing_engines.length > 0 ? ` ${copy.missingEngines}: ${route.missing_engines.join(", ")}.` : ` ${route.message}`}</p> : capabilities && !Object.values(capabilities.routes).some((candidate) => candidate.available) ? <p className="capability-notice capability-blocked" role="status">{copy.noAvailableTargets}</p> : null)}

            <div className="preset-row" aria-label="Presets">
              <button type="button" disabled={capabilities ? !capabilities.routes.webp?.available : false} onClick={() => { changeTarget("webp"); setQuality("78"); }}>{copy.presetImage}</button>
              <button type="button" disabled={capabilities ? !capabilities.routes.png?.available : false} onClick={() => changeTarget("png")}>{copy.presetArchive}</button>
              <button type="button" disabled={capabilities ? !capabilities.routes.pdf?.available : false} onClick={() => changeTarget("pdf")}>{copy.presetPdf}</button>
              {presets.map((preset) => <button type="button" key={preset.preset_id} disabled={capabilities ? !capabilities.routes[preset.target_format]?.available : false} onClick={() => applyPreset(preset)}>{preset.name}</button>)}
              <button type="button" onClick={() => setTab("presets")}>+ {copy.savePreset}</button>
            </div>

            <div className="action-row">
              <button className="secondary" type="button" disabled={!inputPath || !outputPath || busy !== null || capabilityBusy || !routeAvailable} onClick={previewPlan}>{busy === "plan" ? copy.planning : copy.inspectPlan}</button>
              <button className="primary" type="button" disabled={!preview || busy !== null || !routeAvailable} onClick={runConversion}>{busy === "run" ? copy.running : copy.run}</button>
              <button className="secondary" type="button" disabled={!preview || busy !== null || !routeAvailable} onClick={queueConversion}>{busy === "queue" ? copy.queueing : copy.queueOnly}</button>
              {busy === "run" && <button className="danger" type="button" onClick={cancel}>{copy.cancel}</button>}
            </div>

            {preview && <PlanView preview={preview} expert={expert} copy={copy} />}
          </div>

          <aside className="side-panel">
            <div className="mode-switch" role="group" aria-label={copy.mode}>
              <button type="button" className={!expert ? "selected" : ""} onClick={() => setExpert(false)}>{copy.basic}</button>
              <button type="button" className={expert ? "selected" : ""} onClick={() => setExpert(true)}>{copy.expert}</button>
            </div>
            <p className="section-label">{copy.recommended}</p>
            <div className="recommendations">{recommendations.map((value, index) => { const candidate = capabilities?.routes[value]; return <button type="button" key={value} disabled={capabilities ? !candidate?.available : false} title={candidate?.message} onClick={() => changeTarget(value)}><span>{index + 1}</span>{value.toUpperCase()}{candidate && !candidate.available ? ` · ${copy.unavailable}` : ""}</button>; })}</div>
            <div className="privacy-card"><strong>LOCAL</strong><p>{copy.privacy}</p></div>
          </aside>
        </section>
      )}

      {tab === "jobs" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">SQLITE</p><h1>{copy.jobs}</h1><p>{copy.queueHint}</p></div><div className="heading-actions"><button className="primary" type="button" disabled={busy !== null} onClick={runQueueWindow}>{busy === "queue-run" ? copy.runningQueue : copy.runQueue}</button>{busy === "queue-run" && <button className="secondary" type="button" onClick={pauseQueueFinishCurrent}>{copy.pauseFinishCurrent}</button>}{busy === "queue-run" && <button className="danger" type="button" onClick={stopQueueWindow}>{copy.stopQueue}</button>}<button type="button" onClick={refreshJobs}>{copy.refresh}</button></div></div>
          {queueReport && (
            <p className="success-notice" role="status" aria-live="polite">
              {copy.queueReport}: selected {queueReport.selected} · completed {queueReport.completed} · warning {queueReport.warning} · blocked {queueReport.blocked} · failed {queueReport.failed} · cancelled {queueReport.cancelled} · peak {queueReport.peak_active}/{queueReport.parallelism}{queueReport.stopped ? " · stopped" : ""}
            </p>
          )}
          <div className="bulk-toolbar">
            <label>{copy.filterJobs}<input value={jobSearch} maxLength={200} onChange={(event) => setJobSearch(event.target.value)} placeholder={copy.filterJobsHint} /></label>
            <div className="heading-actions">
              <button className="primary" type="button" disabled={bulkBusy || busy === "queue-run"} onClick={() => runBulkAction("retry", ["failed", "cancelled", "interrupted"])}>{copy.retryMatching}</button>
              <button type="button" disabled={bulkBusy || busy === "queue-run"} onClick={() => runBulkAction("resume", ["blocked", "interrupted"])}>{copy.resumeMatching}</button>
              <button className="danger" type="button" disabled={bulkBusy || busy === "queue-run"} onClick={() => runBulkAction("cancel", ["planned", "queued", "blocked", "interrupted"])}>{copy.cancelMatching}</button>
            </div>
          </div>
          {bulkReport && <p className="success-notice" role="status" aria-live="polite">{copy.bulkReport}: {bulkReport.transitioned} / {bulkReport.matched} · {copy.skippedState} {bulkReport.skipped_state} · {copy.skippedConflict} {bulkReport.skipped_conflict}</p>}
          <div className="job-list">{visibleJobs.length === 0 ? <p className="empty">{copy.historyEmpty}</p> : visibleJobs.map((job) => { const resumable = job.state === "interrupted" || job.state === "blocked"; const retryable = job.state === "failed" || job.state === "cancelled"; return <article key={job.id}><div><strong>{job.output_path}</strong><small>{job.input_path}</small></div><span className={`status status-${job.state}`}>{job.state}</span><span className="job-actions">{(resumable || retryable) && <button className="primary" type="button" disabled={jobActionBusy !== null || busy === "queue-run" || bulkBusy} onClick={() => requeueJob(job)}>{jobActionBusy === job.id ? (resumable ? copy.resumingJob : copy.retryingJob) : (resumable ? copy.resumeJob : copy.retryJob)}</button>}<button type="button" onClick={() => loadReport(job.id)}>{copy.selectJob}</button></span></article>; })}</div>
          <details className="benchmark"><summary>{copy.benchmark}</summary><button type="button" onClick={runBenchmark}>{copy.benchmark}</button>{benchmark && <p>{benchmark.total_jobs.toLocaleString()} / {benchmark.emitted_batches} batches / {benchmark.elapsed_milliseconds} ms</p>}<p>{queueSnapshot.totalJobs.toLocaleString()} projected · {queueSnapshot.completed.toLocaleString()} completed</p></details>
        </section>
      )}

      {tab === "presets" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">PORTABLE JSON</p><h1>{copy.presets}</h1><p>{copy.presetsHint}</p></div><div className="heading-actions"><button className="secondary" type="button" disabled={presetBusy} onClick={importPresets}>{copy.importPresets}</button><button type="button" disabled={presetBusy || presets.length === 0} onClick={exportPresets}>{copy.exportPresets}</button></div></div>
          {presetNotice && <p className="success-notice" role="status" aria-live="polite">{presetNotice}</p>}
          <section className="preset-editor" aria-label={copy.presetEditor}>
            <div><p className="section-label">{editingPresetId ? copy.editPreset : copy.newPreset}</p><h2>{editingPresetId ? copy.editPreset : copy.saveCurrentSettings}</h2></div>
            <div className="preset-fields"><label>{copy.presetName}<input maxLength={80} value={presetName} onChange={(event) => setPresetName(event.target.value)} /></label><label>{copy.target}<select value={target} onChange={(event) => changeTarget(event.target.value)}>{targetOptions.map((value) => <option key={value} disabled={capabilities ? !capabilities.routes[value]?.available : false}>{value}</option>)}</select></label><label>{copy.quality}<input type="number" min="1" max="100" value={quality} onChange={(event) => setQuality(event.target.value)} /></label><label>{copy.width}<input type="number" min="1" max="16384" value={width} onChange={(event) => setWidth(event.target.value)} /></label><label>{copy.dpi}<input type="number" min="36" max="600" value={dpi} onChange={(event) => setDpi(event.target.value)} /></label><label>{copy.colorMode}<select value={colorMode} onChange={(event) => setColorMode(event.target.value)}><option value="rgb">RGB</option><option value="gray">Gray</option></select></label><label className="checkbox-control"><input type="checkbox" checked={preserveAllStreams} onChange={(event) => setPreserveAllStreams(event.target.checked)} />{copy.preserveAllStreams}</label></div>
            <div className="action-row"><button className="primary" type="button" disabled={presetBusy || presetName.trim().length === 0} onClick={savePreset}>{presetBusy ? copy.savingPreset : copy.savePreset}</button>{editingPresetId && <button className="secondary" type="button" onClick={resetPresetEditor}>{copy.cancelEdit}</button>}</div>
          </section>
          <div className="preset-list">{presets.length === 0 ? <p className="empty">{copy.noPresets}</p> : presets.map((preset) => <article key={preset.preset_id}><div><strong>{preset.name}</strong><small>{preset.target_format.toUpperCase()} · Q {preset.quality ?? "—"} · {preset.width ? `${preset.width}px` : copy.originalSize}</small></div><div className="preset-actions"><button type="button" onClick={() => applyPreset(preset)}>{copy.applyPreset}</button><button type="button" onClick={() => editPreset(preset)}>{copy.editPreset}</button><button className={pendingDeleteId === preset.preset_id ? "danger" : "secondary"} type="button" disabled={presetBusy} onClick={() => deletePreset(preset.preset_id)}>{pendingDeleteId === preset.preset_id ? copy.confirmDelete : copy.deletePreset}</button></div></article>)}</div>
        </section>
      )}

      {tab === "engines" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">LOCAL INVENTORY</p><h1>{copy.doctor}</h1><p>{copy.doctorHint}</p></div><div className="heading-actions"><button className="secondary" type="button" disabled={engineBusy} onClick={importEnginePack}>{engineBusy ? copy.verifyingEnginePack : copy.importEnginePack}</button><button type="button" onClick={refreshEngines}>{copy.refresh}</button></div></div>
          {!doctor ? <p className="empty">{copy.importHint}</p> : <div className="engine-grid">{Object.entries(doctor.engines).map(([name, health]) => <article key={name}><strong>{name}</strong><span className={`status ${health.available ? "status-completed" : "status-failed"}`}>{health.available ? `✓ ${copy.available}` : `× ${copy.unavailable}`}</span><small>{health.identity?.version ?? health.message}</small></article>)}</div>}
          <div className="pack-section"><p className="section-label">{copy.importedPacks}</p>{enginePacks.length === 0 ? <p className="empty">{copy.noImportedPacks}</p> : <div className="pack-list">{enginePacks.map((pack) => <article key={pack.manifest_sha256 ?? pack.manifest_path}><div><strong>{pack.engine_id ?? copy.invalidPack} {pack.version ?? ""}</strong><small>{pack.manifest_path}</small><small>{pack.executable_names.join(", ") || pack.message}</small></div><span className={`status ${pack.valid ? "status-warning" : "status-failed"}`}>{pack.valid ? (pack.signature_present ? copy.signaturePending : copy.unverified) : copy.invalidPack}</span></article>)}</div>}</div>
        </section>
      )}

      {tab === "reports" && (
        <section className="page-card">
          <div className="page-heading"><div><p className="section-label">VALIDATION</p><h1>{copy.report}</h1></div>{report && <span className={`report-status report-${report.status}`}>{report.status}</span>}</div>
          {!report ? <p className="empty">{copy.noReport}</p> : <ReportView report={report} copy={copy} />}
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
    </main>
  );
}

function PlanView({ preview, expert, copy }: { preview: Preview; expert: boolean; copy: (typeof messages)[Language] }) {
  return <section className="plan-card" aria-live="polite"><div className="plan-heading"><div><p className="section-label">{copy.detected}</p><h2>{preview.probe.format.id.toUpperCase()} → {preview.plan.target_format.toUpperCase()}</h2></div><span className={`loss loss-${preview.plan.steps.some((step) => step.loss_class === "lossy") ? "lossy" : "safe"}`}>{preview.plan.steps.map((step) => step.loss_class).join(" · ")}</span></div><div className="change-grid"><ChangeList title={copy.preserved} values={preview.plan.changes.preserved} symbol="✓" /><ChangeList title={copy.changed} values={preview.plan.changes.changed} symbol="△" /><ChangeList title={copy.dropped} values={preview.plan.changes.dropped} symbol="−" /><ChangeList title={copy.unknown} values={preview.plan.changes.unknown} symbol="?" /></div><h3>{copy.engineSteps}</h3><ol className="steps">{preview.plan.steps.map((step) => <li key={step.step_id}><div><strong>{step.engine.engine_id}</strong><small>{step.operation} · {step.engine.certification}</small></div><code>{step.capability_id}</code>{expert && <pre>{JSON.stringify(step.arguments, null, 2)}</pre>}</li>)}</ol>{expert && <p className="typed-note">{copy.commandBoundary}<br /><code>{preview.plan.plan_hash}</code></p>}</section>;
}

function ChangeList({ title, values, symbol }: { title: string; values: string[]; symbol: string }) {
  return <div><h3>{symbol} {title}</h3>{values.length === 0 ? <span>—</span> : <ul>{values.map((value) => <li key={value}>{value}</li>)}</ul>}</div>;
}

function ReportView({ report, copy }: { report: ValidationReport; copy: (typeof messages)[Language] }) {
  const passed = report.checks.filter((check) => check.required && check.status === "pass").length;
  const required = report.checks.filter((check) => check.required).length;
  return <div className="report-body"><div className="report-summary"><div><span>{copy.requiredChecks}</span><strong>{passed}/{required}</strong></div><div><span>{copy.openPathHint}</span><strong>{report.output.display_path ?? "—"}</strong></div></div><div className="check-list">{report.checks.map((check) => <article key={check.code}><span aria-hidden="true">{check.status === "pass" ? "✓" : check.status === "fail" ? "×" : "!"}</span><div><strong>{check.code}</strong><small>{check.message}</small></div><em>{check.status}</em></article>)}</div></div>;
}
