import { type FormEvent, useId, useState } from "react";
import { useT } from "@/i18n";

const message = (cause: unknown) =>
  cause instanceof Error ? cause.message : String(cause);

/**
 * Champ de l'adresse du serveur et son bouton « Save » (écran d'accueil et
 * réglages). Le brouillon part de `saved` ; pour le remettre à jour après un
 * changement venu d'ailleurs, donner au composant une `key` qui change. Aide
 * commune aux deux emplacements : t("chat.server.addressHint").
 */
export function AddressField({
  id,
  saved,
  effective,
  onSave,
  hint,
  describedBy,
}: {
  id: string;
  /** Adresse enregistrée ; vide = automatique. */
  saved: string;
  /** Adresse utilisée, montrée quand le champ est vide. */
  effective: string;
  /**
   * Enregistre l'adresse (chaîne vide : automatique) ; rejette si invalide.
   * Peut renvoyer l'adresse enregistrée (normalisée), qui remplace le texte.
   */
  onSave: (url: string) => Promise<string | void>;
  /** Aide sous le champ, remplacée par l'erreur. */
  hint?: string;
  /** Description donnée ailleurs (ligne des réglages). */
  describedBy?: string;
}) {
  const t = useT();
  const [value, setValue] = useState(saved);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const hintId = useId();
  const unchanged = value.trim() === saved;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    if (unchanged || saving) return;
    setSaving(true);
    setError(null);
    try {
      const stored = await onSave(value.trim());
      if (typeof stored === "string") setValue(stored);
    } catch (cause) {
      setError(message(cause));
    } finally {
      setSaving(false);
    }
  };

  const below = error ?? hint;
  return (
    <form onSubmit={submit} aria-busy={saving || undefined}>
      <div className="flex gap-2">
        <input
          id={id}
          value={value}
          placeholder={effective}
          onChange={(event) => setValue(event.target.value)}
          spellCheck={false}
          autoComplete="off"
          aria-invalid={error ? true : undefined}
          aria-describedby={
            [describedBy, below ? hintId : null].filter(Boolean).join(" ") ||
            undefined
          }
          className="h-9 min-w-0 flex-1 rounded-lg border border-neutral-300 bg-white px-3 text-sm text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 disabled:opacity-50 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100 dark:placeholder:text-neutral-500"
        />
        <button
          type="submit"
          disabled={saving || unchanged}
          className="h-9 rounded-lg border border-neutral-300 px-3 text-sm font-medium text-neutral-800 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-60 dark:border-neutral-700 dark:text-neutral-200 dark:hover:bg-neutral-800"
        >
          {saving ? t("chat.actions.saving") : t("chat.actions.save")}
        </button>
      </div>
      {error ? (
        <p
          id={hintId}
          role="alert"
          className="mt-2 text-xs break-words text-red-600 dark:text-red-400"
        >
          {error}
        </p>
      ) : (
        hint && (
          <p
            id={hintId}
            className="mt-2 text-xs text-neutral-500 dark:text-neutral-400"
          >
            {hint}
          </p>
        )
      )}
    </form>
  );
}
