// Sélecteur du modèle d'images de la zone de saisie en mode Image (propre à
// ChatonLM ; contrat .tools/research/84-image-chat-contract.md § 4) : à la
// place du sélecteur d'Ollama (ModelPicker.tsx), dont il reprend l'apparence
// et la conduite (bouton de largeur limitée, menu au-dessus, survol, flèches,
// Entrée, Échap consommée, fermeture au clic dehors, même poignée de
// fermeture). Il liste les modèles d'images installés (coche sur le choisi) ;
// en choisir un fait ce que fait « Use » dans l'onglet Images du ModelHub
// (useImageActions, commande image_use_model, modèles relus ensuite). La
// dernière ligne, séparée, mène à cet onglet. Sans champ de recherche, le
// focus va à la liste à l'ouverture et revient au bouton après Échap.

import { CheckIcon } from "@heroicons/react/20/solid";
import { useNavigate } from "@tanstack/react-router";
import {
  type JSX,
  type Ref,
  useCallback,
  useEffect,
  useId,
  useImperativeHandle,
  useRef,
  useState,
} from "react";
import type { ImageModelInfo } from "@/bindings/ImageModelInfo";
import type { ModelPickerHandle } from "@/components/ModelPicker";
import { useImageActions, useImageModels } from "@/hooks/useImageModels";
import { useT } from "@/i18n";
import { errorMessage } from "@/lib/ipc";
import { imageModelLabel } from "@/lib/imageModels";

export function ImageModelPicker({
  onModelSelect,
  onError,
  ref,
}: {
  /** Modèle choisi dans la liste (le menu est déjà fermé). */
  onModelSelect?: () => void;
  /** Choix refusé par le moteur : son message ; `null` : choix réussi. */
  onError?: (message: string | null) => void;
  ref?: Ref<ModelPickerHandle>;
}): JSX.Element {
  const t = useT();
  const [isOpen, setIsOpen] = useState(false);
  const { data, isError } = useImageModels();
  const { use } = useImageActions();
  const installed = (data ?? []).filter((model) => model.installed);
  const selected = installed.find((model) => model.selected) ?? null;
  const dropdownRef = useRef<HTMLDivElement>(null);
  const buttonRef = useRef<HTMLButtonElement>(null);

  const close = useCallback(() => setIsOpen(false), []);
  useImperativeHandle(ref, () => ({ close }), [close]);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  // Échap consommée (la zone de saisie n'arrête pas la réponse), focus rendu
  // au bouton.
  useEffect(() => {
    if (!isOpen) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      if (event.key !== "Escape") return;
      event.preventDefault();
      setIsOpen(false);
      buttonRef.current?.focus();
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isOpen]);

  const handleModelSelect = (model: ImageModelInfo) => {
    close();
    onModelSelect?.();
    if (model.selected) return;
    use.mutate(model.id, {
      onSuccess: () => onError?.(null),
      onError: (error: unknown) => onError?.(errorMessage(error)),
    });
  };

  return (
    <div className="relative" ref={dropdownRef}>
      <button
        ref={buttonRef}
        type="button"
        title={t("chat.image.pickerButton")}
        aria-haspopup="listbox"
        aria-expanded={isOpen}
        onClick={() => setIsOpen(!isOpen)}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            setIsOpen(!isOpen);
          }
        }}
        className="flex max-w-64 cursor-pointer items-center gap-1.5 rounded-full bg-neutral-200 px-3.5 py-1.5 text-neutral-800 select-none focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-neutral-700 dark:text-neutral-100"
      >
        <div className="flex min-w-0 items-center gap-2">
          {data === undefined && !isError ? (
            <span>{t("chat.picker.loading")}</span>
          ) : selected ? (
            <span className="truncate">{imageModelLabel(selected)}</span>
          ) : (
            <span className="truncate">{t("chat.image.chooseModel")}</span>
          )}
        </div>
        <svg
          className="h-3 w-3 flex-none opacity-70"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          viewBox="0 0 24 24"
        >
          <path
            strokeLinecap="round"
            strokeLinejoin="round"
            d="M19 9l-7 7-7-7"
          />
        </svg>
      </button>
      {isOpen && (
        <div className="absolute right-0 bottom-full z-50 mb-2 w-64 overflow-hidden rounded-2xl border border-neutral-100 bg-white text-[15px] text-neutral-800 shadow-xl shadow-black/5 backdrop-blur-lg dark:border-neutral-600/40 dark:bg-neutral-800 dark:text-white dark:ring-black/20">
          <ImageModelList
            models={installed}
            onModelSelect={handleModelSelect}
            onClose={close}
          />
        </div>
      )}
    </div>
  );
}

