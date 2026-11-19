//! Golden wire frames include reserved examples for readers, not for capability negotiation.
use super::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

/// Compare JSON objects rather than serialization order, which is not part of the wire contract.
fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) {
    let typed: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(typed).unwrap(), value);
}

/// Check the full published instance messages including absent and explicitly null fields.
#[test]
fn instance_messages_roundtrip() {
    roundtrip::<Message>(
        json!({"v": 1, "seq": 0, "stage": "hello", "protocols": [1], "aproxy_version": "0.2.0", "name": "adaptive", "process": "persistent", "stages": ["request", "decide", "notify"], "worker_id": 0, "features": [], "overrides": ["timeouts.connect_secs", "timeouts.first_byte_secs", "timeouts.read_idle_secs", "keepalive.interval_secs", "keepalive.trigger", "keepalive.heartbeat", "retry.max_backoff_secs", "retry.max_attempts"]}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 3, "stage": "request", "request_id": "19ac3f2e8b5d1a2b-42", "delivery": "retry", "digest": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08", "request": {"method": "POST", "url": "https://relay.example.com/v1/messages?beta=true", "headers": {"content-type": "application/json", "x-api-key": "sk-…"}}, "client": {"path": "/v1/messages?beta=true", "stream": true}}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 7, "stage": "request", "request_id": "19ac3f2e8b5d1a2b-42", "delivery": "forward", "request": {"method": "POST", "url": "https://relay.example.com/v1/messages?beta=true", "headers": {"content-type": "application/json", "x-api-key": "sk-…"}, "body": "{\"model\":\"claude-sonnet-4-5\",\"stream\":true}"}, "client": {"path": "/v1/messages?beta=true", "stream": true}}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 12, "stage": "decide", "request_id": "19ac3f2e8b5d1a2b-44", "state": "…", "attempt": 6, "elapsed_ms": 42000, "delivery": "retry", "outcome": "failure", "failure": {"code": "upstream_status", "details": {"status": 429}}, "request": {"method": "POST", "url": "https://relay.example.com/v1/chat/completions"}, "response": {"status": 429, "headers": {"retry-after": "60"}, "body": "{\"error\":{\"message\":\"rate limited\"}}", "truncated": null}, "fallback": {"decision": "retry", "delay_ms": 20000, "by": "default"}}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 15, "stage": "response", "request_id": "…", "state": "…", "request": {"method": "POST", "url": "https://relay.example.com/v1/chat/completions"}, "response": {"status": 200, "headers": {"content-type": "application/json"}, "body": "{…openai…}"}}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 20, "stage": "stream", "event": "open", "stream_id": "19ac3f2e8b5d1a2b-42/2", "request_id": "…", "state": "…", "request": {"method": "POST", "url": "…"}, "response": {"status": 200, "headers": {"content-type": "text/event-stream"}}}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 21, "stage": "stream", "event": "data", "stream_id": "19ac3f2e8b5d1a2b-42/2", "data": "data: {\"choices\":[{\"delta\":{\"content\":\"Hel\"}}]}\n\n"}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 30, "stage": "stream", "event": "close", "stream_id": "19ac3f2e8b5d1a2b-42/2"}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 31, "stage": "stream", "event": "abort", "stream_id": "19ac3f2e8b5d1a2b-45/1", "reason": {"code": "upstream_body_interrupted"}}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 40, "stage": "heartbeat", "request_id": "…", "state": "…", "heartbeat": {"beat": 3, "elapsed_ms": 45000, "attempt": 2, "last_failure": {"code": "upstream_status", "details": {"status": 529}}}, "client": {"path": "/v1/messages", "stream": true}, "data": ": keepalive\n\n"}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 51, "stage": "notify", "event": "request_end", "request_id": "19ac3f2e8b5d1a2b-42", "state": "{\"digest\":\"9f86d081…\"}", "digest": "9f86d081884c7d659a2feaa0c55ad015a3bf4f1b2b0b822cd15d6c15b0f00a08", "delivery": "retry", "upgraded": true, "attempts": 2, "upstream_status": 200, "stream_completed": true, "duration_ms": 48210, "outcome": "ok", "code": null}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 52, "stage": "notify", "event": "attempt_end", "request_id": "…", "attempt": 1, "at": 1760190000999u64, "outcome": "failure", "failure": {"code": "upstream_status", "details": {"status": 429}}, "response": {"status": 429, "headers": {"x-ratelimit-remaining-requests": "0"}}}),
    );
    roundtrip::<Message>(
        json!({"v": 1, "seq": 50, "stage": "admit", "request_id": "…", "state": "…", "attempt": 1, "request": {"method": "POST", "url": "…"}, "queue": {"length": 3, "position": 1, "waited_ms": 1200}}),
    );
}

/// Check the full published hello reply including absent and explicitly null fields.
#[test]
fn hello_reply_roundtrip() {
    roundtrip::<HelloReply>(
        json!({"v": 1, "seq": 0, "protocol": 1, "stages": ["request", "decide", "notify"], "request_body": {"request": "digest", "notify": "digest"}, "features": [], "name": "aproxy-format adaptive", "version": "0.2.0"}),
    );
}

/// Check the full published stage replies including absent and explicitly null fields.
#[test]
fn stage_replies_roundtrip() {
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 3, "delivery": "forward", "state": "{\"digest\":\"9f86d081…\"}", "report": {"summary": "no recent failure for this request; delivering forward", "actions": [{"type": "custom", "label": "deliver forward"}]}}),
    );
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 7, "request": {"url": "https://relay.example.com/v1/chat/completions", "body": "{…}"}, "overrides": {"timeouts": {"first_byte_secs": 900}}, "report": {"summary": "anthropic_messages -> openai_chat", "actions": [{"type": "protocol_convert", "from": "anthropic_messages", "to": "openai_chat"}]}}),
    );
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 7, "fail": {"retry": false, "code": "model_not_routed", "reason": "no channel configured for model gpt-9", "response": {"status": 404, "headers": {"content-type": "application/json"}, "body": "{\"type\":\"error\",\"error\":{\"type\":\"not_found_error\",\"message\":\"model gpt-9 is not configured\"}}"}}}),
    );
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 12, "decision": "retry", "delay_ms": 60000, "reroute": true, "report": {"summary": "429 on k2; upstream asks 60s; switch key and wait", "actions": [{"type": "custom", "label": "cooldown k2 60s"}]}}),
    );
    roundtrip::<Reply>(json!({"v": 1, "seq": 15, "response": {"body": "{…anthropic…}"}}));
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 20, "response": {"headers": {"content-type": "text/event-stream"}}, "abort_tail": "event: error\ndata: {\"type\":\"error\",\"error\":{\"type\":\"api_error\",\"message\":\"stream conversion aborted\"}}\n\n"}),
    );
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 21, "data": "event: content_block_delta\ndata: {…}\n\n"}),
    );
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 30, "data": "event: message_stop\ndata: {\"type\":\"message_stop\"}\n\n"}),
    );
    roundtrip::<Reply>(json!({"v": 1, "seq": 31, "data": "…"}));
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 40, "data": "event: ping\ndata: {\"type\":\"ping\"}\n\n"}),
    );
    roundtrip::<Reply>(json!({"v": 1, "seq": 51}));
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 50, "admit": false, "retry_after_ms": 5000, "reason": "5h quota window full"}),
    );
    roundtrip::<Reply>(
        json!({"v": 1, "seq": 50, "admit": true, "grant": {"count": 10, "ttl_ms": 60000}}),
    );
}

