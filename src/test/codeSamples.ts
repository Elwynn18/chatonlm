// Échantillons de code des 19 langages du surligneur, pour les tests :
// mots-clés, chaînes, commentaires, nombres, fonctions, balises, attributs,
// imbrications (html → css/js, markdown → js), titres, gras, italique.
export const CODE_SAMPLES: Record<string, string> = {
  javascript: `// Debounce helper
export function debounce(fn, wait = 100) {
  let timer = null;
  return (...args) => {
    clearTimeout(timer);
    timer = setTimeout(() => fn.apply(this, args), wait);
  };
}
const re = /^(?<user>[\\w.]+)@example\\.com$/u;
console.log(\`Hello \${name}!\`, 0x1f, 3.14e-2, null, undefined);
class Foo extends Bar { #priv = 1; static get x() { return this.#priv ?? 42; } }`,
  typescript: `import type { Message } from "./types";

interface ChatState<T extends Message = Message> {
  readonly id: string;
  messages: T[];
  status?: "idle" | "streaming" | "error";
}

export async function load(id: string): Promise<ChatState> {
  const res = await fetch(\`/api/chat/\${encodeURIComponent(id)}\`);
  if (!res.ok) throw new Error(\`HTTP \${res.status}\`);
  return (await res.json()) as ChatState;
}

enum Role { User = "user", Assistant = "assistant" }
type Keys = keyof typeof Role;
const sum = (xs: number[]): number => xs.reduce((a, b) => a + b, 0);`,
  python: `#!/usr/bin/env python3
"""Fetch models from Ollama."""
import json
from dataclasses import dataclass

@dataclass
class Model:
    name: str
    size: int = 0

def list_models(host: str = "http://127.0.0.1:11434") -> list[Model]:
    with urllib.request.urlopen(f"{host}/api/tags", timeout=5) as r:
        data = json.load(r)
    return [Model(m["name"], m.get("size", 0)) for m in data["models"] if m]

if __name__ == "__main__":
    print(*list_models(), sep="\\n")  # done
    lambda x: x ** 2 if x > 0 else None`,
  bash: `#!/usr/bin/env bash
set -euo pipefail
OLLAMA_HOST="\${OLLAMA_HOST:-127.0.0.1:11434}"
for model in "$@"; do
  if ! ollama show "$model" >/dev/null 2>&1; then
    echo "Pulling $model..." >&2
    ollama pull "$model" || exit 1
  fi
done
count=$(ollama list | wc -l)
cat <<EOF
Installed: $count models
EOF
[[ -n "$HOME" && -d ~/.ollama ]] && echo ok`,
  shell: `$ curl -fsSL https://ollama.com/install.sh | sh
$ sudo systemctl enable --now ollama
export PATH="$HOME/.local/bin:$PATH"
ls -la /usr/share/ollama | grep -v '^total' # list`,
  json: `{
  "model": "llama3.2",
  "messages": [{ "role": "user", "content": "Why is the sky blue?" }],
  "stream": true,
  "options": { "temperature": 0.7, "num_ctx": 4096, "stop": null },
  "keep_alive": -1,
  "think": false
}`,
  html: `<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <style>
    body { font-family: system-ui, sans-serif; color: #333; }
    .card:hover > a[href^="http"] { margin: 0 auto !important; }
  </style>
</head>
<body>
  <!-- main content -->
  <div id="app" class="card" data-x='1'>Hello &amp; welcome</div>
  <script type="module">
    const el = document.getElementById("app");
    el.addEventListener("click", () => alert(\`clicked \${Date.now()}\`));
  </script>
</body>
</html>`,
  css: `@import url("fonts.css");
:root { --accent: #4078f2; --radius: 0.5rem; }
@media (prefers-color-scheme: dark) {
  :root:not([data-theme="light"]) { --accent: hsl(210 80% 60% / 0.9); }
}
.btn::after, .btn:focus-visible {
  content: "\\2192";
  transform: translateX(calc(100% - 2px)) rotate(45deg);
  transition: opacity 150ms ease-in-out;
}
@keyframes typing { 0%, 100% { opacity: .2 } 50% { opacity: 1 } }`,
  tsx: `import { useState } from "react";

type Props = { label: string; onCopy?: (text: string) => void };

export function CopyButton({ label, onCopy }: Props) {
  const [copied, setCopied] = useState<boolean>(false);
  return (
    <button
      className="copy-button"
      onClick={() => { onCopy?.(label); setCopied(true); }}
      aria-label={copied ? "Copied" : "Copy"}
    >
      {copied ? <CheckIcon className="h-4 w-4" /> : "Copy"}
    </button>
  );
}`,
  jsx: `import React from "react";

export default function List({ items }) {
  if (!items.length) return <p className="empty">Nothing here</p>;
  return (
    <ul>
      {items.map((it, i) => (
        <li key={it.id ?? i} style={{ color: it.done ? "gray" : "inherit" }}>
          {it.title} &mdash; {i + 1}
        </li>
      ))}
    </ul>
  );
}`,
  go: `package main

import (
	"encoding/json"
	"fmt"
	"net/http"
)

type Tag struct {
	Name string \`json:"name"\`
	Size int64  \`json:"size"\`
}

func main() {
	resp, err := http.Get("http://127.0.0.1:11434/api/tags")
	if err != nil {
		panic(err)
	}
	defer resp.Body.Close()
	var out struct{ Models []Tag }
	_ = json.NewDecoder(resp.Body).Decode(&out)
	fmt.Printf("%d models\\n", len(out.Models))
}`,
  rust: `use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
pub struct Model<'a> {
    pub name: &'a str,
    #[serde(default)]
    pub size: u64,
}

fn count(models: &[Model<'_>]) -> HashMap<String, usize> {
    let mut map = HashMap::new();
    for m in models.iter().filter(|m| m.size > 1_000_000) {
        *map.entry(m.name.to_owned()).or_insert(0) += 1;
    }
    println!("{:?} {}", map, r#"raw "string""#);
    map
}`,
  java: `package com.example;

import java.util.List;
import java.util.stream.Collectors;

/** Simple service. */
public final class ModelService {
    private static final int MAX = 1_000;

    @Override
    public String toString() { return "ModelService"; }

    public List<String> names(List<Model> models) {
        return models.stream()
            .filter(m -> m.size() > 0L)
            .map(Model::name)
            .collect(Collectors.toList());
    }
}`,
  c: `#include <stdio.h>
#include <stdlib.h>
#define MAX(a, b) ((a) > (b) ? (a) : (b))

typedef struct { const char *name; size_t size; } model_t;

static int compare(const void *a, const void *b) {
    const model_t *x = a, *y = b;
    return (x->size > y->size) - (x->size < y->size);
}

int main(int argc, char **argv) {
    model_t models[] = {{"llama", 4u}, {"qwen", 7u}};
    qsort(models, 2, sizeof *models, compare);
    printf("%s %zu\\n", models[0].name, MAX(models[0].size, 0x10));
    return EXIT_SUCCESS; /* done */
}`,
  cpp: `#include <iostream>
#include <vector>
#include <memory>

namespace app {
template <typename T>
class Buffer final {
public:
    explicit Buffer(std::size_t n) : data_(std::make_unique<T[]>(n)), n_(n) {}
    [[nodiscard]] constexpr std::size_t size() const noexcept { return n_; }
    T& operator[](std::size_t i) { return data_[i]; }
private:
    std::unique_ptr<T[]> data_;
    std::size_t n_;
};
}  // namespace app

int main() {
    auto v = std::vector<int>{1, 2, 3};
    for (const auto& x : v) std::cout << x << R"(raw\\n)" << '\\n';
    app::Buffer<double> b{8};
    return static_cast<int>(b.size()) - 8;
}`,
  sql: `-- Messages per chat
WITH recent AS (
  SELECT c.id, c.title, COUNT(m.id) AS n
  FROM chats AS c
  LEFT JOIN messages m ON m.chat_id = c.id
  WHERE c.updated_at > datetime('now', '-7 days')
  GROUP BY c.id
)
SELECT * FROM recent WHERE n > 0 ORDER BY n DESC LIMIT 10;
INSERT INTO settings (key, value) VALUES ('theme', 'dark')
  ON CONFLICT(key) DO UPDATE SET value = excluded.value;
CREATE INDEX IF NOT EXISTS idx_m ON messages(chat_id);`,
  swift: `import Foundation

struct Model: Codable, Identifiable {
    let id = UUID()
    var name: String
    var size: Int64 = 0
}

@MainActor
final class Store: ObservableObject {
    @Published private(set) var models: [Model] = []

    func load() async throws {
        let url = URL(string: "http://127.0.0.1:11434/api/tags")!
        let (data, _) = try await URLSession.shared.data(from: url)
        models = try JSONDecoder().decode([Model].self, from: data)
        print("Loaded \\(models.count) models")
    }
}`,
  yaml: `# GitHub Actions
name: CI
on:
  push:
    branches: [main]
jobs:
  build:
    runs-on: ubuntu-22.04
    env:
      NODE_VERSION: "24"
      DEBUG: false
    steps:
      - uses: actions/checkout@v4
      - run: |
          npm ci
          npm test -- --run
      - name: Anchors
        with: &defaults { retries: 3, timeout: 1.5 }`,
  markdown: `# Title

Some **bold**, *italic*, \`code\` and a [link](https://ollama.com).

- item one
- [x] done

> quote

\`\`\`js
const x = 1;
\`\`\`

\`\`\`ruby
puts "hi"
\`\`\`

| a | b |
|---|---|
| 1 | 2 |`,
};

