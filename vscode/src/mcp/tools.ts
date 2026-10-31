// Outils que ChatonLM donne aux agents par MCP (liste blanche de cinq outils,
// chacun sur une méthode du pont qui existe déjà) : description en anglais
// (langue des agents), entrées vérifiées et bornées, exécution annulable.
// Rien d'autre n'est joignable : ni l'historique, ni les réglages (l'accès
// agents est en plus limité par l'app elle-même, contrat 97 § 1). Seulement
// cette machine : un Ollama sur une autre machine (`app.state.local`) ou un
// modèle du cloud est refusé ; la correction demande `localOnly`. Seuls
// `chatonlm_models` et `chatonlm_proofread` sont marqués en lecture seule
// (`readOnlyHint`) : VS Code demande confirmation pour les autres (voix à
// haute voix, carte graphique occupée, modèle chargé). Sans dépendance à VS
// Code (dans `dist/mcp.cjs`). Contrats : .tools/research/95-lot4b-contract.md
// (2), 97-lot4b-fix-contract.md ; méthodes : 87-lot1a-contract.md.
import {
  type Call,
  ClosedError,
  type Notification,
  RpcError,
  TooLargeError,
} from "../bridge/rpc";
import { BRIDGE_FORBIDDEN } from "../../shared/protocol";
import { oneLine, type ToolName } from "./files";

/** Ce que les outils attendent du pont (la liaison `Link`). */
export type BridgeLike = {
  call<T>(method: string, params?: unknown, timeoutMs?: number): Call<T>;
  cancel(id: number): void;
  onNotification(listener: (n: Notification) => void): () => void;
};

export type Content =
  | { type: "text"; text: string }
  | { type: "image"; data: string; mimeType: string };

export type ToolResult = { content: Content[]; isError?: boolean };

/** Bornes des entrées (en caractères). */
export const LIMITS = {
  question: 32_000,
  model: 200,
  prompt: 4_000,
  speech: 2_000,
  proofread: 20_000,
} as const;

type Field =
  | {
      type: "string";
      description: string;
      maxLength: number;
      enum?: readonly string[];
      required?: boolean;
    }
  | {
      type: "integer";
      description: string;
      minimum: number;
      maximum: number;
      multipleOf?: number;
      required?: boolean;
    };

type ToolSpec = {
  name: ToolName;
  description: string;
  fields: Record<string, Field>;
  /** `readOnlyHint` : sans effet visible ni coût pour l'utilisateur ; VS
   * Code ne demande alors pas de confirmation. */
  readOnly: boolean;
};

const SIDE = {
  type: "integer",
  minimum: 256,
  maximum: 1536,
  multipleOf: 16,
} as const;

/** Les cinq outils, et rien d'autre. */
export const TOOLS: readonly ToolSpec[] = [
  {
    name: "chatonlm_ask",
    description:
      "Ask a question to a language model that runs on this computer through the ChatonLM app, and get its answer as text. Nothing is kept in ChatonLM's history. Cloud models are refused. Use chatonlm_models to list the models.",
    fields: {
      question: {
        type: "string",
        description: "The question or task for the local model.",
        maxLength: LIMITS.question,
        required: true,
      },
      model: {
        type: "string",
        description:
          "Name of an installed local model (from chatonlm_models). Default: the model chosen in ChatonLM.",
        maxLength: LIMITS.model,
      },
    },
    // Charge un modèle (mémoire, carte graphique).
    readOnly: false,
  },
  {
    name: "chatonlm_generate_image",
    description:
      "Generate a PNG image on this computer with the image model chosen in ChatonLM. Returns the image. ChatonLM writes no file.",
    fields: {
      prompt: {
        type: "string",
        description: "What the image shows.",
        maxLength: LIMITS.prompt,
        required: true,
      },
      width: {
        ...SIDE,
        description:
          "Width in pixels, a multiple of 16 from 256 to 1536. Default: the model's size.",
      },
      height: {
        ...SIDE,
        description:
          "Height in pixels, a multiple of 16 from 256 to 1536. Default: the model's size.",
      },
    },
    // Occupe la carte graphique, peut décharger le modèle de l'utilisateur.
    readOnly: false,
  },
  {
    name: "chatonlm_speak",
    description:
      "Read a short text aloud to the user with ChatonLM's local voice. Returns once it has been read.",
    fields: {
      text: {
        type: "string",
        description: "The text to read aloud (short).",
        maxLength: LIMITS.speech,
        required: true,
      },
      language: {
        type: "string",
        description: "Language of the text. Default: ChatonLM's language.",
        maxLength: 2,
        enum: ["en", "fr", "es"],
      },
    },
    // Parle à voix haute (et coupe une lecture en cours).
    readOnly: false,
  },
  {
    name: "chatonlm_proofread",
    description:
      "Correct the spelling and grammar of a text (mode correct), or rephrase it (mode rephrase), with ChatonLM's proofreading model on this computer. Returns the new text.",
    fields: {
      text: {
        type: "string",
        description: "The text to proofread.",
        maxLength: LIMITS.proofread,
        required: true,
      },
      mode: {
        type: "string",
        description: "correct (default) or rephrase.",
        maxLength: 8,
        enum: ["correct", "rephrase"],
      },
    },
    // Rend un texte, rien d'autre ; seulement sur cette machine (`localOnly`).
    readOnly: true,
  },
  {
    name: "chatonlm_models",
    description:
      "List the language models installed in ChatonLM that run on this computer, and the one chosen by default.",
    fields: {},
    readOnly: true,
  },
];

