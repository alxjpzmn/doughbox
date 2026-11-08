use std::fmt::Write;

use crate::services::shared::urls::parse_public_origin;

use super::{Proposal, TradeResult};

const REVIEW_PATH: &str = "/dashboard/trading";

/// Inputs for an owner review link. The link is not a credential: opening it
/// never approves or submits an order.
pub struct LinkInput<'a> {
    pub configured_origin: Option<&'a str>,
    pub proposal_id: &'a str,
}

pub fn approval_url(input: &LinkInput<'_>) -> Option<String> {
    let origin = parse_public_origin(input.configured_origin?)?;
    let id = encode_query(input.proposal_id);
    if id.is_empty() {
        return None;
    }
    Some(format!("{origin}{REVIEW_PATH}?proposal={id}"))
}

pub fn proposal_review_text(proposal: &Proposal, url: Option<&str>, telegram: Option<&str>) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "{}", environment_label(&proposal.environment));
    let _ = writeln!(text, "{}", neutralize_links(&proposal.title));
    for trade in &proposal.trades {
        let _ = writeln!(text, "{}", trade_line(trade));
    }
    text.push('\n');
    if proposal.status == "pending" {
        text.push_str("Approve in Doughbox:\n");
    } else {
        text.push_str("Open in Doughbox:\n");
    }
    if let Some(url) = url {
        let _ = writeln!(text, "{url}");
        text.push_str("Opening this link does not submit the order. Sign in and confirm on the Trading page.\n");
    } else {
        text.push_str("No approval link could be built. Set DOUGHBOX_PUBLIC_URL to an absolute http(s) origin with no path, query, or credentials.\n");
        text.push_str("Otherwise open Trading → Orders in Doughbox. This tool cannot approve or submit the order.\n");
    }
    if let Some(telegram) = telegram {
        text.push_str(match (telegram, url.is_some()) {
            ("requested", true) => "A Telegram notification with this link was requested.\n",
            ("requested", false) => "A Telegram notification was requested without an approval link.\n",
            ("not_configured", _) => "Telegram is not configured, so no notification was sent.\n",
            ("unavailable", _) => "Telegram configuration is unavailable, so no notification was sent.\n",
            ("not_repeated", _) => "This proposal already existed, so no new Telegram notification was sent.\n",
            _ => "",
        });
    }
    text
}

pub fn proposal_notification(proposal: &Proposal, url: Option<&str>) -> String {
    let mut text = String::new();
    let _ = writeln!(text, "<b>Trading proposal awaiting approval</b>");
    let _ = writeln!(text, "{}", environment_label(&proposal.environment));
    text.push('\n');
    let _ = writeln!(text, "<b>{}</b>", escape_html(&neutralize_links(&proposal.title)));
    for trade in proposal.trades.iter().take(10) {
        let _ = writeln!(text, "{}", escape_html(&trade_line(trade)));
    }
    if proposal.trades.len() > 10 {
        let _ = writeln!(text, "and {} more", proposal.trades.len() - 10);
    }
    text.push('\n');
    if let Some(url) = url.filter(|url| url.starts_with("https://") || url.starts_with("http://")) {
        let _ = writeln!(text, "<a href=\"{}\">Review and approve</a>", escape_html(url));
        let _ = writeln!(text, "{url}");
    } else {
        text.push_str("Open Doughbox → Trading → Orders.\n");
    }
    text.push_str("Opening the link does not submit the order. Confirm in Doughbox.");
    text
}

fn environment_label(environment: &str) -> &'static str {
    if environment == "live" {
        "LIVE · real money"
    } else {
        "Demo"
    }
}

fn trade_line(trade: &TradeResult) -> String {
    format!(
        "{} {} {} @ {} {}",
        side_label(&trade.trade.side),
        neutralize_links(&trade.trade.quantity),
        neutralize_links(&trade.trade.ticker),
        neutralize_links(&trade.trade.limit_price),
        neutralize_links(&trade.currency),
    )
}

fn side_label(side: &str) -> String {
    match side {
        "buy" => "Buy".into(),
        "sell" => "Sell".into(),
        other => neutralize_links(other),
    }
}

