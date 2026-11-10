use crate::shared::constants::*;
use crate::shared::models::*;
use crate::shared::traits::*;
use serde_json::{Value, json};
use std::sync::Arc;

pub fn tool_schemas(include_mutations: bool) -> Vec<Value> {
    let mut schemas = vec![
        json!({"type":"function","function":{"name":"search_files","description":"Search authorized indexed file metadata using intitle:, type:, tag:, date: filters","parameters":{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":["query"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"get_file_metadata","description":"Read an authorized file by stable ID","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"list_tags","description":"List tags attached to authorized files","parameters":{"type":"object","properties":{},"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"semantic_search","description":"Search an available local semantic dataset within authorized roots","parameters":{"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":100}},"required":["query"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"get_index_status","description":"Read local index and pending synchronization status","parameters":{"type":"object","properties":{},"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"read_file_content","description":"Read bounded UTF-8 text from an authorized file if content permission is enabled","parameters":{"type":"object","properties":{"id":{"type":"string"}},"required":["id"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"update_file_description","description":"Propose replacing a file description; requires enabled metadata write and interactive approval bound to revision","parameters":{"type":"object","properties":{"id":{"type":"string"},"revision":{"type":"integer"},"description":{"type":"string"}},"required":["id","revision","description"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"set_file_tags","description":"Propose replacing file tag IDs with interactive revision-bound approval","parameters":{"type":"object","properties":{"id":{"type":"string"},"revision":{"type":"integer"},"tags":{"type":"array","items":{"type":"integer"},"maxItems":MAX_FILE_TAGS}},"required":["id","revision","tags"],"additionalProperties":false}}}),
        json!({"type":"function","function":{"name":"operate_files","description":"Propose a queued filesystem operation; requires interactive approval for exact targets and revisions","parameters":{"type":"object","properties":{"kind":{"type":"string","enum":["copy","move","rename","trash","delete"]},"ids":{"type":"array","items":{"type":"string"},"minItems":1,"maxItems":100},"destination":{"type":"string"},"name":{"type":"string"}},"required":["kind","ids"],"additionalProperties":false}}}),
    ];
    if !include_mutations {
        schemas.truncate(6);
    }
    schemas
}

pub struct MetadataTools {
    repository: Arc<dyn MetadataRepository>,
    vectors: Arc<dyn VectorSearch>,
    policy: Arc<dyn PermissionPolicy>,
    approver: Option<Arc<dyn MutationApprover>>,
    operations: Option<Arc<dyn FileOperationQueue>>,
}
impl MetadataTools {
    pub fn new(
        repository: Arc<dyn MetadataRepository>,
        vectors: Arc<dyn VectorSearch>,
        policy: Arc<dyn PermissionPolicy>,
        approver: Option<Arc<dyn MutationApprover>>,
        operations: Option<Arc<dyn FileOperationQueue>>,
    ) -> Self {
        Self {
            repository,
            vectors,
            policy,
            approver,
            operations,
        }
    }
}
impl ToolDispatcher for MetadataTools {
    fn dispatch(&self, name: &str, arguments: Value) -> Result<Value, String> {
        if arguments.to_string().len() > MAX_QUERY_BYTES {
            return Err("Tool arguments are too large".into());
        }
        let object = arguments
            .as_object()
            .ok_or("Tool arguments must be an object")?;
        let config = self.policy.get();
        if !config.enabled || !config.metadata_read || config.allowed_roots.is_empty() {
            return Err("Metadata access is not allowed".into());
        }
        let roots = config
            .allowed_roots
            .iter()
            .map(|value| crate::shared::utils::parse_config_path(value))
            .collect::<Result<Vec<_>, _>>()?;
        match name {
            "search_files" | "semantic_search" => {
                if object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "query" | "limit"))
                {
                    return Err("Unknown tool argument".into());
                }
                let text = arguments["query"]
                    .as_str()
                    .ok_or("Query must be a string")?;
                if text.len() > MAX_QUERY_BYTES || text.chars().any(char::is_control) {
                    return Err("Tool query is invalid".into());
                }
                let limit = match arguments.get("limit") {
                    Some(value) => value
                        .as_u64()
                        .filter(|value| (1..=100).contains(value))
                        .ok_or("Limit must be between 1 and 100")?
                        as usize,
                    None => 20,
                };
                for root in &roots {
                    self.policy.authorize(
                        root,
                        if name == "semantic_search" {
                            "semantic_search"
                        } else {
                            "metadata_read"
                        },
                    )?;
                }
                let query = crate::shared::query::parse_query(
                    text,
                    SearchScope::SelectedRoots,
                    roots[0].clone(),
                    roots,
                    limit,
                )?;
                let results = if name == "semantic_search" {
                    self.vectors.search(query)?
                } else {
                    self.repository
                        .search_keyword(&query)
                        .map_err(|error| error.to_string())?
                };
                let mut output = Vec::new();
                for hit in results {
                    // A deleted or out-of-scope hit is skipped instead of failing the whole call.
                    if self
                        .policy
                        .authorize(&hit.file.path, "metadata_read")
                        .is_err()
                    {
                        continue;
                    }
                    output.push(json!({"id":hit.file.id,"name":hit.file.name,"path":crate::shared::utils::config_path(&hit.file.path),"revision":hit.file.revision,"tags":hit.file.tags,"description":hit.file.description.chars().take(2048).collect::<String>()}));
                }
                Ok(json!({"files":output}))
            }
            "update_file_description" | "set_file_tags" => {
                if object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "id" | "revision" | "description" | "tags"))
                {
                    return Err("Unknown tool argument".into());
                }
                let id = arguments["id"].as_str().ok_or("ID must be a string")?;
                uuid::Uuid::parse_str(id).map_err(|_| "ID is invalid")?;
                let revision = arguments["revision"]
                    .as_i64()
                    .filter(|revision| *revision > 0)
                    .ok_or("Revision is invalid")?;
                let file = self
                    .repository
                    .get_file(id)
                    .map_err(|error| error.to_string())?
                    .ok_or("File is unavailable")?;
                self.policy.authorize(&file.path, "metadata_write")?;
                if file.revision != revision || file.is_missing {
                    return Err("Revision changed; read the metadata again".into());
                }
                let intent = if name == "update_file_description" {
                    let source = arguments["description"]
                        .as_str()
                        .ok_or("Description must be a string")?
                        .to_string();
                    if source.len() > MAX_DESCRIPTION_BYTES || source.contains('\0') {
                        return Err("Description is invalid".into());
                    }
                    for (_, target, _) in crate::shared::utils::reference_nodes(&source) {
                        let target = self
                            .repository
                            .get_file(&target)
                            .map_err(|error| error.to_string())?
                            .ok_or("Reference target is unavailable")?;
                        self.policy.authorize(&target.path, "metadata_read")?;
                    }
                    MutationIntent::Description {
                        id: id.into(),
                        revision,
                        source,
                    }
                } else {
                    let tags = arguments["tags"]
                        .as_array()
                        .filter(|tags| tags.len() <= MAX_FILE_TAGS)
                        .ok_or("Tags must be an array of at most 64 IDs")?
                        .iter()
                        .map(|tag| {
                            tag.as_i64()
                                .filter(|tag| *tag > 0)
                                .ok_or("Tag ID is invalid")
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    MutationIntent::Tags {
                        id: id.into(),
                        revision,
                        tags,
                    }
                };
                self.approver
                    .as_ref()
                    .ok_or("Write tools are only available with approval in the GUI")?
                    .approve(intent.clone())?;
                self.policy.authorize(&file.path, "metadata_write")?;
                match intent {
                    MutationIntent::Description { source, .. } => {
                        self.repository.update_description(id, revision, &source)
                    }
                    MutationIntent::Tags { tags, .. } => {
                        self.repository.set_tags(id, revision, &tags)
                    }
                    _ => return Err("Mutation is invalid".into()),
                }
                .map_err(|error| error.to_string())?;
                Ok(json!({"updated":true,"id":id}))
            }
            "operate_files" => {
                if object
                    .keys()
                    .any(|key| !matches!(key.as_str(), "kind" | "ids" | "destination" | "name"))
                {
                    return Err("Unknown tool argument".into());
                }
                let kind = match arguments["kind"].as_str() {
                    Some("copy") => OperationKind::Copy,
                    Some("move") => OperationKind::Move,
                    Some("rename") => OperationKind::Rename,
                    Some("trash") => OperationKind::Trash,
                    Some("delete") => OperationKind::Delete,
                    _ => return Err("Operation kind is invalid".into()),
                };
                let ids = arguments["ids"]
                    .as_array()
                    .filter(|ids| !ids.is_empty() && ids.len() <= 100)
                    .ok_or("Choose between 1 and 100 IDs")?;
                let mut sources = Vec::new();
                let mut identities = Vec::new();
                let mut revisions = Vec::new();
                for value in ids {
                    let id = value.as_str().ok_or("ID must be a string")?;
                    uuid::Uuid::parse_str(id).map_err(|_| "ID is invalid")?;
                    let file = self
                        .repository
                        .get_file(id)
                        .map_err(|error| error.to_string())?
                        .ok_or("File is unavailable")?;
                    self.policy.authorize(&file.path, "filesystem_write")?;
                    if file.is_missing {
                        return Err("File is unavailable".into());
                    }
                    if let Some(native) = file.native_id {
                        identities.push((file.path.clone(), native));
                    }
                    revisions.push((file.id, file.revision));
                    sources.push(file.path);
                }
                let destination = if matches!(kind, OperationKind::Copy | OperationKind::Move) {
                    std::path::PathBuf::from(
                        arguments["destination"]
                            .as_str()
                            .ok_or("Destination must be a string")?,
                    )
                } else {
                    sources[0]
                        .parent()
                        .ok_or("Parent folder is unavailable")?
                        .to_path_buf()
                };
                self.policy.authorize(&destination, "filesystem_write")?;
                let name = arguments["name"].as_str().unwrap_or_default().to_string();
                if kind == OperationKind::Rename {
                    if sources.len() != 1 {
                        return Err("Rename needs exactly one file".into());
                    }
                    crate::shared::utils::validate_component(&name)?;
                }
                let request = OperationRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    kind,
                    sources,
                    identities,
                    revisions,
                    destination,
                    name,
                    collision: CollisionPolicy::Skip,
                    is_permanent_confirmed: kind == OperationKind::Delete,
                    allow_trash_fallback: false,
                };
                self.approver
                    .as_ref()
                    .ok_or("Write tools are only available with approval in the GUI")?
                    .approve(MutationIntent::Files(request.clone()))?;
                for source in &request.sources {
                    self.policy.authorize(source, "filesystem_write")?;
                }
                self.policy
                    .authorize(&request.destination, "filesystem_write")?;
                let id = request.id.clone();
                self.operations
                    .as_ref()
                    .ok_or("Operation queue is unavailable")?
                    .submit(request)?;
                Ok(json!({"queued":true,"operation_id":id}))
            }
            "get_file_metadata" | "read_file_content" => {
                if object.keys().any(|key| key != "id") {
                    return Err("Unknown tool argument".into());
                }
                let id = arguments["id"].as_str().ok_or("ID must be a string")?;
                uuid::Uuid::parse_str(id).map_err(|_| "File ID is invalid")?;
                let file = self
                    .repository
                    .get_file(id)
                    .map_err(|error| error.to_string())?
                    .ok_or("File is unavailable")?;
                self.policy.authorize(
                    &file.path,
                    if name == "read_file_content" {
                        "content_read"
                    } else {
                        "metadata_read"
                    },
                )?;
                if name == "get_file_metadata" {
                    return serde_json::to_value(file)
                        .map_err(|_| "Metadata could not be read".into());
                }
                if file.is_directory
                    || file.size > MAX_PREVIEW_TEXT_BYTES
                    || !PREVIEWABLE_TEXT_EXTENSIONS.contains(&file.extension.as_str())
                {
                    return Err("Content is not supported or too large".into());
                }
                use std::io::Read;
                let mut bytes = Vec::new();
                crate::shared::utils::open_verified_file(&file.path, file.native_id.as_deref())?
                    .take(MAX_PREVIEW_TEXT_BYTES + 1)
                    .read_to_end(&mut bytes)
                    .map_err(|_| "File could not be read")?;
                if bytes.len() as u64 > MAX_PREVIEW_TEXT_BYTES {
                    return Err("Content is too large".into());
                }
                self.policy.authorize(&file.path, "content_read")?;
                Ok(
                    json!({"id":id,"text":String::from_utf8(bytes).map_err(|_| "File is not UTF-8")?}),
                )
            }
            "list_tags" => {
                if !object.is_empty() {
                    return Err("This tool takes no arguments".into());
                }
                let mut tags = std::collections::BTreeMap::new();
                for file in self
                    .repository
                    .all_files()
                    .map_err(|error| error.to_string())?
                {
                    if self.policy.authorize(&file.path, "metadata_read").is_ok() {
                        for tag in file.tags {
                            tags.insert(tag.id, tag);
                        }
                    }
                    if tags.len() >= MAX_RESULTS {
                        break;
                    }
                }
                Ok(json!({"tags":tags.into_values().collect::<Vec<_>>()}))
            }
            "get_index_status" => {
                if !object.is_empty() {
                    return Err("This tool takes no arguments".into());
                }
                let dataset = self
                    .repository
                    .dataset()
                    .map_err(|error| error.to_string())?;
                let mut pending = 0usize;
                for item in self
                    .repository
                    .pending(MAX_RESULTS)
                    .map_err(|error| error.to_string())?
                {
                    if let Some(file) = self
                        .repository
                        .get_file(&item.id)
                        .map_err(|error| error.to_string())?
                    {
                        pending +=
                            usize::from(self.policy.authorize(&file.path, "metadata_read").is_ok());
                    }
                }
                Ok(
                    json!({"has_dataset":dataset.is_some(),"dimension":dataset.map(|manifest| manifest.dimension),"authorized_pending_bounded":pending}),
                )
            }
            _ => Err("Tool is unavailable".into()),
        }
    }
}
