# Security policy

## Supported versions

Only the latest published release of ChatonLM, and the latest published
version of the ChatonLM extension for VS Code, receive security fixes. Fixes
ship in a new release; the app offers it through its update check.

## Reporting a vulnerability

Please do not open a public issue for a security problem. Report it privately,
in one of these ways:

- **GitHub**: the [Security tab](https://github.com/Elwynn18/chatonlm/security)
  of this repository, then **Report a vulnerability**;
- **Email**: contact.chatonlm@gmail.com, with "Security" in the subject.

Include the version, the operating system, the steps to reproduce the problem
and what an attacker could do with it. A proof of concept helps, but is not
required.

ChatonLM is maintained by one person. You can expect a first reply within
about a week; the time needed for a fix depends on the severity and the scope
of the problem. You will be told when a fix is released, and credited in the
release notes if you wish.

## Scope

In scope:

- the desktop app, for Linux and Windows (including its packages, installer
  and update mechanism);
- the local bridge used by the VS Code extension, and its MCP server;
- Easy Link (the Telegram and Discord bot connection);
- the ChatonLM extension for VS Code;
- the service behind the website (`site/server/`).

Out of scope:

- Ollama itself, including the built-in copy shipped with ChatonLM: please
  report those issues to the [Ollama project](https://github.com/ollama/ollama);
- the models, and what they answer;
- Telegram, Discord, Hugging Face, ollama.com and other third-party services.
