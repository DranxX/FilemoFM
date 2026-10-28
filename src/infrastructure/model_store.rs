use crate::shared::constants::*;
use crate::shared::models::*;
use crate::shared::traits::ModelRepository;
use reqwest::{
    Url,
    blocking::{Client, Response},
};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::net::{IpAddr, ToSocketAddrs};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub struct HuggingFaceModelStore {
    paths: RuntimePaths,
}
impl HuggingFaceModelStore {
    pub fn new(paths: RuntimePaths) -> Self {
        Self { paths }
    }
    /// Verifies a fully downloaded `.part` file and atomically publishes it with its manifest.
    fn publish_partial(
        &self,
        artifact: &ModelArtifact,
        partial: &Path,
        target: &Path,
    ) -> Result<VerifiedModel, String> {
        if digest_file(partial)? != artifact.sha256 {
            let _ = fs::remove_file(partial);
            return Err("Artifact size or checksum mismatch; the partial file was removed".into());
        }
        verify_gguf(partial)?;
        fs::rename(partial, target).map_err(|_| "Artifact could not be published")?;
        let model = VerifiedModel {
            artifact: artifact.clone(),
            path: target.to_path_buf(),
        };
        let bytes = serde_json::to_vec_pretty(&model).map_err(|_| "Model manifest failed")?;
        super::config::atomic_write(
            &self.paths.models.join(format!("{}.json", artifact.sha256)),
            &bytes,
        )
        .map_err(|_| "Model manifest could not be saved")?;
        Ok(model)
    }
}

pub fn digest_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|_| "Artifact could not be read")?;
    let mut hash = Sha256::new();
    let mut buffer = vec![0u8; 128 * 1024];
    loop {
        let count = file
            .read(&mut buffer)
            .map_err(|_| "Artifact could not be read")?;
        if count == 0 {
            break;
        }
        hash.update(&buffer[..count]);
    }
    Ok(format!("{:x}", hash.finalize()))
}

pub fn json_response(response: Response) -> Result<Value, String> {
    if !response.status().is_success() {
        return Err("Service rejected the request".into());
    }
    let mut bytes = Vec::new();
    response
        .take(MAX_HTTP_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| "Response could not be read")?;
    if bytes.len() as u64 > MAX_HTTP_BYTES {
        return Err("Service response is too large".into());
    }
    serde_json::from_slice(&bytes).map_err(|_| "Service response format is invalid".into())
}

fn is_public(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(ip) => {
            !(ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_unspecified()
                || ip.is_broadcast()
                || ip.is_multicast()
                || ip.octets()[0] == 0
                || ip.octets()[0] >= 240)
        }
        IpAddr::V6(ip) => {
            if let Some(ip) = ip.to_ipv4_mapped() {
                is_public(IpAddr::V4(ip))
            } else {
                !(ip.is_loopback()
                    || ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_unique_local()
                    || ip.is_unicast_link_local())
            }
        }
    }
}

fn trusted_download(url: &Url) -> Result<Client, String> {
    let host = url.host_str().ok_or("Download host is invalid")?;
    let trusted =
        host == "huggingface.co" || host.ends_with(".hf.co") || host.ends_with(".huggingface.co");
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || !trusted
        || url.port().is_some_and(|port| port != 443)
    {
        return Err("Download host is not allowed".into());
    }
    let addresses: Vec<_> = (host, 443)
        .to_socket_addrs()
        .map_err(|_| "Download host is unavailable")?
        .collect();
    if addresses.is_empty() || addresses.iter().any(|address| !is_public(address.ip())) {
        return Err("Download address is not allowed".into());
    }
    Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .https_only(true)
        .no_proxy()
        .resolve_to_addrs(host, &addresses)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(600))
        .build()
        .map_err(|_| "Downloader is unavailable".into())
}

