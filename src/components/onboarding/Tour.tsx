// Premier démarrage (propre à ChatonLM), second temps : visite guidée de
// l'app, dans l'app elle-même. Quatre étapes ; chacune ouvre la page dont
// elle parle, met en lumière l'élément à connaître (le reste de la fenêtre
// est assombri) et l'explique dans une bulle posée à côté. Montée par la
// route racine tant que le réglage `onboardingDone` est faux ; « Get
// started », « Skip » ou Échap l'enregistrent, et la visite se ferme sur une
// nouvelle conversation.
//
// La barre latérale est ouverte pour la visite (fermée au lancement, elle
// cacherait les entrées montrées) et le reste. Pendant la visite l'app est
// inerte (route racine) : seule la bulle répond, le focus n'en sort pas.
// Un élément introuvable (fenêtre trop étroite, page en échec) : la bulle se
// place au centre, sans zone mise en lumière.

import { useNavigate } from "@tanstack/react-router";
import { useEffect, useId, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useSettings } from "@/hooks/useSettings";
import { useT } from "@/i18n";
import { errorMessage } from "@/lib/ipc";
import { isTabKey } from "@/lib/shortcuts";
import { setSidebarOpen } from "@/lib/sidebar";
import {
  type Box,
  bubblePlace,
  grown,
  SPOT_MARGIN,
  sameBox,
  targetBox,
} from "@/lib/tour";

/** Étapes de la visite, dans l'ordre (quatre au plus). */
const STEPS = [
  { id: "chat", href: "/c/new", targets: ['[data-tour="composer"]'] },
  { id: "hub", href: "/hub", targets: ['nav a[href="/hub"]'] },
  {
    id: "tools",
    href: "/proofread",
    targets: ['nav a[href="/proofread"]', 'nav a[href="/study"]'],
  },
  { id: "settings", href: "/settings", targets: ['nav a[href="/settings"]'] },
] as const;

/** Attente d'un élément à montrer avant de placer la bulle au centre. */
const TARGET_WAIT_MS = 800;

/** Zone mise en lumière et coin de la bulle ; sans zone : bulle au centre. */
interface Placement {
  spot: Box | null;
  bubble: { left: number; top: number } | null;
}

const CENTERED: Placement = { spot: null, bubble: null };

const primaryButton =
  "rounded-full bg-neutral-900 px-4 py-1.5 text-sm font-medium text-white outline-none hover:bg-neutral-700 focus-visible:ring-2 focus-visible:ring-accent-fg focus-visible:ring-offset-2 disabled:opacity-60 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200 dark:focus-visible:ring-offset-neutral-800";

const quietButton =
  "rounded-full px-4 py-1.5 text-sm text-neutral-700 outline-none hover:bg-neutral-100 focus-visible:ring-2 focus-visible:ring-accent-fg disabled:opacity-60 dark:text-neutral-200 dark:hover:bg-neutral-700";

