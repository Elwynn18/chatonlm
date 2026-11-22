// « Erase my data » (propre à ChatonLM) : ce que la page garde en mémoire
// hors des requêtes (magasins de module) et qui vient de l'utilisateur est
// oublié une fois les données effacées : page Proofread (texte, revue, page
// gardée), flux de l'Étude (explications, questions), envois et réponses des
// conversations, brouillons cédés au mode vocal, conversation suivante
// demandée éphémère. Le reste (choix du modèle de la synthèse, état de la
// barre latérale, zoom…) n'est pas une donnée de l'utilisateur.

import { forgetVoiceModeDrafts } from "@/hooks/useVoiceMode";
import { resetProofread } from "@/hooks/useProofread";
import { resetStudyStreams } from "@/hooks/useStudy";
import { setNewChatEphemeral } from "@/lib/ephemeral";
import { forgetStreams } from "@/lib/streams";

export function forgetPageMemory() {
  resetProofread();
  resetStudyStreams();
  forgetStreams();
  forgetVoiceModeDrafts();
  setNewChatEphemeral(false);
}
