// Outils de la page Tools (propre à ChatonLM) : une entrée par carte, dans
// l'ordre de la page (Study, Proofread, Transcription).

import {
  AcademicCapIcon,
  ChatBubbleBottomCenterTextIcon,
  DocumentCheckIcon,
} from "@heroicons/react/24/outline";
import type { ComponentType, SVGProps } from "react";
import type { TKey } from "@/i18n";

/** Un outil de la page : sa carte et la page qu'elle ouvre. */
export interface Tool {
  /** Nom court : `data-testid="tool-card-<id>"`. */
  id: string;
  /** Page de l'outil (un outil de plus : son chemin ici). */
  to: "/study" | "/proofread" | "/transcription";
  icon: ComponentType<SVGProps<SVGSVGElement>>;
  /** Nom de l'outil : le titre de sa page. */
  name: TKey;
  /** Une phrase qui dit ce qu'il fait. */
  description: TKey;
}

/**
 * Outils, dans l'ordre des cartes. Un outil de plus : une entrée ici, avec
 * son icône et ses textes (`tools.cards.<id>`).
 */
export const TOOLS: readonly Tool[] = [
  {
    id: "study",
    to: "/study",
    icon: AcademicCapIcon,
    name: "study.title",
    description: "tools.cards.study",
  },
  {
    id: "proofread",
    to: "/proofread",
    icon: DocumentCheckIcon,
    name: "proofread.title",
    description: "tools.cards.proofread",
  },
  {
    id: "transcription",
    to: "/transcription",
    icon: ChatBubbleBottomCenterTextIcon,
    name: "transcription.title",
    description: "tools.cards.transcription",
  },
];
