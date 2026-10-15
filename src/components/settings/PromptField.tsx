// Champ d'une consigne envoyée aux modèles (propre à ChatonLM, sorti de
// VoiceMode.tsx pour le pré-prompt de l'onglet Models) : enregistré en
// quittant le champ, et aussi quand la carte disparaît (autre page) ou avant
// Ctrl+Q sans que le champ soit quitté. Un texte vide ou fait d'espaces, ou
// identique à `defaultText`, est enregistré vide (le moteur prend alors son
// défaut). Un brouillon refusé reste dans le champ. Bouton de remise à zéro
// (vide) sous le champ.

import { PencilSquareIcon } from "@heroicons/react/20/solid";
import {
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { Row, RowError } from "@/components/settings/Row";
import { onBeforeQuit } from "@/lib/quit";

const SMALL_BUTTON =
  "inline-flex flex-none items-center gap-1.5 rounded-lg border border-zinc-950/10 bg-white px-3 py-1 text-sm/6 font-medium text-zinc-950 shadow-sm outline-none hover:bg-zinc-50 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:cursor-default disabled:opacity-50 dark:border-white/15 dark:bg-zinc-900 dark:text-white dark:hover:bg-zinc-800";

export function PromptField({
  id,
  descriptionId,
  label,
  description,
  saved,
  defaultText = "",
  placeholder,
  maxLength,
  resetLabel,
  onSave,
  error,
  ...data
}: {
  id: string;
  descriptionId: string;
  label: ReactNode;
  description: ReactNode;
  /** Texte enregistré ; vide = défaut du moteur. */
  saved: string;
  /** Défaut du moteur : un texte identique est enregistré vide. */
  defaultText?: string;
  placeholder: string;
  maxLength: number;
  resetLabel: string;
  /** Enregistre le texte ; faux s'il est refusé. */
  onSave: (text: string) => Promise<boolean>;
  error?: string;
  /** Attributs `data-*` du champ (essais). */
  [data: `data-${string}`]: string;
}) {
  // null : pas de modification en cours, le champ montre l'enregistré.
  const [draft, setDraft] = useState<string | null>(null);
  const value = draft ?? saved;
  // Texte tapé pas encore envoyé, lu hors du rendu (démontage, Ctrl+Q).
  const unsavedRef = useRef<string | null>(null);
  const latestRef = useRef({ saved, defaultText, onSave });
  useEffect(() => {
    latestRef.current = { saved, defaultText, onSave };
  });
  /** Envoie le texte tapé s'il diffère de l'enregistré ; faux si refusé. */
  const flush = useCallback(async () => {
    const text = unsavedRef.current;
    if (text === null) return true;
    unsavedRef.current = null;
    const latest = latestRef.current;
    const cleaned =
      text.trim() === "" || text.trim() === latest.defaultText.trim()
        ? ""
        : text;
    if (cleaned === latest.saved) return true;
    const done = await latest.onSave(cleaned);
    // Refusé : à renvoyer, sauf si un texte plus récent attend déjà.
    if (!done) unsavedRef.current ??= text;
    return done;
  }, []);
  useEffect(() => onBeforeQuit(flush), [flush]);
  // Démontage (autre page) sans quitter le champ : envoyé quand même.
  useEffect(() => () => void flush(), [flush]);
  const commit = async () => {
    const text = unsavedRef.current;
    if (await flush()) {
      setDraft((current) => (current === text ? null : current));
    }
  };
  return (
    <Row
      icon={PencilSquareIcon}
      label={<label htmlFor={id}>{label}</label>}
      description={description}
      descriptionId={descriptionId}
    >
      <textarea
        id={id}
        {...data}
        aria-describedby={descriptionId}
        rows={6}
        maxLength={maxLength}
        spellCheck={false}
        value={value}
        placeholder={placeholder}
        onChange={(event) => {
          setDraft(event.target.value);
          unsavedRef.current = event.target.value;
        }}
        onBlur={() => void commit()}
        className="mt-3 block w-full resize-y rounded-lg border border-neutral-300 bg-white px-3 py-2 font-mono text-[13px]/5 text-neutral-900 outline-none placeholder:text-neutral-400 focus-visible:border-accent-fg focus-visible:ring-2 focus-visible:ring-accent-fg/40 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-100 dark:placeholder:text-neutral-500"
      />
      <div className="mt-2">
        <button
          type="button"
          className={SMALL_BUTTON}
          disabled={saved === "" && (draft === null || draft === "")}
          onClick={() => {
            unsavedRef.current = null;
            setDraft(null);
            if (saved !== "") void onSave("");
          }}
        >
          {resetLabel}
        </button>
      </div>
      <RowError message={error} />
    </Row>
  );
}
