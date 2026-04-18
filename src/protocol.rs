pub use dispatch_courier_protocol::*;

pub fn capabilities() -> CourierCapabilities {
    CourierCapabilities {
        courier_id: "seren-cloud".to_string(),
        kind: CourierKind::Custom,
        supports_chat: true,
        supports_job: true,
        supports_heartbeat: true,
        supports_local_tools: false,
        supports_mounts: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dispatch_channel_protocol::{OutboundAttachment, OutboundMessageEnvelope};
    use std::collections::BTreeMap;

    #[test]
    fn run_request_round_trips_dispatch_wire_format() {
        let request = PluginRequestEnvelope {
            protocol_version: COURIER_PLUGIN_PROTOCOL_VERSION,
            request: PluginRequest::Run {
                parcel_dir: "/tmp/demo".to_string(),
                session: CourierSession {
                    id: "session-1".to_string(),
                    parcel_digest: "digest".to_string(),
                    entrypoint: Some("chat".to_string()),
                    label: None,
                    turn_count: 1,
                    elapsed_ms: 42,
                    history: vec![ConversationMessage {
                        role: "user".to_string(),
                        content: "hello".to_string(),
                    }],
                    resolved_mounts: Vec::new(),
                    backend_state: Some("{\"deployment_id\":\"dep-1\"}".to_string()),
                },
                operation: CourierOperation::Chat {
                    input: "hello".to_string(),
                },
            },
        };

        let json = serde_json::to_string(&request).unwrap();
        let parsed: PluginRequestEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, request);
    }

    #[test]
    fn channel_reply_event_round_trips_dispatch_wire_format() {
        let response = PluginResponse::Event {
            event: CourierEvent::ChannelReply {
                message: OutboundMessageEnvelope {
                    content: "reply text".to_string(),
                    content_type: Some("text/plain".to_string()),
                    attachments: vec![OutboundAttachment {
                        name: "report.txt".to_string(),
                        mime_type: "text/plain".to_string(),
                        data_base64: Some("aGVsbG8=".to_string()),
                        url: None,
                        storage_key: None,
                    }],
                    metadata: BTreeMap::from([(
                        "conversation_id".to_string(),
                        "conv-1".to_string(),
                    )]),
                },
            },
        };

        let json = serde_json::to_string(&response).unwrap();
        let parsed: PluginResponse = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, response);
    }
}
