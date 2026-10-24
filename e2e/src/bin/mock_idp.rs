//! Runs [`e2e::MockIdp`] as a standalone server for `.claude/skills/verify-event-manager`.
//!
//! Env: `MOCK_IDP_ADDR` (default `127.0.0.1:18081`), `MOCK_IDP_KEY` (RSA private key PEM path,
//! required), `MOCK_IDP_CLIENT_ID` (default `event-manager`), `MOCK_IDP_CLIENT_SECRET` (when set,
//! the token endpoint requires it).

use std::env;

use e2e::MockIdp;

fn main() -> Result<(), String> {
    let addr = env::var("MOCK_IDP_ADDR").unwrap_or_else(|_| "127.0.0.1:18081".into());
    let key_path = env::var("MOCK_IDP_KEY").map_err(|_| "MOCK_IDP_KEY must be set")?;
    let key = std::fs::read_to_string(&key_path).map_err(|e| format!("{key_path}: {e}"))?;
    let client_id = env::var("MOCK_IDP_CLIENT_ID").unwrap_or_else(|_| "event-manager".into());
    let secret = env::var("MOCK_IDP_CLIENT_SECRET").ok();

    let idp = MockIdp::start(&addr, &client_id, secret.as_deref(), &key)?;
    println!("mock idp listening on {}", idp.issuer());
    loop {
        std::thread::park();
    }
}