fn download_request(mut url: Url, offset: u64) -> Result<Response, String> {
    for _ in 0..6 {
        let client = trusted_download(&url)?;
        let mut request = client.get(url.clone());
        if offset > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={offset}-"));
        }
        let response = request
            .send()
            .map_err(|_| "Download failed; no connection")?;
        if response.status().is_redirection() {
            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|header| header.to_str().ok())
                .ok_or("Download redirect is invalid")?;
            url = url
                .join(location)
                .map_err(|_| "Download redirect is invalid")?;
            continue;
        }
        if response.status().is_success() {
            return Ok(response);
        }
        return Err("Artifact is unavailable or requires access".into());
    }
    Err("Too many download redirects".into())
}

impl ModelRepository for HuggingFaceModelStore {
    fn inspect(&self, repo: &str) -> Result<ModelArtifact, String> {
        let url = Url::parse(repo).map_err(|_| "Repository URL is invalid")?;
        if url.scheme() != "https"
            || url.host_str() != Some("huggingface.co")
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err("Use an HTTPS Hugging Face repository URL".into());
        }
        let requested = url.path().trim_matches('/');
        let repo = if requested == "Qwen/Qwen3-Embedding-0.6B" {
            DEFAULT_ARTIFACT_REPO
        } else {
            requested
        };
        let parts: Vec<_> = repo.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || !part
                        .chars()
                        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.'))
            })
        {
            return Err("Repository must be in publisher/model form".into());
        }
        let endpoint = Url::parse(&format!(
            "https://huggingface.co/api/models/{repo}?blobs=true"
        ))
        .map_err(|_| "Repository is invalid")?;
        let data = json_response(download_request(endpoint, 0)?)?;
        let revision = data["sha"]
            .as_str()
            .filter(|value| value.len() == 40 && value.chars().all(|ch| ch.is_ascii_hexdigit()))
            .ok_or("Repository revision is unavailable")?;
        let siblings = data["siblings"]
            .as_array()
            .ok_or("Repository manifest is unavailable")?;
        let file = siblings
            .iter()
            .find(|item| {
                item["rfilename"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(DEFAULT_ARTIFACT_FILE))
            })
            .or_else(|| {
                siblings.iter().find(|item| {
                    item["rfilename"]
                        .as_str()
                        .is_some_and(|name| name.to_lowercase().ends_with("-f16.gguf"))
                })
            })
            .ok_or("Repository needs a compatible Qwen3 GGUF F16 artifact")?;
        let filename = file["rfilename"]
            .as_str()
            .ok_or("Artifact name is invalid")?;
        if Path::new(filename).components().count() != 1 || filename.contains(['\\', '/']) {
            return Err("Artifact path is invalid".into());
        }
        let sha256 = file["lfs"]["sha256"]
            .as_str()
            .ok_or("Publisher digest is unavailable")?;
        let bytes = file["lfs"]["size"]
            .as_u64()
            .or_else(|| file["size"].as_u64())
            .ok_or("Artifact size is unavailable")?;
        if bytes == 0
            || bytes > MAX_MODEL_BYTES
            || sha256.len() != 64
            || !sha256.chars().all(|ch| ch.is_ascii_hexdigit())
        {
            return Err("Artifact is not supported".into());
        }
        Ok(ModelArtifact {
            repo: repo.into(),
            revision: revision.into(),
            filename: filename.into(),
            bytes,
            sha256: sha256.into(),
            license: data["cardData"]["license"]
                .as_str()
                .unwrap_or("Not specified by publisher")
                .into(),
        })
    }

    fn download(
        &self,
        artifact: &ModelArtifact,
        cancel: &AtomicBool,
        sender: &EventSink,
    ) -> Result<VerifiedModel, String> {
        if artifact.bytes > MAX_MODEL_BYTES
            || artifact.sha256.len() != 64
            || !artifact.sha256.chars().all(|ch| ch.is_ascii_hexdigit())
            || artifact.revision.len() != 40
            || !artifact.revision.chars().all(|ch| ch.is_ascii_hexdigit())
            || artifact.repo.split('/').count() != 2
            || !artifact
                .repo
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-_/.".contains(ch))
            || Path::new(&artifact.filename).components().count() != 1
            || artifact.filename.contains(['/', '\\'])
            || artifact.filename.starts_with('.')
        {
            return Err("Artifact manifest is invalid".into());
        }
        let target = self.paths.models.join(format!("{}.gguf", artifact.sha256));
        if target
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
        {
            return Err("Artifact path is invalid".into());
        }
        if target.is_file() && digest_file(&target)? == artifact.sha256 {
            verify_gguf(&target)?;
            return Ok(VerifiedModel {
                artifact: artifact.clone(),
                path: target,
            });
        }
        let available = fs2::available_space(&self.paths.models)
            .map_err(|_| "Free disk space could not be checked")?;
        let partial = self.paths.models.join(format!("{}.part", artifact.sha256));
        if partial
            .symlink_metadata()
            .is_ok_and(|metadata| !metadata.is_file() || metadata.file_type().is_symlink())
        {
            return Err("Partial artifact path is invalid".into());
        }
        let mut offset = partial.metadata().map_or(0, |metadata| metadata.len());
        if offset > artifact.bytes {
            return Err(
                "Partial artifact is invalid; delete the .part file from the models folder".into(),
            );
        }
        if available < artifact.bytes.saturating_sub(offset) {
            return Err("Not enough disk space".into());
        }
        if offset == artifact.bytes {
            // Already fully downloaded; verify instead of requesting an empty range.
            return self.publish_partial(artifact, &partial, &target);
        }
        let endpoint = Url::parse(&format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            artifact.repo, artifact.revision, artifact.filename
        ))
        .map_err(|_| "Artifact manifest is invalid")?;
        let mut response = download_request(endpoint, offset)?;
        if offset > 0 && response.status() != reqwest::StatusCode::PARTIAL_CONTENT {
            offset = 0;
        }
        if offset > 0 {
            let range = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|header| header.to_str().ok())
                .ok_or("Resume is not supported")?;
            if !range.starts_with(&format!("bytes {offset}-"))
                || !range.ends_with(&format!("/{}", artifact.bytes))
            {
                return Err("Partial download does not match the revision".into());
            }
        }
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(offset == 0);
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
        }
        let mut output = options
            .open(&partial)
            .map_err(|_| "Artifact could not be written")?;
        output
            .seek(SeekFrom::Start(offset))
            .map_err(|_| "Resume failed")?;
        let mut buffer = vec![0u8; 128 * 1024];
        let mut done = offset;
        let mut notified = Instant::now();
        loop {
            if cancel.load(Ordering::Acquire) {
                return Err("Download cancelled; partial file kept".into());
            }
            let count = response
                .read(&mut buffer)
                .map_err(|_| "Download interrupted; partial file kept")?;
            if count == 0 {
                break;
            }
            done = done
                .checked_add(count as u64)
                .filter(|done| *done <= artifact.bytes)
                .ok_or("Artifact exceeds the manifest size")?;
            output
                .write_all(&buffer[..count])
                .map_err(|_| "Writing the artifact failed")?;
            if notified.elapsed() >= Duration::from_millis(100) {
                let _ = sender.try_send(BackgroundMessage::DownloadProgress {
                    done,
                    total: artifact.bytes,
                });
                notified = Instant::now();
            }
        }
        output.sync_all().map_err(|_| "Artifact was not saved")?;
        drop(output);
        if done != artifact.bytes {
            let _ = fs::remove_file(&partial);
            return Err("Artifact size or checksum mismatch; the partial file was removed".into());
        }
        self.publish_partial(artifact, &partial, &target)
    }
    fn active(&self) -> Result<Option<VerifiedModel>, String> {
        let manifest = self.paths.models.join("active.json");
        if !manifest.exists() {
            return Ok(None);
        }
        let bytes = fs::read(manifest).map_err(|_| "Model manifest could not be read")?;
        if bytes.len() > MAX_DESCRIPTION_BYTES {
            return Err("Model manifest is invalid".into());
        }
        let mut model: VerifiedModel =
            serde_json::from_slice(&bytes).map_err(|_| "Model manifest is invalid")?;
        if model.artifact.sha256.len() != 64
            || !model
                .artifact
                .sha256
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        {
            return Err("Model manifest checksum is invalid".into());
        }
        model.path = self
            .paths
            .models
            .join(format!("{}.gguf", model.artifact.sha256));
        if !model.path.is_file() {
            return Ok(None);
        }
        Ok(Some(model))
    }
    fn activate(&self, model: &VerifiedModel) -> Result<(), String> {
        if model.artifact.sha256.len() != 64
            || !model
                .artifact
                .sha256
                .chars()
                .all(|ch| ch.is_ascii_hexdigit())
        {
            return Err("Model manifest is invalid".into());
        }
        let mut model = model.clone();
        model.path = self
            .paths
            .models
            .join(format!("{}.gguf", model.artifact.sha256));
        if model.path.parent() != Some(self.paths.models.as_path())
            || digest_file(&model.path)? != model.artifact.sha256
        {
            return Err("Model artifact is not verified".into());
        }
        verify_gguf(&model.path)?;
        super::config::atomic_write(
            &self.paths.models.join("active.json"),
            &serde_json::to_vec_pretty(&model).map_err(|_| "Model manifest failed")?,
        )
        .map_err(|_| "Model could not be activated".into())
    }
}