export function Tour() {
  const t = useT();
  const navigate = useNavigate();
  const { setSettings } = useSettings();
  const [index, setIndex] = useState(0);
  // Null : pas encore mesuré (bulle invisible, pour ne pas la voir sauter).
  const [placement, setPlacement] = useState<Placement | null>(null);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const cardRef = useRef<HTMLDivElement>(null);
  const titleId = useId();
  const textId = useId();
  const step = STEPS[index];
  const last = index === STEPS.length - 1;

  // Les entrées montrées sont dans la barre latérale, fermée au lancement.
  useEffect(() => {
    setSidebarOpen(true);
  }, []);

  // Chaque étape ouvre sa page, et son bouton principal reçoit le focus
  // (Entrée avance ; le titre et le texte sont lus avec la boîte).
  useEffect(() => {
    void navigate({ href: step.href });
    cardRef.current?.querySelector<HTMLElement>("[data-default]")?.focus();
  }, [navigate, step]);

  // La zone suit ses éléments à chaque image : barre latérale qui s'ouvre,
  // page qui se charge, fenêtre redimensionnée, zoom.
  useEffect(() => {
    const since = performance.now();
    let frame = 0;
    const measure = () => {
      const target = targetBox(step.targets);
      const card = cardRef.current;
      let next: Placement | null = null;
      if (target && card) {
        const spot = grown(target, SPOT_MARGIN);
        next = {
          spot,
          bubble: bubblePlace(
            spot,
            { width: card.offsetWidth, height: card.offsetHeight },
            { width: window.innerWidth, height: window.innerHeight },
          ),
        };
      } else if (performance.now() - since >= TARGET_WAIT_MS) {
        next = CENTERED;
      }
      if (next) {
        const found = next;
        setPlacement((previous) =>
          previous &&
          sameBox(previous.spot, found.spot) &&
          previous.bubble?.left === found.bubble?.left &&
          previous.bubble?.top === found.bubble?.top
            ? previous
            : found,
        );
      }
      frame = requestAnimationFrame(measure);
    };
    frame = requestAnimationFrame(measure);
    return () => cancelAnimationFrame(frame);
  }, [step]);

  /** Fin ou abandon : enregistré, puis une nouvelle conversation. */
  const finish = async () => {
    if (saving) return;
    setSaving(true);
    setError(null);
    try {
      await setSettings({ onboardingDone: true });
      void navigate({ href: "/c/new" });
    } catch (cause) {
      setError(errorMessage(cause));
      setSaving(false);
    }
  };

  // Échap abandonne ; Tab reste dans la bulle.
  useEffect(() => {
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === "Escape") {
        event.preventDefault();
        cardRef.current
          ?.querySelector<HTMLElement>("[data-tour-close]")
          ?.click();
      } else if (isTabKey(event)) {
        const buttons = [
          ...(cardRef.current?.querySelectorAll<HTMLButtonElement>(
            "button:not(:disabled)",
          ) ?? []),
        ];
        const current = buttons.indexOf(
          document.activeElement as HTMLButtonElement,
        );
        const move = event.shiftKey ? -1 : 1;
        event.preventDefault();
        buttons[(current + move + buttons.length) % buttons.length]?.focus();
      }
    };
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  const spot = placement?.spot ?? null;
  const bubble = placement?.bubble ?? null;

  return createPortal(
    <div
      data-testid="tour"
      data-step={step.id}
      className="fixed inset-0 z-50 select-none"
      onContextMenu={(event) => event.preventDefault()}
    >
      {/* Tout clic hors de la bulle s'arrête ici ; sans zone mise en lumière,
          c'est ce voile qui assombrit la fenêtre. */}
      <div
        className={`absolute inset-0 ${spot ? "" : "bg-black/35 dark:bg-black/55"}`}
      />
      {spot && (
        <div
          data-testid="tour-spot"
          className="pointer-events-none absolute rounded-xl shadow-[0_0_0_2px_var(--color-accent),0_0_0_9999px_rgb(0_0_0/0.35)] transition-[left,top,width,height] duration-200 dark:shadow-[0_0_0_2px_var(--color-accent),0_0_0_9999px_rgb(0_0_0/0.55)]"
          style={{
            left: spot.left,
            top: spot.top,
            width: spot.width,
            height: spot.height,
          }}
        />
      )}
      <div
        ref={cardRef}
        role="dialog"
        aria-modal="true"
        aria-labelledby={titleId}
        aria-describedby={textId}
        className={`absolute w-80 max-w-[calc(100vw-1.5rem)] rounded-2xl border border-neutral-200 bg-white p-5 shadow-xl dark:border-neutral-700 dark:bg-neutral-800 ${
          placement ? "opacity-100" : "opacity-0"
        }`}
        style={
          bubble
            ? { left: bubble.left, top: bubble.top }
            : { left: "50%", top: "50%", transform: "translate(-50%, -50%)" }
        }
      >
        {/* Le focus reste sur le bouton d'une étape à l'autre : la nouvelle
            étape est annoncée par cette zone. */}
        <div aria-live="polite">
          <p className="mb-1 text-xs font-medium text-neutral-500 dark:text-neutral-400">
            {t("onboarding.progress", {
              step: index + 1,
              total: STEPS.length,
            })}
          </p>
          <h2
            id={titleId}
            className="font-rounded text-base font-medium text-neutral-900 dark:text-white"
          >
            {t(`onboarding.steps.${step.id}.title`)}
          </h2>
          <p
            id={textId}
            className="mt-2 text-sm/6 text-neutral-800 dark:text-neutral-100"
          >
            {t(`onboarding.steps.${step.id}.text`)}
          </p>
        </div>
        {error && (
          <p
            role="alert"
            className="mt-3 text-xs break-words text-red-600 dark:text-red-400"
          >
            {t("onboarding.error", { error })}
          </p>
        )}
        <div className="mt-5 flex flex-wrap items-center justify-end gap-2">
          {!last && (
            <button
              type="button"
              data-testid="tour-skip"
              data-tour-close=""
              className={`mr-auto ${quietButton}`}
              disabled={saving}
              onClick={() => void finish()}
            >
              {t("onboarding.actions.skip")}
            </button>
          )}
          {index > 0 && (
            <button
              type="button"
              data-testid="tour-back"
              className={quietButton}
              disabled={saving}
              onClick={() => setIndex(index - 1)}
            >
              {t("onboarding.actions.back")}
            </button>
          )}
          {last ? (
            <button
              type="button"
              data-testid="tour-start"
              data-default=""
              data-tour-close=""
              className={primaryButton}
              disabled={saving}
              onClick={() => void finish()}
            >
              {t("onboarding.actions.start")}
            </button>
          ) : (
            <button
              type="button"
              data-testid="tour-next"
              data-default=""
              className={primaryButton}
              onClick={() => setIndex(index + 1)}
            >
              {t("onboarding.actions.next")}
            </button>
          )}
        </div>
      </div>
    </div>,
    document.body,
  );
}