/// Omitted fields and unrelated future keys must not be confused with an empty patch.
#[test]
fn patches_preserve_omitted_and_replace_explicitly_empty() {
    let mut r = Request::new("POST".into(), "https://example.test".into());
    r.headers = Headers::from([("first".into(), "1".into())]);
    r.body = Body::from_bytes(vec![255]);
    let patch: RequestPatch = serde_json::from_value(json!({"headers":{},"future":true})).unwrap();
    assert_eq!(serde_json::to_value(&patch).unwrap(), json!({"headers":{}}));
    r.apply(patch);
    assert_eq!(r.headers, Headers::new());
    assert_eq!(r.body.as_bytes(), Some([255].as_slice()));
    r.apply(serde_json::from_value(json!({"body":""})).unwrap());
    assert_eq!(r.body.as_bytes(), Some([].as_slice()));
    let patch = ResponsePatch::new();
    assert_eq!(serde_json::to_value(&patch).unwrap(), json!({}));
    let mut r = Response::new(200);
    r.apply(serde_json::from_value(json!({"status":201,"body_b64":"/w=="})).unwrap());
    r.apply(patch);
    assert_eq!(r.status, 201);
    assert_eq!(
        serde_json::to_value(r).unwrap(),
        json!({"status":201,"headers":{},"body_b64":"/w=="})
    );
}

