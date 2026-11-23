//! Message-waiting (RFC 3842) subscription for an account's own mailbox.
//!
//! The Aria PBX pushes an unsolicited message-summary NOTIFY only when a
//! voicemail is left, read or deleted. Without a subscription, a message that
//! arrived while Aria was closed showed no indicator until the next change.
//! Subscribing after each registration makes the server answer with the
//! current counts straight away; the subscription refresh timer then keeps
//! it alive like any other.

use std::sync::Arc;
use tokio::sync::RwLock;

use crate::sip::builder::{self, build_subscribe};
use crate::sip::presence::{EventType, Subscription, SubscriptionState};
use crate::sip::state::AccountState;
use crate::sip::{ManagerState, RegistrationState};

/// How long we ask the server to keep the MWI subscription.
const MWI_EXPIRES: u32 = 3600;

/// The account's own address of record: the mailbox the server reports on.
pub fn mailbox_uri(username: &str, domain: &str) -> String {
    format!("sip:{username}@{domain}")
}

/// What to do about MWI after a registration succeeded.
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    /// A subscription is already pending or active; the refresh timer has it.
    AlreadySubscribed,
    /// The server refused message-summary for this account earlier.
    Refused,
    Subscribe,
}

fn plan(subscriptions: &[Subscription], refused: bool) -> Plan {
    if refused {
        return Plan::Refused;
    }
    let live = subscriptions.iter().any(|s| {
        s.event_type == EventType::MessageSummary && s.state != SubscriptionState::Terminated
    });
    if live {
        Plan::AlreadySubscribed
    } else {
        Plan::Subscribe
    }
}

/// Subscribe the account to its own message summary, unless it already is.
///
/// Called on every successful REGISTER (initial, refresh and reconnect). The
/// saved re-subscription list is purged of message-summary entries first:
/// those are re-created here, and the frontend's generic re-subscribe (which
/// always targets the active account) must not duplicate them.
pub async fn subscribe_after_registration(
    state: &Arc<RwLock<ManagerState>>,
    account_id: &str,
) -> Result<Option<String>, String> {
    let (config, local_addr, server_addr, transport, sub) = {
        let mut s = state.write().await;
        let account = s.get_account_mut(account_id).ok_or("Account not found")?;
        let refused = account.mwi_refused;
        account
            .pending_resubscriptions
            .retain(|(_, event)| *event != EventType::MessageSummary);
        if account.registration.status() != RegistrationState::Registered {
            return Err("Not registered".into());
        }
        if plan(&account.subscriptions, refused) != Plan::Subscribe {
            return Ok(None);
        }
        let local_addr = account.local_addr.ok_or("No local addr")?;
        let server_addr = account.server_addr.ok_or("No server addr")?;
        let transport = account.transport.clone().ok_or("No transport")?;
        let sub = Subscription {
            id: uuid::Uuid::new_v4().to_string(),
            target_uri: mailbox_uri(&account.config.username, &account.config.domain),
            event_type: EventType::MessageSummary,
            state: SubscriptionState::Pending,
            expires: MWI_EXPIRES,
            cseq: 1,
            call_id: builder::generate_call_id(),
            from_tag: builder::generate_tag(),
            to_tag: None,
            auth_attempts: 0,
            expires_at: None,
        };
        // On the list before the request goes out, so a fast challenge finds it.
        account.subscriptions.push(sub.clone());
        (account.config.clone(), local_addr, server_addr, transport, sub)
    };

    let msg = build_subscribe(
        &config,
        &sub.target_uri,
        local_addr,
        &sub.call_id,
        sub.cseq,
        &sub.from_tag,
        None,
        &sub.event_type,
        sub.expires,
        None,
    );
    // Resent until answered; dropped (so the next registration tries again)
    // if never answered.
    if let Err(e) = crate::sip::send_subscribe(state, account_id, &sub.call_id, &transport, server_addr, msg).await {
        let mut s = state.write().await;
        if let Some(account) = s.get_account_mut(account_id) {
            account.subscriptions.retain(|x| x.id != sub.id);
        }
        return Err(e);
    }
    log::info!("Sent message-summary SUBSCRIBE for {}", sub.target_uri);
    Ok(Some(sub.id))
}

/// A final failure for a SUBSCRIBE: remember a refused message-summary so the
/// next REGISTER refresh does not ask again.
pub fn note_subscribe_failure(account: &mut AccountState, call_id: &str) {
    let is_mwi = account
        .subscriptions
        .iter()
        .any(|s| s.call_id == call_id && s.event_type == EventType::MessageSummary);
    if is_mwi {
        log::warn!("Server refused message-summary for {}; not asking again", account.config.id);
        account.mwi_refused = true;
    }
}

