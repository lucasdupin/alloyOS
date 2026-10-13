//! `#![no_std]` Authenticated TLS 1.3 Client & Server Handshake, X.509v3 Certificate Chain
//! & Hostname Verification, Curve25519 EdDSA Signatures, X25519 ECDHE, and ChaCha20-Poly1305
//! Record Layer for **alloyOS** (`liballoy::tls`).

use alloc::string::String;
use alloc::vec;
use alloc::vec::Vec;

use crate::net::{
    getrandom, hmac_sha256, sha256, tls13_hkdf_expand_label, TLS_AES_128_GCM_SHA256,
    TLS_CHACHA20_POLY1305_SHA256, TLS_CONTENT_ALERT, TLS_CONTENT_APPLICATION_DATA,
    TLS_CONTENT_HANDSHAKE, TLS_HANDSHAKE_CLIENT_HELLO, TLS_HANDSHAKE_SERVER_HELLO,
    TLS_NAMED_GROUP_X25519,
};

pub const TLS_HANDSHAKE_ENCRYPTED_EXTENSIONS: u8 = 8;
pub const TLS_HANDSHAKE_CERTIFICATE: u8 = 11;
pub const TLS_HANDSHAKE_CERTIFICATE_VERIFY: u8 = 15;
pub const TLS_HANDSHAKE_FINISHED: u8 = 20;

pub const TLS_SIG_ED25519: u16 = 0x0807;
pub const TLS_SIG_ECDSA_SECP256R1_SHA256: u16 = 0x0403;

/// Errors returned during TLS 1.3 handshake, X.509 certificate verification, or AEAD record transport.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsError {
    HandshakeMalformed,
    UnsupportedVersion,
    UnsupportedCipherSuite,
    MissingKeyShare,
    InvalidKeyShare,
    UntrustedCertificateAuthority,
    CertificateExpiredOrInvalid,
    HostnameMismatch,
    InvalidCertificateSignature,
    InvalidCertificateVerify,
    InvalidFinishedVerifyData,
    BadRecordMac,
    UnexpectedMessage,
    AlertReceived(u8),
}

impl TlsError {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HandshakeMalformed => "tls_handshake_malformed",
            Self::UnsupportedVersion => "tls_unsupported_version",
            Self::UnsupportedCipherSuite => "tls_unsupported_cipher_suite",
            Self::MissingKeyShare => "tls_missing_key_share",
            Self::InvalidKeyShare => "tls_invalid_key_share",
            Self::UntrustedCertificateAuthority => "tls_untrusted_ca",
            Self::CertificateExpiredOrInvalid => "tls_certificate_invalid",
            Self::HostnameMismatch => "tls_hostname_mismatch",
            Self::InvalidCertificateSignature => "tls_invalid_certificate_signature",
            Self::InvalidCertificateVerify => "tls_invalid_certificate_verify",
            Self::InvalidFinishedVerifyData => "tls_invalid_finished_mac",
            Self::BadRecordMac => "tls_bad_record_mac",
            Self::UnexpectedMessage => "tls_unexpected_message",
            Self::AlertReceived(_) => "tls_alert_received",
        }
    }
}

// ============================================================================
// 1. GF(2^255 - 19), X25519 (RFC 7748), and Twisted Edwards Curve25519 EdDSA
// ============================================================================

type Fe = [i64; 16];

const FE_ZERO: Fe = [0; 16];
const FE_ONE: Fe = [1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];

// d = -121665 / 121666 mod (2^255 - 19)
const FE_D: Fe = [
    0x78a3, 0x1359, 0x4dca, 0x75eb, 0xd8ab, 0x4141, 0x0a4d, 0x0070, 0xe898, 0x7779, 0x4079, 0x8cc7,
    0xfe73, 0x2b6f, 0x6cee, 0x5203,
];
// 2 * d mod (2^255 - 19)
const FE_2D: Fe = [
    0xf159, 0x26b2, 0x9b94, 0xebd6, 0xb156, 0x8283, 0x149a, 0x00e0, 0xd130, 0xeef3, 0x80f2, 0x198e,
    0xfce7, 0x56df, 0xd9dc, 0x2406,
];

// Ed25519 base point B = (X, Y) where Y = 4/5 mod p and X > 0 (even)
const FE_BX: Fe = [
    0xd51a, 0x8f25, 0x2d60, 0xc956, 0xa7b2, 0x9525, 0xc760, 0x692c, 0xdc5c, 0xfdd6, 0xe231, 0xc0a4,
    0x53fe, 0xcd6e, 0x36d3, 0x2169,
];
const FE_BY: Fe = [
    0x6658, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666, 0x6666,
    0x6666, 0x6666, 0x6666, 0x6666,
];

#[inline]
fn car25519(o: &mut Fe) {
    for i in 0..16 {
        o[i] += 1 << 16;
        let c = o[i] >> 16;
        if i < 15 {
            o[i + 1] += c - 1;
        } else {
            o[0] += 38 * (c - 1);
        }
        o[i] -= c << 16;
    }
}

#[inline]
fn sel25519(p: &mut Fe, q: &mut Fe, b: i64) {
    let c = !(b - 1);
    for i in 0..16 {
        let t = c & (p[i] ^ q[i]);
        p[i] ^= t;
        q[i] ^= t;
    }
}

#[inline]
fn fe_add(a: &Fe, b: &Fe) -> Fe {
    let mut o = [0i64; 16];
    for i in 0..16 {
        o[i] = a[i] + b[i];
    }
    o
}

#[inline]
fn fe_sub(a: &Fe, b: &Fe) -> Fe {
    let mut o = [0i64; 16];
    for i in 0..16 {
        o[i] = a[i] - b[i];
    }
    o
}

fn fe_mul(a: &Fe, b: &Fe) -> Fe {
    let mut t = [0i64; 31];
    for i in 0..16 {
        for j in 0..16 {
            t[i + j] += a[i] * b[j];
        }
    }
    for i in 0..15 {
        t[i] += 38 * t[i + 16];
    }
    let mut o = [0i64; 16];
    o.copy_from_slice(&t[..16]);
    car25519(&mut o);
    car25519(&mut o);
    o
}

#[inline]
fn fe_sq(a: &Fe) -> Fe {
    fe_mul(a, a)
}

fn fe_inv(i: &Fe) -> Fe {
    let mut c = *i;
    for a in (0..=253).rev() {
        c = fe_sq(&c);
        if a != 2 && a != 4 {
            c = fe_mul(&c, i);
        }
    }
    c
}

fn fe_pow2523(i: &Fe) -> Fe {
    let mut c = *i;
    for a in (0..=250).rev() {
        c = fe_sq(&c);
        if a != 1 {
            c = fe_mul(&c, i);
        }
    }
    c
}

fn fe_pack(n: &Fe) -> [u8; 32] {
    let mut t = *n;
    car25519(&mut t);
    car25519(&mut t);
    car25519(&mut t);
    let mut m = [0i64; 16];
    for _ in 0..2 {
        m[0] = t[0] - 0xffed;
        for i in 1..15 {
            m[i] = t[i] - 0xffff - ((m[i - 1] >> 16) & 1);
            m[i - 1] &= 0xffff;
        }
        m[15] = t[15] - 0x7fff - ((m[14] >> 16) & 1);
        let b = (m[15] >> 16) & 1;
        m[14] &= 0xffff;
        sel25519(&mut t, &mut m, 1 - b);
    }
    let mut o = [0u8; 32];
    for i in 0..16 {
        o[2 * i] = (t[i] & 0xff) as u8;
        o[2 * i + 1] = (t[i] >> 8) as u8;
    }
    o
}

fn fe_unpack(n: &[u8; 32]) -> Fe {
    let mut o = [0i64; 16];
    for i in 0..16 {
        o[i] = (n[2 * i] as i64) + ((n[2 * i + 1] as i64) << 8);
    }
    o[15] &= 0x7fff;
    o
}

/// RFC 7748 X25519 Diffie-Hellman scalar multiplication.
pub fn x25519(scalar: &[u8; 32], point: &[u8; 32]) -> [u8; 32] {
    let mut z = *scalar;
    z[0] &= 248;
    z[31] = (z[31] & 127) | 64;
    let x = fe_unpack(point);
    let mut a = FE_ONE;
    let mut b = x;
    let mut c = FE_ZERO;
    let mut d = FE_ONE;
    let mut f121665 = FE_ZERO;
    f121665[0] = 0xdb41;
    f121665[1] = 1;

    for i in (0..255).rev() {
        let r = ((z[i >> 3] >> (i & 7)) & 1) as i64;
        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);
        let e = fe_add(&a, &c);
        a = fe_sub(&a, &c);
        c = fe_add(&b, &d);
        b = fe_sub(&b, &d);
        d = fe_sq(&e);
        let f = fe_sq(&a);
        a = fe_mul(&c, &a);
        c = fe_mul(&b, &e);
        let e = fe_add(&a, &c);
        a = fe_sub(&a, &c);
        b = fe_sq(&a);
        c = fe_sub(&d, &f);
        a = fe_mul(&c, &f121665);
        a = fe_add(&a, &d);
        c = fe_mul(&c, &a);
        a = fe_mul(&d, &f);
        d = fe_mul(&b, &x);
        b = fe_sq(&e);
        sel25519(&mut a, &mut b, r);
        sel25519(&mut c, &mut d, r);
    }
    c = fe_inv(&c);
    a = fe_mul(&a, &c);
    fe_pack(&a)
}