// Lignes qui passent par les motifs réécrits contre le retour arrière
// exponentiel (highlighterCore.ts, PATTERN_REWRITES) : leurs jetons doivent
// rester ceux des grammaires d'origine.
export const REWRITTEN_PATTERN_SAMPLES: Record<string, string[]> = {
  css: [
    ".sm\\:hover\\:bg-blue-500\\/50:hover { color: red }",
    ".\\31 23abc, .\\abcdef1, .\\abcdef, .\\abcdefg, .\\1234567 { a: b }",
    "svg|a, *|b, |c, ns|el > .x\\2c y { x: y }",
    "#\\@foo\\.bar\\ baz, .a\\0 b { --x\\:y: 1; --é\\31 z: 2 }",
    'a\\ b\\\\c\\"d, .é-ü, ñ|x, :not(.a\\:b, #c\\31 0) { }',
    "@media (min-width: 640px) { .md\\:w-1\\/2 { width: 50% } }",
    "@counter-style x\\31 y { symbols: a } @font-feature-values F\\6f o { }",
    "a { font-variant: annotation(b\\31 c); --v\\ff: var(--w\\ff0000) }",
    ".b\\b\\b\\b\\b\\, #b\\b\\bb\\ { }",
  ],
  go: [
    "var c chan chan<- <-chan int",
    "func f(in <-chan string, out chan<- []*T) (chan <- chan int, error) {}",
    "type S struct { C chan <- chan int; D []chan []int }",
    "type T struct { a chan  <-  chan   int; b, c <-chan *[]T }",
    "x := make(chan chan <- int, 3)",
    "var ch, done chan <- <- chan struct{}",
    'x := struct { Name string `json:"name"` Size int64 `json:"size"` }{}',
    'var cfg = struct{ Host string; Port int; Debug bool }{"a", 1, true}',
    "type T struct { A, B int; C chan<- string; D []*map[string]int }",
    "m := map[string]struct{ a int; b int }{} // }",
    "type Point struct { X, Y float64",
    "type E struct { }a b/}",
  ],
  cpp: [
    "int **p = &*&x; int *& r = *p; void f(int * /*c*/ & x);",
    "#define X(a) a & * b",
    "#define M int *& /**/ x",
    "T operator*(const T& a, T &&b); T& operator&=(T * const* c);",
    "static const unsigned long int * volatile * const p = nullptr;",
    "auto f(int&&... xs) -> decltype(*&xs) { return **&p; }",
  ],
};

