use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use sha2::{Digest, Sha256};

use crate::config::models_dir;
use crate::error::TranscribeError;

const WHISPER_MODEL_TABLE: &str = include_str!("../../whisper-models.toml");
const DOWNLOAD_CHUNK_BYTES: usize = 1 << 20;
const PROGRESS_LOG_STEP_PERCENT: u64 = 10;

#[derive(Debug, Deserialize)]
struct WhisperModelTable {
    download_base_url: String,
    models: BTreeMap<String, KnownWhisperModel>,
}

#[derive(Debug, Deserialize)]
struct KnownWhisperModel {
    file_name: String,
    sha256: String,
}

pub fn locate_or_download_model(model: &str) -> Result<PathBuf, TranscribeError> {
    if looks_like_model_path(model) {
        return existing_model_file(Path::new(model));
    }
    let table: WhisperModelTable = toml::from_str(WHISPER_MODEL_TABLE)?;
    let Some(known_model) = table.models.get(model) else {
        tracing::error!(
            model,
            known_models = ?table.models.keys().collect::<Vec<_>>(),
            "Unknown whisper model name in [daemon.whisper] model"
        );
        return Err(TranscribeError::UnknownModelName);
    };
    let model_path = models_dir().join(&known_model.file_name);
    if model_path.exists() {
        return Ok(model_path);
    }
    let download_url = format!("{}/{}", table.download_base_url, known_model.file_name);
    download_verified_model(&download_url, &known_model.sha256, &model_path)?;
    Ok(model_path)
}

fn looks_like_model_path(model: &str) -> bool {
    model.contains('/') || model.ends_with(".bin")
}

fn existing_model_file(path: &Path) -> Result<PathBuf, TranscribeError> {
    if path.exists() {
        return Ok(path.to_path_buf());
    }
    tracing::error!(path = %path.display(), "Whisper model file does not exist");
    Err(TranscribeError::ModelNotFound)
}

fn download_verified_model(
    download_url: &str,
    expected_sha256: &str,
    model_path: &Path,
) -> Result<(), TranscribeError> {
    let partial_path = model_path.with_extension("bin.part");
    if let Some(models_directory) = model_path.parent() {
        std::fs::create_dir_all(models_directory).inspect_err(|error| {
            tracing::error!(%error, path = %models_directory.display(), "Could not create the models directory")
        })?;
    }
    tracing::info!(
        url = download_url,
        destination = %model_path.display(),
        "Downloading the whisper model once; dictation starts when it is ready"
    );
    let response = ureq::get(download_url).call().map_err(Box::new).inspect_err(
        |error| tracing::error!(%error, url = download_url, "Could not download the whisper model"),
    )?;
    let total_bytes = response
        .header("Content-Length")
        .and_then(|length| length.parse::<u64>().ok());
    let actual_sha256 = stream_to_file_with_digest(response.into_reader(), &partial_path, total_bytes)
        .inspect_err(|error| tracing::error!(%error, path = %partial_path.display(), "Could not write the downloaded whisper model"))?;
    if actual_sha256 != expected_sha256 {
        let _ = std::fs::remove_file(&partial_path);
        tracing::error!(
            expected = expected_sha256,
            actual = actual_sha256,
            url = download_url,
            "Downloaded whisper model has the wrong checksum; removed it"
        );
        return Err(TranscribeError::ModelChecksumMismatch);
    }
    std::fs::rename(&partial_path, model_path)?;
    tracing::info!(path = %model_path.display(), "Whisper model downloaded and verified");
    Ok(())
}

fn stream_to_file_with_digest(
    mut body: impl Read,
    destination: &Path,
    total_bytes: Option<u64>,
) -> std::io::Result<String> {
    let mut writer = BufWriter::new(File::create(destination)?);
    let mut digest = Sha256::new();
    let mut chunk = vec![0_u8; DOWNLOAD_CHUNK_BYTES];
    let mut written_bytes = 0_u64;
    let mut logged_percent = 0_u64;
    loop {
        let read_bytes = body.read(&mut chunk)?;
        if read_bytes == 0 {
            break;
        }
        let received = &chunk[..read_bytes];
        digest.update(received);
        writer.write_all(received)?;
        written_bytes += read_bytes as u64;
        if let Some(total_bytes) = total_bytes.filter(|total| *total > 0) {
            let percent = written_bytes * 100 / total_bytes;
            if percent >= logged_percent + PROGRESS_LOG_STEP_PERCENT {
                logged_percent = percent - percent % PROGRESS_LOG_STEP_PERCENT;
                tracing::info!("Whisper model download {logged_percent}%");
            }
        }
    }
    writer.flush()?;
    Ok(hex::encode(digest.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_table_parses_and_has_a_hash_per_model() {
        let table: WhisperModelTable = toml::from_str(WHISPER_MODEL_TABLE).expect("table parses");
        assert!(table.models.contains_key("base.en"));
        assert!(table.models.contains_key("large-v3-turbo"));
        assert!(table
            .models
            .values()
            .all(|model| model.sha256.len() == 64 && model.file_name.ends_with(".bin")));
    }

    #[test]
    fn default_model_name_is_in_the_table() {
        let table: WhisperModelTable = toml::from_str(WHISPER_MODEL_TABLE).expect("table parses");
        let default_model = crate::config::WhisperConfig::default().model;
        assert!(table.models.contains_key(&default_model));
    }

    #[test]
    fn paths_and_bin_files_are_not_looked_up_by_name() {
        assert!(looks_like_model_path(
            "/nix/store/abc-ggml-large-v3-turbo.bin"
        ));
        assert!(looks_like_model_path("ggml-custom.bin"));
        assert!(!looks_like_model_path("large-v3-turbo"));
    }

    #[test]
    fn missing_explicit_path_is_an_error() {
        assert!(matches!(
            locate_or_download_model("/definitely/not/here/ggml-x.bin"),
            Err(TranscribeError::ModelNotFound)
        ));
    }

    #[test]
    fn unknown_name_is_an_error() {
        assert!(matches!(
            locate_or_download_model("gigantic-v9"),
            Err(TranscribeError::UnknownModelName)
        ));
    }

    #[test]
    fn digest_matches_known_sha256_of_streamed_bytes() {
        let directory = std::env::temp_dir().join("notulist-model-digest-test");
        std::fs::create_dir_all(&directory).expect("temp dir");
        let destination = directory.join("payload.bin.part");
        let digest =
            stream_to_file_with_digest(&b"abc"[..], &destination, Some(3)).expect("writes");
        assert_eq!(
            digest,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        assert_eq!(std::fs::read(&destination).expect("reads"), b"abc");
        let _ = std::fs::remove_dir_all(&directory);
    }
}
