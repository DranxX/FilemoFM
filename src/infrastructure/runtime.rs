use super::model_store::{digest_file, json_response};
use crate::shared::constants::*;
use crate::shared::models::*;
use crate::shared::traits::{EncoderFactory, TextEncoder, VectorBackendFactory, VectorDataset};
use reqwest::blocking::Client;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::fs;
use std::net::TcpListener;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

pub fn register_runtime(
    paths: &RuntimePaths,
    source: &Path,
    kind: &str,
    version: &str,
) -> Result<(), String> {
    if !matches!(kind, "qdrant" | "llama-server")
        || version.trim().is_empty()
        || version.len() > 128
        || !source.is_file()
        || source
            .symlink_metadata()
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        return Err("Runtime or version is invalid".into());
    }
    let filename = if cfg!(windows) {
        format!("{kind}.exe")
    } else {
        kind.into()
    };
    let directory = paths.runtime.join(kind);
    fs::create_dir_all(&directory).map_err(|_| "Runtime folder could not be created")?;
    if directory
        .symlink_metadata()
        .map_err(|_| "Runtime folder is unavailable")?
        .file_type()
        .is_symlink()
    {
        return Err("Runtime folder is invalid".into());
    }
    let target = directory.join(&filename);
    if target
        .symlink_metadata()
        .is_ok_and(|metadata| metadata.file_type().is_symlink() || !metadata.is_file())
    {
        return Err("Runtime target is invalid".into());
    }
    if source != target {
        fs::copy(source, &target).map_err(|_| "Runtime could not be copied")?;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o700))
            .map_err(|_| "Runtime could not be made executable")?;
    }
    let mut companions = Vec::new();
    let source_directory =
        fs::canonicalize(source.parent().ok_or("Runtime folder is unavailable")?)
            .map_err(|_| "Runtime folder is unavailable")?;
    for entry in fs::read_dir(&source_directory).map_err(|_| "Runtime folder could not be read")? {
        let entry = entry.map_err(|_| "Runtime folder contents could not be read")?;
        let name = entry
            .file_name()
            .to_str()
            .ok_or("Runtime library name is invalid")?
            .to_string();
        if !(name.ends_with(".dll") || name.contains(".so")) {
            continue;
        }
        let library =
            fs::canonicalize(entry.path()).map_err(|_| "Runtime library is unavailable")?;
        if companions.len() >= MAX_RUNTIME_LIBRARIES
            || !library.starts_with(&source_directory)
            || !library.is_file()
        {
            return Err("Runtime library package is invalid".into());
        }
        let output = directory.join(&name);
        if entry.path() != output {
            if output
                .symlink_metadata()
                .is_ok_and(|metadata| metadata.file_type().is_symlink())
            {
                return Err("Runtime library target is invalid".into());
            }
            fs::copy(library, &output).map_err(|_| "Runtime library could not be copied")?;
        }
        companions.push(RuntimeCompanion {
            filename: name,
            sha256: digest_file(&output)?,
        });
    }
    companions.sort_by(|left, right| left.filename.cmp(&right.filename));
    let manifest = RuntimeArtifact {
        filename,
        companions,
        sha256: digest_file(&target)?,
        version: version.trim().into(),
    };
    super::config::atomic_write(
        &paths.runtime.join(format!("{kind}.json")),
        &serde_json::to_vec_pretty(&manifest).map_err(|_| "Runtime manifest failed")?,
    )
    .map_err(|_| "Runtime manifest could not be written".into())
}

