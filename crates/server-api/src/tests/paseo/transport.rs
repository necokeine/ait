use super::*;

#[tokio::test]
async fn malformed_json_closes_only_its_physical_connection() {
    let fixture = Fixture::start().await;
    let mut bad = connected(&fixture, json!(["connection.ping"])).await;
    let mut good = connected(&fixture, json!(["connection.ping"])).await;
    bad.send(Message::Text("{".into())).await.unwrap();
    let error = receive(&mut bad).await;
    assert_eq!(error["code"], "invalid_message");
    assert!(error["request_id"].is_null());
    assert!(matches!(
        bad.next().await.unwrap().unwrap(),
        Message::Close(_)
    ));
    assert_quiet_before_ping(&mut good).await;
    fixture.stop().await;
}

#[tokio::test]
async fn binary_terminal_input_requires_its_own_negotiated_capability() {
    let fixture = Fixture::start().await;
    let mut socket = connected(&fixture, json!(["connection.ping"])).await;
    socket
        .send(Message::Binary(vec![2, 0, b'x'].into()))
        .await
        .unwrap();
    let error = receive(&mut socket).await;
    assert_eq!(error["code"], "unsupported_capability");
    assert!(error["request_id"].is_null());
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn uninstalled_terminal_binary_input_returns_explicit_not_implemented() {
    let fixture = Fixture::start().await;
    let mut socket = connected(&fixture, json!(["connection.ping", "terminal.input"])).await;
    socket
        .send(Message::Binary(vec![2, 0, b'x'].into()))
        .await
        .unwrap();
    assert_eq!(receive(&mut socket).await["code"], "not_implemented");
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn uncorrelated_event_errors_never_borrow_a_previous_request_id() {
    let fixture = Fixture::start().await;
    let mut socket = event_socket(&fixture).await;
    assert_quiet_before_ping(&mut socket).await;
    send(
        &mut socket,
        json!({"type":"event","method":"session.heartbeat","params":{}}),
    )
    .await;
    let error = receive(&mut socket).await;
    assert_eq!(error["code"], "invalid_message");
    assert!(error["request_id"].is_null());
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn ping_control_frames_do_not_consume_the_hello_or_rpc_message() {
    let fixture = Fixture::start().await;
    let mut socket = fixture.socket().await;
    socket
        .send(Message::Ping(b"pre-hello".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Pong(b"pre-hello".to_vec().into())
    );
    send(&mut socket, hello()).await;
    assert_eq!(receive(&mut socket).await["type"], "server_info");
    socket
        .send(Message::Ping(b"active".to_vec().into()))
        .await
        .unwrap();
    assert_eq!(
        socket.next().await.unwrap().unwrap(),
        Message::Pong(b"active".to_vec().into())
    );
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn fragmented_utf8_json_with_interleaved_control_frame_is_one_request() {
    let fixture = Fixture::start().await;
    let mut socket = connected(&fixture, json!(["connection.ping"])).await;
    let payload = json!({"type":"request","request_id":"split","method":"connection.ping","params":{"nonce":"中文🦀"}}).to_string();
    let boundary = payload.find('中').unwrap() + 1;
    let (first, last) = payload.as_bytes().split_at(boundary);
    socket
        .send(Message::Frame(WebSocketFrame::message(
            first.to_vec(),
            OpCode::Data(Data::Text),
            false,
        )))
        .await
        .unwrap();
    socket
        .send(Message::Ping(b"fragment-ping".to_vec().into()))
        .await
        .unwrap();
    assert!(matches!(
        socket.next().await.unwrap().unwrap(),
        Message::Pong(_)
    ));
    socket
        .send(Message::Frame(WebSocketFrame::message(
            last.to_vec(),
            OpCode::Data(Data::Continue),
            true,
        )))
        .await
        .unwrap();
    let response = receive(&mut socket).await;
    assert_eq!(response["request_id"], "split");
    assert_eq!(response["result"]["nonce"], "中文🦀");
    assert_quiet_before_ping(&mut socket).await;
    fixture.stop().await;
}

#[tokio::test]
async fn pipelined_requests_preserve_correlation_and_response_order() {
    let fixture = Fixture::start().await;
    let mut socket = connected(&fixture, json!(["connection.ping"])).await;
    for index in 0..32 {
        send(&mut socket, json!({"type":"request","request_id":format!("request-{index}"),"method":"connection.ping","params":{"nonce":index.to_string()}})).await;
    }
    for index in 0..32 {
        let response = receive(&mut socket).await;
        assert_eq!(response["request_id"], format!("request-{index}"));
        assert_eq!(response["result"]["nonce"], index.to_string());
    }
    fixture.stop().await;
}

#[tokio::test]
async fn a_silent_pre_hello_socket_does_not_block_a_negotiated_peer() {
    let fixture = Fixture::start().await;
    let silent = fixture.socket().await;
    let mut active = connected(&fixture, json!(["connection.ping"])).await;
    assert_quiet_before_ping(&mut active).await;
    fixture.stop().await;
    drop(silent);
}

#[tokio::test]
async fn dropped_connections_return_their_admission_permit() {
    let fixture = Fixture::start().await;
    let socket = event_socket(&fixture).await;
    assert_eq!(
        fixture.api.shared.connections.available_permits(),
        server_protocol::MAX_CONNECTIONS - 1
    );
    drop(socket);
    tokio::time::timeout(Duration::from_secs(3), async {
        while fixture.api.shared.connections.available_permits() != server_protocol::MAX_CONNECTIONS
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let mut replacement = event_socket(&fixture).await;
    assert_quiet_before_ping(&mut replacement).await;
    fixture.stop().await;
}

#[tokio::test]
async fn loopback_alias_origin_is_accepted_only_on_the_bound_port() {
    let fixture = Fixture::start().await;
    for host in ["localhost", "127.0.0.1"] {
        let mut upgrade = format!("ws://{}/v1/ws", fixture.address)
            .into_client_request()
            .unwrap();
        upgrade
            .headers_mut()
            .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
        upgrade.headers_mut().insert(
            "origin",
            format!("http://{host}:{}", fixture.address.port())
                .parse()
                .unwrap(),
        );
        let (mut socket, _) = connect_async(upgrade.clone()).await.unwrap();
        send(&mut socket, hello()).await;
        assert_eq!(receive(&mut socket).await["type"], "server_info");
        socket.close(None).await.unwrap();
        upgrade.headers_mut().insert(
            "origin",
            format!("http://{host}:{}", fixture.address.port().saturating_sub(1))
                .parse()
                .unwrap(),
        );
        let error = connect_async(upgrade).await.unwrap_err();
        assert!(
            matches!(error, tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == StatusCode::FORBIDDEN)
        );
    }
    fixture.stop().await;
}

#[tokio::test]
async fn a_different_loopback_address_requires_explicit_browser_origin_permission() {
    // Rust binds one address; unlike Paseo it does not implicitly trust every loopback alias.
    let origin = "http://[::1]:8081";
    for allowed in [false, true] {
        let origins = if allowed {
            vec![origin.to_owned()]
        } else {
            Vec::new()
        };
        let fixture = Fixture::with_origins(Services::default(), origins).await;
        let mut upgrade = format!("ws://{}/v1/ws", fixture.address)
            .into_client_request()
            .unwrap();
        upgrade
            .headers_mut()
            .insert("authorization", format!("Bearer {TOKEN}").parse().unwrap());
        upgrade
            .headers_mut()
            .insert("origin", origin.parse().unwrap());
        let result = connect_async(upgrade).await;
        if allowed {
            let (mut socket, _) = result.unwrap();
            send(&mut socket, hello()).await;
            assert_eq!(receive(&mut socket).await["type"], "server_info");
            socket.close(None).await.unwrap();
        } else {
            assert!(
                matches!(result.unwrap_err(), tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == StatusCode::FORBIDDEN)
            );
        }
        fixture.stop().await;
    }
}
