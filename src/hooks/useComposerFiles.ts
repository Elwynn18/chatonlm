// Fichiers joints au prochain message : préparés par le backend (zone de
// préparation, src-tauri/src/attachments : validés, lus et extraits une
// fois), refusés avec le texte officiel, ou en cours de lecture. Code propre
// à ChatonLM : l'officiel lit les octets dans la page et les envoie avec le
// message.

import { useCallback, useEffect, useRef, useState } from "react";
import type { StagedFile } from "@/bindings/StagedFile";
import type { StageResult } from "@/bindings/StageResult";
import { basename, MAX_FILE_SIZE } from "@/lib/attachments";
import {
  stageBytes,
  stageClipboardImage,
  stagePaths,
  unstage,
} from "@/lib/ipc";
import { pastedName } from "@/lib/pastedFiles";

/**
 * Fichier refusé, affiché en rouge jusqu'à ce qu'on le retire. `error` :
 * raison donnée par le moteur ; null : fichier illisible, montré avec le
 * texte officiel (« Error reading file », fileValidation.ts) traduit à
 * l'affichage (chat.files.readError), pour suivre un changement de langue.
 */
export interface FileError {
  filename: string;
  error: string | null;
}

/** Fichier collé que la page n'a pas pu lire. */
interface Unreadable {
  status: "unreadable";
  filename: string;
}

/**
 * Fichier en cours de lecture (un PDF peut demander quelques secondes) :
 * l'envoi attend, sans quoi le message partirait sans lui.
 */
export interface ReadingFile {
  key: number;
  filename: string;
}

let nextReadingKey = 1;

/**
 * Composer affiché (un seul à la fois) : il reçoit les chemins choisis dans
 * le sélecteur natif, même ouvert depuis un composer quitté entre-temps (le
 * sélecteur n'est pas modal sous Linux : on peut changer de conversation).
 */
let activeComposer: { addPaths: (paths: string[]) => void } | null = null;

/** Donne des chemins au composer affiché ; aucun : ils sont ignorés. */
export function addPathsToActiveComposer(paths: string[]) {
  activeComposer?.addPaths(paths);
}

/**
 * Prépare un fichier collé. Trop gros, ses octets ne sont pas lus : sa taille
 * suffit au backend pour le refuser (après l'extension, comme l'officiel).
 * Illisible : refusé sans gêner les autres.
 */
async function stagePasted(
  file: File,
  filename: string,
): Promise<StageResult | Unreadable> {
  try {
    const data =
      file.size > MAX_FILE_SIZE
        ? new Uint8Array(0)
        : new Uint8Array(await file.arrayBuffer());
    return await stageBytes(filename, data, file.size);
  } catch (error) {
    console.error("Failed to read a pasted file:", error);
    return { status: "unreadable", filename };
  }
}

/** Libère des fichiers préparés que plus rien n'enverra. */
export function release(ids: string[]) {
  if (ids.length === 0) return;
  unstage(ids).catch((error: unknown) =>
    console.error("Failed to remove attachments:", error),
  );
}

/**
 * `onStaged` : au moins un fichier vient d'être préparé (pastille ajoutée).
 * `initialFiles` : fichiers déjà préparés que le composer reprend à son
 * montage (brouillon cédé par celui de la nouvelle conversation, mode vocal).
 */