/// Both spellings together and malformed base64 are invalid rather than a lossy rewrite.
#[test]
fn invalid_bytes_are_rejected() {
    for patch in [json!({"body":"x","body_b64":""}), json!({"body_b64":"%%%"})] {
        assert!(serde_json::from_value::<RequestPatch>(patch).is_err());
    }
    assert!(
        serde_json::from_value::<Reply>(json!({"v":1,"seq":1,"data":"","data_b64":""})).is_err()
    );
}

/// A program can render future actions while ignoring keys not yet in the contract.
#[test]
fn extension_fields_are_ignored_and_action_labels_survive() {
    let reply: Reply=serde_json::from_value(json!({"v":1,"seq":8,"future":"x","report":{"summary":"done","actions":[{"type":"future","label":"show me","private":false}]}})).unwrap();
    assert_eq!(
        serde_json::to_value(reply).unwrap(),
        json!({"v":1,"seq":8,"report":{"summary":"done","actions":[{"type":"future","label":"show me"}]}})
    );
    assert_eq!(
        serde_json::to_value(Reply::new(8)).unwrap(),
        json!({"v":1,"seq":8})
    );
    let mut hello =
        serde_json::to_value(HelloReply::new(vec![], "tool".into(), "1".into())).unwrap();
    hello["request_body"] = json!({"request":"surprise"});
    assert!(serde_json::from_value::<HelloReply>(hello).is_err());
    let mut ctx = Context::new("id".into());
    ctx.digest = Some(None);
    roundtrip::<Context>(serde_json::to_value(ctx).unwrap());
}

/// Future stages choose their own vocabulary; the four negotiated stages have fixed needs.
#[test]
fn hello_checks_only_negotiated_body_need_keys() {
    let mut hello =
        json!({"v":1,"seq":0,"protocol":1,"stages":[],"features":[],"name":"tool","version":"1"});
    hello["request_body"] = json!({"request":"digest","response":"future","other":{"future":[]}});
    let reply: HelloReply = serde_json::from_value(hello.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(reply).unwrap()["request_body"],
        json!({"request":"digest"})
    );
    for stage in ["request", "retry", "decide", "notify"] {
        for invalid in [json!("future"), json!({"digest":null})] {
            hello["request_body"] = json!({(stage):invalid});
            assert!(serde_json::from_value::<HelloReply>(hello.clone()).is_err());
        }
    }
    hello["request_body"] = json!({"notify":"full"});
    assert!(serde_json::from_value::<HelloReply>(hello).is_err());
}

