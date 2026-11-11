use std::fmt::{self, Write};

use itertools::Itertools;
use rust_decimal::Decimal;

use crate::services::shared::urls::parse_public_origin;

use super::{pies::{PieResult, PieSettings, PreparedPieMutation}, Proposal, ProposalContent, TradeResult};

const REVIEW_PATH: &str = "/dashboard/trading";
const NOTIFICATION_ITEMS: usize = 12;
// Escaped UTF-8 bytes conservatively bound rendered UTF-16 length. Leave room
// for the review footer and the notification transport's debug prefix.
const NOTIFICATION_BODY_BYTES: usize = 4096 - 256;

struct NotificationBody {
    text: String,
    overflowed: bool,
}

impl Write for NotificationBody {
    fn write_str(&mut self, value: &str) -> fmt::Result {
        if self.overflowed || value.len() > NOTIFICATION_BODY_BYTES - self.text.len() {
            self.overflowed = true;
            return Err(fmt::Error);
        }
        self.text.push_str(value);
        Ok(())
    }
}

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
    for line in content_lines(proposal) {
        let _ = writeln!(text, "{}", neutralize_links(&line));
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
    let mut text = NotificationBody { text: String::with_capacity(1024), overflowed: false };
    let _ = writeln!(text, "<b>Trading proposal · Awaiting approval</b>");
    let _ = writeln!(text, "<b>{}</b>", environment_label(&proposal.environment));
    let compact_end = text.text.len();
    let _ = writeln!(text, "\n<b>{}</b>", html(&proposal.title));
    match &proposal.content {
        ProposalContent::Trades { trades } => {
            let _ = writeln!(text, "\n<b>Limit orders · {}</b>", trades.len());
            for trade in trades.iter().take(NOTIFICATION_ITEMS) {
                let _ = writeln!(text, "• <b>{}</b> {} × <code>{}</code>\n  Limit: <b>{} {}</b>",
                    html(&side_label(&trade.trade.side)), html(&trade.trade.quantity),
                    html(&trade.trade.ticker), html(&trade.trade.limit_price), html(&trade.currency));
            }
            if trades.len() > NOTIFICATION_ITEMS {
                let _ = writeln!(text, "<i>… {} more orders in Doughbox.</i>", trades.len() - NOTIFICATION_ITEMS);
            }
        }
        ProposalContent::Pie { pie } => pie_notification(&mut text, pie),
    }
    if text.overflowed {
        // A failed write can stop inside an HTML tag; discard the entire summary.
        text.text.truncate(compact_end);
        text.text.push_str("\n<i>Summary omitted because it is too long. Review the full proposal in Doughbox.</i>\n");
    }
    let mut text = text.text;
    text.push('\n');
    if let Some(url) = url.filter(|url| url.starts_with("https://") || url.starts_with("http://")) {
        let _ = writeln!(text, "<a href=\"{}\"><b>Review and approve</b></a>", escape_html(url));
    } else {
        text.push_str("Open Doughbox → Trading → Orders.\n");
    }
    text.push_str("<i>Nothing is submitted until you confirm in Doughbox.</i>");
    text
}

fn pie_notification(text: &mut impl Write, pie: &PieResult) {
    let _ = writeln!(text);
    match &pie.mutation {
        PreparedPieMutation::Create { .. } => {
            let _ = text.write_str("<b>Create Pie</b>\n<i>Configuration only · no funding or purchases.</i>\n");
        }
        PreparedPieMutation::Update { pie_id, .. } => {
            let _ = writeln!(text, "<b>Update Pie #{}</b>\n<i>Settings only · no shares bought or sold.</i>", html(pie_id));
        }
        PreparedPieMutation::Duplicate { pie_id, .. } => {
            let _ = writeln!(text, "<b>Duplicate Pie #{}</b>\n<i>Composition only; no holdings, AutoInvest or goals copied. No funding.</i>", html(pie_id));
        }
        PreparedPieMutation::Delete { pie_id } => {
            let _ = writeln!(text, "<b>Permanently delete Pie #{}</b>\n<i>Empty investments and Pie cash in Trading 212 first.</i>", html(pie_id));
            if let Some(name) = pie.before.as_ref().and_then(|pie| pie.settings.as_ref()).and_then(|settings| settings.name.as_deref()) {
                let _ = writeln!(text, "Pie: <b>{}</b>", html(name));
            }
            return;
        }
    }

    let before = if matches!(pie.mutation, PreparedPieMutation::Update { .. }) {
        let settings = pie.before.as_ref().and_then(|pie| pie.settings.as_ref());
        if settings.is_none() {
            let _ = text.write_str("<i>Previous settings unavailable; review the full proposal in Doughbox.</i>\n");
        }
        settings
    } else {
        None
    };
    let Some(after) = &pie.expected_settings else {
        let _ = text.write_str("<i>Intended settings unavailable; review the full proposal in Doughbox.</i>\n");
        return;
    };
    pie_settings_notification(text, before, after);
    pie_allocation_notification(text, before, after);
}

