// Remplace Mermaid (plusieurs Mo), importé à la demande par Streamdown pour ses
// diagrammes : ChatonLM affiche un bloc ```mermaid comme du code.
export default {
  initialize() {},
  render(): Promise<never> {
    // Jamais montré : les blocs ```mermaid passent par CodeBlock
    // (StreamingMarkdownContent), qui n'appelle pas Mermaid. i18n-ignore
    return Promise.reject(new Error("Mermaid is not included"));
  },
};
