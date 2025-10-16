# helix-k8s

Kubernetes leader election integration for Helix relay's auction service.

## Overview

This crate enables high-availability deployments of the Helix relay in Kubernetes. Multiple relay instances run simultaneously, but only one (the "leader") actively processes auction traffic at any given time. This architecture provides fast automatic failover while maintaining the performance benefits of in-memory auction state.

**Key Features**:
- **Zero-latency leader-only traffic** - K8s routes requests only to the active leader via readinessProbe
- **Ultra-fast failover** - New leader elected within 2-3 seconds of failure detection
- **Automatic rotation** - Leaders voluntarily step down every N slots for fair load distribution
- **Slot-aware transitions** - All leadership changes respect slot boundaries to prevent dropped bids
- **Split-brain prevention** - Multiple safeguards ensure only one leader exists at a time

## Why Leader Election?

The Helix relay's auction mechanism depends on shared in-memory state (`BestGetHeader`, `BidSorter`, `LocalCache`) that cannot be distributed across multiple processes. However, running a single instance creates a single point of failure. 

Leader election solves this by:
- Running multiple relay instances for redundancy
- Electing one as the active leader to handle auction traffic
- Keeping followers ready as hot standbys
- Automatically promoting a follower when the leader fails
- Ensuring smooth transitions without data loss

## Architecture

### Leader Election Mechanism

Uses the Kubernetes Lease API (`coordination.k8s.io/v1`) for distributed leader election:

1. **Startup**: All pods attempt to acquire the lease
2. **Winner becomes leader**: First pod to acquire lease starts serving traffic
3. **Followers standby**: Other pods wait for lease to become available
4. **Continuous renewal**: Leader renews lease every 500ms to maintain leadership
5. **Failure detection**: If leader stops renewing, lease expires after 1 second
6. **Automatic failover**: Followers detect expired lease and compete to become new leader

### Traffic Routing

Kubernetes uses the `/health/leader` readinessProbe to determine which pod should receive traffic:

- **Leader pod**: `/health/leader` returns `200 OK` → Marked Ready → Receives traffic
- **Follower pods**: `/health/leader` returns `503 Service Unavailable` → Not Ready → No traffic

This ensures that only the elected leader processes auction requests, while followers remain invisible to the load balancer.

### Automatic Rotation

To ensure fair resource utilization and regularly test failover mechanisms, leaders voluntarily step down after holding leadership for a configured number of slots (default: 32 slots ≈ 6.4 minutes):

1. Leader detects rotation interval reached
2. Waits for current slot to complete safely
3. Releases lease
4. Backs off briefly to allow others to compete
5. Returns to follower mode
6. Another pod becomes the new leader

This creates a rotation pattern where leadership circulates among all healthy pods.

## Modules

### Lease Manager (`lease_manager.rs`)

Core orchestration of the leader election lifecycle:

- **Lease acquisition**: Attempts to create or claim the Kubernetes Lease object
- **Leadership verification**: Post-acquisition checks to prevent split-brain scenarios
- **Continuous renewal**: Maintains leadership by renewing lease every 500ms
- **Rotation tracking**: Monitors slot count to determine when to voluntarily step down
- **Graceful release**: Cleanly releases lease during shutdown or rotation
- **State callbacks**: Triggers `on_elected_leader()` and `on_lost_leadership()` events

### Slot-Aware Shutdown (`slot_aware_shutdown.rs`)

Ensures leadership transitions occur at safe boundaries to prevent mid-slot disruptions:

- **Slot advancement detection**: Waits for the slot to advance beyond the initial slot
- **Auctioneer state tracking**: Slot advancement indicates the auctioneer called `on_new_slot()`, which cleans up all previous slot state
- **Safe transition guarantee**: Whether payload was delivered, slot was empty, or missed - slot advancement = safe to transition
- **Timeout protection**: 4-second safety timeout prevents infinite waits if housekeeper is delayed
- **Multiple triggers**: Handles shutdown signals, voluntary rotation, and unexpected lease loss
- **Bid protection**: Ensures proposers who received headers can successfully call `get_payload` before leadership changes

The logic leverages the auctioneer's internal state machine: when it transitions from `Broadcasting`/`Sorting` to `Slot` state (triggered by a new slot event), it has cleaned up all previous slot data, making it safe to transfer leadership.

