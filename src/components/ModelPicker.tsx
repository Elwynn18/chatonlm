/*
 * SPDX-License-Identifier: MIT
 * SPDX-FileCopyrightText: Ollama
 * SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
 *
 * Adapted from Ollama (https://github.com/ollama/ollama),
 * file app/ui/app/src/components/ModelPicker.tsx, commit 42e911bc3d05.
 * Modified for ChatonLM: the stale-model check is made by the banner itself
 * (StaleModelBanner.tsx, src/hooks/useOutdatedModels.ts), not by the picker;
 * no cloud models; the selected model keeps its state during a search; Escape is consumed so that it never
 * reaches the rest of the app; typed close handle instead of a method added
 * to the DOM node; a typed name is looked up in the registry 300 ms after the
 * last keystroke (src/hooks/useModels.ts), not at each one; long names are
 * cut at their start so that their tag stays visible, a Hugging Face name
 * (hf.co/...) shown as a title and a tag chip (full name as tooltip); the
 * button has a maximum width; names compared without case (src/lib/models.ts);
 * texts translated into English, Spanish and French (src/i18n).
 */

import { ArrowDownTrayIcon } from "@heroicons/react/24/outline";
import {
  type JSX,
  type Ref,
  type RefObject,
  useCallback,
  useEffect,
  useImperativeHandle,
  useMemo,
  useRef,
  useState,
} from "react";
import { ModelLabel } from "@/components/ModelLabel";
import {
  useRecommendedModels,
  useSelectedModel,
  useUpstreamModel,
} from "@/hooks/useModels";
import { useT } from "@/i18n";
import { sameModel } from "@/lib/modelNames";
import {
  type ModelEntry,
  pickerEntries,
  upstreamCandidate,
} from "@/lib/models";

/** Fermeture du menu par la zone de saisie (un seul menu ouvert à la fois). */
export interface ModelPickerHandle {
  close: () => void;
}

export function ModelPicker({
  chatId,
  onModelSelect,
  onEscape,
  isDisabled,
  ref,
}: {
  chatId?: string;
  onModelSelect?: () => void;
  onEscape?: () => void;
  isDisabled?: boolean;
  ref?: Ref<ModelPickerHandle>;
}): JSX.Element {
  const t = useT();
  const [isOpen, setIsOpen] = useState(false);
  const [searchQuery, setSearchQuery] = useState("");
  const { selected, models, loading, select } = useSelectedModel(chatId);
  const recommended = useRecommendedModels();
  const listed = useMemo(
    () => pickerEntries(models, selected, searchQuery, recommended),
    [models, selected, searchQuery, recommended],
  );
  // Nom tapé qu'aucune ligne ne porte : proposé s'il existe au registre (il
  // sera téléchargé au prochain envoi).
  const upstream = useUpstreamModel(
    upstreamCandidate(listed, searchQuery) ?? "",
  );
  const entries = useMemo(
    () =>
      upstream === null
        ? listed
        : pickerEntries(models, selected, searchQuery, recommended, upstream),
    [listed, upstream, models, selected, searchQuery, recommended],
  );
  const dropdownRef = useRef<HTMLDivElement>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const listRef = useRef<HTMLDivElement>(null);

  // Clear search when closed
  const close = useCallback(() => {
    setIsOpen(false);
    setSearchQuery("");
  }, []);

  useImperativeHandle(ref, () => ({ close }), [close]);

  useEffect(() => {
    const handleClickOutside = (event: MouseEvent) => {
      if (
        dropdownRef.current &&
        !dropdownRef.current.contains(event.target as Node)
      ) {
        setIsOpen(false);
        setSearchQuery("");
      }
    };
    document.addEventListener("mousedown", handleClickOutside);
    return () => document.removeEventListener("mousedown", handleClickOutside);
  }, []);

  // Focus search when opened, scroll to the selected model
  useEffect(() => {
    if (!isOpen) return;
    searchInputRef.current?.focus();
    const index = entries.findIndex(
      (e) => selected !== null && sameModel(e.model, selected.model),
    );
    if (index !== -1) scrollToItem(listRef.current, index);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOpen]);

  // When searching, scroll to top of list
  useEffect(() => {
    if (searchQuery && listRef.current) listRef.current.scrollTop = 0;
  }, [searchQuery]);

  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (!isOpen) return;

      if (event.key === "Escape") {
        event.preventDefault();
        setIsOpen(false);
        setSearchQuery("");
        onEscape?.();
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [isOpen, onEscape]);

  const handleModelSelect = (entry: ModelEntry) => {
    void select(entry.model);
    close();
    onModelSelect?.();
  };

  return (
    <div className="relative" ref={dropdownRef}>
      <button
        type="button"
        title={t("chat.picker.button")}
        aria-haspopup="listbox"
        aria-expanded={isOpen}
        onClick={() => (isOpen ? close() : setIsOpen(true))}
        onKeyDown={(e) => {
          if (e.key === "Enter") {
            e.preventDefault();
            if (isOpen) close();
            else setIsOpen(true);
          }
        }}
        className="flex max-w-64 cursor-pointer items-center gap-1.5 rounded-full bg-neutral-200 px-3.5 py-1.5 text-neutral-800 select-none focus:outline-2 focus:outline-offset-2 focus:outline-accent-fg dark:bg-neutral-700 dark:text-neutral-100"
      >
        <div className="flex min-w-0 items-center gap-2">
          {isDisabled || loading ? (
            <span>{t("chat.picker.loading")}</span>
          ) : selected ? (
            <ModelLabel model={selected.model} />
          ) : (
            <span>{t("chat.picker.none")}</span>
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
          <div className="border-b border-neutral-100 px-1 py-2 dark:border-neutral-700">
            <input
              ref={searchInputRef}
              type="text"
              value={searchQuery}
              onChange={(e) => setSearchQuery(e.target.value)}
              placeholder={t("chat.picker.search")}
              aria-label={t("chat.picker.searchLabel")}
              autoCorrect="off"
              spellCheck={false}
              className="w-full rounded-md border-none border-neutral-200 bg-transparent px-2 py-0.5 outline-none focus:border-neutral-400 dark:border-neutral-600 dark:focus:border-neutral-400"
            />
          </div>

          <ModelList
            listRef={listRef}
            entries={entries}
            selected={selected}
            onModelSelect={handleModelSelect}
          />
        </div>
      )}
    </div>
  );
}

