use crate::shared::models::AgentConfig;
use crate::shared::traits::PermissionPolicy;
use parking_lot::RwLock;
use std::path::Path;

pub struct AgentAccessPolicy {
    config: RwLock<AgentConfig>,
}
impl AgentAccessPolicy {
    pub fn new(config: AgentConfig) -> Self {
        Self {
            config: RwLock::new(config),
        }
    }
}
impl PermissionPolicy for AgentAccessPolicy {
    fn get(&self) -> AgentConfig {
        self.config.read().clone()
    }
    fn set(&self, config: AgentConfig) {
        *self.config.write() = config;
    }
    fn authorize(&self, path: &Path, capability: &str) -> Result<(), String> {
        let config = self.config.read();
        if !config.enabled {
            return Err("Agent is disabled".into());
        }
        let allowed = match capability {
            "metadata_read" => config.metadata_read,
            "content_read" => config.content_read,
            "semantic_search" => config.semantic_search,
            "metadata_write" => config.metadata_write,
            "filesystem_write" => config.filesystem_write,
            _ => false,
        };
        if !allowed {
            return Err("Tool capability is not allowed".into());
        }
        let canonical =
            crate::shared::utils::canonicalize(path).map_err(|_| "Target is unavailable")?;
        if !config
            .allowed_roots
            .iter()
            .filter_map(|root| {
                crate::shared::utils::parse_config_path(root)
                    .ok()
                    .and_then(|root| crate::shared::utils::canonicalize(&root).ok())
            })
            .any(|root| canonical.starts_with(root))
        {
            return Err("Target is outside the allowed roots".into());
        }
        Ok(())
    }
}
