# Custom-provider usage API

This fork lets a custom model provider expose Codex-compatible quota statistics
for `/status`. Implement the HTTP endpoint below and enable it in `config.toml`.
The response uses the existing Codex backend format; OpenAI organization usage
reports and arbitrary billing API formats are not supported.

## Configuration

```toml
model_provider = "custom"

[model_providers.custom]
name = "Custom API"
base_url = "https://api.example.com/v1"
env_key = "CUSTOM_API_KEY"
supports_usage = true

# Optional absolute HTTP(S) endpoint; otherwise GET <base_url>/usage.
# usage_url = "https://api.example.com/backend-api/wham/usage"

# Optional provider headers and query parameters also apply to usage reads.
# http_headers = { "X-Project" = "example" }
# query_params = { scope = "project" }
```

`supports_usage` defaults to `false`. `usage_url` takes effect only when this flag
is enabled. Restart Codex after changing provider settings.

## Global usage proxy

Use a dedicated quota service regardless of the inference provider:

```toml
[usage_proxy]
url = "https://usage.example.com/quota"
env_key = "USAGE_PROXY_KEY"
```

Both fields are required. The URL must be absolute HTTP(S). Set the named
environment variable on the **app-server host**; missing, empty, or whitespace-only
values fail the read. The bodyless `GET` uses the full URL, including its query,
and `Authorization: Bearer <environment value>`. It never inherits inference
credentials, provider headers, or provider query parameters.

Usage reads select the configured proxy first, then an opted-in custom provider,
then existing account usage. The proxy works with built-in and custom providers,
including when `supports_usage = false`. No failed proxy read falls back to
provider or account usage. Redirects are rejected and reads time out after
10 seconds.

The proxy follows existing configuration-layer precedence. Named profiles use
`<name>.config.toml`, selected with `--profile <name>`; put `[usage_proxy]` in that
file to override the global settings. Project-local configuration cannot set
credential-routing endpoints. Restart after changing configuration. Omit the
configuration to disable the proxy.

The same payload, informational RPC metadata, immediate `/status` rendering,
asynchronous cache updates, and failure behavior described below apply to the
proxy. Reads happen only on `/status` or explicit `account/rateLimits/read` calls;
there is no startup polling, periodic polling, billing read, or recovery read.
Analytics/history, thread usage and costs, inference traffic, and internal memory
quota checks retain their existing routing.

## HTTP endpoints and authentication

| Configuration | HTTP request |
| --- | --- |
| `base_url = "https://api.example.com/v1"` | `GET https://api.example.com/v1/usage` |
| `base_url = "https://api.example.com/v1/"` | `GET https://api.example.com/v1/usage` |
| Explicit `usage_url` | `GET` the configured URL exactly, with provider query parameters appended |

The request has no body. Configured provider headers, environment-derived
headers, and query parameters are preserved. An override URL keeps its existing
query parameters as well. Configure default-route query parameters using
`query_params`, with a path-only `base_url`.

Authentication uses the model provider's credential resolver. For the example
above, set `CUSTOM_API_KEY`; the request includes
`Authorization: Bearer <CUSTOM_API_KEY>`. Stored ChatGPT credentials do not
replace this custom API key. Providers using asynchronous credential resolution
or request signing receive the final URL and headers before authentication is
applied, including when `usage_url` overrides the destination.

Return a successful HTTP status with a UTF-8 JSON body, preferably with
`Content-Type: application/json`. Redirects are rejected, including redirects
to another path on the same host. The app-server bounds credential resolution
and the usage read together to 10 seconds; the HTTP request also has a 10-second
timeout.

## Response format

The HTTP response uses **snake_case** keys. A minimal valid response is:

```json
{"plan_type": "pro"}
```

That response carries no quota windows. To display quota data, return windows
as in this example. Reset timestamps are illustrative Unix seconds.

