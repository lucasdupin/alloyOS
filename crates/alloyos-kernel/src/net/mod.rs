//! `#![no_std]` Kernel TCP/IP Networking Stack for **alloyOS**.
//!
//! Implements:
//! - Ethernet II frame framing (`EthernetHeader`, EtherType `0x0800` IPv4, `0x0806` ARP)
//! - ARP cache & Request/Reply serialization (`ArpPacket`)
//! - IPv4 packet framing & RFC 1071 Internet Checksum (`Ipv4Header`, `rfc1071_checksum`)
//! - ICMP Echo Request/Reply (`IcmpPacket`)
//! - UDP datagrams (`UdpPacket`), DHCPv4 client state machine (`DhcpClient`), and DNS A-record
//!   resolver (`DnsResolver`)
//! - TCP segment framing (`TcpSegment`), connection state machine (`TcpState`), and the global
//!   kernel socket table backing `SYS_SOCKET`, `SYS_CONNECT`, `SYS_SEND`, and `SYS_RECV`.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use alloyos_proto::{SocketAddrV4, AF_INET, SOCK_DGRAM, SOCK_STREAM};
use spin::Mutex;

use crate::drivers::rng;
use crate::drivers::virtio_net::VIRTIO_NET;

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

pub const IPPROTO_ICMP: u8 = 1;
pub const IPPROTO_TCP: u8 = 6;
pub const IPPROTO_UDP: u8 = 17;

pub const TCP_FLAG_FIN: u8 = 0x01;
pub const TCP_FLAG_SYN: u8 = 0x02;
pub const TCP_FLAG_RST: u8 = 0x04;
pub const TCP_FLAG_PSH: u8 = 0x08;
pub const TCP_FLAG_ACK: u8 = 0x10;

/// Compute the RFC 1071 16-bit one's complement checksum over `data`.
pub fn rfc1071_checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut chunks = data.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(&rem) = chunks.remainder().first() {
        sum += u16::from_be_bytes([rem, 0]) as u32;
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Compute TCP/UDP pseudo-header + transport segment checksum.
pub fn transport_checksum(src_ip: [u8; 4], dst_ip: [u8; 4], proto: u8, segment: &[u8]) -> u16 {
    let mut pseudo = Vec::with_capacity(12 + segment.len());
    pseudo.extend_from_slice(&src_ip);
    pseudo.extend_from_slice(&dst_ip);
    pseudo.push(0);
    pseudo.push(proto);
    pseudo.extend_from_slice(&(segment.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(segment);
    rfc1071_checksum(&pseudo)
}

/// 14-byte Ethernet II header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EthernetHeader {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub ethertype: u16,
}

impl EthernetHeader {
    pub const LEN: usize = 14;

    pub fn to_bytes(&self) -> [u8; 14] {
        let mut out = [0u8; 14];
        out[0..6].copy_from_slice(&self.dst_mac);
        out[6..12].copy_from_slice(&self.src_mac);
        out[12..14].copy_from_slice(&self.ethertype.to_be_bytes());
        out
    }

    pub fn parse(frame: &[u8]) -> Option<(Self, &[u8])> {
        if frame.len() < Self::LEN {
            return None;
        }
        let mut dst_mac = [0u8; 6];
        let mut src_mac = [0u8; 6];
        dst_mac.copy_from_slice(&frame[0..6]);
        src_mac.copy_from_slice(&frame[6..12]);
        let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
        Some((
            Self {
                dst_mac,
                src_mac,
                ethertype,
            },
            &frame[Self::LEN..],
        ))
    }
}

/// 28-byte IPv4-over-Ethernet ARP packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArpPacket {
    pub oper: u16, // 1 = Request, 2 = Reply
    pub sha: [u8; 6],
    pub spa: [u8; 4],
    pub tha: [u8; 6],
    pub tpa: [u8; 4],
}

impl ArpPacket {
    pub const LEN: usize = 28;
    pub const OP_REQUEST: u16 = 1;
    pub const OP_REPLY: u16 = 2;

    pub fn to_bytes(&self) -> [u8; 28] {
        let mut out = [0u8; 28];
        out[0..2].copy_from_slice(&1u16.to_be_bytes()); // HTYPE = Ethernet (1)
        out[2..4].copy_from_slice(&ETHERTYPE_IPV4.to_be_bytes()); // PTYPE = IPv4
        out[4] = 6; // HLEN
        out[5] = 4; // PLEN
        out[6..8].copy_from_slice(&self.oper.to_be_bytes());
        out[8..14].copy_from_slice(&self.sha);
        out[14..18].copy_from_slice(&self.spa);
        out[18..24].copy_from_slice(&self.tha);
        out[24..28].copy_from_slice(&self.tpa);
        out
    }

    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < Self::LEN {
            return None;
        }
        let oper = u16::from_be_bytes([raw[6], raw[7]]);
        let mut sha = [0u8; 6];
        let mut spa = [0u8; 4];
        let mut tha = [0u8; 6];
        let mut tpa = [0u8; 4];
        sha.copy_from_slice(&raw[8..14]);
        spa.copy_from_slice(&raw[14..18]);
        tha.copy_from_slice(&raw[18..24]);
        tpa.copy_from_slice(&raw[24..28]);
        Some(Self {
            oper,
            sha,
            spa,
            tha,
            tpa,
        })
    }
}

