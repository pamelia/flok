//! The `webfetch` tool — fetches content from a URL.
//!
//! Returns the page content as text, stripping HTML tags for a cleaner
//! representation. Supports HTML, JSON, and plain text responses.

use super::{Tool, ToolContext, ToolOutput};

const MAX_RESPONSE_BYTES: usize = 100_000;
const TIMEOUT_SECS: u64 = 30;

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
            .redirect(reqwest::redirect::Policy::limited(5))
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

        // Validate URL
        let parsed = reqwest::Url::parse(url).map_err(|e| anyhow::anyhow!("invalid URL: {e}"))?;

        // Security: block private/internal URLs
        if let Some(host) = parsed.host_str() {
            if host == "localhost"
                || host == "127.0.0.1"
                || host == "0.0.0.0"
                || host.starts_with("10.")
                || host.starts_with("172.")
                || host.starts_with("192.168.")
                || host == "169.254.169.254" // Cloud metadata
                || host == "metadata.google.internal"
            {
                return Ok(ToolOutput::error("Blocked: cannot fetch from private/internal URLs."));
            }
        }

        // Upgrade HTTP to HTTPS
        let fetch_url = if parsed.scheme() == "http" {
            url.replacen("http://", "https://", 1)
        } else {
            url.to_string()
        };

        let response = self
            .client
            .get(&fetch_url)
            .send()
            .await
            .map_err(|e| anyhow::anyhow!("Failed to fetch {fetch_url}: {e}"))?;

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
