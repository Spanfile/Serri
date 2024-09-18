mod device;
mod template;

use std::{net::SocketAddr, sync::Arc};

use axum::{
    body::Body,
    extract::ConnectInfo,
    http::{Request, StatusCode},
    response::IntoResponse,
    routing, Extension, Router,
};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::{debug_span, info};

use crate::{
    config::SerriConfig,
    serial_controller::SerialController,
    web::template::{AboutTemplate, BaseTemplate, IndexTemplate, NotFoundTemplate},
};

const DIST_PATH: Option<&str> = option_env!("SERRI_DIST_PATH");
const DEFAULT_DIST_PATH: &str = "dist";

pub async fn run(
    serri_config: SerriConfig,
    controllers: Vec<SerialController>,
) -> anyhow::Result<()> {
    let serri_config = Arc::new(serri_config);
    let app = Router::new()
        .route("/", routing::get(root))
        .route("/about", routing::get(about))
        .nest("/device", device::router())
        .nest_service(
            "/dist",
            ServeDir::new(DIST_PATH.unwrap_or(DEFAULT_DIST_PATH)),
        )
        .fallback(not_found)
        .layer(Extension(Arc::clone(&serri_config)))
        .layer(Extension(Arc::new(controllers)))
        .layer(
            TraceLayer::new_for_http().make_span_with(|request: &Request<Body>| {
                let ConnectInfo(addr) = request
                    .extensions()
                    .get::<ConnectInfo<SocketAddr>>()
                    .expect(
                        "ConnectInfo missing (did you forget to call \
                         app.into_make_service_with_connect_info()?)",
                    );

                debug_span!(
                    "request",
                    method = %request.method(),
                    uri = %request.uri(),
                    version = ?request.version(),
                    %addr,
                )
            }),
        );

    let listener = tokio::net::TcpListener::bind(serri_config.listen).await?;
    info!(listen = %serri_config.listen, "Serving web");

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(super::shutdown_signal())
    .await?;

    Ok(())
}

async fn not_found(Extension(serri_config): Extension<Arc<SerriConfig>>) -> impl IntoResponse {
    (
        StatusCode::NOT_FOUND,
        NotFoundTemplate {
            base_template: BaseTemplate {
                serri_config,
                active_path: "",
            },
        },
    )
}

async fn root(Extension(serri_config): Extension<Arc<SerriConfig>>) -> IndexTemplate {
    IndexTemplate {
        base_template: BaseTemplate {
            serri_config,
            active_path: "/",
        },
        active_device_index: None,
    }
}

async fn about(Extension(serri_config): Extension<Arc<SerriConfig>>) -> AboutTemplate {
    AboutTemplate {
        base_template: BaseTemplate {
            serri_config,
            active_path: "/about",
        },
    }
}
