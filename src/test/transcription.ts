// Faux moteur de l'outil Transcription pour le simulateur : options (deux
// modèles installés, un non installé), fichiers « choisis » et lus,
// transcription en cours (passages en delta, comme le moteur), historique,
// exports, outils yt-dlp et Deno (accord exigé, progression), synthèse.
// Le test pilote l'avancement par `progress`, `finish`, `toolsEvent` et
// `synthesisEvent`, qui émettent les événements du moteur. À installer
// après fakeBackend (qui répond à dictation_info, list_models…).
// Pour les tests seulement.

import type { AppError } from "@/bindings/AppError";
import type { SynthesisStatus } from "@/bindings/SynthesisStatus";
import type { ToolsStatus } from "@/bindings/ToolsStatus";
import type { ToolStatus } from "@/bindings/ToolStatus";
import type { TranscriptionDetail } from "@/bindings/TranscriptionDetail";
import type { TranscriptionError } from "@/bindings/TranscriptionError";
import type { TranscriptionItem } from "@/bindings/TranscriptionItem";
import type { TranscriptionJob } from "@/bindings/TranscriptionJob";
import type { TranscriptionOptions } from "@/bindings/TranscriptionOptions";
import type { TranscriptionProbe } from "@/bindings/TranscriptionProbe";
import type { TranscriptionRequest } from "@/bindings/TranscriptionRequest";
import type { TranscriptionSegment } from "@/bindings/TranscriptionSegment";
import type { TranscriptionStage } from "@/bindings/TranscriptionStage";
import type { RawBody, Simulator } from "@/test/simulator";

export const PARAKEET = "parakeet-tdt-0.6b-v3-int8";
export const WHISPER = "whisper-large-v3-turbo";

export interface FakeTranscription {
  options: TranscriptionOptions;
  job: TranscriptionJob | null;
  /** Historique, du plus récent au plus ancien. */
  items: TranscriptionDetail[];
  tools: ToolsStatus;
  synthesis: SynthesisStatus | null;
  /** Chemin rendu par le sélecteur (null : annulé). */
  picked: string | null;
  /** Fiche rendue par transcription_probe, par chemin. */
  probes: Record<string, TranscriptionProbe>;
  /** Fichiers enregistrés par save_document. */
  saves: { kind: string; name: string; text: string; bytes: number[] }[];
  /** Chemin rendu par save_document (null : boîte annulée). */
  savePath: string | null;
  /** Erreur rendue par une commande, par nom. */
  failures: Map<string, AppError>;
  /** Nouvelle ligne de l'historique (finie), en tête. */
  add(
    patch?: Partial<TranscriptionItem>,
    segments?: TranscriptionSegment[],
  ): TranscriptionDetail;
  /** Avance la transcription en cours ; `segments` : nouveaux passages. */
  progress(
    patch: Partial<TranscriptionJob>,
    segments?: TranscriptionSegment[],
  ): Promise<void>;
  /** Fin de la transcription en cours (enregistrée dans l'historique). */
  finish(stage: TranscriptionStage, error?: TranscriptionError): Promise<void>;
  /** État des outils changé, émis comme le moteur. */
  toolsEvent(
    patch: Partial<ToolsStatus>,
    tools?: Partial<ToolStatus>[],
  ): Promise<void>;
  /** État de la synthèse, émis comme le moteur. */
  synthesisEvent(status: SynthesisStatus): Promise<void>;
}

export function segment(start: number, end: number, text: string) {
  return { start, end, text };
}

export function tool(
  id: ToolStatus["tool"],
  patch: Partial<ToolStatus> = {},
): ToolStatus {
  const ytdlp = id === "ytDlp";
  return {
    tool: id,
    name: ytdlp ? "yt-dlp" : "Deno",
    phase: "absent",
    version: null,
    latest: null,
    checkedAt: null,
    completedBytes: 0,
    totalBytes: 0,
    downloadBytes: ytdlp ? 36 * 2 ** 20 : 45 * 2 ** 20,
    installedBytes: null,
    license: ytdlp ? "Unlicense" : "MIT",
    licenseUrl: ytdlp
      ? "https://github.com/yt-dlp/yt-dlp/blob/master/LICENSE"
      : "https://github.com/denoland/deno/blob/main/LICENSE.md",
    thirdPartyLicenses: null,
    error: null,
    ...patch,
  };
}

export function installedTools(): ToolsStatus {
  return {
    supported: true,
    ready: true,
    inUse: false,
    tools: [
      tool("ytDlp", { phase: "installed", version: "2026.10.26" }),
      tool("deno", { phase: "installed", version: "2.9.8" }),
    ],
  };
}

