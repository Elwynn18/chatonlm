# SPDX-License-Identifier: MIT
# SPDX-FileCopyrightText: 2026 Elwynn18 and ChatonLM contributors
#
# Inspired by pocket-tts 3.3.0 (https://github.com/kyutai-labs/pocket-tts,
# MIT, Copyright (c) Kyutai): the official modules are reused for their
# weights; the streaming steps are rewritten as stateless functions.
"""Export ONNX de Pocket TTS (modèle 6 couches d'une langue), sans état caché.

Trois graphes ; l'état (caches KV, tampons des convolutions) passe par les
entrées et les sorties, et la boucle (échantillonnage, fin de phrase, flux)
reste du côté de l'appelant (Rust, `src-tauri/src/tts/engine.rs`) :

- text_conditioner.onnx (par langue) : token_ids [1,T] int64 -> text_embeds
  [1,T,1024] et bos_embeds [1,1,1024] (= input_linear(bos_emb), première
  entrée de la génération).
- flow_lm_step.onnx (par langue) : inputs_embeds [1,T,1024], past_kv
  [P,6,2,16,64], noise [1,32] -> latent [1,32], mimi_latent [1,32]
  (dénormalisé), eos_logit [1,1], next_embeds [1,1,1024] (=
  input_linear(latent)), new_kv [T,6,2,16,64]. Le pas LSD (1 étape) est
  fusionné : latent = noise + flow_net(h, s=0, t=1, noise). Disposition
  [P, ...] : l'appelant ajoute new_kv au bout d'un tampon contigu.
- mimi_decoder.onnx (commun aux langues : mêmes poids) : latent [1,N,32],
  offset [] int64 (pas du transformer = 16 x latents déjà décodés), past_kv
  [250,2,2,8,64], conv_state [14720] (tampons des convolutions bout à bout,
  ordre dans manifest.json) -> audio [1,1,1920N], present_kv, new_conv_state.

Aucune dimension nulle (torch.export spécialiserait) : l'appelant passe le
texte et BOS ensemble au premier pas (le transformer est causal).

Usage : export_onnx.py <langue> <dossier de sortie> [--only text|flow|mimi]
<langue> : nom de configuration de pocket_tts (french, spanish, english...).
"""

import argparse
import json
import math
from pathlib import Path

import torch
import torch.nn.functional as F
from torch import nn

from pocket_tts import TTSModel
from pocket_tts.modules.conv import StreamingConv1d, StreamingConvTranspose1d
from pocket_tts.modules.seanet import SEANetResnetBlock

MIMI_WINDOW = 250  # contexte du transformer de Mimi (config mimi.transformer.context)