fn verified_command(paths: &RuntimePaths, kind: &str) -> Result<Command, String> {
    let bytes = fs::read(paths.runtime.join(format!("{kind}.json")))
        .map_err(|_| format!("Register the {kind} runtime in Settings › Index & vector"))?;
    if bytes.len() > MAX_DESCRIPTION_BYTES {
        return Err("Runtime manifest is invalid".into());
    }
    let manifest: RuntimeArtifact =
        serde_json::from_slice(&bytes).map_err(|_| "Runtime manifest is invalid")?;
    let expected = if cfg!(windows) {
        format!("{kind}.exe")
    } else {
        kind.into()
    };
    if manifest.filename != expected
        || manifest.sha256.len() != 64
        || !manifest.sha256.chars().all(|ch| ch.is_ascii_hexdigit())
    {
        return Err("Runtime manifest is invalid".into());
    }
    let directory = paths.runtime.join(kind);
    if manifest.version.trim().is_empty()
        || manifest.version.len() > 128
        || manifest.companions.len() > MAX_RUNTIME_LIBRARIES
    {
        return Err("Runtime version is not verified".into());
    }
    for companion in &manifest.companions {
        if companion.filename.contains(['/', '\\'])
            || companion.filename.starts_with('.')
            || companion.sha256.len() != 64
        {
            return Err("Runtime library manifest is invalid".into());
        }
        let path = directory.join(&companion.filename);
        if path
            .symlink_metadata()
            .map_err(|_| "Runtime library is unavailable")?
            .file_type()
            .is_symlink()
            || digest_file(&path)? != companion.sha256
        {
            return Err("Runtime library checksum changed".into());
        }
    }
    for entry in fs::read_dir(&directory).map_err(|_| "Runtime folder is unavailable")? {
        let entry = entry.map_err(|_| "Runtime folder could not be read")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if (name.ends_with(".dll") || name.contains(".so"))
            && !manifest
                .companions
                .iter()
                .any(|companion| companion.filename == name)
        {
            return Err(
                "Unregistered runtime library found; register the trusted package again".into(),
            );
        }
    }
    let executable = directory.join(&manifest.filename);
    if executable
        .symlink_metadata()
        .map_err(|_| "Runtime is unavailable")?
        .file_type()
        .is_symlink()
        || digest_file(&executable)? != manifest.sha256
    {
        return Err("Runtime checksum changed; register the trusted runtime again".into());
    }
    let mut command = Command::new(executable);
    command
        .current_dir(&directory)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env_clear();
    #[cfg(target_os = "linux")]
    command.env("LD_LIBRARY_PATH", &directory);
    for key in ["SYSTEMROOT", "WINDIR", "TEMP", "TMP", "LANG"] {
        if let Some(value) = std::env::var_os(key) {
            command.env(key, value);
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(CREATE_NO_WINDOW);
    }
    Ok(command)
}

fn available_port() -> Result<u16, String> {
    TcpListener::bind(("127.0.0.1", 0))
        .and_then(|listener| listener.local_addr())
        .map(|address| address.port())
        .map_err(|_| "No local port available".into())
}
fn local_client() -> Result<Client, String> {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(120))
        .build()
        .map_err(|_| "Local client is unavailable".into())
}

struct OwnedProcess(Child);
impl OwnedProcess {
    /// Spawns a helper that cannot outlive the app, even when the app crashes or is killed:
    /// a kill-on-close Job Object on Windows, the parent-death signal on Linux. Without this a
    /// leftover Qdrant keeps its storage locked and the vector backend stays offline.
    fn spawn(mut command: Command) -> std::io::Result<Self> {
        #[cfg(target_os = "linux")]
        unsafe {
            use std::os::unix::process::CommandExt;
            command.pre_exec(|| {
                rustix::process::set_parent_process_death_signal(Some(
                    rustix::process::Signal::KILL,
                ))
                .map_err(std::io::Error::from)
            });
        }
        let child = command.spawn()?;
        #[cfg(windows)]
        if let Some(job) = kill_on_close_job() {
            use std::os::windows::io::AsRawHandle;
            unsafe {
                windows_sys::Win32::System::JobObjects::AssignProcessToJobObject(
                    job as _,
                    child.as_raw_handle() as _,
                );
            }
        }
        Ok(Self(child))
    }
}