### Health Endpoint (`health.rs`)

Provides the readinessProbe endpoint that Kubernetes uses for traffic routing:

- **Leader check**: Returns HTTP 200 when pod is the active leader
- **Follower response**: Returns HTTP 503 when pod is a follower
- **Termination awareness**: Returns HTTP 503 when pod is shutting down
- **Fast response**: Sub-millisecond checks using atomic boolean flags

### Metrics (`metrics.rs`)

Exports Prometheus metrics for monitoring leader election health:

- **Leader state**: Current leadership status (1=leader, 0=follower)
- **Renewal tracking**: Successful and failed lease renewal attempts
- **Transition counting**: Total number of leadership changes
- **Timing histogram**: Time spent waiting for slot completion during transitions

## Configuration

The leader election system is configured via the `k8s_leader_election` section in your relay config:

**Timing Parameters**:
- `lease_duration_seconds` - How long lease remains valid without renewal (default: 1.0s)
- `renew_deadline_seconds` - How often leader renews the lease (default: 0.5s)
- `retry_period_seconds` - How often followers check for expired lease (default: 1.0s)

**Behavior Parameters**:
- `enabled` - Enable/disable leader election (default: false)
- `lease_name` - Name of the Kubernetes Lease object (default: "helix-relay-leader")
- `rotation_interval_slots` - Slots before voluntary rotation (default: 32, null to disable)
- `slot_completion_timeout_seconds` - Max wait for slot completion during transitions (default: 4s)

**Timing Relationship**: The `renew_deadline` should be less than `lease_duration` to give the leader a "head start" and prevent followers from stealing the lease during normal renewal. Recommended: `renew_deadline = 0.5 * lease_duration`.

## Failover Behavior

### Normal Operation
- Leader serves all traffic and renews lease every 500ms
- Followers idle and check lease status every 1 second
- Every 32 slots, leader voluntarily rotates to next pod

### Leader Crashes
1. Leader pod crashes - lease renewal stops
2. After 1 second, followers detect expired lease
3. Followers race to acquire the lease
4. Winner becomes new leader
5. K8s readinessProbe detects new leader (~1-2s)
6. Traffic routes to new leader
7. **Total failover: 2-3 seconds**

### Graceful Shutdown (Deployment Rollout)
1. Pod receives SIGTERM signal
2. Sets terminating flag (health endpoint returns 503)
3. K8s removes pod from service endpoints
4. Waits for current slot to complete (0-4s)
5. Releases lease immediately
6. New leader elected from remaining pods
7. **Total graceful failover: 1-6 seconds**

### Voluntary Rotation (Every 32 Slots)
1. Leader detects 32 slots held
2. Waits for slot completion
3. Releases lease at slot boundary
4. Backs off 3 seconds (prevents immediate re-acquisition)
5. Another pod acquires lease
6. **Total rotation time: 1-5 seconds**

## Split-Brain Prevention

Multiple layered safeguards ensure only one leader exists:

**1. Post-Acquisition Verification**  
After acquiring lease, pod pauses briefly and re-reads the lease to confirm it's still the holder. This handles eventual consistency in the Kubernetes API.

**2. Continuous Holder Verification**  
During each renewal cycle, pod verifies it's still the lease holder. If another pod somehow acquired the lease, immediate step-down occurs.

**3. Rotation Backoff**  
After voluntarily releasing the lease, pod waits 3x the retry period before attempting to re-acquire. This prevents immediate re-acquisition and allows other pods to compete fairly.

**4. Decoupled Renewal from Slot Initialization**  
Lease renewals begin immediately upon acquiring leadership, even if slot information isn't available yet. This prevents renewal delays that could trigger false lease expiration.

## Kubernetes Integration

### Required RBAC Permissions

The auction service pods require a ServiceAccount with permissions to manage Lease objects:

- `get` - Read lease status
- `create` - Create lease if it doesn't exist  
- `update` - Update lease holder during acquisition
- `patch` - Patch lease during renewals
- `delete` - Delete lease during graceful shutdown

These permissions are scoped to a single namespace and limited to the specific lease name.

### Required Environment Variables

- `HOSTNAME` - Pod name (automatically set by Kubernetes via `metadata.name`)
- Namespace is read from `/var/run/secrets/kubernetes.io/serviceaccount/namespace`

