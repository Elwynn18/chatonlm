# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
#
# Uses pocket-tts 3.3.0 (https://github.com/kyutai-labs/pocket-tts, MIT,
# Copyright (c) Kyutai) as the reference the Rust port is compared to.
"""Fixtures des essais Rust du moteur de lecture (src-tauri/src/tts/fixtures/).

unit <voix> <sortie> : pour chaque langue, jetons du tokenizer Python
    (`tokenizers`, comme pocket_tts) sur un petit corpus, décodages,
    `prepare_text_prompt` et `split_into_best_sentences` du paquet, avec le
    sous-ensemble du vocabulaire qu'il touche (le Viterbi ne voit que les
    pièces présentes dans le texte : le résultat est le même qu'avec les
    4 000) -> prompt-<langue>.json.
reference <voix> <sortie> : boucle de référence (numpy + ONNX Runtime, 1 fil,
    graphes finaux) avec un bruit imposé : conditionneur, premiers pas de
    FlowLM, premiers blocs de Mimi, fin de la phrase -> reference-<langue>.json.

<voix> : dossier des voix assemblées (assemble.py), un sous-dossier
pocket-tts-<langue> par langue.
"""

import json
import math
import sys
from pathlib import Path

LANGS = {"fr": ("french", "estelle"), "en": ("english", "alba"), "es": ("spanish", "lola")}

CORPUS = {
    "fr": [
        "Bonjour ! Comment ça va ?",
        "Oui.",
        "  Il fait beau aujourd'hui  ",
        "l’été dernier, nous sommes allés à la mer",
        "Elle a dit : « Je reviens dans cinq minutes. »",
        "Version 3.5 du logiciel ; elle corrige deux défauts (mineurs).",
        "Attendez… Je réfléchis encore,",
        "Les élèves étudient l'histoire, la géographie, les mathématiques, la physique, "
        "la chimie, la biologie, la musique, le dessin, la philosophie, l'économie, "
        "le droit, l'informatique et les langues anciennes au lycée.",
        "Première phrase. Deuxième phrase ! Troisième phrase ? Quatrième phrase. "
        "Cinquième phrase, un peu plus longue que les autres. Sixième. Septième phrase "
        "pour finir le paragraphe, avec quelques mots de plus.",
        "Le café 🙂 et le thé 東京 sont prêts.\nOn y va ?",
        "“Pourquoi ?”, demanda-t-il.",
        "Il est quatorze heures trente.",
        "Il a dit « oui »",
        "A P I REST et G P U.",
    ],
    "en": [
        "Hello! How are you?",
        "Yes.",
        "  It is a sunny day  ",
        "She said: \"I'll be back in five minutes.\"",
        "Version 3.5 fixes two bugs; it's (mostly) stable.",
        "Wait… I'm still thinking,",
        "The students learn history, geography, mathematics, physics, chemistry, "
        "biology, music, drawing, philosophy, economics, law, computer science and "
        "ancient languages at school every single day of the week.",
        "First sentence. Second sentence! Third sentence? Fourth sentence. Fifth "
        "sentence, a little longer than the others. Sixth. Seventh sentence to end "
        "the paragraph, with a few more words.",
        "Coffee 🙂 and tea 東京 are ready.\nShall we go?",
        "“Why?”, he asked.",
        "The A P I and the G P U.",
    ],
    "es": [
        "¡Hola! ¿Cómo estás?",
        "Sí.",
        "  Hace buen tiempo hoy  ",
        "Ella dijo: «Vuelvo en cinco minutos».",
        "La versión 3.5 corrige dos errores; es (casi) estable.",
        "Espera… Todavía estoy pensando,",
        "Los alumnos estudian historia, geografía, matemáticas, física, química, "
        "biología, música, dibujo, filosofía, economía, derecho, informática y lenguas "
        "antiguas en el instituto todos los días.",
        "Primera frase. ¡Segunda frase! ¿Tercera frase? Cuarta frase. Quinta frase, un "
        "poco más larga que las otras. Sexta. Séptima frase para terminar el párrafo, "
        "con algunas palabras más.",
        "El café 🙂 y el té 東京 están listos.\n¿Vamos?",
        "La A P I REST y la G P U de Nvidia.",
    ],
}

# Phrases de la référence de la boucle (une par langue, un seul morceau).
REFERENCE_TEXT = {
    "fr": "Bonjour, je lis cette phrase pour vérifier le moteur.",
    "en": "Hello, I am reading this sentence to check the engine.",
    "es": "Hola, leo esta frase para comprobar el motor.",
}
REFERENCE_STEPS = 10
REFERENCE_BLOCKS = 4
NOISE_SEED = 1234


