use chrono::Duration;
use openidconnect::core::{CoreClient, CoreProviderMetadata};
use openidconnect::{
    ClientId, ClientSecret, EndpointMaybeSet, EndpointNotSet, EndpointSet, IssuerUrl, RedirectUrl,
    reqwest,
};

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
    /// Reads `OIDC_ISSUER_URL`, `OIDC_CLIENT_ID`, `OIDC_CLIENT_SECRET`, `OIDC_REDIRECT_URL`,
    /// `POST_LOGIN_REDIRECT` and `SESSION_TTL_HOURS` from the environment.
    pub fn from_env() -> Result<Self, String> {
        let required = |key: &str| std::env::var(key).map_err(|_| format!("{key} must be set"));
        let optional = |key: &str| std::env::var(key).ok().filter(|v| !v.is_empty());

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
            client_secret: optional("OIDC_CLIENT_SECRET"),
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

/// Shared SSO state: the discovered provider client plus an HTTP client for back-channel calls.
pub struct Sso {
    pub client: OidcClient,
    pub http: reqwest::Client,
    pub config: SsoConfig,
}

impl Sso {
    /// Fetches the provider's discovery document and signing keys.
    pub async fn discover(config: SsoConfig) -> Result<Self, String> {
        // Following redirects on back-channel requests would allow SSRF.
        let http = reqwest::ClientBuilder::new()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| format!("failed to build HTTP client: {e}"))?;

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

        Ok(Self {
            client,
            http,
            config,
        })
    }

    /// Cookies are marked `Secure` whenever the service is reached over HTTPS.
    pub fn secure_cookies(&self) -> bool {
        self.config.redirect_url.starts_with("https://")
    }
}