export function useComposerFiles({
  onStaged,
  initialFiles,
}: { onStaged?: () => void; initialFiles?: StagedFile[] } = {}) {
  const [files, setFiles] = useState<StagedFile[]>(initialFiles ?? []);
  const [errors, setErrors] = useState<FileError[]>([]);
  const [reading, setReading] = useState<ReadingFile[]>([]);
  // Lectures d'une image du presse-papiers : sans pastille (rien ne dit
  // encore qu'il y en a une), mais l'envoi les attend aussi.
  const [clipboardReads, setClipboardReads] = useState(0);
  // Copie de `files` pour l'envoi et le démontage, hors du rendu : écrite en
  // même temps que l'état, jamais lue pendant le rendu.
  const filesRef = useRef<StagedFile[]>(initialFiles ?? []);
  const mountedRef = useRef(false);
  // Change à chaque `discard` : une lecture commencée avant n'ajoute plus
  // rien (ses fichiers sont libérés).
  const generationRef = useRef(0);
  const onStagedRef = useRef(onStaged);
  useEffect(() => {
    onStagedRef.current = onStaged;
  });

  const updateFiles = useCallback(
    (change: (files: StagedFile[]) => StagedFile[]) => {
      filesRef.current = change(filesRef.current);
      setFiles(filesRef.current);
    },
    [],
  );

  // Composer quitté (autre conversation, page) : ses fichiers ne partiront
  // plus, comme l'officiel qui vide le formulaire.
  useEffect(() => {
    mountedRef.current = true;
    return () => {
      mountedRef.current = false;
      release(filesRef.current.map((file) => file.id));
      filesRef.current = [];
    };
  }, []);

  /** Résultats d'une préparation : pastilles et erreurs. */
  const receive = useCallback(
    (results: (StageResult | Unreadable)[], generation: number) => {
      const staged = results.flatMap((result) =>
        result.status === "staged" ? [result.file] : [],
      );
      // Lecture finie après le départ du composer, ou commencée avant qu'il
      // soit vidé (Échap, édition) : rien ne les enverra.
      if (!mountedRef.current || generation !== generationRef.current) {
        release(staged.map((file) => file.id));
        return;
      }
      const rejected = results.flatMap((result): FileError[] =>
        result.status === "rejected"
          ? [{ filename: result.filename, error: result.error }]
          : result.status === "unreadable"
            ? [{ filename: result.filename, error: null }]
            : [],
      );
      if (staged.length > 0) {
        updateFiles((all) => [...all, ...staged]);
        onStagedRef.current?.();
      }
      if (rejected.length > 0) {
        setErrors((current) => [...current, ...rejected]);
      }
    },
    [updateFiles],
  );

  /** Lit des fichiers : `stage` les prépare, `filenames` les montre en attendant. */
  const read = useCallback(
    (
      filenames: string[],
      stage: () => Promise<(StageResult | Unreadable)[]>,
    ) => {
      const pending = filenames.map((filename) => ({
        key: nextReadingKey++,
        filename,
      }));
      const keys = new Set(pending.map((file) => file.key));
      const generation = generationRef.current;
      setReading((current) => [...current, ...pending]);
      stage()
        .then(
          (results) => receive(results, generation),
          (error: unknown) => {
            console.error("Failed to add files:", error);
            if (generation !== generationRef.current) return;
            setErrors((current) => [
              ...current,
              ...filenames.map((filename) => ({ filename, error: null })),
            ]);
          },
        )
        .finally(() =>
          setReading((current) => current.filter((f) => !keys.has(f.key))),
        );
    },
    [receive],
  );

  /** Fichiers choisis par leur chemin (sélecteur natif, dépôt). */
  const addPaths = useCallback(
    (paths: string[]) => {
      if (paths.length === 0) return;
      read(paths.map(basename), () => stagePaths(paths));
    },
    [read],
  );

  // Composer affiché : il reçoit les chemins du sélecteur natif.
  useEffect(() => {
    const composer = { addPaths };
    activeComposer = composer;
    return () => {
      if (activeComposer === composer) activeComposer = null;
    };
  }, [addPaths]);

  /** Fichiers collés (octets). */
  const addPasted = useCallback(
    (files: File[]) => {
      if (files.length === 0) return;
      const names = files.map(pastedName);
      read(names, () =>
        Promise.all(files.map((file, i) => stagePasted(file, names[i]))),
      );
    },
    [read],
  );

  /** Image du presse-papiers, que la page ne voit pas (voir pastedFiles). */
  const addClipboardImage = useCallback(() => {
    setClipboardReads((count) => count + 1);
    const generation = generationRef.current;
    stageClipboardImage()
      .then(
        (result) => {
          if (result) receive([result], generation);
        },
        (error: unknown) =>
          console.error("Failed to paste the clipboard image:", error),
      )
      .finally(() => setClipboardReads((count) => count - 1));
  }, [receive]);

  const remove = useCallback(
    (id: string) => {
      updateFiles((all) => all.filter((file) => file.id !== id));
      release([id]);
    },
    [updateFiles],
  );

  const removeError = useCallback((index: number) => {
    setErrors((current) => current.filter((_, i) => i !== index));
  }, []);

  /**
   * Vide le composer : fichiers libérés, erreurs effacées, lectures en cours
   * abandonnées (leurs fichiers seront libérés à leur arrivée).
   */
  const discard = useCallback(() => {
    generationRef.current += 1;
    release(filesRef.current.map((file) => file.id));
    updateFiles(() => []);
    setErrors([]);
    setReading([]);
  }, [updateFiles]);

  /**
   * Fichiers à envoyer, retirés du composer (ils ne seront pas libérés à son
   * démontage : le backend les prend avec le message).
   */
  const take = useCallback((): StagedFile[] => {
    const taken = filesRef.current;
    updateFiles(() => []);
    setErrors([]);
    return taken;
  }, [updateFiles]);

  return {
    files,
    errors,
    reading,
    /** Une lecture est en cours (pastille ou presse-papiers) : l'envoi attend. */
    busy: reading.length > 0 || clipboardReads > 0,
    addPaths,
    addPasted,
    addClipboardImage,
    discard,
    remove,
    removeError,
    take,
  };
}
