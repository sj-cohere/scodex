use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use codex_api::ModelsClient;
use codex_api::Provider;
use codex_api::ReqwestTransport;
use codex_api::ResponsesApiRequest;
use codex_api::map_api_error;
use codex_http_client::ClientRouteClass;
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_login::default_client::ClientRedirectPolicy;
use codex_login::default_client::create_client_for_route_async;
use codex_model_provider_info::ACP_PROD_BASE_URL;
use codex_model_provider_info::ModelProviderInfo;
use codex_models_manager::collaboration_mode_presets::builtin_collaboration_mode_presets;
use codex_models_manager::manager::ModelsManager;
use codex_models_manager::manager::ModelsManagerFuture;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_models_manager::manager::StaticModelsManager;
use codex_models_manager::model_info::model_info_for_custom_provider;
use codex_protocol::account::ProviderAccount;
use codex_protocol::config_types::CollaborationModeMask;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CoreResult;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::openai_models::ReasoningEffortPreset;
use http::HeaderMap;
use serde::Deserialize;
use tokio::sync::RwLock;
use tokio::sync::TryLockError;
use tracing::warn;

use crate::ModelProvider;
use crate::ModelProviderFuture;
use crate::ProviderAccountResult;
use crate::ProviderAccountState;
use crate::ResolvedResponsesProvider;
use crate::auth::ProviderAuthScope;
use crate::auth::ResolvedProviderAuth;
use crate::auth::unauthenticated_auth_provider;
use crate::workspace_routing::WorkspaceRoutingContext;

pub(crate) const ACP_DEV_BASE_URL: &str = "https://llm-gateway-acp-dev.tail5566.ts.net/v1";
pub(crate) const ACP_ASTRA_PICKER_ID: &str = "acp-gpt-6-astra";

const ACP_MODEL_IDS: [&str; 15] = [
    ACP_ASTRA_PICKER_ID,
    "acp-command-bls-nightly-previous",
    "acp-command-a-plus-05-2026",
    "acp-command-bls-nightly",
    "acp-north-mini-code-1-0",
    "acp-claude-sonnet-4-5",
    "acp-claude-opus-4-6",
    "acp-claude-sonnet-5",
    "acp-claude-opus-4-8",
    "acp-claude-opus-5",
    "acp-gpt-5.6-terra",
    "acp-gpt-5.6-luna",
    "acp-gpt-5.6-sol",
    "acp-gpt-5.5",
    "acp-gemma-4-31b",
];

pub(crate) fn upstream_model_id(picker_id: &str) -> Option<&str> {
    ACP_MODEL_IDS
        .contains(&picker_id)
        .then(|| picker_id.strip_prefix("acp-"))
        .flatten()
}

fn model_catalog() -> Vec<ModelInfo> {
    ACP_MODEL_IDS
        .iter()
        .enumerate()
        .map(|(index, picker_id)| {
            let mut model = model_info_for_custom_provider(picker_id);
            model.display_name = (*picker_id).to_string();
            model.visibility = ModelVisibility::List;
            model.priority = i32::try_from(index).unwrap_or(i32::MAX);
            model.used_fallback_model_metadata = false;
            model.default_reasoning_level = None;
            model.supported_reasoning_levels.clear();
            model.default_reasoning_summary = ReasoningSummary::None;
            model.supports_reasoning_summary_parameter = false;
            if *picker_id == ACP_ASTRA_PICKER_ID {
                model.default_reasoning_level = Some(ReasoningEffort::Medium);
                model.supported_reasoning_levels = [
                    (
                        ReasoningEffort::Low,
                        "Fast responses with lighter reasoning",
                    ),
                    (
                        ReasoningEffort::Medium,
                        "Balances speed and reasoning depth",
                    ),
                    (
                        ReasoningEffort::High,
                        "Greater reasoning depth for complex problems",
                    ),
                    (
                        ReasoningEffort::XHigh,
                        "Extra high reasoning depth for complex problems",
                    ),
                    (
                        ReasoningEffort::Max,
                        "Maximum reasoning depth for the hardest problems",
                    ),
                ]
                .into_iter()
                .map(|(effort, description)| ReasoningEffortPreset {
                    effort,
                    description: description.to_string(),
                })
                .collect();
            }
            model
        })
        .collect()
}

