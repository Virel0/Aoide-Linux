//! Port of `SyncWireTests.swift` ("Sync wire format"), plus the payload-depth rule the
//! contract added after it.

use serde_json::{Value, json};

use super::{make_playlist, payload, store, text};
use crate::{
    Millis, Op, OpKind, PullCursor, PullResponse, PushCursor, PushRequest, PushResponse,
    QueueState, RejectedOp, SyncFault, SyncPolicy, WireOp, canonical_json, new_id,
};

fn op_with(
    entity: &str,
    entity_id: &str,
    operation: OpKind,
    body: Option<String>,
    op_id: &str,
    created_at: Millis,
) -> Op {
    let body = body.unwrap_or_else(|| {
        text(&payload(&make_playlist(
            entity_id,
            "Late Night",
            created_at,
            false,
            "phone",
        )))
    });
    Op::new(entity, entity_id, operation, body, created_at).with_id(op_id)
}

fn op() -> Op {
    op_with(
        "playlist",
        "list-1",
        OpKind::Upsert,
        None,
        &new_id(),
        1_700_000_000_000,
    )
}

fn with_payload(body: &str) -> Op {
    op_with(
        "playlist",
        "list-1",
        OpKind::Upsert,
        Some(body.to_owned()),
        &new_id(),
        1_700_000_000_000,
    )
}

fn fault(op: &Op) -> Option<SyncFault> {
    WireOp::make(op).err()
}

// MARK: - Translation

#[test]
fn a_valid_op_converts_with_its_entity_translated() {
    let wire = WireOp::make(&op()).unwrap();
    assert_eq!(wire.entity, "playlists");
    assert_eq!(wire.operation, "upsert");
    assert_eq!(wire.entity_id, "list-1");
    assert_eq!(wire.payload["name"], "Late Night");
}

#[test]
fn queue_state_converts_identified_by_its_device() {
    let state = QueueState::new("phone", "iPhone", &["a", "b"], 0, 0, 1_000);
    let wire = WireOp::make(&Op::new(
        "queueState",
        "phone",
        OpKind::Upsert,
        text(&payload(&state)),
        1_000,
    ))
    .unwrap();
    assert_eq!(wire.entity, "queue_state");
    assert_eq!(wire.entity_id, "phone");
}

/// Unknown keys ride along untouched, so adding one needs no server deploy.
#[test]
fn an_unrecognised_column_in_the_payload_is_carried_not_refused() {
    let mut fields = payload(&make_playlist("list-1", "Late Night", 1, false, "mac"));
    fields["colourTheme"] = json!("dusk");
    let wire = WireOp::make(&with_payload(&text(&fields))).unwrap();
    assert_eq!(wire.payload["colourTheme"], "dusk");
}

