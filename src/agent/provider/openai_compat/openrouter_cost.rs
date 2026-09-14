//! Best-effort lookup of OpenRouter's authoritative `total_cost` for a completion.
//!
//! The chat completion response only carries the OpenAI-standard `usage` token counts.
//! OpenRouter exposes the USD cost of a generation via a second endpoint,
//! `GET /generation?id={completion_id}`, which returns `data.total_cost`
//! (see <https://openrouter.ai/docs/api-reference/get-a-generation>).
//!
//! The extra roundtrip lets the billing layer charge the real cost instead of estimating it
//! from the per-token pricing table. The call is best-effort: any failure (transient HTTP
//! error, missing field, unexpected body) is returned as an error and the caller degrades to
//! the pricing table.

use anyhow::{Context, anyhow};
use serde::Deserialize;

/// Hosts served by OpenRouter. The `/generation` endpoint is OpenRouter-specific (other
/// OpenAI-compatible providers answer 404), so the lookup is only attempted against them.
const OPENROUTER_HOST: &str = "openrouter.ai";

#[derive(Debug, Deserialize)]
struct GenerationEnvelope {
    data: GenerationData,
}

#[derive(Debug, Deserialize)]
struct GenerationData {
    /// USD cost charged by OpenRouter for this generation.
    /// May be absent for streaming or failed generations.
    total_cost: Option<f64>,
}

/// Whether `base_url` points at OpenRouter (`openrouter.ai` or one of its subdomains).
pub fn is_openrouter_base_url(base_url: &str) -> bool {
    let Ok(url) = url::Url::parse(base_url) else {
        return false;
    };

    let Some(host) = url.host_str() else {
        return false;
    };

    host == OPENROUTER_HOST || host.ends_with(&format!(".{OPENROUTER_HOST}"))
}

/// Fetches `data.total_cost` for a known completion id.
///
/// `base_url` is the same base URL the chat completion was sent to, with a trailing slash.
/// Returns `Err` on any non-2xx response, parse failure or missing `total_cost`.
/// The caller is expected to log and degrade gracefully.
pub async fn fetch_total_cost_usd(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    completion_id: &str,
) -> anyhow::Result<f64> {
    let url = format!(
        "{}generation?id={}",
        base_url,
        percent_encode_query_value(completion_id)
    );

    let response = client
        .get(&url)
        .bearer_auth(api_key)
        .send()
        .await
        .context("GET /generation")?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().await.unwrap_or_default();
        return Err(anyhow!(
            "OpenRouter /generation returned HTTP {status}: {body}"
        ));
    }

    let envelope: GenerationEnvelope = response
        .json()
        .await
        .context("decode /generation envelope")?;

    envelope
        .data
        .total_cost
        .ok_or_else(|| anyhow!("OpenRouter /generation envelope missing data.total_cost"))
}

/// Minimal percent-encoder for a single query value.
///
/// OpenRouter completion ids look like `gen-<alphanumeric>`, so this only needs to be safe
/// against a stray `&` or `?`, not implement full RFC 3986.
fn percent_encode_query_value(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());

    for byte in value.bytes() {
        match byte {
            b'a'..=b'z' | b'A'..=b'Z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                encoded.push(byte as char)
            }
            _ => encoded.push_str(&format!("%{byte:02X}")),
        }
    }

    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, http::StatusCode, response::IntoResponse, routing::get};
    use serde_json::json;
    use std::net::SocketAddr;

    /// Spins up a mock that mimics the shape of OpenRouter's `/generation` endpoint.
    /// Returns the bound address; callers build the base URL (with trailing slash) from it.
    async fn spawn_mock(body: serde_json::Value, status: StatusCode) -> SocketAddr {
        let app = Router::new().route(
            "/generation",
            get(move || {
                let body = body.clone();
                async move { (status, axum::Json(body)).into_response() }
            }),
        );

        serve(app).await
    }

    async fn serve(app: Router) -> SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        addr
    }

    fn client() -> reqwest::Client {
        reqwest::Client::new()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn happy_path_returns_total_cost() {
        let addr = spawn_mock(
            json!({"data": {"id": "gen-abc", "total_cost": 0.0024}}),
            StatusCode::OK,
        )
        .await;

        let cost =
            fetch_total_cost_usd(&client(), &format!("http://{addr}/"), "sk-test", "gen-abc")
                .await
                .unwrap();

        assert!((cost - 0.0024).abs() < 1e-12);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn not_found_returns_err() {
        let addr = spawn_mock(json!({"error": "not found"}), StatusCode::NOT_FOUND).await;

        let err = fetch_total_cost_usd(&client(), &format!("http://{addr}/"), "sk-test", "gen-abc")
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("404"), "unexpected error: {err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn missing_total_cost_returns_err() {
        // OpenRouter may omit `total_cost` for streaming or failed generations.
        // The caller must fall back to the pricing table rather than treat the cost as zero.
        let addr = spawn_mock(json!({"data": {"id": "gen-abc"}}), StatusCode::OK).await;

        let err = fetch_total_cost_usd(&client(), &format!("http://{addr}/"), "sk-test", "gen-abc")
            .await
            .unwrap_err()
            .to_string();

        assert!(err.contains("total_cost"), "unexpected error: {err}");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn server_error_returns_err() {
        // Transient upstream incidents must surface as errors so the caller falls back to
        // per-token pricing instead of recording a zero cost.
        for status in [StatusCode::BAD_GATEWAY, StatusCode::SERVICE_UNAVAILABLE] {
            let addr = spawn_mock(json!({"error": "upstream"}), status).await;

            let err =
                fetch_total_cost_usd(&client(), &format!("http://{addr}/"), "sk-test", "gen-abc")
                    .await
                    .unwrap_err()
                    .to_string();

            assert!(
                err.contains(status.as_str()),
                "unexpected error for {status}: {err}"
            );
        }
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn malformed_json_returns_err() {
        async fn bad_body() -> impl IntoResponse {
            (
                StatusCode::OK,
                [("content-type", "application/json")],
                "this is not json {{{",
            )
        }

        let addr = serve(Router::new().route("/generation", get(bad_body))).await;

        let err = fetch_total_cost_usd(&client(), &format!("http://{addr}/"), "sk-test", "gen-abc")
            .await
            .unwrap_err()
            .to_string();

        assert!(
            err.contains("decode /generation envelope"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn percent_encodes_unsafe_chars() {
        assert_eq!(percent_encode_query_value("gen-abc_123"), "gen-abc_123");
        assert_eq!(percent_encode_query_value("gen?evil"), "gen%3Fevil");
        assert_eq!(percent_encode_query_value("a&b"), "a%26b");
        assert_eq!(percent_encode_query_value("é"), "%C3%A9");
    }

    #[test]
    fn detects_openrouter_base_url() {
        assert!(is_openrouter_base_url("https://openrouter.ai/api/v1"));
        assert!(is_openrouter_base_url("https://openrouter.ai/api/v1/"));
        assert!(is_openrouter_base_url("https://api.openrouter.ai/v1/"));

        assert!(!is_openrouter_base_url("https://api.openai.com/v1/"));
        assert!(!is_openrouter_base_url("https://notopenrouter.ai/api/v1/"));
        assert!(!is_openrouter_base_url("http://localhost:8080/v1/"));
        assert!(!is_openrouter_base_url(""));
    }
}