#[derive(Debug)]
pub(crate) struct AcpModelProvider {
    info: ModelProviderInfo,
}

impl AcpModelProvider {
    pub(crate) fn new(info: ModelProviderInfo) -> Self {
        Self { info }
    }
}

impl ModelProvider for AcpModelProvider {
    fn info(&self) -> &ModelProviderInfo {
        &self.info
    }

    fn auth_manager(&self) -> Option<Arc<AuthManager>> {
        None
    }

    fn auth(&self) -> ModelProviderFuture<'_, Option<CodexAuth>> {
        Box::pin(async { None })
    }

    fn account_state(&self) -> ProviderAccountResult {
        Ok(ProviderAccountState {
            account: None::<ProviderAccount>,
            requires_openai_auth: false,
        })
    }

    fn prepare_responses_request(
        &self,
        request: &mut ResponsesApiRequest,
        provider: &mut Provider,
    ) -> CoreResult<()> {
        let picker_id = request.model.clone();
        let upstream_id = upstream_model_id(&picker_id).ok_or_else(|| {
            CodexErr::InvalidRequest(format!("unsupported ACP model `{picker_id}`"))
        })?;
        let is_astra = picker_id == ACP_ASTRA_PICKER_ID;
        provider.base_url = if is_astra {
            ACP_DEV_BASE_URL.to_string()
        } else {
            ACP_PROD_BASE_URL.to_string()
        };
        if upstream_id.starts_with("command-") || upstream_id.starts_with("north-") {
            request.parallel_tool_calls = None;
        }
        request.input.retain(|item| {
            !matches!(
                item,
                ResponseItem::Message { content, .. } if content.iter().all(|item| match item {
                    ContentItem::InputText { text } | ContentItem::OutputText { text } => {
                        text.trim().is_empty()
                    }
                    ContentItem::InputImage { .. } | ContentItem::InputAudio { .. } => false,
                })
            )
        });
        if !is_astra
            && request.reasoning.as_ref().is_some_and(|reasoning| {
                reasoning.effort.is_none()
                    && reasoning.summary.is_none()
                    && reasoning.context.is_none()
            })
        {
            request.reasoning = None;
        }
        if is_astra
            && let Some(effort) = request
                .reasoning
                .as_ref()
                .and_then(|reasoning| reasoning.effort.as_ref())
            && !matches!(
                effort,
                ReasoningEffort::Low
                    | ReasoningEffort::Medium
                    | ReasoningEffort::High
                    | ReasoningEffort::XHigh
                    | ReasoningEffort::Max
            )
        {
            return Err(CodexErr::InvalidRequest(format!(
                "unsupported reasoning effort `{}` for `{ACP_ASTRA_PICKER_ID}`",
                effort.as_str()
            )));
        }
        request.max_output_tokens.get_or_insert(/*value*/ 16_384);
        request.model = upstream_id.to_string();
        Ok(())
    }

    fn responses_api_provider<'a>(
        &'a self,
        _routing_context: &'a WorkspaceRoutingContext,
    ) -> ModelProviderFuture<'a, CoreResult<ResolvedResponsesProvider>> {
        Box::pin(async move {
            Ok(ResolvedResponsesProvider {
                provider: self.info.to_api_provider(/*auth_mode*/ None)?,
                redirect_policy: ClientRedirectPolicy::Reject,
            })
        })
    }

    fn api_auth_for_scope(
        &self,
        _scope: ProviderAuthScope,
    ) -> ModelProviderFuture<'_, CoreResult<ResolvedProviderAuth>> {
        Box::pin(async { Ok(ResolvedProviderAuth::new(unauthenticated_auth_provider())) })
    }

    fn models_manager(
        &self,
        _codex_home: PathBuf,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        match config_model_catalog {
            Some(model_catalog) => Arc::new(StaticModelsManager::new(
                /*auth_manager*/ None,
                model_catalog,
            )),
            None => Arc::new(AcpModelsManager::new()),
        }
    }

    fn models_manager_without_cache(
        &self,
        config_model_catalog: Option<ModelsResponse>,
    ) -> SharedModelsManager {
        self.models_manager(PathBuf::new(), config_model_catalog)
    }
}