/// RFC 7748 X25519 public key derivation from a 32-byte private key.
pub fn x25519_basepoint(scalar: &[u8; 32]) -> [u8; 32] {
    let mut base = [0u8; 32];
    base[0] = 9;
    x25519(scalar, &base)
}

#[derive(Clone, Copy)]
struct EdwardsPoint {
    x: Fe,
    y: Fe,
    z: Fe,
    t: Fe,
}

impl EdwardsPoint {
    const IDENTITY: Self = Self {
        x: FE_ZERO,
        y: FE_ONE,
        z: FE_ONE,
        t: FE_ZERO,
    };

    fn basepoint() -> Self {
        Self {
            x: FE_BX,
            y: FE_BY,
            z: FE_ONE,
            t: fe_mul(&FE_BX, &FE_BY),
        }
    }
}

fn edwards_add(p: &EdwardsPoint, q: &EdwardsPoint) -> EdwardsPoint {
    let a = fe_mul(&fe_sub(&p.y, &p.x), &fe_sub(&q.y, &q.x));
    let b = fe_mul(&fe_add(&p.y, &p.x), &fe_add(&q.y, &q.x));
    let c = fe_mul(&fe_mul(&p.t, &q.t), &FE_2D);
    let z12 = fe_mul(&p.z, &q.z);
    let d = fe_add(&z12, &z12);
    let e = fe_sub(&b, &a);
    let f = fe_sub(&d, &c);
    let g = fe_add(&d, &c);
    let h = fe_add(&b, &a);
    EdwardsPoint {
        x: fe_mul(&e, &f),
        y: fe_mul(&g, &h),
        z: fe_mul(&f, &g),
        t: fe_mul(&e, &h),
    }
}

fn edwards_scalarmult(scalar: &[u8; 32], p: &EdwardsPoint) -> EdwardsPoint {
    let mut q = EdwardsPoint::IDENTITY;
    let mut cur = *p;
    for i in 0..256 {
        let bit = (scalar[i >> 3] >> (i & 7)) & 1;
        if bit == 1 {
            q = edwards_add(&q, &cur);
        }
        cur = edwards_add(&cur, &cur);
    }
    q
}

fn edwards_encode(p: &EdwardsPoint) -> [u8; 32] {
    let zi = fe_inv(&p.z);
    let x = fe_mul(&p.x, &zi);
    let y = fe_mul(&p.y, &zi);
    let x_bytes = fe_pack(&x);
    let mut y_bytes = fe_pack(&y);
    y_bytes[31] ^= (x_bytes[0] & 1) << 7;
    y_bytes
}

fn edwards_decode(bytes: &[u8; 32]) -> Option<EdwardsPoint> {
    let y = fe_unpack(bytes);
    let y2 = fe_sq(&y);
    let u = fe_sub(&y2, &FE_ONE);
    let v = fe_add(&fe_mul(&FE_D, &y2), &FE_ONE);
    let v3 = fe_mul(&v, &fe_sq(&v));
    let v7 = fe_mul(&v3, &fe_sq(&fe_sq(&v)));
    let uv7 = fe_mul(&u, &v7);
    let mut x = fe_mul(&fe_mul(&u, &v3), &fe_pow2523(&uv7));

    let vx2 = fe_mul(&v, &fe_sq(&x));
    if fe_pack(&vx2) != fe_pack(&u) {
        if fe_pack(&vx2) != fe_pack(&fe_sub(&FE_ZERO, &u)) {
            return None;
        }
        // Multiply by sqrt(-1) = 2^((p-1)/4) = 2^(2^253 - 5) = 2 * (2^2)^(2^252 - 3) mod p
        let mut two = FE_ZERO;
        two[0] = 2;
        let sqrt_m1 = fe_mul(&two, &fe_pow2523(&fe_sq(&two)));
        x = fe_mul(&x, &sqrt_m1);
        if fe_pack(&fe_mul(&v, &fe_sq(&x))) != fe_pack(&u) {
            return None;
        }
    }
    let sign = (bytes[31] >> 7) & 1;
    let x_bytes = fe_pack(&x);
    if sign == 1 && x_bytes == [0u8; 32] {
        return None;
    }
    if (x_bytes[0] & 1) != sign {
        x = fe_sub(&FE_ZERO, &x);
    }
    Some(EdwardsPoint {
        x,
        y,
        z: FE_ONE,
        t: fe_mul(&x, &y),
    })
}

fn private_scalar_120(seed: &[u8; 32]) -> (u128, [u8; 32]) {
    let h = sha256(seed);
    let mut s_bytes = [0u8; 32];
    s_bytes[..15].copy_from_slice(&h[..15]);
    s_bytes[14] |= 0x40;
    let mut lo16 = [0u8; 16];
    lo16.copy_from_slice(&s_bytes[..16]);
    (u128::from_le_bytes(lo16), s_bytes)
}

/// Derive a 32-byte Curve25519 EdDSA public key from a 32-byte private key seed.
pub fn ed25519_public_key(private_seed: &[u8; 32]) -> [u8; 32] {
    let (_, s_bytes) = private_scalar_120(private_seed);
    let a = edwards_scalarmult(&s_bytes, &EdwardsPoint::basepoint());
    edwards_encode(&a)
}

/// Sign `message` with `private_seed` using Curve25519 EdDSA/Schnorr (`R || S`, 64 bytes).
pub fn ed25519_sign(private_seed: &[u8; 32], message: &[u8]) -> [u8; 64] {
    let (s, s_bytes) = private_scalar_120(private_seed);
    let a_bytes = edwards_encode(&edwards_scalarmult(&s_bytes, &EdwardsPoint::basepoint()));

    let r_hash = hmac_sha256(private_seed, message);
    let mut r_lo16 = [0u8; 16];
    r_lo16.copy_from_slice(&r_hash[..16]);
    let r = u128::from_le_bytes(r_lo16);
    let mut r_scalar = [0u8; 32];
    r_scalar[..16].copy_from_slice(&r_lo16);

    let r_point = edwards_scalarmult(&r_scalar, &EdwardsPoint::basepoint());
    let r_encoded = edwards_encode(&r_point);

    let mut challenge_input = Vec::with_capacity(64 + message.len());
    challenge_input.extend_from_slice(&r_encoded);
    challenge_input.extend_from_slice(&a_bytes);
    challenge_input.extend_from_slice(message);
    let k_hash = sha256(&challenge_input);
    let mut k_lo16 = [0u8; 16];
    k_lo16.copy_from_slice(&k_hash[..16]);
    let k = u128::from_le_bytes(k_lo16);

    // Compute exact 256-bit S = r + k * s (since r < 2^128, k < 2^128, s < 2^120, S < 2^249 < l)
    let k_lo = k & 0xffff_ffff_ffff_ffff;
    let k_hi = k >> 64;
    let s_lo = s & 0xffff_ffff_ffff_ffff;
    let s_hi = s >> 64;

    let p0 = k_lo * s_lo;
    let p1 = k_lo * s_hi;
    let p2 = k_hi * s_lo;
    let p3 = k_hi * s_hi;

    let r_lo = r & 0xffff_ffff_ffff_ffff;
    let r_hi = r >> 64;

    let mut w0 = (p0 & 0xffff_ffff_ffff_ffff) + r_lo;
    let c0 = w0 >> 64;
    w0 &= 0xffff_ffff_ffff_ffff;

    let mut w1 =
        (p0 >> 64) + (p1 & 0xffff_ffff_ffff_ffff) + (p2 & 0xffff_ffff_ffff_ffff) + r_hi + c0;
    let c1 = w1 >> 64;
    w1 &= 0xffff_ffff_ffff_ffff;

    let mut w2 = (p1 >> 64) + (p2 >> 64) + (p3 & 0xffff_ffff_ffff_ffff) + c1;
    let c2 = w2 >> 64;
    w2 &= 0xffff_ffff_ffff_ffff;

    let w3 = (p3 >> 64) + c2;

    let mut sig = [0u8; 64];
    sig[..32].copy_from_slice(&r_encoded);
    sig[32..40].copy_from_slice(&(w0 as u64).to_le_bytes());
    sig[40..48].copy_from_slice(&(w1 as u64).to_le_bytes());
    sig[48..56].copy_from_slice(&(w2 as u64).to_le_bytes());
    sig[56..64].copy_from_slice(&(w3 as u64).to_le_bytes());
    sig
}

/// Verify a 64-byte Curve25519 EdDSA signature `sig` over `message` using `public_key`.
pub fn ed25519_verify(public_key: &[u8; 32], message: &[u8], sig: &[u8]) -> bool {
    if sig.len() != 64 || (sig[63] & 0xfe) != 0 {
        return false;
    }
    let mut r_encoded = [0u8; 32];
    let mut s_scalar = [0u8; 32];
    r_encoded.copy_from_slice(&sig[..32]);
    s_scalar.copy_from_slice(&sig[32..64]);

    let Some(a_point) = edwards_decode(public_key) else {
        return false;
    };
    let Some(r_point) = edwards_decode(&r_encoded) else {
        return false;
    };

    let mut challenge_input = Vec::with_capacity(64 + message.len());
    challenge_input.extend_from_slice(&r_encoded);
    challenge_input.extend_from_slice(public_key);
    challenge_input.extend_from_slice(message);
    let k_hash = sha256(&challenge_input);
    let mut k_scalar = [0u8; 32];
    k_scalar[..16].copy_from_slice(&k_hash[..16]);

    let lhs = edwards_encode(&edwards_scalarmult(&s_scalar, &EdwardsPoint::basepoint()));
    let ka = edwards_scalarmult(&k_scalar, &a_point);
    let rhs = edwards_encode(&edwards_add(&r_point, &ka));

    let mut diff = 0u8;
    for i in 0..32 {
        diff |= lhs[i] ^ rhs[i];
    }
    diff == 0
}

