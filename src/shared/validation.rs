use crate::shared::{constants::*, errors::ConfigError, models::*};

pub fn validate_config(config: &AppConfig) -> Result<(), ConfigError> {
    let general = &config.general;
    if config.version != CONFIG_VERSION
        || !matches!(general.theme.as_str(), "dark" | "light" | "system")
        || !general.font_size.is_finite()
        || !(10.0..=24.0).contains(&general.font_size)
        || !general.ui_scale.is_finite()
        || !(0.75..=2.5).contains(&general.ui_scale)
        || !(150.0..=480.0).contains(&general.sidebar_width)
        || !(240.0..=600.0).contains(&general.inspector_width)
        || !(1..=MAX_RESULTS).contains(&config.search.limit)
        || config.search.debounce_ms > 5000
        || !(MIN_QWEN_DIMENSIONS..=DEFAULT_DIMENSIONS).contains(&config.embedding.dimension)
        || !(10..=86400).contains(&config.embedding.idle_seconds)
        || !(1..=64).contains(&config.embedding.threads)
        || !(512..=32768).contains(&config.embedding.context_tokens)
        || !(100..=50_000_000).contains(&config.scanner.size_entry_budget)
        || !(1..=600).contains(&config.scanner.size_time_budget_seconds)
        || !crate::shared::i18n::is_valid_code(&general.language)
        || general.last_path.len() > MAX_PATH_BYTES
        || (!general.last_path.is_empty()
            && crate::shared::utils::parse_config_path(&general.last_path).is_err())
        || general.pending_restore.as_ref().is_some_and(|path| {
            path.len() > MAX_PATH_BYTES || crate::shared::utils::parse_config_path(path).is_err()
        })
        || !(1..=365).contains(&general.log_retention_days)
        || !(MIN_WINDOW_WIDTH..=MAX_WINDOW_SIZE).contains(&general.last_window_width)
        || !(MIN_WINDOW_HEIGHT..=MAX_WINDOW_SIZE).contains(&general.last_window_height)
        || config.search.roots.len() > MAX_CONFIG_ROOTS
        || config.agent.endpoints.len() > MAX_AGENT_ENDPOINTS
        || config.agent.allowed_roots.len() > MAX_CONFIG_ROOTS
        || config.sidebar.shares.len() > MAX_CONFIG_ROOTS
        || config.sidebar.closed_sections.len() > MAX_CONFIG_ROOTS
        || config
            .sidebar
            .closed_sections
            .iter()
            .any(|section| section.len() > 128 || section.chars().any(char::is_control))
        || config.sidebar.favorites.len() > 128
        || config.scanner.excluded_paths.len() > 128
    {
        return Err(ConfigError::Parse);
    }
    for (field, paths) in [
        (
            "Search roots must be absolute folder paths",
            &config.search.roots,
        ),
        (
            "Agent allowed roots must be absolute folder paths",
            &config.agent.allowed_roots,
        ),
        (
            "Network shares must be absolute paths such as \\\\server\\share",
            &config.sidebar.shares,
        ),
        (
            "Favorites must be absolute folder paths",
            &config.sidebar.favorites,
        ),
        (
            "Excluded paths must be absolute folder paths",
            &config.scanner.excluded_paths,
        ),
    ] {
        for path in paths {
            if path.len() > MAX_PATH_BYTES
                || path.chars().any(char::is_control)
                || crate::shared::utils::parse_config_path(path).is_err()
            {
                return Err(ConfigError::Field(field));
            }
        }
    }
    for endpoint in &config.agent.endpoints {
        let url = reqwest::Url::parse(&endpoint.base_url)
            .map_err(|_| ConfigError::Field("Agent endpoint URL is not a valid URL"))?;
        let local = matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
        );
        if endpoint.name.trim().is_empty() || endpoint.name.len() > 128 {
            return Err(ConfigError::Field(
                "Agent endpoint needs a name (up to 128 characters)",
            ));
        }
        if endpoint.model.trim().is_empty() || endpoint.model.len() > 256 {
            return Err(ConfigError::Field("Agent endpoint needs a model name"));
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
            || url.host_str().is_none()
        {
            return Err(ConfigError::Field(
                "Agent endpoint URL must not contain credentials, a query or a fragment",
            ));
        }
        if !(url.scheme() == "https"
            || (local && url.scheme() == "http" && endpoint.allow_local_unauthenticated))
        {
            return Err(ConfigError::Field(
                "Agent endpoint URL must use HTTPS, or http on this computer with unauthenticated loopback allowed",
            ));
        }
        if !(1..=600).contains(&endpoint.timeout_seconds) {
            return Err(ConfigError::Field(
                "Agent endpoint timeout must be 1–600 seconds",
            ));
        }
        if !endpoint
            .credential_env
            .chars()
            .all(|ch| ch.is_ascii_uppercase() || ch.is_ascii_digit() || ch == '_')
        {
            return Err(ConfigError::Field(
                "Credential variable names may only use A–Z, 0–9 and _",
            ));
        }
    }
    Ok(())
}