def config(lang):
    from pocket_tts.utils.config import CONFIGS_DIR, load_config

    return load_config(CONFIGS_DIR / f"{LANGS[lang][0]}.yaml")


class Recorder:
    """Tokenizer de pocket_tts qui note chaque texte encodé (sous-vocabulaire)."""

    def __init__(self, path):
        import tokenizers

        self.tok = tokenizers.Tokenizer.from_file(str(path))
        self.seen = set()

    def encode(self, text):
        self.seen.add(text)
        return self.tok.encode(text).ids

    def decode(self, ids):
        return self.tok.decode(ids)

    def __call__(self, text):
        import torch

        return torch.tensor(self.encode(text))[None, :]


def mini_vocab(path, seen):
    model = json.loads(Path(path).read_text())["model"]
    vocab = model["vocab"]
    haystack = ["▁" + s.replace(" ", "▁") for s in seen]
    keep = []
    for i, (piece, score) in enumerate(vocab):
        special = i < 4 or (piece.startswith("<0x") and len(piece) == 6)
        if special or any(piece in h for h in haystack):
            keep.append([i, piece, score])
    return {
        "unk_id": model["unk_id"],
        "size": len(vocab),
        "min_score": min(score for _, score in vocab),
        "vocab": keep,
    }


def unit(voices: Path, out: Path):
    from pocket_tts.models.text_chunking import prepare_text_prompt, split_into_best_sentences

    out.mkdir(parents=True, exist_ok=True)
    for lang in LANGS:
        cfg = config(lang)
        path = voices / f"pocket-tts-{lang}" / "tokenizer.json"
        tok = Recorder(path)
        options = (
            cfg.pad_with_spaces_for_short_inputs,
            cfg.remove_semicolons,
            cfg.append_terminal_punctuation,
            cfg.capitalize_first_letter,
            cfg.replace_characters,
        )
        encode, decode, prepare, chunks = [], [], [], []
        for text in CORPUS[lang]:
            ids = tok.encode(text)
            encode.append([text, ids])
            for part in (ids, ids[1:], ids[:3], ids[2:5]):
                decode.append([part, tok.decode(part)])
            prepared, guess = prepare_text_prompt(text, *options)
            prepare.append([text, prepared, guess])
            parts = split_into_best_sentences(tok, text, 50, *options)
            items = []
            for chunk in parts:
                prepared_chunk, guess = prepare_text_prompt(chunk, *options)
                ids = tok.encode(prepared_chunk)
                items.append(
                    {
                        "text": prepared_chunk,
                        "tokens": ids,
                        "frames_after_eos": guess + 2,
                        "max_gen_len": math.ceil((len(ids) / 3.0 + 2.0) * 12.5),
                    }
                )
            chunks.append([text, items])
        _, *eos = tok.encode(".!...?")
        _, *fallback = tok.encode(",;:")
        fixture = {
            "language": lang,
            "tokenizer": mini_vocab(path, tok.seen),
            "eos_tokens": eos,
            "fallback_tokens": fallback,
            "encode": encode,
            "decode": decode,
            "prepare": prepare,
            "chunks": chunks,
        }
        target = out / f"prompt-{lang}.json"
        target.write_text(json.dumps(fixture, ensure_ascii=False, separators=(",", ":")))
        print(target, target.stat().st_size, "bytes,", len(fixture["tokenizer"]["vocab"]), "pieces")