```json
{
  "plan_type": "pro",
  "rate_limit": {
    "allowed": true,
    "limit_reached": false,
    "primary_window": {
      "used_percent": 42,
      "limit_window_seconds": 18000,
      "reset_after_seconds": 3600,
      "reset_at": 2000000000
    },
    "secondary_window": {
      "used_percent": 18,
      "limit_window_seconds": 604800,
      "reset_after_seconds": 86400,
      "reset_at": 2000082800
    }
  },
  "credits": {
    "has_credits": true,
    "unlimited": false,
    "balance": "12.50"
  },
  "additional_rate_limits": [
    {
      "limit_name": "Extra model",
      "metered_feature": "extra-model",
      "rate_limit": {
        "allowed": true,
        "limit_reached": false,
        "primary_window": {
          "used_percent": 75,
          "limit_window_seconds": 3600,
          "reset_after_seconds": 600,
          "reset_at": 1999997000
        }
      }
    }
  ]
}
```

| Field | Contract |
| --- | --- |
| `plan_type` | Required string. Recognized values include `free`, `plus`, `pro`, `team`, `business`, and `enterprise`. Unrecognized strings map to `unknown`. |
| `rate_limit` | Optional object or `null`. If supplied, `allowed` and `limit_reached` are required booleans. |
| `primary_window`, `secondary_window` | Each is optional or `null`. Every supplied window requires all four integer fields shown above. |
| `used_percent` | Percentage consumed; use integers from 0 to 100. `/status` displays the remaining percentage. |
| `limit_window_seconds` | Window duration in seconds. Positive durations convert to minutes, rounding up; nonpositive durations have no duration label. |
| `reset_after_seconds` | Required by the decoder. The displayed reset time uses `reset_at`. |
| `reset_at` | Absolute reset time in Unix seconds. All four window integers must fit a signed 32-bit integer. |
| `credits` | Optional object or `null`. Requires boolean `has_credits` and `unlimited`; `balance` is an optional string or `null`, not a JSON number. |
| `additional_rate_limits` | Optional array or `null`. Each entry requires string `limit_name` and `metered_feature`; its `rate_limit` is optional or `null` and follows the same contract as the primary limit. |

The main quota receives the snapshot ID `codex`. Additional quotas use
`metered_feature` as their ID and `limit_name` as their display label. Use stable,
unique additional IDs distinct from `codex`. Credits belong to the main quota;
additional quotas do not receive separate credit balances.

Optional `spend_control` and `rate_limit_reached_type` fields also use the
existing backend decoder. Their complete types, along with the recognized plan
names, are defined in
[RateLimitStatusPayload](codex-rs/codex-backend-openapi-models/src/models/rate_limit_status_payload.rs)
and
[SpendControlStatusDetails](codex-rs/codex-backend-openapi-models/src/models/spend_control_status_details.rs).
Unknown JSON fields are ignored.

## App-server API and `/status`

The TUI requests usage through the existing `account/rateLimits/read` JSON-RPC
method. Other app-server clients can use the same method:

```json
{"jsonrpc": "2.0", "id": 1, "method": "account/rateLimits/read", "params": {}}
```

For an opted-in provider, no ChatGPT login is required. The response retains the
existing RPC format with **camelCase** keys: `rateLimits` contains the main
snapshot and `rateLimitsByLimitId` contains all snapshots, including `codex`.
Window fields become `usedPercent`, `windowDurationMins`, and `resetsAt`.

Custom reads are informational. `ordinaryUsageAllowed`, `rateLimitResetCredits`,
`accountId`, and `rateLimitUpsell` are returned as `null`, even if corresponding
metadata appears in the HTTP response. Custom usage does not trigger ChatGPT
model-switch prompts, billing banners, reset-credit actions, or inference
recovery. No reset-credit or billing endpoint is required.

`/status` renders immediately, then updates asynchronously with the result.
Custom usage is fetched on demand through `/status`; it does not enable startup
prefetch or periodic polling, even when stored ChatGPT credentials exist.

Missing provider credentials cause a failed read. HTTP errors, redirects,
malformed responses, and timeouts also fail the refresh. The TUI finishes the
pending refresh, retains any cached quota data, and stays responsive. Failed
reads do not replace cached data with zero usage.

The request implementation is in
[provider_usage.rs](codex-rs/backend-client/src/client/provider_usage.rs);
the RPC adapter is in
[the account processor](codex-rs/app-server/src/request_processors/account_processor/provider_usage.rs).
