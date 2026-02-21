#![cfg(feature = "http")]

//! Integration tests for the real HTTP handler.
//! These tests require network access and the `http` feature flag.
//! Run with: cargo test -p boruna-vm --features http

use boruna_bytecode::{Capability, Value};
use boruna_vm::capability_gateway::{CapabilityHandler, NetPolicy};
use boruna_vm::http_handler::HttpHandler;

#[test]
fn test_http_handler_real_get() {
    let handler = HttpHandler::new(NetPolicy::default());
    // httpbin.org provides a stable test endpoint
    let result = handler.handle_net_fetch(&[Value::String("https://httpbin.org/get".into())]);
    match result {
        Ok(Value::String(body)) => {
            assert!(
                body.contains("httpbin.org"),
                "response should mention httpbin"
            );
        }
        Ok(other) => panic!("expected String, got {other:?}"),
        Err(e) => {
            // Network may not be available in CI — skip gracefully
            eprintln!("skipping real HTTP test (network unavailable): {e}");
        }
    }
}

#[test]
fn test_http_handler_ssrf_blocked_localhost() {
    let handler = HttpHandler::new(NetPolicy::default());
    let result = handler.handle_net_fetch(&[Value::String("http://localhost:9999/secret".into())]);
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert!(
        err.contains("localhost"),
        "error should mention localhost: {err}"
    );
}

#[test]
fn test_http_handler_ssrf_blocked_private_ip() {
    let handler = HttpHandler::new(NetPolicy::default());
    let result = handler.handle_net_fetch(&[Value::String("http://192.168.1.1/admin".into())]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("private IP"));
}

#[test]
fn test_http_handler_domain_allowlist_blocks() {
    let policy = NetPolicy {
        allowed_domains: vec!["api.allowed.com".to_string()],
        ..NetPolicy::default()
    };
    let handler = HttpHandler::new(policy);
    let result = handler.handle_net_fetch(&[Value::String("https://api.blocked.com/data".into())]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not in allowlist"));
}

#[test]
fn test_http_handler_method_allowlist_blocks() {
    let policy = NetPolicy {
        allowed_methods: vec!["GET".to_string()],
        ..NetPolicy::default()
    };
    let handler = HttpHandler::new(policy);
    let result = handler.handle_net_fetch(&[
        Value::String("https://api.example.com/".into()),
        Value::String("POST".into()),
    ]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("not in allowlist"));
}

#[test]
fn test_http_handler_delegates_non_net_fetch() {
    let mut handler = HttpHandler::new(NetPolicy::default());
    // TimeNow should return mock value via fallback
    let result = handler.handle(&Capability::TimeNow, &[]);
    assert!(result.is_ok());
    assert_eq!(result.unwrap(), Value::Int(1700000000));
}

#[test]
fn test_http_handler_blocks_ftp_scheme() {
    let handler = HttpHandler::new(NetPolicy::default());
    let result =
        handler.handle_net_fetch(&[Value::String("ftp://files.example.com/secret.txt".into())]);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("scheme"));
}
