// SPDX-License-Identifier: Apache-2.0

use super::*;
use serde_json::{Value, json};

#[test]
fn retry_wire_admission_preserves_numeric_data_and_open_error_categories() {
    for (token, expected) in [
        ("account_on_hold", ApiRetryError::AccountOnHold),
        ("model_not_found", ApiRetryError::ModelNotFound),
        ("oauth_org_not_allowed", ApiRetryError::OauthOrgNotAllowed),
        ("overloaded", ApiRetryError::Overloaded),
        ("future_error", ApiRetryError::Unknown),
    ] {
        let payload = json!({"type":"api_retry_update","attempt":2,"max_retries":5,
            "retry_delay_ms":549.888_169_845_942_6,"error_status":529,"error":token});
        let actual: SessionUpdate = serde_json::from_value(payload).unwrap();
        assert_eq!(
            actual,
            SessionUpdate::ApiRetryUpdate {
                attempt: 2,
                max_retries: 5,
                retry_delay_ms: 549.888_169_845_942_6,
                error_status: Some(529),
                error: expected,
            }
        );
    }
    for (delay, status) in [(0.0, None), (1500.0, Some(Value::Null))] {
        let mut payload = json!({"type":"api_retry_update","attempt":3,"max_retries":7,
            "retry_delay_ms":delay,"error":"future_error"});
        if let Some(status) = status {
            payload["error_status"] = status;
        }
        assert_eq!(
            serde_json::from_value::<SessionUpdate>(payload.clone()).unwrap(),
            SessionUpdate::ApiRetryUpdate {
                attempt: 3,
                max_retries: 7,
                retry_delay_ms: delay,
                error_status: None,
                error: ApiRetryError::Unknown
            }
        );
        payload["retry_delay_ms"] = json!(-1);
        assert!(serde_json::from_value::<SessionUpdate>(payload).is_err());
    }
}

#[test]
fn retry_metadata_wire_patch_distinguishes_omission_replacement_and_clear() {
    for (fields, supplied, expected) in [
        (json!({}), false, None),
        (json!({"task_metadata":{}}), true, None),
        (
            json!({"task_metadata":{"subagent_retry":{"state":"clear"}}}),
            true,
            Some(SubagentRetryUpdate::Clear),
        ),
        (
            json!({"task_metadata":{"subagent_retry":{"state":"waiting","agent_id":"worker-17","attempt":2,"max_retries":4,"retry_delay_ms":1500,"error_status":429,"error_category":"rate_limit"}}}),
            true,
            Some(SubagentRetryUpdate::Waiting {
                agent_id: Some("worker-17".into()),
                attempt: 2,
                max_retries: 4,
                retry_delay_ms: 1500,
                error_status: Some(429),
                error_category: Some("rate_limit".into()),
            }),
        ),
        (
            json!({"task_metadata":{"subagent_retry":{"state":"waiting","attempt":1,"max_retries":3,"retry_delay_ms":0}}}),
            true,
            Some(SubagentRetryUpdate::Waiting {
                agent_id: None,
                attempt: 1,
                max_retries: 3,
                retry_delay_ms: 0,
                error_status: None,
                error_category: None,
            }),
        ),
    ] {
        let payload = json!({"type":"tool_call_update","tool_call_update":{"tool_call_id":"retry-target","fields":fields}});
        let SessionUpdate::ToolCallUpdate { tool_call_update: update } =
            serde_json::from_value(payload).unwrap()
        else {
            panic!("tool update")
        };
        assert_eq!(update.tool_call_id, "retry-target");
        assert_eq!(update.fields.task_metadata.is_some(), supplied);
        assert_eq!(update.fields.task_metadata.and_then(|m| m.subagent_retry), expected);
    }
}
