//! The `webfetch` tool — fetches content from a URL.
//!
//! Returns the page content as text, stripping HTML tags for a cleaner
//! representation. Supports HTML, JSON, and plain text responses.

use std::net::{Ipv4Addr, Ipv6Addr};

use super::{Tool, ToolContext, ToolOutput};

const MAX_RESPONSE_BYTES: usize = 100_000;
const TIMEOUT_SECS: u64 = 30;
const MAX_REDIRECTS: usize = 5;

/// Fetch content from a URL.
pub struct WebfetchTool {
    client: reqwest::Client,
}

impl WebfetchTool {
    /// Create a new webfetch tool with a shared HTTP client.
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(TIMEOUT_SECS))
            .user_agent("flok/0.0.1")
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap_or_else(|_| reqwest::Client::new());
        Self { client }
    }
}

impl Default for WebfetchTool {
    fn default() -> Self {
        Self::new()
    }
}

fn normalize_domain(domain: &str) -> String {
    domain.trim_end_matches('.').to_ascii_lowercase()
}

fn is_blocked_ipv4(addr: Ipv4Addr) -> bool {
    let octets = addr.octets();
    addr.is_private()
        || addr.is_loopback()
        || addr.is_link_local()
        || addr.is_broadcast()
        || addr.is_unspecified()
        || addr.is_multicast()
        || octets[0] == 0
        || (octets[0] == 100 && (64..=127).contains(&octets[1]))
        || (octets[0] == 198 && (octets[1] == 18 || octets[1] == 19))
}

fn is_blocked_ipv6(addr: Ipv6Addr) -> bool {
    if let Some(mapped) = addr.to_ipv4_mapped() {
        return is_blocked_ipv4(mapped);
    }

    let segments = addr.segments();
    addr.is_loopback()
        || addr.is_unspecified()
        || addr.is_multicast()
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] & 0xffc0) == 0xfe80
}

fn validate_fetch_url(url: &reqwest::Url) -> anyhow::Result<()> {
    match url.scheme() {
        "https" | "http" => {}
        scheme => anyhow::bail!("unsupported URL scheme: {scheme}"),
    }

    if !url.username().is_empty() || url.password().is_some() {
        anyhow::bail!("embedded URL credentials are not allowed");
    }

    let host = url.host().ok_or_else(|| anyhow::anyhow!("URL is missing a host"))?;
    match host {
        url::Host::Domain(domain) => {
            let normalized = normalize_domain(domain);
            if normalized == "localhost"
                || normalized.ends_with(".localhost")
                || normalized == "metadata.google.internal"
                || normalized.ends_with(".internal")
            {
                anyhow::bail!("cannot fetch from private/internal URLs");
            }
        }
        url::Host::Ipv4(addr) => {
            if is_blocked_ipv4(addr) {
                anyhow::bail!("cannot fetch from private/internal URLs");
            }
        }
        url::Host::Ipv6(addr) => {
            if is_blocked_ipv6(addr) {
                anyhow::bail!("cannot fetch from private/internal URLs");
            }
        }
    }

    Ok(())
}

fn upgrade_insecure_url(url: reqwest::Url) -> anyhow::Result<reqwest::Url> {
    if url.scheme() != "http" {
        return Ok(url);
    }

    let mut upgraded = url;
    upgraded.set_scheme("https").map_err(|()| anyhow::anyhow!("failed to upgrade URL to https"))?;
    Ok(upgraded)
}

#[async_trait::async_trait]
impl Tool for WebfetchTool {
    fn name(&self) -> &'static str {
        "webfetch"
    }

    fn description(&self) -> &'static str {
        "Fetch content from a URL. Returns the page content as text. \
         Supports HTML (stripped to text), JSON, and plain text."
    }

    fn parameters_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "required": ["url"],
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch content from (must be https)"
                }
            }
        })
    }

    fn permission_level(&self) -> super::PermissionLevel {
        // Network access is sensitive
        super::PermissionLevel::Write
    }

    fn describe_invocation(&self, args: &serde_json::Value) -> String {
        let url = args["url"].as_str().unwrap_or("(unknown)");
        format!("webfetch: {url}")
    }

    async fn execute(
        &self,
        args: serde_json::Value,
        _ctx: &ToolContext,
    ) -> anyhow::Result<ToolOutput> {
        let url = args["url"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("missing required parameter: url"))?;

        let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;
        let mut fetch_url = upgrade_insecure_url(parsed)?;
        if let Err(error) = validate_fetch_url(&fetch_url) {
            return Ok(ToolOutput::error(format!("Blocked: {error}")));
        }

        let mut redirect_count = 0usize;
        let response = loop {
            let response = self
                .client
                .get(fetch_url.clone())
                .send()
                .await
                .map_err(|e| anyhow::anyhow!("Failed to fetch {fetch_url}: {e}"))?;

            if !response.status().is_redirection() {
                break response;
            }

            if response.headers().get(reqwest::header::LOCATION).is_none() {
                return Ok(ToolOutput::error(format!(
                    "HTTP {} for {} without redirect location",
                    response.status(),
                    fetch_url
                )));
            }

            let location = response
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| anyhow::anyhow!("invalid redirect location header"))?;

            let next_url = fetch_url
                .join(location)
                .map_err(|e| anyhow::anyhow!("invalid redirect target {location}: {e}"))?;
            let next_url = upgrade_insecure_url(next_url)?;
            if let Err(error) = validate_fetch_url(&next_url) {
                return Ok(ToolOutput::error(format!("Blocked redirect target: {error}")));
            }

            if next_url == fetch_url {
                return Ok(ToolOutput::error(format!("Redirect loop detected for {fetch_url}")));
            }

            if redirect_count >= MAX_REDIRECTS {
                return Ok(ToolOutput::error(format!(
                    "Too many redirects while fetching {next_url}"
                )));
            }

            redirect_count += 1;
            fetch_url = next_url;
        };

        let status = response.status();
        if !status.is_success() {
            return Ok(ToolOutput::error(format!("HTTP {status} for {fetch_url}")));
        }

        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        // Read body with size limit
        let bytes = response.bytes().await?;
        if bytes.len() > MAX_RESPONSE_BYTES {
            let text = String::from_utf8_lossy(&bytes[..MAX_RESPONSE_BYTES]);
            return Ok(ToolOutput::success(format!(
                "{text}\n\n... (truncated, {:.0}KB of {:.0}KB shown)",
                MAX_RESPONSE_BYTES as f64 / 1024.0,
                bytes.len() as f64 / 1024.0,
            )));
        }

        let text = String::from_utf8_lossy(&bytes);

        // If HTML, do a basic strip of tags
        let output =
            if content_type.contains("html") { strip_html_tags(&text) } else { text.to_string() };

        // Truncate very long text output
        if output.len() > MAX_RESPONSE_BYTES {
            Ok(ToolOutput::success(format!("{}\n\n... (truncated)", &output[..MAX_RESPONSE_BYTES])))
        } else {
            Ok(ToolOutput::success(output))
        }
    }
}

