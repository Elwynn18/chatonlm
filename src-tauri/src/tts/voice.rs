//! État d'une voix précalculée de Kyutai (`<voix>.safetensors`) : non pas un
//! vecteur, mais le cache KV du transformer de FlowLM après la voix (rapport
//! 57 § 1). Six couches `transformer.layers.<i>.self_attn/cache` en
//! `[2, 1, P, 16, 64]` f32, plus `offset` (= P, positions remplies) et `pad`
//! (= 0). Réarrangé ici une fois pour toutes dans la disposition du graphe
//! `flow_lm_step` : `[P, couche, k/v, tête, dim]`.
//!
//! En-tête safetensors lu à la main : 8 octets (longueur, petit-boutiste),
//! JSON `{nom: {dtype, shape, data_offsets}}`, puis les données.

use std::collections::HashMap;
use std::path::Path;

use serde::Deserialize;

pub const LAYERS: usize = 6;
pub const HEADS: usize = 16;
pub const HEAD_DIM: usize = 64;
/// Flottants d'une position du cache : couches × (k, v) × têtes × dim.
pub const KV_ROW: usize = LAYERS * 2 * HEADS * HEAD_DIM;

/// Cache de la voix, `[positions, KV_ROW]`.
pub struct Voice {
    pub positions: usize,
    pub kv: Vec<f32>,
}

impl std::fmt::Debug for Voice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Voice")
            .field("positions", &self.positions)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Entry {
    dtype: String,
    shape: Vec<usize>,
    data_offsets: (usize, usize),
}

struct SafeTensors<'a> {
    entries: HashMap<String, Entry>,
    data: &'a [u8],
}

impl<'a> SafeTensors<'a> {
    fn parse(bytes: &'a [u8]) -> Result<Self, String> {
        let length = bytes
            .get(..8)
            .map(|head| u64::from_le_bytes(head.try_into().unwrap()) as usize)
            .ok_or("voice: file too short")?;
        let header = bytes
            .get(8..8usize.saturating_add(length))
            .ok_or("voice: truncated header")?;
        let mut raw: HashMap<String, serde_json::Value> =
            serde_json::from_slice(header).map_err(|error| format!("voice header: {error}"))?;
        raw.remove("__metadata__");
        let entries = raw
            .into_iter()
            .map(|(name, value)| {
                serde_json::from_value(value)
                    .map(|entry| (name.clone(), entry))
                    .map_err(|error| format!("voice tensor {name}: {error}"))
            })
            .collect::<Result<_, _>>()?;
        Ok(Self {
            entries,
            data: &bytes[8 + length..],
        })
    }

    /// Octets et forme d'un tenseur de type `dtype`.
    fn tensor(&self, name: &str, dtype: &str, width: usize) -> Result<(&[u8], &[usize]), String> {
        let entry = self
            .entries
            .get(name)
            .ok_or_else(|| format!("voice: missing tensor {name}"))?;
        if entry.dtype != dtype {
            return Err(format!("voice: {name} is {}, not {dtype}", entry.dtype));
        }
        let (start, end) = entry.data_offsets;
        let count: usize = entry.shape.iter().product();
        let data = self
            .data
            .get(start..end)
            .filter(|data| data.len() == count * width)
            .ok_or_else(|| format!("voice: bad offsets for {name}"))?;
        Ok((data, &entry.shape))
    }

    fn scalar_i64(&self, name: &str) -> Result<i64, String> {
        let (data, _) = self.tensor(name, "I64", 8)?;
        let bytes = data
            .get(..8)
            .ok_or_else(|| format!("voice: empty {name}"))?;
        Ok(i64::from_le_bytes(bytes.try_into().unwrap()))
    }
}

