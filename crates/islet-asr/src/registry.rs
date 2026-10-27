//! The catalogue of speech models Islet can run.
//!
//! Adding a model is one new [`ModelSpec`]: its files, sizes, and SHA-256
//! hashes are pinned here, and the downloader refuses anything that does not
//! match. The hashes of the large files are the Hugging Face LFS object ids,
//! which are SHA-256 of the file contents.

/// How the worker runs a model. Each family is one sherpa-onnx recognizer
/// configuration; the worker maps the kind to the right config.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EngineKind {
    /// NeMo transducer (Parakeet TDT): `encoder`, `decoder`, `joiner`, `tokens`.
    NemoTransducer,
}

impl EngineKind {
    /// Name used on the worker protocol.
    #[must_use]
    pub const fn wire_name(self) -> &'static str {
        match self {
            Self::NemoTransducer => "nemo_transducer",
        }
    }

    #[must_use]
    pub fn from_wire_name(name: &str) -> Option<Self> {
        match name {
            "nemo_transducer" => Some(Self::NemoTransducer),
            _ => None,
        }
    }
}

/// One downloadable file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    pub name: &'static str,
    pub url: &'static str,
    pub size: u64,
    /// Lowercase hex SHA-256.
    pub sha256: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelSpec {
    /// Stable id stored in `config.toml` (`dictation.model`).
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    /// Who made it, for the About/attribution text.
    pub maker: &'static str,
    pub languages: &'static str,
    pub license: &'static str,
    pub engine: EngineKind,
    /// Directory under the models folder.
    pub dir: &'static str,
    pub files: &'static [ModelFile],
}

impl ModelSpec {
    /// Total download size in bytes.
    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

pub const DEFAULT_MODEL_ID: &str = "parakeet-tdt-0.6b-v3-int8";

pub const PARAKEET_V3: ModelSpec = ModelSpec {
    id: "parakeet-tdt-0.6b-v3-int8",
    name: "Parakeet TDT 0.6B v3",
    summary: "Fast, accurate, and multilingual with punctuation and capitalization.",
    maker: "NVIDIA",
    languages: "25 European languages",
    license: "CC-BY-4.0",
    engine: EngineKind::NemoTransducer,
    dir: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8",
    files: &[
        ModelFile {
            name: "encoder.int8.onnx",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/encoder.int8.onnx",
            size: 652_184_281,
            sha256: "acfc2b4456377e15d04f0243af540b7fe7c992f8d898d751cf134c3a55fd2247",
        },
        ModelFile {
            name: "decoder.int8.onnx",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/decoder.int8.onnx",
            size: 11_845_275,
            sha256: "179e50c43d1a9de79c8a24149a2f9bac6eb5981823f2a2ed88d655b24248db4e",
        },
        ModelFile {
            name: "joiner.int8.onnx",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/joiner.int8.onnx",
            size: 6_355_277,
            sha256: "3164c13fc2821009440d20fcb5fdc78bff28b4db2f8d0f0b329101719c0948b3",
        },
        ModelFile {
            name: "tokens.txt",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v3-int8/resolve/main/tokens.txt",
            size: 93_939,
            sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
        },
    ],
};

pub const PARAKEET_V2: ModelSpec = ModelSpec {
    id: "parakeet-tdt-0.6b-v2-int8",
    name: "Parakeet TDT 0.6B v2",
    summary: "English only; a little more accurate on English than v3.",
    maker: "NVIDIA",
    languages: "English",
    license: "CC-BY-4.0",
    engine: EngineKind::NemoTransducer,
    dir: "sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8",
    files: &[
        ModelFile {
            name: "encoder.int8.onnx",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/encoder.int8.onnx",
            size: 652_184_296,
            sha256: "a32b12d17bbbc309d0686fbbcc2987b5e9b8333a7da83fa6b089f0a2acd651ab",
        },
        ModelFile {
            name: "decoder.int8.onnx",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/decoder.int8.onnx",
            size: 7_257_753,
            sha256: "b6bb64963457237b900e496ee9994b59294526439fbcc1fecf705b31a15c6b4e",
        },
        ModelFile {
            name: "joiner.int8.onnx",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/joiner.int8.onnx",
            size: 1_739_080,
            sha256: "7946164367946e7f9f29a122407c3252b680dbae9a51343eb2488d057c3c43d2",
        },
        ModelFile {
            name: "tokens.txt",
            url: "https://huggingface.co/csukuangfj/sherpa-onnx-nemo-parakeet-tdt-0.6b-v2-int8/resolve/main/tokens.txt",
            size: 9_384,
            sha256: "ec182b70dd42113aff6c5372c75cac58c952443eb22322f57bbd7f53977d497d",
        },
    ],
};

/// Every model offered in the app, in display order.
pub const MODELS: &[ModelSpec] = &[PARAKEET_V3, PARAKEET_V2];

/// Silero voice-activity detection, shared by every model. Hash pinned from
/// the official sherpa-onnx release asset.
pub const SILERO_VAD: ModelFile = ModelFile {
    name: "silero_vad.onnx",
    url: "https://github.com/k2-fsa/sherpa-onnx/releases/download/asr-models/silero_vad.onnx",
    size: 643_854,
    sha256: "9e2449e1087496d8d4caba907f23e0bd3f78d91fa552479bb9c23ac09cbb1fd6",
};

/// An optional download that is not a speech model: files to fetch, and
/// optionally one archive among them to unpack into [`AddOn::RUNTIME_DIR`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AddOn {
    pub id: &'static str,
    pub name: &'static str,
    pub summary: &'static str,
    pub maker: &'static str,
    pub license: &'static str,
    /// Directory under the models folder.
    pub dir: &'static str,
    pub files: &'static [ModelFile],
    /// Name of the file in `files` that is a zip to unpack.
    pub archive: Option<&'static str>,
    /// The model file the runtime loads.
    pub model: &'static str,
    /// Program to run, inside the unpacked runtime.
    pub program: &'static str,
}