fn pie_settings_notification(text: &mut impl Write, before: Option<&PieSettings>, after: &PieSettings) {
    let mut heading_written = false;
    for (label, old, new) in [
        ("Name", before.and_then(|s| s.name.as_deref()), after.name.as_deref()),
        ("Dividends", before.and_then(|s| s.dividend_cash_action.as_deref()), after.dividend_cash_action.as_deref()),
        ("Goal", before.and_then(|s| s.goal.as_deref()), after.goal.as_deref()),
        ("End date", before.and_then(|s| s.end_date.as_deref()), after.end_date.as_deref()),
        ("Icon", before.and_then(|s| s.icon.as_deref()), after.icon.as_deref()),
    ] {
        if old == new || (label == "Goal" && same_decimal(old, new)) {
            continue;
        }
        if !heading_written {
            let _ = writeln!(text, "\n<b>{}</b>", if before.is_some() { "Settings changes" } else { "Requested settings" });
            heading_written = true;
        }
        let _ = write!(text, "• {label}: ");
        if before.is_some() {
            let _ = write!(text, "{} → ", html(setting_value(label, old)));
        }
        let _ = writeln!(text, "<b>{}</b>", html(setting_value(label, new)));
    }
}

fn pie_allocation_notification(text: &mut impl Write, before: Option<&PieSettings>, after: &PieSettings) {
    let old_targets = before.and_then(|settings| settings.instrument_shares.as_ref());
    let Some(targets) = &after.instrument_shares else {
        let _ = text.write_str("\n<i>Target allocation unavailable.</i>\n");
        return;
    };
    let tickers = old_targets.into_iter().flat_map(|targets| targets.keys())
        .merge(targets.keys()).dedup();
    let mut count = 0;
    for ticker in tickers {
        let old = old_targets.and_then(|targets| targets.get(ticker));
        let new = targets.get(ticker);
        if old_targets.is_some() && old.zip(new).is_some_and(|(old, new)| same_decimal(old.as_deref(), new.as_deref())) {
            continue;
        }
        count += 1;
        if count == 1 {
            let _ = writeln!(text, "\n<b>{}</b>", if old_targets.is_some() { "Target allocation changes" } else { "Target allocation" });
        }
        if count > NOTIFICATION_ITEMS {
            continue;
        }
        let _ = write!(text, "• <code>{}</code>: ", html(ticker));
        match (old_targets, old, new) {
            (Some(_), Some(old), Some(new)) => {
                let _ = writeln!(text, "{} → <b>{}</b>", html(&allocation_weight(old.as_deref())), html(&allocation_weight(new.as_deref())));
            }
            (Some(_), None, Some(new)) => {
                let _ = writeln!(text, "<b>Added · {}</b>", html(&allocation_weight(new.as_deref())));
            }
            (Some(_), Some(old), None) => {
                let _ = writeln!(text, "{} → <b>Removed</b>", html(&allocation_weight(old.as_deref())));
            }
            (_, _, Some(new)) => {
                let _ = writeln!(text, "<b>{}</b>", html(&allocation_weight(new.as_deref())));
            }
            _ => unreachable!("a merged target is present in at least one allocation"),
        }
    }
    if count > NOTIFICATION_ITEMS {
        let _ = writeln!(text, "<i>… {} more allocation {} in Doughbox.</i>",
            count - NOTIFICATION_ITEMS, if old_targets.is_some() { "changes" } else { "targets" });
    } else if count == 0 && old_targets.is_some() {
        let _ = text.write_str("\n<i>Target allocation unchanged.</i>\n");
    }
}

fn setting_value<'a>(label: &str, value: Option<&'a str>) -> &'a str {
    match (label, value) {
        ("Dividends", Some("REINVEST")) => "Reinvest",
        ("Dividends", Some("TO_ACCOUNT_CASH")) => "Account cash",
        (_, Some(value)) => value,
        (_, None) => "Not supplied",
    }
}

fn decimal(value: &str) -> Option<Decimal> {
    Decimal::from_str_exact(value).or_else(|_| Decimal::from_scientific(value)).ok()
}

