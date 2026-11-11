// Onglet « Web search » des réglages (propre à ChatonLM : l'app officielle
// passe par ollama.com). La recherche est faite depuis cet ordinateur :
// - présentation : ce que voit qui (ChatonLM rien ; le moteur la requête et
//   l'adresse IP), et le bouton globe de la zone de saisie ;
// - choix du moteur (boutons radio, enregistré aussitôt) : DuckDuckGo par
//   défaut, sans compte ; Brave Search ou Mojeek avec la clé d'API de
//   l'utilisateur ; une instance SearXNG (son adresse). Moteur personnel
//   essayé d'abord, DuckDuckGo en secours (dit sous le choix) ;
// - champ de la clé (ou de l'adresse) du moteur choisi : envoyée au moteur
//   de l'app, jamais relue (la page ne sait que « configurée ou non ») ;
//   « Test » essaie ce moteur seul et dit le résultat ; « Remove key » l'oublie
//   (DuckDuckGo reprend) ; lien vers la page où obtenir une clé.
// Réglage à part (table de la recherche web) : « Reset to defaults » n'y
// touche pas. « Saved » de la page après chaque enregistrement.

import {
  ArrowTopRightOnSquareIcon,
  GlobeAltIcon,
  LockClosedIcon,
} from "@heroicons/react/20/solid";
import { useQuery, useQueryClient } from "@tanstack/react-query";
import { type FormEvent, useEffect, useId, useRef, useState } from "react";
import type { WebSearchCheck } from "@/bindings/WebSearchCheck";
import type { WebSearchEngine } from "@/bindings/WebSearchEngine";
import type { WebSearchEngineChoice } from "@/bindings/WebSearchEngineChoice";
import { RowError } from "@/components/settings/Row";
import { type TKey, useT } from "@/i18n";
import {
  checkWebSearchEngine,
  errorMessage,
  forgetWebSearchSecret,
  setWebSearchEngine,
  webSearchEngine,
} from "@/lib/ipc";
import { openLink } from "@/lib/openLink";

/** Requête du moteur choisi. */
const webSearchEngineKey = ["webSearchEngine"] as const;

/** Noms des moteurs (marques, jamais traduites). */
const NAMES: Record<WebSearchEngine, string> = {
  duckDuckGo: "DuckDuckGo", // i18n-ignore : marque
  brave: "Brave Search", // i18n-ignore : marque
  mojeek: "Mojeek", // i18n-ignore : marque
  searxng: "SearXNG", // i18n-ignore : marque
};

const ENGINES: readonly { engine: WebSearchEngine; description: TKey }[] = [
  {
    engine: "duckDuckGo",
    description: "settings.webSearch.engines.duckDuckGo",
  },
  { engine: "brave", description: "settings.webSearch.engines.brave" },
  { engine: "mojeek", description: "settings.webSearch.engines.mojeek" },
  { engine: "searxng", description: "settings.webSearch.engines.searxng" },
];

/** Où obtenir une clé, ou activer le format JSON de SearXNG. */
const HELP_URLS: Record<Exclude<WebSearchEngine, "duckDuckGo">, string> = {
  brave: "https://brave.com/search/api/",
  mojeek: "https://www.mojeek.com/services/search/web-search-api/",
  searxng: "https://docs.searxng.org/admin/settings/settings_search.html",
};

/** Exemple d'adresse d'une instance locale (pas un texte d'interface). */
const ADDRESS_EXAMPLE = "http://localhost:8080"; // i18n-ignore : exemple d'adresse

const CARD = "rounded-xl bg-white p-4 dark:bg-neutral-800";
const TEXT = "text-sm/6 text-zinc-950 dark:text-white";
const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";
const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const PRIMARY_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-700/90 bg-zinc-800 px-3 py-1 text-sm/6 font-medium text-white shadow-sm outline-none hover:bg-zinc-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 aria-disabled:cursor-default aria-disabled:opacity-50 dark:border-white/10 dark:bg-white dark:text-zinc-900 dark:hover:bg-zinc-200";
const LINK =
  "inline-flex items-center gap-1 rounded-sm text-sm/6 text-accent-fg underline underline-offset-2 outline-none";

