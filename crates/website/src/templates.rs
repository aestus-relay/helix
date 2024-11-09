use askama::Template;
use crate::models::DeliveredPayload;
// Remove unnecessary imports from the top scope
// Remove `use scaler::Formatter;` and other imports here

// Define the `filters` module without `use super::*;`
pub mod filters {
    use std::fmt::Display;
    use num_format::{Locale, ToFormattedString};
    use alloy_primitives::utils::format_units;
    use ethereum_consensus::primitives::U256;
    use askama::Result;
    use askama::Error;

    pub fn pretty_int<T>(i: &T) -> Result<String>
    where
        T: ToFormattedString,
    {
        Ok(i.to_formatted_string(&Locale::en))
    }

    pub fn wei_to_eth(wei: &U256) -> Result<String> {
        let eth = format_units(*wei, "ether")
            .map_err(|_| Error::Fmt(std::fmt::Error))?;
        Ok(format!("{:.6}", eth)) // Format to 6 decimal places
    }
}

#[derive(Template)]
#[template(path = "index.html")]
pub struct IndexTemplate {
    pub network: String,
    pub relay_url: String,
    pub relay_pubkey: String,
    pub show_config_details: bool,
    pub network_validators: i64,
    pub registered_validators: i64,
    pub latest_slot: i32,
    pub recent_payloads: Vec<DeliveredPayload>,
    pub num_delivered_payloads: i64,
    pub value_link: String,
    pub value_order_icon: String,
    pub link_beaconchain: String,
    pub link_etherscan: String,
    pub link_data_api: String,
    pub capella_fork_version: String,
    pub bellatrix_fork_version: String,
    pub genesis_fork_version: String,
    pub genesis_validators_root: String,
    pub builder_signing_domain: String
    //pub beacon_proposer_signing_domain: String //May be irrelevant?
}
