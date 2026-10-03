//! Lightweight DNS & Service Discovery for prod-code cluster (*.code.internal).
//!
//! Provides embedded DNS/mDNS resolution, project-to-node mapping, SRV record
//! encoding/decoding (RFC 1035 / RFC 2782), and zero-configuration cluster discovery (Phase 5.2).

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::time::Duration;
use serde::{Deserialize, Serialize};
use crate::discovery::DiscoveredNode;
use crate::messages::content_hash;

/// Default synthetic top-level domain for internal cluster resolution.
pub const CODE_INTERNAL_SUFFIX: &str = ".code.internal";

/// Root internal domain.
pub const CODE_INTERNAL_ROOT: &str = "code.internal";

/// Standard DNS SRV service prefix for prod-code.
pub const SRV_SERVICE_NAME: &str = "_prod-code._tcp";

/// Standard DNS query types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DnsQueryType {
    A,
    TXT,
    SRV,
    ANY,
    Other(u16),
}

impl DnsQueryType {
    pub fn from_u16(val: u16) -> Self {
        match val {
            1 => Self::A,
            16 => Self::TXT,
            33 => Self::SRV,
            255 => Self::ANY,
            other => Self::Other(other),
        }
    }

    pub fn to_u16(self) -> u16 {
        match self {
            Self::A => 1,
            Self::TXT => 16,
            Self::SRV => 33,
            Self::ANY => 255,
            Self::Other(v) => v,
        }
    }
}

/// A parsed DNS Question.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsQuestion {
    pub name: String,
    pub qtype: DnsQueryType,
    pub qclass: u16,
}

/// A DNS SRV record (RFC 2782).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsSrvRecord {
    pub priority: u16,
    pub weight: u16,
    pub port: u16,
    pub target: String,
}

/// A DNS Answer record.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DnsRecordData {
    A(Ipv4Addr),
    SRV(DnsSrvRecord),
    TXT(String),
}

/// A full DNS Answer entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DnsAnswer {
    pub name: String,
    pub ttl: u32,
    pub data: DnsRecordData,
}

/// Checks whether a hostname or domain string targets the `.code.internal` virtual domain.
pub fn is_code_internal_domain(domain: &str) -> bool {
    let lower = domain.trim().to_ascii_lowercase();
    let host = lower.split(':').next().unwrap_or(&lower);
    host == CODE_INTERNAL_ROOT
        || host.ends_with(CODE_INTERNAL_SUFFIX)
        || host.ends_with(".code.local")
        || host.starts_with(SRV_SERVICE_NAME)
}

/// Extracts the target project name from a `.code.internal` domain name.
/// E.g. "shop.code.internal" -> Some("shop"), "shop.code.internal:9400" -> Some("shop"),
/// "cluster.code.internal" -> Some("cluster").
pub fn extract_project_name(domain: &str) -> Option<&str> {
    let trimmed = domain.trim();
    let host = trimmed.split(':').next().unwrap_or(trimmed);
    if host.eq_ignore_ascii_case(CODE_INTERNAL_ROOT) {
        return Some("cluster");
    }
    if let Some(prefix) = host.strip_suffix(CODE_INTERNAL_SUFFIX) {
        return Some(prefix);
    }
    if let Some(prefix) = host.strip_suffix(".code.local") {
        return Some(prefix);
    }
    None
}

/// Maps a project name to its designated server node using warm workspace inspection
/// and deterministic rendezvous hashing (Phase 5.2).
pub fn resolve_project_node<'a>(project: &str, nodes: &'a [DiscoveredNode]) -> Option<&'a DiscoveredNode> {
    if nodes.is_empty() {
        return None;
    }
    let proj_clean = project.trim();

    // 1. Prefer a node that already has the exact workspace loaded
    let warm_node = nodes.iter().find(|n| {
        n.workspaces
            .iter()
            .any(|w| w.name.eq_ignore_ascii_case(proj_clean))
    });
    if let Some(n) = warm_node {
        return Some(n);
    }

    // 2. Prefer a node that has a workspace starting with the project prefix (e.g. repo--wt-123)
    let prefix_node = nodes.iter().find(|n| {
        n.workspaces
            .iter()
            .any(|w| w.name.to_ascii_lowercase().starts_with(&proj_clean.to_ascii_lowercase()))
    });
    if let Some(n) = prefix_node {
        return Some(n);
    }

    // 3. Fallback to deterministic rendezvous hashing over available nodes
    nodes.iter().max_by_key(|n| {
        let key = format!("{}:{}", proj_clean, n.addr);
        content_hash(key.as_bytes())
    })
}