/** Définitions données à `tools/list`. */
export function toolList() {
  return TOOLS.map((tool) => ({
    name: tool.name,
    description: tool.description,
    inputSchema: {
      type: "object",
      properties: Object.fromEntries(
        Object.entries(tool.fields).map(([key, field]) => {
          const schema: Record<string, unknown> = { ...field };
          delete schema.required;
          return [key, schema];
        }),
      ),
      required: Object.entries(tool.fields)
        .filter(([, field]) => field.required)
        .map(([key]) => key),
      additionalProperties: false,
    },
    annotations: {
      readOnlyHint: tool.readOnly,
      destructiveHint: false,
      openWorldHint: false,
    },
  }));
}

export class InputError extends Error {}

/**
 * Arguments vérifiés d'un outil : champ inconnu, type, longueur, bornes ;
 * chaînes requises non vides. Lève InputError (message pour l'agent).
 */
export function validate(
  name: ToolName,
  args: unknown,
): Record<string, string | number> {
  const spec = TOOLS.find((tool) => tool.name === name);
  if (!spec) throw new InputError(`Unknown tool: ${name}`);
  const given = args ?? {};
  if (typeof given !== "object" || Array.isArray(given)) {
    throw new InputError("The arguments must be an object.");
  }
  const out: Record<string, string | number> = {};
  for (const [key, value] of Object.entries(given)) {
    // Propres au schéma seulement (pas `toString`, `constructor`…).
    const field = Object.hasOwn(spec.fields, key) ? spec.fields[key] : null;
    if (!field) throw new InputError(`Unknown argument: ${oneLine(key, 40)}`);
    if (value === undefined || value === null) continue;
    if (field.type === "string") {
      if (typeof value !== "string") {
        throw new InputError(`${key} must be a string.`);
      }
      if (value.length > field.maxLength) {
        throw new InputError(
          `${key} is too long (${field.maxLength} characters at most).`,
        );
      }
      if (field.enum && !field.enum.includes(value)) {
        throw new InputError(
          `${key} must be one of: ${field.enum.join(", ")}.`,
        );
      }
      if (!value.trim()) continue;
      out[key] = value;
    } else {
      if (
        typeof value !== "number" ||
        !Number.isInteger(value) ||
        value < field.minimum ||
        value > field.maximum ||
        (field.multipleOf !== undefined && value % field.multipleOf !== 0)
      ) {
        throw new InputError(
          `${key} must be an integer from ${field.minimum} to ${field.maximum}${
            field.multipleOf ? `, a multiple of ${field.multipleOf}` : ""
          }.`,
        );
      }
      out[key] = value;
    }
  }
  for (const [key, field] of Object.entries(spec.fields)) {
    if (field.required && out[key] === undefined) {
      throw new InputError(`${key} is required.`);
    }
  }
  return out;
}

/** Début de la demande, montré dans le panneau de VS Code. */
export function detailOf(
  name: ToolName,
  args: Record<string, string | number>,
): string | null {
  switch (name) {
    case "chatonlm_ask":
      return oneLine(args.question, 80);
    case "chatonlm_generate_image":
      return oneLine(args.prompt, 80);
    case "chatonlm_speak":
    case "chatonlm_proofread":
      return oneLine(args.text, 80);
    case "chatonlm_models":
      return null;
  }
}

