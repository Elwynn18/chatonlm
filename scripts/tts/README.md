# Read-aloud voices: ONNX export of Pocket TTS

ChatonLM reads answers aloud with [Pocket TTS](https://github.com/kyutai-labs/pocket-tts)
(Kyutai, MIT code, CC BY 4.0 weights), run by ONNX Runtime from Rust
(`src-tauri/src/tts/`). These scripts turn a pinned revision of the official
weights into the files the app downloads. The code is inspired by
pocket-tts (MIT, Copyright (c) Kyutai).

| Script             | Role                                                                                                                                                     |
| ------------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `export_onnx.py`   | Exports `text_conditioner.onnx`, `flow_lm_step.onnx` (one per language) and `mimi_decoder.onnx` (shared), with explicit state, plus `manifest.json`.     |
| `quantize.py`      | Dynamic int8 quantization of `flow_lm_step.onnx`, per channel, transformer only (flow network kept in fp32).                                             |
| `assemble.py`      | Builds `pocket-tts-<fr\|en\|es>/` with the app's file names, downloads `tokenizer.json` and the voice from Kyutai, prints sizes and SHA-256.             |
| `fixtures.py`      | Writes the Rust test fixtures (`src-tauri/src/tts/fixtures/`): tokenizer and text preparation of the Python package, step-by-step reference of the loop. |
| `requirements.txt` | Pinned Python environment.                                                                                                                               |

## Re-exporting a revision

1. In a Python 3.12 virtual environment kept under `.tools/`:
   `pip install -r scripts/tts/requirements.txt`. To move to newer weights,
   bump `pocket-tts`: its `config/<language>.yaml` pins the weight and
   tokenizer revisions, `utils.get_predefined_voice` the voice revision (a
   voice only works with the weights it was computed from).
2. For each language (`french`, `english`, `spanish`):
   `python export_onnx.py french out/fr`, then
   `python quantize.py out/fr/flow_lm_step.onnx out/fr/flow_lm_step.int8.onnx`.
   The Mimi decoder is identical for all languages (`assemble.py` checks it).
3. Listen before shipping (int8 changes durations and pauses if done per
   tensor; see `.tools/research/57-tts-engine.md`).
4. Update the revisions in `assemble.py`, then
   `python assemble.py .tools/voice-models --export fr=out/fr --export en=out/en --export es=out/es --release out/release`.
   `out/release/` holds the GitHub release assets (`mimi_decoder.onnx`,
   `<lang>-flow_lm_step.int8.onnx`, `<lang>-text_conditioner.onnx`).
5. Copy the printed sizes and SHA-256 into `CATALOG`
   (`src-tauri/src/tts/models.rs`), bump the release tag, regenerate the
   fixtures (`python fixtures.py unit .tools/voice-models src-tauri/src/tts/fixtures`
   and `python fixtures.py reference …`) and run the contracts:
   `CHATONLM_VOICE_MODELS_DIR=$PWD/.tools/voice-models cargo test --manifest-path src-tauri/Cargo.toml --lib tts_contract -- --ignored`.
