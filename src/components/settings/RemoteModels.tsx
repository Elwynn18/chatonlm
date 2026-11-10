// Modèles distants (Settings › Models, propre à ChatonLM ; remplace les
// modèles cloud d'Ollama) : réglage « Use remote models » (coupé par
// défaut), fournisseurs compatibles avec l'API d'OpenAI (Ollama Cloud,
// OpenRouter, Mistral, autre serveur à son adresse) avec la clé de
// l'utilisateur, vérification de la clé. La clé part vers le moteur et n'en
// revient jamais : la page n'en connaît que les quatre derniers caractères.
// Un Ollama d'une autre machine réglé dans les versions 0.9 (« Server
// address », plus lu depuis le moteur llama.cpp) est rappelé par une phrase
// discrète qui ouvre l'ajout d'un « Other server (OpenAI-compatible) » à son
// adresse, tant qu'il n'est pas ajouté. Une clé enregistrée se retire
// (« Remove key », après confirmation) ; changer l'adresse d'un serveur sans
// taper de clé la retire aussi (le moteur ne la suit jamais vers une autre
// adresse) : le formulaire le dit sous le champ de la clé.

import { CloudIcon, ServerStackIcon } from "@heroicons/react/24/outline";
import { useMutation, useQueryClient } from "@tanstack/react-query";
import { useId, useState } from "react";
import type { RemoteProvider } from "@/bindings/RemoteProvider";
import type { RemoteProviderInput } from "@/bindings/RemoteProviderInput";
import type { RemoteProviderKind } from "@/bindings/RemoteProviderKind";
import { ConfirmDialog } from "@/components/ConfirmDialog";
import { Row, RowError, Switch } from "@/components/settings/Row";
import { modelsKey } from "@/hooks/useModels";
import { useRemoteProviders } from "@/hooks/useRemoteProviders";
import { useSettings } from "@/hooks/useSettings";
import { type TFunction, useT } from "@/i18n";
import {
  checkRemoteProvider,
  deleteRemoteProvider,
  errorMessage,
  saveRemoteProvider,
} from "@/lib/ipc";
import {
  checkText,
  formerServer,
  kindName,
  remoteProvidersKey,
} from "@/lib/remoteModels";

const KINDS: readonly RemoteProviderKind[] = [
  "ollamaCloud",
  "openRouter",
  "mistral",
  "custom",
];

const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";
const PRIMARY_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-700/90 bg-zinc-800 px-3 py-1 text-sm/6 font-medium text-white shadow-sm outline-none hover:bg-zinc-700 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/10 dark:bg-white dark:text-zinc-900 dark:hover:bg-zinc-200";
const FIELD =
  "h-9 w-full max-w-sm rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
const SELECT =
  "h-9 w-full max-w-sm rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";
const MUTED = "text-sm/6 text-zinc-500 dark:text-zinc-400";

