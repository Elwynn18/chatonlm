// Icônes dessinées pour ChatonLM (sans équivalent dans Heroicons).

/** Afficher ou masquer la barre latérale. */
export function SidebarToggleIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <rect x="2.75" y="4" width="18.5" height="16" rx="3.25" />
      <path d="M9 4v16" />
      <path d="M5 8h1.5M5 11h1.5M5 14h1.5" />
    </svg>
  );
}

/** Onde (mode conversation vocale) : cinq traits verticaux. */
export function VoiceWaveIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={2}
      strokeLinecap="round"
      aria-hidden="true"
      className={className}
    >
      <path d="M4 10v4M8 7v10M12 4v16M16 7v10M20 10v4" />
    </svg>
  );
}

/**
 * Chaton-fantôme (conversation éphémère, propre à ChatonLM) : tête de chat
 * aux oreilles pointues, corps de fantôme ondulé.
 */
export function GhostCatIcon({ className }: { className?: string }) {
  return (
    <svg
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.6}
      strokeLinecap="round"
      strokeLinejoin="round"
      aria-hidden="true"
      className={className}
    >
      <path d="M5 20V10.5C5 9.6 5.2 8.8 5.6 8.1L6.3 3.6L9.6 6.1C10.4 5.9 11.2 5.8 12 5.8S13.6 5.9 14.4 6.1L17.7 3.6L18.4 8.1C18.8 8.8 19 9.6 19 10.5V20Q17.83 18.2 16.67 20Q15.5 21.8 14.33 20Q13.17 18.2 12 20Q10.83 21.8 9.67 20Q8.5 18.2 7.33 20Q6.17 21.8 5 20Z" />
      <circle cx="9.6" cy="11.8" r="1.05" fill="currentColor" stroke="none" />
      <circle cx="14.4" cy="11.8" r="1.05" fill="currentColor" stroke="none" />
      <path d="M11.2 14.3L12 14.9L12.8 14.3" />
    </svg>
  );
}
