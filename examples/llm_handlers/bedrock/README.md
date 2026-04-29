# AWS Bedrock LLM Handler — Reference Skeleton

Reference skeleton for routing `llm.call` to AWS Bedrock's `InvokeModel` API. Bedrock signs every request with AWS SigV4; rather than hand-roll signing (~100 LOC of HMAC plumbing), this skeleton uses the official `aws-sdk-bedrockruntime` crate.

**This is a reference skeleton — not a production handler.** It compiles in your fork once you wire in a real `aws_sdk_bedrockruntime::Client` (the file uses a `BedrockClient` placeholder so the example doesn't drag the AWS SDK into Boruna's workspace).

## Setup

In your integrator crate:

```toml
[dependencies]
aws-config = { version = "1", features = ["behavior-version-latest"] }
aws-sdk-bedrockruntime = "1"
tokio = { version = "1", features = ["rt"] }
serde_json = "1"
boruna-vm = { path = "..." }
boruna-bytecode = { path = "..." }
```

Replace the placeholder `BedrockClient` and the `from_env` body with:

```rust
let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
    .load()
    .await;
let client = aws_sdk_bedrockruntime::Client::new(&config);
```

## IAM requirements

Attach a role with at minimum:

```json
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Action": ["bedrock:InvokeModel"],
    "Resource": "arn:aws:bedrock:<region>::foundation-model/<model-id>"
  }]
}
```

Bedrock model access also needs to be **explicitly granted** in the AWS console (AWS Bedrock → Model access → request access for each foundation model your account uses).

## Configuration

| Variable | Purpose |
|---|---|
| `BEDROCK_MODEL_ID` | e.g. `anthropic.claude-3-5-sonnet-20241022-v2:0`, `anthropic.claude-sonnet-4-5-20241022-v2:0`, `meta.llama3-1-70b-instruct-v1:0` |
| `AWS_REGION` | Read by the AWS SDK; pick a region where your model is available |
| `AWS_ACCESS_KEY_ID` / `AWS_SECRET_ACCESS_KEY` / `AWS_SESSION_TOKEN` | Standard AWS credential chain — IAM role / IMDS / SSO / env all work |

## Sync trait, async SDK

`CapabilityHandler::handle` is sync; the AWS SDK is async. The skeleton owns a per-instance `tokio::runtime::Runtime` and `block_on`s each call. This is the same pattern Boruna's S3 / GCS / Azure `BundleStorage` adapters use — see `orchestrator/src/audit/storage_s3.rs` for the canonical discussion.

## Model-family wire shape

Bedrock's `InvokeModel` returns the model's raw response body verbatim — the wire shape is **per model family**. This skeleton shows the **Anthropic-on-Bedrock** request body (Claude family). For others:

| Family | Request shape | Response shape |
|---|---|---|
| Anthropic (Claude) | `{anthropic_version, max_tokens, messages, temperature}` | `content[0].text` |
| Meta (Llama) | `{prompt, max_gen_len, temperature}` | `generation` |
| Amazon (Titan) | `{inputText, textGenerationConfig: {maxTokenCount, temperature}}` | `results[0].outputText` |
| Mistral | `{prompt, max_tokens, temperature}` | `outputs[0].text` |
| Cohere (Command) | `{message, max_tokens, temperature}` | `text` |

Replace the `serde_json::json!` body and the response-extraction `.get("content")` chain to match your model.

## Cross-region inference profiles

For production workloads, use Bedrock's **cross-region inference profiles** (e.g. `us.anthropic.claude-...`) rather than direct model IDs. They give you automatic failover across regions with no code changes.

```bash
export BEDROCK_MODEL_ID="us.anthropic.claude-sonnet-4-5-20241022-v2:0"
```

## What this skeleton doesn't do

- **Provisioned throughput.** For high-volume workloads, request a Provisioned Throughput unit (PT) and pass its ARN as `model_id` instead of the foundation-model ID.
- **Guardrails.** Bedrock has a separate `apply_guardrail` API and an `applyGuardrails` parameter on `InvokeModel`. Wire those in if your compliance posture requires content filtering.
- **Streaming.** Use `invoke_model_with_response_stream` for streaming, but Boruna's capability calls are synchronous so you'll need to collect the stream before returning.
- **Cost accounting.** Bedrock returns `usage.input_tokens` + `usage.output_tokens` in the response — extract them in your handler if you need per-call cost tracking.

## Determinism

Bedrock-served models inherit their underlying determinism story:
- **Anthropic-on-Bedrock**: `temperature: 0` is honored; no `seed` parameter.
- **Llama-on-Bedrock**: `temperature: 0` only.
- **Titan / Mistral / Cohere**: same — `temperature: 0`, no seed.

For full reproducibility, combine with `boruna workflow run --record-net-to <tape>`. But note: AWS SDK calls don't go through Boruna's `RecordingHttpHandler` — you'd need to record at the SDK layer (e.g. `aws_smithy_runtime` interceptors) or use Boruna's `--replay-net-from` only for non-Bedrock capabilities in the same workflow.

See [`docs/guides/llm-integration.md`](../../../docs/guides/llm-integration.md) for the full discussion.