function configured(
  choice: WebSearchEngineChoice,
  engine: WebSearchEngine,
): boolean {
  switch (engine) {
    case "duckDuckGo":
      return true;
    case "brave":
      return choice.braveConfigured;
    case "mojeek":
      return choice.mojeekConfigured;
    case "searxng":
      return choice.searxngConfigured;
  }
}

/** `onSaved` : badge « Saved » de la page. */
export function WebSearch({ onSaved }: { onSaved: () => void }) {
  const t = useT();
  const queryClient = useQueryClient();
  const { data, error: loadError } = useQuery({
    queryKey: webSearchEngineKey,
    queryFn: webSearchEngine,
  });
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const legendId = useId();
  // Une réponse peut arriver après la sortie de la page.
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);

  /** Lance un enregistrement ; vrai s'il a réussi. */
  const save = async (
    write: () => Promise<WebSearchEngineChoice>,
  ): Promise<boolean> => {
    setSaving(true);
    setError(null);
    try {
      queryClient.setQueryData(webSearchEngineKey, await write());
      if (mounted.current) onSaved();
      return true;
    } catch (cause) {
      if (mounted.current) setError(errorMessage(cause));
      return false;
    } finally {
      if (mounted.current) setSaving(false);
    }
  };

  if (loadError) return <RowError message={errorMessage(loadError)} />;
  if (!data) return null;
  const engine = data.engine;

  return (
    <div data-testid="web-search-settings" className="space-y-4">
      <section className={CARD}>
        <div className="flex items-start gap-3">
          <LockClosedIcon
            aria-hidden="true"
            className="mt-1 h-5 w-5 flex-none text-black dark:text-neutral-100"
          />
          <div className="min-w-0 space-y-1">
            <h2 className={`${TEXT} font-medium`}>
              {t("settings.webSearch.title")}
            </h2>
            <p className={MUTED}>{t("settings.webSearch.privacy")}</p>
            <p className={MUTED}>{t("settings.webSearch.howTo")}</p>
          </div>
        </div>
      </section>

      <section className={CARD}>
        <div className="flex items-start gap-3">
          <GlobeAltIcon
            aria-hidden="true"
            className="mt-1 h-5 w-5 flex-none text-black dark:text-neutral-100"
          />
          <div className="min-w-0 flex-1">
            <h2 id={legendId} className={`${TEXT} font-medium`}>
              {t("settings.webSearch.engine")}
            </h2>
            <div
              role="radiogroup"
              aria-labelledby={legendId}
              className="mt-2 space-y-2"
            >
              {ENGINES.map(({ engine: value, description }) => (
                <label
                  key={value}
                  className="flex cursor-pointer items-start gap-2"
                >
                  <input
                    type="radio"
                    name="web-search-engine"
                    value={value}
                    checked={engine === value}
                    onChange={() => {
                      if (!saving) void save(() => setWebSearchEngine(value));
                    }}
                    className="mt-1.5 h-4 w-4 flex-none accent-accent-fg"
                  />
                  <span className="min-w-0">
                    <span className={TEXT}>{NAMES[value]}</span>
                    <span className={`block ${MUTED}`}>{t(description)}</span>
                  </span>
                </label>
              ))}
            </div>
            <RowError message={error} />
            {engine !== "duckDuckGo" && (
              <Personal
                key={engine}
                engine={engine}
                isConfigured={configured(data, engine)}
                saving={saving}
                save={save}
              />
            )}
          </div>
        </div>
      </section>
    </div>
  );
}