/** Arrêt demandé (agent, panneau de VS Code, réglage coupé). */
export class StoppedError extends Error {
  constructor() {
    super("stopped");
  }
}

const text = (value: string): ToolResult => ({
  content: [{ type: "text", text: value }],
});
const failure = (value: string): ToolResult => ({
  content: [{ type: "text", text: value }],
  isError: true,
});

/** Modèle du cloud, d'après son nom (comme l'app). */
export const looksCloud = (model: string) => /[:-]cloud$/i.test(model);

const CANCELLED = -32800;

/** Message d'un refus de l'app (déjà traduit par elle) ou d'un échec. */
export function bridgeErrorText(error: unknown): string {
  if (error instanceof RpcError) {
    if (error.code === -32601) {
      return "This version of ChatonLM can't do this. Update ChatonLM.";
    }
    if (error.code === BRIDGE_FORBIDDEN) {
      return "ChatonLM does not let agents do this.";
    }
    if (error.code === -32030) {
      return (
        error.appMessage ??
        "ChatonLM has no voice for this language yet. Open ChatonLM to download one."
      );
    }
    return error.appMessage ?? `ChatonLM refused: ${error.message}`;
  }
  if (error instanceof ClosedError) return "The link with ChatonLM was lost.";
  if (error instanceof TooLargeError) {
    return "This is too large to send to ChatonLM.";
  }
  return "ChatonLM could not do this.";
}

/** Attend une requête du pont ; l'arrêt annule la requête dans l'app. */
async function awaitCall<T>(
  bridge: BridgeLike,
  call: Call<T>,
  signal: AbortSignal,
): Promise<T> {
  const onAbort = () => bridge.cancel(call.id);
  if (signal.aborted) onAbort();
  signal.addEventListener("abort", onAbort);
  try {
    return await call.result;
  } catch (error) {
    if (
      signal.aborted ||
      (error instanceof RpcError && error.code === CANCELLED)
    ) {
      throw new StoppedError();
    }
    throw error;
  } finally {
    signal.removeEventListener("abort", onAbort);
  }
}

type Details = { remote?: unknown; contextLength?: unknown };

type AppState = { selectedModel?: unknown; local?: unknown };

/** Refus d'un Ollama sur une autre machine (réglage du serveur de l'app). */
export const NOT_LOCAL_TEXT =
  "ChatonLM uses an Ollama server on another computer. ChatonLM only lets agents use models that run on this computer.";

async function appState(bridge: BridgeLike): Promise<AppState> {
  return (await bridge.call<AppState>("app.state").result) ?? {};
}

/**
 * Le modèle tourne-t-il hors de cette machine (cloud, ou Ollama de l'app sur
 * une autre machine) ? `local` : `app.state.local` déjà lu, sinon demandé.
 * Absent : InputError.
 */
export async function isCloud(
  bridge: BridgeLike,
  model: string,
  local?: boolean,
): Promise<boolean> {
  if (looksCloud(model)) return true;
  // Ce que l'app ne dit pas compte comme une autre machine.
  const here = local ?? (await appState(bridge)).local === true;
  if (!here) return true;
  let details: Details;
  try {
    details = await bridge.call<Details>("models.details", { model }).result;
  } catch (error) {
    if (error instanceof RpcError && error.code === -32020) {
      throw new InputError(
        `${oneLine(model, 80)} is not installed in ChatonLM. Use chatonlm_models to list the models.`,
      );
    }
    throw error;
  }
  // Ce que l'app ne dit pas compte comme hors de la machine.
  return details?.remote !== false;
}

type SendResult = {
  chatId: string | null;
  outcome: "done" | "error" | "cancelled";
};

type ChatEventParams = {
  requestId?: unknown;
  event?: {
    type?: unknown;
    text?: unknown;
    message?: unknown;
    chatId?: unknown;
  };
};

/**
 * Question à un modèle local, dans une conversation éphémère effacée
 * aussitôt après (rien ne reste dans l'historique de l'app).
 */
