//! Log set-up (`api-v1.md` §9). Nothing secret may be logged at any level, `TRACE`
//! included. The relay's own code never logs a body, a header, a token or a query string;
//! the third-party crates that could (the WebSocket protocol layer traces every message it
//! receives, and the first message carries the session token) are held at `INFO` whatever
//! `RUST_LOG` says.

use tracing_subscriber::EnvFilter;

use crate::config::LogFormat;

/// Directives that win over any `RUST_LOG`: these crates may see secrets.
pub const HELD_AT_INFO: &str = "tungstenite=info,tokio_tungstenite=info,hyper=info,hyper_util=info,h2=info,\
axum=info,axum_core=info,tower=info,tower_http=info,ureq=info,ureq_proto=info,rustls=info,rusqlite=info,\
tokio=info,mio=info,http=info,http_body=info";

/// The filter for a user-provided `RUST_LOG` (default `info`) with the held crates appended.
pub fn filter(user: Option<&str>) -> EnvFilter {
    let base = user
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("info");
    EnvFilter::try_new(format!("{base},{HELD_AT_INFO}"))
        .unwrap_or_else(|_| EnvFilter::new(format!("info,{HELD_AT_INFO}")))
}

/// Installs the global subscriber: stdout, JSON by default (`log_format`).
pub fn init(format: LogFormat) {
    let f = filter(std::env::var("RUST_LOG").ok().as_deref());
    let builder = tracing_subscriber::fmt()
        .with_env_filter(f)
        .with_writer(std::io::stdout);
    let r = match format {
        LogFormat::Json => builder
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .try_init(),
        LogFormat::Text => builder.try_init(),
    };
    if r.is_err() {
        eprintln!("tomi-relay: a logger is already installed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_crates_stay_at_info_even_under_trace() {
        let f = filter(Some("trace"));
        let text = f.to_string();
        assert!(text.contains("tungstenite=info"), "{text}");
        assert!(text.contains("trace"), "{text}");
    }

    #[test]
    fn a_broken_rust_log_falls_back_to_info() {
        let f = filter(Some("this is [not valid"));
        assert!(f.to_string().contains("info"));
    }
}
