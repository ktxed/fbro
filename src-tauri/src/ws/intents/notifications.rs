use serde_json::Value;

use super::arg_str;
use crate::notifications::NotificationInput;
use crate::ws::protocol::{now, ServerMessage};
use crate::ws::runtime::IntentContext;

pub async fn handle(name: &str, args: &Value, ctx: &mut IntentContext<'_>) {
    let service = ctx.state.lock().await.notifications.clone();
    let message = apply_intent(name, args, &service);
    let _ = ctx.tx.send(message).await;
}

fn apply_intent(
    name: &str,
    args: &Value,
    service: &crate::notifications::NotificationService,
) -> ServerMessage {
    let request_id = arg_str(args, "requestId").unwrap_or_default().to_string();
    let result = if request_id.trim().is_empty() {
        Err("A notification requestId is required".to_string())
    } else if name == "PublishNotification" {
        parse_input(args).map(|input| {
            let notification = service.publish_request(&request_id, input);
            ServerMessage::PublishNotificationOk {
                timestamp: now(),
                request_id: request_id.clone(),
                notification_id: notification.id,
            }
        })
    } else {
        service.clear();
        Ok(ServerMessage::ClearNotificationsOk {
            timestamp: now(),
            request_id: request_id.clone(),
        })
    };
    result.unwrap_or_else(|message| {
        if name == "PublishNotification" {
            ServerMessage::PublishNotificationFailed {
                timestamp: now(),
                request_id,
                code: "INVALID_NOTIFICATION".into(),
                message,
            }
        } else {
            ServerMessage::ClearNotificationsFailed {
                timestamp: now(),
                request_id,
                code: "INVALID_NOTIFICATION_REQUEST".into(),
                message,
            }
        }
    })
}

fn parse_input(args: &Value) -> Result<NotificationInput, String> {
    let message = arg_str(args, "message")
        .filter(|message| !message.trim().is_empty())
        .ok_or_else(|| "Notification message must be a nonempty string".to_string())?;
    let level = args
        .get("level")
        .map(|value| {
            serde_json::from_value(value.clone())
                .map_err(|_| "Notification level must be info, warning, or error".to_string())
        })
        .transpose()?;
    let channels = args
        .get("channels")
        .map(|value| {
            if value
                .as_array()
                .is_some_and(|channels| channels.iter().any(|channel| channel == "os"))
            {
                return Err("OS delivery is selected by backend operations".to_string());
            }
            serde_json::from_value(value.clone())
                .map_err(|_| "Notification channels must be an array of toast or log".to_string())
        })
        .transpose()?;
    Ok(NotificationInput {
        message: message.to_string(),
        level,
        channels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notifications::{NotificationChannel, NotificationLevel};

    #[test]
    fn notifications_publish_intent_preserves_defaults_and_explicit_empty_channels() {
        let input = parse_input(&serde_json::json!({"message": "copied"})).unwrap();
        assert_eq!(input.message, "copied");
        assert_eq!(input.level, None);
        assert_eq!(input.channels, None);
        let input = parse_input(
            &serde_json::json!({"message": "diagnostic", "level": "warning", "channels": []}),
        )
        .unwrap();
        assert_eq!(input.level, Some(NotificationLevel::Warning));
        assert_eq!(input.channels, Some(vec![]));
        let input =
            parse_input(&serde_json::json!({"message": "log", "channels": ["log"]})).unwrap();
        assert_eq!(input.channels, Some(vec![NotificationChannel::Log]));
    }

    #[test]
    fn notifications_publish_intent_rejects_malformed_values() {
        for args in [
            serde_json::json!({}),
            serde_json::json!({"message": "   "}),
            serde_json::json!({"message": 5}),
            serde_json::json!({"message": "x", "level": "critical"}),
            serde_json::json!({"message": "x", "channels": "toast"}),
            serde_json::json!({"message": "x", "channels": ["system"]}),
            serde_json::json!({"message": "x", "channels": ["os"]}),
            serde_json::json!({"message": "x", "channels": ["toast", "os"]}),
        ] {
            assert!(parse_input(&args).is_err(), "{args}");
        }
    }

    #[test]
    fn notifications_frontend_retry_receives_same_canonical_id_and_one_history_record() {
        let service = crate::notifications::NotificationService::default();
        let args = serde_json::json!({"requestId": "retry-1", "message": "copied", "channels": []});
        let first = apply_intent("PublishNotification", &args, &service);
        let second = apply_intent("PublishNotification", &args, &service);
        let first = serde_json::to_value(first).unwrap();
        let second = serde_json::to_value(second).unwrap();
        assert_eq!(first["type"], "PublishNotificationOk");
        assert_eq!(first["requestId"], "retry-1");
        assert_eq!(first["notificationId"], second["notificationId"]);
        assert_eq!(service.snapshot().entries.len(), 1);
        assert_eq!(service.snapshot().version, 1);
    }

    #[test]
    fn notifications_invalid_clear_preserves_history_and_valid_clear_is_correlated() {
        let service = crate::notifications::NotificationService::default();
        service.publish(NotificationInput {
            message: "history".into(),
            channels: Some(vec![]),
            ..Default::default()
        });
        let invalid = serde_json::to_value(apply_intent(
            "ClearNotifications",
            &serde_json::json!({}),
            &service,
        ))
        .unwrap();
        assert_eq!(invalid["type"], "ClearNotificationsFailed");
        assert_eq!(service.snapshot().entries.len(), 1);
        let valid = serde_json::to_value(apply_intent(
            "ClearNotifications",
            &serde_json::json!({"requestId": "clear-1"}),
            &service,
        ))
        .unwrap();
        assert_eq!(valid["type"], "ClearNotificationsOk");
        assert_eq!(valid["requestId"], "clear-1");
        assert!(service.snapshot().entries.is_empty());
    }
}
