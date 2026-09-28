//! Answering meeting invitations through Microsoft Graph: the invitation
//! message (an `eventMessage`) links to the event on the user's calendar,
//! and `accept` / `tentativelyAccept` / `decline` on that event tell the
//! organizer, with an optional comment and, for tentative or decline, a
//! `proposedNewTime`. Needs `Calendars.ReadWrite` (Graph answers 403
//! without it, and the app then answers by email).

use penguin_provider::{Error, InvitationAnswer, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::client::{enc, GraphClient};
use crate::convert::format_date;
use crate::http::{Body, Method, Retry};

/// The event action for a response.
pub(crate) fn action(response: &str) -> Result<&'static str> {
    Ok(match response {
        "accepted" => "accept",
        "tentative" => "tentativelyAccept",
        "declined" => "decline",
        other => return Err(Error::InvalidInput(format!("unknown response {other}"))),
    })
}

/// The action's body. Graph accepts `proposedNewTime` only with
/// tentativelyAccept and decline, and only with `sendResponse: true`.
pub(crate) fn action_body(answer: &InvitationAnswer) -> Result<Value> {
    let mut body = json!({ "sendResponse": true });
    if let Some(c) = answer
        .comment
        .as_deref()
        .map(str::trim)
        .filter(|c| !c.is_empty())
    {
        body["comment"] = Value::String(c.to_string());
    }
    if let Some((start, end)) = answer.proposal {
        if answer.response == "accepted" {
            return Err(Error::InvalidInput(
                "a new time can be proposed with Maybe or No, not Yes".into(),
            ));
        }
        if end <= start {
            return Err(Error::InvalidInput(
                "the proposed time must end after it starts".into(),
            ));
        }
        let t = |ms: i64| json!({ "dateTime": format_date(ms).trim_end_matches('Z'), "timeZone": "UTC" });
        body["proposedNewTime"] = json!({ "start": t(start), "end": t(end) });
    }
    Ok(body)
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct EventRef {
    id: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct EventMessage {
    event: Option<EventRef>,
}

impl GraphClient {
    pub(crate) async fn respond_to_invitation(
        &self,
        message_id: &str,
        answer: &InvitationAnswer,
    ) -> Result<()> {
        let action = action(&answer.response)?;
        let body = action_body(answer)?;
        let m: EventMessage = self
            .api
            .get_json(
                &format!("/me/messages/{}", enc(message_id)),
                &[
                    ("$select", "id".into()),
                    (
                        "$expand",
                        "microsoft.graph.eventMessage/event($select=id)".into(),
                    ),
                ],
                &[],
                "eventMessage",
            )
            .await?;
        let event = m
            .event
            .map(|e| e.id)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| {
                Error::NotFound("this message isn't linked to an event on your calendar".into())
            })?;
        self.api
            .call(
                Method::Post,
                &format!("/me/events/{}/{action}", enc(&event)),
                &[],
                Body::Json(body),
                &[],
                Retry::OnlyIfRejected,
            )
            .await?;
        tracing::info!(account = %self.account_id(), action, "invitation answered through Graph");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(response: &str) -> InvitationAnswer {
        InvitationAnswer {
            response: response.into(),
            comment: None,
            proposal: None,
        }
    }

    #[test]
    fn actions_and_bodies() {
        assert_eq!(action("accepted").unwrap(), "accept");
        assert_eq!(action("tentative").unwrap(), "tentativelyAccept");
        assert_eq!(action("declined").unwrap(), "decline");
        assert!(action("maybe").is_err());

        assert_eq!(
            action_body(&answer("accepted")).unwrap(),
            json!({"sendResponse": true})
        );
        let mut a = answer("tentative");
        a.comment = Some("  Can we move it? ".into());
        a.proposal = Some((1_790_000_000_000, 1_790_003_600_000));
        assert_eq!(
            action_body(&a).unwrap(),
            json!({
                "sendResponse": true,
                "comment": "Can we move it?",
                "proposedNewTime": {
                    "start": {"dateTime": "2026-09-21T14:13:20", "timeZone": "UTC"},
                    "end": {"dateTime": "2026-09-21T15:13:20", "timeZone": "UTC"}
                }
            })
        );
        // Graph refuses a proposal with accept; so does Penguin, up front.
        a.response = "accepted".into();
        assert!(action_body(&a).is_err());
        a.response = "declined".into();
        a.proposal = Some((10, 10));
        assert!(action_body(&a).is_err());
    }
}
