// Sélecteur du modèle propre à une page de ChatonLM (Proofread : réglage
// `proofreadModel` ; Study : `studyModel`) : « Same as chat » (le modèle du
// chat entre parenthèses), puis les modèles installés ; un modèle réglé absent
// de la liste reste montré. Les textes viennent de la page (son espace de
// langue).

import { type Ref, useState } from "react";
import { useModels } from "@/hooks/useModels";
import { useSettings } from "@/hooks/useSettings";
import { errorMessage } from "@/lib/ipc";
import { sameModel } from "@/lib/modelNames";

const SELECT =
  "h-8 max-w-64 min-w-0 rounded-lg border border-neutral-300 bg-white pr-7 pl-2 text-sm text-neutral-900 outline-none focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100";

/** Textes du sélecteur, traduits par la page. */
export interface ModelSelectTexts {
  label: string;
  sameAsChat: (model: string) => string;
  sameAsChatNone: string;
  notInstalled: (name: string) => string;
  saveError: (message: string) => string;
}

export function ModelSettingSelect({
  id,
  setting,
  texts,
  selectRef,
}: {
  id: string;
  setting: "proofreadModel" | "studyModel";
  texts: ModelSelectTexts;
  selectRef?: Ref<HTMLSelectElement>;
}) {
  const { settings, setSettings } = useSettings();
  const models = useModels();
  const [saveError, setSaveError] = useState<string | null>(null);
  const read = models.data !== undefined;
  const names = (models.data ?? []).map((model) => model.model);
  const value = settings?.[setting] ?? "";
  // Même modèle à la casse près : l'option de la liste.
  const listedName =
    value === "" ? "" : names.find((name) => sameModel(name, value));
  const chatModel = settings?.selectedModel ?? "";
  const save = (model: string) => {
    setSaveError(null);
    setSettings({ [setting]: model }).catch((cause: unknown) =>
      setSaveError(errorMessage(cause)),
    );
  };
  return (
    <div className="flex min-w-0 flex-col items-end">
      <div className="flex min-w-0 items-center gap-2">
        <label
          htmlFor={id}
          className="text-sm/6 text-zinc-700 select-none dark:text-zinc-300"
        >
          {texts.label}
        </label>
        <select
          ref={selectRef}
          id={id}
          className={SELECT}
          value={listedName ?? value}
          disabled={!settings}
          onChange={(event) => save(event.target.value)}
        >
          <option value="">
            {chatModel ? texts.sameAsChat(chatModel) : texts.sameAsChatNone}
          </option>
          {listedName === undefined && (
            <option value={value}>
              {read ? texts.notInstalled(value) : value}
            </option>
          )}
          {names.map((name) => (
            <option key={name} value={name}>
              {name}
            </option>
          ))}
        </select>
      </div>
      {saveError && (
        <p role="alert" className="mt-1 text-xs text-red-600 dark:text-red-400">
          {texts.saveError(saveError)}
        </p>
      )}
    </div>
  );
}