/// Non-exhaustive objects must remain constructible without downstream struct literals.
#[test]
fn public_constructors_and_retry_input() {
    let _ = (
        Hello::new,
        HelloReply::new,
        Context::new,
        Client::new,
        Failure::new,
        Retry::new,
        Heartbeat::new,
        Fallback::new,
        Request::new,
        Response::new,
        RequestPatch::new,
        ResponsePatch::new,
        RequestMessage::new,
        RetryMessage::new,
        ObservedResponse::new,
        DecideMessage::new,
        ResponseMessage::new,
        HeartbeatMessage::new,
        StreamOpen::new,
        StreamMessage::new,
        Completion::new,
        AttemptEnd::new,
        Queue::new,
        AdmitMessage::new,
        Grant::new,
        Message::new,
        Fail::new,
        Reply::new,
        Action::new,
        Report::new,
    );
    let retry = RetryMessage::new(
        RequestMessage::new(
            Context::new("id".into()),
            Delivery::Retry,
            Request::new("POST".into(), "https://example.test/".into()),
            Client::new("/".into(), false),
        ),
        Retry::new(2, Failure::new("upstream_status".into()), 80),
    );
    let wire = serde_json::to_value(Message::new(3, Stage::Retry(retry))).unwrap();
    assert_eq!(
        wire,
        json!({"v":1,"seq":3,"stage":"retry","request_id":"id","delivery":"retry","request":{"method":"POST","url":"https://example.test/","headers":{}},"client":{"path":"/","stream":false},"retry":{"attempt":2,"last_failure":{"code":"upstream_status"},"elapsed_ms":80}})
    );
    roundtrip::<Message>(wire);
}

/// Framing never guesses field order or accepts an uncorrelated reply.
#[test]
fn frames_require_objects_and_both_replies_require_seq() {
    let array = json!([1, 0, 1, [], {}, [], "tool", "1"]);
    for value in [array, json!(false), json!(null)] {
        assert!(serde_json::from_value::<HelloReply>(value.clone()).is_err());
        assert!(serde_json::from_value::<Reply>(value.clone()).is_err());
        assert!(serde_json::from_value::<Message>(value).is_err());
    }
    assert!(serde_json::from_value::<Reply>(json!({"v":1})).is_err());
    assert!(
        serde_json::from_value::<HelloReply>(
            json!({"v":1,"protocol":1,"stages":[],"features":[],"name":"tool","version":"1"})
        )
        .is_err()
    );
}

/// A vocabulary member is a bare string, not an externally tagged unit variant.
#[test]
fn tokens_reject_object_spellings_and_unknown_words() {
    fn check<T: serde::de::DeserializeOwned>(word: &str) {
        assert!(serde_json::from_value::<T>(json!(word)).is_ok());
        assert!(serde_json::from_value::<T>(json!({(word):null})).is_err());
        assert!(serde_json::from_value::<T>(json!("unnegotiated")).is_err());
    }
    check::<Decision>("retry");
    check::<Delivery>("retry");
    check::<Process>("persistent");
    check::<BodyNeed>("digest");
}

/// A null optional patch leaves the value alone and does not conflict with real bytes.
#[test]
fn null_reply_fields_mean_absence() {
    let binary: ResponsePatch =
        serde_json::from_value(json!({"body":null,"body_b64":"/w=="})).unwrap();
    assert_eq!(binary.body, Body::Binary(vec![255]));
    let unchanged: RequestPatch = serde_json::from_value(json!({"body":null})).unwrap();
    assert_eq!(unchanged.body, Body::Absent);
    let r: Reply = serde_json::from_value(
        json!({"v":1,"seq":3,"decision":null,"delay_ms":null,"state":null,"response":null}),
    )
    .unwrap();
    assert_eq!(r, Reply::new(3));
}

/// Full HTTP objects keep empty header tables; metadata views cannot expose those headers.
#[test]
fn full_and_metadata_http_views_have_distinct_fields() {
    let input = json!({"method":"POST","url":"u"});
    let req: Request = serde_json::from_value(input.clone()).unwrap();
    assert_eq!(
        serde_json::to_value(req).unwrap(),
        json!({"method":"POST","url":"u","headers":{}})
    );
    let resp: Response = serde_json::from_value(json!({"status":200})).unwrap();
    assert_eq!(
        serde_json::to_value(resp).unwrap(),
        json!({"status":200,"headers":{}})
    );
    let meta = RequestMeta::new("POST".into(), "u".into());
    assert_eq!(serde_json::to_value(&meta).unwrap(), input);
    assert_eq!(
        serde_json::to_value(DecideRequest::new(meta.clone(), Body::Absent)).unwrap(),
        input
    );
    assert_eq!(
        serde_json::to_value(DecideRequest::new(meta, Body::Binary(vec![255]))).unwrap(),
        json!({"method":"POST","url":"u","body_b64":"/w=="})
    );
}