// Scroll an item to the center of the list
function scrollToItem(container: HTMLDivElement | null, index: number) {
  if (!container || index < 0) return;
  const item = container.children[index] as HTMLElement | undefined;
  if (!item) return;
  const containerHeight = container.clientHeight;
  container.scrollTop =
    item.offsetTop - containerHeight / 2 + item.clientHeight / 2;
}

function ModelList({
  listRef,
  entries,
  selected,
  onModelSelect,
}: {
  listRef: RefObject<HTMLDivElement | null>;
  entries: ModelEntry[];
  selected: ModelEntry | null;
  onModelSelect: (entry: ModelEntry) => void;
}): JSX.Element {
  const t = useT();
  const [highlightedIndex, setHighlightedIndex] = useState(-1);
  const isSelected = (entry: ModelEntry) =>
    selected !== null && sameModel(selected.model, entry.model);

  // Handle keyboard navigation
  useEffect(() => {
    const handleKeyDown = (event: KeyboardEvent) => {
      if (entries.length === 0) return;

      switch (event.key) {
        case "ArrowDown":
          event.preventDefault();
          setHighlightedIndex((prev) => {
            const next = prev < entries.length - 1 ? prev + 1 : 0;
            scrollToItem(listRef.current, next);
            return next;
          });
          break;
        case "ArrowUp":
          event.preventDefault();
          setHighlightedIndex((prev) => {
            const next = prev > 0 ? prev - 1 : entries.length - 1;
            scrollToItem(listRef.current, next);
            return next;
          });
          break;
        case "Enter":
          event.preventDefault();
          if (highlightedIndex >= 0 && highlightedIndex < entries.length) {
            onModelSelect(entries[highlightedIndex]);
          }
          break;
      }
    };

    document.addEventListener("keydown", handleKeyDown);
    return () => document.removeEventListener("keydown", handleKeyDown);
  }, [entries, highlightedIndex, onModelSelect, listRef]);

  return (
    <div
      ref={listRef}
      role="listbox"
      aria-label={t("chat.picker.list")}
      className="h-64 overflow-x-hidden overflow-y-auto"
    >
      {entries.length === 0 ? (
        <div className="px-3 py-2 text-neutral-500 dark:text-neutral-400">
          {t("chat.picker.empty")}
        </div>
      ) : (
        entries.map((entry, index) => (
          <button
            key={entry.model}
            type="button"
            role="option"
            aria-selected={isSelected(entry)}
            title={entry.model}
            onClick={() => onModelSelect(entry)}
            onMouseEnter={() => setHighlightedIndex(index)}
            className={`flex w-full cursor-pointer items-center gap-2 px-3 py-2 hover:bg-neutral-100 focus:outline-none dark:hover:bg-neutral-700/60 ${
              highlightedIndex === index || isSelected(entry)
                ? "bg-neutral-100 dark:bg-neutral-700/60"
                : ""
            }`}
          >
            <span className="min-w-0 flex-1 text-left">
              <ModelLabel model={entry.model} showPublisher />
            </span>
            {entry.info === null && (
              <ArrowDownTrayIcon
                className="h-4 w-4 text-neutral-500 dark:text-neutral-400"
                strokeWidth={1.75}
                aria-label={t("chat.picker.notDownloaded")}
              />
            )}
          </button>
        ))
      )}
    </div>
  );
}
