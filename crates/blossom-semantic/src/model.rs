//! The built-in models: which files, from which pinned revision, with which
//! checksums, and how each is used. Downloading and running them needs the
//! `builtin-model` feature; this part is always there so settings can be
//! read and shown without it.

use std::path::{Path, PathBuf};

/// A built-in embedding model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// `BAAI/bge-small-en-v1.5`: English, 384 dimensions, ~130 MB.
    English,
    /// `intfloat/multilingual-e5-small`: about 100 languages, 384
    /// dimensions, ~490 MB.
    Multilingual,
}

/// How a model turns per-token vectors into one vector.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pooling {
    /// The first (`[CLS]`) token's vector.
    Cls,
    /// The mean over the real (unpadded) tokens.
    Mean,
}

/// A file of a model, pinned by size and SHA-256.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModelFile {
    /// Name in the repository and on disk.
    pub name: &'static str,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256, lowercase hex.
    pub sha256: &'static str,
}

impl Model {
    /// Every built-in model.
    pub const ALL: [Model; 2] = [Model::English, Model::Multilingual];

    /// The config name: `english` or `multilingual`.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Model::English => "english",
            Model::Multilingual => "multilingual",
        }
    }

    /// The model for a config name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Model> {
        Model::ALL
            .into_iter()
            .find(|m| m.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The Hugging Face repository.
    #[must_use]
    pub fn repo(self) -> &'static str {
        match self {
            Model::English => "BAAI/bge-small-en-v1.5",
            Model::Multilingual => "intfloat/multilingual-e5-small",
        }
    }

    /// The pinned repository commit.
    #[must_use]
    pub fn revision(self) -> &'static str {
        match self {
            Model::English => "5c38ec7c405ec4b44b94cc5a9bb96e735b38267a",
            Model::Multilingual => "614241f622f53c4eeff9890bdc4f31cfecc418b3",
        }
    }

    /// The files needed, pinned.
    #[must_use]
    pub fn files(self) -> &'static [ModelFile] {
        match self {
            Model::English => &[
                ModelFile {
                    name: "config.json",
                    size: 743,
                    sha256: "094f8e891b932f2000c92cfc663bac4c62069f5d8af5b5278c4306aef3084750",
                },
                ModelFile {
                    name: "tokenizer.json",
                    size: 711_396,
                    sha256: "d241a60d5e8f04cc1b2b3e9ef7a4921b27bf526d9f6050ab90f9267a1f9e5c66",
                },
                ModelFile {
                    name: "model.safetensors",
                    size: 133_466_304,
                    sha256: "3c9f31665447c8911517620762200d2245a2518d6e7208acc78cd9db317e21ad",
                },
            ],
            Model::Multilingual => &[
                ModelFile {
                    name: "config.json",
                    size: 655,
                    sha256: "69137736cab8b8903a07fe8afaafdda25aac55415a12a55d1bffa9f581abf959",
                },
                ModelFile {
                    name: "tokenizer.json",
                    size: 17_082_730,
                    sha256: "0b44a9d7b51c3c62626640cda0e2c2f70fdacdc25bbbd68038369d14ebdf4c39",
                },
                ModelFile {
                    name: "model.safetensors",
                    size: 470_641_600,
                    sha256: "1a55775f53449dac10a2bcbc312469fac40b96d53198c407081a831f81c98477",
                },
            ],
        }
    }

    /// Total download size in bytes.
    #[must_use]
    pub fn download_size(self) -> u64 {
        self.files().iter().map(|f| f.size).sum()
    }

    /// The embedder id: the model plus its revision, so a new revision gets
    /// fresh vectors.
    #[must_use]
    pub fn id(self) -> String {
        let short = &self.revision()[..7];
        match self {
            Model::English => format!("bge-small-en-v1.5-{short}"),
            Model::Multilingual => format!("multilingual-e5-small-{short}"),
        }
    }

    /// How token vectors are pooled (from the model's `1_Pooling/config.json`).
    #[must_use]
    pub fn pooling(self) -> Pooling {
        match self {
            Model::English => Pooling::Cls,
            Model::Multilingual => Pooling::Mean,
        }
    }

    /// Put before a search query, as the model was trained.
    #[must_use]
    pub fn query_prefix(self) -> &'static str {
        match self {
            Model::English => "Represent this sentence for searching relevant passages: ",
            Model::Multilingual => "query: ",
        }
    }

    /// Put before each passage, as the model was trained.
    #[must_use]
    pub fn passage_prefix(self) -> &'static str {
        match self {
            Model::English => "",
            Model::Multilingual => "passage: ",
        }
    }

    /// Where the files live under the data folder.
    #[must_use]
    pub fn dir(self, data_dir: &Path) -> PathBuf {
        data_dir.join("models").join(self.id())
    }

    /// Whether every file is present with its pinned size. (The checksum is
    /// checked when downloading.)
    #[must_use]
    pub fn is_downloaded(self, data_dir: &Path) -> bool {
        let dir = self.dir(data_dir);
        self.files().iter().all(|f| {
            std::fs::metadata(dir.join(f.name)).is_ok_and(|m| m.is_file() && m.len() == f.size)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_ids_and_folders() {
        assert_eq!(Model::from_name(" English "), Some(Model::English));
        assert_eq!(Model::from_name("multilingual"), Some(Model::Multilingual));
        assert_eq!(Model::from_name("server"), None);
        assert_eq!(Model::English.id(), "bge-small-en-v1.5-5c38ec7");
        assert_eq!(
            Model::Multilingual.dir(Path::new("/d")),
            Path::new("/d/models/multilingual-e5-small-614241f")
        );
        assert_eq!(Model::English.download_size(), 134_178_443);
        for m in Model::ALL {
            for f in m.files() {
                assert_eq!(f.sha256.len(), 64);
            }
        }
    }

    #[test]
    fn downloaded_means_every_file_at_its_size() {
        let dir = tempfile::tempdir().unwrap();
        let m = Model::English;
        assert!(!m.is_downloaded(dir.path()));
        let folder = m.dir(dir.path());
        std::fs::create_dir_all(&folder).unwrap();
        for f in m.files() {
            let file = std::fs::File::create(folder.join(f.name)).unwrap();
            file.set_len(f.size).unwrap();
        }
        assert!(m.is_downloaded(dir.path()));
        std::fs::File::create(folder.join("config.json"))
            .unwrap()
            .set_len(1)
            .unwrap();
        assert!(!m.is_downloaded(dir.path()));
    }
}
