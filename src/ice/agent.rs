use std::time::Duration;

use thiserror::Error;

use crate::id::random_id;

use super::candidate::{Candidate, CandidateKind};
use super::pair::CandidatePair;

#[derive(Debug, Error)]
pub enum IceError {
    #[error("ICE bad candidate: {0}")]
    BadCandidate(String),
}

#[derive(Debug)]
pub struct IceAgent {
    /// Timing advance (Ta) value.
    ///
    /// ICE agents SHOULD use a default Ta value, 50 ms, but MAY use another
    /// value based on the characteristics of the associated data.
    ///
    /// If an agent wants to use a Ta value other than the default value, the
    /// agent MUST indicate the proposed value to its peer during the
    /// establishment of the ICE session.  Both agents MUST use the higher
    /// value of the proposed values.
    timing_advance: Option<Duration>,

    /// Whether this agent is operating as ice-lite.
    ice_lite: bool,

    /// Username for this side.
    username: String,

    /// Password for this side.
    password: String,

    /// If this side is controlling or controlled.
    controlling: bool,

    /// Current state of the agent.
    state: IceConnectionState,

    /// All local candidates, in the order they are "discovered" (either by
    /// adding explicitly using add_candidate, or via binding/allocation
    /// requests.
    local_candidates: Vec<Candidate>,

    /// All remote candidates, in the order we get to know them.
    remote_candidates: Vec<Candidate>,

    /// The candidate pairs.
    candidate_pairs: Vec<CandidatePair>,
}

/// States the [`IceAgent`] can be in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IceConnectionState {
    /// The ICE agent is gathering addresses.
    New,

    /// The ICE agent is checking pairs of local and remote candidates against one
    /// another to try to find a compatible match, but has not yet found a pair
    /// which will allow the peer connection to be made. It is possible that
    /// gathering of candidates is also still underway.
    Checking,

    /// A usable pairing of local and remote candidates has been found, and the
    /// connection has been established. The agent is not in `Completed` because
    /// it is still gathering candidates or is still checking candidates against
    /// one another looking for a better connection to use.
    Connected,

    /// The ICE agent has finished gathering candidates, has checked all pairs
    /// against one another, and has found a working connection.
    Completed,

    /// The ICE candidate has checked all candidates pairs against one another and has
    /// failed to find compatible matches.
    Failed,

    /// Connection failed. This is a less stringent test than `failed` and may trigger
    /// intermittently and resolve just as spontaneously on less reliable networks,
    /// or during temporary disconnections. When the problem resolves, the connection
    /// may return to the connected state.
    Disconnected,

    /// The ICE agent has shut down and is no longer handling requests.
    Closed,
}

impl IceAgent {
    pub fn new() -> Self {
        // Username Fragment and Password:  Values used to perform connectivity
        // checks.  The values MUST be unguessable, with at least 128 bits of
        // random number generator output used to generate the password, and
        // at least 24 bits of output to generate the username fragment.
        let username = random_id::<3>().to_string();
        let password = random_id::<16>().to_string();

        IceAgent {
            timing_advance: None,
            ice_lite: false,
            username,
            password,
            controlling: false,
            state: IceConnectionState::New,
            local_candidates: vec![],
            remote_candidates: vec![],
            candidate_pairs: vec![],
        }
    }

    /// The STUN username.
    pub fn username(&self) -> &str {
        &self.username
    }

    /// The STUN password.
    pub fn password(&self) -> &str {
        &self.password
    }

    /// Whether this side is controlling or controlled.
    pub fn controlling(&self) -> bool {
        self.controlling
    }

    /// Set whether we are the controlling side.
    ///
    /// ### Panics
    ///
    /// Panics if we have started running the ice agent.
    pub fn set_controlling(&mut self, v: bool) {
        self.controlling = v;
    }

    /// Current ice agent state.
    pub fn state(&self) -> IceConnectionState {
        self.state
    }

    /// Timing advance (Ta).
    ///
    /// Every time Ta
    /// expires, the agent can generate another new STUN or TURN transaction.
    /// This transaction can be either a retry of a previous transaction that
    /// failed with a recoverable error (such as authentication failure) or a
    /// transaction for a new host candidate and STUN or TURN server pair.
    ///
    /// The agent SHOULD NOT generate transactions more frequently than once
    /// per each ta expiration.
    fn timing_advance(&self) -> Duration {
        self.timing_advance.unwrap_or(Duration::from_millis(50))
    }

