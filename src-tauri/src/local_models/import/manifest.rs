//! Manifestes d'un Ollama (logique pure) : JSON Docker v2 dont les couches
//! (`layers[].mediaType`) disent ce qu'est chaque blob (`blobs/sha256-<hex>`),
//! ou liste de variantes (`application/vnd.ollama.manifest.list.v2+json`,
//! Ollama 0.40 : `runner: ggml` pour son moteur, `runner: llamacpp` pour
//! llama.cpp, poids et projecteur séparés). Rapport `99-llamacpp-preuve.md`
//! § 2 : la variante `llamacpp` se charge dans llama-server, la `ggml` non
//! quand la vision y est fondue.

use serde::Deserialize;

use super::names::is_digest_hex;

pub const MODEL: &str = "application/vnd.ollama.image.model";
pub const PROJECTOR: &str = "application/vnd.ollama.image.projector";
pub const PARAMS: &str = "application/vnd.ollama.image.params";
pub const SYSTEM: &str = "application/vnd.ollama.image.system";
pub const TEMPLATE: &str = "application/vnd.ollama.image.template";
pub const ADAPTER: &str = "application/vnd.ollama.image.adapter";
pub const TENSOR: &str = "application/vnd.ollama.image.tensor";
pub const LIST: &str = "application/vnd.ollama.manifest.list.v2+json";

/// Taille maximale d'un manifeste ou d'une liste lus.
pub const MAX_MANIFEST: u64 = 1024 * 1024;
/// Taille maximale d'une couche `params` ou `system` lue.
pub const MAX_TEXT: u64 = 64 * 1024;

/// Couche d'un manifeste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layer {
    pub media_type: String,
    /// Empreinte SHA-256 du blob (hexadécimal, minuscules).
    pub digest: String,
    pub size: u64,
}

/// Manifeste lu.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Manifest {
    pub layers: Vec<Layer>,
    /// `runner` d'Ollama 0.40 (`ggml`, `llamacpp`) ; `None` avant.
    pub runner: Option<String>,
    /// Liste de variantes citée en couche (manifeste d'Ollama 0.40 qui
    /// garde, à l'ancienne place, la variante `ggml` et la liste).
    pub list: Option<String>,
    /// Modèle du cloud d'Ollama (`remote_host` écrit dans le manifeste lui-
    /// même ; le cas d'Ollama est le blob de configuration : [`config_is_cloud`]).
    pub cloud: bool,
    /// Empreinte du blob de configuration (`config.digest`, `ConfigV2`
    /// d'Ollama : `remote_host`, `model_format`…).
    pub config: Option<String>,
}

/// Entrée d'une liste de variantes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variant {
    pub digest: String,
    pub runner: Option<String>,
}

/// Contenu d'un fichier de manifeste.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parsed {
    Manifest(Manifest),
    List(Vec<Variant>),
}

#[derive(Deserialize)]
struct RawManifest {
    #[serde(rename = "mediaType", default)]
    media_type: Option<String>,
    #[serde(default)]
    layers: Vec<RawLayer>,
    #[serde(default)]
    manifests: Vec<RawLayer>,
    #[serde(default)]
    runner: Option<String>,
    #[serde(default)]
    config: Option<RawConfig>,
    #[serde(default)]
    remote_host: Option<String>,
}

#[derive(Deserialize)]
struct RawConfig {
    #[serde(default)]
    digest: Option<String>,
    #[serde(default)]
    remote_host: Option<String>,
    #[serde(default)]
    remote_model: Option<String>,
}

/// Le blob de configuration (`ConfigV2` d'Ollama,
/// `types/model/config.go`) est celui d'un modèle du cloud :
/// `remote_host` ou `remote_model` non vides (comme
/// `server/model_show_cache.go` d'Ollama).
pub fn config_is_cloud(bytes: &[u8]) -> bool {
    if bytes.len() as u64 > MAX_MANIFEST {
        return false;
    }
    serde_json::from_slice::<RawConfig>(bytes).is_ok_and(|config| {
        config.remote_host.is_some_and(|host| !host.is_empty())
            || config.remote_model.is_some_and(|model| !model.is_empty())
    })
}

