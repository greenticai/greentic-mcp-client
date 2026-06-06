//! Auth header configuration for remote MCP servers.

use std::fmt;

/// How to authenticate to a remote MCP server. `None` for `header_name` means
/// the standard `Authorization: Bearer <token>` form; a custom name (e.g.
/// `X-Api-Key`) sends the raw token as that header's value.
pub struct McpAuth {
    pub header_name: Option<String>,
    pub token: String,
}

impl McpAuth {
    /// Standard `Authorization: Bearer <token>`.
    #[must_use]
    pub fn bearer(token: impl Into<String>) -> Self {
        Self {
            header_name: None,
            token: token.into(),
        }
    }

    /// The `(header name, header value)` pair to attach to every request.
    #[must_use]
    pub fn header(&self) -> (String, String) {
        match self.header_name.as_deref() {
            None => (
                "Authorization".to_string(),
                format!("Bearer {}", self.token),
            ),
            Some(name) if name.eq_ignore_ascii_case("authorization") => {
                (name.to_string(), format!("Bearer {}", self.token))
            }
            Some(name) => (name.to_string(), self.token.clone()),
        }
    }
}

// Manual Debug so the token can never leak into logs or error chains.
impl fmt::Debug for McpAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("McpAuth")
            .field("header_name", &self.header_name)
            .field("token", &"<redacted>")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_authorization_bearer() {
        let (name, value) = McpAuth::bearer("tok-1").header();
        assert_eq!(name, "Authorization");
        assert_eq!(value, "Bearer tok-1");
    }

    #[test]
    fn explicit_authorization_header_still_gets_bearer_prefix() {
        let auth = McpAuth {
            header_name: Some("authorization".into()),
            token: "tok-1".into(),
        };
        let (_, value) = auth.header();
        assert_eq!(value, "Bearer tok-1");
    }

    #[test]
    fn custom_header_sends_raw_token() {
        let auth = McpAuth {
            header_name: Some("X-Api-Key".into()),
            token: "tok-1".into(),
        };
        let (name, value) = auth.header();
        assert_eq!(name, "X-Api-Key");
        assert_eq!(value, "tok-1");
    }

    #[test]
    fn debug_redacts_token() {
        let auth = McpAuth::bearer("super-secret");
        let dbg = format!("{auth:?}");
        assert!(!dbg.contains("super-secret"));
        assert!(dbg.contains("<redacted>"));
    }
}
