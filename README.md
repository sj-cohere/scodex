# Scodex

Scodex is a fork of [OpenAI Codex](https://github.com/openai/codex) configured
for ACP inference over Tailscale. It keeps the normal Codex command-line
options, tools, sandboxing, and approval flow while using the `scodex`
executable and a separate default state directory.

## Build and install

Rust 1.95 is pinned by `codex-rs/rust-toolchain.toml`.

```shell
cd codex-rs
cargo build --release -p codex-cli --bin scodex
```

To install from a checkout:

```shell
cargo install --path codex-rs/cli --bin scodex
```

Run `scodex`. Unless `CODEX_HOME` is explicitly set, configuration, history,
credentials, logs, and caches live under `~/.scodex`; standard Codex state in
`~/.codex` is not used.

## ACP routing

Scodex uses the Responses HTTP/SSE transport without OpenAI authentication,
WebSockets, model substitution, or the local Responses proxy. Access to the
Tailscale hostnames is required.

- `acp-gpt-6-astra` routes to
  `https://llm-gateway-acp-dev.tail5566.ts.net/v1/responses` as
  `gpt-6-astra`.
- Every other allowed picker ID routes to
  `https://llm-gateway-acp-prod.tail5566.ts.net/v1/responses` after exactly one
  leading `acp-` is removed.
- Both gateways' `/v1/models` endpoints refresh the picker. Only the configured
  allowlist is exposed; an unknown model is rejected rather than replaced.

The allowed picker IDs are:

```text
acp-gpt-6-astra
acp-command-bls-nightly-previous
acp-command-a-plus-05-2026
acp-command-bls-nightly
acp-north-mini-code-1-0
acp-claude-sonnet-4-5
acp-claude-opus-4-6
acp-claude-sonnet-5
acp-claude-opus-4-8
acp-claude-opus-5
acp-gpt-5.6-terra
acp-gpt-5.6-luna
acp-gpt-5.6-sol
acp-gpt-5.5
acp-gemma-4-31b
```

`acp-gpt-6-astra` is the default. It advertises exactly
`low`, `medium`, `high`, `xhigh`, and `max` reasoning effort, with `medium` as
the default. Scodex forwards the selected effort unchanged and defaults ACP
requests to 16,384 output tokens. No reasoning choices are inferred for the
other models.

Model discovery confirms catalog visibility only. Inference permission is
checked separately by sending a request; failures are surfaced without
falling back to another model.

## Upstream

General Codex architecture and development guidance remains in the upstream
[documentation](https://developers.openai.com/codex) and this repository's
[contributing guide](./docs/contributing.md).

This repository is licensed under the [Apache-2.0 License](LICENSE).
