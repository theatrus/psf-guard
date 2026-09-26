use super::*;
use serde_json::{json, Value};
use tokio::io::{duplex, DuplexStream};

const SESSION: &str = "0123456789abcdef0123456789abcdef";

pub(super) fn command(id: u64, payload: Command) -> Message {
    Message {
        protocol_version: PROTOCOL_VERSION,
        session_id: SESSION.into(),
        request_id: id,
        payload,
    }
}

fn hello() -> Message {
    command(
        0,
        Command::Hello {
            runtime_version: RUNTIME_VERSION.into(),
            engine_version: ENGINE_VERSION.into(),
            contract_version: CONTRACT_VERSION,
            rig_id: "rig-1".into(),
        },
    )
}

pub(super) async fn send(stream: &mut DuplexStream, message: &Message) {
    write_frame(stream, &serde_json::to_vec(message).unwrap())
        .await
        .unwrap();
}

pub(super) async fn receive_reply(stream: &mut DuplexStream) -> Reply {
    // Match the host's request budget; real FULL-sync SQLite I/O is not a
    // one-second latency test on shared CI disks.
    let data = read_frame(stream, Duration::from_secs(5))
        .await
        .unwrap()
        .unwrap();
    serde_json::from_slice(&data).unwrap()
}

pub(super) async fn handshake(stream: &mut DuplexStream) {
    send(stream, &hello()).await;
    let response = receive_reply(stream).await;
    assert_eq!(response.session_id, SESSION);
    assert_eq!(response.request_id, 0);
    assert!(matches!(response.payload, ResultMessage::Ready { .. }));
}

#[tokio::test]
async fn handshake_ping_and_shutdown_are_correlated() {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    send(&mut client, &command(1, Command::Ping)).await;
    let reply = receive_reply(&mut client).await;
    assert_eq!(reply.protocol_version, PROTOCOL_VERSION);
    assert_eq!(reply.request_id, 1);
    assert!(matches!(reply.payload, ResultMessage::Pong));
    send(&mut client, &command(2, Command::Shutdown)).await;
    assert!(matches!(
        receive_reply(&mut client).await.payload,
        ResultMessage::Stopped
    ));
    client.shutdown().await.unwrap();
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test(start_paused = true)]
async fn shutdown_keeps_reply_alive_until_host_disconnects() {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    send(&mut client, &command(1, Command::Shutdown)).await;
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_millis(100)).await;
    assert!(!task.is_finished());
    assert!(matches!(
        receive_reply(&mut client).await.payload,
        ResultMessage::Stopped
    ));
    client.shutdown().await.unwrap();
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test(start_paused = true)]
async fn shutdown_wait_is_bounded_and_cannot_dispatch_more_commands() {
    for extra_command in [false, true] {
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve(server));
        handshake(&mut client).await;
        send(&mut client, &command(1, Command::Shutdown)).await;
        assert!(matches!(
            receive_reply(&mut client).await.payload,
            ResultMessage::Stopped
        ));
        if extra_command {
            send(&mut client, &command(2, Command::Ping)).await;
        }
        assert_eq!(
            task.await.unwrap(),
            Err(if extra_command {
                ProtocolError::InvalidMessage
            } else {
                ProtocolError::Timeout
            })
        );
    }
}

#[tokio::test]
async fn invalid_handshakes_never_start_a_session() {
    for field in [
        "protocol_version",
        "runtime_version",
        "engine_version",
        "contract_version",
        "session_id",
        "request_id",
        "rig_id",
    ] {
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve(server));
        let mut message = serde_json::to_value(hello()).unwrap();
        match field {
            "protocol_version" | "request_id" => message[field] = json!(99),
            "session_id" => message[field] = json!("not-a-session"),
            "contract_version" => message["payload"][field] = json!(99),
            "rig_id" => message["payload"][field] = json!(""),
            _ => message["payload"][field] = json!("wrong"),
        }
        write_frame(&mut client, &serde_json::to_vec(&message).unwrap())
            .await
            .unwrap();
        assert!(task.await.unwrap().is_err(), "{field}");
        assert!(read_frame(&mut client, Duration::from_secs(1))
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn cannot_evaluate_before_handshake_or_repeat_a_handshake() {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    send(&mut client, &command(0, Command::Ping)).await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::InvalidHandshake));
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    let mut second = hello();
    second.request_id = 1;
    send(&mut client, &second).await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::InvalidHandshake));
}

#[tokio::test]
async fn stale_session_duplicate_and_out_of_order_requests_close_the_session() {
    for (id, session, expected) in [
        (1, SESSION, ProtocolError::RequestOrder),
        (3, SESSION, ProtocolError::RequestOrder),
        (
            2,
            "ffffffffffffffffffffffffffffffff",
            ProtocolError::SessionMismatch,
        ),
    ] {
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve(server));
        handshake(&mut client).await;
        send(&mut client, &command(1, Command::Ping)).await;
        receive_reply(&mut client).await;
        let mut message = command(id, Command::Ping);
        message.session_id = session.into();
        send(&mut client, &message).await;
        assert_eq!(task.await.unwrap(), Err(expected));
        assert!(read_frame(&mut client, Duration::from_secs(1))
            .await
            .unwrap()
            .is_none());
    }
}

#[tokio::test]
async fn malformed_utf8_and_unknown_ipc_fields_are_rejected() {
    for frame in [b"\xff".to_vec(), b"{".to_vec(), b"{\"extra\":1}".to_vec()] {
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve(server));
        write_frame(&mut client, &frame).await.unwrap();
        assert_eq!(task.await.unwrap(), Err(ProtocolError::InvalidMessage));
    }
}