#[derive(Debug)]
struct AcpModelsManager {
    models: RwLock<Vec<ModelInfo>>,
}

impl AcpModelsManager {
    fn new() -> Self {
        Self {
            models: RwLock::new(model_catalog()),
        }
    }

    async fn refresh(&self, http_client_factory: HttpClientFactory) -> CoreResult<()> {
        let (dev, prod) = tokio::join!(
            fetch_model_ids(ACP_DEV_BASE_URL, http_client_factory.clone()),
            fetch_model_ids(ACP_PROD_BASE_URL, http_client_factory),
        );
        let dev = dev?;
        let prod = prod?;
        let discovered = model_catalog()
            .into_iter()
            .filter(|model| {
                upstream_model_id(&model.slug).is_some_and(|upstream_id| {
                    if model.slug == ACP_ASTRA_PICKER_ID {
                        dev.contains(upstream_id)
                    } else {
                        prod.contains(upstream_id)
                    }
                })
            })
            .collect();
        *self.models.write().await = discovered;
        Ok(())
    }
}

impl ModelsManager for AcpModelsManager {
    fn raw_model_catalog(
        &self,
        refresh_strategy: RefreshStrategy,
        http_client_factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, ModelsResponse> {
        Box::pin(async move {
            if !matches!(refresh_strategy, RefreshStrategy::Offline)
                && let Err(err) = self.refresh(http_client_factory).await
            {
                warn!("failed to refresh ACP models: {err}");
            }
            ModelsResponse {
                models: self.models.read().await.clone(),
            }
        })
    }

    fn get_remote_models(&self) -> ModelsManagerFuture<'_, Vec<ModelInfo>> {
        Box::pin(async move { self.models.read().await.clone() })
    }

    fn try_get_remote_models(&self) -> Result<Vec<ModelInfo>, TryLockError> {
        self.models.try_read().map(|models| models.clone())
    }

    fn auth_manager(&self) -> Option<&AuthManager> {
        None
    }

    fn list_collaboration_modes(&self) -> Vec<CollaborationModeMask> {
        builtin_collaboration_mode_presets()
    }

    fn refresh_if_new_etag(
        &self,
        _etag: String,
        http_client_factory: HttpClientFactory,
    ) -> ModelsManagerFuture<'_, ()> {
        Box::pin(async move {
            if let Err(err) = self.refresh(http_client_factory).await {
                warn!("failed to refresh ACP models: {err}");
            }
        })
    }
}

#[derive(Deserialize)]
struct OpenAiModelList {
    data: Vec<OpenAiModel>,
}

#[derive(Deserialize)]
struct OpenAiModel {
    id: String,
}

async fn fetch_model_ids(
    base_url: &str,
    http_client_factory: HttpClientFactory,
) -> CoreResult<BTreeSet<String>> {
    let mut provider_info = ModelProviderInfo::create_acp_provider();
    provider_info.base_url = Some(base_url.to_string());
    let provider = provider_info.to_api_provider(/*auth_mode*/ None)?;
    let request_url =
        ModelsClient::<ReqwestTransport>::request_url(&provider, env!("CARGO_PKG_VERSION"));
    let transport = create_client_for_route_async(
        http_client_factory,
        request_url.clone(),
        ClientRouteClass::Api,
    )
    .await
    .map(ReqwestTransport::from_http_client)?;
    let client = ModelsClient::new(transport, provider, unauthenticated_auth_provider());
    let (body, _) = client
        .list_models_raw(
            request_url,
            HeaderMap::new(),
            /*response_body_limit_bytes*/ Some(1024 * 1024),
        )
        .await
        .map_err(map_api_error)?;
    let response: OpenAiModelList = serde_json::from_slice(&body).map_err(|err| {
        CodexErr::Io(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to decode ACP model list: {err}"),
        ))
    })?;
    Ok(response.data.into_iter().map(|model| model.id).collect())
}

#[cfg(test)]
#[path = "acp_tests.rs"]
mod tests;
