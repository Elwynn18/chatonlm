// @vitest-environment node
import { describe, expect, it } from "vitest";
import { commitMessageFrom, compose, fence, truncateDiff } from "./prompts";
import {
  buildReceipt,
  filterPieces,
  historyOf,
  holdsSecret,
  isSecretFile,
  looksRemote,
  maskSecrets,
  type Piece,
  splitDiff,
  unquoteGitPath,
} from "./receipt";

const labels = {
  marker: "[secret removed]",
  lines: (label: string, a: number, b: number) =>
    a === b ? `${label}:${a}` : `${label}:${a}-${b}`,
};

const code = (text: string, file = "src/config.ts", first = 10): Piece => ({
  kind: "selection",
  label: file,
  path: file,
  lines: [first, first + text.split("\n").length - 1],
  text,
});

describe("secret files", () => {
  it.each([
    ".env",
    ".env.local",
    "deploy/.env.production",
    "server.pem",
    "tls/server.key",
    "id_rsa",
    "id_ed25519.pub",
    "cert.p12",
    "cert.pfx",
    ".npmrc",
    ".pypirc",
    "credentials.json",
    "secrets.yaml",
    "C:\\work\\.env",
    ".envrc",
    "deploy/prod.env",
    ".netrc",
    ".git-credentials",
    "infra/terraform.tfvars",
    "prod.tfvars.json",
    "release.jks",
    "app.keystore",
    "kubeconfig",
    "kubeconfig.yaml",
  ])("%s is excluded", (file) => {
    expect(isSecretFile(file)).toBe(true);
  });

  it.each(["src/env.ts", "keyboard.ts", "README.md", "my.environment.ts"])(
    "%s is not",
    (file) => {
      expect(isSecretFile(file)).toBe(false);
    },
  );
});

describe("secret values", () => {
  it.each([
    'const key = "AKIAABCDEFGHIJKLMNOP";',
    "token: ghp_" + "a".repeat(36),
    "OPENAI=sk-" + "A".repeat(24),
    "slack xoxb-1234567890-abcdef",
    'password = "hunter22"',
    '"apiKey": "abcd1234"',
    "  db_token: 'xyzw9876'",
    "API_KEY=abcdef123",
    'accessToken: "abcd1234"',
    "secretKey = 'qwerty12'",
    "STRIPE=sk_live_" + "a1".repeat(12),
    "rk_live_" + "Z9".repeat(12),
    "GH=github_pat_" + "a".repeat(30),
    "GOOGLE=AIza" + "b".repeat(35),
    "GITLAB=glpat-" + "c".repeat(20),
    "DATABASE_URL=postgres://admin:S3cretPw@db.example/prod",
    "url: 'https://user:pa55word@host'",
    "password: hunter2hunter",
    "  db_password = s3cr3t-value",
    "+  token: abcdef0123",
    "OPENAI_KEY=abcdefgh12",
    "PRIVATE_KEY=abcdefgh12",
    'OPENAI_KEY="abcdefgh12"',
    // Formes du terminal (relecture 90, V4)
    'curl -H "Authorization: Bearer abcdefghijklmnop" https://api.x',
    "token eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.sig",
    "mysql -u root -pS3cretPassw0rd mydb",
    "curl -u admin:hunter2hunter https://x",
    "docker login -u me -p s3cretpassword",
    "docker login --password-stdin <<< dckr_pat_abcdefghijkl",
    "huggingface-cli login --token hf_" + "a".repeat(34),
    "npm config set //registry.npmjs.org/:_authToken=npm_" + "b".repeat(36),
    "error: header x-api-key: 0123456789abcdef rejected",
    "aws_secret_access_key = wJalrXUtnFEMI/K7MDENG/bPxRfiCYEXAMPLEKEY",
    "deploy --password hunter2hunter",
  ])("masks %s", (line) => {
    const { text, masked } = maskSecrets(`a\n${line}\nb`, "[x]");
    expect(masked).toEqual([[2, 2]]);
    expect(text.split("\n")[1]).toMatch(/\[x\]$/);
    expect(text).not.toContain(line.trim());
  });

  it("masks a whole private key block", () => {
    const key = [
      "-----BEGIN OPENSSH PRIVATE KEY-----",
      "b3BlbnNzaC1rZXktdjEAAAAA",
      "-----END OPENSSH PRIVATE KEY-----",
    ].join("\n");
    const { text, masked } = maskSecrets(`x\n${key}\ny`, "[x]");
    expect(masked).toEqual([[2, 4]]);
    expect(text).toBe("x\n[x]\n[x]\n[x]\ny");
  });

  it("masks the body of a key selected without its BEGIN line", () => {
    const body = Array.from({ length: 4 }, (_, i) =>
      String.fromCharCode(65 + i).repeat(64),
    );
    const { text, masked } = maskSecrets(
      ["x", ...body, "QUJD==", "y"].join("\n"),
      "[x]",
    );
    expect(masked).toEqual([[2, 6]]);
    expect(text).toBe("x\n[x]\n[x]\n[x]\n[x]\n[x]\ny");
  });

  it("leaves ordinary code alone", () => {
    const source = [
      "const password = input.value;",
      'const tokenizer = "words";',
      "if (secret) {}",
      "password = request.form",
      "token: string",
      "token = None",
      "  password: ${DB_PASSWORD}",
      "max_tokens: 1000",
      "const url = 'https://example.com/a:b@c';",
      "A".repeat(64),
      "B".repeat(64),
      "gcc -pthread -o app main.c",
      "docker run -u 1000:1000 image",
      "mkdir -p build/output",
      "const headers = { Authorization: `Bearer ${token}` };",
      "secrets --password-file=/run/secrets/db",
      "primary_key = id",
    ].join("\n");
    expect(maskSecrets(source, "[x]").masked).toEqual([]);
  });
});

