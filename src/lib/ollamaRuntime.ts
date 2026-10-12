// Ollama qui sert l'app : celui de la machine (ou d'une adresse réglée) ou
// l'Ollama intégré au paquet de ChatonLM (ollama_runtime du moteur). Textes de
// la ligne « Ollama » de Settings › General, dans la langue de `t`.
import type { OllamaRuntime } from "@/bindings/OllamaRuntime";
import type { TFunction } from "@/i18n";

/**
 * Lequel sert : « Built-in Ollama 0.40.0 », « Ollama 0.35.1 on this
 * computer », « Ollama 0.6.0 at <adresse> » ; sans version tant qu'il ne
 * répond pas (l'intégré : sa version livrée).
 */
export function servingText(t: TFunction, runtime: OllamaRuntime): string {
  if (runtime.kind === "builtin") {
    const version = runtime.builtinVersion ?? runtime.servedVersion;
    return version
      ? t("settings.ollama.builtin", { version })
      : t("settings.ollama.builtinNoVersion");
  }
  const { servedVersion: version, url } = runtime;
  if (runtime.local) {
    return version
      ? t("settings.ollama.local", { version })
      : t("settings.ollama.localNoVersion");
  }
  return version
    ? t("settings.ollama.remote", { version, url })
    : t("settings.ollama.remoteNoVersion", { url });
}

/**
 * Bouton qui quitte l'intégré choisi (`source` builtin) : l'Ollama que le
 * choix automatique reprendrait (`machineUrl` : adresse réglée dans les
 * réglages ou par OLLAMA_HOST, sinon celle par défaut), nommé par son
 * adresse quand il n'est pas sur cette machine.
 */
export function leaveBuiltinText(
  t: TFunction,
  runtime: Pick<OllamaRuntime, "machineUrl" | "machineLocal">,
): string {
  return runtime.machineLocal
    ? t("settings.ollama.useLocal")
    : t("settings.ollama.useAddress", { url: runtime.machineUrl });
}