async function ask(
  bridge: BridgeLike,
  args: Record<string, string | number>,
  signal: AbortSignal,
): Promise<ToolResult> {
  const app = await appState(bridge);
  if (app.local !== true) return failure(NOT_LOCAL_TEXT);
  let model = typeof args.model === "string" ? args.model.trim() : null;
  if (!model) {
    model =
      typeof app.selectedModel === "string" && app.selectedModel
        ? app.selectedModel
        : null;
  }
  if (!model) {
    return failure(
      "No model is chosen in ChatonLM. Pass a model from chatonlm_models.",
    );
  }
  if (await isCloud(bridge, model, true)) {
    return failure(
      `${oneLine(model, 80)} runs in the cloud. ChatonLM only lets agents use models that run on this computer.`,
    );
  }
  if (signal.aborted) throw new StoppedError();
  const call = bridge.call<SendResult>("chat.send", {
    content: args.question,
    model,
    ephemeral: true,
  });
  let answer = "";
  let error: string | null = null;
  let created: string | null = null;
  const off = bridge.onNotification(({ method, params }) => {
    if (method !== "chat.event") return;
    const p = (params ?? {}) as ChatEventParams;
    if (p.requestId !== call.id || !p.event) return;
    const event = p.event;
    if (event.type === "content" && typeof event.text === "string") {
      answer += event.text;
    } else if (event.type === "error" && typeof event.message === "string") {
      error = event.message;
    } else if (
      event.type === "chatCreated" &&
      typeof event.chatId === "string"
    ) {
      created = event.chatId;
    }
  });
  let result: SendResult | null = null;
  try {
    result = await awaitCall(bridge, call, signal);
  } finally {
    off();
    const chatId = result?.chatId ?? created;
    if (chatId) {
      // Conversation éphémère du pont : effacée, réponse arrêtée d'abord.
      await bridge.call("chat.forget", { chatId }, 5000).result.catch(() => {});
    }
  }
  if (result.outcome === "cancelled") throw new StoppedError();
  if (result.outcome === "error") {
    return failure(error ?? "The model could not answer.");
  }
  return text(answer);
}

const PNG = /^iVBORw0KGgo[A-Za-z0-9+/]*={0,2}$/;

async function generateImage(
  bridge: BridgeLike,
  args: Record<string, string | number>,
  signal: AbortSignal,
): Promise<ToolResult> {
  const params: Record<string, unknown> = { prompt: args.prompt };
  if (args.width !== undefined) params.width = args.width;
  if (args.height !== undefined) params.height = args.height;
  const result = await awaitCall(
    bridge,
    bridge.call<{
      pngBase64?: unknown;
      model?: unknown;
      width?: unknown;
      height?: unknown;
      seed?: unknown;
    }>("image.generate", params),
    signal,
  );
  if (typeof result?.pngBase64 !== "string" || !PNG.test(result.pngBase64)) {
    return failure("ChatonLM did not return a PNG image.");
  }
  const about = [
    typeof result.model === "string"
      ? `model ${oneLine(result.model, 80)}`
      : null,
    typeof result.width === "number" && typeof result.height === "number"
      ? `${result.width}×${result.height}`
      : null,
    typeof result.seed === "number" ? `seed ${result.seed}` : null,
  ].filter((part) => part !== null);
  return {
    content: [
      { type: "image", data: result.pngBase64, mimeType: "image/png" },
      {
        type: "text",
        text: `Image generated by ChatonLM${about.length ? ` (${about.join(", ")})` : ""}.`,
      },
    ],
  };
}

type SpeechEvent = {
  readId?: unknown;
  type?: unknown;
  segmentId?: unknown;
  message?: unknown;
};

const SEGMENT = "agent";

