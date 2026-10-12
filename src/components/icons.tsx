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