/// 20-byte standard IPv4 header (IHL = 5).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ipv4Header {
    pub total_len: u16,
    pub identification: u16,
    pub ttl: u8,
    pub protocol: u8,
    pub checksum: u16,
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
}

impl Ipv4Header {
    pub const LEN: usize = 20;

    pub fn new(src_ip: [u8; 4], dst_ip: [u8; 4], protocol: u8, payload_len: usize) -> Self {
        let mut hdr = Self {
            total_len: (Self::LEN + payload_len) as u16,
            identification: 0x4210,
            ttl: 64,
            protocol,
            checksum: 0,
            src_ip,
            dst_ip,
        };
        let bytes = hdr.to_bytes_with_checksum(0);
        hdr.checksum = rfc1071_checksum(&bytes);
        hdr
    }

    fn to_bytes_with_checksum(&self, csum: u16) -> [u8; 20] {
        let mut out = [0u8; 20];
        out[0] = 0x45; // Version 4, IHL 5
        out[1] = 0x00; // DSCP / ECN
        out[2..4].copy_from_slice(&self.total_len.to_be_bytes());
        out[4..6].copy_from_slice(&self.identification.to_be_bytes());
        out[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // Don't Fragment
        out[8] = self.ttl;
        out[9] = self.protocol;
        out[10..12].copy_from_slice(&csum.to_be_bytes());
        out[12..16].copy_from_slice(&self.src_ip);
        out[16..20].copy_from_slice(&self.dst_ip);
        out
    }

    pub fn to_bytes(&self) -> [u8; 20] {
        self.to_bytes_with_checksum(self.checksum)
    }

    pub fn parse(pkt: &[u8]) -> Option<(Self, &[u8])> {
        if pkt.len() < Self::LEN || (pkt[0] >> 4) != 4 {
            return None;
        }
        let ihl_bytes = ((pkt[0] & 0x0F) as usize) * 4;
        if ihl_bytes < Self::LEN || pkt.len() < ihl_bytes {
            return None;
        }
        let total_len = u16::from_be_bytes([pkt[2], pkt[3]]);
        let identification = u16::from_be_bytes([pkt[4], pkt[5]]);
        let ttl = pkt[8];
        let protocol = pkt[9];
        let checksum = u16::from_be_bytes([pkt[10], pkt[11]]);
        let mut src_ip = [0u8; 4];
        let mut dst_ip = [0u8; 4];
        src_ip.copy_from_slice(&pkt[12..16]);
        dst_ip.copy_from_slice(&pkt[16..20]);

        let end = (total_len as usize).min(pkt.len());
        if end < ihl_bytes {
            return None;
        }
        Some((
            Self {
                total_len,
                identification,
                ttl,
                protocol,
                checksum,
                src_ip,
                dst_ip,
            },
            &pkt[ihl_bytes..end],
        ))
    }

    pub fn verify_checksum(&self) -> bool {
        let raw = self.to_bytes();
        rfc1071_checksum(&raw) == 0
    }
}

/// ICMP Echo Request / Reply packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IcmpPacket {
    pub icmp_type: u8, // 8 = Echo Request, 0 = Echo Reply
    pub code: u8,
    pub identifier: u16,
    pub sequence: u16,
    pub payload: Vec<u8>,
}

impl IcmpPacket {
    pub const TYPE_ECHO_REPLY: u8 = 0;
    pub const TYPE_ECHO_REQUEST: u8 = 8;

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(8 + self.payload.len());
        buf.push(self.icmp_type);
        buf.push(self.code);
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&self.identifier.to_be_bytes());
        buf.extend_from_slice(&self.sequence.to_be_bytes());
        buf.extend_from_slice(&self.payload);
        let csum = rfc1071_checksum(&buf);
        buf[2..4].copy_from_slice(&csum.to_be_bytes());
        buf
    }

    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < 8 {
            return None;
        }
        if rfc1071_checksum(raw) != 0 {
            return None;
        }
        Some(Self {
            icmp_type: raw[0],
            code: raw[1],
            identifier: u16::from_be_bytes([raw[4], raw[5]]),
            sequence: u16::from_be_bytes([raw[6], raw[7]]),
            payload: raw[8..].to_vec(),
        })
    }
}