#[derive(Deserialize)]
struct RawLayer {
    #[serde(rename = "mediaType", default)]
    media_type: String,
    #[serde(default)]
    digest: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    runner: Option<String>,
}

/// `sha256:<hex>` → `<hex>` ; `None` pour une autre forme.
pub fn digest_hex(digest: &str) -> Option<String> {
    let hex = digest.strip_prefix("sha256:")?.to_ascii_lowercase();
    is_digest_hex(&hex).then_some(hex)
}

/// Nom du blob d'une empreinte (`sha256-<hex>`, sous Linux comme sous
/// Windows depuis Ollama 0.1.x).
pub fn blob_name(hex: &str) -> String {
    format!("sha256-{hex}")
}

/// Lit un manifeste ou une liste ; `None` s'il est illisible ou trop gros.
/// Une couche à l'empreinte illisible est ignorée.
pub fn parse(bytes: &[u8]) -> Option<Parsed> {
    if bytes.len() as u64 > MAX_MANIFEST {
        return None;
    }
    let raw: RawManifest = serde_json::from_slice(bytes).ok()?;
    if raw.media_type.as_deref() == Some(LIST)
        || (!raw.manifests.is_empty() && raw.layers.is_empty())
    {
        return Some(Parsed::List(
            raw.manifests
                .into_iter()
                .filter_map(|entry| {
                    Some(Variant {
                        digest: digest_hex(&entry.digest)?,
                        runner: entry.runner,
                    })
                })
                .collect(),
        ));
    }
    let mut list = None;
    let mut layers = Vec::new();
    for layer in raw.layers {
        let Some(digest) = digest_hex(&layer.digest) else {
            continue;
        };
        if layer.media_type == LIST {
            list.get_or_insert(digest);
            continue;
        }
        layers.push(Layer {
            media_type: layer.media_type,
            digest,
            size: layer.size,
        });
    }
    let config = raw
        .config
        .as_ref()
        .and_then(|config| config.digest.as_deref().and_then(digest_hex));
    let cloud = raw.remote_host.is_some_and(|host| !host.is_empty())
        || raw
            .config
            .and_then(|config| config.remote_host)
            .is_some_and(|host| !host.is_empty());
    Some(Parsed::Manifest(Manifest {
        layers,
        runner: raw.runner,
        list,
        cloud,
        config,
    }))
}

impl Manifest {
    /// Première couche du type `media_type`.
    pub fn layer(&self, media_type: &str) -> Option<&Layer> {
        self.layers
            .iter()
            .find(|layer| layer.media_type == media_type)
    }

    /// Nombre de couches du type `media_type`.
    pub fn count(&self, media_type: &str) -> usize {
        self.layers
            .iter()
            .filter(|layer| layer.media_type == media_type)
            .count()
    }
}

/// Variante retenue pour llama.cpp.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chosen {
    pub manifest: Manifest,
    /// Variante `runner: llamacpp` d'une liste.
    pub from_list: bool,
}

