use crate::models::*;
use crate::*;
use serde_json::json;

#[test]
fn iso_round_trip() {
    let t = Timestamp::new("2026-11-10T09:30:05.123Z");
    let ms = t.unix_ms().unwrap();
    assert_eq!(Timestamp::from_unix_ms(ms), t);
    assert_eq!(parse_iso_ms("1970-01-01T00:00:00.000Z"), Some(0));
    assert_eq!(parse_iso_ms("1970-01-01T01:00:00+01:00"), Some(0));
}

#[test]
fn uuid_lowercases() {
    let u: Uuid = serde_json::from_value(json!("0190ABCD-0000-7000-8000-000000000000")).unwrap();
    assert_eq!(u.as_str(), "0190abcd-0000-7000-8000-000000000000");
    assert!(Uuid::is_shape(&u));
}

#[test]
fn string_enum_keeps_unknown() {
    let t: ChannelType = serde_json::from_value(json!("direct_message")).unwrap();
    assert_eq!(t, ChannelType::DirectMessage);
    let u: ChannelType = serde_json::from_value(json!("voice")).unwrap();
    assert_eq!(u, ChannelType::Unknown("voice".into()));
    assert_eq!(serde_json::to_value(&u).unwrap(), json!("voice"));
}

#[test]
fn server_frames_are_tagged_by_event() {
    let ack: ServerFrame = serde_json::from_value(json!({"event":"ACK","data":{"id":"0190abcd-0000-7000-8000-000000000001"},"ts":1.5,"refId":"0190abcd-0000-7000-8000-000000000001"})).unwrap();
    assert!(matches!(ack, ServerFrame::Ack(_)), "{ack:?}");
    let sub: ServerFrame = serde_json::from_value(json!({"event":"ACK","data":{"channels":[{"channelId":"0190abcd-0000-7000-8000-000000000002","nextCursor":null}]},"ts":1.0})).unwrap();
    assert!(matches!(sub, ServerFrame::Subscribed(_)), "{sub:?}");
    let unsub: ServerFrame = serde_json::from_value(json!({"event":"ACK","data":{"channelIds":[]},"ts":1.0})).unwrap();
    assert!(matches!(unsub, ServerFrame::Unsubscribed(_)), "{unsub:?}");
    let unknown: ServerFrame = serde_json::from_value(json!({"event":"SOMETHING_NEW","data":{},"ts":1.0})).unwrap();
    assert!(matches!(unknown, ServerFrame::Unknown(_)));
    let pong: ServerFrame = serde_json::from_value(json!({"event":"PONG","data":{},"ts":1.0})).unwrap();
    assert!(matches!(pong, ServerFrame::Pong(_)));
    // A known event with a wrong shape is malformed (the connection drops it).
    assert!(serde_json::from_value::<ServerFrame>(json!({"event":"MESSAGE_CREATED","data":{},"ts":1.0})).is_err());
}

#[test]
fn client_frames_carry_their_op() {
    let f = ClientFrame::Ping(PingFrame { id: Uuid::new("0190abcd-0000-7000-8000-000000000003"), payload: EmptyObject {} });
    let v = serde_json::to_value(&f).unwrap();
    assert_eq!(v["op"], "PING");
    assert_eq!(v["payload"], json!({}));
}

#[test]
fn error_body_reads_any_code() {
    let e: ErrorResponse = serde_json::from_value(json!({"error":{"code":"CHANNEL_NOT_FOUND","message":"x"}})).unwrap();
    assert_eq!(e.error.code, ErrorCode::ChannelNotFound);
    assert_eq!(constants::error_status("CHANNEL_NOT_FOUND"), Some(404));
}

#[test]
fn double_option_distinguishes_clear_from_leave() {
    let leave = UpdateIssueRequest { title: Some("t".into()), ..Default::default() };
    assert_eq!(serde_json::to_value(&leave).unwrap(), json!({"title":"t"}));
    let clear = UpdateIssueRequest { assignee_id: Some(None), ..Default::default() };
    assert_eq!(serde_json::to_value(&clear).unwrap(), json!({"assigneeId": null}));
}

#[test]
fn deep_work_request_union() {
    let on = SetDeepWorkRequest::ActiveOn(SetDeepWorkRequestActiveOn { until: Timestamp::new("2026-11-10T10:00:00.000Z") });
    assert_eq!(serde_json::to_value(&on).unwrap(), json!({"active": true, "until": "2026-11-10T10:00:00.000Z"}));
    let off = SetDeepWorkRequest::ActiveOff(SetDeepWorkRequestActiveOff {});
    assert_eq!(serde_json::to_value(&off).unwrap(), json!({"active": false}));
}