/// Only replies can omit a fragment. Worker-owned stream events do not repeat request context.
#[test]
fn stream_and_heartbeat_inputs_require_fragments() {
    let stream = json!({"v":1,"seq":1,"stage":"stream","stream_id":"r/1","event":"data"});
    let beat = json!({"v":1,"seq":1,"stage":"heartbeat","request_id":"r","client":{"path":"/","stream":true},"heartbeat":{"beat":1,"attempt":1,"elapsed_ms":4}});
    for mut frame in [stream.clone(), beat] {
        assert!(serde_json::from_value::<Message>(frame.clone()).is_err());
        frame["data_b64"] = json!("AP8=");
        let msg: Message = serde_json::from_value(frame.clone()).unwrap();
        let fragment = match &msg.stage {
            Stage::Stream(s) => match &s.event {
                StreamEvent::Data(f) => f,
                _ => unreachable!(),
            },
            Stage::Heartbeat(b) => &b.data,
            _ => unreachable!(),
        };
        assert_eq!(fragment.as_bytes(), &[0, 255]);
        assert_eq!(serde_json::to_value(msg).unwrap(), frame);
    }
    assert_eq!(
        serde_json::to_value(Fragment::from_bytes(vec![])).unwrap(),
        json!({"data":""})
    );
    for (event, name) in [
        (StreamEvent::Data(Fragment::from_bytes(vec![])), "data"),
        (StreamEvent::Close, "close"),
        (
            StreamEvent::Abort {
                reason: Failure::new("e".into()),
            },
            "abort",
        ),
    ] {
        let msg = Message::new(1, Stage::Stream(StreamMessage::new("r/1".into(), event)));
        let mut expected = stream.clone();
        expected["event"] = json!(name);
        if name == "abort" {
            expected["reason"] = json!({"code":"e"});
        } else if name == "data" {
            expected["data"] = json!("");
        }
        assert_eq!(serde_json::to_value(msg).unwrap(), expected);
    }
    let raw = json!({"v":1,"seq":1,"data_b64":"AP8=","state":"after"});
    let reply: Reply = serde_json::from_value(raw.clone()).unwrap();
    assert_eq!(reply.data.as_bytes(), Some(&[0, 255][..]));
    assert_eq!(serde_json::to_value(reply).unwrap(), raw);
}

/// A malformed delay invalidates the vote, unlike a bad delivery hint, which is advisory.
#[test]
fn decide_delays_are_checked_and_delivery_does_not_discard_patches() {
    for n in [0, MAX_DELAY_MS] {
        let raw = json!({"v":1,"seq":1,"delay_ms":n});
        let r: Reply = serde_json::from_value(raw.clone()).unwrap();
        assert_eq!(r.delay_ms.unwrap(), RetryDelay::new(n).unwrap());
        assert_eq!(r.delay_ms.unwrap().millis(), n);
        assert_eq!(serde_json::to_value(r).unwrap(), raw);
    }
    assert!(RetryDelay::new(MAX_DELAY_MS + 1).is_err());
    for n in [json!(-1), json!(0.5), json!("0"), json!(MAX_DELAY_MS + 1)] {
        assert!(serde_json::from_value::<Reply>(json!({"v":1,"seq":1,"delay_ms":n})).is_err());
    }
    for vote in [json!(42), json!("later"), json!({"retry":null})] {
        let r: Reply =
            serde_json::from_value(json!({"v":1,"seq":15,"response":{"body":"x"},"delivery":vote}))
                .unwrap();
        assert_eq!(r.delivery, Some(vote));
        for status in [429, 502] {
            let mut base = Response::new(status);
            base.apply(r.response.clone().unwrap());
            assert_eq!(base.status, status);
            assert_eq!(base.body, Body::Text("x".into()));
        }
    }
}