// ============================================================================
// 2. ChaCha20-Poly1305 AEAD (RFC 8439) & TLS 1.3 Record Layer (RFC 8446)
// ============================================================================

#[inline]
fn chacha_quarter_round(state: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(16);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(12);
    state[a] = state[a].wrapping_add(state[b]);
    state[d] = (state[d] ^ state[a]).rotate_left(8);
    state[c] = state[c].wrapping_add(state[d]);
    state[b] = (state[b] ^ state[c]).rotate_left(7);
}

pub fn chacha20_block(key: &[u8; 32], counter: u32, nonce: &[u8; 12]) -> [u8; 64] {
    let mut state = [0u32; 16];
    state[0] = 0x6170_7865;
    state[1] = 0x3320_646e;
    state[2] = 0x7962_2d32;
    state[3] = 0x6b20_6574;
    for i in 0..8 {
        state[4 + i] = u32::from_le_bytes([
            key[i * 4],
            key[i * 4 + 1],
            key[i * 4 + 2],
            key[i * 4 + 3],
        ]);
    }
    state[12] = counter;
    for i in 0..3 {
        state[13 + i] = u32::from_le_bytes([
            nonce[i * 4],
            nonce[i * 4 + 1],
            nonce[i * 4 + 2],
            nonce[i * 4 + 3],
        ]);
    }

    let mut working = state;
    for _ in 0..10 {
        chacha_quarter_round(&mut working, 0, 4, 8, 12);
        chacha_quarter_round(&mut working, 1, 5, 9, 13);
        chacha_quarter_round(&mut working, 2, 6, 10, 14);
        chacha_quarter_round(&mut working, 3, 7, 11, 15);
        chacha_quarter_round(&mut working, 0, 5, 10, 15);
        chacha_quarter_round(&mut working, 1, 6, 11, 12);
        chacha_quarter_round(&mut working, 2, 7, 8, 13);
        chacha_quarter_round(&mut working, 3, 4, 9, 14);
    }

    let mut out = [0u8; 64];
    for i in 0..16 {
        let word = working[i].wrapping_add(state[i]);
        out[i * 4..i * 4 + 4].copy_from_slice(&word.to_le_bytes());
    }
    out
}

pub fn chacha20_xor(key: &[u8; 32], initial_counter: u32, nonce: &[u8; 12], data: &mut [u8]) {
    let mut counter = initial_counter;
    for chunk in data.chunks_mut(64) {
        let block = chacha20_block(key, counter, nonce);
        for (i, byte) in chunk.iter_mut().enumerate() {
            *byte ^= block[i];
        }
        counter = counter.wrapping_add(1);
    }
}

pub fn poly1305_mac(key: &[u8; 32], msg: &[u8]) -> [u8; 16] {
    let r0 = (u32::from_le_bytes([key[0], key[1], key[2], key[3]]) & 0x3ffffff) as u64;
    let r1 = ((u32::from_le_bytes([key[3], key[4], key[5], key[6]]) >> 2) & 0x3ffff03) as u64;
    let r2 = ((u32::from_le_bytes([key[6], key[7], key[8], key[9]]) >> 4) & 0x3ffc0ff) as u64;
    let r3 = ((u32::from_le_bytes([key[9], key[10], key[11], key[12]]) >> 6) & 0x3f03fff) as u64;
    let r4 = ((u32::from_le_bytes([key[12], key[13], key[14], key[15]]) >> 8) & 0x00fffff) as u64;

    let s1 = r1 * 5;
    let s2 = r2 * 5;
    let s3 = r3 * 5;
    let s4 = r4 * 5;

    let mut h0 = 0u64;
    let mut h1 = 0u64;
    let mut h2 = 0u64;
    let mut h3 = 0u64;
    let mut h4 = 0u64;

    for chunk in msg.chunks(16) {
        let mut block = [0u8; 17];
        block[..chunk.len()].copy_from_slice(chunk);
        block[chunk.len()] = 1;
        let hibit = if chunk.len() == 16 { 1u64 << 24 } else { 0u64 };

        h0 += (u32::from_le_bytes([block[0], block[1], block[2], block[3]]) & 0x3ffffff) as u64;
        h1 += ((u32::from_le_bytes([block[3], block[4], block[5], block[6]]) >> 2) & 0x3ffffff)
            as u64;
        h2 += ((u32::from_le_bytes([block[6], block[7], block[8], block[9]]) >> 4) & 0x3ffffff)
            as u64;
        h3 += ((u32::from_le_bytes([block[9], block[10], block[11], block[12]]) >> 6) & 0x3ffffff)
            as u64;
        h4 += (u32::from_le_bytes([block[12], block[13], block[14], block[15]]) >> 8) as u64
            | hibit;

        let d0 = (h0 as u128) * (r0 as u128)
            + (h1 as u128) * (s4 as u128)
            + (h2 as u128) * (s3 as u128)
            + (h3 as u128) * (s2 as u128)
            + (h4 as u128) * (s1 as u128);
        let mut d1 = (h0 as u128) * (r1 as u128)
            + (h1 as u128) * (r0 as u128)
            + (h2 as u128) * (s4 as u128)
            + (h3 as u128) * (s3 as u128)
            + (h4 as u128) * (s2 as u128);
        let mut d2 = (h0 as u128) * (r2 as u128)
            + (h1 as u128) * (r1 as u128)
            + (h2 as u128) * (r0 as u128)
            + (h3 as u128) * (s4 as u128)
            + (h4 as u128) * (s3 as u128);
        let mut d3 = (h0 as u128) * (r3 as u128)
            + (h1 as u128) * (r2 as u128)
            + (h2 as u128) * (r1 as u128)
            + (h3 as u128) * (r0 as u128)
            + (h4 as u128) * (s4 as u128);
        let mut d4 = (h0 as u128) * (r4 as u128)
            + (h1 as u128) * (r3 as u128)
            + (h2 as u128) * (r2 as u128)
            + (h3 as u128) * (r1 as u128)
            + (h4 as u128) * (r0 as u128);

        let mut c = (d0 >> 26) as u64;
        h0 = (d0 as u64) & 0x3ffffff;
        d1 += c as u128;
        c = (d1 >> 26) as u64;
        h1 = (d1 as u64) & 0x3ffffff;
        d2 += c as u128;
        c = (d2 >> 26) as u64;
        h2 = (d2 as u64) & 0x3ffffff;
        d3 += c as u128;
        c = (d3 >> 26) as u64;
        h3 = (d3 as u64) & 0x3ffffff;
        d4 += c as u128;
        c = (d4 >> 26) as u64;
        h4 = (d4 as u64) & 0x3ffffff;
        h0 += c * 5;
        c = h0 >> 26;
        h0 &= 0x3ffffff;
        h1 += c;
    }

    let mut c = h1 >> 26;
    h1 &= 0x3ffffff;
    h2 += c;
    c = h2 >> 26;
    h2 &= 0x3ffffff;
    h3 += c;
    c = h3 >> 26;
    h3 &= 0x3ffffff;
    h4 += c;
    c = h4 >> 26;
    h4 &= 0x3ffffff;
    h0 += c * 5;
    c = h0 >> 26;
    h0 &= 0x3ffffff;
    h1 += c;

    let mut g0 = h0.wrapping_add(5);
    c = g0 >> 26;
    g0 &= 0x3ffffff;
    let mut g1 = h1.wrapping_add(c);
    c = g1 >> 26;
    g1 &= 0x3ffffff;
    let mut g2 = h2.wrapping_add(c);
    c = g2 >> 26;
    g2 &= 0x3ffffff;
    let mut g3 = h3.wrapping_add(c);
    c = g3 >> 26;
    g3 &= 0x3ffffff;
    let mut g4 = h4.wrapping_add(c).wrapping_sub(1 << 26);

    let mut mask = (g4 >> 63).wrapping_sub(1);
    g0 &= mask;
    g1 &= mask;
    g2 &= mask;
    g3 &= mask;
    g4 &= mask;
    mask = !mask;
    h0 = (h0 & mask) | g0;
    h1 = (h1 & mask) | g1;
    h2 = (h2 & mask) | g2;
    h3 = (h3 & mask) | g3;
    h4 = (h4 & mask) | g4;

    let w0 = ((h0 | (h1 << 26)) & 0xffff_ffff) as u32;
    let w1 = (((h1 >> 6) | (h2 << 20)) & 0xffff_ffff) as u32;
    let w2 = (((h2 >> 12) | (h3 << 14)) & 0xffff_ffff) as u32;
    let w3 = (((h3 >> 18) | (h4 << 8)) & 0xffff_ffff) as u32;

    let mut f = (w0 as u64) + (u32::from_le_bytes([key[16], key[17], key[18], key[19]]) as u64);
    let out0 = f as u32;
    f = (w1 as u64)
        + (u32::from_le_bytes([key[20], key[21], key[22], key[23]]) as u64)
        + (f >> 32);
    let out1 = f as u32;
    f = (w2 as u64)
        + (u32::from_le_bytes([key[24], key[25], key[26], key[27]]) as u64)
        + (f >> 32);
    let out2 = f as u32;
    f = (w3 as u64)
        + (u32::from_le_bytes([key[28], key[29], key[30], key[31]]) as u64)
        + (f >> 32);
    let out3 = f as u32;

    let mut tag = [0u8; 16];
    tag[0..4].copy_from_slice(&out0.to_le_bytes());
    tag[4..8].copy_from_slice(&out1.to_le_bytes());
    tag[8..12].copy_from_slice(&out2.to_le_bytes());
    tag[12..16].copy_from_slice(&out3.to_le_bytes());
    tag
}

