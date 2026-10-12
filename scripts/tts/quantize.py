# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
#
# Inspired by pocket-tts 3.3.0 (https://github.com/kyutai-labs/pocket-tts,
# MIT, Copyright (c) Kyutai): same choice of layers as
# pocket_tts.quantization.RECOMMENDED_CONFIG.
"""Quantification dynamique int8 (poids) du graphe flow_lm_step.onnx.

Variante retenue (rapport 57 § 3) : int8 par canal, transformer seul
(attention et FFN) ; réseau de flux, entrée et tête EOS restent en fp32.

Usage : quantize.py <entrée> <sortie> [--per-tensor] [--all-layers]

Les value_info de l'exportateur dynamo contredisent l'inférence de formes
d'onnx : on les retire avant de quantifier.
"""

import argparse
import tempfile
from pathlib import Path

import onnx
from onnxruntime.quantization import QuantType, quantize_dynamic

# Formes des poids de l'attention et du FFN du transformer (d = 1024).
TRANSFORMER_SHAPES = {(1024, 3072), (3072, 1024), (1024, 1024), (1024, 4096), (4096, 1024)}


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("src", type=Path)
    parser.add_argument("dst", type=Path)
    parser.add_argument("--per-tensor", action="store_true", help="une échelle par tenseur")
    parser.add_argument("--all-layers", action="store_true", help="aussi le réseau de flux")
    args = parser.parse_args()

    model = onnx.load(str(args.src))
    del model.graph.value_info[:]
    exclude = []
    if not args.all_layers:
        inits = {i.name: tuple(i.dims) for i in model.graph.initializer}
        for node in model.graph.node:
            if node.op_type in ("MatMul", "Gemm"):
                shapes = {inits[i] for i in node.input if i in inits}
                if not (shapes & TRANSFORMER_SHAPES):
                    exclude.append(node.name)
        total = sum(1 for n in model.graph.node if n.op_type in ("MatMul", "Gemm"))
        print(f"excluded: {len(exclude)}, quantized: {total - len(exclude)}")
    with tempfile.TemporaryDirectory(dir=args.dst.parent) as tmp:
        clean = Path(tmp) / "clean.onnx"
        onnx.save(model, str(clean))
        quantize_dynamic(
            str(clean),
            str(args.dst),
            weight_type=QuantType.QInt8,
            per_channel=not args.per_tensor,
            op_types_to_quantize=["MatMul", "Gemm"],
            # En mode dynamique, ORT remplace chaque Gemm par <nom>_MatMul + <nom>_Add.
            nodes_to_exclude=exclude + [n + "_MatMul" for n in exclude],
        )


if __name__ == "__main__":
    main()