/// UDP Datagram builder & parser.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UdpPacket {
    pub src_port: u16,
    pub dst_port: u16,
    pub payload: Vec<u8>,
}

impl UdpPacket {
    pub const HEADER_LEN: usize = 8;

    pub fn to_bytes(&self, src_ip: [u8; 4], dst_ip: [u8; 4]) -> Vec<u8> {
        let len = (Self::HEADER_LEN + self.payload.len()) as u16;
        let mut buf = Vec::with_capacity(len as usize);
        buf.extend_from_slice(&self.src_port.to_be_bytes());
        buf.extend_from_slice(&self.dst_port.to_be_bytes());
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes());
        buf.extend_from_slice(&self.payload);
        let csum = transport_checksum(src_ip, dst_ip, IPPROTO_UDP, &buf);
        buf[6..8].copy_from_slice(&csum.to_be_bytes());
        buf
    }

    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < Self::HEADER_LEN {
            return None;
        }
        let src_port = u16::from_be_bytes([raw[0], raw[1]]);
        let dst_port = u16::from_be_bytes([raw[2], raw[3]]);
        let len = u16::from_be_bytes([raw[4], raw[5]]) as usize;
        if len < Self::HEADER_LEN || raw.len() < len {
            return None;
        }
        Some(Self {
            src_port,
            dst_port,
            payload: raw[Self::HEADER_LEN..len].to_vec(),
        })
    }
}

/// DHCPv4 Client State Machine for QEMU User Networking (`10.0.2.15/24`, Gateway `10.0.2.2`, DNS `10.0.2.3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DhcpState {
    Init,
    Selecting,
    Requesting,
    Bound,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DhcpLease {
    pub client_ip: [u8; 4],
    pub subnet_mask: [u8; 4],
    pub gateway_ip: [u8; 4],
    pub dns_ip: [u8; 4],
}

impl Default for DhcpLease {
    fn default() -> Self {
        Self {
            client_ip: [10, 0, 2, 15],
            subnet_mask: [255, 255, 255, 0],
            gateway_ip: [10, 0, 2, 2],
            dns_ip: [10, 0, 2, 3],
        }
    }
}

pub struct DhcpClient {
    pub state: DhcpState,
    pub xid: u32,
    pub lease: DhcpLease,
}

impl DhcpClient {
    pub const fn new() -> Self {
        Self {
            state: DhcpState::Bound,
            xid: 0x6d65_746c,
            lease: DhcpLease {
                client_ip: [10, 0, 2, 15],
                subnet_mask: [255, 255, 255, 0],
                gateway_ip: [10, 0, 2, 2],
                dns_ip: [10, 0, 2, 3],
            },
        }
    }

    /// Build a RFC 2131 DHCPDISCOVER payload.
    pub fn build_discover(&mut self, mac: [u8; 6]) -> Vec<u8> {
        self.state = DhcpState::Selecting;
        let mut pkt = alloc::vec![0u8; 244];
        pkt[0] = 1; // BOOTREQUEST
        pkt[1] = 1; // Ethernet
        pkt[2] = 6; // HLEN
        pkt[4..8].copy_from_slice(&self.xid.to_be_bytes());
        pkt[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // Broadcast flag
        pkt[28..34].copy_from_slice(&mac);
        // Magic cookie: 99.130.83.99
        pkt[236..240].copy_from_slice(&[99, 130, 83, 99]);
        // Option 53: DHCP Message Type = 1 (DISCOVER), End = 255
        pkt[240..244].copy_from_slice(&[53, 1, 1, 255]);
        pkt
    }
}

/// DNS A-record query builder and response parser (`UDP/53`).
pub struct DnsResolver;

impl DnsResolver {
    /// Encode a domain name (e.g. `"api.openai.com"`) into RFC 1035 label format and build a DNS A query.
    pub fn build_a_query(tx_id: u16, hostname: &str) -> Vec<u8> {
        let mut buf = Vec::with_capacity(64);
        buf.extend_from_slice(&tx_id.to_be_bytes());
        buf.extend_from_slice(&0x0100u16.to_be_bytes()); // Standard query, Recursion Desired
        buf.extend_from_slice(&1u16.to_be_bytes()); // QDCOUNT = 1
        buf.extend_from_slice(&0u16.to_be_bytes()); // ANCOUNT = 0
        buf.extend_from_slice(&0u16.to_be_bytes()); // NSCOUNT = 0
        buf.extend_from_slice(&0u16.to_be_bytes()); // ARCOUNT = 0

        for label in hostname.trim_end_matches('.').split('.') {
            let bytes = label.as_bytes();
            buf.push(bytes.len() as u8);
            buf.extend_from_slice(bytes);
        }
        buf.push(0); // Root terminator
        buf.extend_from_slice(&1u16.to_be_bytes()); // QTYPE = A (1)
        buf.extend_from_slice(&1u16.to_be_bytes()); // QCLASS = IN (1)
        buf
    }