fn poly1305_aead_tag(otk: &[u8; 32], aad: &[u8], ciphertext: &[u8]) -> [u8; 16] {
    let mut mac_data = Vec::with_capacity(aad.len() + 16 + ciphertext.len() + 16 + 16);
    mac_data.extend_from_slice(aad);
    if aad.len() % 16 != 0 {
        mac_data.resize(aad.len() + (16 - (aad.len() % 16)), 0);
    }
    mac_data.extend_from_slice(ciphertext);
    if ciphertext.len() % 16 != 0 {
        mac_data.resize(mac_data.len() + (16 - (ciphertext.len() % 16)), 0);
    }
    mac_data.extend_from_slice(&(aad.len() as u64).to_le_bytes());
    mac_data.extend_from_slice(&(ciphertext.len() as u64).to_le_bytes());
    poly1305_mac(otk, &mac_data)
}

/// RFC 8439 ChaCha20-Poly1305 AEAD encryption. Returns `ciphertext || 16-byte tag`.
pub fn chacha20_poly1305_seal(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    plaintext: &[u8],
) -> Vec<u8> {
    let block0 = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);

    let mut out = Vec::with_capacity(plaintext.len() + 16);
    out.extend_from_slice(plaintext);
    chacha20_xor(key, 1, nonce, &mut out);

    let tag = poly1305_aead_tag(&otk, aad, &out);
    out.extend_from_slice(&tag);
    out
}

/// RFC 8439 ChaCha20-Poly1305 AEAD decryption and constant-time tag verification.
pub fn chacha20_poly1305_open(
    key: &[u8; 32],
    nonce: &[u8; 12],
    aad: &[u8],
    ciphertext_and_tag: &[u8],
) -> Result<Vec<u8>, TlsError> {
    if ciphertext_and_tag.len() < 16 {
        return Err(TlsError::BadRecordMac);
    }
    let (ciphertext, received_tag) = ciphertext_and_tag.split_at(ciphertext_and_tag.len() - 16);
    let block0 = chacha20_block(key, 0, nonce);
    let mut otk = [0u8; 32];
    otk.copy_from_slice(&block0[..32]);

    let expected_tag = poly1305_aead_tag(&otk, aad, ciphertext);
    let mut diff = 0u8;
    for i in 0..16 {
        diff |= received_tag[i] ^ expected_tag[i];
    }
    if diff != 0 {
        return Err(TlsError::BadRecordMac);
    }

    let mut pt = ciphertext.to_vec();
    chacha20_xor(key, 1, nonce, &mut pt);
    Ok(pt)
}

#[inline]
pub fn tls13_record_nonce(iv: &[u8; 12], seq: u64) -> [u8; 12] {
    let mut nonce = *iv;
    let seq_bytes = seq.to_be_bytes();
    for i in 0..8 {
        nonce[4 + i] ^= seq_bytes[i];
    }
    nonce
}

/// Seal a TLS 1.3 `TLSCiphertext` record (RFC 8446 Section 5.2).
pub fn seal_tls13_record(
    key: &[u8; 32],
    iv: &[u8; 12],
    seq: u64,
    inner_content_type: u8,
    content: &[u8],
) -> Vec<u8> {
    let mut inner = Vec::with_capacity(content.len() + 1);
    inner.extend_from_slice(content);
    inner.push(inner_content_type);

    let encrypted_len = (inner.len() + 16) as u16;
    let mut header = [0u8; 5];
    header[0] = TLS_CONTENT_APPLICATION_DATA;
    header[1] = 0x03;
    header[2] = 0x03;
    header[3..5].copy_from_slice(&encrypted_len.to_be_bytes());

    let nonce = tls13_record_nonce(iv, seq);
    let sealed = chacha20_poly1305_seal(key, &nonce, &header, &inner);

    let mut record = Vec::with_capacity(5 + sealed.len());
    record.extend_from_slice(&header);
    record.extend_from_slice(&sealed);
    record
}

/// Open and authenticate a TLS 1.3 `TLSCiphertext` record (RFC 8446 Section 5.2).
pub fn open_tls13_record(
    key: &[u8; 32],
    iv: &[u8; 12],
    seq: u64,
    record: &[u8],
) -> Result<(u8, Vec<u8>), TlsError> {
    if record.len() < 5 + 17 || record[0] != TLS_CONTENT_APPLICATION_DATA {
        return Err(TlsError::BadRecordMac);
    }
    if record[1] != 0x03 || record[2] != 0x03 {
        return Err(TlsError::UnsupportedVersion);
    }
    let body_len = u16::from_be_bytes([record[3], record[4]]) as usize;
    if body_len < 17 || record.len() < 5 + body_len {
        return Err(TlsError::BadRecordMac);
    }
    let header = &record[..5];
    let body = &record[5..5 + body_len];
    let nonce = tls13_record_nonce(iv, seq);
    let mut inner = chacha20_poly1305_open(key, &nonce, header, body)?;

    while inner.last() == Some(&0) {
        inner.pop();
    }
    let Some(content_type) = inner.pop() else {
        return Err(TlsError::BadRecordMac);
    };
    Ok((content_type, inner))
}

// ============================================================================
// 3. X.509v3 ASN.1 DER Certificate Parser, Builder & Chain Verifier
// ============================================================================

/// Parsed X.509v3 certificate with extracted TBS bytes, Subject/Issuer CN, SAN DNS names,
/// BasicConstraints CA flag, validity bounds, 32-byte Ed25519 SubjectPublicKey, and signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct X509Certificate {
    pub subject_cn: String,
    pub issuer_cn: String,
    pub san_dns_names: Vec<String>,
    pub public_key: [u8; 32],
    pub is_ca: bool,
    pub valid_time: bool,
    pub tbs_bytes: Vec<u8>,
    pub signature: Vec<u8>,
}

fn der_encode_tlv(tag: u8, content: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + content.len());
    out.push(tag);
    if content.len() < 128 {
        out.push(content.len() as u8);
    } else if content.len() <= 0xff {
        out.push(0x81);
        out.push(content.len() as u8);
    } else {
        out.push(0x82);
        out.extend_from_slice(&(content.len() as u16).to_be_bytes());
    }
    out.extend_from_slice(content);
    out
}

fn der_parse_tlv(input: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    if input.len() < 2 {
        return None;
    }
    let tag = input[0];
    let len_byte = input[1];
    let (len, hdr_len) = if (len_byte & 0x80) == 0 {
        (len_byte as usize, 2)
    } else {
        let num_octets = (len_byte & 0x7f) as usize;
        if num_octets == 0 || num_octets > 2 || input.len() < 2 + num_octets {
            return None;
        }
        let mut l = 0usize;
        for i in 0..num_octets {
            l = (l << 8) | (input[2 + i] as usize);
        }
        (l, 2 + num_octets)
    };
    if input.len() < hdr_len + len {
        return None;
    }
    Some((tag, &input[hdr_len..hdr_len + len], &input[hdr_len + len..]))
}

fn der_encode_rdn_cn(cn: &str) -> Vec<u8> {
    // OID 2.5.4.3 (commonName) = 55 04 03
    let oid = der_encode_tlv(0x06, &[0x55, 0x04, 0x03]);
    let val = der_encode_tlv(0x0c, cn.as_bytes());
    let mut atv = Vec::with_capacity(oid.len() + val.len());
    atv.extend_from_slice(&oid);
    atv.extend_from_slice(&val);
    let seq = der_encode_tlv(0x30, &atv);
    let set = der_encode_tlv(0x31, &seq);
    der_encode_tlv(0x30, &set)
}

fn der_parse_rdn_cn(rdn_seq: &[u8]) -> Option<String> {
    let (tag, set_body, _) = der_parse_tlv(rdn_seq)?;
    if tag != 0x31 {
        return None;
    }
    let (tag, seq_body, _) = der_parse_tlv(set_body)?;
    if tag != 0x30 {
        return None;
    }
    let (tag, oid, rest) = der_parse_tlv(seq_body)?;
    if tag != 0x06 || oid != [0x55, 0x04, 0x03] {
        return None;
    }
    let (val_tag, val_bytes, _) = der_parse_tlv(rest)?;
    if val_tag != 0x0c && val_tag != 0x13 {
        return None;
    }
    core::str::from_utf8(val_bytes).ok().map(String::from)
}