/** Lecture à voix haute ; rendue quand le texte a été lu. */
async function speak(
  bridge: BridgeLike,
  args: Record<string, string | number>,
  signal: AbortSignal,
): Promise<ToolResult> {
  // Évènements gardés dès l'envoi : ils peuvent précéder la réponse.
  const events: SpeechEvent[] = [];
  let wake: (() => void) | null = null;
  const off = bridge.onNotification(({ method, params }) => {
    if (method !== "speech.event") return;
    events.push((params ?? {}) as SpeechEvent);
    wake?.();
  });
  let readId: string | null = null;
  const stop = () => {
    if (readId) {
      void bridge.call("speech.stop", { readId }).result.catch(() => {});
    }
  };
  signal.addEventListener("abort", stop);
  try {
    const params: Record<string, unknown> = {
      segments: [{ id: SEGMENT, text: args.text }],
    };
    if (typeof args.language === "string") params.language = args.language;
    const started = await awaitCall(
      bridge,
      bridge.call<{ readId?: unknown }>("speech.read", params),
      signal,
    );
    if (typeof started?.readId !== "string") {
      return failure("ChatonLM could not read this text aloud.");
    }
    readId = started.readId;
    if (signal.aborted) stop();
    for (;;) {
      const event = events.shift();
      if (!event) {
        if (signal.aborted) throw new StoppedError();
        await new Promise<void>((resolve) => {
          wake = resolve;
          signal.addEventListener("abort", () => resolve(), { once: true });
        });
        wake = null;
        continue;
      }
      if (event.readId !== readId) continue;
      if (event.type === "finished" && event.segmentId === SEGMENT) {
        return text("Read aloud.");
      }
      if (event.type === "stopped") throw new StoppedError();
      if (event.type === "error") {
        return failure(
          typeof event.message === "string"
            ? event.message
            : "ChatonLM could not read this text aloud.",
        );
      }
    }
  } finally {
    off();
    signal.removeEventListener("abort", stop);
  }
}

async function proofread(
  bridge: BridgeLike,
  args: Record<string, string | number>,
  signal: AbortSignal,
): Promise<ToolResult> {
  const result = await awaitCall(
    bridge,
    // `localOnly` : l'app refuse (`notLocal`) un modèle de correction du
    // cloud, distant, ou un Ollama sur une autre machine (contrat 97, A2).
    bridge.call<{ text?: unknown }>("proofread.run", {
      text: args.text,
      mode: args.mode ?? "correct",
      localOnly: true,
    }),
    signal,
  );
  if (typeof result?.text !== "string") {
    return failure("ChatonLM could not proofread this text.");
  }
  return text(result.text);
}

type ModelEntry = {
  model?: unknown;
  parameterSize?: unknown;
  quantizationLevel?: unknown;
  contextLength?: unknown;
  capabilities?: unknown;
};

/** Modèles qui tournent sur cette machine (pas ceux du cloud). */
async function models(
  bridge: BridgeLike,
  signal: AbortSignal,
): Promise<ToolResult> {
  const app = await appState(bridge);
  // Ollama sur une autre machine : aucun modèle ne tourne sur celle-ci.
  if (app.local !== true) return failure(NOT_LOCAL_TEXT);
  const list = await awaitCall(
    bridge,
    bridge.call<ModelEntry[]>("models.list"),
    signal,
  );
  const entries = (Array.isArray(list) ? list : [])
    .filter(
      (m): m is ModelEntry & { model: string } => typeof m?.model === "string",
    )
    .slice(0, 100);
  const checked = await Promise.all(
    entries.map(async (m) => {
      try {
        return (await isCloud(bridge, m.model, true)) ? null : m;
      } catch {
        return null;
      }
    }),
  );
  const local = checked.filter((m) => m !== null);
  const selected =
    typeof app.selectedModel === "string" &&
    local.some((m) => m.model === app.selectedModel)
      ? app.selectedModel
      : null;
  return text(
    JSON.stringify(
      {
        default: selected,
        models: local.map((m) => ({
          name: m.model,
          parameters:
            typeof m.parameterSize === "string" ? m.parameterSize : null,
          quantization:
            typeof m.quantizationLevel === "string"
              ? m.quantizationLevel
              : null,
          contextLength:
            typeof m.contextLength === "number" ? m.contextLength : null,
          capabilities: Array.isArray(m.capabilities)
            ? m.capabilities.filter((c) => typeof c === "string")
            : [],
        })),
      },
      null,
      2,
    ),
  );
}

/** Exécute un outil déjà vérifié. StoppedError : arrêté. */
export function runTool(
  name: ToolName,
  args: Record<string, string | number>,
  bridge: BridgeLike,
  signal: AbortSignal,
): Promise<ToolResult> {
  switch (name) {
    case "chatonlm_ask":
      return ask(bridge, args, signal);
    case "chatonlm_generate_image":
      return generateImage(bridge, args, signal);
    case "chatonlm_speak":
      return speak(bridge, args, signal);
    case "chatonlm_proofread":
      return proofread(bridge, args, signal);
    case "chatonlm_models":
      return models(bridge, signal);
  }
}
