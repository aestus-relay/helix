use axum::{extract::State, response::Html, extract::Query};
use std::collections::HashMap;
use std::sync::Arc;
use askama::Template;
use crate::state::AppState;
use tracing::{info, debug, error};

pub async fn index(
    State(state): State<Arc<AppState>>,
    Query(params): Query<HashMap<String, String>>
) -> Result<Html<String>, axum::http::StatusCode> {
    info!("Handling index request");
    let order_by = params.get("order_by").map(|s| s.as_str());
    debug!("Order by parameter: {:?}", order_by);

    let cached_templates = match state.cached_templates.read() {
        Ok(guard) => guard,
        Err(e) => {
            error!("Failed to acquire read lock on cached templates: {:?}", e);
            return Err(axum::http::StatusCode::INTERNAL_SERVER_ERROR);
        }
    };

    let template = match order_by {
        Some("-value") => &cached_templates.by_value_desc,
        Some("value") => &cached_templates.by_value_asc,
        _ => &cached_templates.default,
    };

    template.render()
        .map(Html)
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)
}