fn read_integer(file: &mut File, count: usize) -> Result<u64, String> {
    let mut bytes = [0u8; 8];
    file.read_exact(&mut bytes[..count])
        .map_err(|_| "GGUF header is invalid")?;
    Ok(u64::from_le_bytes(bytes))
}
fn read_string(file: &mut File) -> Result<String, String> {
    let length = read_integer(file, 8)?;
    if length > MAX_DESCRIPTION_BYTES as u64 {
        return Err("GGUF metadata is too large".into());
    }
    let mut bytes = vec![0u8; length as usize];
    file.read_exact(&mut bytes)
        .map_err(|_| "GGUF metadata is incomplete")?;
    String::from_utf8(bytes).map_err(|_| "GGUF metadata is invalid".into())
}
fn skip_value(file: &mut File, kind: u64, depth: usize) -> Result<(), String> {
    if depth > 4 {
        return Err("GGUF metadata is nested too deeply".into());
    }
    let width = match kind {
        0 | 1 | 7 => 1,
        2 | 3 => 2,
        4..=6 => 4,
        10..=12 => 8,
        8 => {
            read_string(file)?;
            return Ok(());
        }
        9 => {
            let element = read_integer(file, 4)?;
            let count = read_integer(file, 8)?;
            if count > 1000000 {
                return Err("GGUF metadata is too large".into());
            }
            for _ in 0..count {
                skip_value(file, element, depth + 1)?;
            }
            return Ok(());
        }
        _ => return Err("GGUF metadata type is not supported".into()),
    };
    let mut bytes = [0u8; 8];
    file.read_exact(&mut bytes[..width])
        .map_err(|_| "GGUF metadata is incomplete".into())
}
fn verify_gguf(path: &Path) -> Result<(), String> {
    let mut file = File::open(path).map_err(|_| "Model could not be read")?;
    let mut magic = [0; 4];
    file.read_exact(&mut magic)
        .map_err(|_| "Model header is invalid")?;
    if &magic != b"GGUF" || read_integer(&mut file, 4)? != 3 {
        return Err("Model format must be GGUF v3".into());
    }
    let tensors = read_integer(&mut file, 8)?;
    let count = read_integer(&mut file, 8)?;
    if tensors == 0 || tensors > 1000000 || count > 4096 {
        return Err("GGUF manifest is invalid".into());
    }
    let mut architecture = String::new();
    let mut dimensions = 0;
    for _ in 0..count {
        if file
            .stream_position()
            .map_err(|_| "Reading GGUF metadata failed")?
            > 32 * 1024 * 1024
        {
            return Err("GGUF metadata is too large".into());
        }
        let key = read_string(&mut file)?;
        let kind = read_integer(&mut file, 4)?;
        if key == "general.architecture" && kind == 8 {
            architecture = read_string(&mut file)?;
        } else if key == "qwen3.embedding_length" && kind == 4 {
            dimensions = read_integer(&mut file, 4)?;
        } else {
            skip_value(&mut file, kind, 0)?;
        }
    }
    if architecture != "qwen3" || dimensions != DEFAULT_DIMENSIONS as u64 {
        return Err("This runtime requires a 1024-dimension Qwen3 embedding model".into());
    }
    Ok(())
}