fn same_decimal(left: Option<&str>, right: Option<&str>) -> bool {
    left == right || matches!((left.and_then(decimal), right.and_then(decimal)), (Some(left), Some(right)) if left == right)
}

fn allocation_weight(value: Option<&str>) -> String {
    match value {
        Some(value) => match decimal(value).and_then(|weight| weight.checked_mul(Decimal::from(100))) {
            Some(percent) => format!("{}%", percent.normalize()),
            None => format!("{value} (raw weight)"),
        },
        None => "Unknown".into(),
    }
}

fn html(value: &str) -> Html<'_> {
    Html(value)
}

struct Html<'a>(&'a str);

impl fmt::Display for Html<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, character) in self.0.char_indices() {
            if character == '.' && self.0[..index].ends_with("www") {
                formatter.write_str("\u{200b}")?;
            }
            match character {
                '&' => formatter.write_str("&amp;")?,
                '<' => formatter.write_str("&lt;")?,
                '>' => formatter.write_str("&gt;")?,
                '"' => formatter.write_str("&quot;")?,
                _ => formatter.write_char(character)?,
            }
            if character == ':' && self.0[index..].starts_with("://") {
                formatter.write_str("\u{200b}")?;
            }
        }
        Ok(())
    }
}

fn content_lines(proposal: &Proposal) -> Vec<String> {
    match &proposal.content {
        ProposalContent::Trades { trades } => trades.iter().map(trade_line).collect(),
        ProposalContent::Pie { pie } => {
            let mut lines = vec![match &pie.mutation {
                PreparedPieMutation::Create { settings } => format!("Create Pie: {}. No funding.", settings.name),
                PreparedPieMutation::Update { pie_id, settings } => format!("Change Pie #{pie_id} settings: {}. No shares bought or sold.", settings.name),
                PreparedPieMutation::Duplicate { pie_id, name, .. } => format!("Duplicate composition of Pie #{pie_id} as {name}. No holdings, AutoInvest or goals copied; no funding."),
                PreparedPieMutation::Delete { pie_id } => format!("Permanently delete Pie #{pie_id}. Owner must confirm investments and Pie cash were emptied in Trading 212."),
            }];
            if let Some(id) = &pie.broker_pie_id {
                lines.push(format!("Broker destination Pie #{id}"));
            }
            if let Some(before) = &pie.before {
                lines.push(format!("Source Pie #{}", before.id));
                if let Some(settings) = &before.settings {
                    lines.extend(settings_lines("Before / source", settings));
                }
            }
            if let Some(settings) = &pie.expected_settings {
                lines.extend(settings_lines("After / intended", settings));
            }
            lines.push("Review the complete immutable diff in Doughbox. External broker apps can still change settings after preflight.".into());
            lines
        }
    }
}

