use actix_web::{middleware, web, App, HttpServer};
use mojinprince_server::{api, state::AppState, Config};
use utoipa::OpenApi;
use utoipa_swagger_ui::SwaggerUi;

#[actix_web::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let cfg = Config::from_env()?;
    tracing::info!(bind_addr = %cfg.bind_addr, "starting mojinprince-server");

    let state = AppState::new(cfg.clone()).await?;
    // 自动跑 migrations
    sqlx::migrate!("./migrations").run(&state.db).await?;
    tracing::info!("migrations applied");
    let _quote_scheduler =
        mojinprince_server::service::scheduler::spawn_quote_scheduler(state.clone());
    let _report_scheduler =
        mojinprince_server::service::scheduler::spawn_report_scheduler(state.clone());
    let _ingest_scheduler =
        mojinprince_server::service::scheduler::spawn_ingest_scheduler(state.clone());

    let bind = cfg.bind_addr.clone();
    let prefix = cfg.api_prefix.clone();
    let openapi = api::openapi::ApiDoc::openapi();

    HttpServer::new(move || {
        App::new()
            .app_data(web::Data::new(state.clone()))
            .app_data(web::Data::new(state.db.clone()))
            .wrap(middleware::Logger::default())
            .route("/health", web::get().to(api::health::health))
            .service(
                SwaggerUi::new("/swagger-ui/{_:.*}").url("/api-docs/openapi.json", openapi.clone()),
            )
            .service(
                web::scope(&prefix)
                    .configure(api::quote::configure)
                    .configure(api::ai::configure)
                    .configure(api::data::configure)
                    .configure(api::review::configure)
                    .configure(api::ingest::configure)
                    .configure(api::pick::configure)
                    .configure(api::export::configure)
                    .configure(api::ws::configure),
            )
    })
    .bind(bind)?
    .run()
    .await?;
    Ok(())
}