impl AddOn {
    /// Where the archive is unpacked, under the add-on's directory.
    pub const RUNTIME_DIR: &'static str = "runtime";

    #[must_use]
    pub fn total_size(&self) -> u64 {
        self.files.iter().map(|file| file.size).sum()
    }
}

/// On-device AI cleanup: Qwen2.5 1.5B Instruct (4-bit) run by llama.cpp's CPU
/// server. Both pinned by SHA-256: the runtime is the official llama.cpp
/// release asset, the model the official Qwen GGUF.
pub const AI_CLEANUP: AddOn = AddOn {
    id: "ai-cleanup",
    name: "Qwen2.5 1.5B Instruct",
    summary: "Rewrites each dictation: drops what you corrected, fixes grammar, makes lists.",
    maker: "Qwen \u{b7} llama.cpp",
    license: "Apache-2.0 \u{b7} MIT",
    dir: "ai-cleanup",
    files: &[
        ModelFile {
            name: "llama-b11927-bin-win-cpu-x64.zip",
            url: "https://github.com/ggml-org/llama.cpp/releases/download/b11927/llama-b11927-bin-win-cpu-x64.zip",
            size: 24_227_896,
            sha256: "f70b7fe800e683f58fed3199fcf9c961932307f30916498a7758dce5f1d76ce9",
        },
        ModelFile {
            name: "qwen2.5-1.5b-instruct-q4_k_m.gguf",
            url: "https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF/resolve/main/qwen2.5-1.5b-instruct-q4_k_m.gguf",
            size: 1_117_320_736,
            sha256: "6a1a2eb6d15622bf3c96857206351ba97e1af16c30d7a74ee38970e434e9407e",
        },
    ],
    archive: Some("llama-b11927-bin-win-cpu-x64.zip"),
    model: "qwen2.5-1.5b-instruct-q4_k_m.gguf",
    program: "llama-server.exe",
};

/// Directory (under the models folder) holding files shared by all models.
pub const SUPPORT_DIR: &str = "support";

#[must_use]
pub fn find(id: &str) -> Option<&'static ModelSpec> {
    MODELS.iter().find(|model| model.id == id)
}

/// The model for `id`, or the default when the id is unknown (for example a
/// config written by a newer build).
#[must_use]
pub fn find_or_default(id: &str) -> &'static ModelSpec {
    find(id).unwrap_or(&PARAKEET_V3)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_findable() {
        for (index, model) in MODELS.iter().enumerate() {
            assert_eq!(find(model.id), Some(model));
            assert!(MODELS[index + 1..].iter().all(|other| other.id != model.id));
        }
        assert_eq!(find("nope"), None);
        assert_eq!(find_or_default("nope").id, DEFAULT_MODEL_ID);
    }

    #[test]
    fn urls_point_at_the_file_they_name() {
        for model in MODELS {
            for file in model.files {
                assert!(
                    file.url.ends_with(file.name),
                    "{} -> {}",
                    file.name,
                    file.url
                );
                assert!(file.url.starts_with("https://"));
                assert_eq!(file.sha256.len(), 64);
                assert!(file.sha256.chars().all(|c| c.is_ascii_hexdigit()));
            }
        }
        assert_eq!(PARAKEET_V3.total_size(), 670_478_772);
        for file in AI_CLEANUP.files {
            assert!(file.url.ends_with(file.name) && file.url.starts_with("https://"));
            assert_eq!(file.sha256.len(), 64);
        }
        assert!(AI_CLEANUP
            .files
            .iter()
            .any(|file| Some(file.name) == AI_CLEANUP.archive));
        assert!(AI_CLEANUP.files.iter().any(|file| file.name == AI_CLEANUP.model));
    }

    #[test]
    fn engine_names_round_trip() {
        let kind = EngineKind::NemoTransducer;
        assert_eq!(EngineKind::from_wire_name(kind.wire_name()), Some(kind));
        assert_eq!(EngineKind::from_wire_name("bogus"), None);
    }
}