// Lignes qui figeaient le moteur JavaScript avant ces réécritures : de 1 s
// à 25 s chacune dans V8 (plus dans WebKitGTK), le temps doublant ou
// quadruplant à chaque unité ajoutée.
export const CATASTROPHIC_LINES: { lang: string; code: string }[] = [
  { lang: "css", code: "b\\".repeat(30) },
  { lang: "html", code: `<style>\n${"b\\".repeat(30)}\n</style>` },
  { lang: "markdown", code: `\`\`\`css\n${"b\\".repeat(26)}\n\`\`\`` },
  { lang: "css", code: `.a${"\\a".repeat(24)}=` },
  { lang: "css", code: `a${"\\abcdef".repeat(8)}` },
  { lang: "css", code: `a[${"é".repeat(24)}]` },
  { lang: "go", code: `func f(a ${"chan ".repeat(24)}x/` },
  { lang: "go", code: `func f(${"chan <- ".repeat(14)}x/` },
  { lang: "go", code: `type T struct {\n\tA ${"chan ".repeat(20)}!\n}` },
  { lang: "go", code: `var x = struct {${"chan ".repeat(6)}/}` },
  { lang: "go", code: `type T struct { }${"a".repeat(24)}/}` },
  { lang: "go", code: 'type Tag struct { Name string `json:"name"`' },
  { lang: "cpp", code: `int ${"*".repeat(20)};` },
  { lang: "cpp", code: `#define X int ${"*".repeat(20)};` },
  { lang: "cpp", code: `#define X(a) int ${"&".repeat(22)};` },
];