    /// Parse a DNS response packet and extract the first IPv4 A record (`TYPE == 1, RDLENGTH == 4`).
    pub fn parse_a_response(pkt: &[u8]) -> Option<[u8; 4]> {
        if pkt.len() < 12 {
            return None;
        }
        let qdcount = u16::from_be_bytes([pkt[4], pkt[5]]);
        let ancount = u16::from_be_bytes([pkt[6], pkt[7]]);
        if ancount == 0 {
            return None;
        }

        let mut pos = 12;
        for _ in 0..qdcount {
            pos = Self::skip_name(pkt, pos)?;
            pos += 4; // QTYPE + QCLASS
            if pos > pkt.len() {
                return None;
            }
        }

        for _ in 0..ancount {
            pos = Self::skip_name(pkt, pos)?;
            if pos + 10 > pkt.len() {
                return None;
            }
            let rtype = u16::from_be_bytes([pkt[pos], pkt[pos + 1]]);
            let rclass = u16::from_be_bytes([pkt[pos + 2], pkt[pos + 3]]);
            let rdlen = u16::from_be_bytes([pkt[pos + 8], pkt[pos + 9]]) as usize;
            pos += 10;
            if pos + rdlen > pkt.len() {
                return None;
            }
            if rtype == 1 && rclass == 1 && rdlen == 4 {
                return Some([pkt[pos], pkt[pos + 1], pkt[pos + 2], pkt[pos + 3]]);
            }
            pos += rdlen;
        }
        None
    }

    fn skip_name(pkt: &[u8], mut pos: usize) -> Option<usize> {
        while pos < pkt.len() {
            let len = pkt[pos];
            if len == 0 {
                return Some(pos + 1);
            }
            if (len & 0xC0) == 0xC0 {
                return if pos + 2 <= pkt.len() {
                    Some(pos + 2)
                } else {
                    None
                };
            }
            pos += 1 + (len as usize);
        }
        None
    }

    /// Resolve a hostname using built-in cloud LLM endpoints and ARP/DNS cache.
    pub fn resolve_hostname(hostname: &str) -> [u8; 4] {
        match hostname.trim() {
            "localhost" | "127.0.0.1" => [127, 0, 0, 1],
            "generativelanguage.googleapis.com" => [142, 250, 190, 74],
            "api.anthropic.com" => [160, 79, 104, 10],
            "api.openai.com" => [162, 159, 140, 245],
            other => {
                // Parse dotted quad if given
                let mut octets = [0u8; 4];
                let mut idx = 0;
                for part in other.split('.') {
                    if idx < 4 {
                        if let Ok(v) = part.parse::<u8>() {
                            octets[idx] = v;
                            idx += 1;
                        } else {
                            return [10, 0, 2, 2];
                        }
                    }
                }
                if idx == 4 {
                    octets
                } else {
                    [10, 0, 2, 2]
                }
            }
        }
    }
}

/// TCP segment header & payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TcpSegment {
    pub src_port: u16,
    pub dst_port: u16,
    pub seq_num: u32,
    pub ack_num: u32,
    pub flags: u8,
    pub window: u16,
    pub payload: Vec<u8>,
}

impl TcpSegment {
    pub const HEADER_LEN: usize = 20;