/** Clé (ou adresse) du moteur personnel choisi, essai, retrait. */
function Personal({
  engine,
  isConfigured,
  saving,
  save,
}: {
  engine: Exclude<WebSearchEngine, "duckDuckGo">;
  isConfigured: boolean;
  saving: boolean;
  save: (write: () => Promise<WebSearchEngineChoice>) => Promise<boolean>;
}) {
  const t = useT();
  const [secret, setSecret] = useState("");
  const [check, setCheck] = useState<WebSearchCheck | null>(null);
  const [checking, setChecking] = useState(false);
  const fieldId = useId();
  const statusId = useId();
  const mounted = useRef(false);
  useEffect(() => {
    mounted.current = true;
    return () => {
      mounted.current = false;
    };
  }, []);
  const address = engine === "searxng";
  const name = NAMES[engine];

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (secret.trim() === "" || saving) return;
    setCheck(null);
    if (await save(() => setWebSearchEngine(engine, secret))) {
      if (mounted.current) setSecret("");
    }
  };

  const test = async () => {
    if (checking) return;
    setChecking(true);
    setCheck(null);
    try {
      const result = await checkWebSearchEngine(engine);
      if (mounted.current) setCheck(result);
    } catch (cause) {
      if (mounted.current)
        setCheck({ ok: false, message: errorMessage(cause) });
    } finally {
      if (mounted.current) setChecking(false);
    }
  };

  return (
    <div className="mt-4 space-y-2 border-t border-zinc-950/5 pt-4 dark:border-white/10">
      <p className={MUTED}>
        {isConfigured
          ? t("settings.webSearch.fallback", { engine: name })
          : t(
              address
                ? "settings.webSearch.waitingAddress"
                : "settings.webSearch.waitingKey",
            )}
      </p>
      <form onSubmit={(event) => void submit(event)}>
        <label htmlFor={fieldId} className={TEXT}>
          {address
            ? t("settings.webSearch.addressLabel")
            : t("settings.webSearch.keyLabel", { engine: name })}
        </label>
        <div className="mt-1 flex flex-wrap gap-2">
          <input
            id={fieldId}
            type={address ? "text" : "password"}
            inputMode={address ? "url" : undefined}
            autoComplete="off"
            autoCapitalize="off"
            spellCheck={false}
            value={secret}
            onChange={(event) => setSecret(event.target.value)}
            placeholder={
              address
                ? isConfigured
                  ? t("settings.webSearch.replaceAddress")
                  : ADDRESS_EXAMPLE
                : isConfigured
                  ? t("settings.webSearch.replaceKey")
                  : undefined
            }
            className="h-9 min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100"
          />
          <button
            type="submit"
            disabled={saving || secret.trim() === ""}
            className={PRIMARY_BUTTON}
          >
            {t("settings.webSearch.save")}
          </button>
        </div>
      </form>
      {isConfigured && (
        <div className="flex flex-wrap items-center gap-2">
          <span className={MUTED}>
            {t(
              address
                ? "settings.webSearch.addressSaved"
                : "settings.webSearch.keySaved",
            )}
          </span>
          <button
            type="button"
            onClick={() => void test()}
            aria-disabled={checking || undefined}
            aria-describedby={check ? statusId : undefined}
            className={SMALL_BUTTON}
          >
            {checking
              ? t("settings.webSearch.testing")
              : t("settings.webSearch.test")}
          </button>
          <button
            type="button"
            disabled={saving}
            onClick={() => {
              setCheck(null);
              void save(() => forgetWebSearchSecret(engine));
            }}
            className={SMALL_BUTTON}
          >
            {t(
              address
                ? "settings.webSearch.removeAddress"
                : "settings.webSearch.removeKey",
            )}
          </button>
        </div>
      )}
      <p
        id={statusId}
        role="status"
        data-testid="web-search-check"
        className={
          check?.ok === false
            ? "text-sm/6 break-words text-red-600 dark:text-red-400"
            : TEXT
        }
      >
        {check?.message ?? ""}
      </p>
      <button
        type="button"
        onClick={() => void openLink(HELP_URLS[engine])}
        className={LINK}
      >
        {address
          ? t("settings.webSearch.searxngHelp")
          : t("settings.webSearch.getKey", { engine: name })}
        <ArrowTopRightOnSquareIcon aria-hidden="true" className="h-4 w-4" />
      </button>
    </div>
  );
}