/// Resolves a domain string (such as `shop.code.internal` or `cluster.code.internal:9400`)
/// to physical `SocketAddr` endpoints using cluster discovery and DNS mapping.
pub fn resolve_smart_domain(
    domain: &str,
    nodes: &[DiscoveredNode],
    default_port: u16,
) -> Option<Vec<SocketAddr>> {
    let trimmed = domain.trim();
    let (host, port) = match trimmed.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().unwrap_or(default_port)),
        None => (trimmed, default_port),
    };

    if !is_code_internal_domain(host) {
        return None;
    }

    if nodes.is_empty() {
        // If no nodes discovered yet, fallback to loopback with requested port
        return Some(vec![SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), port)]);
    }

    let project = extract_project_name(host).unwrap_or(host);

    if project.eq_ignore_ascii_case("cluster")
        || project.eq_ignore_ascii_case("all")
        || host.starts_with(SRV_SERVICE_NAME)
    {
        // Return all known nodes, using node's own port if port was default
        let addrs: Vec<SocketAddr> = nodes
            .iter()
            .map(|n| {
                let p = if port == default_port { n.addr.port() } else { port };
                SocketAddr::new(n.addr.ip(), p)
            })
            .collect();
        return Some(addrs);
    }

    // Check if host matches a specific node IP (e.g. "node-192-168-2-10.code.internal")
    for node in nodes {
        let ip_slug = node.addr.ip().to_string().replace('.', "-");
        if project.contains(&ip_slug) {
            let p = if port == default_port { node.addr.port() } else { port };
            return Some(vec![SocketAddr::new(node.addr.ip(), p)]);
        }
    }

    // Resolve designated project node via warm check & rendezvous hashing
    if let Some(node) = resolve_project_node(project, nodes) {
        let p = if port == default_port { node.addr.port() } else { port };
        return Some(vec![SocketAddr::new(node.addr.ip(), p)]);
    }

    None
}

/// Generates dynamic DNS SRV records for all active cluster daemon instances (RFC 2782).
pub fn generate_srv_records(nodes: &[DiscoveredNode]) -> Vec<DnsSrvRecord> {
    let mut records = Vec::with_capacity(nodes.len());
    for (i, node) in nodes.iter().enumerate() {
        // Priority: lower number = higher priority. Scale with load: nodes with low load get priority 10.
        let priority = if node.load_per_cpu < 0.5 { 10 } else { 20 };
        // Weight: proportional to available memory (min 10)
        let weight = ((node.mem_avail_mb / 1024).clamp(10, 1000)) as u16;
        let target = format!("node-{}.code.internal", i + 1);
        records.push(DnsSrvRecord {
            priority,
            weight,
            port: node.addr.port(),
            target,
        });
    }
    records
}

// ── Wire-Format DNS Encoding & Decoding (RFC 1035 / RFC 2782) ─────────────────

/// Encodes a dotted domain name into DNS wire format (`\x04shop\x04code\x08internal\x00`).
pub fn encode_dns_name(name: &str) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(name.len() + 2);
    for label in name.trim_matches('.').split('.') {
        if label.is_empty() {
            continue;
        }
        let len = label.len().min(63) as u8;
        bytes.push(len);
        bytes.extend_from_slice(&label.as_bytes()[..len as usize]);
    }
    bytes.push(0); // Zero root label
    bytes
}

