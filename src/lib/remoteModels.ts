// Modèles distants (propre à ChatonLM) : erreurs propres aux modèles
// distants (RemoteErrorActions.tsx), noms des sortes de fournisseurs et
// résultat d'une vérification en clair (settings/RemoteModels.tsx), nom
// montré du fournisseur d'un modèle distant (sélecteur), ancien Ollama d'une
// autre machine (réglage « Server address » des versions 0.9).

import type { ChatErrorKind } from "@/bindings/ChatErrorKind";
import type { RemoteCheck } from "@/bindings/RemoteCheck";
import type { RemoteProvider } from "@/bindings/RemoteProvider";
import type { RemoteProviderKind } from "@/bindings/RemoteProviderKind";
import type { TFunction, TKey } from "@/i18n";
import { parseModelName } from "@/lib/modelNames";

/** Fournisseurs enregistrés (cache de React Query). */
export const remoteProvidersKey = ["remoteProviders"] as const;

/** Erreurs propres aux modèles distants. */
export type RemoteErrorKind = Extract<
  ChatErrorKind,
  "remoteOff" | "remoteKey" | "remoteLimit" | "remoteUnreachable"
>;

export function isRemoteError(
  kind: ChatErrorKind | undefined,
): kind is RemoteErrorKind {
  return (
    kind === "remoteOff" ||
    kind === "remoteKey" ||
    kind === "remoteLimit" ||
    kind === "remoteUnreachable"
  );
}

const KIND_NAMES: Record<RemoteProviderKind, TKey> = {
  ollamaCloud: "settings.remoteModels.kinds.ollamaCloud",
  openRouter: "settings.remoteModels.kinds.openRouter",
  mistral: "settings.remoteModels.kinds.mistral",
  custom: "settings.remoteModels.kinds.custom",
};

/** Nom d'une sorte de fournisseur. */
export function kindName(t: TFunction, kind: RemoteProviderKind): string {
  return t(KIND_NAMES[kind]);
}

/** Résultat d'une vérification, en clair. */
export function checkText(t: TFunction, check: RemoteCheck): string {
  if (check.ok) {
    return t("settings.remoteModels.check.ok", { count: check.models });
  }
  switch (check.problem) {
    case "noKey":
      return t("settings.remoteModels.check.noKey");
    case "key":
      return t("settings.remoteModels.check.key");
    case "limit":
      return t("settings.remoteModels.check.limit");
    case "unreachable":
      return t("settings.remoteModels.check.unreachable");
    case "notFound":
      return t("settings.remoteModels.check.notFound");
    case "server":
      return t("settings.remoteModels.check.server", {
        status: check.status ?? 0,
      });
    default:
      return t("settings.remoteModels.check.malformed");
  }
}

/**
 * Nom montré du fournisseur d'un modèle distant (`@remote/<id>/<modèle>`) :
 * le nom réglé de la ligne des fournisseurs (« Serveur du salon »), sinon
 * celui que la liste des modèles donne (`ChatModel.provider`), sinon son
 * identifiant court (« custom-2 ») ; null hors d'un modèle distant.
 */
export function providerName(
  model: string,
  providers: readonly RemoteProvider[] | undefined,
  listed: string | null = null,
): string | null {
  const name = parseModelName(model);
  if (name.kind !== "remote" || name.publisher === null) return null;
  return (
    providers?.find((provider) => provider.id === name.publisher)?.name ||
    listed ||
    name.publisher
  );
}

/** Ancien Ollama d'une autre machine, à ajouter comme serveur compatible. */
export interface FormerServer {
  /** Adresse réglée, telle qu'enregistrée. */
  address: string;
  /** Adresse de son API compatible avec OpenAI (`<origine>/v1`). */
  baseUrl: string;
}

/** Noms d'hôte de cette machine. */
function isLocalHost(host: string): boolean {
  const name = host.toLowerCase().replace(/^\[|\]$/g, "");
  return (
    name === "" ||
    name === "localhost" ||
    name.endsWith(".localhost") ||
    name === "::1" ||
    name === "0.0.0.0" ||
    name === "::" ||
    /^127\./.test(name)
  );
}

/**
 * Adresse de l'Ollama des versions 0.9 (`formerServerUrl`, plus réglable)
 * quand elle désignait une autre machine : son
 * adresse et celle de l'API compatible avec OpenAI qu'Ollama y sert (port
 * 11434 si l'adresse en http n'en donne pas, comme Ollama). Null : vide,
 * illisible, ou cette machine.
 */
export function formerServer(
  formerServerUrl: string | null | undefined,
): FormerServer | null {
  const address = formerServerUrl?.trim() ?? "";
  if (!address) return null;
  let url: URL;
  try {
    url = new URL(
      /^[a-z][a-z0-9+.-]*:\/\//i.test(address) ? address : `http://${address}`,
    );
  } catch {
    return null;
  }
  if (url.protocol !== "http:" && url.protocol !== "https:") return null;
  if (isLocalHost(url.hostname)) return null;
  const port = url.port || (url.protocol === "http:" ? "11434" : "");
  const origin = `${url.protocol}//${url.hostname}${port ? `:${port}` : ""}`;
  return { address, baseUrl: `${origin}/v1` };
}
