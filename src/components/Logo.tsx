// Logo de ChatonLM : la tête de chaton rousse d'assets/logo.svg (mêmes tracés,
// essai Logo.test.tsx), sans son carré crème : les rayures et les yeux sont
// percés et laissent voir le fond, clair ou sombre. Même emploi que le logo de
// l'app officielle (60 px, centré, marge basse), dessin original.

import { useId } from "react";

/** Cadre resserré sur la tête, pour qu'elle remplisse la taille demandée. */
const VIEW_BOX = "48 52 416 416";

export function Logo({
  size = 60,
  className = "mb-8",
}: {
  size?: number;
  className?: string;
}) {
  const mask = useId();
  return (
    <div className={`flex justify-center select-none ${className}`}>
      <svg
        width={size}
        height={size}
        viewBox={VIEW_BOX}
        fill="none"
        aria-hidden="true"
      >
        <mask
          id={mask}
          maskUnits="userSpaceOnUse"
          x="0"
          y="0"
          width="512"
          height="512"
        >
          <rect width="512" height="512" fill="#fff" />
          <path
            d="M230 204V232M256 204V242M282 204V232"
            stroke="#000"
            strokeWidth="13"
            strokeLinecap="round"
          />
          <ellipse cx="192" cy="330" rx="21" ry="28" fill="#000" />
          <ellipse cx="320" cy="330" rx="21" ry="28" fill="#000" />
        </mask>
        <path
          d="M256 182C282 182 300 172 316 152L370 90C384 74 408 80 410 102L424 246C434 272 438 294 438 312C438 388 356 440 256 440C156 440 74 388 74 312C74 294 78 272 88 246L102 102C104 80 128 74 142 90L196 152C212 172 230 182 256 182Z"
          fill="#F28A38"
          mask={`url(#${mask})`}
        />
      </svg>
    </div>
  );
}