export function RemoteModels({ onSaved }: { onSaved: () => void }) {
  const t = useT();
  const { settings, setSettings } = useSettings();
  const providers = useRemoteProviders();
  const [settingError, setSettingError] = useState<string | null>(null);
  // Formulaire d'ajout ouvert (null : fermé), avec ses valeurs de départ.
  const [adding, setAdding] = useState<ProviderPreset | null>(null);
  const ids = {
    title: useId(),
    use: useId(),
    useDescription: useId(),
  };
  if (!settings) return null;

  const toggle = async (remoteModels: boolean) => {
    setSettingError(null);
    try {
      await setSettings({ remoteModels });
      onSaved();
    } catch (cause) {
      setSettingError(errorMessage(cause));
    }
  };

  const list = providers.data ?? [];
  // Ollama d'une autre machine réglé en 0.9, pas encore ajouté.
  const former = formerServer(settings.formerServerUrl);
  const formerShown =
    former !== null &&
    providers.isSuccess &&
    !list.some(
      (provider) =>
        provider.kind === "custom" &&
        sameServer(provider.baseUrl ?? "", former.baseUrl),
    );
  return (
    <section
      aria-labelledby={ids.title}
      data-testid="remote-models-settings"
      className="mt-4 overflow-hidden rounded-xl bg-white dark:bg-neutral-800"
    >
      <div className="space-y-4 p-4">
        <h2
          id={ids.title}
          className="text-base/7 font-semibold text-zinc-950 dark:text-white"
        >
          {t("settings.remoteModels.title")}
        </h2>
        <div className="flex items-start justify-between gap-4">
          <Row
            icon={CloudIcon}
            label={<span id={ids.use}>{t("settings.remoteModels.use")}</span>}
            description={t("settings.remoteModels.description")}
            descriptionId={ids.useDescription}
          >
            <RowError message={settingError} />
          </Row>
          <Switch
            label={t("settings.remoteModels.use")}
            checked={settings.remoteModels}
            describedBy={ids.useDescription}
            onChange={(on) => void toggle(on)}
          />
        </div>

        {providers.isError && (
          <p role="alert" className="text-sm text-red-600 dark:text-red-400">
            {t("settings.remoteModels.loadFailed", {
              error: errorMessage(providers.error),
            })}
          </p>
        )}
        {list.length > 0 && (
          <ul
            aria-label={t("settings.remoteModels.providers")}
            className="space-y-3"
          >
            {list.map((provider) => (
              <ProviderItem
                key={provider.id}
                provider={provider}
                onSaved={onSaved}
              />
            ))}
          </ul>
        )}
        {providers.isSuccess && list.length === 0 && adding === null && (
          <p className={MUTED}>{t("settings.remoteModels.empty")}</p>
        )}
        {formerShown && adding === null && (
          <div
            data-testid="remote-former-server"
            className="flex flex-wrap items-center gap-x-3 gap-y-2"
          >
            <p className={`min-w-0 flex-1 break-words ${MUTED}`}>
              {t("settings.remoteModels.formerServer", {
                address: former.address,
              })}
            </p>
            <button
              type="button"
              className={SMALL_BUTTON}
              onClick={() =>
                setAdding({ kind: "custom", baseUrl: former.baseUrl })
              }
            >
              {t("settings.remoteModels.formerServerAdd")}
            </button>
          </div>
        )}
        {adding !== null ? (
          <ProviderForm
            preset={adding}
            onDone={() => {
              setAdding(null);
              onSaved();
            }}
            onCancel={() => setAdding(null)}
          />
        ) : (
          <button
            type="button"
            className={SMALL_BUTTON}
            onClick={() => setAdding({})}
          >
            {t("settings.remoteModels.add")}
          </button>
        )}
      </div>
    </section>
  );
}

/** Après un changement : fournisseurs et sélecteur relus. */
function useRefresh() {
  const queryClient = useQueryClient();
  return () => {
    void queryClient.invalidateQueries({ queryKey: remoteProvidersKey });
    void queryClient.invalidateQueries({ queryKey: modelsKey });
  };
}

function keyText(t: TFunction, provider: RemoteProvider): string {
  if (provider.keyHint !== null) {
    return t("settings.remoteModels.keyHint", { hint: provider.keyHint });
  }
  return t(
    provider.hasKey
      ? "settings.remoteModels.keySaved"
      : "settings.remoteModels.noKey",
  );
}

