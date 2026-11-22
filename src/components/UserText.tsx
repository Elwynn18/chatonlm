// Texte d'un message envoyé : le texte tel qu'écrit (lignes et retraits
// gardés par le whitespace-pre-wrap de la bulle), sauf les blocs de code
// entre ``` et le code en ligne entre accents graves, montrés comme dans les
// réponses (coloration, bouton Copier), comme Discord ou Claude. Le reste
// n'est pas lu comme du Markdown : un « * » ou un « # » restent tels quels.
import { Fragment, memo, useMemo } from "react";
import { CodeBlockView } from "@/components/StreamingMarkdownContent";
import { splitFences, splitInlineCode } from "@/lib/codeFence";

export const UserText = memo(function UserText({
  content,
}: {
  content: string;
}) {
  const parts = useMemo(() => splitFences(content), [content]);
  return parts.map((part, index) =>
    part.kind === "code" ? (
      <div key={index} className="whitespace-normal" data-user-code="">
        <CodeBlockView
          language={part.language}
          code={part.code}
          className="my-2"
        />
      </div>
    ) : (
      <Fragment key={index}>
        {splitInlineCode(part.text).map((inline, j) =>
          inline.kind === "code" ? (
            <code
              key={j}
              className="rounded bg-neutral-100 px-1.5 py-0.5 font-mono text-sm dark:bg-neutral-800"
            >
              {inline.text}
            </code>
          ) : (
            <Fragment key={j}>{inline.text}</Fragment>
          ),
        )}
      </Fragment>
    ),
  );
});