/// The payload is opaque: a key is not re-cased on the way out.
#[test]
fn payload_keys_leave_exactly_as_they_were_written() {
    let mut fields = payload(&make_playlist("list-1", "Late Night", 1, false, "mac"));
    fields["Notes"] = json!("capitalised by somebody else");
    let wire = WireOp::make(&with_payload(&text(&fields))).unwrap();
    assert_eq!(wire.payload["Notes"], "capitalised by somebody else");
    assert!(wire.payload.get("notes").is_none());
    let request = serde_json::to_string(&PushRequest {
        device_id: "phone".into(),
        ops: vec![wire],
    })
    .unwrap();
    assert!(request.contains(r#""Notes":"capitalised by somebody else""#));
}

// MARK: - Refusals

#[test]
fn an_op_for_the_track_cache_can_never_reach_the_wire() {
    let raw = op_with(
        "track",
        "list-1",
        OpKind::Upsert,
        None,
        &new_id(),
        1_700_000_000_000,
    );
    assert_eq!(fault(&raw), Some(SyncFault::UnknownEntity("track".into())));
}

#[test]
fn an_entity_this_build_does_not_know_is_refused() {
    let raw = op_with(
        "download",
        "list-1",
        OpKind::Upsert,
        None,
        &new_id(),
        1_700_000_000_000,
    );
    assert_eq!(
        fault(&raw),
        Some(SyncFault::UnknownEntity("download".into()))
    );
}

#[test]
fn an_operation_the_server_would_not_match_is_refused() {
    let mut raw = op();
    raw.operation = "UPSERT".into();
    assert_eq!(
        fault(&raw),
        Some(SyncFault::UnknownOperation("UPSERT".into()))
    );
}

#[test]
fn an_entity_id_that_disagrees_with_the_payload_is_refused() {
    let mut mismatched = op();
    mismatched.entity_id = "list-2".into();
    assert_eq!(
        fault(&mismatched),
        Some(SyncFault::IdentityMismatch {
            entity_id: "list-2".into(),
            payload_id: Some("list-1".into())
        })
    );
}

#[test]
fn a_payload_with_no_identity_field_at_all_is_refused() {
    assert_eq!(
        fault(&with_payload(r#"{"name":"Late Night"}"#)),
        Some(SyncFault::IdentityMismatch {
            entity_id: "list-1".into(),
            payload_id: None
        })
    );
}

#[test]
fn a_payload_that_is_not_a_json_object_is_refused() {
    for body in ["[1,2,3]", "\"a string\"", "not json at all", ""] {
        assert_eq!(
            fault(&with_payload(body)),
            Some(SyncFault::PayloadNotAnObject),
            "{body}"
        );
    }
}

#[test]
fn a_payload_over_the_servers_limit_is_refused_before_it_is_sent() {
    let mut list = make_playlist("list-1", "Late Night", 1, false, "phone");
    list.notes = Some("x".repeat(300 * 1024));
    let raw = with_payload(&text(&payload(&list)));
    let fault = fault(&raw).unwrap();
    assert!(fault.reason().contains("payload is"));
    let SyncFault::PayloadTooLarge { bytes } = fault else {
        panic!("expected an oversized payload")
    };
    assert!(bytes > SyncPolicy::MAX_PAYLOAD_BYTES);
}

#[test]
fn ids_outside_the_servers_bounds_are_refused() {
    let created = 1_700_000_000_000;
    assert_eq!(
        fault(&op_with(
            "playlist",
            "list-1",
            OpKind::Upsert,
            None,
            "",
            created
        )),
        Some(SyncFault::EmptyOpId)
    );
    assert_eq!(
        fault(&op_with(
            "playlist",
            "list-1",
            OpKind::Upsert,
            None,
            &"a".repeat(129),
            created
        )),
        Some(SyncFault::OpIdTooLong(129))
    );
    assert_eq!(
        fault(&op_with(
            "playlist",
            "",
            OpKind::Upsert,
            None,
            &new_id(),
            created
        )),
        Some(SyncFault::EmptyEntityId)
    );
    let mut raw = op();
    raw.entity_id = "b".repeat(257);
    assert_eq!(fault(&raw), Some(SyncFault::EntityIdTooLong(257)));
}

#[test]
fn a_timestamp_the_server_would_not_accept_is_refused() {
    assert_eq!(
        fault(&op_with(
            "playlist",
            "list-1",
            OpKind::Upsert,
            None,
            &new_id(),
            0
        )),
        Some(SyncFault::NonPositiveTimestamp(0))
    );
    assert_eq!(
        fault(&op_with(
            "playlist",
            "list-1",
            OpKind::Upsert,
            None,
            &new_id(),
            -5
        )),
        Some(SyncFault::NonPositiveTimestamp(-5))
    );
}

#[test]
fn a_delete_must_carry_the_flag_in_its_payload() {
    let alive = text(&payload(&make_playlist(
        "list-1",
        "Late Night",
        1,
        false,
        "phone",
    )));
    let raw = op_with(
        "playlist",
        "list-1",
        OpKind::Delete,
        Some(alive),
        &new_id(),
        1_700_000_000_000,
    );
    assert_eq!(fault(&raw), Some(SyncFault::DeleteWithoutFlag));

    let dead = text(&payload(&make_playlist(
        "list-1",
        "Late Night",
        1,
        true,
        "phone",
    )));
    let raw = op_with(
        "playlist",
        "list-1",
        OpKind::Delete,
        Some(dead),
        &new_id(),
        1_700_000_000_000,
    );
    assert_eq!(fault(&raw), None);
}

#[test]
fn the_delete_flag_reads_as_set_whether_it_is_spelled_true_or_1() {
    for spelling in ["true", "1"] {
        let body = format!(
            r#"{{"deleted":{spelling},"id":"list-1","name":"x","originDevice":"p","sortIndex":"V","updatedAt":1}}"#
        );
        let raw = op_with(
            "playlist",
            "list-1",
            OpKind::Delete,
            Some(body),
            &new_id(),
            1_700_000_000_000,
        );
        assert_eq!(fault(&raw), None, "{spelling}");
    }
    let unset = r#"{"deleted":0,"id":"list-1","name":"x","originDevice":"p","sortIndex":"V","updatedAt":1}"#;
    let raw = op_with(
        "playlist",
        "list-1",
        OpKind::Delete,
        Some(unset.into()),
        &new_id(),
        1_700_000_000_000,
    );
    assert_eq!(fault(&raw), Some(SyncFault::DeleteWithoutFlag));
}

/// The contract's per-op ceiling is 32 levels. Refused here, an op that deep cannot
/// reach the server's whole-batch parser limit and take its batch-mates down with it.
#[test]
fn a_payload_nested_past_the_servers_limit_is_refused_before_it_is_sent() {
    let nest = |levels: usize| {
        let mut inner = json!("leaf");
        for _ in 0..levels.saturating_sub(1) {
            inner = json!({ "deeper": inner });
        }
        let mut fields = payload(&make_playlist("list-1", "Late Night", 1, false, "phone"));
        fields["rules"] = inner;
        text(&fields)
    };
    // The payload object itself is the first level, so `nest(n)` is n deep in total.
    assert_eq!(fault(&with_payload(&nest(32))), None);
    assert_eq!(
        fault(&with_payload(&nest(33))),
        Some(SyncFault::PayloadTooDeep { depth: 33 })
    );
}

// MARK: - Envelopes

#[test]
fn a_push_request_encodes_the_shape_the_contract_documents() {
    let wire = WireOp::make(&op_with(
        "playlist",
        "list-1",
        OpKind::Upsert,
        None,
        "op-1",
        1_700_000_000_000,
    ))
    .unwrap();
    let json = canonical_json(
        &serde_json::to_value(PushRequest {
            device_id: "phone".into(),
            ops: vec![wire],
        })
        .unwrap(),
    );
    assert!(json.contains(r#""deviceId":"phone""#));
    assert!(json.contains(r#""opId":"op-1""#));
    assert!(json.contains(r#""entity":"playlists""#));
    assert!(json.contains(r#""operation":"upsert""#));
    assert!(json.contains(r#""createdAt":1700000000000"#));
    // The payload is an object, not the string it is stored as.
    assert!(json.contains(r#""payload":{"#));
}

#[test]
fn a_push_response_decodes_rejections_and_all() {
    let json =
        r#"{"accepted":["a","b"],"rejected":[{"opId":"c","reason":"bad entity"}],"cursor":12345}"#;
    let response: PushResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.accepted, ["a", "b"]);
    assert_eq!(
        response.rejected,
        [RejectedOp {
            op_id: "c".into(),
            reason: "bad entity".into()
        }]
    );
    assert_eq!(response.cursor, Some(PushCursor(12345)));
}

#[test]
fn a_push_response_survives_a_rejected_list_in_an_unexpected_shape() {
    let missing: PushResponse = serde_json::from_str(r#"{"accepted":["a"],"cursor":7}"#).unwrap();
    assert!(missing.rejected.is_empty());
    let bare: PushResponse =
        serde_json::from_str(r#"{"accepted":[],"rejected":["c"],"cursor":7}"#).unwrap();
    assert_eq!(
        bare.rejected
            .iter()
            .map(|r| r.op_id.as_str())
            .collect::<Vec<_>>(),
        ["c"]
    );
}

#[test]
fn a_push_response_without_a_cursor_still_acknowledges_its_ops() {
    let response: PushResponse = serde_json::from_str(r#"{"accepted":["a","b"]}"#).unwrap();
    assert_eq!(response.accepted, ["a", "b"]);
    assert_eq!(response.cursor, None);
}

#[test]
fn a_pull_response_decodes_with_the_three_fields_the_server_adds() {
    let json = r#"{"ops":[{"opId":"o1","entity":"playlists","entityId":"l1","operation":"upsert",
        "payload":{"id":"l1"},"createdAt":1754500000000,"seq":42,"deviceId":"mac",
        "receivedAt":1754500000500}],"cursor":12400,"hasMore":true}"#;
    let response: PullResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.cursor, PullCursor(12400));
    assert!(response.has_more);
    assert_eq!(response.ops.len(), 1);
    assert_eq!(response.ops[0].seq, 42);
    assert_eq!(response.ops[0].device_id, "mac");
    assert_eq!(response.ops[0].received_at, 1_754_500_000_500);
    assert_eq!(response.unreadable_ops, 0);
}

/// One unreadable entry must not cost the page — the page is every future page.
#[test]
fn one_unreadable_op_does_not_cost_the_rest_of_the_batch() {
    let json = r#"{"ops":[
        {"opId":"o1","entity":"playlists","entityId":"l1","operation":"upsert",
         "payload":{"id":"l1"},"createdAt":1,"seq":1,"deviceId":"mac","receivedAt":2},
        {"opId":"o2","entity":"playlists","seq":2},
        {"opId":"o3","entity":"playlists","entityId":"l3","operation":"upsert",
         "payload":{"id":"l3"},"createdAt":1,"seq":3,"deviceId":"mac","receivedAt":2}
        ],"cursor":3,"hasMore":false}"#;
    let response: PullResponse = serde_json::from_str(json).unwrap();
    assert_eq!(
        response
            .ops
            .iter()
            .map(|o| o.op_id.as_str())
            .collect::<Vec<_>>(),
        ["o1", "o3"]
    );
    assert_eq!(response.unreadable_ops, 1);
    assert_eq!(response.cursor, PullCursor(3));
}

#[test]
fn cursors_are_bare_integers_on_the_wire() {
    assert_eq!(
        serde_json::from_str::<PullCursor>("99").unwrap(),
        PullCursor(99)
    );
    assert_eq!(
        serde_json::from_str::<PushCursor>("99").unwrap(),
        PushCursor(99)
    );
    assert_eq!(serde_json::to_string(&PullCursor(99)).unwrap(), "99");
}

#[test]
fn the_pull_cursor_round_trips_through_the_store() {
    let store = store();
    assert_eq!(store.pull_cursor().unwrap(), None);
    store.set_sync_cursor("4200").unwrap();
    assert_eq!(store.pull_cursor().unwrap(), Some(PullCursor(4200)));
}

// MARK: - JSON model

#[test]
fn payload_json_survives_a_round_trip_without_changing_type() {
    let original = json!({
        "id": "x",
        "updatedAt": 1_754_500_000_000_i64,
        "deleted": false,
        "ratio": 0.5,
        "notes": null,
        "tags": ["a", 2]
    });
    let round: Value = serde_json::from_str(&text(&original)).unwrap();
    assert_eq!(round, original);
}

/// Every conflict is decided by comparing two of these.
#[test]
fn a_millisecond_timestamp_stays_an_integer() {
    let decoded: Value = serde_json::from_str(r#"{"t":1754500000123}"#).unwrap();
    assert_eq!(decoded["t"].as_i64(), Some(1_754_500_000_123));
    assert!(decoded["t"].is_i64());
}

#[test]
fn a_boolean_does_not_arrive_as_a_number() {
    let decoded: Value = serde_json::from_str(r#"{"deleted":true}"#).unwrap();
    assert_eq!(decoded["deleted"], Value::Bool(true));
}
