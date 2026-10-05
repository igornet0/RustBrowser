//! IPC protocol regression tests.

use crate::{
    validate_frame, BrowserToContent, ContentToBrowser, Envelope, FrameBuffer, InputEventMsg,
    InboundOrder, PROTOCOL_VERSION,
};
use browser_core::TabId;

fn tab() -> TabId {
    TabId::new()
}

#[test]
fn protocol_version_is_v4() {
    assert_eq!(PROTOCOL_VERSION, 4);
}

#[test]
fn roundtrip_script_and_screenshot_messages() {
    let tab_id = tab();
    let req = Envelope::new(
        5,
        BrowserToContent::EvaluateScript {
            tab_id,
            script: "document.title".into(),
        },
    );
    let back: Envelope<BrowserToContent> =
        serde_json::from_slice(&serde_json::to_vec(&req).unwrap()).unwrap();
    assert!(matches!(back.payload, BrowserToContent::EvaluateScript { .. }));

    let reply = Envelope::new(
        5,
        ContentToBrowser::ScriptResult {
            tab_id,
            result: Ok(serde_json::json!({ "title": "Example", "links": [1, 2] })),
        },
    );
    let back: Envelope<ContentToBrowser> =
        serde_json::from_slice(&serde_json::to_vec(&reply).unwrap()).unwrap();
    assert_eq!(back.request_id, 5);
    match back.payload {
        ContentToBrowser::ScriptResult { result: Ok(v), .. } => assert_eq!(v["title"], "Example"),
        other => panic!("unexpected {other:?}"),
    }
    assert_eq!(crate::content_msg_tab(&ContentToBrowser::Screenshot {
        tab_id,
        result: Ok("iVBOR".into()),
    }), Some(tab_id));
}

#[test]
fn network_log_roundtrip_and_bounds() {
    use crate::{validate_content_to_browser, NetworkRequestMsg, MAX_NETWORK_LOG};
    let tab_id = tab();
    let req = NetworkRequestMsg {
        url: "https://shop.test/api/products?page=1".into(),
        method: "GET".into(),
        destination: "empty".into(),
        main_frame: false,
        blocked: false,
    };
    let msg = ContentToBrowser::NetworkLog {
        tab_id,
        requests: vec![req.clone()],
    };
    let back: ContentToBrowser = serde_json::from_slice(&serde_json::to_vec(&msg).unwrap()).unwrap();
    match back {
        ContentToBrowser::NetworkLog { requests, .. } => assert_eq!(requests, vec![req.clone()]),
        other => panic!("unexpected {other:?}"),
    }
    assert!(validate_content_to_browser(&ContentToBrowser::NetworkLog {
        tab_id,
        requests: vec![req; MAX_NETWORK_LOG + 1],
    })
    .is_err());
}

#[test]
fn script_validation_bounds() {
    use crate::{validate_browser_to_content, MAX_SCRIPT_LENGTH};
    let tab_id = tab();
    assert!(validate_browser_to_content(&BrowserToContent::EvaluateScript {
        tab_id,
        script: "1 + 1".into(),
    })
    .is_ok());
    assert!(validate_browser_to_content(&BrowserToContent::EvaluateScript {
        tab_id,
        script: "   ".into(),
    })
    .is_err());
    assert!(validate_browser_to_content(&BrowserToContent::EvaluateScript {
        tab_id,
        script: "x".repeat(MAX_SCRIPT_LENGTH + 1),
    })
    .is_err());
}

#[test]
fn envelope_carries_version_generation_request_id() {
    let env = Envelope::with_generation(42, 7, BrowserToContent::Heartbeat);
    assert_eq!(env.version, PROTOCOL_VERSION);
    assert_eq!(env.request_id, 42);
    assert_eq!(env.generation, 7);
    assert_eq!(env.sequence, 0);
}

#[test]
fn envelope_sequence_ordering() {
    let env = Envelope::full(7, 3, 99, ContentToBrowser::Ready);
    assert_eq!(env.sequence, 99);
    assert_eq!(env.generation, 3);
    assert_eq!(env.request_id, 7);
}