describe("holdsSecret", () => {
  it("tests a whole text line by line, private keys included", () => {
    // Rapport 94 (S3) : un passage de plusieurs lignes, pas une seule ligne.
    expect(
      holdsSecret(
        "Setup\n\nConfigure the database:\npassword: hunter2xyz\n\nThen run it.",
      ),
    ).toBe(true);
    expect(
      holdsSecret(
        "Key:\n-----BEGIN RSA PRIVATE KEY-----\nMIIEow\n-----END RSA PRIVATE KEY-----",
      ),
    ).toBe(true);
    expect(holdsSecret("Explain why the tokenizer runs twice.")).toBe(false);
  });
});

describe("what leaves", () => {
  it("removes secret files and masks secrets for a cloud model", () => {
    const pieces = [
      code("PORT=1\nTOKEN=abcd1234", ".env", 1),
      code('const a = 1;\nconst password = "hunter22";', "src/db.ts", 20),
    ];
    const { pieces: kept, blocked } = filterPieces(pieces, "cloud", labels);
    expect(kept).toHaveLength(1);
    expect(kept[0].text).toBe("const a = 1;\n[secret removed]");
    expect(blocked).toEqual([
      { label: ".env", reason: "secretFile" },
      { label: "src/db.ts:21", reason: "secretValue" },
    ]);
  });

  it("sends everything as is to a local model", () => {
    const pieces = [code("TOKEN=abcd1234", ".env", 1)];
    expect(filterPieces(pieces, "local", labels)).toEqual({
      pieces,
      blocked: [],
    });
  });

  it("drops secret files from a diff and masks the rest", () => {
    const diff = [
      "diff --git a/.env b/.env",
      "+TOKEN=abcd1234",
      "diff --git a/src/a.ts b/src/a.ts",
      '+const password = "hunter22";',
      "+const ok = 1;",
      "",
    ].join("\n");
    const { pieces, blocked } = filterPieces(
      [{ kind: "diff", label: "Staged changes", text: diff }],
      "cloud",
      labels,
    );
    expect(pieces[0].text).toBe(
      [
        "diff --git a/src/a.ts b/src/a.ts",
        "+[secret removed]",
        "+const ok = 1;",
        "",
      ].join("\n"),
    );
    expect(blocked).toEqual([
      { label: ".env", reason: "secretFile" },
      { label: "src/a.ts", reason: "secretValue" },
    ]);
  });

  it("finds the file of a diff section whose path git quoted", () => {
    // Nom non ASCII : git cite le chemin et écrit ses octets en octal.
    const diff = [
      'diff --git "a/caf\\303\\251/.env" "b/caf\\303\\251/.env"',
      '--- "a/caf\\303\\251/.env"',
      '+++ "b/caf\\303\\251/.env"',
      "+TOKEN=abcd1234",
      "diff --git a/old.env b/old.env",
      "deleted file mode 100644",
      "--- a/old.env",
      "+++ /dev/null",
      "-X=1",
      "diff --git a/a b.ts b/a b.ts",
      "+x",
      "",
    ].join("\n");
    expect(splitDiff(diff).map((s) => s.file)).toEqual([
      "café/.env",
      "old.env",
      "a b.ts",
    ]);
    expect(unquoteGitPath('"tab\\there \\"q\\""')).toBe('tab\there "q"');
    const { pieces, blocked } = filterPieces(
      [{ kind: "diff", label: "Staged changes", text: diff }],
      "cloud",
      labels,
    );
    expect(blocked.map((b) => b.label)).toEqual(["café/.env", "old.env"]);
    expect(pieces[0].text).not.toContain("abcd1234");
  });

  it("filters for any destination off this computer, unknown included", () => {
    const pieces = [code("TOKEN=abcd1234", ".env", 1)];
    for (const destination of ["cloud", "remote", "unknown"] as const) {
      expect(filterPieces(pieces, destination, labels).pieces).toEqual([]);
    }
  });

  it("counts the earlier messages that go with the new one", () => {
    const history = historyOf([
      { content: "abcd", thinking: "" },
      {
        content: "efgh",
        thinking: "ij",
        toolCalls: [{ result: "klmn" }, { result: null }],
      },
    ]);
    expect(history).toEqual({ messages: 2, chars: 14 });
    const receipt = buildReceipt("x".repeat(10), [], [], "cloud", 100, history);
    expect(receipt.history).toEqual(history);
    expect(receipt.chars).toBe(10);
    expect(receipt.tokens).toBe(6);
    expect(
      buildReceipt("x", [], [], "local", null, { messages: 0, chars: 0 }),
    ).not.toHaveProperty("history");
  });

  it("builds the receipt: items, characters, estimated tokens", () => {
    const piece = code("abc\ndef");
    const receipt = buildReceipt("x".repeat(10), [piece], [], "local", 4096);
    expect(receipt).toEqual({
      items: [
        {
          kind: "selection",
          label: "src/config.ts",
          path: "src/config.ts",
          lines: [10, 11],
          chars: 7,
        },
      ],
      chars: 10,
      tokens: 3,
      contextLength: 4096,
      destination: "local",
      blocked: [],
    });
  });

  it("guesses a cloud model from its name only as a fallback", () => {
    expect(looksRemote("gpt-oss:120b-cloud")).toBe(true);
    expect(looksRemote("kimi:cloud")).toBe(true);
    expect(looksRemote("qwen3:0.6b")).toBe(false);
  });
});

describe("prompts", () => {
  it("fences code longer than any fence inside it", () => {
    expect(fence("a", "ts")).toBe("```ts\na\n```");
    expect(fence("````\nx\n````", "md")).toBe("`````md\n````\nx\n````\n`````");
  });

  it("puts the instruction first, then each piece under its heading", () => {
    const piece = { ...code("let a;"), language: "ts" };
    expect(
      compose("Explain.", [{ piece, heading: "`src/config.ts`, line 10:" }]),
    ).toBe("Explain.\n\n`src/config.ts`, line 10:\n```ts\nlet a;\n```");
  });

  it("cuts a long diff at a line end", () => {
    const { text, truncated } = truncateDiff("aaaa\nbbbb\ncccc\n", 11);
    expect(text).toBe("aaaa\nbbbb\n");
    expect(truncated).toBe(true);
    expect(truncateDiff("short").truncated).toBe(false);
  });

  it("takes the commit message out of a reply", () => {
    expect(commitMessageFrom("```\nfix: a\n\n- b\n```")).toBe("fix: a\n\n- b");
    expect(commitMessageFrom("<think>hmm</think>\nfeat: x")).toBe("feat: x");
    expect(commitMessageFrom('"chore: y"')).toBe("chore: y");
  });
});
