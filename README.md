# ChatonLM

**The tools of local AI in one desktop app, for Windows and Linux.**

ChatonLM is a free, open-source app for using large language models on your
own computer. One window brings together chat, image generation, a model
browser for Hugging Face, dictation, read aloud and voice conversations,
proofreading and step-by-step courses. Models run in a built-in engine,
[llama.cpp](https://github.com/ggml-org/llama.cpp), so it works out of the
box: nothing else to install, no account, no subscription, no telemetry.

ChatonLM is an independent project. Its chat layout and behavior follow the
official Ollama app for Windows and macOS; its look is its own.

Website: [chatonlm.com](https://chatonlm.com), in English, French and Spanish.

![ChatonLM chat window, with a conversation and the chat history](docs/screenshots/chat.webp)

> [!NOTE]
> ChatonLM 0.10 is a beta. Please report problems in
> [Issues](https://github.com/Elwynn18/chatonlm/issues).

## Features

- **Chat**, as in the official app: streaming answers with Markdown, code
  highlighting and math, editable messages, and a history with favorites.
  Reasoning models get a **Think** button, with a thinking level when the
  model supports it. A gauge shows how full the model's context is, and long
  conversations can be compacted, automatically or on demand. A model that
  loops is stopped or flagged.
- **Ephemeral chats**: click the ghost kitten at the top right (or press
  Ctrl+Shift+N) for a chat that stays out of your history and is erased as
  soon as you leave it, or when ChatonLM quits. Keep saves it if you change
  your mind.
- **Attachments**: images (for vision models), PDFs, text and code files, added
  with the + button, by drag and drop, or by pasting.
- **Image generation**: create pictures right in the chat, with the Image
  button or by asking a model that can use tools. A built-in image engine
  ([stable-diffusion.cpp](https://github.com/leejet/stable-diffusion.cpp))
  makes them on your computer, on the graphics card or, more slowly, on the
  processor. Pick free image models from Hugging Face in ModelHub, Images tab
  (FLUX.2 klein, Z-Image and more), with what each one is good at, its speed,
  the graphics memory it needs and its license.
- **ModelHub**: browse open models from Hugging Face, see which ones fit your
  graphics card and memory, then download them (GGUF files) in one click.
  Installed models and downloads have their own tabs. When a newer version of
  an installed model is published, ChatonLM says so and updates it in one
  click. An optional Uncensored section (off by default, in Settings >
  Models) lists popular community versions of official models with their
  refusals removed.
- **Models from Ollama**: if you used Ollama, or ChatonLM 0.9 and its
  built-in Ollama, bring your models over in one click in Settings > Models,
  without downloading them again (see [Engine and models](#engine-and-models)).
- **Web search**: let models search the web, with no account, through
  ChatonLM's own search service (results powered by Brave Search).
- **Remote models** (optional, off by default, in Settings > Models): use
  larger models that run elsewhere, with your own API key: Ollama Cloud,
  OpenRouter, Mistral, or any server compatible with the OpenAI API. They
  appear in the same model picker.
- **Dictation**: press a global shortcut and speak in any app. Speech is
  transcribed on your computer by Parakeet (25 European languages) or Whisper
  (99 languages), optionally cleaned up by a local model, copied to the
  clipboard and pasted where your cursor is.
- **Read aloud and voice mode**: answers can be read aloud by a natural voice
  (English, French or Spanish), and in [voice mode](#voice-mode) you talk
  with the model hands-free: ChatonLM transcribes what you say, sends it, reads
  the answer aloud and listens again. A global shortcut also reads aloud the
  text selected in any app (press it again to stop). Everything runs on your
  computer.
- **Proofread**: correct or rephrase a text in the app, or the text selected
  in any app with a global shortcut, which puts the corrected text in its
  place. On "Auto", it uses the largest model already loaded, or else the
  chat model.
- **Study**: pick a topic, and your model writes a short course with lessons,
  worked examples and quizzes, from your own documents and the web if you
  want. Like proofreading, it can pick its model on "Auto".
- **Models settings**: keep models loaded for as long as you want, preload the
  chosen model, see loaded models and unload them (all of them when you quit,
  unless you turn it off), follow a graphics memory gauge, and set a system
  prompt sent at the start of every chat.
- **Easy Link**: talk to your models from your phone, through your own
  Telegram or Discord bot, and get the pictures they create (see
  [Easy Link](#easy-link)).
- **VS Code extension**: your local models in VS Code and VSCodium, through
  ChatonLM (see [VS Code](#vs-code)).
- **Built-in engine**: no setup needed. Models run in llama.cpp, on the
  graphics card through Vulkan or on the processor, with optional NVIDIA
  acceleration (see [Engine and models](#engine-and-models)).
- **Updates** from Settings, signed and checked before they are installed.
- **English, Spanish and French** interface: you choose the language on first
  launch (the one of your system is proposed) and can change it in Settings.
- **A guided tour on first launch**: four steps that open the main pages and
  point at what each one is for. You can skip it at any time.
- Light and dark themes, a tray icon, and keyboard shortcuts (listed in
  Settings › Keyboard shortcuts).
- **The same tools on Windows and Linux.**

| ModelHub                                                                   | Proofread                                                                                  |
| -------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------ |
| ![ModelHub, with models from Hugging Face](docs/screenshots/modelhub.webp) | ![Proofread page, with the corrections of a text](docs/screenshots/proofread.webp)         |
| **Study**                                                                  | **Models settings**                                                                        |
| ![Study page, creating a course](docs/screenshots/study.webp)              | ![Models settings, with a loaded model, in the dark theme](docs/screenshots/settings.webp) |

## Voice mode

Click the wave button next to Send, or press Ctrl+Shift+Space, to talk with
the model of the conversation. ChatonLM listens, sends your sentence when you
stop speaking, reads the answer aloud, then listens again. The conversation is
written and saved as usual.

- **100% local**: speech detection, transcription, echo cancellation and the
  voice run on your computer. Only the transcribed text is sent to the
  model, like a typed message: it stays on your computer unless you chose a
  remote model.
- **Interrupt by speaking**: echo cancellation removes ChatonLM's own voice
  from what the microphone hears, so you can cut an answer just by talking,
  even on speakers (headphones work best). Space or a click on the bar also
  interrupts; Esc ends voice mode.
- **Settings › Voice mode**: hands-free or push-to-talk, the pause that ends
  your turn, interruption by speaking, and the instructions given to the
  model (short spoken answers, no reasoning unless you allow it).
- **Requirements**: a microphone, the speech recognition model of Dictation
  (Parakeet or Whisper) and the voice of your language (English, French or
  Spanish). When one is missing, ChatonLM offers to download it, with its
  size, then starts.

## Easy Link

Easy Link connects a Telegram or Discord bot that you create to the models of
your computer: you write to the bot from your phone, and your model answers.
The conversations are saved in ChatonLM like the others, marked with a small
icon.

- **Set up in Settings › Easy Link**: create a bot (with @BotFather on
  Telegram, or in the Discord developer portal), paste its token, then link
  your account with the 6-digit pairing code that ChatonLM shows. Only linked
  accounts are answered; you can link several and unlink them at any time.
- **Commands**: `/new`, `/stop`, `/models`, `/model`, `/load`, `/unload`,
  `/status`, `/web on` and `/web off`, `/ephemeral on` and `/ephemeral off`,
  `/keep`, `/help` (on Discord, `!models` also works). Models cannot be downloaded or deleted from the bot, and settings
  cannot be changed.
- **Photos and voice messages**: a photo goes to a model that can see; a voice
  message is transcribed on your computer by the speech recognition model of
  Dictation, then sent as text.
- **Pictures**: ask the bot for a picture, and if the model can use tools and
  an image model is chosen in ChatonLM, the image arrives as a photo.
- **Web search**, with `/web on`.
- **Ephemeral mode**, with `/ephemeral on`: the bot's conversations stay out
  of ChatonLM's history and are erased by `/new`, `/ephemeral off` or when
  ChatonLM quits; `/keep` saves the current one.
- **ChatonLM must be running** on your computer (by default, it keeps running
  in the tray when you close the window). Nothing is hosted elsewhere, and no port is
  opened on your computer.
- **On Discord**, the bot answers private messages only, and Discord lets you
  write to a bot only if you share a server with it: ChatonLM gives you the
  link that invites your bot to one of your servers.

Your messages and the answers travel through the servers of Telegram or
Discord (see [Privacy](#privacy)).

## VS Code

The ChatonLM extension brings the models of the app into VS Code: explain, fix
and test code, write commit messages, guided tours of a file or of your changes
read aloud, hands-free voice and dictation, local autocomplete, and more. Install
it from the [Visual Studio Marketplace](https://marketplace.visualstudio.com/items?itemName=chatonlm.chatonlm)
or, for VSCodium and other editors, from [Open VSX](https://open-vsx.org/extension/chatonlm/chatonlm),
then pair it in **Settings › VS Code**. It talks to ChatonLM through a link that
stays on your computer and has no telemetry. Details: [vscode/README.md](vscode/README.md).

## Install

ChatonLM runs on 64-bit PCs (x86-64), on Windows 11 and on Linux. Download
the files from the
[latest release](https://github.com/Elwynn18/chatonlm/releases/latest) or
from [chatonlm.com](https://chatonlm.com).

### Windows

For Windows 11, 64-bit. Run `chatonlm_<version>_x64-setup.exe`: ChatonLM
installs for your user account, without administrator rights.

- The installer doesn't have a certificate that Windows recognizes yet, so
  Windows shows a "Windows protected your PC" screen: click **More info**,
  then **Run anyway**. Updates are signed by ChatonLM and checked before they
  are installed (see [Updates](#updates)).
- ChatonLM uses Microsoft Edge WebView2, which is part of Windows 11. If it
  is missing, the installer downloads it.

### Linux

ChatonLM needs glibc 2.35 or later: Ubuntu 22.04, Debian 12, Fedora 36 and
later, and most current distributions. The release packages are tested on
Ubuntu 22.04 and 24.04 and on Fedora 44, and ChatonLM is developed on Arch
Linux.

#### AppImage (recommended)

The AppImage runs on any distribution, without installation:

```bash
chmod +x chatonlm_*_amd64.AppImage
./chatonlm_*_amd64.AppImage
```

- Keep it in a folder of your own, for example `~/Applications`: ChatonLM
  updates itself by replacing the file, so the folder must be writable.
- On its first start, ChatonLM adds itself to your applications menu. If you
  move the AppImage, it updates the menu entry the next time you start it. You
  can remove the entry in Settings › General › Applications menu.
- The AppImage needs FUSE, which most desktop distributions include. If it
  doesn't start for that reason, install your distribution's `fuse3` package,
  or start it without FUSE:

  ```bash
  APPIMAGE_EXTRACT_AND_RUN=1 ./chatonlm_*_amd64.AppImage
  ```

#### Debian and Ubuntu (.deb)

For Ubuntu 22.04 or later, Debian 12 or later, and their derivatives:

```bash
sudo apt install ./chatonlm_*_amd64.deb
```

### Requirements

- **Graphics card (optional).** The built-in engine runs models on the
  graphics card through Vulkan when a Vulkan driver is installed (your
  card's driver on Windows; on Linux, your card's driver and the Vulkan
  loader, `libvulkan1` on Debian and Ubuntu), and on the processor otherwise.
  On an NVIDIA GeForce RTX 20 series card or newer, with driver 580 or later,
  you can also turn on NVIDIA acceleration: ChatonLM then downloads its CUDA 13
  pack, after showing you NVIDIA's license (see
  [Engine and models](#engine-and-models)).
- **Dictation with Whisper** needs a processor with AVX2 (x86-64-v3). Without
  it, ChatonLM offers only Parakeet, which works on any x86-64 processor.
- **Global shortcuts** (dictation, proofreading, reading the selection
  aloud). On Windows, you choose the keys in Settings › Keyboard shortcuts
  (Ctrl+Alt+D, Ctrl+Alt+C and Ctrl+Alt+R by default). On Linux, they use the
  desktop's GlobalShortcuts portal: KDE Plasma 6 or GNOME 48 and later; on
  other desktops, bind `chatonlm --dictate`, `chatonlm --proofread` and
  `chatonlm --read-aloud` to keys in your desktop's own shortcut settings.
- **Pasting for you**: the dictated text where your cursor is, the corrected
  text over the selection. On Linux, ChatonLM sends the paste keystroke
  through the desktop's RemoteDesktop portal (KDE Plasma, GNOME), which asks
  for your permission the first time; on other desktops the text is copied,
  and you paste it with Ctrl+V. On Windows, it can't reach an app started as
  administrator. Turn it off in Settings › Dictation or Settings › Keyboard
  shortcuts to only copy the text.
- **Tray icon**: the notification area on Windows; on Linux, built into KDE
  Plasma and most desktops, while GNOME needs the AppIndicator extension.
  Closing the window hides ChatonLM in the tray; turn off "Keep running in the
  system tray" in Settings › General to quit instead. Without a tray, closing
  the window quits ChatonLM.

## Engine and models

ChatonLM runs models itself, with `llama-server` from
[llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT), included in the
packages. There is no server to install or start, and ChatonLM no longer
connects to an Ollama server. The engine listens only on 127.0.0.1, on a
private port, and stops when you quit ChatonLM. Settings › General › Engine
shows its version and whether models run on the graphics card or on the
processor.

- **Graphics card**: Vulkan by default, on the processor when there is no
  usable graphics card.
- **NVIDIA acceleration** (optional): for NVIDIA GeForce RTX 20 series cards
  and newer, with driver 580 or later, in Settings › General › Engine.
  ChatonLM shows NVIDIA's license, then downloads its CUDA 13 pack: its own
  build of llama.cpp's CUDA backend, from this repository's releases, and
  NVIDIA's CUDA libraries, from NVIDIA. Nothing is downloaded until you ask.
- **Models** come from Hugging Face, as GGUF files, through ModelHub. They are
  stored in the `models` folder of [ChatonLM's data](#your-data); choose
  another folder in Settings › General › Model location, which can move your
  models there.
- **Models from Ollama**: Settings › Models › Models from Ollama finds the
  models of the Ollama installed on your computer and of the built-in Ollama
  of ChatonLM 0.9, and copies them in one click, without downloading them
  again. The installed Ollama is not changed: its models are copied, and
  nothing in its folder is deleted. Ollama's cloud models have nothing to
  import, and a model stored in a format of Ollama's own has to be downloaded
  again from Hugging Face. Once the models of the built-in Ollama are
  imported, ChatonLM offers to delete its old folder to free up space.
- **Remote models** (off by default): in Settings › Models › Remote models,
  add a provider with your own API key: Ollama Cloud, OpenRouter, Mistral, or
  any server compatible with the OpenAI API (for example an Ollama on another
  computer). Its models then appear in the model picker, marked as remote.
  The keys are kept in ChatonLM's data folder, readable by your user only,
  and never written to the logs. Dictation never uses a remote model.

## Updates

ChatonLM checks for a new version when it starts, then every hour, from this
repository's GitHub releases. With **Auto-download updates** on (the
default), it downloads the update in the background; it is installed when you
click **Restart to update**, never on its own.

- Every update is signed. ChatonLM checks the signature, the version and the
  package type before installing, and refuses anything else.
- **Windows**: ChatonLM runs the new installer itself; no administrator
  rights are needed.
- **AppImage**: the new file replaces the old one in the same folder; no
  password is needed.
- **.deb**: your system asks for your password to install the package.
- A copy that ChatonLM doesn't manage (installed by a distribution's package
  manager, Flatpak or Snap, or built from source) never checks for updates;
  the Updates card explains why. Updates then come through your package
  manager.

## Privacy

ChatonLM has no account of its own, no telemetry and no analytics. Your chats,
attachments, generated images and settings stay on your computer, dictation
and voice mode are transcribed locally, and images are generated locally by
the built-in image engine, which does not connect to the internet.

ChatonLM itself connects only to:

- **Hugging Face** (huggingface.co and its status page), when you open
  ModelHub (model lists, model cards, file sizes; the first-launch tour opens
  it at its second step), when you browse or download image models, and when
  you download a model, a speech recognition model or a voice. It is also
  asked whether a newer version of an installed model is available: for the
  model selected in a chat, and for all of them when you open the Installed
  tab of ModelHub. Only the name of the model is sent;
- **GitHub** (github.com), to check for and download updates, and to download
  the model files of a voice;
- **GitHub and NVIDIA's servers**, only if you ask for NVIDIA acceleration,
  to download the CUDA 13 pack: ChatonLM's part from this repository's
  releases, NVIDIA's CUDA libraries from NVIDIA;
- **ChatonLM's search service** (relay.chatonlm.com), only when a model
  searches the web: it receives the search and returns results from Brave
  Search. There is no account: your installation registers once with a key
  of its own, made on your computer. Only the titles and links of the
  results are kept in the chat;
- **the providers of remote models** you add, only if remote models are
  turned on (they are off by default): the messages and attachments you send
  to one of their models are processed there, under their own terms;
- **Telegram** (api.telegram.org) **or Discord** (discord.com and its file
  servers), only if you connect a bot in Settings › Easy Link, and only while
  it is turned on: the messages, photos and voice messages you send to your
  bot, and the answers and pictures of your model, go through that service. The token of
  your bot is stored in the database of ChatonLM, on your computer.

Chats with local models are processed by the built-in engine, on your
computer. Links open in your web browser.

## Your data

Everything ChatonLM stores is in one folder (apart from the menu entry of the
AppImage):

- Windows: `%LOCALAPPDATA%\com.chatonlm.chatonlm`
- Linux: `~/.local/share/com.chatonlm.chatonlm`

It holds the database (chats, attachments, settings), the models (unless
you chose another Model location), the NVIDIA acceleration pack if you
downloaded it, the API keys of remote models, the speech recognition models,
the voices, the ModelHub cache, logs and downloaded updates.

To keep it elsewhere, set the `CHATONLM_DATA_DIR` environment variable to an
absolute path before starting ChatonLM. On Linux, for example:

```bash
CHATONLM_DATA_DIR=/path/to/folder ./chatonlm_*_amd64.AppImage
```

### Uninstall

1. Remove the app.
   - **Windows**: Settings › Apps › Installed apps › ChatonLM › Uninstall.
     Your data is kept unless you tick the box that deletes it.
   - **AppImage**: first turn off Settings › General › Applications menu ›
     **Show in the applications menu** (this removes the menu entry and its
     icon), then delete the AppImage. If the AppImage is already gone, delete
     `~/.local/share/applications/com.chatonlm.chatonlm.desktop`.
   - **.deb**: `sudo apt remove chatonlm`.
2. Delete the [data folder](#your-data) to remove your chats, settings and
   models (and the folder chosen in Model location, if you chose one).
3. On Linux, if you used Model location, or **Fix** for the Ollama service of
   your system, with ChatonLM 0.9, it added files named `zz-chatonlm-*.conf` to
   `/etc/systemd/system/ollama.service.d/`. They keep working without
   ChatonLM. To undo them, move your models back to Ollama's default folder,
   delete the files, then run
   `sudo systemctl daemon-reload && sudo systemctl restart ollama`.

## Build from source

These steps are for Linux. You need Rust (the version in
`rust-toolchain.toml`, installed by rustup), Node.js 24 (`.nvmrc`), and the
development files of WebKitGTK 4.1, GTK 3, libsoup 3, ALSA and D-Bus, with
CMake and Clang. On Ubuntu 22.04 or later:

```bash
sudo apt install build-essential pkg-config cmake clang libclang-dev \
  libwebkit2gtk-4.1-dev libgtk-3-dev libsoup-3.0-dev \
  libjavascriptcoregtk-4.1-dev libasound2-dev libdbus-1-dev
```

Then:

```bash
git clone https://github.com/Elwynn18/chatonlm.git
cd chatonlm
npm ci
npm run tauri build -- --no-bundle
```

The program is `src-tauri/target/release/chatonlm`. A copy built this way:

- has no text engine: the packages include `llama-server`, built from
  llama.cpp by `scripts/build-llama-server.sh` (pinned in
  `packaging/llama.env`), whose output in `.tools/runtimes/llama-server` a
  development build finds on its own;
- needs ONNX Runtime 1.28 for Parakeet dictation: set `ORT_DYLIB_PATH` to its
  `libonnxruntime.so.1`, for example from Microsoft's
  `onnxruntime-linux-x64-1.28.3.tgz`. Without it, Whisper still works;
- doesn't update itself.

For development, `npm run tauri dev -- --config src-tauri/tauri.dev.conf.json`
starts ChatonLM with hot reload, under its own identifier, so it doesn't share
data with an installed copy.

The release packages are built by GitHub Actions, which also builds the
pinned llama.cpp, fetches the pinned ONNX Runtime and writes the license
notices: on Ubuntu
22.04 for Linux ([`packages.yml`](.github/workflows/packages.yml)), and with
the MSVC toolchain for Windows
([`packages-windows.yml`](.github/workflows/packages-windows.yml), the
reference for building on Windows).

### Contributing

Issues and pull requests are welcome. Before opening a pull request, please
run `npm run lint`, `npm run typecheck`, `npm test` and
`cargo test --manifest-path src-tauri/Cargo.toml`. Comments in the code and
commit messages are in French; the interface is in English, Spanish and
French, and every text change goes into all three languages
(`src/locales/`). A fix or an improvement applies to both Windows and Linux.

To report a security problem, please do not open an issue: see
[SECURITY.md](SECURITY.md).

## License and credits

ChatonLM is released under the [MIT License](LICENSE), Copyright (c) 2026
Elwynn18 and ChatonLM contributors.

[THIRD_PARTY_LICENSES](THIRD_PARTY_LICENSES) lists the third-party material it
includes, with their notices. The packages also carry the licenses of every
Rust crate and npm package built into ChatonLM, of ONNX Runtime and of the
built-in engine: in `/usr/share/doc/chatonlm` on Linux, and in the `licenses`
folder of the installation on Windows.

- The interface follows, and partly adapts, the official
  [Ollama](https://github.com/ollama/ollama) app (MIT License, Copyright (c)
  Ollama); the adapted files are listed in THIRD_PARTY_LICENSES, part 1.
- The packages include `llama-server`, the server of
  [llama.cpp](https://github.com/ggml-org/llama.cpp) (MIT), built by
  ChatonLM with the CPU and Vulkan backends of ggml (THIRD_PARTY_LICENSES,
  part 11), and [ONNX Runtime](https://github.com/microsoft/onnxruntime)
  1.28.3 (MIT), unmodified.
- The optional NVIDIA acceleration pack holds ChatonLM's build of the CUDA
  backend of ggml (MIT), published with each release. NVIDIA's CUDA
  libraries are not shipped by ChatonLM: they are downloaded from NVIDIA, on
  demand, under NVIDIA's license, which ChatonLM shows first.
- Models are downloaded on demand from Hugging Face, under their own
  licenses. Models imported from Ollama are copied from your computer.
- The Windows package also contains the Microsoft Visual C++ runtime
  libraries, under Microsoft's redistribution terms (THIRD_PARTY_LICENSES,
  part 9).
- The AppImage also contains system libraries from Ubuntu 22.04, some under
  the GNU LGPL. Their source code, and that of the AppImage runtime, is
  published with each release as `chatonlm_<version>_amd64.AppImage-sources.tar`.
- The packages include sd-server, the server of
  [stable-diffusion.cpp](https://github.com/leejet/stable-diffusion.cpp)
  (MIT), built by ChatonLM with the Vulkan backend of ggml
  (THIRD_PARTY_LICENSES, part 10). Image models are downloaded on demand from
  Hugging Face, under their own licenses, shown in ModelHub.
- Speech recognition models are downloaded on demand, not shipped:
  [Parakeet TDT 0.6B v3](https://huggingface.co/nvidia/parakeet-tdt-0.6b-v3)
  by NVIDIA (CC BY 4.0, in the ONNX conversion by
  [istupakov](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx)),
  and [Whisper](https://github.com/openai/whisper) by OpenAI (MIT, in the
  [whisper.cpp](https://github.com/ggml-org/whisper.cpp) format).
- Voices for Read aloud are downloaded on demand, not shipped:
  [Pocket TTS](https://github.com/kyutai-labs/pocket-tts) by Kyutai (model
  CC BY 4.0, exported to ONNX and quantized by ChatonLM; code MIT), with the
  voices estelle (CC0), alba (Alba MacKenna, CC BY 4.0) and lola (Mozilla
  Common Voice, CC0).
- Voice mode detects speech with the
  [Silero VAD](https://github.com/snakers4/silero-vad) model by the Silero
  Team (MIT), built into ChatonLM, and cancels echo with
  [sonora](https://github.com/dignifiedquire/sonora), a Rust port of the
  WebRTC audio processing (BSD-3-Clause).
- Built with [Tauri](https://tauri.app), [React](https://react.dev),
  [Streamdown](https://github.com/vercel/streamdown),
  [Shiki](https://shiki.style) and [KaTeX](https://katex.org).

ChatonLM is not affiliated with, endorsed or sponsored by Ollama. "Ollama" is a
trademark of its owner and is used only to describe compatibility.