def rope(q, k, positions, max_period):
    """Comme pocket_tts.modules.rope.apply_rope ; q, k : [B, T, H, D], positions : [T]."""
    B, T, H, D = q.shape
    ds = torch.arange(D // 2, dtype=torch.float32)
    freqs = torch.exp(ds * (-math.log(max_period) * 2 / D))
    ts = positions.to(torch.float32).view(-1, 1, 1)
    q = q.view(B, T, H, D // 2, 2)
    k = k.view(B, T, H, D // 2, 2)
    qr, qi = q[..., 0], q[..., 1]
    kr, ki = k[..., 0], k[..., 1]
    rotr = torch.cos(freqs * ts)
    roti = torch.sin(freqs * ts)
    qo = torch.stack([qr * rotr - qi * roti, qr * roti + qi * rotr], dim=-1).view(B, T, H, D)
    ko = torch.stack([kr * rotr - ki * roti, kr * roti + ki * rotr], dim=-1).view(B, T, H, D)
    return qo, ko


def layer_step(layer, x, past_k, past_v, positions, mask, max_period):
    """Une couche StreamingTransformerLayer ; past_k/v : [1, H, P, D]."""
    attn = layer.self_attn
    h = layer.norm1(x)
    qkv = attn.in_proj(h)
    B, T, _ = qkv.shape
    q, k, v = qkv.view(B, T, 3, attn.num_heads, attn.dim_per_head).unbind(2)
    q, k = rope(q, k, positions, max_period)
    qh, kh, vh = q.transpose(1, 2), k.transpose(1, 2), v.transpose(1, 2)
    K = torch.cat([past_k, kh], dim=2)
    V = torch.cat([past_v, vh], dim=2)
    o = F.scaled_dot_product_attention(qh, K, V, attn_mask=mask)
    o = o.transpose(1, 2).reshape(B, T, -1)
    x = x + layer.layer_scale_1(attn.out_proj(o))
    ff = layer.linear2(F.gelu(layer.linear1(layer.norm2(x)), approximate="tanh"))
    x = x + layer.layer_scale_2(ff)
    return x, k[0], v[0]  # k, v : [T, H, D] (après RoPE, comme le cache officiel)


class TextConditioner(nn.Module):
    def __init__(self, flow_lm):
        super().__init__()
        self.f = flow_lm

    def forward(self, token_ids):
        bos = self.f.input_linear(self.f.bos_emb.view(1, 1, -1))
        return self.f.conditioner.embed(token_ids), bos


class FlowLMStep(nn.Module):
    def __init__(self, flow_lm):
        super().__init__()
        self.f = flow_lm
        self.max_period = flow_lm.transformer.max_period

    def forward(self, inputs_embeds, past_kv, noise):
        P = past_kv.shape[0]
        T = inputs_embeds.shape[1]
        pos_k = torch.arange(T + P)
        pos_q = pos_k[P:]
        mask = (pos_k[None, :] <= pos_q[:, None])[None, None]
        x = inputs_embeds
        new = []
        for i, layer in enumerate(self.f.transformer.layers):
            pk = past_kv[:, i, 0].permute(1, 0, 2).unsqueeze(0)
            pv = past_kv[:, i, 1].permute(1, 0, 2).unsqueeze(0)
            x, k, v = layer_step(layer, x, pk, pv, pos_q, mask, self.max_period)
            new.append(torch.stack([k, v], dim=1))  # [T, 2, H, D]
        new_kv = torch.stack(new, dim=1)  # [T, L, 2, H, D]
        h = self.f.out_norm(x)[:, -1]
        eos_logit = self.f.out_eos(h)
        s = torch.zeros_like(noise[:, :1])
        t = torch.ones_like(noise[:, :1])
        latent = noise + self.f.flow_net(h, s, t, noise)
        mimi_latent = latent * self.f.emb_std + self.f.emb_mean
        next_embeds = self.f.input_linear(latent).unsqueeze(1)
        return latent, mimi_latent, eos_logit, next_embeds, new_kv


def conv_step(m: StreamingConv1d, x, prev):
    if prev is None:
        return m.conv(x), None
    xc = torch.cat([prev, x], dim=-1)
    tp = prev.shape[-1]
    return m.conv(xc), xc[..., -tp:]


def convtr_step(m: StreamingConvTranspose1d, x, partial):
    y = m.convtr(x)
    pt = partial.shape[-1]
    head = y[..., :pt] + partial
    mid = y[..., pt:-pt]
    tail = y[..., -pt:]
    if m.convtr.bias is not None:
        tail = tail - m.convtr.bias[:, None]
    return torch.cat([head, mid], dim=-1), tail


class MimiDecoder(nn.Module):
    """Décodeur Mimi en flux, état explicite (tampons dans l'ordre de `self.slots`)."""

    def __init__(self, mimi):
        super().__init__()
        self.m = mimi
        self.tr = mimi.decoder_transformer.transformer
        self.max_period = self.tr.max_period
        self.context = self.tr.context
        assert self.context == MIMI_WINDOW
        # Inventaire des tampons d'état (convolutions à état non vide).
        up = mimi.upsample.convtr
        self.slots = [("upsample", up, (1, up.convtr.out_channels, up._kernel_size - up._stride))]
        for name, mod in mimi.decoder.named_modules():
            if isinstance(mod, StreamingConv1d):
                tp = mod._effective_kernel_size - mod._stride
                if tp > 0:
                    self.slots.append((f"decoder.{name}", mod, (1, mod.conv.in_channels, tp)))
            elif isinstance(mod, StreamingConvTranspose1d):
                shape = (1, mod.convtr.out_channels, mod._kernel_size - mod._stride)
                self.slots.append((f"decoder.{name}", mod, shape))
        self.index = {id(mod): i for i, (_, mod, _) in enumerate(self.slots)}

    def forward(self, latent, offset, past_kv, conv_state):
        # Tampons des convolutions aplatis bout à bout dans un seul vecteur.
        states = []
        pos = 0
        for _, _, shape in self.slots:
            n = shape[0] * shape[1] * shape[2]
            states.append(conv_state[pos : pos + n].view(*shape))
            pos += n
        new_states = [None] * len(states)

        def run_conv(mod, x):
            i = self.index.get(id(mod))
            y, st = conv_step(mod, x, None if i is None else states[i])
            if i is not None:
                new_states[i] = st
            return y

        def run_convtr(mod, x):
            i = self.index[id(mod)]
            y, st = convtr_step(mod, x, states[i])
            new_states[i] = st
            return y

        x = self.m.quantizer.output_proj(latent.transpose(1, 2))
        x = run_convtr(self.m.upsample.convtr, x)
        # Transformer (fenêtre glissante de MIMI_WINDOW positions passées).
        z = x.transpose(1, 2)
        T = z.shape[1]
        W = past_kv.shape[0]
        pos_q = offset + torch.arange(T)
        pos_k = torch.cat([offset - W + torch.arange(W), pos_q])
        delta = pos_q[:, None] - pos_k[None, :]
        mask = ((pos_k[None, :] >= 0) & (delta >= 0) & (delta < self.context))[None, None]
        new = []
        for i, layer in enumerate(self.tr.layers):
            pk = past_kv[:, i, 0].permute(1, 0, 2).unsqueeze(0)
            pv = past_kv[:, i, 1].permute(1, 0, 2).unsqueeze(0)
            z, k, v = layer_step(layer, z, pk, pv, pos_q, mask, self.max_period)
            new.append(torch.stack([k, v], dim=1))
        present = torch.cat([past_kv, torch.stack(new, dim=1)], dim=0)[-W:]
        x = z.transpose(1, 2)
        # SEANet.
        for layer in self.m.decoder.model:
            if isinstance(layer, StreamingConv1d):
                x = run_conv(layer, x)
            elif isinstance(layer, StreamingConvTranspose1d):
                x = run_convtr(layer, x)
            elif isinstance(layer, SEANetResnetBlock):
                v = x
                for sub in layer.block:
                    v = run_conv(sub, v) if isinstance(sub, StreamingConv1d) else sub(v)
                x = x + v
            else:
                x = layer(x)
        return x, present, torch.cat([t.reshape(-1) for t in new_states])


def export(language: str, out: Path, only: str | None):
    out.mkdir(parents=True, exist_ok=True)
    model = TTSModel.load_model(language=language)
    model.eval()
    f = model.flow_lm
    L = len(f.transformer.layers)
    H = f.transformer.layers[0].self_attn.num_heads
    D = f.transformer.layers[0].self_attn.dim_per_head
    C = f.dim
    ld = f.ldim
    T_dim = torch.export.Dim("T", min=1, max=4096)
    P_dim = torch.export.Dim("P", min=1, max=8192)
    common = {"dynamo": True, "external_data": False, "optimize": True}
    with torch.no_grad():
        if only in (None, "text"):
            torch.onnx.export(
                TextConditioner(f),
                (torch.randint(0, 4000, (1, 7)),),
                str(out / "text_conditioner.onnx"),
                input_names=["token_ids"],
                output_names=["text_embeds", "bos_embeds"],
                dynamic_shapes={"token_ids": {1: T_dim}},
                **common,
            )
        if only in (None, "flow"):
            torch.onnx.export(
                FlowLMStep(f),
                (torch.randn(1, 5, C), torch.randn(170, L, 2, H, D), torch.randn(1, ld)),
                str(out / "flow_lm_step.onnx"),
                input_names=["inputs_embeds", "past_kv", "noise"],
                output_names=["latent", "mimi_latent", "eos_logit", "next_embeds", "new_kv"],
                dynamic_shapes={"inputs_embeds": {1: T_dim}, "past_kv": {0: P_dim}, "noise": None},
                **common,
            )
        dec = MimiDecoder(model.mimi)
        lt = model.mimi.decoder_transformer.transformer.layers
        mH, mD = lt[0].self_attn.num_heads, lt[0].self_attn.dim_per_head
        state_len = sum(s[0] * s[1] * s[2] for (_, _, s) in dec.slots)
        if only in (None, "mimi"):
            N_dim = torch.export.Dim("N", min=1, max=256)
            torch.onnx.export(
                dec,
                (
                    torch.randn(1, 3, ld),
                    torch.tensor(32, dtype=torch.int64),
                    torch.zeros(MIMI_WINDOW, len(lt), 2, mH, mD),
                    torch.zeros(state_len),
                ),
                str(out / "mimi_decoder.onnx"),
                input_names=["latent", "offset", "past_kv", "conv_state"],
                output_names=["audio", "present_kv", "new_conv_state"],
                dynamic_shapes=({1: N_dim}, None, None, None),
                **common,
            )
    manifest = {
        "language": language,
        "flow_lm": {"layers": L, "heads": H, "head_dim": D, "dim": C, "latent_dim": ld},
        "mimi": {
            "layers": len(lt),
            "heads": mH,
            "head_dim": mD,
            "window": MIMI_WINDOW,
            "steps_per_latent": int(model.mimi.encoder_frame_rate / model.mimi.frame_rate),
            "samples_per_latent": int(model.mimi.sample_rate / model.mimi.frame_rate),
            "conv_state_len": state_len,
            "states": [
                {"name": f"state_{i}", "module": s[0], "shape": list(s[2])}
                for i, s in enumerate(dec.slots)
            ],
        },
        "temp": model.temp,
        "eos_threshold": model.eos_threshold,
        "sample_rate": model.sample_rate,
    }
    (out / "manifest.json").write_text(json.dumps(manifest, indent=1))
    print(json.dumps(manifest)[:600])


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("language")
    parser.add_argument("out", type=Path)
    parser.add_argument("--only", choices=["text", "flow", "mimi"])
    args = parser.parse_args()
    export(args.language, args.out, args.only)
