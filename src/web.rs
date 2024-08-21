mod device;
mod template;

use std::sync::Arc;

use axum::{http::StatusCode, response::IntoResponse, routing, Extension, Router};
use tower_http::services::ServeDir;

use crate::{
    config::SerriConfig,
    web::template::{BaseTemplate, ConfigTemplate, IndexTemplate, NotFoundTemplate},
};

pub async fn run(serri_config: SerriConfig) -> anyhow::Result<()> {
    let serri_config = Arc::new(serri_config);
    let app = Router::new()
        .route("/", routing::get(root))
        .route("/config", routing::get(config))
        .nest("/device", device::router())
        .nest_service("/dist", ServeDir::new("dist"))
        .fallback(not_found)
        .layer(Extension(Arc::clone(&serri_config)));

    let listener = tokio::net::TcpListener::bind(serri_config.listen).await?;
    axum::serve(listener, app).await?;

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

async fn config(Extension(serri_config): Extension<Arc<SerriConfig>>) -> ConfigTemplate {
    ConfigTemplate {
        base_template: BaseTemplate {
            serri_config,
            active_path: "/config",
        },
    }
}
