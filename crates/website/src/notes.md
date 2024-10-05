Variables expected by index.html:

Network -> network (website_config)
RelayURL: -> relay_url (website config)
RelayPubkey: -> relay_url (website config)
ShowConfigDetails: -> show_config_details (website config)

CapellaForkVersion: -> capella_fork_version (network config) 
BellatrixForkVersion: -> bellatrix_fork_version (network config)
GenesisForkVersion: -> genesis_fork_version (network config)
GenesisValidatorsRoot: -> genesis_validators_root (network config)
BuilderSigningDomain: -> builder_signing_domain (network config)
BeaconProposerSigningDomain: -> beacon_proposer_signing_domain (network_config)

ValidatorsTotal: -> network_validators (postgres known_validators table count total)
ValidatorsRegistered: -> registered_validators (postgres validator_registrations table count total entries)
HeadSlot: -> head_slot (postrgres slot table, latest entry (highest) "number" column)
Payloads: -> recent_payloads (postgres delivered_payload table, filtered)
NumPayloadsDelivered: num_delivered_payloads (postgres delivered_payload table count total entries)


ValueOrderIcon: -> internal config?
ValueLink: -> value_link internal config?
LinkBeaconchain: -> link_beaconchain (website config)
LinkEtherscan: -> link_etherscan (website config)
LinkDataAPI: -> link_data_api (website config)

Variable from the go config:

ListenAddress:     websiteListenAddr,
RelayPubkeyHex:    relayPubkey,
NetworkDetails:    networkInfo,
Redis:             redis,
DB:                db,
Log:               log,
ShowConfigDetails: websiteShowConfigDetails,
LinkBeaconchain:   websiteLinkBeaconchain,
LinkEtherscan:     websiteLinkEtherscan,
LinkDataAPI:       websiteLinkDataAPI,
RelayURL:          websiteRelayURL,

Payload Data:
Slot
ParentHash
BlockHash
BuilderPubkey
ProposerPubkey
ProposerFeeRecipient
GasLimit
GasUsed
Value
NumTx
BlockNumber

Example yaml config:

postgres:
  hostname: postgres-helix
  db_name: helixdb
  user: relay
  password: aestus
region: 1
  region_name: helder_single
redis:
  url: redis://redis-helix:6379
simulator:
  url: http://geth-helder:8547
beacon_clients:
  - url: http://lighthouse-helder:5062
website:
  enabled: true
  port: 8080
  listen_address: 0.0.0.0
  show_config_details: false
  network_name: "helder"
  relay_url: https://helder.aestus.live
  relay_pubkey: "0x12d44ae7ee1b325571447a81150d4e023cc871cf78be201564b71052cddf165a"
  link_beaconchain: https://helder.beaconcha.in
  link_etherscan: https://helder.etherscan.io
  link_data_api: https://helder.aestus.live
  ...anything else
router_config:
  enabled_routes:
  route: All
  rate_limit: null 
network_config: !Custom
  dir_path: /app/network_helder.yml
  genesis_validator_root: '0xa55f9089402f027c67db4a43b6eb7fbb7b2eb79f194a90a2cd4f31913e47b336'
  genesis_time: 1718967660
...