/// Track the subscription state a message-summary NOTIFY carries (RFC 6665
/// §4.1.3), renewing on the reasons that mean "subscribe again" and dropping
/// it on the others. Unsolicited NOTIFYs (no matching dialog) are ignored.
pub async fn track_notify_state(state: &Arc<RwLock<ManagerState>>, account_id: &str, text: &str) {
    let Some(call_id) = builder::extract_header(text, "Call-ID") else {
        return;
    };
    let header = builder::extract_header(text, "Subscription-State").unwrap_or_default();
    let mut s = state.write().await;
    let Some(account) = s.get_account_mut(account_id) else {
        return;
    };
    let Some(sub) = account.subscriptions.iter_mut().find(|x| x.call_id == call_id) else {
        return;
    };
    match subscription_state_action(&header) {
        StateAction::Active => sub.state = SubscriptionState::Active,
        StateAction::Pending => sub.state = SubscriptionState::Pending,
        StateAction::Renew => *sub = sub.renewed(),
        StateAction::Drop => account.subscriptions.retain(|x| x.call_id != call_id),
        StateAction::None => {}
    }
}

#[derive(Debug, PartialEq, Eq)]
enum StateAction {
    Active,
    Pending,
    Renew,
    Drop,
    None,
}

fn subscription_state_action(header: &str) -> StateAction {
    let mut parts = header.split(';');
    let value = parts.next().unwrap_or("").trim().to_lowercase();
    match value.as_str() {
        "active" => StateAction::Active,
        "pending" => StateAction::Pending,
        "terminated" => {
            let reason = parts.find_map(|p| p.trim().strip_prefix("reason=").map(|r| r.trim().to_lowercase()));
            match reason.as_deref() {
                Some("timeout" | "deactivated" | "probation" | "giveup") => StateAction::Renew,
                _ => StateAction::Drop,
            }
        }
        _ => StateAction::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(event_type: EventType, state: SubscriptionState) -> Subscription {
        Subscription {
            id: "x".into(),
            target_uri: "sip:1001@pbx.example".into(),
            event_type,
            state,
            expires: 3600,
            cseq: 1,
            call_id: "c".into(),
            from_tag: "f".into(),
            to_tag: None,
            auth_attempts: 0,
            expires_at: None,
        }
    }

    #[test]
    fn mailbox_is_the_accounts_own_aor() {
        assert_eq!(mailbox_uri("1001", "pbx.example"), "sip:1001@pbx.example");
    }

    #[test]
    fn subscribes_when_nothing_is_live() {
        assert_eq!(plan(&[], false), Plan::Subscribe);
        // A BLF subscription to the same extension is not an MWI one.
        let blf = sub(EventType::Dialog, SubscriptionState::Active);
        assert_eq!(plan(&[blf], false), Plan::Subscribe);
        // A terminated one is about to be dropped; make a new one.
        let dead = sub(EventType::MessageSummary, SubscriptionState::Terminated);
        assert_eq!(plan(&[dead], false), Plan::Subscribe);
    }

    #[test]
    fn refresh_registers_do_not_duplicate() {
        let pending = sub(EventType::MessageSummary, SubscriptionState::Pending);
        assert_eq!(plan(&[pending], false), Plan::AlreadySubscribed);
        let active = sub(EventType::MessageSummary, SubscriptionState::Active);
        assert_eq!(plan(&[active], false), Plan::AlreadySubscribed);
    }

    #[test]
    fn a_refusal_sticks() {
        assert_eq!(plan(&[], true), Plan::Refused);
    }

    #[test]
    fn subscription_state_reasons() {
        assert_eq!(subscription_state_action("active;expires=3600"), StateAction::Active);
        assert_eq!(subscription_state_action("pending"), StateAction::Pending);
        assert_eq!(subscription_state_action("terminated;reason=timeout"), StateAction::Renew);
        assert_eq!(subscription_state_action("terminated;reason=noresource"), StateAction::Drop);
        assert_eq!(subscription_state_action("terminated"), StateAction::Drop);
        assert_eq!(subscription_state_action(""), StateAction::None);
    }

    #[test]
    fn subscribe_request_asks_for_message_summary() {
        let config = crate::sip::account::AccountConfig {
            id: "a".into(),
            display_name: String::new(),
            username: "1001".into(),
            domain: "pbx.example".into(),
            password: String::new(),
            transport: crate::sip::TransportType::Udp,
            port: 5060,
            registrar: None,
            outbound_proxy: None,
            auth_username: None,
            auth_realm: None,
            tls_server_name: None,
            enabled: true,
            auto_record: false,
            tls_insecure: false,
            srtp_mode: Default::default(),
            codecs: crate::sip::account::default_codec_preferences(),
        };
        let msg = build_subscribe(
            &config,
            &mailbox_uri("1001", "pbx.example"),
            "192.0.2.10:5060".parse().unwrap(),
            "cid",
            1,
            "ftag",
            None,
            &EventType::MessageSummary,
            MWI_EXPIRES,
            None,
        );
        assert!(msg.starts_with("SUBSCRIBE sip:1001@pbx.example SIP/2.0\r\n"), "{msg}");
        assert!(msg.contains("Event: message-summary\r\n"));
        assert!(msg.contains("Accept: application/simple-message-summary\r\n"));
        assert!(msg.contains("To: <sip:1001@pbx.example>\r\n"));
        assert!(msg.contains("Expires: 3600\r\n"));
    }
}
