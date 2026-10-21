use actix_web::{App, HttpServer, middleware::Logger, web};
use event_manager::db::Db;
use event_manager::sso::{Sso, SsoConfig};

#[actix_web::main]
async fn main() -> std::io::Result<()> {
    env_logger::init_from_env(env_logger::Env::new().default_filter_or("info"));

    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://events.db".to_string());
    let bind = std::env::var("BIND_ADDR").unwrap_or_else(|_| "127.0.0.1:8080".to_string());

    let db = web::Data::new(
        Db::connect(&database_url, 5)
            .await
            .map_err(std::io::Error::other)?,
    );

    let sso_config = SsoConfig::from_env().map_err(std::io::Error::other)?;
    log::info!("discovering SSO provider {}", sso_config.issuer_url);
    let sso = web::Data::new(Sso::discover(sso_config).await.map_err(std::io::Error::other)?);

    log::info!("listening on http://{bind}");
    HttpServer::new(move || {
        App::new()
            .wrap(Logger::default())
            .app_data(db.clone())
            .app_data(sso.clone())
            .configure(event_manager::configure)
    })
    .bind(bind)?
    .run()
    .await
}