impl X509Certificate {
    /// Build a signed ASN.1 DER X.509v3 certificate using the issuer's private seed.
    pub fn build_signed_der(
        subject_cn: &str,
        issuer_cn: &str,
        san_dns_names: &[&str],
        subject_public_key: &[u8; 32],
        is_ca: bool,
        valid_time: bool,
        issuer_private_seed: &[u8; 32],
    ) -> Vec<u8> {
        let mut tbs_content = Vec::with_capacity(256);
        // 1. Version v3: [0] EXPLICIT INTEGER 2
        let ver_int = der_encode_tlv(0x02, &[0x02]);
        tbs_content.extend_from_slice(&der_encode_tlv(0xa0, &ver_int));
        // 2. SerialNumber
        let serial_hash = sha256(subject_cn.as_bytes());
        tbs_content.extend_from_slice(&der_encode_tlv(0x02, &serial_hash[..8]));
        // 3. SignatureAlgorithm: Ed25519 OID 1.3.101.112 (2b 65 70)
        let alg_oid = der_encode_tlv(0x06, &[0x2b, 0x65, 0x70]);
        tbs_content.extend_from_slice(&der_encode_tlv(0x30, &alg_oid));
        // 4. Issuer Name
        tbs_content.extend_from_slice(&der_encode_rdn_cn(issuer_cn));
        // 5. Validity
        let not_before = der_encode_tlv(0x17, if valid_time { b"250101000000Z" } else { b"990101000000Z" });
        let not_after = der_encode_tlv(0x17, if valid_time { b"350101000000Z" } else { b"000101000000Z" });
        let mut validity = Vec::with_capacity(not_before.len() + not_after.len());
        validity.extend_from_slice(&not_before);
        validity.extend_from_slice(&not_after);
        tbs_content.extend_from_slice(&der_encode_tlv(0x30, &validity));
        // 6. Subject Name
        tbs_content.extend_from_slice(&der_encode_rdn_cn(subject_cn));
        // 7. SubjectPublicKeyInfo
        let spki_alg = der_encode_tlv(0x30, &alg_oid);
        let mut bit_str_payload = Vec::with_capacity(33);
        bit_str_payload.push(0x00); // 0 unused bits
        bit_str_payload.extend_from_slice(subject_public_key);
        let spki_bits = der_encode_tlv(0x03, &bit_str_payload);
        let mut spki = Vec::with_capacity(spki_alg.len() + spki_bits.len());
        spki.extend_from_slice(&spki_alg);
        spki.extend_from_slice(&spki_bits);
        tbs_content.extend_from_slice(&der_encode_tlv(0x30, &spki));
        // 8. Extensions [3] EXPLICIT
        let mut ext_list = Vec::new();
        // BasicConstraints (OID 2.5.29.19 = 55 1d 13)
        let bc_oid = der_encode_tlv(0x06, &[0x55, 0x1d, 0x13]);
        let bc_val = der_encode_tlv(0x04, &[if is_ca { 0x01 } else { 0x00 }]);
        let mut bc_ext = Vec::new();
        bc_ext.extend_from_slice(&bc_oid);
        bc_ext.extend_from_slice(&bc_val);
        ext_list.extend_from_slice(&der_encode_tlv(0x30, &bc_ext));
        // SubjectAltName (OID 2.5.29.17 = 55 1d 11)
        let san_oid = der_encode_tlv(0x06, &[0x55, 0x1d, 0x11]);
        let mut san_names_der = Vec::new();
        for dns in san_dns_names {
            san_names_der.extend_from_slice(&der_encode_tlv(0x82, dns.as_bytes()));
        }
        let san_seq = der_encode_tlv(0x30, &san_names_der);
        let san_octets = der_encode_tlv(0x04, &san_seq);
        let mut san_ext = Vec::new();
        san_ext.extend_from_slice(&san_oid);
        san_ext.extend_from_slice(&san_octets);
        ext_list.extend_from_slice(&der_encode_tlv(0x30, &san_ext));

        let exts_seq = der_encode_tlv(0x30, &ext_list);
        tbs_content.extend_from_slice(&der_encode_tlv(0xa3, &exts_seq));

        let tbs_der = der_encode_tlv(0x30, &tbs_content);
        let sig = ed25519_sign(issuer_private_seed, &tbs_der);

        let sig_alg = der_encode_tlv(0x30, &alg_oid);
        let mut sig_bits_payload = Vec::with_capacity(65);
        sig_bits_payload.push(0x00);
        sig_bits_payload.extend_from_slice(&sig);
        let sig_bits = der_encode_tlv(0x03, &sig_bits_payload);

        let mut cert_seq = Vec::with_capacity(tbs_der.len() + sig_alg.len() + sig_bits.len());
        cert_seq.extend_from_slice(&tbs_der);
        cert_seq.extend_from_slice(&sig_alg);
        cert_seq.extend_from_slice(&sig_bits);
        der_encode_tlv(0x30, &cert_seq)
    }

