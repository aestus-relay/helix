#[derive(Debug, Default, Clone)]
pub struct ElectGatewayTrace {
    pub receive: u64,
    pub deserialize: u64,
    pub validation_complete: u64,
    pub gateway_election_saved: u64,
}

#[derive(Debug, Default, Clone)]
pub struct GetGatewayTrace {
    pub receive: u64,
    pub gateway_fetched: u64,
}