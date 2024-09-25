// crates/website/src/handlers.rs

use axum::{extract::State, response::Html, extract::Query};
use std::collections::HashMap;
use askama::Template;
use crate::state::AppState;
use crate::templates::IndexTemplate;
use crate::database_queries::WebsiteDatabaseService;
use helix_utils::signing::{compute_builder_domain};
use hex::encode as hex_encode;

pub async fn index(
    State(state): State<AppState>,
    Query(params): Query<HashMap<String, String>>
) -> Result<Html<String>, axum::http::StatusCode> {
    // Handle order_by parameter
    let order_by = params.get("order_by").map(|s| s.as_str());
    let (value_link, value_order_icon) = match order_by {
        Some("-value") => ("/?order_by=value", "▼"),
        Some("value") => ("/", "▲"),
        _ => ("/?order_by=-value", ""),
    };

    // Fetch data from database
    let num_network_validators = state.db_pool.get_num_network_validators().await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let num_registered_validators = state.db_pool.get_num_registered_validators().await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let latest_slot = state.db_pool.get_latest_slot().await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let recent_payloads = state.db_pool.get_recent_delivered_payloads(30).await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;
    let num_delivered_payloads = state.db_pool.get_num_delivered_payloads().await
        .map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?;

    // Build template
    let template = IndexTemplate {
        network: state.chain_info.network.to_string(),
        relay_url: state.website_config.relay_url.clone(),
        relay_pubkey: state.website_config.relay_pubkey.clone(),
        show_config_details: state.website_config.show_config_details,
        network_validators: num_network_validators,
        registered_validators: num_registered_validators,
        latest_slot: latest_slot,
        recent_payloads: recent_payloads,
        num_delivered_payloads: num_delivered_payloads,
        value_link: value_link.to_string(),
        value_order_icon: value_order_icon.to_string(),
        link_beaconchain: state.website_config.link_beaconchain.clone(),
        link_etherscan: state.website_config.link_etherscan.clone(),
        link_data_api: state.website_config.link_data_api.clone(),
        capella_fork_version: hex_encode(state.chain_info.context.capella_fork_version),
        bellatrix_fork_version: hex_encode(state.chain_info.context.bellatrix_fork_version),
        genesis_fork_version: hex_encode(state.chain_info.context.genesis_fork_version),
        genesis_validators_root: hex_encode(state.chain_info.genesis_validators_root.as_ref() as &[u8]),
        builder_signing_domain: compute_builder_domain(&state.chain_info.context)
            .map(hex_encode)
            .unwrap_or_else(|e| {
                String::from("Error computing builder domain")
            }),
        /* beacon_proposer_signing_domain:  //May be irrelevant? */
    };

    Ok(Html(template.render().map_err(|_| axum::http::StatusCode::INTERNAL_SERVER_ERROR)?))
}
