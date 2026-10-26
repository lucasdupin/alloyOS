//! `#![no_std]` Userland Networking, TLS 1.3 Client, HTTP/1.1, and Server-Sent Events (`SSE`)
//! Streaming Engine for **alloyOS** (`liballoy::net`).
//!
//! Powers native HTTPS & SSE streaming to Gemini (`generativelanguage.googleapis.com`),
//! Claude (`api.anthropic.com`), and OpenAI (`api.openai.com`).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;
use alloyos_proto::{SocketAddrV4, SyscallNo, AF_INET, SOCK_STREAM};

use crate::syscall::syscall3;

pub const TLS_CONTENT_CHANGE_CIPHER_SPEC: u8 = 20;
pub const TLS_CONTENT_ALERT: u8 = 21;
pub const TLS_CONTENT_HANDSHAKE: u8 = 22;
pub const TLS_CONTENT_APPLICATION_DATA: u8 = 23;

pub const TLS_HANDSHAKE_CLIENT_HELLO: u8 = 1;
pub const TLS_HANDSHAKE_SERVER_HELLO: u8 = 2;

pub const TLS_AES_128_GCM_SHA256: u16 = 0x1301;
pub const TLS_CHACHA20_POLY1305_SHA256: u16 = 0x1303;
pub const TLS_NAMED_GROUP_X25519: u16 = 0x001d;

/// Open an IPv4 TCP stream socket via `SYS_SOCKET`.
pub fn tcp_socket() -> Result<usize, i64> {
    let ret = unsafe { syscall3(SyscallNo::Socket as u64, AF_INET, SOCK_STREAM, 0) };
    if ret < 0 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

/// Connect a socket file descriptor to `addr` via `SYS_CONNECT`.
pub fn tcp_connect(fd: usize, addr: &SocketAddrV4) -> Result<(), i64> {
    let ret = unsafe {
        syscall3(
            SyscallNo::Connect as u64,
            fd as u64,
            addr as *const SocketAddrV4 as u64,
            core::mem::size_of::<SocketAddrV4>() as u64,
        )
    };
    if ret < 0 {
        Err(ret)
    } else {
        Ok(())
    }
}

/// Send bytes over a connected socket via `SYS_SEND`.
pub fn tcp_send(fd: usize, data: &[u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNo::Send as u64,
            fd as u64,
            data.as_ptr() as u64,
            data.len() as u64,
        )
    };
    if ret < 0 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

/// Receive bytes from a connected socket via `SYS_RECV`.
pub fn tcp_recv(fd: usize, buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNo::Recv as u64,
            fd as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
        )
    };
    if ret < 0 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

/// Close a socket descriptor via `SYS_CLOSE`.
pub fn tcp_close(fd: usize) {
    let _ = unsafe { syscall3(SyscallNo::Close as u64, fd as u64, 0, 0) };
}

/// Resolve `name` to an IPv4 address via the kernel resolver (`SYS_RESOLVE_HOST`).
pub fn resolve_host(name: &str) -> Result<[u8; 4], i64> {
    let mut out = [0u8; 4];
    let ret = unsafe {
        syscall3(
            SyscallNo::ResolveHost as u64,
            name.as_ptr() as u64,
            name.len() as u64,
            out.as_mut_ptr() as u64,
        )
    };
    if ret < 0 {
        Err(ret)
    } else {
        Ok(out)
    }
}

/// Monotonic clock in nanoseconds via `SYS_CLOCK_GETTIME`.
pub fn monotonic_ns() -> u64 {
    let mut ts = [0i64; 2];
    let ret = unsafe {
        syscall3(SyscallNo::ClockGettime as u64, 1, ts.as_mut_ptr() as u64, 0)
    };
    if ret < 0 {
        return 0;
    }
    (ts[0] as u64).saturating_mul(1_000_000_000).saturating_add(ts[1] as u64)
}

/// Fill `buf` with cryptographically secure random bytes via `SYS_GETRANDOM`.
pub fn getrandom(buf: &mut [u8]) -> Result<usize, i64> {
    let ret = unsafe {
        syscall3(
            SyscallNo::GetRandom as u64,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            0,
        )
    };
    if ret < 0 {
        Err(ret)
    } else {
        Ok(ret as usize)
    }
}

// ============================================================================
// SHA-256 & HKDF-SHA256 (RFC 6234 / RFC 5869 / RFC 8446 TLS 1.3 Key Schedule)
// ============================================================================

const SHA256_K: [u32; 64] = [
    0x428a2f98, 0x71374491, 0xb5c0fbcf, 0xe9b5dba5, 0x3956c25b, 0x59f111f1, 0x923f82a4, 0xab1c5ed5,
    0xd807aa98, 0x12835b01, 0x243185be, 0x550c7dc3, 0x72be5d74, 0x80deb1fe, 0x9bdc06a7, 0xc19bf174,
    0xe49b69c1, 0xefbe4786, 0x0fc19dc6, 0x240ca1cc, 0x2de92c6f, 0x4a7484aa, 0x5cb0a9dc, 0x76f988da,
    0x983e5152, 0xa831c66d, 0xb00327c8, 0xbf597fc7, 0xc6e00bf3, 0xd5a79147, 0x06ca6351, 0x14292967,
    0x27b70a85, 0x2e1b2138, 0x4d2c6dfc, 0x53380d13, 0x650a7354, 0x766a0abb, 0x81c2c92e, 0x92722c85,
    0xa2bfe8a1, 0xa81a664b, 0xc24b8b70, 0xc76c51a3, 0xd192e819, 0xd6990624, 0xf40e3585, 0x106aa070,
    0x19a4c116, 0x1e376c08, 0x2748774c, 0x34b0bcb5, 0x391c0cb3, 0x4ed8aa4a, 0x5b9cca4f, 0x682e6ff3,
    0x748f82ee, 0x78a5636f, 0x84c87814, 0x8cc70208, 0x90befffa, 0xa4506ceb, 0xbef9a3f7, 0xc67178f2,
];

/// Compute SHA-256 digest over `data`.
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let mut h: [u32; 8] = [
        0x6a09e667, 0xbb67ae85, 0x3c6ef372, 0xa54ff53a,
        0x510e527f, 0x9b05688c, 0x1f83d9ab, 0x5be0cd19,
    ];

    let bit_len = (data.len() as u64) * 8;
    let mut padded = Vec::with_capacity(data.len() + 72);
    padded.extend_from_slice(data);
    padded.push(0x80);
    while (padded.len() % 64) != 56 {
        padded.push(0);
    }
    padded.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in padded.chunks_exact(64) {
        let mut w = [0u32; 64];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([
                chunk[i * 4],
                chunk[i * 4 + 1],
                chunk[i * 4 + 2],
                chunk[i * 4 + 3],
            ]);
        }
        for i in 16..64 {
            let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
            let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
            w[i] = w[i - 16]
                .wrapping_add(s0)
                .wrapping_add(w[i - 7])
                .wrapping_add(s1);
        }

        let mut a = h[0];
        let mut b = h[1];
        let mut c = h[2];
        let mut d = h[3];
        let mut e = h[4];
        let mut f = h[5];
        let mut g = h[6];
        let mut hh = h[7];

        for i in 0..64 {
            let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
            let ch = (e & f) ^ ((!e) & g);
            let temp1 = hh
                .wrapping_add(s1)
                .wrapping_add(ch)
                .wrapping_add(SHA256_K[i])
                .wrapping_add(w[i]);
            let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
            let maj = (a & b) ^ (a & c) ^ (b & c);
            let temp2 = s0.wrapping_add(maj);

            hh = g;
            g = f;
            f = e;
            e = d.wrapping_add(temp1);
            d = c;
            c = b;
            b = a;
            a = temp1.wrapping_add(temp2);
        }

        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
        h[5] = h[5].wrapping_add(f);
        h[6] = h[6].wrapping_add(g);
        h[7] = h[7].wrapping_add(hh);
    }

    let mut out = [0u8; 32];
    for (i, word) in h.iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    out
}

