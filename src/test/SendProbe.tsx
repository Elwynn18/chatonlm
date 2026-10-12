// Composant de test : donne au test l'envoi de l'app (useSendMessage), pour
// ce que la zone de saisie n'envoie pas (message fait d'espaces...).

import { useEffect } from "react";
import { useSendMessage } from "@/hooks/useSendMessage";

export type Send = ReturnType<typeof useSendMessage>["send"];

export function SendProbe({ onSend }: { onSend: (send: Send) => void }) {
  const { send } = useSendMessage();
  useEffect(() => {
    onSend(send);
  }, [onSend, send]);
  return null;
}