/// Decodes a DNS wire-format name starting at `offset`. Supports label sequences without compression.
pub fn decode_dns_name(bytes: &[u8], offset: &mut usize) -> Result<String, String> {
    let mut labels = Vec::new();
    let mut jumped = false;
    let mut current = *offset;
    let mut depth = 0;

    loop {
        if depth > 10 {
            return Err("DNS label compression pointer loop".to_string());
        }
        if current >= bytes.len() {
            return Err("unexpected EOF reading DNS name".to_string());
        }
        let len = bytes[current];
        if len == 0 {
            if !jumped {
                *offset = current + 1;
            }
            break;
        }

        // Pointer (top 2 bits set: 11xxxxxx)
        if len & 0xC0 == 0xC0 {
            if current + 1 >= bytes.len() {
                return Err("truncated DNS pointer".to_string());
            }
            let ptr = (((len & 0x3F) as usize) << 8) | (bytes[current + 1] as usize);
            if !jumped {
                *offset = current + 2;
                jumped = true;
            }
            current = ptr;
            depth += 1;
            continue;
        }

        current += 1;
        let label_len = len as usize;
        if current + label_len > bytes.len() {
            return Err("truncated DNS label".to_string());
        }
        let label_str = std::str::from_utf8(&bytes[current..current + label_len])
            .map_err(|e| format!("invalid UTF-8 in DNS label: {e}"))?;
        labels.push(label_str);
        current += label_len;
        if !jumped {
            *offset = current;
        }
    }

    Ok(labels.join("."))
}

/// Parses an incoming DNS query datagram.
pub fn parse_dns_query(buf: &[u8]) -> Result<(u16, DnsQuestion), String> {
    if buf.len() < 12 {
        return Err("DNS packet too short for header".to_string());
    }
    let id = u16::from_be_bytes([buf[0], buf[1]]);
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]);
    if qdcount == 0 {
        return Err("DNS query has 0 questions".to_string());
    }

    let mut offset = 12;
    let name = decode_dns_name(buf, &mut offset)?;
    if offset + 4 > buf.len() {
        return Err("truncated DNS question fields".to_string());
    }
    let qtype = u16::from_be_bytes([buf[offset], buf[offset + 1]]);
    let qclass = u16::from_be_bytes([buf[offset + 2], buf[offset + 3]]);

    Ok((
        id,
        DnsQuestion {
            name,
            qtype: DnsQueryType::from_u16(qtype),
            qclass,
        },
    ))
}

