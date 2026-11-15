// Texte montré de l'erreur d'un appel d'outil (propre à ChatonLM). Le moteur
// donne au modèle un texte anglais (celui de l'officiel), enregistré avec
// l'appel, et à la page un code (et le statut HTTP d'une page refusée) : la
// page montre la phrase de ce code dans sa langue. Sans code (conversation
// d'avant les codes, erreur de l'image déjà dans la langue du moment ; un code
// enregistré par une version plus récente est relu sans code par le moteur) :
// le texte enregistré, tel quel.
import type { ToolCall } from "@/bindings/ToolCall";
import type { ToolCallErrorCode } from "@/bindings/ToolCallErrorCode";
import type { TFunction, TKey } from "@/i18n";

/** Phrase de chaque code. */
const ERROR_TEXTS: Record<ToolCallErrorCode, TKey> = {
  accessDenied: "chat.tools.errors.accessDenied",
  pageNotFound: "chat.tools.errors.pageNotFound",
  tooManyRequests: "chat.tools.errors.tooManyRequests",
  serverError: "chat.tools.errors.serverError",
  httpStatus: "chat.tools.errors.httpStatus",
  timeout: "chat.tools.errors.timeout",
  network: "chat.tools.errors.network",
  unknownHost: "chat.tools.errors.unknownHost",
  privateAddress: "chat.tools.errors.privateAddress",
  invalidUrl: "chat.tools.errors.invalidUrl",
  tooManyRedirects: "chat.tools.errors.tooManyRedirects",
  unsupportedContent: "chat.tools.errors.unsupportedContent",
  tooLarge: "chat.tools.errors.tooLarge",
  urlNotProvided: "chat.tools.errors.urlNotProvided",
  searchFailed: "chat.tools.errors.searchFailed",
  invalidQuery: "chat.tools.errors.invalidQuery",
  unknownTool: "chat.tools.errors.unknownTool",
  invalidArguments: "chat.tools.errors.invalidArguments",
  internal: "chat.tools.errors.internal",
};

/** Codes dont la phrase cite le statut HTTP (`{status}`). */
const WITH_STATUS: ReadonlySet<ToolCallErrorCode> = new Set([
  "accessDenied",
  "pageNotFound",
  "serverError",
  "httpStatus",
]);

/**
 * Texte de l'erreur de `call` (« Erreur : … ») ; `null` sans erreur. Sans
 * code, ou avec un code qui cite un statut absent : le texte enregistré.
 */
export function toolCallErrorText(
  t: TFunction,
  call: Pick<ToolCall, "error" | "errorCode" | "errorStatus">,
): string | null {
  if (call.error === null) return null;
  const { errorCode: code, errorStatus: status } = call;
  const shown =
    code === undefined || (WITH_STATUS.has(code) && status === undefined)
      ? call.error
      : t(ERROR_TEXTS[code], status === undefined ? undefined : { status });
  return t("chat.tools.error", { error: shown });
}