export function itemOf(
  patch: Partial<TranscriptionItem> = {},
): TranscriptionItem {
  return {
    id: "t-0",
    source: "file",
    origin: "/home/me/lecture.mp3",
    title: "lecture.mp3",
    channel: "",
    url: "",
    site: "",
    durationS: 125,
    engine: "parakeet",
    model: PARAKEET,
    language: "en",
    state: "done",
    error: null,
    preview: "",
    hasSummary: false,
    createdAt: Date.now() - 60_000,
    updatedAt: Date.now() - 60_000,
    finishedAt: Date.now() - 60_000,
    ...patch,
  };
}

function jobOf(id: string, request: TranscriptionRequest): TranscriptionJob {
  const url = request.source.kind === "url";
  const origin = url
    ? (request.source as { url: string }).url
    : (request.source as { path: string }).path;
  return {
    id,
    source: request.source.kind,
    origin,
    stage: "preparing",
    model: request.model ?? PARAKEET,
    engine: (request.model ?? PARAKEET).startsWith("whisper")
      ? "whisper"
      : "parakeet",
    language: request.language ?? null,
    title: url ? null : (origin.split("/").at(-1) ?? origin),
    channel: null,
    url: null,
    site: null,
    durationS: null,
    receivedBytes: null,
    totalBytes: null,
    doneS: 0,
    speed: null,
    segments: [],
    segmentsFrom: 0,
    error: null,
  };
}

const clone = <T>(value: T): T => structuredClone(value);