/// Formats a DNS response datagram.
pub fn format_dns_response(
    id: u16,
    question: &DnsQuestion,
    answers: &[DnsAnswer],
    authoritative: bool,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(512);

    // Header (12 bytes)
    out.extend_from_slice(&id.to_be_bytes()); // ID
    let mut flags: u16 = 0x8180; // Standard query response, Recursion available
    if authoritative {
        flags |= 0x0400; // Authoritative Answer bit
    }
    if answers.is_empty() {
        flags |= 0x0003; // NXDOMAIN if no answers
    }
    out.extend_from_slice(&flags.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    out.extend_from_slice(&(answers.len() as u16).to_be_bytes()); // ANCOUNT
    out.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT = 0
    out.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT = 0

    // Question Section
    out.extend_from_slice(&encode_dns_name(&question.name));
    out.extend_from_slice(&question.qtype.to_u16().to_be_bytes());
    out.extend_from_slice(&question.qclass.to_be_bytes());

    // Answer Section
    for ans in answers {
        out.extend_from_slice(&encode_dns_name(&ans.name));
        match &ans.data {
            DnsRecordData::A(ip) => {
                out.extend_from_slice(&1u16.to_be_bytes()); // TYPE A = 1
                out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN = 1
                out.extend_from_slice(&ans.ttl.to_be_bytes());
                out.extend_from_slice(&4u16.to_be_bytes()); // RDLENGTH = 4
                out.extend_from_slice(&ip.octets());
            }
            DnsRecordData::SRV(srv) => {
                out.extend_from_slice(&33u16.to_be_bytes()); // TYPE SRV = 33
                out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN = 1
                out.extend_from_slice(&ans.ttl.to_be_bytes());
                let target_bytes = encode_dns_name(&srv.target);
                let rdlength = (6 + target_bytes.len()) as u16;
                out.extend_from_slice(&rdlength.to_be_bytes());
                out.extend_from_slice(&srv.priority.to_be_bytes());
                out.extend_from_slice(&srv.weight.to_be_bytes());
                out.extend_from_slice(&srv.port.to_be_bytes());
                out.extend_from_slice(&target_bytes);
            }
            DnsRecordData::TXT(txt) => {
                out.extend_from_slice(&16u16.to_be_bytes()); // TYPE TXT = 16
                out.extend_from_slice(&1u16.to_be_bytes()); // CLASS IN = 1
                out.extend_from_slice(&ans.ttl.to_be_bytes());
                let bytes = txt.as_bytes();
                let len = bytes.len().min(255) as u8;
                out.extend_from_slice(&((len as u16) + 1).to_be_bytes());
                out.push(len);
                out.extend_from_slice(&bytes[..len as usize]);
            }
        }
    }

    out
}

/// Handles an incoming DNS query over UDP, resolving `*.code.internal` or SRV records
/// and generating a standard DNS response packet.
pub fn handle_dns_packet(buf: &[u8], nodes: &[DiscoveredNode]) -> Option<Vec<u8>> {
    let (id, question) = parse_dns_query(buf).ok()?;
    if !is_code_internal_domain(&question.name) {
        return None;
    }

    let mut answers = Vec::new();
    let ttl = 60; // 60s TTL for dynamic discovery

    match question.qtype {
        DnsQueryType::A | DnsQueryType::ANY => {
            if let Some(addrs) = resolve_smart_domain(&question.name, nodes, 9400) {
                for addr in addrs {
                    if let IpAddr::V4(ipv4) = addr.ip() {
                        answers.push(DnsAnswer {
                            name: question.name.clone(),
                            ttl,
                            data: DnsRecordData::A(ipv4),
                        });
                    }
                }
            }
        }
        DnsQueryType::SRV => {
            let srvs = generate_srv_records(nodes);
            for srv in srvs {
                answers.push(DnsAnswer {
                    name: question.name.clone(),
                    ttl,
                    data: DnsRecordData::SRV(srv),
                });
            }
        }
        DnsQueryType::TXT => {
            let count = nodes.len();
            let txt = format!("prod-code-cluster: nodes={count} domain=code.internal");
            answers.push(DnsAnswer {
                name: question.name.clone(),
                ttl,
                data: DnsRecordData::TXT(txt),
            });
        }
        _ => {}
    }

    Some(format_dns_response(id, &question, &answers, true))
}

/// Queries a DNS server over UDP for a specific name and query type.
pub fn query_dns(
    server: SocketAddr,
    domain: &str,
    qtype: DnsQueryType,
    timeout: Duration,
) -> std::io::Result<Vec<DnsAnswer>> {
    let sock = std::net::UdpSocket::bind("0.0.0.0:0")?;
    sock.set_read_timeout(Some(timeout))?;

    let id: u16 = 0x4242;

    let mut query = Vec::with_capacity(64);
    query.extend_from_slice(&id.to_be_bytes());
    query.extend_from_slice(&0x0100u16.to_be_bytes()); // Standard query with recursion
    query.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&0u16.to_be_bytes());
    query.extend_from_slice(&encode_dns_name(domain));
    query.extend_from_slice(&qtype.to_u16().to_be_bytes());
    query.extend_from_slice(&1u16.to_be_bytes());

    sock.send_to(&query, server)?;

    let mut buf = [0u8; 1024];
    let (n, _) = sock.recv_from(&mut buf)?;
    let resp = &buf[..n];
    if resp.len() < 12 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "DNS reply too short",
        ));
    }

    let ancount = u16::from_be_bytes([resp[6], resp[7]]) as usize;
    let mut offset = 12;
    // Skip question
    let _ = decode_dns_name(resp, &mut offset).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e)
    })?;
    offset += 4; // QTYPE + QCLASS

    let mut answers = Vec::with_capacity(ancount);
    for _ in 0..ancount {
        if offset >= resp.len() {
            break;
        }
        let ans_name = decode_dns_name(resp, &mut offset).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e)
        })?;
        if offset + 10 > resp.len() {
            break;
        }
        let atype = u16::from_be_bytes([resp[offset], resp[offset + 1]]);
        let ttl = u32::from_be_bytes([
            resp[offset + 4],
            resp[offset + 5],
            resp[offset + 6],
            resp[offset + 7],
        ]);
        let rdlength = u16::from_be_bytes([resp[offset + 8], resp[offset + 9]]) as usize;
        offset += 10;

        if offset + rdlength > resp.len() {
            break;
        }

        match DnsQueryType::from_u16(atype) {
            DnsQueryType::A if rdlength == 4 => {
                let ip = Ipv4Addr::new(
                    resp[offset],
                    resp[offset + 1],
                    resp[offset + 2],
                    resp[offset + 3],
                );
                answers.push(DnsAnswer {
                    name: ans_name,
                    ttl,
                    data: DnsRecordData::A(ip),
                });
            }
            DnsQueryType::SRV if rdlength >= 6 => {
                let prio = u16::from_be_bytes([resp[offset], resp[offset + 1]]);
                let weight = u16::from_be_bytes([resp[offset + 2], resp[offset + 3]]);
                let port = u16::from_be_bytes([resp[offset + 4], resp[offset + 5]]);
                let mut target_off = offset + 6;
                if let Ok(target) = decode_dns_name(resp, &mut target_off) {
                    answers.push(DnsAnswer {
                        name: ans_name,
                        ttl,
                        data: DnsRecordData::SRV(DnsSrvRecord {
                            priority: prio,
                            weight,
                            port,
                            target,
                        }),
                    });
                }
            }
            DnsQueryType::TXT if rdlength > 0 => {
                let txt_len = resp[offset] as usize;
                if offset + 1 + txt_len <= resp.len() {
                    let text = String::from_utf8_lossy(&resp[offset + 1..offset + 1 + txt_len]).to_string();
                    answers.push(DnsAnswer {
                        name: ans_name,
                        ttl,
                        data: DnsRecordData::TXT(text),
                    });
                }
            }
            _ => {}
        }
        offset += rdlength;
    }

    Ok(answers)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::LoadedWorkspace;

    fn mock_node(addr: &str, workspaces: &[(&str, &str, u32)]) -> DiscoveredNode {
        DiscoveredNode {
            addr: addr.parse().unwrap(),
            engines: vec!["rust".into(), "go".into()],
            rss_mb: 1024,
            load_per_cpu: 0.25,
            cpus: 16,
            mem_total_mb: 64000,
            mem_avail_mb: 32000,
            sessions: 2,
            workspaces: workspaces
                .iter()
                .map(|(n, e, s)| LoadedWorkspace {
                    name: (*n).into(),
                    engine: (*e).into(),
                    sessions: *s,
                })
                .collect(),
            nonce: None,
        }
    }

    #[test]
    fn test_is_code_internal_domain() {
        assert!(is_code_internal_domain("code.internal"));
        assert!(is_code_internal_domain("shop.code.internal"));
        assert!(is_code_internal_domain("billing.code.internal:9400"));
        assert!(is_code_internal_domain("_prod-code._tcp.code.internal"));
        assert!(!is_code_internal_domain("github.com"));
        assert!(!is_code_internal_domain("localhost:9400"));
    }

    #[test]
    fn test_extract_project_name() {
        assert_eq!(extract_project_name("shop.code.internal"), Some("shop"));
        assert_eq!(extract_project_name("billing.code.internal:9400"), Some("billing"));
        assert_eq!(extract_project_name("cluster.code.internal"), Some("cluster"));
        assert_eq!(extract_project_name("code.internal"), Some("cluster"));
        assert_eq!(extract_project_name("example.com"), None);
    }

    #[test]
    fn test_resolve_project_node_warm_preference() {
        let node1 = mock_node("192.168.2.10:9400", &[("shop", "rust", 1)]);
        let node2 = mock_node("192.168.2.20:9400", &[("billing", "go", 2)]);
        let nodes = vec![node1, node2];

        let resolved_shop = resolve_project_node("shop", &nodes).expect("shop node");
        assert_eq!(resolved_shop.addr, "192.168.2.10:9400".parse().unwrap());

        let resolved_billing = resolve_project_node("billing", &nodes).expect("billing node");
        assert_eq!(resolved_billing.addr, "192.168.2.20:9400".parse().unwrap());

        // Unloaded project resolves deterministically via rendezvous hashing
        let resolved_other = resolve_project_node("analytics", &nodes).expect("analytics node");
        assert!(resolved_other.addr == "192.168.2.10:9400".parse().unwrap() || resolved_other.addr == "192.168.2.20:9400".parse().unwrap());
    }

    #[test]
    fn test_resolve_smart_domain_mapping() {
        let node1 = mock_node("192.168.2.10:9400", &[("shop", "rust", 1)]);
        let node2 = mock_node("192.168.2.20:9400", &[("billing", "go", 2)]);
        let nodes = vec![node1, node2];

        // Direct project domain
        let addrs = resolve_smart_domain("shop.code.internal", &nodes, 9400).expect("resolved");
        assert_eq!(addrs, vec!["192.168.2.10:9400".parse().unwrap()]);

        // Explicit port override
        let addrs_custom_port = resolve_smart_domain("shop.code.internal:9443", &nodes, 9400).expect("resolved");
        assert_eq!(addrs_custom_port, vec!["192.168.2.10:9443".parse().unwrap()]);

        // Cluster domain returns all nodes
        let cluster_addrs = resolve_smart_domain("cluster.code.internal", &nodes, 9400).expect("cluster");
        assert_eq!(cluster_addrs.len(), 2);
    }

    #[test]
    fn test_dns_wire_encode_decode_round_trip() {
        let question = DnsQuestion {
            name: "shop.code.internal".to_string(),
            qtype: DnsQueryType::A,
            qclass: 1,
        };
        let answers = vec![
            DnsAnswer {
                name: "shop.code.internal".to_string(),
                ttl: 60,
                data: DnsRecordData::A(Ipv4Addr::new(192, 168, 2, 10)),
            }
        ];

        let packet = format_dns_response(1234, &question, &answers, true);
        assert!(!packet.is_empty());

        let (parsed_id, parsed_q) = parse_dns_query(&packet).expect("parse query");
        assert_eq!(parsed_id, 1234);
        assert_eq!(parsed_q.name, "shop.code.internal");
        assert_eq!(parsed_q.qtype, DnsQueryType::A);
    }

    #[test]
    fn test_handle_dns_packet_srv_and_a() {
        let node = mock_node("192.168.2.15:9400", &[("api-gateway", "rust", 1)]);
        let nodes = vec![node];

        // A query
        let query_a = format_dns_response(555, &DnsQuestion {
            name: "api-gateway.code.internal".into(),
            qtype: DnsQueryType::A,
            qclass: 1,
        }, &[], false);

        let resp_a = handle_dns_packet(&query_a, &nodes).expect("resp A");
        assert!(!resp_a.is_empty());

        // SRV query
        let query_srv = format_dns_response(666, &DnsQuestion {
            name: "_prod-code._tcp.code.internal".into(),
            qtype: DnsQueryType::SRV,
            qclass: 1,
        }, &[], false);

        let resp_srv = handle_dns_packet(&query_srv, &nodes).expect("resp SRV");
        assert!(!resp_srv.is_empty());
    }

    #[test]
    fn test_generate_srv_records_weights_and_priority() {
        let mut node1 = mock_node("192.168.2.11:9400", &[]);
        node1.load_per_cpu = 0.1;
        node1.mem_avail_mb = 64 * 1024; // 64 GB

        let mut node2 = mock_node("192.168.2.12:9400", &[]);
        node2.load_per_cpu = 0.9;
        node2.mem_avail_mb = 16 * 1024; // 16 GB

        let srvs = generate_srv_records(&[node1, node2]);
        assert_eq!(srvs.len(), 2);

        // Node 1 (low load) has priority 10, weight 64
        assert_eq!(srvs[0].priority, 10);
        assert_eq!(srvs[0].weight, 64);
        assert_eq!(srvs[0].port, 9400);
        assert_eq!(srvs[0].target, "node-1.code.internal");

        // Node 2 (high load) has priority 20, weight 16
        assert_eq!(srvs[1].priority, 20);
        assert_eq!(srvs[1].weight, 16);
        assert_eq!(srvs[1].port, 9400);
        assert_eq!(srvs[1].target, "node-2.code.internal");
    }

    #[test]
    fn test_dns_records_serialization() {
        let srv = DnsSrvRecord {
            priority: 10,
            weight: 64,
            port: 9400,
            target: "node-1.code.internal".to_string(),
        };
        let serialized = serde_json::to_string(&srv).expect("serialize srv");
        let deserialized: DnsSrvRecord = serde_json::from_str(&serialized).expect("deserialize srv");
        assert_eq!(srv, deserialized);
    }

    #[test]
    fn test_resolve_smart_domain_empty_nodes_fallback() {
        let addrs = resolve_smart_domain("shop.code.internal:9400", &[], 9400).expect("fallback");
        assert_eq!(addrs, vec!["127.0.0.1:9400".parse().unwrap()]);
    }
}