/// Manifeste à importer d'après le fichier `top` : pour une liste (ou un
/// manifeste qui en cite une), la variante `llamacpp` si elle se lit et que
/// ses blobs sont tous là (`blob_ok`) ; sinon le manifeste lui-même (ou la
/// première variante d'une liste qui se lit). `read_blob` lit un blob
/// d'après son empreinte (taille bornée par l'appelant).
pub fn choose(
    top: Parsed,
    read_blob: &dyn Fn(&str) -> Option<Vec<u8>>,
    blob_ok: &dyn Fn(&Layer) -> bool,
) -> Option<Chosen> {
    let read_manifest = |digest: &str| match parse(&read_blob(digest)?)? {
        Parsed::Manifest(manifest) => Some(manifest),
        Parsed::List(_) => None,
    };
    let llamacpp = |variants: &[Variant]| {
        variants
            .iter()
            .filter(|variant| variant.runner.as_deref() == Some("llamacpp"))
            .filter_map(|variant| read_manifest(&variant.digest))
            .find(|manifest| {
                manifest.layer(MODEL).is_some()
                    && manifest
                        .layers
                        .iter()
                        .filter(|layer| [MODEL, PROJECTOR].contains(&layer.media_type.as_str()))
                        .all(blob_ok)
            })
    };
    match top {
        Parsed::List(variants) => {
            if let Some(manifest) = llamacpp(&variants) {
                return Some(Chosen {
                    manifest,
                    from_list: true,
                });
            }
            variants
                .iter()
                .find_map(|variant| read_manifest(&variant.digest))
                .map(|manifest| Chosen {
                    manifest,
                    from_list: false,
                })
        }
        Parsed::Manifest(manifest) => {
            if manifest.runner.as_deref() != Some("llamacpp")
                && let Some(list) = &manifest.list
                && let Some(Parsed::List(variants)) = read_blob(list).and_then(|b| parse(&b))
                && let Some(chosen) = llamacpp(&variants)
            {
                return Some(Chosen {
                    manifest: chosen,
                    from_list: true,
                });
            }
            Some(Chosen {
                manifest,
                from_list: false,
            })
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    // Manifestes réels relevés dans l'Ollama de test (0.40) le 8 nov. 2026.
    const QWEN3: &str = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:b0830f4ff6a0220cfd995455206353b0ed23c0aee865218b154b7a75087b4e55","size":490},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:7f4030143c1c477224c5434f8272c662a8b042079a0a584f0a27a1684fe2e1fa","size":522640096},{"mediaType":"application/vnd.ollama.image.template","digest":"sha256:ae370d884f108d16e7cc8fd5259ebc5773a0afa6e078b11f4ed7e39a27e0dfc4","size":1723},{"mediaType":"application/vnd.ollama.image.license","digest":"sha256:d18a5cc71b84bc4af394a31116bd3932b42241de70c77d2b76d69a314ec8aa12","size":11338},{"mediaType":"application/vnd.ollama.image.params","digest":"sha256:cff3f395ef3756ab63e58b0ad1b32bb6f802905cae1472e6a12034e4246fbbdb","size":120},{"mediaType":"application/vnd.docker.distribution.manifest.v2+json","digest":"sha256:7df6b6e09427a769808717c0a93cadc4ae99ed4eb8bf5ca557c90846becea435","size":858}]}"#;
    pub(crate) const QWEN35_TOP: &str = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:b14c6eab49f986e99b2daf9cedac5183c74d4f5ce2585ebb52b8492778adec39","size":476},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:afb707b6b8fac6e475acc42bc8380fc0b8d2e0e4190be5a969fbf62fcc897db5","size":1036034688},{"mediaType":"application/vnd.ollama.image.license","digest":"sha256:9be69ef463066202c1b1bd299aaf42bad370a01ba4b40d293617859720776c17","size":11354},{"mediaType":"application/vnd.ollama.image.params","digest":"sha256:9371364b27a52acac9d87f88bd93c9db1174d8d6ec57f6888925cdc1788871ff","size":65},{"mediaType":"application/vnd.ollama.manifest.list.v2+json","digest":"sha256:501b519780d7dbfb545bdd2e8b743de54952c9fccc614030d21826abe502d14e","size":464},{"mediaType":"application/vnd.docker.distribution.manifest.v2+json","digest":"sha256:34227f3be981a756d2df36f805041f455ea26ed189513741b338b643d4e107f3","size":741},{"mediaType":"application/vnd.docker.distribution.manifest.v2+json","digest":"sha256:f1e6f0684a22457e8eb5b09d26041e8171c248c6b252c267a0b35ac5afc24ed6","size":943}],"runner":"ggml","format":"gguf"}"#;
    pub(crate) const QWEN35_LIST: &str = r#"{"schemaVersion":2,"mediaType":"application/vnd.ollama.manifest.list.v2+json","manifests":[{"mediaType":"application/vnd.docker.distribution.manifest.v2+json","digest":"sha256:34227f3be981a756d2df36f805041f455ea26ed189513741b338b643d4e107f3","runner":"ggml","format":"gguf"},{"mediaType":"application/vnd.docker.distribution.manifest.v2+json","digest":"sha256:f1e6f0684a22457e8eb5b09d26041e8171c248c6b252c267a0b35ac5afc24ed6","runner":"llamacpp","format":"gguf"}]}"#;
    pub(crate) const QWEN35_LLAMACPP: &str = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:3b69acc357b6a8a986e544cddb976e636d96df1b45e30739e5e12c2dafec7031","size":185},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:4a163d11fddf162ce3b927f36f0ec8e9aedaea5990dd4719802ac8db2577b957","size":834584192},{"mediaType":"application/vnd.ollama.image.projector","digest":"sha256:29bb4248dbc9b78f4d5833bf3761a629e48fa045d23e1eb4309c2819d36671dd","size":207345696},{"mediaType":"application/vnd.ollama.image.license","digest":"sha256:9be69ef463066202c1b1bd299aaf42bad370a01ba4b40d293617859720776c17","size":11354,"from":"qwen3.5:0.8b"},{"mediaType":"application/vnd.ollama.image.params","digest":"sha256:9371364b27a52acac9d87f88bd93c9db1174d8d6ec57f6888925cdc1788871ff","size":65,"from":"qwen3.5:0.8b"}],"runner":"llamacpp","format":"gguf"}"#;
    pub(crate) const QWEN35_GGML: &str = r#"{"schemaVersion":2,"mediaType":"application/vnd.docker.distribution.manifest.v2+json","config":{"mediaType":"application/vnd.docker.container.image.v1+json","digest":"sha256:b14c6eab49f986e99b2daf9cedac5183c74d4f5ce2585ebb52b8492778adec39","size":476},"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"sha256:afb707b6b8fac6e475acc42bc8380fc0b8d2e0e4190be5a969fbf62fcc897db5","size":1036034688},{"mediaType":"application/vnd.ollama.image.license","digest":"sha256:9be69ef463066202c1b1bd299aaf42bad370a01ba4b40d293617859720776c17","size":11354},{"mediaType":"application/vnd.ollama.image.params","digest":"sha256:9371364b27a52acac9d87f88bd93c9db1174d8d6ec57f6888925cdc1788871ff","size":65}],"runner":"ggml","format":"gguf"}"#;

    const LIST_DIGEST: &str = "501b519780d7dbfb545bdd2e8b743de54952c9fccc614030d21826abe502d14e";
    const GGML_DIGEST: &str = "34227f3be981a756d2df36f805041f455ea26ed189513741b338b643d4e107f3";
    const LLAMACPP_DIGEST: &str =
        "f1e6f0684a22457e8eb5b09d26041e8171c248c6b252c267a0b35ac5afc24ed6";

    fn blobs(digest: &str) -> Option<Vec<u8>> {
        match digest {
            LIST_DIGEST => Some(QWEN35_LIST.into()),
            GGML_DIGEST => Some(QWEN35_GGML.into()),
            LLAMACPP_DIGEST => Some(QWEN35_LLAMACPP.into()),
            _ => None,
        }
    }

    #[test]
    fn a_plain_manifest_gives_its_layers() {
        let Some(Parsed::Manifest(manifest)) = parse(QWEN3.as_bytes()) else {
            panic!("manifest");
        };
        let model = manifest.layer(MODEL).unwrap();
        assert_eq!(
            model.digest,
            "7f4030143c1c477224c5434f8272c662a8b042079a0a584f0a27a1684fe2e1fa"
        );
        assert_eq!(model.size, 522_640_096);
        assert!(manifest.layer(PARAMS).is_some());
        assert!(manifest.layer(PROJECTOR).is_none());
        assert_eq!(manifest.list, None);
        assert!(!manifest.cloud);
        assert_eq!(
            manifest.config.as_deref(),
            Some("b0830f4ff6a0220cfd995455206353b0ed23c0aee865218b154b7a75087b4e55")
        );
        let chosen = choose(Parsed::Manifest(manifest.clone()), &blobs, &|_| true).unwrap();
        assert_eq!(chosen.manifest, manifest);
        assert!(!chosen.from_list);
    }

    #[test]
    fn the_llamacpp_variant_wins_when_its_blobs_are_there() {
        let top = parse(QWEN35_TOP.as_bytes()).unwrap();
        let Parsed::Manifest(manifest) = &top else {
            panic!("manifest");
        };
        assert_eq!(manifest.list.as_deref(), Some(LIST_DIGEST));
        let chosen = choose(top.clone(), &blobs, &|_| true).unwrap();
        assert!(chosen.from_list);
        assert_eq!(
            chosen.manifest.layer(MODEL).unwrap().digest,
            "4a163d11fddf162ce3b927f36f0ec8e9aedaea5990dd4719802ac8db2577b957"
        );
        assert_eq!(
            chosen.manifest.layer(PROJECTOR).unwrap().digest,
            "29bb4248dbc9b78f4d5833bf3761a629e48fa045d23e1eb4309c2819d36671dd"
        );
        // Projecteur absent : le manifeste d'origine (vision fondue).
        let chosen = choose(top.clone(), &blobs, &|layer| layer.media_type != PROJECTOR).unwrap();
        assert!(!chosen.from_list);
        assert_eq!(
            chosen.manifest.layer(MODEL).unwrap().digest,
            "afb707b6b8fac6e475acc42bc8380fc0b8d2e0e4190be5a969fbf62fcc897db5"
        );
        // Liste illisible : le manifeste d'origine.
        let chosen = choose(top, &|_| None, &|_| true).unwrap();
        assert!(!chosen.from_list);
        // Rangement d'Ollama 0.40 (`manifests-v2/` vers la liste elle-même).
        let list = parse(QWEN35_LIST.as_bytes()).unwrap();
        assert!(matches!(&list, Parsed::List(variants) if variants.len() == 2));
        let chosen = choose(list.clone(), &blobs, &|_| true).unwrap();
        assert!(chosen.from_list);
        let chosen = choose(list, &blobs, &|layer| layer.media_type != PROJECTOR).unwrap();
        assert!(!chosen.from_list);
        assert_eq!(chosen.manifest.runner.as_deref(), Some("ggml"));
    }

    #[test]
    fn odd_manifests_are_read_safely() {
        assert_eq!(parse(b"not json"), None);
        assert_eq!(parse(&vec![b' '; MAX_MANIFEST as usize + 1]), None);
        let Some(Parsed::Manifest(manifest)) = parse(
            br#"{"layers":[{"mediaType":"application/vnd.ollama.image.model","digest":"md5:abc","size":1}]}"#,
        ) else {
            panic!("manifest");
        };
        assert!(manifest.layers.is_empty());
        let Some(Parsed::Manifest(cloud)) =
            parse(br#"{"config":{"remote_host":"https://ollama.com:443"},"layers":[]}"#)
        else {
            panic!("manifest");
        };
        assert!(cloud.cloud);
        // Forme réelle d'Ollama : `remote_host` dans le blob de configuration
        // (`ConfigV2`, `types/model/config.go`), le manifeste ne cite que son
        // empreinte.
        assert!(config_is_cloud(
            br#"{"model_format":"","remote_host":"https://ollama.com:443","remote_model":"gpt-oss:120b"}"#
        ));
        assert!(config_is_cloud(br#"{"remote_model":"gpt-oss:20b"}"#));
        assert!(!config_is_cloud(
            br#"{"model_format":"gguf","model_family":"qwen3","file_type":"Q4_K_M"}"#
        ));
        assert!(!config_is_cloud(b"not json"));
        assert_eq!(
            digest_hex(&format!("sha256:{}", "A".repeat(64))),
            Some("a".repeat(64))
        );
        assert_eq!(digest_hex("sha256:../../etc"), None);
        assert_eq!(
            blob_name(&"a".repeat(64)),
            format!("sha256-{}", "a".repeat(64))
        );
    }
}