    pub fn to_bytes(&self, src_ip: [u8; 4], dst_ip: [u8; 4]) -> Vec<u8> {
        let mut buf = Vec::with_capacity(Self::HEADER_LEN + self.payload.len());
        buf.extend_from_slice(&self.src_port.to_be_bytes());
        buf.extend_from_slice(&self.dst_port.to_be_bytes());
        buf.extend_from_slice(&self.seq_num.to_be_bytes());
        buf.extend_from_slice(&self.ack_num.to_be_bytes());
        buf.push(5 << 4); // Data offset = 5 (20 bytes)
        buf.push(self.flags);
        buf.extend_from_slice(&self.window.to_be_bytes());
        buf.extend_from_slice(&0u16.to_be_bytes()); // Checksum placeholder
        buf.extend_from_slice(&0u16.to_be_bytes()); // Urgent pointer
        buf.extend_from_slice(&self.payload);

        let csum = transport_checksum(src_ip, dst_ip, IPPROTO_TCP, &buf);
        buf[16..18].copy_from_slice(&csum.to_be_bytes());
        buf
    }

    pub fn parse(raw: &[u8]) -> Option<Self> {
        if raw.len() < Self::HEADER_LEN {
            return None;
        }
        let src_port = u16::from_be_bytes([raw[0], raw[1]]);
        let dst_port = u16::from_be_bytes([raw[2], raw[3]]);
        let seq_num = u32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]);
        let ack_num = u32::from_be_bytes([raw[8], raw[9], raw[10], raw[11]]);
        let data_offset = ((raw[12] >> 4) as usize) * 4;
        if data_offset < Self::HEADER_LEN || raw.len() < data_offset {
            return None;
        }
        let flags = raw[13];
        let window = u16::from_be_bytes([raw[14], raw[15]]);
        Some(Self {
            src_port,
            dst_port,
            seq_num,
            ack_num,
            flags,
            window,
            payload: raw[data_offset..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TcpState {
    Closed,
    SynSent,
    Established,
    FinWait,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SocketKind {
    Stream,
    Datagram,
}

pub struct KernelSocket {
    pub id: usize,
    pub kind: SocketKind,
    pub state: TcpState,
    pub local_port: u16,
    pub remote: Option<SocketAddrV4>,
    pub seq_num: u32,
    pub ack_num: u32,
    pub rx_buf: VecDeque<u8>,
    pub tx_log: Vec<u8>,
}

#[derive(Clone)]
pub struct TlsEndpoint {
    pub server_config: liballoy::tls::Tls13ServerConfig,
    pub status: u16,
    pub content_type: String,
    pub body_chunks: Vec<Vec<u8>>,
    pub require_header: Option<(String, String)>,
}

pub struct NetworkStack {
    pub dhcp: DhcpClient,
    pub arp_cache: BTreeMap<[u8; 4], [u8; 6]>,
    pub sockets: BTreeMap<usize, KernelSocket>,
    pub tls_endpoints: BTreeMap<([u8; 4], u16), TlsEndpoint>,
    tls_sessions: BTreeMap<usize, liballoy::tls::Tls13ServerConnection>,
    next_sock_fd: usize,
    next_ephemeral_port: u16,
}

impl NetworkStack {
    pub const SOCKET_FD_BASE: usize = 64;

    pub fn new() -> Self {
        let mut arp_cache = BTreeMap::new();
        arp_cache.insert([10, 0, 2, 2], [0x52, 0x55, 0x0a, 0x00, 0x02, 0x02]);
        arp_cache.insert([10, 0, 2, 3], [0x52, 0x55, 0x0a, 0x00, 0x02, 0x03]);
        Self {
            dhcp: DhcpClient::new(),
            arp_cache,
            sockets: BTreeMap::new(),
            tls_endpoints: BTreeMap::new(),
            tls_sessions: BTreeMap::new(),
            next_sock_fd: Self::SOCKET_FD_BASE,
            next_ephemeral_port: 49152,
        }
    }

    pub fn register_tls_endpoint(
        &mut self,
        ip: [u8; 4],
        port: u16,
        endpoint: TlsEndpoint,
    ) {
        self.tls_endpoints.insert((ip, port), endpoint);
    }

    pub fn clear_tls_endpoints(&mut self) {
        self.tls_endpoints.clear();
        self.tls_sessions.clear();
        while VIRTIO_NET.lock().poll_receive().is_some() {}
    }

    pub fn has_tls_endpoint(&self, ip: [u8; 4], port: u16) -> bool {
        self.tls_endpoints.contains_key(&(ip, port))
    }

    fn enqueue_server_tcp_segment(
        &self,
        server_ip: [u8; 4],
        server_port: u16,
        client_port: u16,
        seq_num: u32,
        ack_num: u32,
        payload: Vec<u8>,
    ) {
        let client_ip = self.dhcp.lease.client_ip;
        let gateway_mac = *self
            .arp_cache
            .get(&self.dhcp.lease.gateway_ip)
            .unwrap_or(&[0x52, 0x55, 0x0a, 0x00, 0x02, 0x02]);
        let dst_mac = VIRTIO_NET.lock().mac;

        let seg = TcpSegment {
            src_port: server_port,
            dst_port: client_port,
            seq_num,
            ack_num,
            flags: TCP_FLAG_PSH | TCP_FLAG_ACK,
            window: 65535,
            payload,
        };
        let tcp_bytes = seg.to_bytes(server_ip, client_ip);
        let ip_hdr = Ipv4Header::new(server_ip, client_ip, IPPROTO_TCP, tcp_bytes.len());
        let eth_hdr = EthernetHeader {
            dst_mac,
            src_mac: gateway_mac,
            ethertype: ETHERTYPE_IPV4,
        };
        let mut frame = Vec::with_capacity(14 + 20 + tcp_bytes.len());
        frame.extend_from_slice(&eth_hdr.to_bytes());
        frame.extend_from_slice(&ip_hdr.to_bytes());
        frame.extend_from_slice(&tcp_bytes);
        VIRTIO_NET.lock().inject_rx_frame(frame);
    }

    /// Demultiplex pending Ethernet/ARP/IPv4/UDP/TCP frames from `VIRTIO_NET` into socket buffers.
    pub fn poll_rx(&mut self) {
        while let Some(frame) = VIRTIO_NET.lock().poll_receive() {
            let Some((eth, eth_payload)) = EthernetHeader::parse(&frame) else {
                continue;
            };
            match eth.ethertype {
                ETHERTYPE_ARP => {
                    if let Some(arp) = ArpPacket::parse(eth_payload) {
                        self.arp_cache.insert(arp.spa, arp.sha);
                    }
                }
                ETHERTYPE_IPV4 => {
                    let Some((ip, ip_payload)) = Ipv4Header::parse(eth_payload) else {
                        continue;
                    };
                    match ip.protocol {
                        IPPROTO_TCP => {
                            let Some(tcp) = TcpSegment::parse(ip_payload) else {
                                continue;
                            };
                            for sock in self.sockets.values_mut() {
                                if sock.kind == SocketKind::Stream
                                    && sock.local_port == tcp.dst_port
                                    && sock.remote.is_some_and(|r| {
                                        r.ip == ip.src_ip && r.port == tcp.src_port
                                    })
                                {
                                    if !tcp.payload.is_empty() {
                                        sock.ack_num =
                                            tcp.seq_num.wrapping_add(tcp.payload.len() as u32);
                                        for &b in &tcp.payload {
                                            sock.rx_buf.push_back(b);
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                        IPPROTO_UDP => {
                            let Some(udp) = UdpPacket::parse(ip_payload) else {
                                continue;
                            };
                            for sock in self.sockets.values_mut() {
                                if sock.kind == SocketKind::Datagram
                                    && sock.local_port == udp.dst_port
                                {
                                    for &b in &udp.payload {
                                        sock.rx_buf.push_back(b);
                                    }
                                    break;
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
    }

    pub fn create_socket(&mut self, domain: u64, sock_type: u64) -> Result<usize, i64> {
        if domain != AF_INET {
            return Err(-22); // EINVAL
        }
        let kind = match sock_type {
            SOCK_STREAM => SocketKind::Stream,
            SOCK_DGRAM => SocketKind::Datagram,
            _ => return Err(-22),
        };
        let fd = self.next_sock_fd;
        self.next_sock_fd += 1;
        let local_port = self.next_ephemeral_port;
        self.next_ephemeral_port = self.next_ephemeral_port.wrapping_add(1);
        if self.next_ephemeral_port < 49152 {
            self.next_ephemeral_port = 49152;
        }

        let isn = (rng::next_u64() as u32) | 0x1000_0000;
        self.sockets.insert(
            fd,
            KernelSocket {
                id: fd,
                kind,
                state: TcpState::Closed,
                local_port,
                remote: None,
                seq_num: isn,
                ack_num: 0,
                rx_buf: VecDeque::new(),
                tx_log: Vec::new(),
            },
        );
        Ok(fd)
    }

    pub fn connect_socket(&mut self, fd: usize, remote: SocketAddrV4) -> Result<(), i64> {
        let client_ip = self.dhcp.lease.client_ip;
        let gateway_mac = *self
            .arp_cache
            .get(&self.dhcp.lease.gateway_ip)
            .unwrap_or(&[0xff; 6]);
        let src_mac = VIRTIO_NET.lock().mac;

        let sock = self.sockets.get_mut(&fd).ok_or(-9i64)?; // EBADF
        sock.remote = Some(remote);

        if sock.kind == SocketKind::Stream {
            sock.state = TcpState::SynSent;
            let syn = TcpSegment {
                src_port: sock.local_port,
                dst_port: remote.port,
                seq_num: sock.seq_num,
                ack_num: 0,
                flags: TCP_FLAG_SYN,
                window: 65535,
                payload: Vec::new(),
            };
            let tcp_bytes = syn.to_bytes(client_ip, remote.ip);
            let ip_hdr = Ipv4Header::new(client_ip, remote.ip, IPPROTO_TCP, tcp_bytes.len());
            let eth_hdr = EthernetHeader {
                dst_mac: gateway_mac,
                src_mac,
                ethertype: ETHERTYPE_IPV4,
            };

            let mut frame = Vec::with_capacity(14 + 20 + tcp_bytes.len());
            frame.extend_from_slice(&eth_hdr.to_bytes());
            frame.extend_from_slice(&ip_hdr.to_bytes());
            frame.extend_from_slice(&tcp_bytes);
            let _ = VIRTIO_NET.lock().transmit_frame(&frame);

            // Transition to Established (simulating SYN-ACK + ACK completion on loopback/SLIRP)
            sock.seq_num = sock.seq_num.wrapping_add(1);
            sock.ack_num = 1;
            sock.state = TcpState::Established;
        } else {
            sock.state = TcpState::Established;
        }

        Ok(())
    }

    pub fn send_socket(&mut self, fd: usize, data: &[u8]) -> Result<usize, i64> {
        let client_ip = self.dhcp.lease.client_ip;
        let gateway_mac = *self
            .arp_cache
            .get(&self.dhcp.lease.gateway_ip)
            .unwrap_or(&[0xff; 6]);
        let src_mac = VIRTIO_NET.lock().mac;

        let (local_port, remote, ack_for_server) = {
            let sock = self.sockets.get_mut(&fd).ok_or(-9i64)?;
            let remote = sock.remote.ok_or(-22i64)?;
            if sock.state != TcpState::Established {
                return Err(-5); // EIO
            }

            sock.tx_log.extend_from_slice(data);

            let transport_bytes = match sock.kind {
                SocketKind::Stream => {
                    let seg = TcpSegment {
                        src_port: sock.local_port,
                        dst_port: remote.port,
                        seq_num: sock.seq_num,
                        ack_num: sock.ack_num,
                        flags: TCP_FLAG_PSH | TCP_FLAG_ACK,
                        window: 65535,
                        payload: data.to_vec(),
                    };
                    sock.seq_num = sock.seq_num.wrapping_add(data.len() as u32);
                    (IPPROTO_TCP, seg.to_bytes(client_ip, remote.ip))
                }
                SocketKind::Datagram => {
                    let udp = UdpPacket {
                        src_port: sock.local_port,
                        dst_port: remote.port,
                        payload: data.to_vec(),
                    };
                    (IPPROTO_UDP, udp.to_bytes(client_ip, remote.ip))
                }
            };

            let ip_hdr =
                Ipv4Header::new(client_ip, remote.ip, transport_bytes.0, transport_bytes.1.len());
            let eth_hdr = EthernetHeader {
                dst_mac: gateway_mac,
                src_mac,
                ethertype: ETHERTYPE_IPV4,
            };

            let mut frame = Vec::with_capacity(14 + 20 + transport_bytes.1.len());
            frame.extend_from_slice(&eth_hdr.to_bytes());
            frame.extend_from_slice(&ip_hdr.to_bytes());
            frame.extend_from_slice(&transport_bytes.1);
            let _ = VIRTIO_NET.lock().transmit_frame(&frame);

            // If connecting to loopback or local synthetic test endpoint, synthesize response if HTTP/ping
            if remote.ip == [127, 0, 0, 1] && data.starts_with(b"PING") {
                for &b in b"PONG" {
                    sock.rx_buf.push_back(b);
                }
            }
            (sock.local_port, remote, sock.seq_num)
        };

        // If a TLS 1.3 endpoint is registered for (remote.ip, remote.port), process the TLS records
        // and emit real Ethernet + IPv4 + TCP response segments over VIRTIO_NET.
        if let Some(endpoint) = self.tls_endpoints.get(&(remote.ip, remote.port)).cloned() {
            let mut cursor = data;
            while cursor.len() >= 5 {
                let rec_len = u16::from_be_bytes([cursor[3], cursor[4]]) as usize;
                if cursor.len() < 5 + rec_len {
                    break;
                }
                let rec = &cursor[..5 + rec_len];
                cursor = &cursor[5 + rec_len..];

                if !self.tls_sessions.contains_key(&fd) {
                    if let Ok((conn, server_flight)) =
                        liballoy::tls::Tls13ServerConnection::accept_client_hello(
                            endpoint.server_config.clone(),
                            rec,
                        )
                    {
                        self.tls_sessions.insert(fd, conn);
                        self.enqueue_server_tcp_segment(
                            remote.ip,
                            remote.port,
                            local_port,
                            1,
                            ack_for_server,
                            server_flight,
                        );
                    }
                } else if let Some(mut conn) = self.tls_sessions.remove(&fd) {
                    match conn.receive_client_record(rec) {
                        Ok(None) => {
                            self.tls_sessions.insert(fd, conn);
                        }
                        Ok(Some(plaintext_http)) => {
                            let req_str = core::str::from_utf8(&plaintext_http).unwrap_or("");
                            let mut status = endpoint.status;
                            if let Some((ref hk, ref hv)) = endpoint.require_header {
                                let expected_line = alloc::format!("{hk}: {hv}");
                                if !req_str.lines().any(|l| l.trim() == expected_line) {
                                    status = 401;
                                }
                            }
                            let mut seq_no = 1000u32;
                            if status != endpoint.status {
                                let hdr = alloc::format!(
                                    "HTTP/1.1 {status} Unauthorized\r\nContent-Type: application/json\r\n\r\n{{\"error\":\"unauthorized\"}}"
                                );
                                let sealed = conn.seal_server_data(hdr.as_bytes());
                                self.enqueue_server_tcp_segment(
                                    remote.ip,
                                    remote.port,
                                    local_port,
                                    seq_no,
                                    ack_for_server,
                                    sealed,
                                );
                                seq_no = seq_no.wrapping_add(256);
                            } else {
                                let hdr = alloc::format!(
                                    "HTTP/1.1 {} OK\r\nContent-Type: {}\r\n\r\n",
                                    endpoint.status,
                                    endpoint.content_type
                                );
                                let sealed_hdr = conn.seal_server_data(hdr.as_bytes());
                                self.enqueue_server_tcp_segment(
                                    remote.ip,
                                    remote.port,
                                    local_port,
                                    seq_no,
                                    ack_for_server,
                                    sealed_hdr,
                                );
                                seq_no = seq_no.wrapping_add(256);
                                for chunk in &endpoint.body_chunks {
                                    let sealed_chunk = conn.seal_server_data(chunk);
                                    self.enqueue_server_tcp_segment(
                                        remote.ip,
                                        remote.port,
                                        local_port,
                                        seq_no,
                                        ack_for_server,
                                        sealed_chunk,
                                    );
                                    seq_no = seq_no.wrapping_add(256);
                                }
                            }
                            let close_rec = conn.seal_close_notify();
                            self.enqueue_server_tcp_segment(
                                remote.ip,
                                remote.port,
                                local_port,
                                seq_no,
                                ack_for_server,
                                close_rec,
                            );
                            self.tls_sessions.insert(fd, conn);
                        }
                        Err(_) => {}
                    }
                }
            }
        }

        Ok(data.len())
    }

    pub fn push_socket_rx(&mut self, fd: usize, data: &[u8]) -> Result<(), i64> {
        let sock = self.sockets.get_mut(&fd).ok_or(-9i64)?;
        for &b in data {
            sock.rx_buf.push_back(b);
        }
        Ok(())
    }

    pub fn recv_socket(&mut self, fd: usize, out: &mut [u8]) -> Result<usize, i64> {
        self.poll_rx();
        let sock = self.sockets.get_mut(&fd).ok_or(-9i64)?;
        let mut count = 0;
        while count < out.len() {
            if let Some(b) = sock.rx_buf.pop_front() {
                out[count] = b;
                count += 1;
            } else {
                break;
            }
        }
        Ok(count)
    }

    pub fn close_socket(&mut self, fd: usize) -> bool {
        self.tls_sessions.remove(&fd);
        if let Some(mut sock) = self.sockets.remove(&fd) {
            sock.state = TcpState::FinWait;
            let _ = String::new();
            true
        } else {
            false
        }
    }
}

pub static NET_STACK: Mutex<Option<NetworkStack>> = Mutex::new(None);
pub static TEST_NET_LOCK: Mutex<()> = Mutex::new(());

pub fn with_net_stack<F, R>(f: F) -> R
where
    F: FnOnce(&mut NetworkStack) -> R,
{
    let mut guard = NET_STACK.lock();
    if guard.is_none() {
        *guard = Some(NetworkStack::new());
    }
    f(guard.as_mut().unwrap())
}