fn encode_query(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.') {
            encoded.push(byte as char);
        } else {
            let _ = write!(encoded, "%{byte:02X}");
        }
    }
    encoded
}

fn neutralize_links(value: &str) -> String {
    value.replace("://", ":\u{200b}//").replace("www.", "www\u{200b}.")
}

fn escape_html(value: &str) -> String {
    value.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::trading::{TradeInput, TradeResult};

    fn input(configured_origin: Option<&str>) -> LinkInput<'_> {
        LinkInput {
            configured_origin,
            proposal_id: "11111111-1111-4111-8111-111111111111",
        }
    }

    fn proposal(title: &str, note: &str) -> Proposal {
        Proposal {
            id: "11111111-1111-4111-8111-111111111111".into(),
            client_request_id: "req".into(),
            title: title.into(),
            note: note.into(),
            environment: "live".into(),
            status: "pending".into(),
            created_at: "2026-01-02T03:04:05Z".parse().unwrap(),
            trades: vec![TradeResult {
                trade: TradeInput {
                    ticker: "AAPL_US_EQ".into(),
                    side: "buy".into(),
                    quantity: "0.5".into(),
                    limit_price: "150.00".into(),
                    time_validity: "DAY".into(),
                },
                name: "Apple <script>".into(),
                currency: "USD".into(),
                status: "pending".into(),
                broker_order_id: None,
                error: None,
            }],
        }
    }

    #[test]
    fn configured_origin_produces_a_review_link_without_a_capability() {
        let url = approval_url(&input(Some("HTTPS://Doughbox.Example.COM:443/"))).unwrap();
        assert_eq!(
            url,
            "https://doughbox.example.com/dashboard/trading?proposal=11111111-1111-4111-8111-111111111111"
        );
        assert!(!url.contains("token"));
    }

    #[test]
    fn missing_or_invalid_configuration_does_not_produce_a_link() {
        assert!(approval_url(&input(None)).is_none());
        for configured in [
            "",
            "javascript:alert(1)",
            "https://user:pass@doughbox.example.com",
            "https://doughbox.example.com/trading",
            "https://doughbox.example.com?next=https://evil.example",
            "https://doughbox.example.com/#fragment",
        ] {
            assert!(approval_url(&input(Some(configured))).is_none());
        }
    }

    #[test]
    fn configured_ipv6_origin_preserves_a_nondefault_port() {
        let url = approval_url(&input(Some("http://[::1]:8084/"))).unwrap();
        assert_eq!(url, "http://[::1]:8084/dashboard/trading?proposal=11111111-1111-4111-8111-111111111111");
    }

    #[test]
    fn review_text_exposes_only_the_approval_url() {
        let proposal = proposal("Buy now https://evil.example/phish", "see https://evil.example/note and www.evil.example");
        let url = "https://doughbox.example.com/dashboard/trading?proposal=11111111-1111-4111-8111-111111111111";
        let text = proposal_review_text(&proposal, Some(url), Some("requested"));
        assert_eq!(text.matches("https://").count(), 1);
        assert!(text.contains(url));
        assert!(text.contains("does not submit the order"));
        assert!(text.contains("Telegram notification with this link was requested"));
        assert!(!text.contains("evil.example/note"));
        assert!(text.contains("LIVE · real money"));
        assert!(text.contains("Buy 0.5 AAPL_US_EQ @ 150.00 USD"));
    }

    #[test]
    fn telegram_html_escapes_agent_text_and_links_only_the_review_page() {
        let proposal = proposal("Click <a href=\"https://evil.example\">here</a>", "https://evil.example/note");
        let url = "https://doughbox.example.com/dashboard/trading?proposal=11111111-1111-4111-8111-111111111111";
        let text = proposal_notification(&proposal, Some(url));
        assert_eq!(text.matches("<a href=").count(), 1);
        assert!(text.contains(&format!("<a href=\"{url}\">Review and approve</a>")));
        assert!(text.contains("&lt;a href=&quot;https:\u{200b}//evil.example&quot;&gt;"));
        assert!(!text.contains("evil.example/note"));
        assert!(!text.contains("<script>"));
        assert!(text.contains("does not submit the order"));
        assert!(text.contains("LIVE · real money"));
    }
}