export function fakeTranscription(simulator: Simulator): FakeTranscription {
  let next = 1;
  const fake: FakeTranscription = {
    options: {
      models: [
        {
          id: PARAKEET,
          name: "Parakeet TDT 0.6B v3",
          engine: "parakeet",
          installed: true,
          languages: "25 European languages",
        },
        {
          id: WHISPER,
          name: "Whisper Large v3 Turbo",
          engine: "whisper",
          installed: true,
          languages: "99 languages",
        },
        {
          id: "whisper-small",
          name: "Whisper Small",
          engine: "whisper",
          installed: false,
          languages: "99 languages",
        },
      ],
      defaultModel: PARAKEET,
      language: "auto",
    },
    job: null,
    items: [],
    tools: {
      supported: true,
      ready: false,
      inUse: false,
      tools: [tool("ytDlp"), tool("deno")],
    },
    synthesis: null,
    picked: null,
    probes: {},
    saves: [],
    savePath: "/home/me/saved",
    failures: new Map(),
    add(patch = {}, segments = []) {
      const detail: TranscriptionDetail = {
        item: itemOf({ id: `t-${next++}`, ...patch }),
        segments,
        text: segments.map((s) => s.text).join(" "),
        summary: null,
      };
      fake.items.unshift(detail);
      return detail;
    },
    async progress(patch, segments = []) {
      const job = fake.job;
      if (!job) throw simulator.fail("progress: no transcription running");
      const from = job.segments.length;
      Object.assign(job, patch);
      job.segments = [...job.segments, ...segments];
      const detail = fake.items.find((d) => d.item.id === job.id);
      if (detail) {
        detail.segments = clone(job.segments);
        Object.assign(detail.item, {
          title: job.title ?? detail.item.title,
          channel: job.channel ?? "",
          durationS: job.durationS,
        });
      }
      await simulator.emitEvent("transcription-status", {
        job: { ...clone(job), segments, segmentsFrom: from },
      });
    },
    async finish(stage, error) {
      const job = fake.job;
      if (!job) throw simulator.fail("finish: no transcription running");
      job.stage = stage;
      job.error = error ?? null;
      const index = fake.items.findIndex((d) => d.item.id === job.id);
      if (index >= 0) {
        if (job.segments.length === 0) {
          fake.items.splice(index, 1);
        } else {
          const detail = fake.items[index];
          detail.segments = clone(job.segments);
          detail.text = job.segments.map((s) => s.text).join(" ");
          detail.item.state =
            stage === "done"
              ? "done"
              : stage === "cancelled"
                ? "cancelled"
                : "failed";
          detail.item.error = error ?? null;
        }
      }
      await simulator.emitEvent("transcription-status", {
        job: { ...clone(job), segments: [], segmentsFrom: job.segments.length },
      });
    },
    async toolsEvent(patch, tools) {
      Object.assign(fake.tools, patch);
      if (tools) {
        fake.tools.tools = fake.tools.tools.map((item, index) => ({
          ...item,
          ...(tools[index] ?? {}),
        }));
      }
      await simulator.emitEvent("tools-status", clone(fake.tools));
    },
    async synthesisEvent(status) {
      fake.synthesis = clone(status);
      if (status.stage === "done") {
        const detail = fake.items.find((d) => d.item.id === status.id);
        if (detail) {
          detail.summary = {
            markdown: status.markdown,
            model: status.model ?? "",
            at: Date.now(),
          };
          detail.item.hasSummary = true;
        }
      }
      await simulator.emitEvent("transcription-synthesis", clone(status));
    },
  };

  const failure = (command: string) => {
    const error = fake.failures.get(command);
    if (error) throw error;
  };
  const find = (id: unknown) => {
    const detail = fake.items.find((d) => d.item.id === id);
    if (!detail) throw { kind: "notFound", message: "Transcription not found" };
    return detail;
  };

  simulator.handle("transcription_options", () => {
    failure("transcription_options");
    return fake.options;
  });
  simulator.handle("transcription_pick_file", () => {
    failure("transcription_pick_file");
    return fake.picked;
  });
  simulator.handle("transcription_probe", (args) => {
    failure("transcription_probe");
    const path = args.path as string;
    return (
      fake.probes[path] ?? {
        name: path.split("/").at(-1) ?? path,
        durationS: 125,
        error: null,
      }
    );
  });
  simulator.handle("transcription_start", (args) => {
    failure("transcription_start");
    if (fake.job && !["done", "failed", "cancelled"].includes(fake.job.stage)) {
      throw { kind: "invalid", message: "A transcription is already running." };
    }
    const request = args.request as TranscriptionRequest;
    const id = `t-${next++}`;
    fake.job = jobOf(id, request);
    fake.items.unshift({
      item: itemOf({
        id,
        source: fake.job.source,
        origin: fake.job.origin,
        title: fake.job.title ?? fake.job.origin,
        model: fake.job.model,
        engine: fake.job.engine ?? "parakeet",
        language: fake.job.language,
        state: "running",
        durationS: null,
        finishedAt: null,
      }),
      segments: [],
      text: "",
      summary: null,
    });
    return id;
  });
  simulator.handle("transcription_cancel", () => {
    failure("transcription_cancel");
    return null;
  });
  simulator.handle("transcription_status", () => {
    failure("transcription_status");
    return { job: fake.job ? { ...clone(fake.job), segmentsFrom: 0 } : null };
  });
  simulator.handle("transcription_list", () => {
    failure("transcription_list");
    return fake.items.map((d) => ({
      ...d.item,
      preview: d.text.slice(0, 200),
    }));
  });
  simulator.handle("transcription_get", (args) => {
    failure("transcription_get");
    return find(args.id);
  });
  simulator.handle("transcription_delete", (args) => {
    failure("transcription_delete");
    find(args.id);
    fake.items = fake.items.filter((d) => d.item.id !== args.id);
    return null;
  });
  simulator.handle("transcription_rename", (args) => {
    failure("transcription_rename");
    find(args.id).item.title = args.title as string;
    return null;
  });
  simulator.handle("transcription_export", (args) => {
    failure("transcription_export");
    const detail = find(args.id);
    const stamp = args.timestamps ? "[0:00] " : "";
    return `${args.format}:${stamp}${detail.text}`;
  });
  simulator.handle("save_document", (args) => {
    failure("save_document");
    const raw = args as unknown as RawBody;
    fake.saves.push({
      kind: raw.headers["x-kind"],
      name: decodeURIComponent(raw.headers["x-filename"]),
      text: new TextDecoder().decode(new Uint8Array(raw.bytes)),
      bytes: raw.bytes,
    });
    return fake.savePath ? `${fake.savePath}.${raw.headers["x-kind"]}` : null;
  });

  simulator.handle("tools_status", () => {
    failure("tools_status");
    return fake.tools;
  });
  simulator.handle("tools_install", (args) => {
    failure("tools_install");
    if (args.acceptLicenses !== true) {
      throw { kind: "invalid", message: "Licenses not accepted." };
    }
    return null;
  });
  for (const command of ["tools_update", "tools_cancel", "tools_remove"]) {
    simulator.handle(command, () => {
      failure(command);
      return null;
    });
  }

  simulator.handle("transcription_synthesize", (args) => {
    failure("transcription_synthesize");
    fake.synthesis = {
      id: args.id as string,
      stage: "running",
      markdown: "",
      model: (args.model as string | null) ?? "",
      error: null,
    } as SynthesisStatus;
    return null;
  });
  simulator.handle("transcription_synthesis_cancel", () => {
    failure("transcription_synthesis_cancel");
    return null;
  });
  simulator.handle("transcription_synthesis_status", () => {
    failure("transcription_synthesis_status");
    return fake.synthesis;
  });
  simulator.handle("open_external", () => null);

  return fake;
}