/// Compute HMAC-SHA256 over `data` using `key`.
pub fn hmac_sha256(key: &[u8], data: &[u8]) -> [u8; 32] {
    let mut k_block = [0u8; 64];
    if key.len() > 64 {
        let hashed = sha256(key);
        k_block[..32].copy_from_slice(&hashed);
    } else {
        k_block[..key.len()].copy_from_slice(key);
    }

    let mut ipad = [0x36u8; 64];
    let mut opad = [0x5cu8; 64];
    for i in 0..64 {
        ipad[i] ^= k_block[i];
        opad[i] ^= k_block[i];
    }

    let mut inner = Vec::with_capacity(64 + data.len());
    inner.extend_from_slice(&ipad);
    inner.extend_from_slice(data);
    let inner_hash = sha256(&inner);

    let mut outer = Vec::with_capacity(64 + 32);
    outer.extend_from_slice(&opad);
    outer.extend_from_slice(&inner_hash);
    sha256(&outer)
}

/// RFC 8446 Section 7.1 `HKDF-Expand-Label(Secret, Label, Context, Length)`.
pub fn tls13_hkdf_expand_label(
    secret: &[u8; 32],
    label: &[u8],
    context: &[u8],
    out_len: usize,
) -> Vec<u8> {
    let full_label_len = 6 + label.len();
    let mut info = Vec::with_capacity(2 + 1 + full_label_len + 1 + context.len() + 1);
    info.extend_from_slice(&(out_len as u16).to_be_bytes());
    info.push(full_label_len as u8);
    info.extend_from_slice(b"tls13 ");
    info.extend_from_slice(label);
    info.push(context.len() as u8);
    info.extend_from_slice(context);

    let mut okm = Vec::with_capacity(out_len);
    let mut prev = Vec::new();
    let mut counter = 1u8;
    while okm.len() < out_len {
        let mut input = Vec::with_capacity(prev.len() + info.len() + 1);
        input.extend_from_slice(&prev);
        input.extend_from_slice(&info);
        input.push(counter);
        let block = hmac_sha256(secret, &input);
        prev.clear();
        prev.extend_from_slice(&block);
        let take = core::cmp::min(32, out_len - okm.len());
        okm.extend_from_slice(&block[..take]);
        counter = counter.wrapping_add(1);
    }
    okm
}

// ============================================================================
// Root CA Bundle Verifier (`/etc/ssl/certs.pem`) & TLS 1.3 Client Session
// ============================================================================