function ProviderItem({
  provider,
  onSaved,
}: {
  provider: RemoteProvider;
  onSaved: () => void;
}) {
  const t = useT();
  const refresh = useRefresh();
  const [editing, setEditing] = useState(false);
  const [confirming, setConfirming] = useState(false);
  const [confirmingKey, setConfirmingKey] = useState(false);
  const descriptionId = useId();
  const check = useMutation({
    mutationFn: () => checkRemoteProvider(provider.id),
    onSettled: refresh,
  });
  const toggle = useMutation({
    mutationFn: (enabled: boolean) =>
      saveRemoteProvider({
        id: provider.id,
        kind: provider.kind,
        name: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: null,
        enabled,
      }),
    onSuccess: () => onSaved(),
    onSettled: refresh,
  });
  const remove = useMutation({
    mutationFn: () => deleteRemoteProvider(provider.id),
    onSuccess: () => onSaved(),
    onSettled: refresh,
  });
  // Clé retirée (`apiKey: ""`), le reste inchangé.
  const removeKey = useMutation({
    mutationFn: () =>
      saveRemoteProvider({
        id: provider.id,
        kind: provider.kind,
        name: provider.name,
        baseUrl: provider.baseUrl,
        apiKey: "",
        enabled: provider.enabled,
      }),
    onSuccess: () => onSaved(),
    onSettled: refresh,
  });

  if (editing) {
    return (
      <li>
        <ProviderForm
          provider={provider}
          onDone={() => {
            setEditing(false);
            onSaved();
          }}
          onCancel={() => setEditing(false)}
        />
      </li>
    );
  }
  const failure =
    toggle.error ?? remove.error ?? removeKey.error ?? check.error;
  return (
    <li
      data-testid={`remote-provider-${provider.id}`}
      className="rounded-lg border border-neutral-200 p-3 dark:border-neutral-700"
    >
      <div className="flex items-start justify-between gap-4">
        <Row
          icon={ServerStackIcon}
          label={provider.name}
          description={
            provider.kind === "custom" && provider.baseUrl !== null
              ? t("settings.remoteModels.customAt", {
                  address: provider.baseUrl,
                })
              : kindName(t, provider.kind)
          }
          descriptionId={descriptionId}
        >
          <p className={MUTED}>{keyText(t, provider)}</p>
        </Row>
        <Switch
          label={t("settings.remoteModels.enable", { name: provider.name })}
          checked={provider.enabled}
          describedBy={descriptionId}
          disabled={toggle.isPending}
          onChange={(enabled) => toggle.mutate(enabled)}
        />
      </div>
      <div className="mt-3 ml-8 flex flex-wrap gap-2">
        <button
          type="button"
          className={SMALL_BUTTON}
          disabled={check.isPending}
          onClick={() => check.mutate()}
        >
          {t(
            check.isPending
              ? "settings.remoteModels.checking"
              : "settings.remoteModels.check.button",
          )}
        </button>
        <button
          type="button"
          className={SMALL_BUTTON}
          onClick={() => setEditing(true)}
        >
          {t("settings.remoteModels.edit")}
        </button>
        {provider.hasKey && (
          <button
            type="button"
            className={SMALL_BUTTON}
            disabled={removeKey.isPending}
            onClick={() => setConfirmingKey(true)}
          >
            {t("settings.remoteModels.removeKey")}
          </button>
        )}
        <button
          type="button"
          className={SMALL_BUTTON}
          disabled={remove.isPending}
          onClick={() => setConfirming(true)}
        >
          {t("common.actions.remove")}
        </button>
      </div>
      {check.data && (
        <p
          role="status"
          className={`mt-2 ml-8 text-sm ${
            check.data.ok
              ? "text-green-800 dark:text-green-100"
              : "text-red-600 dark:text-red-400"
          }`}
        >
          {checkText(t, check.data)}
        </p>
      )}
      <div className="ml-8">
        <RowError message={failure ? errorMessage(failure) : null} />
      </div>
      {confirming && (
        <ConfirmDialog
          title={t("settings.remoteModels.removeTitle", {
            name: provider.name,
          })}
          message={t("settings.remoteModels.removeText")}
          confirmLabel={t("common.actions.remove")}
          onConfirm={() => {
            setConfirming(false);
            remove.mutate();
          }}
          onCancel={() => setConfirming(false)}
        />
      )}
      {confirmingKey && (
        <ConfirmDialog
          title={t("settings.remoteModels.removeKeyTitle", {
            name: provider.name,
          })}
          message={t("settings.remoteModels.removeKeyText")}
          confirmLabel={t("settings.remoteModels.removeKey")}
          onConfirm={() => {
            setConfirmingKey(false);
            removeKey.mutate();
          }}
          onCancel={() => setConfirmingKey(false)}
        />
      )}
    </li>
  );
}

/** Valeurs de départ d'un ajout (ancien Ollama d'une autre machine). */
interface ProviderPreset {
  kind?: RemoteProviderKind;
  baseUrl?: string;
}

/**
 * Même serveur : adresses égales une fois normalisées comme le moteur
 * (schéma et hôte sans la casse, port par défaut omis, sans « / » ni
 * « /v1 » final) ; une adresse illisible se compare telle quelle.
 */
function sameServer(a: string, b: string): boolean {
  const clean = (text: string) => {
    const trimmed = text.trim();
    let base = trimmed;
    try {
      const url = new URL(trimmed);
      base = `${url.protocol}//${url.host}${url.pathname}`;
    } catch {
      // Adresse illisible : telle quelle.
    }
    return base.replace(/\/+$/, "").replace(/\/v1$/, "");
  };
  return clean(a) === clean(b);
}

/**
 * Ajout (sans `provider`, valeurs de départ `preset`) ou changement d'un
 * fournisseur.
 */