### Health Probes

**Readiness Probe** (determines traffic routing):
- Endpoint: `/health/leader`
- Frequency: Every 1 second
- Leader: Returns 200 → Pod marked Ready → Receives traffic
- Follower: Returns 503 → Pod marked NotReady → No traffic

**Liveness Probe** (determines pod health):
- Endpoint: `/eth/v1/builder/status`
- Frequency: Every 10 seconds
- Checks overall relay health, not leadership status
- Prevents pod termination for followers

## Deployment Considerations

### Replica Count
Run exactly **3 replicas** for the auction service:
- Ensures 2 followers available during voluntary rotation
- Provides redundancy without excessive resource consumption
- Odd number prevents election ties

### Rollout Strategy
Use `maxSurge: 0` and `maxUnavailable: 1` to prevent deadlock:
- Allows terminating old leader before new pod is ready
- Avoids scenario where new pod can't become leader (old pod holds lease)
- Ensures graceful slot-aware transitions during rollouts

### Resource Overhead
Each pod requires:
- **Memory**: ~5-10MB additional for Kubernetes client and TLS
- **CPU**: <0.1% for lease renewal and health checks
- **Network**: ~2 API calls per second per pod (renewals + checks)

### Observability
Monitor these metrics for healthy operation:
- `sum(helix_leader_election_state)` should equal 1 (exactly one leader)
- `rate(helix_leader_transitions_total[1h])` should be ~9-10 with 32-slot rotation
- `rate(helix_lease_failures_total[5m])` should be 0 (no renewal failures)

## Testing

### Verify Leader Election
```bash
kubectl get lease -n relay-helix helix-relay-leader
# Should show one holder

kubectl get pods -n relay-helix -l app=relay-api-auction-helix
# Should show 3 pods, 1 Ready (the leader)
```

### Test Failover
```bash
# Delete current leader
LEADER=$(kubectl get lease -n relay-helix helix-relay-leader -o jsonpath='{.spec.holderIdentity}')
kubectl delete pod -n relay-helix $LEADER

# Watch new leader election (should complete in 2-3 seconds)
kubectl get lease -n relay-helix helix-relay-leader -w
```

### Test Rotation
```bash
# Watch automatic rotation (every ~6.4 minutes with 32-slot interval)
kubectl get lease -n relay-helix helix-relay-leader -w
```

### Test Rollout
```bash
# Trigger deployment rollout
kubectl rollout restart deployment relay-api-auction-helix -n relay-helix

# Watch graceful leadership transitions
kubectl get pods -n relay-helix -l app=relay-api-auction-helix -w
```

## Troubleshooting

### No Leader Elected
**Symptom**: No pods marked Ready, lease exists but no holder  
**Cause**: RBAC permissions insufficient  
**Fix**: Verify ServiceAccount has all required verbs (get, create, update, patch, delete) on leases

### Multiple Leaders (Split-Brain)
**Symptom**: More than one pod shows Ready status  
**Cause**: Should never happen with current safeguards  
**Fix**: Delete lease to force clean re-election: `kubectl delete lease -n relay-helix helix-relay-leader`

### Frequent Unexpected Rotations
**Symptom**: Leader changes more often than rotation interval  
**Cause**: Slot tracking not initialized properly  
**Fix**: Check logs for "Rotation tracking initialized" - ensure slot information is available

### Slow Failover
**Symptom**: Failover takes >5 seconds  
**Cause**: ReadinessProbe polling too infrequent  
**Fix**: Reduce `periodSeconds` in readinessProbe configuration (minimum 1 second recommended)

## Performance Impact

Running with leader election enabled:
- **Request latency**: No overhead - leader processes requests identically to single-instance mode
- **Failover window**: 2-3 seconds of rejected requests (503 responses) during leader transition
- **Resource usage**: ~15-30MB total overhead across 3 pods for Kubernetes client libraries
- **API load**: ~6 calls/second total to Kubernetes API (all pods combined)

The system is designed for production use with minimal performance impact while providing high availability.

---

For deployment instructions and Kubernetes manifest details, see [`../../k8s/README.md`](../../k8s/README.md).

For implementation details and code examples, see the source files in `src/`.
