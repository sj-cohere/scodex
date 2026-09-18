use codex_api::Reasoning;
use codex_api::ResponsesApiRequest;
use codex_model_provider_info::ModelProviderInfo;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use pretty_assertions::assert_eq;
use serde_json::json;

use super::ACP_ASTRA_PICKER_ID;
use super::ACP_DEV_BASE_URL;
use super::AcpModelProvider;
use crate::ModelProvider;

#[test]
fn astra_two_turn_request_preserves_tool_call_and_result_ids() {
    let provider = AcpModelProvider::new(ModelProviderInfo::create_acp_provider());
    let tool_call: ResponseItem = serde_json::from_value(json!({
        "type": "function_call",
        "id": "fc_item_1",
        "name": "shell",
        "arguments": "{\"command\":\"printf ok\"}",
        "call_id": "call_1"
    }))
    .expect("tool call");
    let tool_result: ResponseItem = serde_json::from_value(json!({
        "type": "function_call_output",
        "call_id": "call_1",
        "output": "ok"
    }))
    .expect("tool result");

    let mut first_api_provider = provider
        .info()
        .to_api_provider(/*auth_mode*/ None)
        .expect("ACP API provider");
    let mut first_request = request(ACP_ASTRA_PICKER_ID);
    first_request.input = vec![tool_call.clone()];
    provider
        .prepare_responses_request(&mut first_request, &mut first_api_provider)
        .expect("prepare first ACP request");
    assert_eq!(first_request.input, vec![tool_call.clone()]);

    let mut replay_api_provider = provider
        .info()
        .to_api_provider(/*auth_mode*/ None)
        .expect("ACP API provider");
    let expected_replay = vec![tool_call.clone(), tool_result.clone()];
    let mut replay_request = request(ACP_ASTRA_PICKER_ID);
    replay_request.input = vec![
        serde_json::from_value(json!({
            "type": "message",
            "role": "user",
            "content": [{"type": "input_text", "text": ""}]
        }))
        .expect("empty message"),
        tool_call,
        tool_result,
    ];
    replay_request.reasoning = Some(Reasoning {
        effort: Some(ReasoningEffort::Max),
        summary: None,
        context: None,
    });

    provider
        .prepare_responses_request(&mut replay_request, &mut replay_api_provider)
        .expect("prepare replayed ACP request");

    assert_eq!(replay_api_provider.base_url, ACP_DEV_BASE_URL);
    assert_eq!(replay_request.model, "gpt-6-astra");
    assert_eq!(replay_request.max_output_tokens, Some(16_384));
    assert_eq!(
        replay_request
            .reasoning
            .and_then(|reasoning| reasoning.effort),
        Some(ReasoningEffort::Max)
    );
    assert_eq!(replay_request.input, expected_replay);
}

#[test]
fn command_and_north_requests_omit_parallel_tool_calls() {
    let provider = AcpModelProvider::new(ModelProviderInfo::create_acp_provider());
    for picker_id in ["acp-command-bls-nightly", "acp-north-mini-code-1-0"] {
        let mut api_provider = provider
            .info()
            .to_api_provider(/*auth_mode*/ None)
            .expect("ACP API provider");
        let mut request = request(picker_id);

        provider
            .prepare_responses_request(&mut request, &mut api_provider)
            .expect("prepare ACP request");

        let value = serde_json::to_value(&request).expect("serialize request");
        assert_eq!(value.get("parallel_tool_calls"), None);
        assert_eq!(
            api_provider.base_url,
            codex_model_provider_info::ACP_PROD_BASE_URL
        );
        assert_eq!(request.model, picker_id.trim_start_matches("acp-"));
    }
}

#[test]
fn unknown_model_is_rejected_without_substitution() {
    let provider = AcpModelProvider::new(ModelProviderInfo::create_acp_provider());
    let mut api_provider = provider
        .info()
        .to_api_provider(/*auth_mode*/ None)
        .expect("ACP API provider");
    let mut request = request("acp-not-a-model");

    let error = provider
        .prepare_responses_request(&mut request, &mut api_provider)
        .expect_err("unknown ACP model must be rejected");

    assert!(
        error
            .to_string()
            .contains("unsupported ACP model `acp-not-a-model`"),
        "unexpected error: {error}"
    );
    assert_eq!(request.model, "acp-not-a-model");
}

#[test]
fn astra_rejects_unadvertised_reasoning_effort() {
    let provider = AcpModelProvider::new(ModelProviderInfo::create_acp_provider());
    let mut api_provider = provider
        .info()
        .to_api_provider(/*auth_mode*/ None)
        .expect("ACP API provider");
    let mut request = request(ACP_ASTRA_PICKER_ID);
    request.reasoning = Some(Reasoning {
        effort: Some(ReasoningEffort::Minimal),
        summary: None,
        context: None,
    });

    let error = provider
        .prepare_responses_request(&mut request, &mut api_provider)
        .expect_err("unadvertised Astra effort must be rejected");

    assert!(
        error
            .to_string()
            .contains("unsupported reasoning effort `minimal`"),
        "unexpected error: {error}"
    );
}

fn request(model: &str) -> ResponsesApiRequest {
    ResponsesApiRequest {
        model: model.to_string(),
        instructions: "test".to_string(),
        input: Vec::new(),
        tools: None,
        tool_choice: "auto".to_string(),
        parallel_tool_calls: Some(true),
        reasoning: None,
        store: false,
        stream: true,
        stream_options: None,
        include: Vec::new(),
        service_tier: None,
        prompt_cache_key: None,
        text: None,
        client_metadata: None,
        access_programs: None,
        max_output_tokens: None,
    }
}