/// One job per app process; the handle is never closed, so Windows closes it at exit and
/// terminates every helper assigned to it.
#[cfg(windows)]
fn kill_on_close_job() -> Option<usize> {
    use windows_sys::Win32::System::JobObjects::{
        CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };
    static JOB: std::sync::OnceLock<Option<usize>> = std::sync::OnceLock::new();
    *JOB.get_or_init(|| unsafe {
        let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
        if job.is_null() {
            return None;
        }
        let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
        limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
        let ok = SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const _,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        );
        (ok != 0).then_some(job as usize)
    })
}
impl Drop for OwnedProcess {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

pub struct QdrantBackendFactory {
    paths: RuntimePaths,
}
impl QdrantBackendFactory {
    pub fn new(paths: RuntimePaths) -> Self {
        Self { paths }
    }
}
struct QdrantDataset {
    process: OwnedProcess,
    client: Client,
    endpoint: String,
    token: String,
}
impl VectorBackendFactory for QdrantBackendFactory {
    fn start(&self) -> Result<Box<dyn VectorDataset>, String> {
        let port = available_port()?;
        let mut grpc = available_port()?;
        while grpc == port {
            grpc = available_port()?;
        }
        let token = uuid::Uuid::new_v4().to_string();
        let mut command = verified_command(&self.paths, "qdrant")?;
        command
            .env("QDRANT__SERVICE__HOST", "127.0.0.1")
            .env("QDRANT__SERVICE__HTTP_PORT", port.to_string())
            .env("QDRANT__SERVICE__GRPC_PORT", grpc.to_string())
            .env("QDRANT__SERVICE__API_KEY", &token)
            .env("QDRANT__TELEMETRY_DISABLED", "true")
            .env("QDRANT__CLUSTER__ENABLED", "false")
            .env("QDRANT__SERVICE__ENABLE_CORS", "false")
            .env(
                "QDRANT__STORAGE__STORAGE_PATH",
                self.paths.data.join("vectors"),
            )
            .env("QDRANT__LOG_LEVEL", "ERROR");
        let process = OwnedProcess::spawn(command).map_err(|_| "Qdrant could not be started")?;
        let mut backend = QdrantDataset {
            process,
            client: local_client()?,
            endpoint: format!("http://127.0.0.1:{port}"),
            token,
        };
        let started = Instant::now();
        loop {
            if backend
                .process
                .0
                .try_wait()
                .map_err(|_| "Qdrant status check failed")?
                .is_some()
            {
                return Err("Qdrant stopped during startup".into());
            }
            if backend
                .client
                .get(format!("{}/collections", backend.endpoint))
                .header("api-key", &backend.token)
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            if started.elapsed() > Duration::from_secs(30) {
                return Err("Qdrant was not ready in time".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(Box::new(backend))
    }
}
impl QdrantDataset {
    fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
    ) -> Result<Value, String> {
        let mut request = self
            .client
            .request(method, format!("{}{path}", self.endpoint))
            .header("api-key", &self.token);
        if let Some(body) = body {
            request = request.json(&body);
        }
        json_response(request.send().map_err(|_| "Qdrant is unavailable")?)
    }
    fn validate_dataset(name: &str) -> Result<(), String> {
        if !name.starts_with("filemofm_")
            || name.len() > 128
            || !name
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
        {
            return Err("Dataset does not belong to FilemoFM".into());
        }
        Ok(())
    }
}
impl VectorDataset for QdrantDataset {
    fn ensure(&mut self, name: &str, dimensions: usize) -> Result<bool, String> {
        Self::validate_dataset(name)?;
        if !(MIN_QWEN_DIMENSIONS..=DEFAULT_DIMENSIONS).contains(&dimensions) {
            return Err("Vector dimension is invalid".into());
        }
        let response = self
            .client
            .get(format!("{}/collections/{name}", self.endpoint))
            .header("api-key", &self.token)
            .send()
            .map_err(|_| "Qdrant is unavailable")?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            self.request(
                reqwest::Method::PUT,
                &format!("/collections/{name}"),
                Some(json!({"vectors":{"size":dimensions,"distance":"Cosine"}})),
            )?;
            return Ok(true);
        } else {
            let data = json_response(response)?;
            if data["result"]["config"]["params"]["vectors"]["size"].as_u64()
                != Some(dimensions as u64)
                || data["result"]["config"]["params"]["vectors"]["distance"].as_str()
                    != Some("Cosine")
            {
                return Err("Dataset dimension does not match".into());
            }
        }
        Ok(false)
    }
    fn upsert(
        &mut self,
        name: &str,
        id: &str,
        chunk: usize,
        vector: Vec<f32>,
        revision: i64,
    ) -> Result<(), String> {
        Self::validate_dataset(name)?;
        uuid::Uuid::parse_str(id).map_err(|_| "Vector ID is invalid")?;
        if !(MIN_QWEN_DIMENSIONS..=DEFAULT_DIMENSIONS).contains(&vector.len())
            || vector.iter().any(|value| !value.is_finite())
            || revision < 1
        {
            return Err("Vector payload is invalid".into());
        }
        let namespace = uuid::Uuid::parse_str(id).map_err(|_| "File ID is invalid")?;
        let point = uuid::Uuid::new_v5(&namespace, &chunk.to_le_bytes()).to_string();
        self.request(reqwest::Method::PUT,&format!("/collections/{name}/points?wait=true"),Some(json!({"points":[{"id":point,"vector":vector,"payload":{"file_id":id,"revision":revision}}]})))?;
        Ok(())
    }
    fn delete(&mut self, name: &str, id: &str) -> Result<(), String> {
        Self::validate_dataset(name)?;
        self.request(
            reqwest::Method::POST,
            &format!("/collections/{name}/points/delete?wait=true"),
            Some(json!({"filter":{"must":[{"key":"file_id","match":{"value":id}}]}})),
        )?;
        Ok(())
    }
    fn search(
        &mut self,
        name: &str,
        vector: Vec<f32>,
        ids: &[String],
        limit: usize,
    ) -> Result<Vec<(String, f32, i64)>, String> {
        Self::validate_dataset(name)?;
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let data=self.request(reqwest::Method::POST,&format!("/collections/{name}/points/query"),Some(json!({"query":vector,"filter":{"must":[{"key":"file_id","match":{"any":ids}}]},"limit":limit.min(MAX_RESULTS),"with_payload":true})))?;
        data["result"]["points"]
            .as_array()
            .ok_or_else(|| "Vector search response is invalid".to_string())?
            .iter()
            .map(|hit| {
                Ok((
                    hit["payload"]["file_id"]
                        .as_str()
                        .ok_or("Vector ID is invalid")?
                        .into(),
                    hit["score"]
                        .as_f64()
                        .filter(|value| value.is_finite())
                        .ok_or("Vector score is invalid")? as f32,
                    hit["payload"]["revision"]
                        .as_i64()
                        .ok_or("Vector revision is invalid")?,
                ))
            })
            .collect()
    }
    fn update_revision(&mut self, name: &str, id: &str, revision: i64) -> Result<(), String> {
        Self::validate_dataset(name)?;
        self.request(reqwest::Method::POST,&format!("/collections/{name}/points/payload?wait=true"),Some(json!({"payload":{"revision":revision},"filter":{"must":[{"key":"file_id","match":{"value":id}}]}})))?;
        Ok(())
    }
    fn is_available(&mut self) -> bool {
        self.process
            .0
            .try_wait()
            .is_ok_and(|status| status.is_none())
    }
    fn count(&mut self, name: &str) -> Result<usize, String> {
        Self::validate_dataset(name)?;
        self.request(
            reqwest::Method::POST,
            &format!("/collections/{name}/points/count"),
            Some(json!({"exact":true})),
        )?["result"]["count"]
            .as_u64()
            .map(|count| count as usize)
            .ok_or_else(|| "Vector count is unavailable".into())
    }
}

pub struct LlamaEncoderFactory {
    paths: RuntimePaths,
}
impl LlamaEncoderFactory {
    pub fn new(paths: RuntimePaths) -> Self {
        Self { paths }
    }
}
struct LlamaEncoder {
    _process: OwnedProcess,
    client: Client,
    endpoint: String,
    token: String,
    fingerprint: String,
}
impl EncoderFactory for LlamaEncoderFactory {
    fn load(
        &self,
        model: &VerifiedModel,
        config: &EmbeddingConfig,
    ) -> Result<Box<dyn TextEncoder>, String> {
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
        if digest_file(&model.path)? != model.artifact.sha256 {
            return Err("Model checksum changed".into());
        }
        let port = available_port()?;
        let token = uuid::Uuid::new_v4().to_string();
        let mut command = verified_command(&self.paths, "llama-server")?;
        let runtime_fingerprint = digest_file(&self.paths.runtime.join("llama-server.json"))?;
        command
            .arg("--model")
            .arg(&model.path)
            .args([
                "--embedding",
                "--pooling",
                "last",
                "--host",
                "127.0.0.1",
                "--port",
            ])
            .arg(port.to_string())
            .env("LLAMA_API_KEY", &token)
            .arg("--threads")
            .arg(config.threads.to_string())
            .arg("--ctx-size")
            .arg(config.context_tokens.to_string())
            .args([
                "--parallel",
                "1",
                "--n-gpu-layers",
                "0",
                "--no-webui",
                "--log-disable",
            ]);
        let mut process =
            OwnedProcess::spawn(command).map_err(|_| "Model worker could not be started")?;
        let client = local_client()?;
        let endpoint = format!("http://127.0.0.1:{port}");
        let started = Instant::now();
        loop {
            if process
                .0
                .try_wait()
                .map_err(|_| "Model worker status check failed")?
                .is_some()
            {
                return Err("Model worker stopped while loading".into());
            }
            if client
                .get(format!("{endpoint}/health"))
                .bearer_auth(&token)
                .send()
                .is_ok_and(|response| response.status().is_success())
            {
                break;
            }
            if started.elapsed() > Duration::from_secs(120) {
                return Err("Model was not ready in time".into());
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        let mut encoder = LlamaEncoder {
            _process: process,
            client,
            endpoint,
            token,
            fingerprint: format!(
                "qwen3-last-l2-v2:{}:{}:{}:{}:{:x}",
                model.artifact.sha256,
                runtime_fingerprint,
                config.dimension,
                config.context_tokens,
                Sha256::digest(QUERY_INSTRUCTION.as_bytes())
            ),
        };
        let fixture = encoder.embed("FilemoFM embedding readiness", false)?;
        if fixture.len() != DEFAULT_DIMENSIONS {
            return Err("Model output is not 1024-dimensional".into());
        }
        Ok(Box::new(encoder))
    }
}
impl TextEncoder for LlamaEncoder {
    fn embed(&mut self, text: &str, is_query: bool) -> Result<Vec<f32>, String> {
        if text.len() > MAX_DESCRIPTION_BYTES {
            return Err("Embedding text is too large".into());
        }
        let input = if is_query {
            format!("{QUERY_INSTRUCTION}{text}")
        } else {
            text.into()
        };
        let response = json_response(
            self.client
                .post(format!("{}/v1/embeddings", self.endpoint))
                .bearer_auth(&self.token)
                .json(&json!({"input":[input],"encoding_format":"float"}))
                .send()
                .map_err(|_| "Model inference failed")?,
        )?;
        let vector = response["data"][0]["embedding"]
            .as_array()
            .ok_or("Embedding output is invalid")?
            .iter()
            .map(|value| {
                value
                    .as_f64()
                    .filter(|value| value.is_finite())
                    .map(|value| value as f32)
                    .ok_or("Embedding value is invalid".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        if vector.len() != DEFAULT_DIMENSIONS || vector.iter().any(|value| !value.is_finite()) {
            return Err("Embedding dimension or values are invalid".into());
        }
        Ok(vector)
    }
    fn fingerprint(&self) -> String {
        self.fingerprint.clone()
    }
}