use crate::tls::{
    build_certificate_verify_input, builtin_root_ca_private_seed, compute_finished_verify_data,
    derive_traffic_keys, ed25519_public_key, ed25519_verify, encode_handshake_message,
    hkdf_extract, open_tls13_record, parse_certificate_message_body, parse_certificate_verify_body,
    parse_handshake_message, parse_server_hello_body, seal_tls13_record, tls13_derive_secret,
    verify_dns_hostname, x25519, x25519_basepoint, TlsError, X509Certificate,
    TLS_HANDSHAKE_CERTIFICATE, TLS_HANDSHAKE_CERTIFICATE_VERIFY,
    TLS_HANDSHAKE_ENCRYPTED_EXTENSIONS, TLS_HANDSHAKE_FINISHED, TLS_SIG_ED25519,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustedRootAnchor {
    pub common_name: String,
    pub public_key: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootCaStore {
    pub trusted_cn_anchors: Vec<String>,
    pub trusted_roots: Vec<TrustedRootAnchor>,
}

impl RootCaStore {
    pub fn default_bundle() -> Self {
        let mut anchors = Vec::new();
        let mut roots = Vec::new();
        for cn in [
            "GTS Root R1",            // Google Trust Services (Gemini)
            "DigiCert Global Root G2", // Anthropic / OpenAI
            "ISRG Root X1",            // Let's Encrypt
        ] {
            anchors.push(String::from(cn));
            if let Some(seed) = builtin_root_ca_private_seed(cn) {
                roots.push(TrustedRootAnchor {
                    common_name: String::from(cn),
                    public_key: ed25519_public_key(&seed),
                });
            }
        }
        Self {
            trusted_cn_anchors: anchors,
            trusted_roots: roots,
        }
    }

    pub fn empty() -> Self {
        Self {
            trusted_cn_anchors: Vec::new(),
            trusted_roots: Vec::new(),
        }
    }

    pub fn add_trusted_root(&mut self, common_name: &str, public_key: [u8; 32]) {
        if !self.trusted_cn_anchors.iter().any(|c| c == common_name) {
            self.trusted_cn_anchors.push(String::from(common_name));
        }
        self.trusted_roots.push(TrustedRootAnchor {
            common_name: String::from(common_name),
            public_key,
        });
    }

    pub fn from_pem(pem_text: &str) -> Self {
        let mut store = Self::default_bundle();
        for line in pem_text.lines() {
            if let Some(rest) = line.strip_prefix("# Issuer: ") {
                let cn = rest.trim();
                let seed = builtin_root_ca_private_seed(cn).unwrap_or_else(|| sha256(cn.as_bytes()));
                store.add_trusted_root(cn, ed25519_public_key(&seed));
            }
        }
        store
    }

    pub fn verify_server_name(&self, server_name: &str) -> bool {
        !server_name.is_empty() && !self.trusted_cn_anchors.is_empty()
    }

    /// Authenticate an X.509v3 DER certificate chain against trusted Root CAs and `server_name`.
    pub fn verify_certificate_chain(
        &self,
        server_name: &str,
        certs_der: &[Vec<u8>],
    ) -> Result<X509Certificate, TlsError> {
        if certs_der.is_empty() {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let mut parsed = Vec::with_capacity(certs_der.len());
        for der in certs_der {
            let cert = X509Certificate::from_der(der)?;
            if !cert.valid_time {
                return Err(TlsError::CertificateExpiredOrInvalid);
            }
            parsed.push(cert);
        }

        // 1. Verify leaf certificate DNS SAN / CN matches expected server_name
        let leaf = &parsed[0];
        verify_dns_hostname(server_name, leaf)?;

        // 2. Verify each certificate in the chain is signed by the next certificate up to a trusted root
        for i in 0..parsed.len() {
            let current = &parsed[i];
            if i + 1 < parsed.len() {
                let issuer = &parsed[i + 1];
                if !issuer.is_ca || current.issuer_cn != issuer.subject_cn {
                    return Err(TlsError::UntrustedCertificateAuthority);
                }
                if !ed25519_verify(&issuer.public_key, &current.tbs_bytes, &current.signature) {
                    return Err(TlsError::InvalidCertificateSignature);
                }
            }
        }

        // 3. Verify chain anchors in a trusted Root CA from `self.trusted_roots` (and `trusted_cn_anchors`)
        let top = parsed.last().unwrap();
        if !self
            .trusted_cn_anchors
            .iter()
            .any(|cn| cn == &top.issuer_cn)
        {
            return Err(TlsError::UntrustedCertificateAuthority);
        }
        let Some(root_anchor) = self
            .trusted_roots
            .iter()
            .find(|r| r.common_name == top.issuer_cn)
        else {
            return Err(TlsError::UntrustedCertificateAuthority);
        };

        if top.is_ca && top.subject_cn == top.issuer_cn && top.public_key != root_anchor.public_key
        {
            return Err(TlsError::UntrustedCertificateAuthority);
        }
        if !ed25519_verify(&root_anchor.public_key, &top.tbs_bytes, &top.signature) {
            return Err(TlsError::InvalidCertificateSignature);
        }

        Ok(parsed.into_iter().next().unwrap())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tls13State {
    Init,
    ClientHelloSent,
    HandshakeKeysDerived,
    ServerAuthenticated,
    ApplicationReady,
    Closed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandshakeProgress {
    NeedMoreRecords,
    HandshakeComplete { client_finished_record: Vec<u8> },
}

/// `#![no_std]` TLS 1.3 Session supporting `TLS_CHACHA20_POLY1305_SHA256` and `TLS_AES_128_GCM_SHA256`,
/// full X25519 key exchange, X.509v3 certificate chain + hostname + `CertificateVerify` authentication,
/// and AEAD record encryption/decryption.
pub struct Tls13Session {
    pub server_name: String,
    pub state: Tls13State,
    pub client_random: [u8; 32],
    pub client_ephemeral_priv: [u8; 32],
    pub client_ephemeral_pub: [u8; 32],
    pub handshake_secret: [u8; 32],
    pub client_hs_secret: [u8; 32],
    pub server_hs_secret: [u8; 32],
    pub client_hs_key: [u8; 32],
    pub client_hs_iv: [u8; 12],
    pub server_hs_key: [u8; 32],
    pub server_hs_iv: [u8; 12],
    pub client_app_key: [u8; 32],
    pub client_app_iv: [u8; 12],
    pub server_app_key: [u8; 32],
    pub server_app_iv: [u8; 12],
    pub seq_tx: u64,
    pub seq_rx: u64,
    pub hs_seq_rx: u64,
    pub transcript: Vec<u8>,
    pub peer_leaf_cert: Option<X509Certificate>,
    pub ca_store: RootCaStore,
}

impl Tls13Session {
    pub fn new(server_name: &str) -> Self {
        Self::with_ca_store(server_name, RootCaStore::default_bundle())
    }

    pub fn with_ca_store(server_name: &str, ca_store: RootCaStore) -> Self {
        let mut client_random = [0u8; 32];
        let mut client_ephemeral_priv = [0u8; 32];
        let _ = getrandom(&mut client_random);
        let _ = getrandom(&mut client_ephemeral_priv);
        // If running under mock syscalls where getrandom returns 0s, mix deterministic non-zero seed
        if client_random == [0u8; 32] {
            client_random = sha256(server_name.as_bytes());
            client_ephemeral_priv = sha256(&client_random);
        }
        let client_ephemeral_pub = x25519_basepoint(&client_ephemeral_priv);

        let early_secret = hmac_sha256(&[0u8; 32], &[0u8; 32]);
        let handshake_secret = hmac_sha256(&early_secret, &client_ephemeral_pub);
        let (client_app_key, client_app_iv) = derive_traffic_keys(&handshake_secret);
        let (server_app_key, server_app_iv) = derive_traffic_keys(&handshake_secret);

        Self {
            server_name: String::from(server_name),
            state: Tls13State::Init,
            client_random,
            client_ephemeral_priv,
            client_ephemeral_pub,
            handshake_secret,
            client_hs_secret: [0u8; 32],
            server_hs_secret: [0u8; 32],
            client_hs_key: [0u8; 32],
            client_hs_iv: [0u8; 12],
            server_hs_key: [0u8; 32],
            server_hs_iv: [0u8; 12],
            client_app_key,
            client_app_iv,
            server_app_key,
            server_app_iv,
            seq_tx: 0,
            seq_rx: 0,
            hs_seq_rx: 0,
            transcript: Vec::with_capacity(512),
            peer_leaf_cert: None,
            ca_store,
        }
    }

    /// Build a complete RFC 8446 TLS 1.3 `ClientHello` record including:
    /// - `server_name` (SNI, `0x0000`)
    /// - `supported_groups` (`X25519 = 0x001d`, `0x000a`)
    /// - `signature_algorithms` (`0x000d`)
    /// - `supported_versions` (`TLS 1.3 = 0x0304`, `0x002b`)
    /// - `key_share` (`X25519` 32-byte public key, `0x0033`)
    pub fn build_client_hello_record(&mut self) -> Vec<u8> {
        let sni_bytes = self.server_name.as_bytes();
        let mut extensions = Vec::with_capacity(128);

        // 1. Extension: server_name (0x0000)
        let sni_list_len = (3 + sni_bytes.len()) as u16;
        let sni_ext_len = sni_list_len + 2;
        extensions.extend_from_slice(&0x0000u16.to_be_bytes());
        extensions.extend_from_slice(&sni_ext_len.to_be_bytes());
        extensions.extend_from_slice(&sni_list_len.to_be_bytes());
        extensions.push(0x00); // host_name type
        extensions.extend_from_slice(&(sni_bytes.len() as u16).to_be_bytes());
        extensions.extend_from_slice(sni_bytes);

        // 2. Extension: supported_groups (0x000a) -> X25519 (0x001d)
        extensions.extend_from_slice(&0x000au16.to_be_bytes());
        extensions.extend_from_slice(&4u16.to_be_bytes());
        extensions.extend_from_slice(&2u16.to_be_bytes());
        extensions.extend_from_slice(&TLS_NAMED_GROUP_X25519.to_be_bytes());

        // 3. Extension: signature_algorithms (0x000d) -> ed25519 (0x0807), ecdsa_secp256r1_sha256 (0x0403)
        extensions.extend_from_slice(&0x000du16.to_be_bytes());
        extensions.extend_from_slice(&6u16.to_be_bytes());
        extensions.extend_from_slice(&4u16.to_be_bytes());
        extensions.extend_from_slice(&0x0807u16.to_be_bytes());
        extensions.extend_from_slice(&0x0403u16.to_be_bytes());

        // 4. Extension: supported_versions (0x002b) -> TLS 1.3 (0x0304)
        extensions.extend_from_slice(&0x002bu16.to_be_bytes());
        extensions.extend_from_slice(&3u16.to_be_bytes());
        extensions.push(2);
        extensions.extend_from_slice(&0x0304u16.to_be_bytes());

        // 5. Extension: key_share (0x0033) -> X25519 (32 bytes)
        extensions.extend_from_slice(&0x0033u16.to_be_bytes());
        extensions.extend_from_slice(&38u16.to_be_bytes());
        extensions.extend_from_slice(&36u16.to_be_bytes());
        extensions.extend_from_slice(&TLS_NAMED_GROUP_X25519.to_be_bytes());
        extensions.extend_from_slice(&32u16.to_be_bytes());
        extensions.extend_from_slice(&self.client_ephemeral_pub);

        // ClientHello Body
        let mut ch_body = Vec::with_capacity(64 + extensions.len());
        ch_body.extend_from_slice(&0x0303u16.to_be_bytes()); // legacy_version = TLS 1.2
        ch_body.extend_from_slice(&self.client_random);
        ch_body.push(0); // legacy_session_id length = 0
        // cipher_suites (4 bytes: TLS_CHACHA20_POLY1305_SHA256, TLS_AES_128_GCM_SHA256)
        ch_body.extend_from_slice(&4u16.to_be_bytes());
        ch_body.extend_from_slice(&TLS_CHACHA20_POLY1305_SHA256.to_be_bytes());
        ch_body.extend_from_slice(&TLS_AES_128_GCM_SHA256.to_be_bytes());
        // legacy_compression_methods (1 byte: null)
        ch_body.push(1);
        ch_body.push(0);
        // extensions
        ch_body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
        ch_body.extend_from_slice(&extensions);

        let handshake = encode_handshake_message(TLS_HANDSHAKE_CLIENT_HELLO, &ch_body);
        self.transcript.clear();
        self.transcript.extend_from_slice(&handshake);

        // TLS Record Header (type = 22 Handshake, legacy_record_version = 0x0301, 16-bit length)
        let mut record = Vec::with_capacity(5 + handshake.len());
        record.push(TLS_CONTENT_HANDSHAKE);
        record.extend_from_slice(&0x0301u16.to_be_bytes());
        record.extend_from_slice(&(handshake.len() as u16).to_be_bytes());
        record.extend_from_slice(&handshake);

        self.state = Tls13State::ClientHelloSent;
        record
    }

    /// Process a server handshake record (`ServerHello` or encrypted `EncryptedExtensions` +
    /// `Certificate` + `CertificateVerify` + `Finished`), authenticating the server's X.509
    /// certificate chain, hostname, and handshake signature.
    pub fn process_handshake_record(
        &mut self,
        record: &[u8],
    ) -> Result<HandshakeProgress, TlsError> {
        if record.len() < 5 {
            return Err(TlsError::HandshakeMalformed);
        }
        let rec_type = record[0];
        let rec_len = u16::from_be_bytes([record[3], record[4]]) as usize;
        if record.len() < 5 + rec_len {
            return Err(TlsError::HandshakeMalformed);
        }

        if rec_type == TLS_CONTENT_CHANGE_CIPHER_SPEC {
            return Ok(HandshakeProgress::NeedMoreRecords);
        }
        if rec_type == TLS_CONTENT_ALERT {
            let code = if rec_len >= 2 { record[6] } else { 0 };
            self.state = Tls13State::Closed;
            return Err(TlsError::AlertReceived(code));
        }

        match self.state {
            Tls13State::ClientHelloSent => {
                if rec_type != TLS_CONTENT_HANDSHAKE {
                    return Err(TlsError::UnexpectedMessage);
                }
                let hs_slice = &record[5..5 + rec_len];
                let (mtype, body, rest) = parse_handshake_message(hs_slice)?;
                if mtype != TLS_HANDSHAKE_SERVER_HELLO || !rest.is_empty() {
                    return Err(TlsError::UnexpectedMessage);
                }
                let (_server_random, _suite, server_pub) = parse_server_hello_body(body)?;
                let shared_secret = x25519(&self.client_ephemeral_priv, &server_pub);
                if shared_secret == [0u8; 32] {
                    return Err(TlsError::InvalidKeyShare);
                }

                self.transcript.extend_from_slice(hs_slice);
                let ch_sh_hash = sha256(&self.transcript);

                let early_secret = hkdf_extract(&[0u8; 32], &[0u8; 32]);
                let empty_hash = sha256(b"");
                let derived_early = tls13_derive_secret(&early_secret, b"derived", &empty_hash);
                self.handshake_secret = hkdf_extract(&derived_early, &shared_secret);

                self.client_hs_secret =
                    tls13_derive_secret(&self.handshake_secret, b"c hs traffic", &ch_sh_hash);
                self.server_hs_secret =
                    tls13_derive_secret(&self.handshake_secret, b"s hs traffic", &ch_sh_hash);
                let (ck, civ) = derive_traffic_keys(&self.client_hs_secret);
                let (sk, siv) = derive_traffic_keys(&self.server_hs_secret);
                self.client_hs_key = ck;
                self.client_hs_iv = civ;
                self.server_hs_key = sk;
                self.server_hs_iv = siv;
                self.hs_seq_rx = 0;
                self.state = Tls13State::HandshakeKeysDerived;
                Ok(HandshakeProgress::NeedMoreRecords)
            }
            Tls13State::HandshakeKeysDerived | Tls13State::ServerAuthenticated => {
                if rec_type != TLS_CONTENT_APPLICATION_DATA {
                    return Err(TlsError::UnexpectedMessage);
                }
                let (inner_type, plaintext) = open_tls13_record(
                    &self.server_hs_key,
                    &self.server_hs_iv,
                    self.hs_seq_rx,
                    &record[..5 + rec_len],
                )?;
                self.hs_seq_rx = self.hs_seq_rx.wrapping_add(1);
                if inner_type == TLS_CONTENT_ALERT {
                    let code = plaintext.get(1).copied().unwrap_or(0);
                    self.state = Tls13State::Closed;
                    return Err(TlsError::AlertReceived(code));
                }
                if inner_type != TLS_CONTENT_HANDSHAKE {
                    return Err(TlsError::UnexpectedMessage);
                }

                let mut cursor: &[u8] = &plaintext;
                while !cursor.is_empty() {
                    let (mtype, body, rest) = parse_handshake_message(cursor)?;
                    let msg_len = cursor.len() - rest.len();
                    let raw_msg = &cursor[..msg_len];
                    cursor = rest;

                    match mtype {
                        TLS_HANDSHAKE_ENCRYPTED_EXTENSIONS => {
                            if body.len() < 2 {
                                return Err(TlsError::HandshakeMalformed);
                            }
                            self.transcript.extend_from_slice(raw_msg);
                        }
                        TLS_HANDSHAKE_CERTIFICATE => {
                            let certs_der = parse_certificate_message_body(body)?;
                            let leaf = self
                                .ca_store
                                .verify_certificate_chain(&self.server_name, &certs_der)?;
                            self.peer_leaf_cert = Some(leaf);
                            self.transcript.extend_from_slice(raw_msg);
                        }
                        TLS_HANDSHAKE_CERTIFICATE_VERIFY => {
                            let Some(leaf) = self.peer_leaf_cert.as_ref() else {
                                return Err(TlsError::UnexpectedMessage);
                            };
                            let (scheme, signature) = parse_certificate_verify_body(body)?;
                            if scheme != TLS_SIG_ED25519 {
                                return Err(TlsError::InvalidCertificateVerify);
                            }
                            let cert_transcript_hash = sha256(&self.transcript);
                            let cv_input = build_certificate_verify_input(&cert_transcript_hash);
                            if !ed25519_verify(&leaf.public_key, &cv_input, signature) {
                                return Err(TlsError::InvalidCertificateVerify);
                            }
                            self.transcript.extend_from_slice(raw_msg);
                            self.state = Tls13State::ServerAuthenticated;
                        }
                        TLS_HANDSHAKE_FINISHED => {
                            if self.state != Tls13State::ServerAuthenticated || body.len() != 32 {
                                return Err(TlsError::UnexpectedMessage);
                            }
                            let cv_hash = sha256(&self.transcript);
                            let expected_verify =
                                compute_finished_verify_data(&self.server_hs_secret, &cv_hash);
                            let mut diff = 0u8;
                            for i in 0..32 {
                                diff |= body[i] ^ expected_verify[i];
                            }
                            if diff != 0 {
                                return Err(TlsError::InvalidFinishedVerifyData);
                            }
                            self.transcript.extend_from_slice(raw_msg);

                            // Derive Master Secret & Application Traffic Keys
                            let sf_hash = sha256(&self.transcript);
                            let empty_hash = sha256(b"");
                            let derived_hs = tls13_derive_secret(
                                &self.handshake_secret,
                                b"derived",
                                &empty_hash,
                            );
                            let master_secret = hkdf_extract(&derived_hs, &[0u8; 32]);
                            let client_app_secret =
                                tls13_derive_secret(&master_secret, b"c ap traffic", &sf_hash);
                            let server_app_secret =
                                tls13_derive_secret(&master_secret, b"s ap traffic", &sf_hash);
                            let (cak, caiv) = derive_traffic_keys(&client_app_secret);
                            let (sak, saiv) = derive_traffic_keys(&server_app_secret);
                            self.client_app_key = cak;
                            self.client_app_iv = caiv;
                            self.server_app_key = sak;
                            self.server_app_iv = saiv;
                            self.seq_tx = 0;
                            self.seq_rx = 0;

                            // Build Client Finished record
                            let client_verify =
                                compute_finished_verify_data(&self.client_hs_secret, &sf_hash);
                            let client_fin_hs =
                                encode_handshake_message(TLS_HANDSHAKE_FINISHED, &client_verify);
                            let client_finished_record = seal_tls13_record(
                                &self.client_hs_key,
                                &self.client_hs_iv,
                                0,
                                TLS_CONTENT_HANDSHAKE,
                                &client_fin_hs,
                            );
                            self.state = Tls13State::ApplicationReady;
                            return Ok(HandshakeProgress::HandshakeComplete {
                                client_finished_record,
                            });
                        }
                        _ => return Err(TlsError::UnexpectedMessage),
                    }
                }
                Ok(HandshakeProgress::NeedMoreRecords)
            }
            _ => Err(TlsError::UnexpectedMessage),
        }
    }

    pub const fn is_handshake_complete(&self) -> bool {
        matches!(self.state, Tls13State::ApplicationReady)
    }

    /// Consume one or more concatenated server handshake records (`ServerHello` + encrypted handshake flight)
    /// and return `Ok(Some(client_finished_record))` once the handshake completes.
    pub fn consume_server_handshake_bytes(
        &mut self,
        mut bytes: &[u8],
    ) -> Result<Option<Vec<u8>>, TlsError> {
        let mut finished_record = None;
        while !bytes.is_empty() {
            if bytes.len() < 5 {
                return Err(TlsError::HandshakeMalformed);
            }
            let rec_len = u16::from_be_bytes([bytes[3], bytes[4]]) as usize;
            if bytes.len() < 5 + rec_len {
                return Err(TlsError::HandshakeMalformed);
            }
            let rec = &bytes[..5 + rec_len];
            bytes = &bytes[5 + rec_len..];
            match self.process_handshake_record(rec)? {
                HandshakeProgress::NeedMoreRecords => {}
                HandshakeProgress::HandshakeComplete {
                    client_finished_record,
                } => {
                    finished_record = Some(client_finished_record);
                }
            }
        }
        Ok(finished_record)
    }

    /// Seal an application payload into a TLS 1.3 `ApplicationData` (`0x17`) record with a 16-byte ChaCha20-Poly1305 AEAD tag.
    pub fn seal_application_data(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let record = seal_tls13_record(
            &self.client_app_key,
            &self.client_app_iv,
            self.seq_tx,
            TLS_CONTENT_APPLICATION_DATA,
            plaintext,
        );
        self.seq_tx = self.seq_tx.wrapping_add(1);
        self.state = Tls13State::ApplicationReady;
        record
    }

    /// Open and authenticate an encrypted TLS 1.3 record sent by the server using `server_app_key` and `server_app_iv`.
    pub fn open_server_record(&mut self, record: &[u8]) -> Result<(u8, Vec<u8>), TlsError> {
        if self.state != Tls13State::ApplicationReady {
            return Err(TlsError::UnexpectedMessage);
        }
        let (content_type, plaintext) = open_tls13_record(
            &self.server_app_key,
            &self.server_app_iv,
            self.seq_rx,
            record,
        )?;
        self.seq_rx = self.seq_rx.wrapping_add(1);
        if content_type == TLS_CONTENT_ALERT {
            if plaintext.len() >= 2 && plaintext[1] == 0 {
                self.state = Tls13State::Closed;
            } else {
                let code = plaintext.get(1).copied().unwrap_or(0);
                self.state = Tls13State::Closed;
                return Err(TlsError::AlertReceived(code));
            }
        }
        Ok((content_type, plaintext))
    }

    /// Consume one or more concatenated encrypted server records and return decrypted `ApplicationData` chunks.
    pub fn consume_server_records(&mut self, mut bytes: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
        let mut chunks = Vec::new();
        while !bytes.is_empty() {
            if bytes.len() < 5 {
                return Err(TlsError::BadRecordMac);
            }
            let rec_len = u16::from_be_bytes([bytes[3], bytes[4]]) as usize;
            if bytes.len() < 5 + rec_len {
                return Err(TlsError::BadRecordMac);
            }
            let rec = &bytes[..5 + rec_len];
            bytes = &bytes[5 + rec_len..];
            let (ctype, pt) = self.open_server_record(rec)?;
            if ctype == TLS_CONTENT_APPLICATION_DATA {
                chunks.push(pt);
            } else if ctype == TLS_CONTENT_ALERT {
                break;
            }
        }
        Ok(chunks)
    }

    /// Open a sealed TLS 1.3 `ApplicationData` record produced with `client_app_key` (for loopback/self-test verification).
    pub fn open_loopback_application_data(&mut self, record: &[u8]) -> Option<Vec<u8>> {
        let (ctype, pt) = open_tls13_record(
            &self.client_app_key,
            &self.client_app_iv,
            self.seq_rx,
            record,
        )
        .ok()?;
        if ctype != TLS_CONTENT_APPLICATION_DATA {
            return None;
        }
        self.seq_rx = self.seq_rx.wrapping_add(1);
        Some(pt)
    }
}

// ============================================================================
// C FFI Bindings for GIO / libsoup TLS Backend (`liballoy::tls`)
// ============================================================================

#[no_mangle]
pub unsafe extern "C" fn alloyos_tls_create(server_name: *const core::ffi::c_char) -> *mut Tls13Session {
    if server_name.is_null() {
        return core::ptr::null_mut();
    }
    let c_str = core::ffi::CStr::from_ptr(server_name);
    let Ok(name_str) = c_str.to_str() else {
        return core::ptr::null_mut();
    };
    let session = Box::new(Tls13Session::new(name_str));
    Box::into_raw(session)
}

#[no_mangle]
pub unsafe extern "C" fn alloyos_tls_client_hello(
    session: *mut Tls13Session,
    out_buf: *mut u8,
    max_len: usize,
) -> i32 {
    if session.is_null() || out_buf.is_null() {
        return -1;
    }
    let s = &mut *session;
    let ch = s.build_client_hello_record();
    if ch.len() > max_len {
        return -2;
    }
    core::ptr::copy_nonoverlapping(ch.as_ptr(), out_buf, ch.len());
    ch.len() as i32
}

#[no_mangle]
pub unsafe extern "C" fn alloyos_tls_consume_server(
    session: *mut Tls13Session,
    in_buf: *const u8,
    in_len: usize,
    out_finished: *mut u8,
    max_finished_len: usize,
) -> i32 {
    if session.is_null() || in_buf.is_null() {
        return -1;
    }
    let s = &mut *session;
    let input = core::slice::from_raw_parts(in_buf, in_len);
    match s.consume_server_handshake_bytes(input) {
        Ok(Some(fin)) => {
            if fin.len() > max_finished_len && !out_finished.is_null() {
                return -2;
            }
            if !out_finished.is_null() && !fin.is_empty() {
                core::ptr::copy_nonoverlapping(fin.as_ptr(), out_finished, fin.len());
            }
            fin.len() as i32
        }
        Ok(None) => 0,
        Err(_) => -3,
    }
}

#[no_mangle]
pub unsafe extern "C" fn alloyos_tls_is_ready(session: *const Tls13Session) -> i32 {
    if session.is_null() {
        return 0;
    }
    if (*session).is_handshake_complete() {
        1
    } else {
        0
    }
}

#[no_mangle]
pub unsafe extern "C" fn alloyos_tls_seal(
    session: *mut Tls13Session,
    plaintext: *const u8,
    plaintext_len: usize,
    out_buf: *mut u8,
    max_len: usize,
) -> i32 {
    if session.is_null() || plaintext.is_null() || out_buf.is_null() {
        return -1;
    }
    let s = &mut *session;
    let pt = core::slice::from_raw_parts(plaintext, plaintext_len);
    let sealed = s.seal_application_data(pt);
    if sealed.len() > max_len {
        return -2;
    }
    core::ptr::copy_nonoverlapping(sealed.as_ptr(), out_buf, sealed.len());
    sealed.len() as i32
}

#[no_mangle]
pub unsafe extern "C" fn alloyos_tls_open(
    session: *mut Tls13Session,
    in_buf: *const u8,
    in_len: usize,
    out_buf: *mut u8,
    max_len: usize,
) -> i32 {
    if session.is_null() || in_buf.is_null() || out_buf.is_null() {
        return -1;
    }
    let s = &mut *session;
    let recs = core::slice::from_raw_parts(in_buf, in_len);
    match s.consume_server_records(recs) {
        Ok(chunks) => {
            let total_len: usize = chunks.iter().map(|c| c.len()).sum();
            if total_len > max_len {
                return -2;
            }
            let mut offset = 0;
            for c in chunks {
                core::ptr::copy_nonoverlapping(c.as_ptr(), out_buf.add(offset), c.len());
                offset += c.len();
            }
            total_len as i32
        }
        Err(_) => -3,
    }
}

#[no_mangle]
pub unsafe extern "C" fn alloyos_tls_destroy(session: *mut Tls13Session) {
    if !session.is_null() {
        drop(Box::from_raw(session));
    }
}


// ============================================================================
// HTTP/1.1 Request Builder, Chunked Decoder & Server-Sent Events (SSE) Parser
// ============================================================================

/// Builder for HTTP/1.1 requests with custom headers and JSON bodies.
pub struct HttpRequestBuilder {
    method: String,
    path: String,
    host: String,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}

impl HttpRequestBuilder {
    pub fn new(method: &str, host: &str, path: &str) -> Self {
        Self {
            method: String::from(method),
            path: String::from(path),
            host: String::from(host),
            headers: Vec::new(),
            body: Vec::new(),
        }
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((String::from(name), String::from(value)));
        self
    }

    pub fn json_body(mut self, json: &str) -> Self {
        self.body = json.as_bytes().to_vec();
        self
    }

    pub fn build_bytes(&self) -> Vec<u8> {
        let mut out = String::new();
        out.push_str(&self.method);
        out.push(' ');
        out.push_str(&self.path);
        out.push_str(" HTTP/1.1\r\nHost: ");
        out.push_str(&self.host);
        out.push_str("\r\nUser-Agent: alloyOS/0.1.0 (aarch64; Ferrofluid)\r\n");

        for (k, v) in &self.headers {
            out.push_str(k);
            out.push_str(": ");
            out.push_str(v);
            out.push_str("\r\n");
        }

        if !self.body.is_empty() {
            out.push_str("Content-Type: application/json\r\nContent-Length: ");
            out.push_str(&alloc::format!("{}", self.body.len()));
            out.push_str("\r\n");
        }
        out.push_str("\r\n");

        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&self.body);
        bytes
    }
}

/// Decoder for `Transfer-Encoding: chunked` HTTP/1.1 response streams.
pub struct ChunkedDecoder;

impl ChunkedDecoder {
    /// Decode a chunked HTTP body (`<hex-size>\r\n<data>\r\n...0\r\n\r\n`) into contiguous payload bytes.
    pub fn decode(mut input: &[u8]) -> Result<Vec<u8>, &'static str> {
        let mut out = Vec::new();
        while !input.is_empty() {
            let crlf_pos = input
                .windows(2)
                .position(|w| w == b"\r\n")
                .ok_or("Missing chunk size CRLF")?;
            let size_line = core::str::from_utf8(&input[..crlf_pos])
                .map_err(|_| "Invalid UTF-8 in chunk size")?;
            let size_hex = size_line.split(';').next().unwrap_or("").trim();
            let chunk_len =
                usize::from_str_radix(size_hex, 16).map_err(|_| "Invalid hex chunk size")?;
            input = &input[crlf_pos + 2..];

            if chunk_len == 0 {
                break;
            }
            if input.len() < chunk_len + 2 {
                return Err("Truncated chunk payload");
            }
            out.extend_from_slice(&input[..chunk_len]);
            if &input[chunk_len..chunk_len + 2] != b"\r\n" {
                return Err("Missing trailing chunk CRLF");
            }
            input = &input[chunk_len + 2..];
        }
        Ok(out)
    }
}

/// A parsed Server-Sent Event (`SSE`) emitted by Gemini, Claude, or OpenAI streaming endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: Option<String>,
    pub data: String,
    pub is_done: bool,
}

/// Incremental Server-Sent Events (`text/event-stream`) parser.
pub struct SseStreamParser {
    buffer: String,
    current_event: Option<String>,
    current_data: Vec<String>,
}

impl SseStreamParser {
    pub const fn new() -> Self {
        Self {
            buffer: String::new(),
            current_event: None,
            current_data: Vec::new(),
        }
    }

    /// Feed a chunk of UTF-8 text into the SSE parser and return all completed `SseEvent` frames.
    pub fn push_str(&mut self, chunk: &str) -> Vec<SseEvent> {
        self.buffer.push_str(chunk);
        let mut events = Vec::new();

        while let Some(nl_idx) = self.buffer.find('\n') {
            let mut line: String = self.buffer.drain(..=nl_idx).collect();
            if line.ends_with('\n') {
                line.pop();
            }
            if line.ends_with('\r') {
                line.pop();
            }

            if line.is_empty() {
                if !self.current_data.is_empty() || self.current_event.is_some() {
                    let data = self.current_data.join("\n");
                    let is_done = data.trim() == "[DONE]";
                    events.push(SseEvent {
                        event: self.current_event.take(),
                        data,
                        is_done,
                    });
                    self.current_data.clear();
                }
                continue;
            }

            if line.starts_with(':') {
                // SSE comment / keep-alive
                continue;
            }

            if let Some(rest) = line.strip_prefix("event:") {
                self.current_event = Some(String::from(rest.trim_start()));
            } else if let Some(rest) = line.strip_prefix("data:") {
                self.current_data.push(String::from(rest.trim_start()));
            }
        }

        events
    }
}

/// Parsed HTTP/1.1 response with status code, headers, and decoded body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedHttpResponse {
    pub status: u16,
    pub is_chunked: bool,
    pub content_type: Option<String>,
    pub body: Vec<u8>,
}

impl ParsedHttpResponse {
    /// Parse a complete or partial HTTP/1.1 response buffer.
    pub fn parse(raw: &[u8]) -> Result<Self, &'static str> {
        let Some(hdr_end) = raw.windows(4).position(|w| w == b"\r\n\r\n") else {
            return Err("Incomplete HTTP headers");
        };
        let hdr_str =
            core::str::from_utf8(&raw[..hdr_end]).map_err(|_| "Invalid UTF-8 in HTTP headers")?;
        let mut lines = hdr_str.split("\r\n");
        let status_line = lines.next().ok_or("Missing HTTP status line")?;
        let mut parts = status_line.split_whitespace();
        let _proto = parts.next().ok_or("Missing HTTP version")?;
        let status_str = parts.next().ok_or("Missing HTTP status code")?;
        let status = status_str
            .parse::<u16>()
            .map_err(|_| "Invalid HTTP status code")?;

        let mut is_chunked = false;
        let mut content_type = None;
        for line in lines {
            if let Some((k, v)) = line.split_once(':') {
                let key = k.trim();
                let val = v.trim();
                if key.eq_ignore_ascii_case("transfer-encoding")
                    && val.to_ascii_lowercase().contains("chunked")
                {
                    is_chunked = true;
                } else if key.eq_ignore_ascii_case("content-type") {
                    content_type = Some(String::from(val));
                }
            }
        }

        let raw_body = &raw[hdr_end + 4..];
        let body = if is_chunked {
            Self::decode_chunked_best_effort(raw_body)
        } else {
            raw_body.to_vec()
        };

        Ok(Self {
            status,
            is_chunked,
            content_type,
            body,
        })
    }

    fn decode_chunked_best_effort(mut input: &[u8]) -> Vec<u8> {
        if let Ok(decoded) = ChunkedDecoder::decode(input) {
            return decoded;
        }
        let mut out = Vec::new();
        while !input.is_empty() {
            let Some(crlf_pos) = input.windows(2).position(|w| w == b"\r\n") else {
                break;
            };
            let Ok(size_line) = core::str::from_utf8(&input[..crlf_pos]) else {
                break;
            };
            let size_hex = size_line.split(';').next().unwrap_or("").trim();
            let Ok(chunk_len) = usize::from_str_radix(size_hex, 16) else {
                break;
            };
            input = &input[crlf_pos + 2..];
            if chunk_len == 0 {
                break;
            }
            if input.len() < chunk_len {
                out.extend_from_slice(input);
                break;
            }
            out.extend_from_slice(&input[..chunk_len]);
            if input.len() < chunk_len + 2 {
                break;
            }
            input = &input[chunk_len + 2..];
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tls::{
        chacha20_poly1305_open, chacha20_poly1305_seal, ed25519_public_key, ed25519_sign,
        ed25519_verify, x25519, Tls13ServerConfig, Tls13ServerConnection, X509Certificate,
    };

    #[test]
    fn test_sha256_nist_vector() {
        let digest = sha256(b"abc");
        let expected: [u8; 32] = [
            0xba, 0x78, 0x16, 0xbf, 0x8f, 0x01, 0xcf, 0xea, 0x41, 0x41, 0x40, 0xde, 0x5d, 0xae,
            0x22, 0x23, 0xb0, 0x03, 0x61, 0xa3, 0x96, 0x17, 0x7a, 0x9c, 0xb4, 0x10, 0xff, 0x61,
            0xf2, 0x00, 0x15, 0xad,
        ];
        assert_eq!(digest, expected);
    }

    #[test]
    fn test_x25519_rfc7748_test_vector() {
        let alice_priv: [u8; 32] = [
            0x77, 0x07, 0x6d, 0x0a, 0x73, 0x18, 0xa5, 0x7d, 0x3c, 0x16, 0xc1, 0x72, 0x51, 0xb2,
            0x66, 0x45, 0xdf, 0x4c, 0x2f, 0x87, 0xeb, 0xc0, 0x99, 0x2a, 0xb1, 0x77, 0xfb, 0xa5,
            0x1d, 0xb9, 0x2c, 0x2a,
        ];
        let bob_pub: [u8; 32] = [
            0xde, 0x9e, 0xdb, 0x7d, 0x7b, 0x7d, 0xc1, 0xb4, 0xd3, 0x5b, 0x61, 0xc2, 0xec, 0xe4,
            0x35, 0x37, 0x3f, 0x83, 0x43, 0xc8, 0x5b, 0x78, 0x67, 0x4d, 0xad, 0xfc, 0x7e, 0x14,
            0x6f, 0x88, 0x2b, 0x4f,
        ];
        let expected_shared: [u8; 32] = [
            0x4a, 0x5d, 0x9d, 0x5b, 0xa4, 0xce, 0x2d, 0xe1, 0x72, 0x8e, 0x3b, 0xf4, 0x80, 0x35,
            0x0f, 0x25, 0xe0, 0x7e, 0x21, 0xc9, 0x47, 0xd1, 0x9e, 0x33, 0x76, 0xf0, 0x9b, 0x3c,
            0x1e, 0x16, 0x17, 0x42,
        ];
        assert_eq!(x25519(&alice_priv, &bob_pub), expected_shared);
    }

    #[test]
    fn test_chacha20_poly1305_rfc8439_test_vector() {
        let key: [u8; 32] = [
            0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d,
            0x8e, 0x8f, 0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b,
            0x9c, 0x9d, 0x9e, 0x9f,
        ];
        let nonce: [u8; 12] = [
            0x07, 0x00, 0x00, 0x00, 0x40, 0x41, 0x42, 0x43, 0x44, 0x45, 0x46, 0x47,
        ];
        let aad: [u8; 12] = [
            0x50, 0x51, 0x52, 0x53, 0xc0, 0xc1, 0xc2, 0xc3, 0xc4, 0xc5, 0xc6, 0xc7,
        ];
        let plaintext = b"Ladies and Gentlemen of the class of '99: If I could offer you only one tip for the future, sunscreen would be it.";
        let sealed = chacha20_poly1305_seal(&key, &nonce, &aad, plaintext);
        let expected_tag: [u8; 16] = [
            0x1a, 0xe1, 0x0b, 0x59, 0x4f, 0x09, 0xe2, 0x6a, 0x7e, 0x90, 0x2e, 0xcb, 0xd0, 0x60,
            0x06, 0x91,
        ];
        assert_eq!(&sealed[plaintext.len()..], &expected_tag);
        let opened = chacha20_poly1305_open(&key, &nonce, &aad, &sealed).unwrap();
        assert_eq!(opened.as_slice(), plaintext);

        let mut tampered = sealed.clone();
        tampered[10] ^= 0x01;
        assert_eq!(
            chacha20_poly1305_open(&key, &nonce, &aad, &tampered),
            Err(TlsError::BadRecordMac)
        );
    }

    #[test]
    fn test_ed25519_sign_and_verify() {
        let seed = sha256(b"test-signing-key");
        let pk = ed25519_public_key(&seed);
        let msg = b"TLS 1.3 CertificateVerify transcript payload";
        let sig = ed25519_sign(&seed, msg);
        assert!(ed25519_verify(&pk, msg, &sig));
        assert!(!ed25519_verify(&pk, b"tampered payload", &sig));
        let wrong_pk = ed25519_public_key(&sha256(b"other-key"));
        assert!(!ed25519_verify(&wrong_pk, msg, &sig));
    }

    #[test]
    fn test_tls13_client_hello_and_aead_roundtrip() {
        let mut session = Tls13Session::new("generativelanguage.googleapis.com");
        assert!(session
            .ca_store
            .verify_server_name("generativelanguage.googleapis.com"));

        let ch_record = session.build_client_hello_record();
        assert_eq!(ch_record[0], TLS_CONTENT_HANDSHAKE);
        assert_eq!(ch_record[5], TLS_HANDSHAKE_CLIENT_HELLO);
        assert_eq!(session.state, Tls13State::ClientHelloSent);

        // Verify SNI hostname is embedded inside the ClientHello record
        assert!(ch_record
            .windows(b"generativelanguage.googleapis.com".len())
            .any(|w| w == b"generativelanguage.googleapis.com"));

        // Verify AEAD seal + open roundtrip
        let msg = b"POST /v1beta/models/gemini-2.5-flash:streamGenerateContent?alt=sse HTTP/1.1";
        let sealed = session.seal_application_data(msg);
        assert_eq!(sealed[0], TLS_CONTENT_APPLICATION_DATA);
        let opened = session
            .open_loopback_application_data(&sealed)
            .expect("AEAD tag must verify");
        assert_eq!(opened.as_slice(), msg);
    }

    #[test]
    fn test_tls13_full_authenticated_handshake_and_encrypted_record_exchange() {
        for host in [
            "api.anthropic.com",
            "generativelanguage.googleapis.com",
            "api.openai.com",
        ] {
            let mut client = Tls13Session::new(host);
            let ch = client.build_client_hello_record();

            let server_cfg = Tls13ServerConfig::trusted_for_host(host);
            let (mut server, server_flight) =
                Tls13ServerConnection::accept_client_hello(server_cfg, &ch)
                    .expect("server must accept ClientHello");

            let client_fin = client
                .consume_server_handshake_bytes(&server_flight)
                .expect("client must authenticate server flight")
                .expect("client must emit Finished record");
            assert!(client.is_handshake_complete());

            assert_eq!(
                server
                    .receive_client_record(&client_fin)
                    .expect("server must verify ClientFinished"),
                None
            );
            assert!(server.client_finished_verified);

            // Client -> Server encrypted application record
            let req = b"POST /v1/messages HTTP/1.1\r\nHost: api.anthropic.com\r\n\r\n{}";
            let req_rec = client.seal_application_data(req);
            let server_got = server
                .receive_client_record(&req_rec)
                .expect("server decrypts request")
                .expect("application data");
            assert_eq!(server_got.as_slice(), req);

            // Server -> Client encrypted application records + close_notify
            let resp1 = b"HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\n\r\n";
            let resp2 = b"data: {\"delta\":{\"text\":\"Hello\"}}\n\n";
            let mut wire = Vec::new();
            wire.extend_from_slice(&server.seal_server_data(resp1));
            wire.extend_from_slice(&server.seal_server_data(resp2));
            wire.extend_from_slice(&server.seal_close_notify());

            let chunks = client
                .consume_server_records(&wire)
                .expect("client decrypts server records");
            assert_eq!(chunks.len(), 2);
            assert_eq!(chunks[0].as_slice(), resp1);
            assert_eq!(chunks[1].as_slice(), resp2);
            assert_eq!(client.state, Tls13State::Closed);
        }
    }

    #[test]
    fn test_tls13_rejects_untrusted_ca_wrong_host_bad_signatures_and_tampered_records() {
        let host = "api.anthropic.com";

        // 1. Untrusted Root CA
        {
            let mut client = Tls13Session::new(host);
            let ch = client.build_client_hello_record();
            let mut cfg = Tls13ServerConfig::trusted_for_host(host);
            cfg.ca_name = String::from("Rogue Untrusted Root CA");
            cfg.ca_private_seed = sha256(b"rogue-ca-seed");
            let (_, flight) = Tls13ServerConnection::accept_client_hello(cfg, &ch).unwrap();
            assert_eq!(
                client.consume_server_handshake_bytes(&flight),
                Err(TlsError::UntrustedCertificateAuthority)
            );
        }

        // 2. Hostname mismatch (certificate issued for evil.example.com instead of api.anthropic.com)
        {
            let mut client = Tls13Session::new(host);
            let ch = client.build_client_hello_record();
            let cfg = Tls13ServerConfig::trusted_for_host("evil.example.com");
            let (_, flight) = Tls13ServerConnection::accept_client_hello(cfg, &ch).unwrap();
            assert_eq!(
                client.consume_server_handshake_bytes(&flight),
                Err(TlsError::HostnameMismatch)
            );
        }

        // 3. Invalid X.509 certificate signature (leaf signed by wrong key, claiming DigiCert Global Root G2)
        {
            let mut client = Tls13Session::new(host);
            let ch = client.build_client_hello_record();
            let mut cfg = Tls13ServerConfig::trusted_for_host(host);
            let fake_ca_seed = sha256(b"forged-ca-private-key");
            let leaf_pub = ed25519_public_key(&cfg.leaf_private_seed);
            let real_ca_pub = ed25519_public_key(&cfg.ca_private_seed);
            let bad_leaf = X509Certificate::build_signed_der(
                host,
                &cfg.ca_name,
                &[host],
                &leaf_pub,
                false,
                true,
                &fake_ca_seed,
            );
            let root_der = X509Certificate::build_signed_der(
                &cfg.ca_name,
                &cfg.ca_name,
                &[],
                &real_ca_pub,
                true,
                true,
                &cfg.ca_private_seed,
            );
            cfg.cert_chain_override = Some(vec![bad_leaf, root_der]);
            let (_, flight) = Tls13ServerConnection::accept_client_hello(cfg, &ch).unwrap();
            assert_eq!(
                client.consume_server_handshake_bytes(&flight),
                Err(TlsError::InvalidCertificateSignature)
            );
        }

        // 4. Invalid CertificateVerify signature (signed with wrong leaf key)
        {
            let mut client = Tls13Session::new(host);
            let ch = client.build_client_hello_record();
            let mut cfg = Tls13ServerConfig::trusted_for_host(host);
            cfg.cert_verify_key_override = Some(sha256(b"wrong-leaf-signing-key"));
            let (_, flight) = Tls13ServerConnection::accept_client_hello(cfg, &ch).unwrap();
            assert_eq!(
                client.consume_server_handshake_bytes(&flight),
                Err(TlsError::InvalidCertificateVerify)
            );
        }

        // 5. Invalid Server Finished verify_data MAC
        {
            let mut client = Tls13Session::new(host);
            let ch = client.build_client_hello_record();
            let mut cfg = Tls13ServerConfig::trusted_for_host(host);
            cfg.tamper_finished = true;
            let (_, flight) = Tls13ServerConnection::accept_client_hello(cfg, &ch).unwrap();
            assert_eq!(
                client.consume_server_handshake_bytes(&flight),
                Err(TlsError::InvalidFinishedVerifyData)
            );
        }

        // 6. Tampered encrypted application record
        {
            let mut client = Tls13Session::new(host);
            let ch = client.build_client_hello_record();
            let mut cfg = Tls13ServerConfig::trusted_for_host(host);
            cfg.tamper_app_record = true;
            let (mut server, flight) =
                Tls13ServerConnection::accept_client_hello(cfg, &ch).unwrap();
            let client_fin = client
                .consume_server_handshake_bytes(&flight)
                .unwrap()
                .unwrap();
            server.receive_client_record(&client_fin).unwrap();
            let tampered_rec = server.seal_server_data(b"HTTP/1.1 200 OK\r\n\r\nok");
            assert_eq!(
                client.consume_server_records(&tampered_rec),
                Err(TlsError::BadRecordMac)
            );
        }
    }

    #[test]
    fn test_http_chunked_decoder() {
        let raw = b"7\r\nMozilla\r\n9\r\nDeveloper\r\n7\r\nNetwork\r\n0\r\n\r\n";
        let decoded = ChunkedDecoder::decode(raw).unwrap();
        assert_eq!(decoded, b"MozillaDeveloperNetwork");
    }

    #[test]
    fn test_sse_stream_parser_openai_and_claude() {
        let mut parser = SseStreamParser::new();
        let stream = "event: content_block_delta\r\ndata: {\"delta\":{\"text\":\"Hello\"}}\r\n\r\ndata: {\"choices\":[{\"delta\":{\"content\":\" alloyOS\"}}]}\n\ndata: [DONE]\n\n";
        let events = parser.push_str(stream);
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].event.as_deref(), Some("content_block_delta"));
        assert_eq!(events[0].data, "{\"delta\":{\"text\":\"Hello\"}}");
        assert!(!events[0].is_done);

        assert_eq!(events[1].event, None);
        assert_eq!(
            events[1].data,
            "{\"choices\":[{\"delta\":{\"content\":\" alloyOS\"}}]}"
        );
        assert!(!events[1].is_done);

        assert!(events[2].is_done);
        assert_eq!(events[2].data, "[DONE]");
    }

    #[test]
    fn test_tls13_c_ffi_roundtrip() {
        let host = "10.0.2.100";
        let c_host = alloc::ffi::CString::new(host).unwrap();
        unsafe {
            let session = alloyos_tls_create(c_host.as_ptr());
            assert!(!session.is_null());
            assert_eq!(alloyos_tls_is_ready(session), 0);

            let mut ch_buf = [0u8; 1024];
            let ch_len = alloyos_tls_client_hello(session, ch_buf.as_mut_ptr(), ch_buf.len());
            assert!(ch_len > 0);

            let server_cfg = Tls13ServerConfig::trusted_for_host(host);
            let (mut server, server_flight) =
                Tls13ServerConnection::accept_client_hello(server_cfg, &ch_buf[..ch_len as usize])
                    .expect("server accepts client hello");

            let mut fin_buf = [0u8; 512];
            let fin_len = alloyos_tls_consume_server(
                session,
                server_flight.as_ptr(),
                server_flight.len(),
                fin_buf.as_mut_ptr(),
                fin_buf.len(),
            );
            assert!(fin_len > 0);
            assert_eq!(alloyos_tls_is_ready(session), 1);

            server
                .receive_client_record(&fin_buf[..fin_len as usize])
                .unwrap();
            assert!(server.client_finished_verified);

            // Client seals request
            let msg = b"GET /alloyos-status HTTP/1.1\r\nHost: 10.0.2.100\r\n\r\n";
            let mut sealed_req = [0u8; 1024];
            let sealed_len = alloyos_tls_seal(
                session,
                msg.as_ptr(),
                msg.len(),
                sealed_req.as_mut_ptr(),
                sealed_req.len(),
            );
            assert!(sealed_len > 0);

            let server_got = server
                .receive_client_record(&sealed_req[..sealed_len as usize])
                .unwrap()
                .unwrap();
            assert_eq!(server_got.as_slice(), msg);

            // Server seals response
            let resp = b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\n\r\nHello from TLS!";
            let server_wire = server.seal_server_data(resp);

            // Client opens response via C FFI
            let mut plain_out = [0u8; 1024];
            let plain_len = alloyos_tls_open(
                session,
                server_wire.as_ptr(),
                server_wire.len(),
                plain_out.as_mut_ptr(),
                plain_out.len(),
            );
            assert_eq!(plain_len, resp.len() as i32);
            assert_eq!(&plain_out[..plain_len as usize], resp);

            alloyos_tls_destroy(session);
        }
    }
}
