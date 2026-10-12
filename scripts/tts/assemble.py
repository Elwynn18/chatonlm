# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
"""Assemble les fichiers des voix de ChatonLM et imprime tailles et SHA-256.

Pour chaque langue, le dossier `<sortie>/pocket-tts-<langue>/` reçoit les
fichiers sous leurs noms sur le disque de l'app :

- mimi_decoder.onnx (commun : vérifié identique d'une langue à l'autre),
  flow_lm_step.int8.onnx, text_conditioner.onnx : nos graphes (export_onnx.py
  puis quantize.py) ;
- tokenizer.json et <voix>.safetensors : téléchargés chez Kyutai
  (kyutai/pocket-tts-without-voice-cloning) aux révisions épinglées de la
  configuration de pocket_tts (une voix ne vaut que pour la révision des
  poids dont elle est tirée).

Avec --release <dossier>, y écrit aussi nos graphes sous les noms des assets
de la release GitHub (mimi_decoder.onnx, <langue>-flow_lm_step.int8.onnx,
<langue>-text_conditioner.onnx).

Usage : assemble.py <sortie> --export fr=<dossier> --export en=<dossier>
        --export es=<dossier> [--flow flow_lm_step.int8.onnx] [--release <dossier>]

Les valeurs imprimées vont dans `CATALOG` de src-tauri/src/tts/models.rs.
"""

import argparse
import hashlib
import shutil
import urllib.request
from pathlib import Path

KYUTAI = "https://huggingface.co/kyutai/pocket-tts-without-voice-cloning/resolve"
VOICES_REVISION = "4e1e0a3e611c51c0b4ed8174fc10f32a54644303"

# Langue -> (voix, tokenizer.json et sa révision, voix et sa révision). Révisions de
# pocket-tts 3.3.0 (config/<langue>.yaml, utils.get_predefined_voice).
LANGUAGES = {
    "fr": (
        "estelle",
        ("languages/french/tokenizer.json", "8843db76457a91db32077edf8dfcd1c0e3e755fd"),
        ("languages/french/embeddings/estelle.safetensors", VOICES_REVISION),
    ),
    "en": (
        "alba",
        ("languages/english/tokenizer.json", "00eac05ed3d16bdc3f6b5d598874019c34a89214"),
        ("languages/english/embeddings/alba.safetensors", VOICES_REVISION),
    ),
    "es": (
        "lola",
        ("languages/spanish/tokenizer.json", "4e1e0a3e611c51c0b4ed8174fc10f32a54644303"),
        ("languages/spanish/embeddings/lola.safetensors", VOICES_REVISION),
    ),
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with open(path, "rb") as fh:
        for block in iter(lambda: fh.read(1 << 20), b""):
            digest.update(block)
    return digest.hexdigest()


def place(src: Path, dst: Path):
    """Copie (lien dur si possible : mêmes octets, rien à recopier)."""
    if dst.exists():
        dst.unlink()
    try:
        dst.hardlink_to(src)
    except OSError:
        shutil.copyfile(src, dst)


def download(path: str, revision: str, dst: Path):
    url = f"{KYUTAI}/{revision}/{path}"
    tmp = dst.with_suffix(dst.suffix + ".part")
    with urllib.request.urlopen(url, timeout=60) as response, open(tmp, "wb") as fh:
        shutil.copyfileobj(response, fh)
    tmp.replace(dst)
    print(f"downloaded {url}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("out", type=Path)
    parser.add_argument("--export", action="append", required=True, help="<langue>=<dossier>")
    parser.add_argument("--flow", default="flow_lm_step.int8.onnx", help="FlowLM int8 de l'export")
    parser.add_argument("--release", type=Path)
    args = parser.parse_args()

    exports = dict(item.split("=", 1) for item in args.export)
    assert set(exports) == set(LANGUAGES), f"one export per language: {sorted(LANGUAGES)}"
    mimi = {lang: sha256(Path(d) / "mimi_decoder.onnx") for lang, d in exports.items()}
    assert len(set(mimi.values())) == 1, f"Mimi differs between languages: {mimi}"
    if args.release:
        args.release.mkdir(parents=True, exist_ok=True)
    for lang, (voice, tokenizer, voice_file) in LANGUAGES.items():
        src = Path(exports[lang])
        dst = args.out / f"pocket-tts-{lang}"
        dst.mkdir(parents=True, exist_ok=True)
        place(src / "mimi_decoder.onnx", dst / "mimi_decoder.onnx")
        place(src / args.flow, dst / "flow_lm_step.int8.onnx")
        place(src / "text_conditioner.onnx", dst / "text_conditioner.onnx")
        download(*tokenizer, dst / "tokenizer.json")
        download(*voice_file, dst / f"{voice}.safetensors")
        if args.release:
            place(src / "mimi_decoder.onnx", args.release / "mimi_decoder.onnx")
            place(src / args.flow, args.release / f"{lang}-flow_lm_step.int8.onnx")
            place(src / "text_conditioner.onnx", args.release / f"{lang}-text_conditioner.onnx")
    print()
    for lang, (voice, _, _) in LANGUAGES.items():
        dst = args.out / f"pocket-tts-{lang}"
        names = [
            "mimi_decoder.onnx",
            "flow_lm_step.int8.onnx",
            "text_conditioner.onnx",
            "tokenizer.json",
            f"{voice}.safetensors",
        ]
        total = 0
        print(f"// pocket-tts-{lang}")
        for name in names:
            size = (dst / name).stat().st_size
            total += size
            print(f'file("{name}", {size}, "{sha256(dst / name)}"),')
        print(f"// total {total} bytes\n")


if __name__ == "__main__":
    main()