fn settings_lines(label: &str, settings: &super::pies::PieSettings) -> Vec<String> {
    let mut lines = vec![format!("{label}: {}", settings.name.as_deref().unwrap_or("(name unavailable)"))];
    for (field, value) in [
        ("Dividends", &settings.dividend_cash_action), ("Icon", &settings.icon),
        ("Goal", &settings.goal), ("End date", &settings.end_date),
    ] {
        lines.push(format!("{field}: {}", value.as_deref().unwrap_or("(not supplied)")));
    }
    match &settings.instrument_shares {
        Some(targets) => lines.extend(targets.iter().map(|(ticker, weight)| format!("{ticker}: target weight {}", weight.as_deref().unwrap_or("unavailable")))),
        None => lines.push("Target allocation unavailable".into()),
    }
    lines
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
            broker_account: None,
            content: ProposalContent::Trades { trades: vec![TradeResult {
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
            }] },
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
        assert!(!text.contains("evil.example/note"));
    }

    #[test]
    fn telegram_html_escapes_agent_text_and_links_only_the_review_page() {
        let proposal = proposal("Click <a href=\"https://evil.example\">here</a>", "https://evil.example/note");
        let url = "https://doughbox.example.com/dashboard/trading?proposal=11111111-1111-4111-8111-111111111111";
        let text = proposal_notification(&proposal, Some(url));
        assert_eq!(text.matches("<a href=").count(), 1);
        assert!(text.contains(&format!("<a href=\"{url}\">")));
        assert!(text.contains("&lt;a href=&quot;https:\u{200b}//evil.example&quot;&gt;"));
        assert!(!text.contains("evil.example/note"));
        assert!(!text.contains("<script>"));
    }

    fn pie_proposal() -> Proposal {
        use serde_json::json;
        let mut proposal = proposal("Pie <review>", "");
        proposal.content = ProposalContent::Pie { pie: serde_json::from_value(json!({
            "mutation":{"action":"update","pie_id":"10","settings":{
                "name":"<a href=\"https://evil.example\">destination</a>","dividend_cash_action":"REINVEST",
                "instrument_shares":{"AAPL_US_EQ":"0.7000","MSFT_US_EQ":"0.3000"}
            }},
            "before":{"fetched_at":"2026-11-09T00:00:00Z","id":"10",
                "settings":{"name":"www.evil.example","icon":"<script>","dividend_cash_action":"REINVEST",
                "goal":null,"end_date":null,"instrument_shares":{"AAPL_US_EQ":"0.6","MSFT_US_EQ":"0.4"}},"instruments":[]},
            "expected_settings":{"name":"<a href=\"https://evil.example\">destination</a>","icon":null,
                "dividend_cash_action":"REINVEST","goal":null,"end_date":null,
                "instrument_shares":{"AAPL_US_EQ":"0.7000","MSFT_US_EQ":"0.3000"}},
            "status":"pending","broker_pie_id":null,"error":null,"submitted_at":null,"observation":null,
            "empty_confirmed_by_owner":false
        })).unwrap() };
        proposal
    }

    fn assert_deliverable_review_notification(text: &str, url: &str) {
        let tags = regex::Regex::new(r#"<(/?)(b|i|code|a)(?: href="[^"]*")?>"#).unwrap();
        let mut stack = Vec::new();
        let mut rendered = String::new();
        let mut offset = 0;
        for captures in tags.captures_iter(text) {
            let tag = captures.get(0).unwrap();
            rendered.push_str(&text[offset..tag.start()]);
            offset = tag.end();
            let name = captures.get(2).unwrap().as_str();
            if &captures[1] == "/" {
                assert_eq!(stack.pop(), Some(name), "Unbalanced notification HTML");
            } else {
                stack.push(name);
            }
        }
        rendered.push_str(&text[offset..]);
        assert!(stack.is_empty(), "Unclosed notification HTML");
        assert!(!rendered.contains(['<', '>']), "Malformed notification HTML");
        let rendered = rendered.replace("&lt;", "<").replace("&gt;", ">")
            .replace("&quot;", "\"").replace("&amp;", "&");
        assert!(rendered.encode_utf16().count() <= 4096, "Telegram message exceeds its rendered-text limit");
        assert_eq!(text.matches("<a href=").count(), 1);
        assert!(text.contains(&format!("<a href=\"{url}\">")));
    }

    #[test]
    fn oversized_pie_names_keep_a_deliverable_review_link() {
        use super::super::pies::{domain, DividendCashAction, PieCreateSettings, PieOperationInput};
        let mut proposal = proposal("Large Pie", "");
        let operation = PieOperationInput::Create { settings: PieCreateSettings {
            name: "x".repeat(5000),
            dividend_cash_action: DividendCashAction::REINVEST,
            instrument_shares: [("AAPL_US_EQ".into(), "1".into())].into(),
            icon: None, goal: None, end_date: None,
        } };
        proposal.content = ProposalContent::Pie { pie: domain::prepare(operation, None).unwrap() };
        let url = approval_url(&input(Some("https://doughbox.example"))).unwrap();
        assert_deliverable_review_notification(&proposal_notification(&proposal, Some(&url)), &url);
    }

    #[test]
    fn combined_unicode_settings_cannot_overflow_or_break_notification_html() {
        use super::super::pies::{domain, PieOperationInput, PieSettingsChanges};
        let mut proposal = pie_proposal();
        let ProposalContent::Pie { pie } = &mut proposal.content else { unreachable!() };
        let mut source = pie.before.take().unwrap();
        let settings = source.settings.as_mut().unwrap();
        settings.name = Some("A".repeat(1800));
        settings.icon = Some("🧪<&>\"".repeat(400));
        let operation = PieOperationInput::Update {
            pie_id: source.id.clone(),
            changes: PieSettingsChanges {
                name: Some("B".repeat(1800)),
                icon: Some("🚀<&>\"".repeat(400)),
                ..Default::default()
            },
        };
        proposal.content = ProposalContent::Pie { pie: domain::prepare(operation, Some(source)).unwrap() };
        let url = approval_url(&input(Some("https://doughbox.example"))).unwrap();
        assert_deliverable_review_notification(&proposal_notification(&proposal, Some(&url)), &url);
    }

    #[test]
    fn pie_updates_keep_changed_targets_visible_in_large_pies() {
        let mut proposal = pie_proposal();
        let ProposalContent::Pie { pie } = &mut proposal.content else { unreachable!() };
        let mut before: std::collections::BTreeMap<_, _> = (0..18)
            .map(|index| (format!("UNCHANGED_{index:02}_US_EQ"), Some("0.05".to_owned())))
            .collect();
        before.insert("Y_CHANGED_US_EQ".into(), Some("0.05".into()));
        before.insert("Z_CHANGED_US_EQ".into(), Some("0.05".into()));
        let mut after = before.clone();
        after.insert("Y_CHANGED_US_EQ".into(), Some("0.04".into()));
        after.insert("Z_CHANGED_US_EQ".into(), Some("0.06".into()));
        pie.before.as_mut().unwrap().settings.as_mut().unwrap().instrument_shares = Some(before);
        pie.expected_settings.as_mut().unwrap().instrument_shares = Some(after);

        let text = proposal_notification(&proposal, None);
        assert!(text.contains("Y_CHANGED_US_EQ"));
        assert!(text.contains("Z_CHANGED_US_EQ"));
        assert!(!text.contains("UNCHANGED_"));
        assert!(text.contains("4%"));
        assert!(text.contains("6%"));
    }

    #[test]
    fn pie_updates_distinguish_added_removed_and_numerically_unchanged_targets() {
        let mut proposal = pie_proposal();
        let ProposalContent::Pie { pie } = &mut proposal.content else { unreachable!() };
        pie.before.as_mut().unwrap().settings.as_mut().unwrap().instrument_shares = Some([
            ("ADJUST_US_EQ".into(), Some("0.5".into())),
            ("REMOVE_US_EQ".into(), Some("0.3".into())),
            ("UNCHANGED_US_EQ".into(), Some("0.2".into())),
        ].into());
        pie.expected_settings.as_mut().unwrap().instrument_shares = Some([
            ("ADJUST_US_EQ".into(), Some("0.4".into())),
            ("ADD_US_EQ".into(), Some("0.4".into())),
            ("UNCHANGED_US_EQ".into(), Some("0.2000".into())),
        ].into());

        let text = proposal_notification(&proposal, None);
        assert!(text.contains("ADJUST_US_EQ"));
        assert!(text.contains("50%"));
        assert!(text.contains("40%"));
        assert!(text.contains("ADD_US_EQ"));
        assert!(text.contains("Added"));
        assert!(text.contains("REMOVE_US_EQ"));
        assert!(text.contains("Removed"));
        assert!(!text.contains("UNCHANGED_US_EQ"));
    }

    #[test]
    fn pie_weights_display_exact_percentages_without_treating_unknown_as_zero() {
        for (value, expected) in [
            (Some("0.0001"), "0.01%"),
            (Some("0.3333"), "33.33%"),
            (Some("0.7000"), "70%"),
            (Some("1"), "100%"),
            (Some("1e-4"), "0.01%"),
            (Some("0.123456789123456789"), "12.3456789123456789%"),
            (None, "Unknown"),
            (Some("unavailable"), "unavailable (raw weight)"),
        ] {
            assert_eq!(allocation_weight(value), expected);
        }
    }

    #[test]
    fn missing_previous_allocation_is_not_reported_as_added_targets() {
        let mut proposal = pie_proposal();
        let ProposalContent::Pie { pie } = &mut proposal.content else { unreachable!() };
        pie.before.as_mut().unwrap().settings.as_mut().unwrap().instrument_shares = None;
        pie.expected_settings.as_mut().unwrap().instrument_shares.as_mut().unwrap()
            .insert("MSFT_US_EQ".into(), None);

        let text = proposal_notification(&proposal, None);
        assert!(text.contains("AAPL_US_EQ"));
        assert!(text.contains("70%"));
        assert!(text.contains("MSFT_US_EQ"));
        assert!(text.contains("Unknown"));
        assert!(!text.contains("Added"));
    }

    #[test]
    fn pie_summary_neutralizes_broker_settings_and_preserves_exact_targets() {
        let proposal = pie_proposal();
        let url = "https://doughbox.example.com/dashboard/trading?proposal=fixture";
        let text = proposal_review_text(&proposal, Some(url), None);
        assert!(text.contains("0.7000"));
        assert!(text.contains("0.3000"));
        assert!(!text.contains("https://evil.example"));
        assert!(!text.contains("www.evil.example"));
        let html = proposal_notification(&proposal, Some(url));
        assert_eq!(html.matches("<a href=").count(), 1);
        assert!(!html.contains("<script>"));
        assert!(html.contains("&lt;a href=&quot;https:\u{200b}//evil.example"));
    }
}