/// Basic HTML tag stripping. Removes tags, collapses whitespace,
/// extracts text content. Not a full parser — just good enough for
/// LLM consumption.
fn strip_html_tags(html: &str) -> String {
    let mut result = String::with_capacity(html.len());
    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut last_was_space = false;

    let lower = html.to_lowercase();
    let chars: Vec<char> = html.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();

    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '<' {
            // Check for script/style start
            let remaining: String = lower_chars[i..].iter().take(20).collect();
            if remaining.starts_with("<script") {
                in_script = true;
            } else if remaining.starts_with("<style") {
                in_style = true;
            } else if remaining.starts_with("</script") {
                in_script = false;
            } else if remaining.starts_with("</style") {
                in_style = false;
            }
            in_tag = true;
            i += 1;
            continue;
        }

        if chars[i] == '>' {
            in_tag = false;
            i += 1;
            continue;
        }

        if in_tag || in_script || in_style {
            i += 1;
            continue;
        }

        let c = chars[i];
        if c.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            result.push(c);
            last_was_space = false;
        }

        i += 1;
    }

    // Clean up the result
    let lines: Vec<&str> = result.lines().map(str::trim).filter(|l| !l.is_empty()).collect();

    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strip_html_basic() {
        let html = "<html><body><h1>Hello</h1><p>World</p></body></html>";
        let result = strip_html_tags(html);
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
        assert!(!result.contains('<'));
    }

    #[test]
    fn strip_html_removes_scripts() {
        let html = "<p>before</p><script>alert('xss')</script><p>after</p>";
        let result = strip_html_tags(html);
        assert!(result.contains("before"));
        assert!(result.contains("after"));
        assert!(!result.contains("alert"));
    }

    #[test]
    fn strip_html_removes_styles() {
        let html = "<p>text</p><style>.foo{color:red}</style><p>more</p>";
        let result = strip_html_tags(html);
        assert!(result.contains("text"));
        assert!(!result.contains("color"));
    }

    #[test]
    fn validate_fetch_url_blocks_ipv6_loopback() {
        let url = reqwest::Url::parse("https://[::1]/secret").expect("url");
        let error = validate_fetch_url(&url).expect_err("expected blocked loopback");
        assert!(error.to_string().contains("private/internal"));
    }

    #[test]
    fn validate_fetch_url_blocks_private_ipv4() {
        let url = reqwest::Url::parse("https://192.168.1.20/admin").expect("url");
        let error = validate_fetch_url(&url).expect_err("expected blocked private ipv4");
        assert!(error.to_string().contains("private/internal"));
    }

    #[test]
    fn validate_fetch_url_blocks_localhost_subdomains() {
        let url = reqwest::Url::parse("https://api.localhost/metrics").expect("url");
        let error = validate_fetch_url(&url).expect_err("expected blocked localhost subdomain");
        assert!(error.to_string().contains("private/internal"));
    }

    #[test]
    fn validate_fetch_url_blocks_embedded_credentials() {
        let url = reqwest::Url::parse("https://user:pass@example.com/private").expect("url");
        let error = validate_fetch_url(&url).expect_err("expected blocked credentials");
        assert!(error.to_string().contains("credentials"));
    }

    #[test]
    fn validate_fetch_url_allows_public_https() {
        let url = reqwest::Url::parse("https://example.com/docs").expect("url");
        validate_fetch_url(&url).expect("public https should be allowed");
    }

    #[tokio::test]
    async fn webfetch_blocks_localhost() {
        let tool = WebfetchTool::new();
        let ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));
        let args = serde_json::json!({"url": "http://localhost:8080/secret"});
        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Blocked"));
    }

    #[tokio::test]
    async fn webfetch_blocks_metadata() {
        let tool = WebfetchTool::new();
        let ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));
        let args = serde_json::json!({"url": "http://169.254.169.254/latest/meta-data/"});
        let result = tool.execute(args, &ctx).await.unwrap();
        assert!(result.is_error);
        assert!(result.content.contains("Blocked"));
    }

    #[tokio::test]
    async fn webfetch_invalid_url() {
        let tool = WebfetchTool::new();
        let ctx = ToolContext::test(std::path::PathBuf::from("/tmp"));
        let args = serde_json::json!({"url": "not a url"});
        let result = tool.execute(args, &ctx).await;
        assert!(result.is_err());
    }
}