impl Voice {
    pub fn load(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|error| format!("{}: {error}", path.display()))?;
        Self::parse(&bytes)
    }

    pub fn parse(bytes: &[u8]) -> Result<Self, String> {
        let file = SafeTensors::parse(bytes)?;
        let mut positions = None;
        let mut layers = Vec::with_capacity(LAYERS);
        for layer in 0..LAYERS {
            let prefix = format!("transformer.layers.{layer}.self_attn");
            let offset = file.scalar_i64(&format!("{prefix}/offset"))?;
            let pad = file.scalar_i64(&format!("{prefix}/pad"))?;
            if pad != 0 {
                return Err(format!("voice: layer {layer} has pad {pad}"));
            }
            let (data, shape) = file.tensor(&format!("{prefix}/cache"), "F32", 4)?;
            // [2, 1, P_max, H, D], dont les `offset` premières positions.
            let [2, 1, capacity, HEADS, HEAD_DIM] = *shape else {
                return Err(format!("voice: layer {layer} has shape {shape:?}"));
            };
            let offset = usize::try_from(offset)
                .ok()
                .filter(|offset| (1..=capacity).contains(offset))
                .ok_or_else(|| format!("voice: layer {layer} has offset {offset}"))?;
            if *positions.get_or_insert(offset) != offset {
                return Err("voice: layers disagree on the offset".into());
            }
            layers.push((data, capacity));
        }
        let positions = positions.unwrap_or(0);
        let block = HEADS * HEAD_DIM;
        let mut kv = vec![0.0f32; positions * KV_ROW];
        for (layer, (data, capacity)) in layers.into_iter().enumerate() {
            for half in 0..2 {
                for position in 0..positions {
                    let source = ((half * capacity) + position) * block * 4;
                    let target = position * KV_ROW + (layer * 2 + half) * block;
                    for (value, bytes) in kv[target..target + block]
                        .iter_mut()
                        .zip(data[source..source + block * 4].as_chunks::<4>().0)
                    {
                        *value = f32::from_le_bytes(*bytes);
                    }
                }
            }
        }
        if kv.iter().any(|value| !value.is_finite()) {
            return Err("voice: non-finite values".into());
        }
        Ok(Self { positions, kv })
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// Fichier safetensors de voix synthétique : valeur =
    /// `((couche·2 + k/v)·8 + position)·1024 + tête·64 + dim`, exacte en f32
    /// (au plus 8 positions).
    pub(crate) fn synthetic(capacity: usize, offset: usize, pad: i64) -> Vec<u8> {
        let mut header = serde_json::Map::new();
        let mut data: Vec<u8> = Vec::new();
        header.insert("__metadata__".into(), serde_json::json!({"format": "pt"}));
        for layer in 0..LAYERS {
            let prefix = format!("transformer.layers.{layer}.self_attn");
            let mut add = |name: String, dtype: &str, shape: Vec<usize>, bytes: Vec<u8>| {
                let start = data.len();
                data.extend_from_slice(&bytes);
                header.insert(
                    name,
                    serde_json::json!({"dtype": dtype, "shape": shape,
                        "data_offsets": [start, data.len()]}),
                );
            };
            add(
                format!("{prefix}/offset"),
                "I64",
                vec![1],
                (offset as i64).to_le_bytes().to_vec(),
            );
            add(
                format!("{prefix}/pad"),
                "I64",
                vec![1],
                pad.to_le_bytes().to_vec(),
            );
            let mut cache = Vec::new();
            for half in 0..2 {
                for position in 0..capacity {
                    for index in 0..HEADS * HEAD_DIM {
                        let value = (((layer * 2 + half) * 8 + position) * 1024 + index) as f32;
                        cache.extend_from_slice(&value.to_le_bytes());
                    }
                }
            }
            add(
                format!("{prefix}/cache"),
                "F32",
                vec![2, 1, capacity, HEADS, HEAD_DIM],
                cache,
            );
        }
        let header = serde_json::Value::Object(header).to_string();
        let mut file = (header.len() as u64).to_le_bytes().to_vec();
        file.extend_from_slice(header.as_bytes());
        file.extend_from_slice(&data);
        file
    }

    #[test]
    fn the_cache_is_laid_out_by_position() {
        let voice = Voice::parse(&synthetic(5, 3, 0)).unwrap();
        assert_eq!(voice.positions, 3);
        assert_eq!(voice.kv.len(), 3 * KV_ROW);
        let block = HEADS * HEAD_DIM;
        // Position 2, couche 4, v, élément 7.
        let value = voice.kv[2 * KV_ROW + (4 * 2 + 1) * block + 7];
        assert_eq!(value, (((4 * 2 + 1) * 8 + 2) * 1024 + 7) as f32);
        // Seules les `offset` premières positions sont gardées, chacune à sa
        // ligne.
        for (row, values) in voice.kv.chunks(KV_ROW).enumerate() {
            assert!(values.iter().all(|v| (*v as usize / 1024) % 8 == row));
        }
    }

    #[test]
    fn broken_voices_are_refused() {
        assert!(
            Voice::parse(&synthetic(5, 3, 1))
                .unwrap_err()
                .contains("pad")
        );
        assert!(
            Voice::parse(&synthetic(5, 6, 0))
                .unwrap_err()
                .contains("offset")
        );
        assert!(
            Voice::parse(&synthetic(5, 0, 0))
                .unwrap_err()
                .contains("offset")
        );
        let mut file = synthetic(2, 2, 0);
        file.truncate(file.len() - 4);
        assert!(Voice::parse(&file).unwrap_err().contains("offsets"));
        assert!(Voice::parse(b"1234").is_err());
        // NaN dans le cache.
        let mut file = synthetic(1, 1, 0);
        let nan = f32::NAN.to_le_bytes();
        let end = file.len();
        file[end - 4..].copy_from_slice(&nan);
        assert!(Voice::parse(&file).unwrap_err().contains("non-finite"));
    }
}