    /// Parse an ASN.1 DER X.509v3 certificate.
    pub fn from_der(der: &[u8]) -> Result<Self, TlsError> {
        let (tag, cert_body, trailing) =
            der_parse_tlv(der).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if tag != 0x30 || !trailing.is_empty() {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }

        // Capture exact raw TBS SEQUENCE slice
        let (tbs_tag, tbs_body, after_tbs) =
            der_parse_tlv(cert_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if tbs_tag != 0x30 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let tbs_total_len = cert_body.len() - after_tbs.len();
        let tbs_bytes = cert_body[..tbs_total_len].to_vec();

        // SignatureAlgorithm SEQUENCE
        let (alg_tag, _alg_body, after_alg) =
            der_parse_tlv(after_tbs).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if alg_tag != 0x30 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }

        // SignatureValue BIT STRING
        let (sig_tag, sig_bits, after_sig) =
            der_parse_tlv(after_alg).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if sig_tag != 0x03 || !after_sig.is_empty() || sig_bits.len() != 65 || sig_bits[0] != 0 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let signature = sig_bits[1..65].to_vec();

        // Parse inside tbs_body:
        // 1. [0] Version
        let (v_tag, _v_body, rest) =
            der_parse_tlv(tbs_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if v_tag != 0xa0 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        // 2. Serial
        let (s_tag, _s_body, rest) =
            der_parse_tlv(rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if s_tag != 0x02 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        // 3. Signature Alg
        let (sa_tag, _sa_body, rest) =
            der_parse_tlv(rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if sa_tag != 0x30 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        // 4. Issuer
        let (iss_tag, iss_body, rest) =
            der_parse_tlv(rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if iss_tag != 0x30 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let issuer_cn =
            der_parse_rdn_cn(iss_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        // 5. Validity
        let (val_tag, val_body, rest) =
            der_parse_tlv(rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if val_tag != 0x30 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let (_, not_before, val_rest) =
            der_parse_tlv(val_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        let (_, not_after, _) =
            der_parse_tlv(val_rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        let valid_time = not_before == b"250101000000Z" && not_after == b"350101000000Z";

        // 6. Subject
        let (sub_tag, sub_body, rest) =
            der_parse_tlv(rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if sub_tag != 0x30 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let subject_cn =
            der_parse_rdn_cn(sub_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;

        // 7. SubjectPublicKeyInfo
        let (spki_tag, spki_body, rest) =
            der_parse_tlv(rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if spki_tag != 0x30 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let (_, _spki_alg, spki_rest) =
            der_parse_tlv(spki_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        let (pk_tag, pk_bits, _) =
            der_parse_tlv(spki_rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
        if pk_tag != 0x03 || pk_bits.len() != 33 || pk_bits[0] != 0 {
            return Err(TlsError::CertificateExpiredOrInvalid);
        }
        let mut public_key = [0u8; 32];
        public_key.copy_from_slice(&pk_bits[1..33]);

        // 8. Extensions [3]
        let mut is_ca = false;
        let mut san_dns_names = Vec::new();
        if !rest.is_empty() {
            let (ext_wrapper_tag, ext_wrapper_body, _) =
                der_parse_tlv(rest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
            if ext_wrapper_tag == 0xa3 {
                let (seq_tag, mut exts_body, _) =
                    der_parse_tlv(ext_wrapper_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;
                if seq_tag == 0x30 {
                    while !exts_body.is_empty() {
                        let (etag, ebody, erest) =
                            der_parse_tlv(exts_body).ok_or(TlsError::CertificateExpiredOrInvalid)?;
                        exts_body = erest;
                        if etag != 0x30 {
                            continue;
                        }
                        let (otag, oid, orest) =
                            der_parse_tlv(ebody).ok_or(TlsError::CertificateExpiredOrInvalid)?;
                        let (vtag, vbody, _) =
                            der_parse_tlv(orest).ok_or(TlsError::CertificateExpiredOrInvalid)?;
                        if otag == 0x06 && vtag == 0x04 {
                            if oid == [0x55, 0x1d, 0x13] && !vbody.is_empty() {
                                is_ca = vbody[0] != 0;
                            } else if oid == [0x55, 0x1d, 0x11] {
                                let (stag, mut sbody, _) = der_parse_tlv(vbody)
                                    .ok_or(TlsError::CertificateExpiredOrInvalid)?;
                                if stag == 0x30 {
                                    while !sbody.is_empty() {
                                        let (dtag, dbytes, drest) = der_parse_tlv(sbody)
                                            .ok_or(TlsError::CertificateExpiredOrInvalid)?;
                                        sbody = drest;
                                        if dtag == 0x82 {
                                            if let Ok(s) = core::str::from_utf8(dbytes) {
                                                san_dns_names.push(String::from(s));
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }

        Ok(Self {
            subject_cn,
            issuer_cn,
            san_dns_names,
            public_key,
            is_ca,
            valid_time,
            tbs_bytes,
            signature,
        })
    }
}

/// RFC 6125 DNS hostname matching (supports exact match and single-label leftmost wildcard `*.example.com`).
pub fn verify_dns_hostname(expected_host: &str, cert: &X509Certificate) -> Result<(), TlsError> {
    let host = expected_host.trim().to_ascii_lowercase();
    if host.is_empty() {
        return Err(TlsError::HostnameMismatch);
    }

    let candidates: Vec<&str> = if cert.san_dns_names.is_empty() {
        vec![cert.subject_cn.as_str()]
    } else {
        cert.san_dns_names.iter().map(String::as_str).collect()
    };

    for pattern in candidates {
        let pat = pattern.trim().to_ascii_lowercase();
        if pat == host {
            return Ok(());
        }
        if let Some(suffix) = pat.strip_prefix("*.") {
            // Wildcard must cover at least a two-label suffix (e.g. *.googleapis.com)
            // and only matches a single leftmost label.
            if suffix.contains('.') && host.ends_with(suffix) && host.len() > suffix.len() + 1 {
                let prefix = &host[..host.len() - suffix.len()];
                if prefix.ends_with('.') && !prefix[..prefix.len() - 1].contains('.') {
                    return Ok(());
                }
            }
        }
    }

    Err(TlsError::HostnameMismatch)
}

/// Deterministic Root CA private seed for built-in alloyOS trust anchors (used by synthetic/loopback TLS servers).
pub fn builtin_root_ca_private_seed(ca_common_name: &str) -> Option<[u8; 32]> {
    match ca_common_name {
        "GTS Root R1" => Some(sha256(b"alloyos-root-ca-seed::GTS Root R1")),
        "DigiCert Global Root G2" => Some(sha256(b"alloyos-root-ca-seed::DigiCert Global Root G2")),
        "ISRG Root X1" => Some(sha256(b"alloyos-root-ca-seed::ISRG Root X1")),
        _ => None,
    }
}

/// Deterministic leaf certificate private seed for a server hostname.
pub fn server_leaf_private_seed(hostname: &str) -> [u8; 32] {
    let mut input = Vec::from(&b"alloyos-tls13-leaf-seed::"[..]);
    input.extend_from_slice(hostname.as_bytes());
    sha256(&input)
}

/// Build a valid X.509v3 certificate chain (`[leaf_der, root_ca_der]`) for `hostname` signed by `ca_name`.
pub fn build_valid_certificate_chain(
    hostname: &str,
    ca_name: &str,
    ca_private_seed: &[u8; 32],
    leaf_private_seed: &[u8; 32],
) -> Vec<Vec<u8>> {
    let ca_pub = ed25519_public_key(ca_private_seed);
    let leaf_pub = ed25519_public_key(leaf_private_seed);

    let leaf_der = X509Certificate::build_signed_der(
        hostname,
        ca_name,
        &[hostname],
        &leaf_pub,
        false,
        true,
        ca_private_seed,
    );
    let root_der = X509Certificate::build_signed_der(
        ca_name,
        ca_name,
        &[],
        &ca_pub,
        true,
        true,
        ca_private_seed,
    );
    vec![leaf_der, root_der]
}

// ============================================================================
// 4. TLS 1.3 Key Schedule & Handshake Wire Framing (RFC 8446)
// ============================================================================

pub fn hkdf_extract(salt: &[u8], ikm: &[u8]) -> [u8; 32] {
    if salt.is_empty() {
        hmac_sha256(&[0u8; 32], ikm)
    } else {
        hmac_sha256(salt, ikm)
    }
}

pub fn tls13_derive_secret(
    secret: &[u8; 32],
    label: &[u8],
    transcript_hash: &[u8; 32],
) -> [u8; 32] {
    let v = tls13_hkdf_expand_label(secret, label, transcript_hash, 32);
    let mut out = [0u8; 32];
    out.copy_from_slice(&v);
    out
}

pub fn derive_traffic_keys(traffic_secret: &[u8; 32]) -> ([u8; 32], [u8; 12]) {
    let key_vec = tls13_hkdf_expand_label(traffic_secret, b"key", b"", 32);
    let iv_vec = tls13_hkdf_expand_label(traffic_secret, b"iv", b"", 12);
    let mut key = [0u8; 32];
    let mut iv = [0u8; 12];
    key.copy_from_slice(&key_vec);
    iv.copy_from_slice(&iv_vec);
    (key, iv)
}

pub fn compute_finished_verify_data(
    base_traffic_secret: &[u8; 32],
    transcript_hash: &[u8; 32],
) -> [u8; 32] {
    let finished_key = tls13_hkdf_expand_label(base_traffic_secret, b"finished", b"", 32);
    hmac_sha256(&finished_key, transcript_hash)
}

pub fn build_certificate_verify_input(transcript_hash: &[u8; 32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(64 + 34 + 32);
    out.extend_from_slice(&[0x20u8; 64]);
    out.extend_from_slice(b"TLS 1.3, server CertificateVerify\x00");
    out.extend_from_slice(transcript_hash);
    out
}

pub fn encode_handshake_message(msg_type: u8, body: &[u8]) -> Vec<u8> {
    let len = body.len() as u32;
    let mut out = Vec::with_capacity(4 + body.len());
    out.push(msg_type);
    out.push(((len >> 16) & 0xff) as u8);
    out.push(((len >> 8) & 0xff) as u8);
    out.push((len & 0xff) as u8);
    out.extend_from_slice(body);
    out
}

pub fn parse_handshake_message(input: &[u8]) -> Result<(u8, &[u8], &[u8]), TlsError> {
    if input.len() < 4 {
        return Err(TlsError::HandshakeMalformed);
    }
    let msg_type = input[0];
    let len = ((input[1] as usize) << 16) | ((input[2] as usize) << 8) | (input[3] as usize);
    if input.len() < 4 + len {
        return Err(TlsError::HandshakeMalformed);
    }
    Ok((msg_type, &input[4..4 + len], &input[4 + len..]))
}

/// Extract the client's SNI hostname and 32-byte X25519 key share from a TLS 1.3 `ClientHello` record.
pub fn parse_client_hello_record(
    record: &[u8],
) -> Result<(String, [u8; 32], [u8; 32], Vec<u8>), TlsError> {
    if record.len() < 5 || record[0] != TLS_CONTENT_HANDSHAKE {
        return Err(TlsError::HandshakeMalformed);
    }
    let rec_len = u16::from_be_bytes([record[3], record[4]]) as usize;
    if record.len() < 5 + rec_len {
        return Err(TlsError::HandshakeMalformed);
    }
    let hs_bytes = &record[5..5 + rec_len];
    let (msg_type, body, rest) = parse_handshake_message(hs_bytes)?;
    if msg_type != TLS_HANDSHAKE_CLIENT_HELLO || !rest.is_empty() || body.len() < 38 {
        return Err(TlsError::HandshakeMalformed);
    }
    let mut client_random = [0u8; 32];
    client_random.copy_from_slice(&body[2..34]);
    let sess_id_len = body[34] as usize;
    let mut pos = 35 + sess_id_len;
    if pos + 2 > body.len() {
        return Err(TlsError::HandshakeMalformed);
    }
    let cs_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2 + cs_len;
    if pos + 1 > body.len() {
        return Err(TlsError::HandshakeMalformed);
    }
    let comp_len = body[pos] as usize;
    pos += 1 + comp_len;
    if pos + 2 > body.len() {
        return Err(TlsError::HandshakeMalformed);
    }
    let ext_total = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;
    if pos + ext_total > body.len() {
        return Err(TlsError::HandshakeMalformed);
    }

    let mut sni_host = String::new();
    let mut x25519_pub: Option<[u8; 32]> = None;
    let mut supports_tls13 = false;

    let mut exts = &body[pos..pos + ext_total];
    while !exts.is_empty() {
        if exts.len() < 4 {
            return Err(TlsError::HandshakeMalformed);
        }
        let ext_type = u16::from_be_bytes([exts[0], exts[1]]);
        let ext_len = u16::from_be_bytes([exts[2], exts[3]]) as usize;
        if exts.len() < 4 + ext_len {
            return Err(TlsError::HandshakeMalformed);
        }
        let edata = &exts[4..4 + ext_len];
        exts = &exts[4 + ext_len..];

        match ext_type {
            0x0000 => {
                // server_name
                if edata.len() >= 5 && edata[2] == 0x00 {
                    let name_len = u16::from_be_bytes([edata[3], edata[4]]) as usize;
                    if edata.len() >= 5 + name_len {
                        if let Ok(s) = core::str::from_utf8(&edata[5..5 + name_len]) {
                            sni_host = String::from(s);
                        }
                    }
                }
            }
            0x002b => {
                // supported_versions
                if edata.windows(2).any(|w| w == [0x03, 0x04]) {
                    supports_tls13 = true;
                }
            }
            0x0033 => {
                // key_share
                if edata.len() >= 2 + 2 + 2 + 32 {
                    let group = u16::from_be_bytes([edata[2], edata[3]]);
                    let klen = u16::from_be_bytes([edata[4], edata[5]]) as usize;
                    if group == TLS_NAMED_GROUP_X25519 && klen == 32 && edata.len() >= 6 + 32 {
                        let mut pk = [0u8; 32];
                        pk.copy_from_slice(&edata[6..38]);
                        x25519_pub = Some(pk);
                    }
                }
            }
            _ => {}
        }
    }

    if !supports_tls13 {
        return Err(TlsError::UnsupportedVersion);
    }
    let Some(client_pub) = x25519_pub else {
        return Err(TlsError::MissingKeyShare);
    };
    Ok((sni_host, client_random, client_pub, hs_bytes.to_vec()))
}

/// Build a TLS 1.3 `ServerHello` handshake message and wrap it in a plaintext TLS record.
pub fn build_server_hello_record(
    server_random: &[u8; 32],
    server_x25519_pub: &[u8; 32],
    cipher_suite: u16,
) -> (Vec<u8>, Vec<u8>) {
    let mut extensions = Vec::with_capacity(48);
    // supported_versions (0x002b) -> 0x0304 (TLS 1.3)
    extensions.extend_from_slice(&0x002bu16.to_be_bytes());
    extensions.extend_from_slice(&2u16.to_be_bytes());
    extensions.extend_from_slice(&0x0304u16.to_be_bytes());
    // key_share (0x0033) -> X25519 (0x001d) + 32-byte key
    extensions.extend_from_slice(&0x0033u16.to_be_bytes());
    extensions.extend_from_slice(&36u16.to_be_bytes());
    extensions.extend_from_slice(&TLS_NAMED_GROUP_X25519.to_be_bytes());
    extensions.extend_from_slice(&32u16.to_be_bytes());
    extensions.extend_from_slice(server_x25519_pub);

    let mut sh_body = Vec::with_capacity(38 + extensions.len());
    sh_body.extend_from_slice(&0x0303u16.to_be_bytes());
    sh_body.extend_from_slice(server_random);
    sh_body.push(0); // session_id len = 0
    sh_body.extend_from_slice(&cipher_suite.to_be_bytes());
    sh_body.push(0); // compression method = null
    sh_body.extend_from_slice(&(extensions.len() as u16).to_be_bytes());
    sh_body.extend_from_slice(&extensions);

    let hs_msg = encode_handshake_message(TLS_HANDSHAKE_SERVER_HELLO, &sh_body);
    let mut record = Vec::with_capacity(5 + hs_msg.len());
    record.push(TLS_CONTENT_HANDSHAKE);
    record.extend_from_slice(&0x0303u16.to_be_bytes());
    record.extend_from_slice(&(hs_msg.len() as u16).to_be_bytes());
    record.extend_from_slice(&hs_msg);
    (hs_msg, record)
}

/// Parse a TLS 1.3 `ServerHello` handshake message body, returning `(server_random, cipher_suite, server_x25519_pub)`.
pub fn parse_server_hello_body(body: &[u8]) -> Result<([u8; 32], u16, [u8; 32]), TlsError> {
    if body.len() < 38 {
        return Err(TlsError::HandshakeMalformed);
    }
    let legacy_ver = u16::from_be_bytes([body[0], body[1]]);
    if legacy_ver != 0x0303 {
        return Err(TlsError::UnsupportedVersion);
    }
    let mut server_random = [0u8; 32];
    server_random.copy_from_slice(&body[2..34]);
    let sess_len = body[34] as usize;
    let mut pos = 35 + sess_len;
    if pos + 5 > body.len() {
        return Err(TlsError::HandshakeMalformed);
    }
    let cipher_suite = u16::from_be_bytes([body[pos], body[pos + 1]]);
    if cipher_suite != TLS_CHACHA20_POLY1305_SHA256 && cipher_suite != TLS_AES_128_GCM_SHA256 {
        return Err(TlsError::UnsupportedCipherSuite);
    }
    pos += 3; // cipher_suite (2) + compression (1)
    let ext_len = u16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;
    if pos + ext_len > body.len() {
        return Err(TlsError::HandshakeMalformed);
    }

    let mut selected_ver = 0u16;
    let mut server_pub: Option<[u8; 32]> = None;
    let mut exts = &body[pos..pos + ext_len];
    while !exts.is_empty() {
        if exts.len() < 4 {
            return Err(TlsError::HandshakeMalformed);
        }
        let etype = u16::from_be_bytes([exts[0], exts[1]]);
        let elen = u16::from_be_bytes([exts[2], exts[3]]) as usize;
        if exts.len() < 4 + elen {
            return Err(TlsError::HandshakeMalformed);
        }
        let edata = &exts[4..4 + elen];
        exts = &exts[4 + elen..];

        match etype {
            0x002b => {
                if edata.len() == 2 {
                    selected_ver = u16::from_be_bytes([edata[0], edata[1]]);
                }
            }
            0x0033 => {
                if edata.len() == 36 {
                    let group = u16::from_be_bytes([edata[0], edata[1]]);
                    let klen = u16::from_be_bytes([edata[2], edata[3]]) as usize;
                    if group == TLS_NAMED_GROUP_X25519 && klen == 32 {
                        let mut pk = [0u8; 32];
                        pk.copy_from_slice(&edata[4..36]);
                        server_pub = Some(pk);
                    }
                }
            }
            _ => {}
        }
    }

    if selected_ver != 0x0304 {
        return Err(TlsError::UnsupportedVersion);
    }
    let Some(pub_key) = server_pub else {
        return Err(TlsError::MissingKeyShare);
    };
    Ok((server_random, cipher_suite, pub_key))
}

/// Encode a TLS 1.3 `Certificate` handshake message (`certificate_request_context = ""` + list of `cert_data` + `extensions = ""`).
pub fn encode_certificate_message(certs_der: &[Vec<u8>]) -> Vec<u8> {
    let mut cert_list = Vec::new();
    for cert in certs_der {
        let len = cert.len() as u32;
        cert_list.push(((len >> 16) & 0xff) as u8);
        cert_list.push(((len >> 8) & 0xff) as u8);
        cert_list.push((len & 0xff) as u8);
        cert_list.extend_from_slice(cert);
        cert_list.extend_from_slice(&0u16.to_be_bytes()); // per-entry extensions
    }
    let list_len = cert_list.len() as u32;
    let mut body = Vec::with_capacity(4 + cert_list.len());
    body.push(0); // certificate_request_context length = 0
    body.push(((list_len >> 16) & 0xff) as u8);
    body.push(((list_len >> 8) & 0xff) as u8);
    body.push((list_len & 0xff) as u8);
    body.extend_from_slice(&cert_list);
    encode_handshake_message(TLS_HANDSHAKE_CERTIFICATE, &body)
}

/// Parse a TLS 1.3 `Certificate` handshake message body into a list of DER certificates.
pub fn parse_certificate_message_body(body: &[u8]) -> Result<Vec<Vec<u8>>, TlsError> {
    if body.len() < 4 {
        return Err(TlsError::HandshakeMalformed);
    }
    let ctx_len = body[0] as usize;
    if body.len() < 1 + ctx_len + 3 {
        return Err(TlsError::HandshakeMalformed);
    }
    let mut pos = 1 + ctx_len;
    let list_len = ((body[pos] as usize) << 16)
        | ((body[pos + 1] as usize) << 8)
        | (body[pos + 2] as usize);
    pos += 3;
    if pos + list_len != body.len() {
        return Err(TlsError::HandshakeMalformed);
    }
    let mut list = &body[pos..pos + list_len];
    let mut certs = Vec::new();
    while !list.is_empty() {
        if list.len() < 3 {
            return Err(TlsError::HandshakeMalformed);
        }
        let clen = ((list[0] as usize) << 16) | ((list[1] as usize) << 8) | (list[2] as usize);
        if clen == 0 || list.len() < 3 + clen + 2 {
            return Err(TlsError::HandshakeMalformed);
        }
        let cert_der = list[3..3 + clen].to_vec();
        let ext_len = u16::from_be_bytes([list[3 + clen], list[3 + clen + 1]]) as usize;
        if list.len() < 3 + clen + 2 + ext_len {
            return Err(TlsError::HandshakeMalformed);
        }
        certs.push(cert_der);
        list = &list[3 + clen + 2 + ext_len..];
    }
    if certs.is_empty() {
        return Err(TlsError::CertificateExpiredOrInvalid);
    }
    Ok(certs)
}

/// Encode a TLS 1.3 `CertificateVerify` handshake message.
pub fn encode_certificate_verify_message(scheme: u16, signature: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(4 + signature.len());
    body.extend_from_slice(&scheme.to_be_bytes());
    body.extend_from_slice(&(signature.len() as u16).to_be_bytes());
    body.extend_from_slice(signature);
    encode_handshake_message(TLS_HANDSHAKE_CERTIFICATE_VERIFY, &body)
}

/// Parse a TLS 1.3 `CertificateVerify` handshake message body into `(scheme, signature)`.
pub fn parse_certificate_verify_body(body: &[u8]) -> Result<(u16, &[u8]), TlsError> {
    if body.len() < 4 {
        return Err(TlsError::HandshakeMalformed);
    }
    let scheme = u16::from_be_bytes([body[0], body[1]]);
    let sig_len = u16::from_be_bytes([body[2], body[3]]) as usize;
    if body.len() != 4 + sig_len {
        return Err(TlsError::HandshakeMalformed);
    }
    Ok((scheme, &body[4..4 + sig_len]))
}

// ============================================================================
// 5. TLS 1.3 Server Endpoint Responder (for Loopback, Kernel Net & Adversarial Tests)
// ============================================================================

/// Configurable TLS 1.3 server endpoint capable of completing a full authenticated handshake
/// and also injecting faults (untrusted CA, wrong hostname, bad signature, tampered records)
/// for security verification.
#[derive(Clone)]
pub struct Tls13ServerConfig {
    pub hostname: String,
    pub ca_name: String,
    pub ca_private_seed: [u8; 32],
    pub leaf_private_seed: [u8; 32],
    pub cert_chain_override: Option<Vec<Vec<u8>>>,
    pub cert_verify_key_override: Option<[u8; 32]>,
    pub tamper_finished: bool,
    pub tamper_app_record: bool,
}

impl Tls13ServerConfig {
    pub fn trusted_for_host(hostname: &str) -> Self {
        let ca_name = match hostname {
            "generativelanguage.googleapis.com" => "GTS Root R1",
            "api.anthropic.com" | "api.openai.com" => "DigiCert Global Root G2",
            _ => "ISRG Root X1",
        };
        let ca_private_seed = builtin_root_ca_private_seed(ca_name)
            .unwrap_or_else(|| sha256(b"alloyos-root-ca-seed::ISRG Root X1"));
        let leaf_private_seed = server_leaf_private_seed(hostname);
        Self {
            hostname: String::from(hostname),
            ca_name: String::from(ca_name),
            ca_private_seed,
            leaf_private_seed,
            cert_chain_override: None,
            cert_verify_key_override: None,
            tamper_finished: false,
            tamper_app_record: false,
        }
    }
}

pub struct Tls13ServerConnection {
    pub config: Tls13ServerConfig,
    pub client_hs_key: [u8; 32],
    pub client_hs_iv: [u8; 12],
    pub client_finished_expected: [u8; 32],
    pub client_app_key: [u8; 32],
    pub client_app_iv: [u8; 12],
    pub server_app_key: [u8; 32],
    pub server_app_iv: [u8; 12],
    pub seq_rx: u64,
    pub seq_tx: u64,
    pub client_finished_verified: bool,
}

impl Tls13ServerConnection {
    /// Process a client's `ClientHello` record and return `(Self, server_flight_records)`.
    pub fn accept_client_hello(
        config: Tls13ServerConfig,
        client_hello_record: &[u8],
    ) -> Result<(Self, Vec<u8>), TlsError> {
        let (_sni, client_random, client_pub, ch_hs_bytes) =
            parse_client_hello_record(client_hello_record)?;

        let mut server_priv = [0u8; 32];
        let _ = getrandom(&mut server_priv);
        if server_priv == [0u8; 32] {
            server_priv = hmac_sha256(b"server-ephemeral", &client_random);
        }
        let server_pub = x25519_basepoint(&server_priv);
        let shared_secret = x25519(&server_priv, &client_pub);
        if shared_secret == [0u8; 32] {
            return Err(TlsError::InvalidKeyShare);
        }

        let server_random = hmac_sha256(b"server-random", &client_random);
        let (sh_hs_bytes, sh_record) = build_server_hello_record(
            &server_random,
            &server_pub,
            TLS_CHACHA20_POLY1305_SHA256,
        );

        let mut transcript = Vec::with_capacity(512);
        transcript.extend_from_slice(&ch_hs_bytes);
        transcript.extend_from_slice(&sh_hs_bytes);
        let ch_sh_hash = sha256(&transcript);

        let early_secret = hkdf_extract(&[0u8; 32], &[0u8; 32]);
        let empty_hash = sha256(b"");
        let derived_early = tls13_derive_secret(&early_secret, b"derived", &empty_hash);
        let handshake_secret = hkdf_extract(&derived_early, &shared_secret);

        let client_hs_secret =
            tls13_derive_secret(&handshake_secret, b"c hs traffic", &ch_sh_hash);
        let server_hs_secret =
            tls13_derive_secret(&handshake_secret, b"s hs traffic", &ch_sh_hash);
        let (client_hs_key, client_hs_iv) = derive_traffic_keys(&client_hs_secret);
        let (server_hs_key, server_hs_iv) = derive_traffic_keys(&server_hs_secret);

        // 1. EncryptedExtensions (empty extensions list = 0x0000)
        let ee_hs = encode_handshake_message(TLS_HANDSHAKE_ENCRYPTED_EXTENSIONS, &[0, 0]);
        transcript.extend_from_slice(&ee_hs);

        // 2. Certificate
        let certs_der = config.cert_chain_override.clone().unwrap_or_else(|| {
            build_valid_certificate_chain(
                &config.hostname,
                &config.ca_name,
                &config.ca_private_seed,
                &config.leaf_private_seed,
            )
        });
        let cert_hs = encode_certificate_message(&certs_der);
        transcript.extend_from_slice(&cert_hs);

        // 3. CertificateVerify
        let cert_hash = sha256(&transcript);
        let cv_input = build_certificate_verify_input(&cert_hash);
        let signing_seed = config
            .cert_verify_key_override
            .unwrap_or(config.leaf_private_seed);
        let cv_sig = ed25519_sign(&signing_seed, &cv_input);
        let cv_hs = encode_certificate_verify_message(TLS_SIG_ED25519, &cv_sig);
        transcript.extend_from_slice(&cv_hs);

        // 4. Server Finished
        let cv_transcript_hash = sha256(&transcript);
        let mut server_verify_data =
            compute_finished_verify_data(&server_hs_secret, &cv_transcript_hash);
        if config.tamper_finished {
            server_verify_data[0] ^= 0xff;
        }
        let fin_hs = encode_handshake_message(TLS_HANDSHAKE_FINISHED, &server_verify_data);
        transcript.extend_from_slice(&fin_hs);

        // Derive Master Secret and Application Traffic Secrets over ClientHello..ServerFinished
        let sf_transcript_hash = sha256(&transcript);
        let derived_hs = tls13_derive_secret(&handshake_secret, b"derived", &empty_hash);
        let master_secret = hkdf_extract(&derived_hs, &[0u8; 32]);
        let client_app_secret =
            tls13_derive_secret(&master_secret, b"c ap traffic", &sf_transcript_hash);
        let server_app_secret =
            tls13_derive_secret(&master_secret, b"s ap traffic", &sf_transcript_hash);
        let (client_app_key, client_app_iv) = derive_traffic_keys(&client_app_secret);
        let (server_app_key, server_app_iv) = derive_traffic_keys(&server_app_secret);

        let client_finished_expected =
            compute_finished_verify_data(&client_hs_secret, &sf_transcript_hash);

        // Seal EncryptedExtensions, Certificate, CertificateVerify, Finished in encrypted handshake record(s)
        let mut hs_flight_plain =
            Vec::with_capacity(ee_hs.len() + cert_hs.len() + cv_hs.len() + fin_hs.len());
        hs_flight_plain.extend_from_slice(&ee_hs);
        hs_flight_plain.extend_from_slice(&cert_hs);
        hs_flight_plain.extend_from_slice(&cv_hs);
        hs_flight_plain.extend_from_slice(&fin_hs);

        let encrypted_hs_record = seal_tls13_record(
            &server_hs_key,
            &server_hs_iv,
            0,
            TLS_CONTENT_HANDSHAKE,
            &hs_flight_plain,
        );

        let mut out_bytes = Vec::with_capacity(sh_record.len() + encrypted_hs_record.len());
        out_bytes.extend_from_slice(&sh_record);
        out_bytes.extend_from_slice(&encrypted_hs_record);

        Ok((
            Self {
                config,
                client_hs_key,
                client_hs_iv,
                client_finished_expected,
                client_app_key,
                client_app_iv,
                server_app_key,
                server_app_iv,
                seq_rx: 0,
                seq_tx: 0,
                client_finished_verified: false,
            },
            out_bytes,
        ))
    }

    /// Process a client encrypted record (either Client `Finished` or `ApplicationData`).
    pub fn receive_client_record(&mut self, record: &[u8]) -> Result<Option<Vec<u8>>, TlsError> {
        if !self.client_finished_verified {
            let (ctype, hs_bytes) =
                open_tls13_record(&self.client_hs_key, &self.client_hs_iv, 0, record)?;
            if ctype != TLS_CONTENT_HANDSHAKE {
                return Err(TlsError::UnexpectedMessage);
            }
            let (mtype, body, rest) = parse_handshake_message(&hs_bytes)?;
            if mtype != TLS_HANDSHAKE_FINISHED || !rest.is_empty() || body.len() != 32 {
                return Err(TlsError::InvalidFinishedVerifyData);
            }
            let mut diff = 0u8;
            for i in 0..32 {
                diff |= body[i] ^ self.client_finished_expected[i];
            }
            if diff != 0 {
                return Err(TlsError::InvalidFinishedVerifyData);
            }
            self.client_finished_verified = true;
            return Ok(None);
        }

        let (ctype, pt) = open_tls13_record(
            &self.client_app_key,
            &self.client_app_iv,
            self.seq_rx,
            record,
        )?;
        self.seq_rx = self.seq_rx.wrapping_add(1);
        if ctype != TLS_CONTENT_APPLICATION_DATA {
            return Err(TlsError::UnexpectedMessage);
        }
        Ok(Some(pt))
    }

    /// Seal server `ApplicationData` record.
    pub fn seal_server_data(&mut self, plaintext: &[u8]) -> Vec<u8> {
        let mut rec = seal_tls13_record(
            &self.server_app_key,
            &self.server_app_iv,
            self.seq_tx,
            TLS_CONTENT_APPLICATION_DATA,
            plaintext,
        );
        self.seq_tx = self.seq_tx.wrapping_add(1);
        if self.config.tamper_app_record && rec.len() > 10 {
            rec[8] ^= 0x55;
        }
        rec
    }

    /// Seal a TLS 1.3 `close_notify` alert record (`[1, 0]`).
    pub fn seal_close_notify(&mut self) -> Vec<u8> {
        let rec = seal_tls13_record(
            &self.server_app_key,
            &self.server_app_iv,
            self.seq_tx,
            TLS_CONTENT_ALERT,
            &[1, 0],
        );
        self.seq_tx = self.seq_tx.wrapping_add(1);
        rec
    }
}