def reference(voices: Path, out: Path):
    import numpy as np
    import onnxruntime as ort
    from safetensors.numpy import load_file

    from pocket_tts.models.text_chunking import prepare_text_prompt

    out.mkdir(parents=True, exist_ok=True)
    so = ort.SessionOptions()
    so.intra_op_num_threads = 1
    so.inter_op_num_threads = 1
    so.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    so.graph_optimization_level = ort.GraphOptimizationLevel.ORT_ENABLE_ALL
    for lang, (_, voice) in LANGS.items():
        cfg = config(lang)
        d = voices / f"pocket-tts-{lang}"
        tok = Recorder(d / "tokenizer.json")
        text, guess = prepare_text_prompt(
            REFERENCE_TEXT[lang],
            cfg.pad_with_spaces_for_short_inputs,
            cfg.remove_semicolons,
            cfg.append_terminal_punctuation,
            cfg.capitalize_first_letter,
            cfg.replace_characters,
        )
        tokens = tok.encode(text)
        frames_after_eos = guess + 2
        max_gen_len = math.ceil((len(tokens) / 3.0 + 2.0) * 12.5)
        st = load_file(str(d / f"{voice}.safetensors"))
        caches = [st[f"transformer.layers.{i}.self_attn/cache"][:, 0] for i in range(6)]
        p0 = int(st["transformer.layers.0.self_attn/offset"][0])
        voice_kv = np.ascontiguousarray(
            np.stack(caches, 0)[:, :, :p0].transpose(2, 0, 1, 3, 4)
        )  # [P, L, 2, H, D]
        tc = ort.InferenceSession(str(d / "text_conditioner.onnx"), so)
        fl = ort.InferenceSession(str(d / "flow_lm_step.int8.onnx"), so)
        mi = ort.InferenceSession(str(d / "mimi_decoder.onnx"), so)
        rng = np.random.default_rng(NOISE_SEED)
        noise = (rng.standard_normal((max_gen_len, 32)) * math.sqrt(0.3)).astype(np.float32)
        te, bos = tc.run(None, {"token_ids": np.asarray(tokens, np.int64)[None]})
        x = np.concatenate([te, bos], axis=1)
        buf = np.empty((p0 + x.shape[1] + max_gen_len + 1,) + voice_kv.shape[1:], np.float32)
        buf[:p0] = voice_kv
        P = p0
        steps, latents, eos_step, frames = [], [], None, 0
        for g in range(max_gen_len):
            latent, ml, el, nx, nk = fl.run(
                None, {"inputs_embeds": x, "past_kv": buf[:P], "noise": noise[g][None]}
            )
            buf[P : P + nk.shape[0]] = nk
            P += nk.shape[0]
            if g < REFERENCE_STEPS:
                steps.append(
                    {
                        "latent": latent[0].tolist(),
                        "mimi_latent": ml[0].tolist(),
                        "eos_logit": float(el[0, 0]),
                        "new_kv_rows": int(nk.shape[0]),
                        "new_kv_head": nk[-1].reshape(-1)[:16].tolist(),
                        "next_embeds_head": nx.reshape(-1)[:16].tolist(),
                    }
                )
            if el[0, 0] > -4.0 and eos_step is None and g >= 6:
                eos_step = g
            if eos_step is not None and g >= eos_step + frames_after_eos:
                break
            x = nx
            latents.append(ml[0])
            frames += 1
        # Mimi : premiers latents un par un, puis le reste d'un coup.
        past = np.zeros((250, 2, 2, 8, 64), np.float32)
        conv = np.zeros(14720, np.float32)
        offset = 0
        blocks, audio = [], []
        for i in range(len(latents)):
            if i > REFERENCE_BLOCKS:
                break
            lat = np.stack(latents[i : i + 1] if i < REFERENCE_BLOCKS else latents[i:], 0)[None]
            a, past, conv = mi.run(
                None,
                {"latent": lat, "offset": np.array(offset, np.int64), "past_kv": past, "conv_state": conv},
            )
            offset += 16 * lat.shape[1]
            a = a[0, 0]
            audio.append(a)
            if i < REFERENCE_BLOCKS:
                blocks.append(
                    {
                        "head": a[:32].tolist(),
                        "rms": float(np.sqrt((a**2).mean())),
                        "sum": float(a.sum()),
                    }
                )
        audio = np.concatenate(audio)
        fixture = {
            "language": lang,
            "voice": voice,
            "text": text,
            "tokens": tokens,
            "voice_positions": p0,
            "frames_after_eos": frames_after_eos,
            "max_gen_len": max_gen_len,
            "noise": noise[: max(frames + frames_after_eos + 1, REFERENCE_STEPS)].tolist(),
            "conditioner": {
                "text_head": te[0, :, :8].tolist(),
                "text_sums": te[0].sum(axis=1).tolist(),
                "bos_head": bos.reshape(-1)[:32].tolist(),
                "bos_sum": float(bos.sum()),
            },
            "steps": steps,
            "eos_step": eos_step,
            "frames": frames,
            "mimi_blocks": blocks,
            "audio_samples": int(audio.shape[0]),
            "audio_rms": float(np.sqrt((audio**2).mean())),
        }
        target = out / f"reference-{lang}.json"
        target.write_text(json.dumps(fixture, ensure_ascii=False, separators=(",", ":")))
        print(target, target.stat().st_size, "bytes; frames", frames, "eos", eos_step)


if __name__ == "__main__":
    mode, voices, out = sys.argv[1], Path(sys.argv[2]), Path(sys.argv[3])
    {"unit": unit, "reference": reference}[mode](voices, out)