#[tokio::test]
async fn command_fields_are_strict_even_for_commands_without_data() {
    for payload in [
        r#"{"type":"ping","extra":1}"#,
        r#"{"type":"shutdown","extra":1}"#,
        r#"{"type":"evaluate","request":{},"extra":1}"#,
        r#"{"type":"evaluate","request":{},"request":{}}"#,
        r#"{"type":"ping","type":"shutdown"}"#,
    ] {
        assert!(
            serde_json::from_str::<Command>(payload).is_err(),
            "{payload}"
        );
    }
}

#[tokio::test]
async fn raw_planning_input_keeps_duplicate_fields_and_size_limits() {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    let oversized = format!(
        "{{{}\"contract_version\":2}}",
        " ".repeat(psf_guard_director_core::MAX_REQUEST_BYTES)
    );
    for (index, raw) in [
        r#"{"contract_version":2,"contract_version":2}"#,
        oversized.as_str(),
    ]
    .iter()
    .enumerate()
    {
        let message = format!(
            r#"{{"protocol_version":{PROTOCOL_VERSION},"session_id":"{SESSION}","request_id":{},"payload":{{"type":"evaluate","request":{raw}}}}}"#,
            index + 1
        );
        write_frame(&mut client, message.as_bytes()).await.unwrap();
        let ResultMessage::Decision { response } = receive_reply(&mut client).await.payload else {
            panic!("expected a core error response");
        };
        assert_eq!(response, evaluate_json(raw.as_bytes()));
        assert_eq!(
            serde_json::to_value(response.outcome).unwrap()["status"],
            "error"
        );
    }
    drop(client);
    assert_eq!(task.await.unwrap(), Ok(()));
}

#[tokio::test]
async fn oversized_and_zero_length_frames_reject_before_reading_the_body() {
    for size in [0_u32, u32::MAX, MAX_FRAME_BYTES as u32 + 1] {
        let (mut client, mut server) = duplex(8);
        client.write_all(&size.to_le_bytes()).await.unwrap();
        assert_eq!(
            read_frame(&mut server, Duration::from_secs(1)).await,
            Err(ProtocolError::FrameSize)
        );
    }
}

#[tokio::test]
async fn clean_eof_and_truncated_frame_are_different() {
    let (client, mut server) = duplex(8);
    drop(client);
    assert_eq!(
        read_frame(&mut server, Duration::from_secs(1)).await,
        Ok(None)
    );
    for bytes in [vec![1], vec![2, 0, 0, 0, b'{']] {
        let (mut client, mut server) = duplex(8);
        client.write_all(&bytes).await.unwrap();
        drop(client);
        assert_eq!(
            read_frame(&mut server, Duration::from_secs(1)).await,
            Err(ProtocolError::Transport)
        );
    }
}

#[tokio::test(start_paused = true)]
async fn partial_frame_and_handshake_cannot_wait_forever() {
    let (mut client, mut server) = duplex(8);
    client.write_all(&[1]).await.unwrap();
    assert_eq!(
        read_frame(&mut server, Duration::from_secs(1)).await,
        Err(ProtocolError::Timeout)
    );
    let (_client, server) = duplex(8);
    assert_eq!(serve(server).await, Err(ProtocolError::Timeout));
}

#[tokio::test(start_paused = true)]
async fn idle_connection_and_stalled_response_time_out() {
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::Timeout));
    let (_client, mut server) = duplex(1);
    assert_eq!(
        write_frame(&mut server, b"{}").await,
        Err(ProtocolError::Timeout)
    );
}

fn merge(value: &mut Value, patch: &Value) {
    if let (Some(target), Some(source)) = (value.as_object_mut(), patch.as_object()) {
        for (key, item) in source {
            merge(target.entry(key).or_insert(Value::Null), item);
        }
    } else {
        *value = patch.clone();
    }
}

#[tokio::test]
async fn shared_golden_vectors_are_identical_over_the_protocol() {
    for source in [
        include_str!("../../director-core/tests/fixtures/decisions.json"),
        include_str!("../../director-core/tests/fixtures/rig-windows.json"),
    ] {
        let fixture: Value = serde_json::from_str(source).unwrap();
        let (mut client, server) = duplex(MAX_FRAME_BYTES);
        let task = tokio::spawn(serve(server));
        handshake(&mut client).await;
        for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
            let mut request = fixture["base"].clone();
            merge(&mut request, &case["patch"]);
            let raw = serde_json::value::RawValue::from_string(request.to_string()).unwrap();
            let expected = evaluate_json(raw.get().as_bytes());
            send(
                &mut client,
                &command(index as u64 + 1, Command::Evaluate { request: raw }),
            )
            .await;
            let reply = receive_reply(&mut client).await;
            assert_eq!(reply.request_id, index as u64 + 1);
            let ResultMessage::Decision { response } = reply.payload else {
                panic!("expected a decision");
            };
            assert_eq!(response, expected, "{}", case["name"]);
            assert_eq!(
                serde_json::to_value(response.outcome).unwrap(),
                case["expected"]
            );
        }
        drop(client);
        assert_eq!(task.await.unwrap(), Ok(()));
    }
}

#[tokio::test]
async fn rig_scope_cannot_be_changed_after_handshake() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../director-core/tests/fixtures/decisions.json"
    ))
    .unwrap();
    let mut request = fixture["base"].clone();
    request["assignment"]["rig_id"] = json!("another-rig");
    let (mut client, server) = duplex(MAX_FRAME_BYTES);
    let task = tokio::spawn(serve(server));
    handshake(&mut client).await;
    send(
        &mut client,
        &command(
            1,
            Command::Evaluate {
                request: serde_json::value::RawValue::from_string(request.to_string()).unwrap(),
            },
        ),
    )
    .await;
    assert_eq!(task.await.unwrap(), Err(ProtocolError::WrongRig));
}
