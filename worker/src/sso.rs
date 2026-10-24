use std::cell::RefCell;
use std::rc::Rc;

use chrono::Duration;
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::{
    ClientId, ClientSecret, EndpointMaybeSet, EndpointNotSet, EndpointSet, HttpRequest,
    HttpResponse, IssuerUrl, RedirectUrl,
};
use worker::js_sys::Uint8Array;
use worker::{Env, Fetch, Headers, Method, Request, RequestInit, RequestRedirect};

use crate::error::AppError;

/// Settings for the OpenID Connect provider used for single sign-on.
#[derive(Debug, Clone)]
pub struct SsoConfig {
    /// Issuer URL, e.g. `https://accounts.google.com`. Discovery is done at
    /// `<issuer>/.well-known/openid-configuration`.
    pub issuer_url: String,
    pub client_id: String,
    /// Omit for public clients (PKCE is always used).
    pub client_secret: Option<String>,
    /// Must point at this service's `/auth/callback` and be registered with the provider.
    pub redirect_url: String,
    /// Where the browser is sent after a successful login. When unset, the
    /// callback responds with the session as JSON instead.
    pub post_login_redirect: Option<String>,
    pub session_ttl: Duration,
}

impl SsoConfig {
    /// Reads the vars `OIDC_ISSUER_URL`, `OIDC_CLIENT_ID`, `OIDC_REDIRECT_URL`,
    /// `POST_LOGIN_REDIRECT`, `SESSION_TTL_HOURS` and the secret `OIDC_CLIENT_SECRET`.
    /// Empty values count as unset.
    pub fn from_env(env: &Env) -> Result<Self, String> {
        let optional = |key: &str| {
            env.var(key)
                .ok()
                .map(|v| v.to_string())
                .filter(|v| !v.is_empty())
        };
        let required = |key: &str| optional(key).ok_or_else(|| format!("{key} must be set"));

        let session_ttl_hours = match optional("SESSION_TTL_HOURS") {
            Some(v) => v
                .parse::<i64>()
                .ok()
                .filter(|h| *h > 0)
                .ok_or("SESSION_TTL_HOURS must be a positive integer")?,
            None => 24 * 7,
        };

        Ok(Self {
            issuer_url: required("OIDC_ISSUER_URL")?,
            client_id: required("OIDC_CLIENT_ID")?,
            client_secret: env
                .secret("OIDC_CLIENT_SECRET")
                .ok()
                .map(|v| v.to_string())
                .filter(|v| !v.is_empty()),
            redirect_url: required("OIDC_REDIRECT_URL")?,
            post_login_redirect: optional("POST_LOGIN_REDIRECT"),
            session_ttl: Duration::hours(session_ttl_hours),
        })
    }
}

pub type OidcClient = CoreClient<
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointMaybeSet,
    EndpointMaybeSet,
>;

/// The discovered provider client and the settings it was built from.
pub struct Sso {
    pub client: OidcClient,
    pub config: SsoConfig,
}

thread_local! {
    // Workers have no startup hook, so discovery runs on the first SSO request an isolate
    // serves and is reused until the isolate is recycled.
    static SSO: RefCell<Option<Rc<Sso>>> = const { RefCell::new(None) };
}

impl Sso {
    /// The provider client for this isolate, discovered on first use. A failed discovery is
    /// not cached, so the next request retries it.
    pub async fn get(env: &Env) -> Result<Rc<Self>, AppError> {
        if let Some(sso) = SSO.with_borrow(Clone::clone) {
            return Ok(sso);
        }
        let config = SsoConfig::from_env(env).map_err(AppError::Internal)?;
        let sso = Rc::new(Self::discover(config).await.map_err(AppError::Internal)?);
        SSO.set(Some(sso.clone()));
        Ok(sso)
    }

    /// Fetches the provider's discovery document and signing keys.
    async fn discover(config: SsoConfig) -> Result<Self, String> {
        let issuer = IssuerUrl::new(config.issuer_url.clone())
            .map_err(|e| format!("invalid OIDC_ISSUER_URL: {e}"))?;
        let redirect = RedirectUrl::new(config.redirect_url.clone())
            .map_err(|e| format!("invalid OIDC_REDIRECT_URL: {e}"))?;
        let metadata = CoreProviderMetadata::discover_async(issuer, &http)
            .await
            .map_err(|e| format!("OIDC discovery failed: {e}"))?;

        let client = CoreClient::from_provider_metadata(
            metadata,
            ClientId::new(config.client_id.clone()),
            config.client_secret.clone().map(ClientSecret::new),
        )
        .set_redirect_uri(redirect);

        Ok(Self { client, config })
    }

    /// Cookies are marked `Secure` whenever the service is reached over HTTPS.
    pub fn secure_cookies(&self) -> bool {
        self.config.redirect_url.starts_with("https://")
    }
}

/// Back-channel HTTP client for openidconnect, over the Workers fetch API.
pub async fn http(req: HttpRequest) -> Result<HttpResponse, worker::Error> {
    let (parts, body) = req.into_parts();
    let headers = Headers::new();
    for (name, value) in &parts.headers {
        let value = value
            .to_str()
            .map_err(|e| worker::Error::RustError(e.to_string()))?;
        headers.append(name.as_str(), value)?;
    }
    let mut init = RequestInit::new();
    init.with_method(Method::from(parts.method.as_str().to_string()))
        .with_headers(headers)
        // Following redirects on back-channel requests would allow SSRF.
        .with_redirect(RequestRedirect::Manual);
    if !body.is_empty() {
        init.with_body(Some(Uint8Array::from(body.as_slice()).into()));
    }

    let mut resp = Fetch::Request(Request::new_with_init(&parts.uri.to_string(), &init)?)
        .send()
        .await?;
    let mut out = openidconnect::http::Response::builder().status(resp.status_code());
    for (name, value) in resp.headers().entries() {
        out = out.header(name, value);
    }
    out.body(resp.bytes().await?)
        .map_err(|e| worker::Error::RustError(e.to_string()))
}