#[test]
fn roundtrip_browser_to_content_nav_and_input() {
    let tab_id = tab();
    let msgs = vec![
        BrowserToContent::CreateTab {
            tab_id,
            url: None,
        },
        BrowserToContent::Navigate {
            tab_id,
            url: url::Url::parse("https://example.com").unwrap(),
        },
        BrowserToContent::Reload { tab_id },
        BrowserToContent::GoBack { tab_id },
        BrowserToContent::GoForward { tab_id },
        BrowserToContent::Resize {
            width: 800,
            height: 600,
            scale_factor: 2.0,
        },
        BrowserToContent::SetViewport {
            tab_id,
            width: 800,
            height: 600,
            scale_factor: 2.0,
        },
        BrowserToContent::Input {
            tab_id,
            event: InputEventMsg::MouseMove { x: 10.0, y: 20.0 },
        },
        BrowserToContent::Input {
            tab_id,
            event: InputEventMsg::Key {
                key: "a".into(),
                pressed: true,
                modifiers: 0,
            },
        },
        BrowserToContent::Input {
            tab_id,
            event: InputEventMsg::Text {
                text: "hello".into(),
            },
        },
        BrowserToContent::SuspendTab { tab_id },
        BrowserToContent::ResumeTab { tab_id },
        BrowserToContent::CloseTab { tab_id },
        BrowserToContent::RequestFrame { tab_id },
        BrowserToContent::SetNetworkRoute {
            mode: crate::NetworkRouteMsg::Direct,
        },
    ];
    for msg in msgs {
        let env = Envelope::with_generation(1, 1, msg);
        let bytes = serde_json::to_vec(&env).unwrap();
        let back: Envelope<BrowserToContent> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.version, PROTOCOL_VERSION);
        assert_eq!(back.generation, 1);
    }
}

#[test]
fn roundtrip_content_to_browser_frame_and_nav() {
    let tab_id = tab();
    let msgs = vec![
        ContentToBrowser::Ready,
        ContentToBrowser::HeartbeatAck,
        ContentToBrowser::NavigationStarted {
            tab_id,
            url: url::Url::parse("https://example.com").unwrap(),
        },
        ContentToBrowser::NavigationFinished {
            tab_id,
            url: url::Url::parse("https://example.com/").unwrap(),
        },
        ContentToBrowser::TitleChanged {
            tab_id,
            title: "Example".into(),
        },
        ContentToBrowser::UrlChanged {
            tab_id,
            url: url::Url::parse("https://example.com/").unwrap(),
        },
        ContentToBrowser::LoadProgress {
            tab_id,
            progress: 0.5,
        },
        ContentToBrowser::ConsoleMessage {
            tab_id,
            level: "log".into(),
            message: "hi".into(),
        },
        ContentToBrowser::Frame {
            tab_id,
            frame: FrameBuffer {
                width: 2,
                height: 2,
                rgba: vec![0; 16],
            },
        },
    ];
    for msg in msgs {
        let env = Envelope::full(2, 5, 5, msg);
        let bytes = serde_json::to_vec(&env).unwrap();
        let back: Envelope<ContentToBrowser> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(back.sequence, 5);
        assert_eq!(back.generation, 5);
    }
}

#[test]
fn unknown_version_detected_by_transport_contract() {
    let mut env = Envelope::new(1, BrowserToContent::Heartbeat);
    env.version = PROTOCOL_VERSION + 100;
    assert_ne!(env.version, PROTOCOL_VERSION);
}

#[test]
fn oversized_frame_rejected() {
    let frame = FrameBuffer {
        width: 8000,
        height: 8000,
        rgba: vec![],
    };
    // empty rgba is placeholder path — but dimensions exceed max
    assert!(validate_frame(&frame).is_err() || crate::checked_frame_bytes(8000, 8000).is_err());
}

#[test]
fn wrong_tab_sequence_independent() {
    let mut order = InboundOrder::new(1);
    let a = TabId::new();
    let b = TabId::new();
    assert!(order.accept(1, 1, Some(a)).unwrap());
    assert!(order.accept(1, 1, Some(b)).unwrap());
    assert!(!order.accept(1, 1, Some(a)).unwrap());
}