function ProviderForm({
  provider,
  preset = {},
  onDone,
  onCancel,
}: {
  provider?: RemoteProvider;
  preset?: ProviderPreset;
  onDone: () => void;
  onCancel: () => void;
}) {
  const t = useT();
  const refresh = useRefresh();
  const [kind, setKind] = useState<RemoteProviderKind>(
    provider?.kind ?? preset.kind ?? "openRouter",
  );
  const [name, setName] = useState(provider?.name ?? "");
  const [baseUrl, setBaseUrl] = useState(
    provider?.baseUrl ?? preset.baseUrl ?? "",
  );
  const [apiKey, setApiKey] = useState("");
  const ids = {
    kind: useId(),
    name: useId(),
    url: useId(),
    key: useId(),
    keyHelp: useId(),
    error: useId(),
  };
  const save = useMutation({
    mutationFn: (input: RemoteProviderInput) => saveRemoteProvider(input),
    onSuccess: () => {
      setApiKey("");
      onDone();
    },
    onSettled: refresh,
  });
  const custom = kind === "custom";
  // Le moteur ne suit jamais une clé vers une nouvelle adresse : changer
  // l'adresse d'un serveur sans taper de clé retire celle enregistrée.
  const dropsKey =
    provider !== undefined &&
    provider.hasKey &&
    custom &&
    apiKey.trim() === "" &&
    baseUrl.trim() !== "" &&
    !sameServer(baseUrl, provider.baseUrl ?? "");
  const submit = () => {
    const key = apiKey.trim();
    save.mutate({
      id: provider?.id ?? null,
      kind,
      name: name.trim(),
      baseUrl: custom ? baseUrl.trim() : null,
      // Champ vide : la clé enregistrée reste.
      apiKey: key === "" ? null : key,
      enabled: provider?.enabled ?? true,
    });
  };
  return (
    <form
      data-testid="remote-provider-form"
      aria-label={t(
        provider
          ? "settings.remoteModels.form.editTitle"
          : "settings.remoteModels.form.addTitle",
      )}
      className="space-y-3 rounded-lg border border-neutral-200 p-3 dark:border-neutral-700"
      onSubmit={(event) => {
        event.preventDefault();
        submit();
      }}
    >
      {!provider && (
        <div className="space-y-1">
          <label htmlFor={ids.kind} className="block text-sm/6">
            {t("settings.remoteModels.form.kind")}
          </label>
          <select
            id={ids.kind}
            className={SELECT}
            value={kind}
            onChange={(event) =>
              setKind(event.target.value as RemoteProviderKind)
            }
          >
            {KINDS.map((value) => (
              <option key={value} value={value}>
                {kindName(t, value)}
              </option>
            ))}
          </select>
        </div>
      )}
      <div className="space-y-1">
        <label htmlFor={ids.name} className="block text-sm/6">
          {t("settings.remoteModels.form.name")}
        </label>
        <input
          id={ids.name}
          className={FIELD}
          value={name}
          maxLength={60}
          placeholder={custom ? undefined : kindName(t, provider?.kind ?? kind)}
          onChange={(event) => setName(event.target.value)}
        />
      </div>
      {custom && (
        <div className="space-y-1">
          <label htmlFor={ids.url} className="block text-sm/6">
            {t("settings.remoteModels.form.address")}
          </label>
          <input
            id={ids.url}
            className={FIELD}
            type="url"
            inputMode="url"
            autoComplete="off"
            spellCheck={false}
            required
            placeholder={t("settings.remoteModels.form.addressExample")}
            value={baseUrl}
            onChange={(event) => setBaseUrl(event.target.value)}
          />
        </div>
      )}
      <div className="space-y-1">
        <label htmlFor={ids.key} className="block text-sm/6">
          {t("settings.remoteModels.form.key")}
        </label>
        <input
          id={ids.key}
          className={FIELD}
          type="password"
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          value={apiKey}
          aria-describedby={ids.keyHelp}
          onChange={(event) => setApiKey(event.target.value)}
        />
        <p id={ids.keyHelp} className={MUTED}>
          {t(
            provider?.hasKey
              ? "settings.remoteModels.form.keepKey"
              : custom
                ? "settings.remoteModels.form.keyOptional"
                : "settings.remoteModels.form.keyHelp",
          )}
        </p>
        {dropsKey && (
          <p
            data-testid="remote-key-dropped"
            className="text-sm/6 break-words text-red-600 dark:text-red-400"
          >
            {t("settings.remoteModels.form.keyDropped")}
          </p>
        )}
      </div>
      {save.isError && (
        <p
          id={ids.error}
          role="alert"
          className="text-sm break-words text-red-600 dark:text-red-400"
        >
          {errorMessage(save.error)}
        </p>
      )}
      <div className="flex flex-wrap gap-2">
        <button
          type="submit"
          className={PRIMARY_BUTTON}
          disabled={save.isPending}
        >
          {t("settings.remoteModels.form.save")}
        </button>
        <button type="button" className={SMALL_BUTTON} onClick={onCancel}>
          {t("common.actions.cancel")}
        </button>
      </div>
    </form>
  );
}