    /// Adds a local candidate.
    ///
    /// Returns `false` if the candidate was not added because it is redundant.
    /// Adding loopback addresses or multicast/broadcast addresses causes
    /// an error.
    pub fn add_local_candidate(&mut self, mut c: Candidate) -> bool {
        let ip = c.addr().ip();

        if self.ice_lite {
            // Reject all non-host candidates.
            if c.kind() != CandidateKind::Host {
                return false;
            }
        }

        // https://datatracker.ietf.org/doc/html/rfc8445#section-5.1.2.1
        // The local preference MUST be an integer from 0 (lowest preference) to
        // 65535 (highest preference) inclusive.  When there is only a single IP
        // address, this value SHOULD be set to 65535.  If there are multiple
        // candidates for a particular component for a particular data stream
        // that have the same type, the local preference MUST be unique for each
        // one.
        // ...
        // If an ICE agent is multihomed and has multiple IP addresses, the
        // recommendations in [RFC8421] SHOULD be followed.  If multiple TURN
        // servers are used, local priorities for the candidates obtained from
        // the TURN servers are chosen in a similar fashion as for multihomed
        // local candidates: the local preference value is used to indicate a
        // preference among different servers, but the preference MUST be unique
        // for each one.
        // ================
        //
        // The above presupposes that we know all the candidates when we start
        // the ice agent. That doesn't work for us, so we deliberately do not
        // follow spec. We assign the following intervals for the different
        // types of candidates:
        //
        // 0     - 16384 => relay
        // 16384 - 32768 => srflx
        // 32768 - 49152 => prflx
        // 49152 - 65536 => host
        //
        // And furthermore we subdivide these to interleave IPv6 with IPv4
        // so that odd numbers are ipv6 and even are ipv4.
        //
        // For host candidates this means:
        // 65535 - first ipv6
        // 65534 - first ipv4
        // 65533 - second ipv6
        // 65432 - second ipv4
        let counter_start: u32 = {
            use CandidateKind::*;
            let x = match c.kind() {
                Host => 65_535,
                PeerReflexive => 49_151,
                ServerReflexive => 32_767,
                Relayed => 16_383,
            };
            x - if ip.is_ipv6() { 0 } else { 1 }
        };

        // Count the number of existing candidates of the same kind.
        let same_kind = self
            .local_candidates
            .iter()
            .filter(|v| v.kind() == c.kind())
            .filter(|v| v.addr().is_ipv6() == ip.is_ipv6())
            .count() as u32;

        c.set_local_preference(counter_start - same_kind * 2);

        // A candidate is redundant if and only if its transport address and base equal those
        // of another candidate.  The agent SHOULD eliminate the redundant
        // candidate with the lower priority.
        //
        // NB this must be done _after_ set_local_prefrence(), since the prio() used in the
        // elimination is calculated from that preference.
        if let Some((idx, other)) = self
            .local_candidates
            .iter_mut()
            .enumerate()
            .find(|(_, v)| v.addr() == c.addr() && v.base() == c.base())
        {
            if c.prio() < other.prio() {
                // The new candidate is not better than what we already got.
                return false;
            } else {
                // Stop using the current candidate in favor of the new one.
                other.set_discarded();
                self.discard_candidate_pairs(idx);
            }
        }

        self.local_candidates.push(c);

        true
    }

    /// Adds a local candidate.
    ///
    /// Returns `false` if the candidate was not added because it is redundant.
    /// Adding loopback addresses or multicast/broadcast addresses causes
    /// an error.
    pub fn add_remote_candidate(&mut self, c: Candidate) {}

    /// Invalidate a candidate and remove it from the connection.
    ///
    /// This is done for host candidates disappearing due to changes in the network
    /// interfaces like a WiFi disconnecting or changing IPs.
    ///
    /// Returns `true` if the candidate was found and invalidated.
    pub fn invalidate_candidate(&mut self, c: &Candidate) -> bool {
        if let Some((idx, other)) =
            self.local_candidates.iter_mut().enumerate().find(|(_, v)| {
                v.addr() == c.addr() && v.base() == c.base() && v.raddr() == c.raddr()
            })
        {
            if !other.discarded() {
                other.set_discarded();
                self.discard_candidate_pairs(idx);
                return true;
            }
        }

        false
    }

    /// Discard candidate pairs that contain the candidate identified by a local index.
    fn discard_candidate_pairs(&mut self, local_idx: usize) {
        //
    }
}

#[cfg(test)]
mod test {
    use std::net::SocketAddr;

    use super::*;

    fn ipv4_1() -> SocketAddr {
        "1.2.3.4:5000".parse().unwrap()
    }
    fn ipv4_2() -> SocketAddr {
        "2.3.4.5:5000".parse().unwrap()
    }
    fn ipv6_1() -> SocketAddr {
        "[1001::]:5000".parse().unwrap()
    }
    fn ipv6_2() -> SocketAddr {
        "[1002::]:5000".parse().unwrap()
    }

    #[test]
    fn local_preference_host() {
        let mut agent = IceAgent::new();

        agent.add_local_candidate(Candidate::host(ipv4_1()).unwrap());
        agent.add_local_candidate(Candidate::host(ipv6_1()).unwrap());
        agent.add_local_candidate(Candidate::host(ipv6_2()).unwrap());
        agent.add_local_candidate(Candidate::host(ipv4_2()).unwrap());

        let v: Vec<_> = agent
            .local_candidates
            .iter()
            .map(|c| c.local_preference())
            .collect();

        assert_eq!(v, vec![65534, 65535, 65533, 65532]);
    }

    #[test]
    fn discard_adding_redundant() {
        let mut agent = IceAgent::new();

        // Frequently, a server-reflexive candidate and a host candidate will be
        // redundant when the agent is not behind a NAT.

        let x2 = agent.add_local_candidate(Candidate::host(ipv4_1()).unwrap());
        assert!(x2);

        // this is redundant given we have the direct host candidate above.
        let x1 = agent.add_local_candidate(Candidate::peer_reflexive(ipv4_1(), ipv4_1()));
        assert!(x1 == false);
    }

    #[test]
    fn discard_already_added_redundant() {
        let mut agent = IceAgent::new();

        // Frequently, a server-reflexive candidate and a host candidate will be
        // redundant when the agent is not behind a NAT.

        // this is contrived, but it is redundant when we add the host candidate below.
        let x1 = agent.add_local_candidate(Candidate::peer_reflexive(ipv4_1(), ipv4_1()));
        assert!(x1);

        let x2 = agent.add_local_candidate(Candidate::host(ipv4_1()).unwrap());
        assert!(x2);

        let v: Vec<_> = agent
            .local_candidates
            .iter()
            .map(|v| v.discarded())
            .collect();

        assert_eq!(v, vec![true, false]);
    }
}