// Fait défiler la liste jusqu'à une ligne.
function scrollToItem(container: HTMLDivElement | null, index: number) {
  const item = container?.querySelectorAll("[role='option']")[index] as
    HTMLElement | undefined;
  item?.scrollIntoView?.({ block: "nearest" });
}

/**
 * Liste du menu (montée seulement quand il est ouvert) : modèles installés,
 * puis « More image models ». Le focus y va à l'ouverture ; les flèches
 * parcourent toutes les lignes, Entrée active celle mise en évidence.
 */
function ImageModelList({
  models,
  onModelSelect,
  onClose,
}: {
  models: ImageModelInfo[];
  onModelSelect: (model: ImageModelInfo) => void;
  onClose: () => void;
}): JSX.Element {
  const t = useT();
  const navigate = useNavigate();
  const listRef = useRef<HTMLDivElement>(null);
  const [highlightedIndex, setHighlightedIndex] = useState(-1);
  const baseId = useId();
  // Dernière ligne : l'onglet Images du ModelHub.
  const count = models.length + 1;
  const moreIndex = models.length;

  const openHub = useCallback(() => {
    onClose();
    void navigate({ to: "/hub/images" });
  }, [navigate, onClose]);

  useEffect(() => {
    listRef.current?.focus();
  }, []);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      switch (event.key) {
        case "ArrowDown":
          event.preventDefault();
          setHighlightedIndex((prev) => {
            const next = prev < count - 1 ? prev + 1 : 0;
            scrollToItem(listRef.current, next);
            return next;
          });
          break;
        case "ArrowUp":
          event.preventDefault();
          setHighlightedIndex((prev) => {
            const next = prev > 0 ? prev - 1 : count - 1;
            scrollToItem(listRef.current, next);
            return next;
          });
          break;
        case "Enter":
          event.preventDefault();
          if (highlightedIndex === moreIndex) openHub();
          else if (highlightedIndex >= 0 && highlightedIndex < moreIndex) {
            onModelSelect(models[highlightedIndex]);
          }
          break;
      }
    };
    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [count, moreIndex, highlightedIndex, models, onModelSelect, openHub]);

  const optionClass = (index: number, selected: boolean) =>
    `flex w-full cursor-pointer items-center gap-2 px-3 py-2 text-left hover:bg-neutral-100 focus:outline-none dark:hover:bg-neutral-700/60 ${
      highlightedIndex === index || selected
        ? "bg-neutral-100 dark:bg-neutral-700/60"
        : ""
    }`;

  return (
    <div
      ref={listRef}
      role="listbox"
      tabIndex={-1}
      aria-label={t("chat.image.models")}
      aria-activedescendant={
        highlightedIndex >= 0 ? `${baseId}-${highlightedIndex}` : undefined
      }
      className="outline-none"
    >
      <div className="max-h-64 overflow-x-hidden overflow-y-auto">
        {models.length === 0 ? (
          <div className="px-3 py-2 text-neutral-500 dark:text-neutral-400">
            {t("chat.image.pickerEmpty")}
          </div>
        ) : (
          models.map((model, index) => (
            <div
              key={model.id}
              id={`${baseId}-${index}`}
              role="option"
              aria-selected={model.selected}
              title={imageModelLabel(model)}
              onClick={() => onModelSelect(model)}
              onMouseEnter={() => setHighlightedIndex(index)}
              className={optionClass(index, model.selected)}
            >
              <span className="min-w-0 flex-1 truncate">
                {imageModelLabel(model)}
              </span>
              {model.selected && (
                <CheckIcon
                  className="h-4 w-4 flex-none text-accent-fg"
                  aria-hidden="true"
                />
              )}
            </div>
          ))
        )}
      </div>
      <div
        id={`${baseId}-${moreIndex}`}
        role="option"
        aria-selected={false}
        onClick={openHub}
        onMouseEnter={() => setHighlightedIndex(moreIndex)}
        className={`border-t border-neutral-100 dark:border-neutral-700 ${optionClass(moreIndex, false)}`}
      >
        {t("chat.image.pickerMore")}
      </div>
    </div>
  );
}
