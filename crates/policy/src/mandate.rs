//! Native Rust Agent Mandate Protocol v1 (`am/1`) verifier, caveat lattice,
//! and per-path spend tracker.
//!
//! Wire-compatible with `convexityos/agent-mandate` (`application/agent-mandate+jwt`,
//! Ed25519 `did:key` multicodec `0xed01`, RFC 7638 JWK thumbprints, exact 9-decimal
//! scaled monetary arithmetic, and fail-closed caveat attenuation).

use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::sync::Mutex;

pub const PROTOCOL_VERSION: &str = "am/1";
pub const MANDATE_TYP: &str = "application/agent-mandate+jwt";
pub const MAX_DEPTH: u32 = 4;
pub const DEFAULT_SKEW_SECONDS: u64 = 60;
pub const MONEY_SCALE: u32 = 9;
const SCALE_FACTOR: u128 = 1_000_000_000;

const REGISTERED_TOP_CLAIMS: &[&str] = &[
    "v", "typ", "jti", "iss", "sub", "par", "iat", "nbf", "exp", "cnf", "ledger", "cav", "st",
];

const REGISTERED_CAVEATS: &[&str] = &[
    "max_total",
    "max_per_txn",
    "rate",
    "aud",
    "resources",
    "purpose",
    "task",
    "payer",
    "depth",
    "delegable",
    "currency",
    "settle",
    "step_up",
    "effect",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MandateErrorCode {
    ChainInvalid,
    UnknownCaveat,
    AttenuationViolation,
    DepthExceeded,
    MandateExpired,
    ResourceNotCovered,
    AudienceMismatch,
    PurposeNotCovered,
    EffectNotPermitted,
    CurrencyMismatch,
    AmountExceedsCap,
    BudgetExhausted,
    StepUpRequired,
}

impl MandateErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::ChainInvalid => "chain_invalid",
            Self::UnknownCaveat => "unknown_caveat",
            Self::AttenuationViolation => "attenuation_violation",
            Self::DepthExceeded => "depth_exceeded",
            Self::MandateExpired => "mandate_expired",
            Self::ResourceNotCovered => "resource_not_covered",
            Self::AudienceMismatch => "audience_mismatch",
            Self::PurposeNotCovered => "purpose_not_covered",
            Self::EffectNotPermitted => "effect_not_permitted",
            Self::CurrencyMismatch => "currency_mismatch",
            Self::AmountExceedsCap => "amount_exceeds_cap",
            Self::BudgetExhausted => "budget_exhausted",
            Self::StepUpRequired => "step_up_required",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MandateError {
    pub code: MandateErrorCode,
    pub detail: String,
}

impl MandateError {
    pub fn new(code: MandateErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

impl fmt::Display for MandateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code.as_str(), self.detail)
    }
}

impl std::error::Error for MandateError {}

// ------------------------------------------------------------ Money (exact 9-decimal u128)

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Money {
    pub cur: String,
    pub amt: String,
}

impl Money {
    pub fn new(cur: impl Into<String>, amt: impl Into<String>) -> Self {
        Self {
            cur: cur.into(),
            amt: amt.into(),
        }
    }

    pub fn to_scaled(&self) -> Result<u128, MandateError> {
        if self.cur.len() < 3
            || self.cur.len() > 8
            || !self.cur.bytes().all(|b| b.is_ascii_uppercase())
        {
            return Err(MandateError::new(
                MandateErrorCode::ChainInvalid,
                format!("invalid currency code: {}", self.cur),
            ));
        }
        parse_scaled_decimal(&self.amt).ok_or_else(|| {
            MandateError::new(
                MandateErrorCode::ChainInvalid,
                format!("invalid monetary amount: {}", self.amt),
            )
        })
    }

    pub fn from_scaled(scaled: u128, cur: impl Into<String>) -> Self {
        let whole = scaled / SCALE_FACTOR;
        let frac = scaled % SCALE_FACTOR;
        let amt = if frac == 0 {
            whole.to_string()
        } else {
            let s = format!("{:09}", frac);
            let trimmed = s.trim_end_matches('0');
            format!("{}.{}", whole, trimmed)
        };
        Self {
            cur: cur.into(),
            amt,
        }
    }
}

fn parse_scaled_decimal(s: &str) -> Option<u128> {
    if s.is_empty() {
        return None;
    }
    let (whole_s, frac_s) = match s.split_once('.') {
        Some((w, f)) => (w, Some(f)),
        None => (s, None),
    };
    if whole_s.is_empty() || !whole_s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if whole_s.len() > 1 && whole_s.starts_with('0') {
        return None;
    }
    let whole: u128 = whole_s.parse().ok()?;
    let frac_scaled: u128 = match frac_s {
        None => 0,
        Some(f) => {
            if f.is_empty() || f.len() > MONEY_SCALE as usize || !f.bytes().all(|b| b.is_ascii_digit())
            {
                return None;
            }
            let mut padded = f.to_string();
            while padded.len() < MONEY_SCALE as usize {
                padded.push('0');
            }
            padded.parse().ok()?
        }
    };
    whole.checked_mul(SCALE_FACTOR)?.checked_add(frac_scaled)
}

fn money_leq(child: &Money, parent: &Money) -> Result<(), MandateError> {
    if child.cur != parent.cur {
        return Err(MandateError::new(
            MandateErrorCode::CurrencyMismatch,
            format!("cannot compare {} against {}", child.cur, parent.cur),
        ));
    }
    if child.to_scaled()? <= parent.to_scaled()? {
        Ok(())
    } else {
        Err(MandateError::new(
            MandateErrorCode::AttenuationViolation,
            format!("{} exceeds {} {}", child.amt, parent.amt, parent.cur),
        ))
    }
}

fn money_min(a: &Money, b: &Money) -> Result<Money, MandateError> {
    if a.cur != b.cur {
        return Err(MandateError::new(
            MandateErrorCode::CurrencyMismatch,
            format!("cannot meet {} and {}", a.cur, b.cur),
        ));
    }
    if a.to_scaled()? <= b.to_scaled()? {
        Ok(a.clone())
    } else {
        Ok(b.clone())
    }
}

// ------------------------------------------------------------ Caveats & Claims

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Effect {
    Irreversible,
    Write,
    Read,
}

impl Effect {
    /// Ordered most-authority first (0 = broadest, 2 = narrowest).
    pub fn rank(self) -> u8 {
        match self {
            Self::Irreversible => 0,
            Self::Write => 1,
            Self::Read => 2,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Irreversible => "irreversible",
            Self::Write => "write",
            Self::Read => "read",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettleMode {
    Immediate,
    EscrowSchema,
    EscrowHuman,
}

impl SettleMode {
    fn rank(self) -> u8 {
        match self {
            Self::Immediate => 0,
            Self::EscrowSchema => 1,
            Self::EscrowHuman => 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateCaveat {
    pub n: u32,
    pub per: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepUpCaveat {
    pub over: Money,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateCaveats {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_total: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_per_txn: Option<Money>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<RateCaveat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aud: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resources: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub purpose: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub task: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payer: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub depth: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub currency: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settle: Option<SettleMode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step_up: Option<StepUpCaveat>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effect: Option<Effect>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CnfClaim {
    pub jkt: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateClaims {
    pub v: String,
    pub typ: String,
    pub jti: String,
    pub iss: String,
    pub sub: String,
    pub par: Option<String>,
    pub iat: u64,
    pub nbf: u64,
    pub exp: u64,
    pub cnf: CnfClaim,
    pub ledger: String,
    pub cav: MandateCaveats,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub st: Option<u64>,
}

// ------------------------------------------------------------ Base64url, Base58btc, did:key

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

pub fn base64url_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() * 4).div_ceil(3));
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(B64_TABLE[((n >> 18) & 63) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 63) as usize] as char);
        out.push(B64_TABLE[((n >> 6) & 63) as usize] as char);
        out.push(B64_TABLE[(n & 63) as usize] as char);
        i += 3;
    }
    let rem = data.len() - i;
    if rem == 1 {
        let n = (data[i] as u32) << 16;
        out.push(B64_TABLE[((n >> 18) & 63) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 63) as usize] as char);
    } else if rem == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(B64_TABLE[((n >> 18) & 63) as usize] as char);
        out.push(B64_TABLE[((n >> 12) & 63) as usize] as char);
        out.push(B64_TABLE[((n >> 6) & 63) as usize] as char);
    }
    out
}

pub fn base64url_decode(s: &str) -> Option<Vec<u8>> {
    if s.len() % 4 == 1 {
        return None;
    }
    let mut vals = Vec::with_capacity(s.len());
    for b in s.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'-' => 62,
            b'_' => 63,
            _ => return None,
        };
        vals.push(v);
    }
    let mut out = Vec::with_capacity((vals.len() * 3) / 4);
    let mut i = 0;
    while i + 4 <= vals.len() {
        let n = ((vals[i] as u32) << 18)
            | ((vals[i + 1] as u32) << 12)
            | ((vals[i + 2] as u32) << 6)
            | (vals[i + 3] as u32);
        out.push(((n >> 16) & 0xff) as u8);
        out.push(((n >> 8) & 0xff) as u8);
        out.push((n & 0xff) as u8);
        i += 4;
    }
    let rem = vals.len() - i;
    if rem == 2 {
        let n = ((vals[i] as u32) << 18) | ((vals[i + 1] as u32) << 12);
        out.push(((n >> 16) & 0xff) as u8);
    } else if rem == 3 {
        let n = ((vals[i] as u32) << 18) | ((vals[i + 1] as u32) << 12) | ((vals[i + 2] as u32) << 6);
        out.push(((n >> 16) & 0xff) as u8);
        out.push(((n >> 8) & 0xff) as u8);
    }
    Some(out)
}

const B58_ALPHABET: &[u8; 58] = b"123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

fn base58_encode(input: &[u8]) -> String {
    let mut digits = vec![0u8];
    for &byte in input {
        let mut carry = byte as u32;
        for d in &mut digits {
            carry += (*d as u32) << 8;
            *d = (carry % 58) as u8;
            carry /= 58;
        }
        while carry > 0 {
            digits.push((carry % 58) as u8);
            carry /= 58;
        }
    }
    let mut out = String::new();
    for &b in input {
        if b != 0 {
            break;
        }
        out.push('1');
    }
    for &d in digits.iter().rev() {
        out.push(B58_ALPHABET[d as usize] as char);
    }
    out
}

fn base58_decode(s: &str) -> Option<Vec<u8>> {
    if s.is_empty() {
        return None;
    }
    let mut bytes = vec![0u8];
    for b in s.bytes() {
        let val = B58_ALPHABET.iter().position(|&c| c == b)? as u32;
        let mut carry = val;
        for out_b in &mut bytes {
            carry += (*out_b as u32) * 58;
            *out_b = (carry & 0xff) as u8;
            carry >>= 8;
        }
        while carry > 0 {
            bytes.push((carry & 0xff) as u8);
            carry >>= 8;
        }
    }
    let leading_zeros = s.bytes().take_while(|&b| b == b'1').count();
    let mut out = vec![0u8; leading_zeros];
    while bytes.last() == Some(&0) && bytes.len() > 1 {
        bytes.pop();
    }
    if !(bytes.len() == 1 && bytes[0] == 0 && leading_zeros > 0) {
        for &b in bytes.iter().rev() {
            out.push(b);
        }
    }
    Some(out)
}

/// Encode an Ed25519 verifying key as `did:key:z<base58btc(0xed01 || pubkey)>`.
pub fn did_key_from_verifying_key(vk: &VerifyingKey) -> String {
    let mut multi = Vec::with_capacity(34);
    multi.push(0xed);
    multi.push(0x01);
    multi.extend_from_slice(vk.as_bytes());
    format!("did:key:z{}", base58_encode(&multi))
}

/// Decode `did:key:z...` (with optional `#z...` fragment) into an Ed25519 verifying key.
pub fn verifying_key_from_did_key(did: &str) -> Option<VerifyingKey> {
    let (bare, frag) = match did.split_once('#') {
        Some((b, f)) => (b, Some(f)),
        None => (did, None),
    };
    let rest = bare.strip_prefix("did:key:z")?;
    if let Some(f) = frag {
        if f != bare.strip_prefix("did:key:")? {
            return None;
        }
    }
    let decoded = base58_decode(rest)?;
    if decoded.len() != 34 || decoded[0] != 0xed || decoded[1] != 0x01 {
        return None;
    }
    let key_bytes: [u8; 32] = decoded[2..].try_into().ok()?;
    VerifyingKey::from_bytes(&key_bytes).ok()
}

/// RFC 7638 JWK thumbprint (`cnf.jkt`) for an Ed25519 public key.
pub fn jwk_thumbprint_from_verifying_key(vk: &VerifyingKey) -> String {
    let x = base64url_encode(vk.as_bytes());
    let canonical = format!(r#"{{"crv":"Ed25519","kty":"OKP","x":"{}"}}"#, x);
    let digest = Sha256::digest(canonical.as_bytes());
    base64url_encode(&digest)
}

// ------------------------------------------------------------ Signing & JWS Parsing

pub fn sign_mandate_jws(claims: &MandateClaims, signer: &SigningKey, kid: Option<&str>) -> String {
    let did = did_key_from_verifying_key(&signer.verifying_key());
    let default_kid = format!("{}#{}", did, did.strip_prefix("did:key:").unwrap_or(""));
    let header = serde_json::json!({
        "alg": "EdDSA",
        "typ": MANDATE_TYP,
        "kid": kid.unwrap_or(&default_kid),
    });
    let h_seg = base64url_encode(serde_json::to_string(&header).unwrap().as_bytes());
    let p_seg = base64url_encode(serde_json::to_string(claims).unwrap().as_bytes());
    let signing_input = format!("{}.{}", h_seg, p_seg);
    let sig = signer.sign(signing_input.as_bytes());
    format!("{}.{}", signing_input, base64url_encode(&sig.to_bytes()))
}

struct ParsedMandate {
    claims: MandateClaims,
    signing_input: String,
    signature: [u8; 64],
}

fn parse_mandate_jws(token: &str) -> Result<ParsedMandate, MandateError> {
    let mut parts = token.split('.');
    let h_s = parts.next();
    let p_s = parts.next();
    let s_s = parts.next();
    if parts.next().is_some() {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            "JWS must have exactly three segments",
        ));
    }
    let (h_s, p_s, s_s) = match (h_s, p_s, s_s) {
        (Some(h), Some(p), Some(s)) if !h.is_empty() && !p.is_empty() && !s.is_empty() => (h, p, s),
        _ => {
            return Err(MandateError::new(
                MandateErrorCode::ChainInvalid,
                "malformed JWS compact serialization",
            ));
        }
    };

    let h_bytes = base64url_decode(h_s).ok_or_else(|| {
        MandateError::new(MandateErrorCode::ChainInvalid, "invalid base64url header")
    })?;
    let p_bytes = base64url_decode(p_s).ok_or_else(|| {
        MandateError::new(MandateErrorCode::ChainInvalid, "invalid base64url payload")
    })?;
    let sig_bytes = base64url_decode(s_s).ok_or_else(|| {
        MandateError::new(MandateErrorCode::ChainInvalid, "invalid base64url signature")
    })?;
    let signature: [u8; 64] = sig_bytes.try_into().map_err(|_| {
        MandateError::new(MandateErrorCode::ChainInvalid, "Ed25519 signature must be 64 bytes")
    })?;

    let header: serde_json::Value = serde_json::from_slice(&h_bytes).map_err(|_| {
        MandateError::new(MandateErrorCode::ChainInvalid, "header is not valid JSON")
    })?;
    let h_obj = header.as_object().ok_or_else(|| {
        MandateError::new(MandateErrorCode::ChainInvalid, "header must be a JSON object")
    })?;
    if h_obj.get("alg").and_then(|v| v.as_str()) != Some("EdDSA") {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            "unsupported JWS alg; only EdDSA is permitted",
        ));
    }
    if h_obj.get("typ").and_then(|v| v.as_str()) != Some(MANDATE_TYP) {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            format!("unexpected JWS typ; expected {}", MANDATE_TYP),
        ));
    }

    let raw_payload: serde_json::Value = serde_json::from_slice(&p_bytes).map_err(|_| {
        MandateError::new(MandateErrorCode::ChainInvalid, "payload is not valid JSON")
    })?;
    let p_obj = raw_payload.as_object().ok_or_else(|| {
        MandateError::new(MandateErrorCode::ChainInvalid, "payload must be a JSON object")
    })?;

    // Reject unregistered top-level claims fail-closed (§5.1).
    for key in p_obj.keys() {
        if !REGISTERED_TOP_CLAIMS.contains(&key.as_str()) {
            return Err(MandateError::new(
                MandateErrorCode::ChainInvalid,
                format!("unregistered top-level claim: {}", key),
            ));
        }
    }

    // Reject unregistered caveat keys fail-closed (§6.3).
    let cav_obj = p_obj
        .get("cav")
        .and_then(|v| v.as_object())
        .ok_or_else(|| MandateError::new(MandateErrorCode::ChainInvalid, "missing cav object"))?;
    for key in cav_obj.keys() {
        if !REGISTERED_CAVEATS.contains(&key.as_str()) {
            return Err(MandateError::new(
                MandateErrorCode::UnknownCaveat,
                format!("unregistered caveat key: {}", key),
            ));
        }
    }

    let claims: MandateClaims = serde_json::from_value(raw_payload).map_err(|e| {
        MandateError::new(MandateErrorCode::ChainInvalid, format!("invalid mandate claims: {}", e))
    })?;

    if claims.v != PROTOCOL_VERSION {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            format!("unsupported mandate version {}", claims.v),
        ));
    }
    if claims.typ != "mandate" {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            format!("unexpected typ {}", claims.typ),
        ));
    }
    if claims.jti.is_empty() || claims.iss.is_empty() || claims.sub.is_empty() || claims.ledger.is_empty() {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            "jti, iss, sub, and ledger must be non-empty",
        ));
    }
    if claims.nbf > claims.exp {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            "nbf must not exceed exp",
        ));
    }

    validate_caveats(&claims.cav)?;

    Ok(ParsedMandate {
        claims,
        signing_input: format!("{}.{}", h_s, p_s),
        signature,
    })
}

fn validate_caveats(cav: &MandateCaveats) -> Result<(), MandateError> {
    if let Some(ref m) = cav.max_total {
        m.to_scaled()?;
    }
    if let Some(ref m) = cav.max_per_txn {
        m.to_scaled()?;
    }
    if let Some(ref s) = cav.step_up {
        s.over.to_scaled()?;
    }
    if let Some(ref cur) = cav.currency {
        if cur.len() < 3 || cur.len() > 8 || !cur.bytes().all(|b| b.is_ascii_uppercase()) {
            return Err(MandateError::new(
                MandateErrorCode::ChainInvalid,
                format!("invalid currency caveat: {}", cur),
            ));
        }
    }
    if let Some(ref list) = cav.aud {
        if list.is_empty() || list.iter().any(|s| s.is_empty()) {
            return Err(MandateError::new(MandateErrorCode::ChainInvalid, "aud caveat must be non-empty"));
        }
    }
    if let Some(ref list) = cav.resources {
        if list.is_empty() || list.iter().any(|s| s.is_empty()) {
            return Err(MandateError::new(MandateErrorCode::ChainInvalid, "resources caveat must be non-empty"));
        }
    }
    if let Some(ref list) = cav.purpose {
        if list.is_empty() || list.iter().any(|s| s.is_empty()) {
            return Err(MandateError::new(MandateErrorCode::ChainInvalid, "purpose caveat must be non-empty"));
        }
    }
    if let Some(ref r) = cav.rate {
        if r.n == 0 || r.per.is_empty() {
            return Err(MandateError::new(MandateErrorCode::ChainInvalid, "invalid rate caveat"));
        }
    }
    Ok(())
}

// ------------------------------------------------------------ URI & Host Pattern Matching (§6.4)

fn host_of_uri(uri: &str) -> Option<String> {
    let rest = uri.split_once("://")?.1;
    let authority = rest.split(&['/', '?', '#'][..]).next()?;
    if authority.is_empty() || authority.contains('@') {
        return None;
    }
    Some(authority.to_ascii_lowercase())
}

fn host_pattern_matches(pattern: &str, host: &str) -> bool {
    let p = pattern.to_ascii_lowercase();
    let h = host.to_ascii_lowercase();
    if let Some(suffix) = p.strip_prefix("*.") {
        if h.len() > suffix.len() + 1 && h.ends_with(suffix) {
            let prefix = &h[..h.len() - suffix.len()];
            return prefix.ends_with('.') && !prefix[..prefix.len() - 1].contains('.');
        }
        false
    } else {
        p == h
    }
}

fn host_pattern_contains(parent: &str, child: &str) -> bool {
    let p = parent.to_ascii_lowercase();
    let c = child.to_ascii_lowercase();
    if p == c {
        return true;
    }
    if let Some(suffix) = p.strip_prefix("*.") {
        if c.starts_with("*.") {
            return false;
        }
        return host_pattern_matches(&p, &c) && c.ends_with(suffix);
    }
    false
}

fn uri_pattern_matches(pattern: &str, uri: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        uri.starts_with(prefix)
    } else {
        pattern == uri
    }
}

fn uri_pattern_contains(parent: &str, child: &str) -> bool {
    if parent == child {
        return true;
    }
    if let Some(p_pref) = parent.strip_suffix('*') {
        let c_base = child.strip_suffix('*').unwrap_or(child);
        return c_base.starts_with(p_pref);
    }
    false
}

// ------------------------------------------------------------ Attenuation Lattice (§6-§7)

fn check_narrows(child: &MandateCaveats, parent: &MandateCaveats) -> Result<(), MandateError> {
    if let (Some(c), Some(p)) = (&child.max_total, &parent.max_total) {
        money_leq(c, p)?;
    }
    if let (Some(c), Some(p)) = (&child.max_per_txn, &parent.max_per_txn) {
        money_leq(c, p)?;
    }
    if let (Some(c), Some(p)) = (&child.currency, &parent.currency) {
        if c != p {
            return Err(MandateError::new(
                MandateErrorCode::CurrencyMismatch,
                format!("currency {} differs from parent {}", c, p),
            ));
        }
    }
    if let (Some(c), Some(p)) = (&child.task, &parent.task) {
        if c != p {
            return Err(MandateError::new(
                MandateErrorCode::AttenuationViolation,
                format!("task {} differs from parent {}", c, p),
            ));
        }
    }
    if let (Some(c), Some(p)) = (&child.payer, &parent.payer) {
        if c != p {
            return Err(MandateError::new(
                MandateErrorCode::AttenuationViolation,
                format!("payer {} differs from parent {}", c, p),
            ));
        }
    }
    if let (Some(c), Some(p)) = (child.depth, parent.depth) {
        if c > p {
            return Err(MandateError::new(
                MandateErrorCode::DepthExceeded,
                format!("depth {} exceeds parent {}", c, p),
            ));
        }
    }
    if let (Some(c), Some(p)) = (child.delegable, parent.delegable) {
        if c && !p {
            return Err(MandateError::new(
                MandateErrorCode::AttenuationViolation,
                "cannot restore delegable on a sealed mandate",
            ));
        }
    }
    if let (Some(c), Some(p)) = (child.effect, parent.effect) {
        if c.rank() < p.rank() {
            return Err(MandateError::new(
                MandateErrorCode::AttenuationViolation,
                format!("effect {} is broader than parent {}", c.as_str(), p.as_str()),
            ));
        }
    }
    if let (Some(c), Some(p)) = (child.settle, parent.settle) {
        if c.rank() < p.rank() {
            return Err(MandateError::new(
                MandateErrorCode::AttenuationViolation,
                "settle mode is looser than parent",
            ));
        }
    }
    if let (Some(c), Some(p)) = (&child.step_up, &parent.step_up) {
        // Lower step_up threshold grants LESS freedom (narrower).
        money_leq(&c.over, &p.over)?;
    }
    if let (Some(c_list), Some(p_list)) = (&child.aud, &parent.aud) {
        for ch in c_list {
            if !p_list.iter().any(|ph| host_pattern_contains(ph, ch)) {
                return Err(MandateError::new(
                    MandateErrorCode::AttenuationViolation,
                    format!("aud {} not covered by parent", ch),
                ));
            }
        }
    }
    if let (Some(c_list), Some(p_list)) = (&child.resources, &parent.resources) {
        for cr in c_list {
            if !p_list.iter().any(|pr| uri_pattern_contains(pr, cr)) {
                return Err(MandateError::new(
                    MandateErrorCode::AttenuationViolation,
                    format!("resource {} not covered by parent", cr),
                ));
            }
        }
    }
    if let (Some(c_list), Some(p_list)) = (&child.purpose, &parent.purpose) {
        let pset: HashSet<&str> = p_list.iter().map(String::as_str).collect();
        for cp in c_list {
            if !pset.contains(cp.as_str()) {
                return Err(MandateError::new(
                    MandateErrorCode::AttenuationViolation,
                    format!("purpose {} not granted by parent", cp),
                ));
            }
        }
    }
    Ok(())
}

fn meet_caveats(a: &MandateCaveats, b: &MandateCaveats) -> Result<MandateCaveats, MandateError> {
    let max_total = match (&a.max_total, &b.max_total) {
        (Some(x), Some(y)) => Some(money_min(x, y)?),
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    let max_per_txn = match (&a.max_per_txn, &b.max_per_txn) {
        (Some(x), Some(y)) => Some(money_min(x, y)?),
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    let step_up = match (&a.step_up, &b.step_up) {
        (Some(x), Some(y)) => Some(StepUpCaveat {
            over: money_min(&x.over, &y.over)?,
        }),
        (Some(x), None) | (None, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    let currency = match (&a.currency, &b.currency) {
        (Some(x), Some(y)) if x != y => {
            return Err(MandateError::new(
                MandateErrorCode::CurrencyMismatch,
                format!("currency conflict {} vs {}", x, y),
            ));
        }
        (Some(x), _) | (_, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    let task = match (&a.task, &b.task) {
        (Some(x), Some(y)) if x != y => {
            return Err(MandateError::new(
                MandateErrorCode::AttenuationViolation,
                format!("task conflict {} vs {}", x, y),
            ));
        }
        (Some(x), _) | (_, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    let payer = match (&a.payer, &b.payer) {
        (Some(x), Some(y)) if x != y => {
            return Err(MandateError::new(
                MandateErrorCode::AttenuationViolation,
                format!("payer conflict {} vs {}", x, y),
            ));
        }
        (Some(x), _) | (_, Some(x)) => Some(x.clone()),
        (None, None) => None,
    };
    let depth = match (a.depth, b.depth) {
        (Some(x), Some(y)) => Some(x.min(y)),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    let delegable = match (a.delegable, b.delegable) {
        (Some(x), Some(y)) => Some(x && y),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    let effect = match (a.effect, b.effect) {
        (Some(x), Some(y)) => Some(if x.rank() >= y.rank() { x } else { y }),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    let settle = match (a.settle, b.settle) {
        (Some(x), Some(y)) => Some(if x.rank() >= y.rank() { x } else { y }),
        (Some(x), None) | (None, Some(x)) => Some(x),
        (None, None) => None,
    };
    let aud = match (&a.aud, &b.aud) {
        (Some(xs), Some(ys)) => {
            let mut kept: Vec<String> = xs
                .iter()
                .filter(|x| ys.iter().any(|y| host_pattern_contains(y, x)))
                .chain(ys.iter().filter(|y| xs.iter().any(|x| host_pattern_contains(x, y))))
                .cloned()
                .collect();
            kept.sort();
            kept.dedup();
            Some(kept)
        }
        (Some(xs), None) | (None, Some(xs)) => Some(xs.clone()),
        (None, None) => None,
    };
    let resources = match (&a.resources, &b.resources) {
        (Some(xs), Some(ys)) => {
            let mut kept: Vec<String> = xs
                .iter()
                .filter(|x| ys.iter().any(|y| uri_pattern_contains(y, x)))
                .chain(ys.iter().filter(|y| xs.iter().any(|x| uri_pattern_contains(x, y))))
                .cloned()
                .collect();
            kept.sort();
            kept.dedup();
            Some(kept)
        }
        (Some(xs), None) | (None, Some(xs)) => Some(xs.clone()),
        (None, None) => None,
    };
    let purpose = match (&a.purpose, &b.purpose) {
        (Some(xs), Some(ys)) => {
            let yset: HashSet<&str> = ys.iter().map(String::as_str).collect();
            let mut kept: Vec<String> = xs
                .iter()
                .filter(|x| yset.contains(x.as_str()))
                .cloned()
                .collect();
            kept.sort();
            kept.dedup();
            Some(kept)
        }
        (Some(xs), None) | (None, Some(xs)) => Some(xs.clone()),
        (None, None) => None,
    };

    Ok(MandateCaveats {
        max_total,
        max_per_txn,
        rate: b.rate.clone().or_else(|| a.rate.clone()),
        aud,
        resources,
        purpose,
        task,
        payer,
        depth,
        delegable,
        currency,
        settle,
        step_up,
        effect,
    })
}

// ------------------------------------------------------------ Chain Verification (§7)

#[derive(Debug, Clone)]
pub struct MandateActionRequest {
    pub resource: String,
    pub amount: Money,
    /// Absent means `Effect::Irreversible` (broadest blast radius, fail-closed).
    pub effect: Option<Effect>,
    pub purpose: Option<String>,
    pub task_id: Option<String>,
}

pub struct VerifyMandateOptions {
    pub trusted_root_issuers: HashSet<String>,
    pub ledger: String,
    pub now: u64,
    pub skew_seconds: u64,
    pub request: Option<MandateActionRequest>,
    /// Set true when an explicit step-up approval covers this action.
    pub step_up_approved: bool,
}

#[derive(Debug, Clone)]
pub struct VerifiedMandateChain {
    pub chain_hash: String,
    pub root_jti: String,
    pub path: Vec<String>,
    pub principal: String,
    pub agent: String,
    pub ledger: String,
    pub effective: MandateCaveats,
    pub nbf: u64,
    pub exp: u64,
    /// Per-node `max_total` budgets along `path` for spend tracking.
    pub node_budgets: Vec<(String, Option<Money>)>,
}

pub fn chain_hash(tokens: &[String]) -> String {
    let joined = tokens.join("\n");
    format!("sha256:{}", hex::encode(Sha256::digest(joined.as_bytes())))
}

pub fn verify_mandate_chain(
    tokens: &[String],
    opts: &VerifyMandateOptions,
) -> Result<VerifiedMandateChain, MandateError> {
    if tokens.is_empty() {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            "mandate chain is empty",
        ));
    }
    if tokens.len() - 1 > MAX_DEPTH as usize {
        return Err(MandateError::new(
            MandateErrorCode::DepthExceeded,
            format!("chain depth {} exceeds protocol maximum {}", tokens.len() - 1, MAX_DEPTH),
        ));
    }

    let mut parsed = Vec::with_capacity(tokens.len());
    for tok in tokens {
        parsed.push(parse_mandate_jws(tok)?);
    }

    let root = &parsed[0].claims;
    if root.par.is_some() {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            "root mandate must have par = null",
        ));
    }
    if !opts.trusted_root_issuers.contains(&root.iss) {
        return Err(MandateError::new(
            MandateErrorCode::ChainInvalid,
            format!("untrusted root issuer: {}", root.iss),
        ));
    }

    let mut effective = MandateCaveats::default();
    let mut eff_nbf = 0u64;
    let mut eff_exp = u64::MAX;
    let mut path = Vec::with_capacity(parsed.len());
    let mut node_budgets = Vec::with_capacity(parsed.len());
    let mut prev_depth: Option<u32> = None;

    for (idx, link) in parsed.iter().enumerate() {
        let c = &link.claims;
        if c.ledger != opts.ledger {
            return Err(MandateError::new(
                MandateErrorCode::ChainInvalid,
                format!("link {} ledger {} does not match serving ledger {}", idx, c.ledger, opts.ledger),
            ));
        }

        // Verify signature against link.iss (for root) and enforce prev.sub == cur.iss for children.
        if idx > 0 {
            let parent = &parsed[idx - 1].claims;
            if c.par.as_deref() != Some(parent.jti.as_str()) {
                return Err(MandateError::new(
                    MandateErrorCode::ChainInvalid,
                    format!("link {} par does not match parent jti", idx),
                ));
            }
            if c.iss != parent.sub {
                return Err(MandateError::new(
                    MandateErrorCode::ChainInvalid,
                    format!("link {} iss {} does not match parent sub {}", idx, c.iss, parent.sub),
                ));
            }
            if effective.delegable == Some(false) {
                return Err(MandateError::new(
                    MandateErrorCode::AttenuationViolation,
                    "cannot delegate past a mandate with delegable = false",
                ));
            }
            if let Some(pd) = prev_depth {
                if pd == 0 {
                    return Err(MandateError::new(
                        MandateErrorCode::DepthExceeded,
                        "delegation depth exhausted at parent",
                    ));
                }
                if let Some(cd) = c.cav.depth {
                    if cd > pd - 1 {
                        return Err(MandateError::new(
                            MandateErrorCode::DepthExceeded,
                            format!("child depth {} exceeds parent depth - 1 ({})", cd, pd - 1),
                        ));
                    }
                }
            }
            if c.nbf < eff_nbf || c.exp > eff_exp {
                return Err(MandateError::new(
                    MandateErrorCode::AttenuationViolation,
                    "child validity window widens parent window",
                ));
            }
        }

        let vk = verifying_key_from_did_key(&c.iss).ok_or_else(|| {
            MandateError::new(
                MandateErrorCode::ChainInvalid,
                format!("link {} iss is not a valid Ed25519 did:key: {}", idx, c.iss),
            )
        })?;
        let sig = Signature::from_bytes(&link.signature);
        if vk.verify_strict(link.signing_input.as_bytes(), &sig).is_err() {
            return Err(MandateError::new(
                MandateErrorCode::ChainInvalid,
                format!("link {} Ed25519 signature verification failed", idx),
            ));
        }

        // Compare against the accumulated meet (prevents grandchild escape past an omitted caveat).
        if idx > 0 {
            check_narrows(&c.cav, &effective)?;
        }
        effective = meet_caveats(&effective, &c.cav)?;

        // Track mandatory depth decrement structurally.
        prev_depth = match (prev_depth, c.cav.depth) {
            (Some(pd), Some(cd)) => Some(cd.min(pd.saturating_sub(1))),
            (Some(pd), None) => Some(pd.saturating_sub(1)),
            (None, Some(cd)) => Some(cd),
            (None, None) => None,
        };

        eff_nbf = eff_nbf.max(c.nbf);
        eff_exp = eff_exp.min(c.exp);
        path.push(c.jti.clone());
        node_budgets.push((c.jti.clone(), c.cav.max_total.clone()));
    }

    // Time window check with skew tolerance.
    if opts.now.saturating_add(opts.skew_seconds) < eff_nbf
        || opts.now > eff_exp.saturating_add(opts.skew_seconds)
    {
        return Err(MandateError::new(
            MandateErrorCode::MandateExpired,
            format!("now {} outside effective window [{}, {}]", opts.now, eff_nbf, eff_exp),
        ));
    }

    // Evaluate action request against effective caveats if supplied.
    if let Some(ref req) = opts.request {
        evaluate_request(&effective, req, opts.step_up_approved)?;
    }

    let leaf = &parsed[parsed.len() - 1].claims;
    Ok(VerifiedMandateChain {
        chain_hash: chain_hash(tokens),
        root_jti: root.jti.clone(),
        path,
        principal: root.iss.clone(),
        agent: leaf.sub.clone(),
        ledger: root.ledger.clone(),
        effective,
        nbf: eff_nbf,
        exp: eff_exp,
        node_budgets,
    })
}

fn evaluate_request(
    eff: &MandateCaveats,
    req: &MandateActionRequest,
    step_up_approved: bool,
) -> Result<(), MandateError> {
    let req_scaled = req.amount.to_scaled()?;

    if let Some(ref cur) = eff.currency {
        if &req.amount.cur != cur {
            return Err(MandateError::new(
                MandateErrorCode::CurrencyMismatch,
                format!("request currency {} does not match mandate currency {}", req.amount.cur, cur),
            ));
        }
    }
    if let Some(ref cap) = eff.max_per_txn {
        if req.amount.cur != cap.cur {
            return Err(MandateError::new(
                MandateErrorCode::CurrencyMismatch,
                format!("request currency {} differs from max_per_txn {}", req.amount.cur, cap.cur),
            ));
        }
        if req_scaled > cap.to_scaled()? {
            return Err(MandateError::new(
                MandateErrorCode::AmountExceedsCap,
                format!("{} {} exceeds max_per_txn {} {}", req.amount.amt, req.amount.cur, cap.amt, cap.cur),
            ));
        }
    }
    if let Some(ref total) = eff.max_total {
        if req.amount.cur != total.cur {
            return Err(MandateError::new(
                MandateErrorCode::CurrencyMismatch,
                format!("request currency {} differs from max_total {}", req.amount.cur, total.cur),
            ));
        }
        if req_scaled > total.to_scaled()? {
            return Err(MandateError::new(
                MandateErrorCode::BudgetExhausted,
                format!("{} {} exceeds max_total {} {}", req.amount.amt, req.amount.cur, total.amt, total.cur),
            ));
        }
    }
    if let Some(ref aud_list) = eff.aud {
        let host = host_of_uri(&req.resource).ok_or_else(|| {
            MandateError::new(
                MandateErrorCode::AudienceMismatch,
                format!("cannot extract host from resource {}", req.resource),
            )
        })?;
        if !aud_list.iter().any(|p| host_pattern_matches(p, &host)) {
            return Err(MandateError::new(
                MandateErrorCode::AudienceMismatch,
                format!("host {} outside aud allowlist", host),
            ));
        }
    }
    if let Some(ref res_list) = eff.resources {
        if !res_list.iter().any(|p| uri_pattern_matches(p, &req.resource)) {
            return Err(MandateError::new(
                MandateErrorCode::ResourceNotCovered,
                format!("resource {} outside grant", req.resource),
            ));
        }
    }
    if let Some(ref purp_list) = eff.purpose {
        match req.purpose.as_deref() {
            Some(p) if purp_list.iter().any(|allowed| allowed == p) => {}
            _ => {
                return Err(MandateError::new(
                    MandateErrorCode::PurposeNotCovered,
                    "request purpose outside mandate grant",
                ));
            }
        }
    }
    if let Some(ref task) = eff.task {
        if req.task_id.as_deref() != Some(task.as_str()) {
            return Err(MandateError::new(
                MandateErrorCode::PurposeNotCovered,
                format!("request task_id does not match pinned task {}", task),
            ));
        }
    }
    if let Some(allowed_effect) = eff.effect {
        let actual = req.effect.unwrap_or(Effect::Irreversible);
        if actual.rank() < allowed_effect.rank() {
            return Err(MandateError::new(
                MandateErrorCode::EffectNotPermitted,
                format!("effect {} exceeds permitted {}", actual.as_str(), allowed_effect.as_str()),
            ));
        }
    }
    if let Some(ref su) = eff.step_up {
        if req.amount.cur != su.over.cur {
            return Err(MandateError::new(
                MandateErrorCode::CurrencyMismatch,
                format!("request currency {} differs from step_up {}", req.amount.cur, su.over.cur),
            ));
        }
        if req_scaled > su.over.to_scaled()? && !step_up_approved {
            return Err(MandateError::new(
                MandateErrorCode::StepUpRequired,
                format!("amount {} {} exceeds step_up threshold {} {}", req.amount.amt, req.amount.cur, su.over.amt, su.over.cur),
            ));
        }
    }
    Ok(())
}

// ------------------------------------------------------------ Per-Path Spend Tracker & Provenance

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateSpendRecord {
    pub spend_id: String,
    pub chain_hash: String,
    pub path: Vec<String>,
    pub resource: String,
    pub amount: Money,
    pub effect: Effect,
    pub refunded: bool,
    pub timestamp: u64,
}

impl MandateSpendRecord {
    pub fn digest(&self) -> String {
        let json = serde_json::to_string(self).unwrap_or_default();
        format!("sha256:{}", hex::encode(Sha256::digest(json.as_bytes())))
    }
}

/// Cryptographic mandate provenance attached to a signed `VitnaRunReceiptV1`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MandateProvenanceRecord {
    pub chain_hash: String,
    pub root_jti: String,
    pub path: Vec<String>,
    pub ledger: String,
    pub currency: String,
    pub total_spent: String,
    pub spend_digests: Vec<String>,
}

struct TrackerState {
    next_id: u64,
    /// Per mandate `jti`: (currency, spent_scaled).
    node_spent: HashMap<String, (String, u128)>,
    records: BTreeMap<String, MandateSpendRecord>,
}

/// Thread-safe in-memory spend tracker enforcing per-ancestor `max_total` budgets
/// across an autonomous agent session's mandated tool calls.
pub struct MandateSpendTracker {
    state: Mutex<TrackerState>,
}

impl Default for MandateSpendTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl MandateSpendTracker {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(TrackerState {
                next_id: 1,
                node_spent: HashMap::new(),
                records: BTreeMap::new(),
            }),
        }
    }

    /// Atomically checks every ancestor's remaining budget along `verified.path`
    /// and debits `amount` from all of them.
    pub fn reserve(
        &self,
        verified: &VerifiedMandateChain,
        resource: &str,
        amount: &Money,
        effect: Effect,
        timestamp: u64,
    ) -> Result<MandateSpendRecord, MandateError> {
        let debit = amount.to_scaled()?;
        let mut st = self.state.lock().expect("spend tracker mutex poisoned");

        // Phase 1: check every ancestor node before mutating any counter.
        for (jti, budget_opt) in &verified.node_budgets {
            if let Some((existing_cur, _)) = st.node_spent.get(jti) {
                if existing_cur != &amount.cur {
                    return Err(MandateError::new(
                        MandateErrorCode::CurrencyMismatch,
                        format!("mandate {} tracks {} but spend is {}", jti, existing_cur, amount.cur),
                    ));
                }
            }
            if let Some(budget) = budget_opt {
                if budget.cur != amount.cur {
                    return Err(MandateError::new(
                        MandateErrorCode::CurrencyMismatch,
                        format!("mandate {} budget is {} but spend is {}", jti, budget.cur, amount.cur),
                    ));
                }
                let already = st.node_spent.get(jti).map(|(_, s)| *s).unwrap_or(0);
                let cap = budget.to_scaled()?;
                if already.saturating_add(debit) > cap {
                    return Err(MandateError::new(
                        MandateErrorCode::BudgetExhausted,
                        format!("mandate {} budget exhausted", jti),
                    ));
                }
            }
        }

        // Phase 2: debit every node in path.
        for (jti, _) in &verified.node_budgets {
            let entry = st
                .node_spent
                .entry(jti.clone())
                .or_insert_with(|| (amount.cur.clone(), 0));
            entry.1 = entry.1.saturating_add(debit);
        }

        let spend_id = format!("spn_{:06}", st.next_id);
        st.next_id += 1;
        let rec = MandateSpendRecord {
            spend_id: spend_id.clone(),
            chain_hash: verified.chain_hash.clone(),
            path: verified.path.clone(),
            resource: resource.to_string(),
            amount: amount.clone(),
            effect,
            refunded: false,
            timestamp,
        };
        st.records.insert(spend_id, rec.clone());
        Ok(rec)
    }

    /// Refunds a spend ONLY if its effect is `Effect::Read`.
    /// Write and irreversible calls stay debited even on failure to prevent
    /// failed-call-refund-retry budget drain.
    pub fn refund_if_reversible(&self, spend_id: &str) -> bool {
        let mut st = self.state.lock().expect("spend tracker mutex poisoned");
        let (path, cur, debit) = match st.records.get_mut(spend_id) {
            Some(rec) if !rec.refunded && rec.effect == Effect::Read => {
                rec.refunded = true;
                let debit = rec.amount.to_scaled().unwrap_or(0);
                (rec.path.clone(), rec.amount.cur.clone(), debit)
            }
            _ => return false,
        };
        for jti in path {
            if let Some((c, spent)) = st.node_spent.get_mut(&jti) {
                if *c == cur {
                    *spent = spent.saturating_sub(debit);
                }
            }
        }
        true
    }

    /// Remaining scaled spend on `jti` given its declared `max_total`.
    pub fn spent_on(&self, jti: &str) -> Option<Money> {
        let st = self.state.lock().expect("spend tracker mutex poisoned");
        st.node_spent
            .get(jti)
            .map(|(cur, scaled)| Money::from_scaled(*scaled, cur.clone()))
    }

    /// Export cryptographic provenance for all non-refunded spends under `verified`.
    pub fn provenance_for(&self, verified: &VerifiedMandateChain, currency: &str) -> MandateProvenanceRecord {
        let st = self.state.lock().expect("spend tracker mutex poisoned");
        let mut total_scaled = 0u128;
        let mut digests = Vec::new();
        for rec in st.records.values() {
            if rec.chain_hash == verified.chain_hash && !rec.refunded && rec.amount.cur == currency {
                total_scaled = total_scaled.saturating_add(rec.amount.to_scaled().unwrap_or(0));
                digests.push(rec.digest());
            }
        }
        let total = Money::from_scaled(total_scaled, currency);
        MandateProvenanceRecord {
            chain_hash: verified.chain_hash.clone(),
            root_jti: verified.root_jti.clone(),
            path: verified.path.clone(),
            ledger: verified.ledger.clone(),
            currency: total.cur,
            total_spent: total.amt,
            spend_digests: digests,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generate_policy_signing_key;

    fn make_claims(
        iss: &str,
        sub: &str,
        sub_vk: &VerifyingKey,
        par: Option<&str>,
        jti: &str,
        cav: MandateCaveats,
    ) -> MandateClaims {
        MandateClaims {
            v: PROTOCOL_VERSION.to_string(),
            typ: "mandate".to_string(),
            jti: jti.to_string(),
            iss: iss.to_string(),
            sub: sub.to_string(),
            par: par.map(str::to_string),
            iat: 1000,
            nbf: 1000,
            exp: 5000,
            cnf: CnfClaim {
                jkt: jwk_thumbprint_from_verifying_key(sub_vk),
            },
            ledger: "https://ledger.example".to_string(),
            cav,
            st: None,
        }
    }

    #[test]
    fn did_key_roundtrip_and_chain_verification() {
        let principal = generate_policy_signing_key();
        let agent = generate_policy_signing_key();
        let subagent = generate_policy_signing_key();

        let p_did = did_key_from_verifying_key(&principal.verifying_key());
        let a_did = did_key_from_verifying_key(&agent.verifying_key());
        let s_did = did_key_from_verifying_key(&subagent.verifying_key());

        assert_eq!(
            verifying_key_from_did_key(&p_did).unwrap().as_bytes(),
            principal.verifying_key().as_bytes()
        );

        let root_claims = make_claims(
            &p_did,
            &a_did,
            &agent.verifying_key(),
            None,
            "mnd_root",
            MandateCaveats {
                currency: Some("USD".to_string()),
                max_total: Some(Money::new("USD", "10.00")),
                max_per_txn: Some(Money::new("USD", "2.00")),
                aud: Some(vec!["convexityos.com".to_string()]),
                effect: Some(Effect::Read),
                depth: Some(2),
                delegable: Some(true),
                ..Default::default()
            },
        );
        let child_claims = make_claims(
            &a_did,
            &s_did,
            &subagent.verifying_key(),
            Some("mnd_root"),
            "mnd_child",
            MandateCaveats {
                max_total: Some(Money::new("USD", "3.00")),
                max_per_txn: Some(Money::new("USD", "1.00")),
                resources: Some(vec!["https://convexityos.com/api/v1/*".to_string()]),
                ..Default::default()
            },
        );

        let root_tok = sign_mandate_jws(&root_claims, &principal, None);
        let child_tok = sign_mandate_jws(&child_claims, &agent, None);

        let mut trusted = HashSet::new();
        trusted.insert(p_did.clone());

        let verified = verify_mandate_chain(
            &[root_tok.clone(), child_tok.clone()],
            &VerifyMandateOptions {
                trusted_root_issuers: trusted.clone(),
                ledger: "https://ledger.example".to_string(),
                now: 2000,
                skew_seconds: DEFAULT_SKEW_SECONDS,
                request: Some(MandateActionRequest {
                    resource: "https://convexityos.com/api/v1/quote/NVDA".to_string(),
                    amount: Money::new("USD", "0.25"),
                    effect: Some(Effect::Read),
                    purpose: None,
                    task_id: None,
                }),
                step_up_approved: false,
            },
        )
        .expect("valid chain must verify");

        assert_eq!(verified.path, vec!["mnd_root", "mnd_child"]);
        assert_eq!(verified.effective.effect, Some(Effect::Read));

        // Write action under a Read-only mandate fails closed.
        let write_err = verify_mandate_chain(
            &[root_tok.clone(), child_tok.clone()],
            &VerifyMandateOptions {
                trusted_root_issuers: trusted.clone(),
                ledger: "https://ledger.example".to_string(),
                now: 2000,
                skew_seconds: DEFAULT_SKEW_SECONDS,
                request: Some(MandateActionRequest {
                    resource: "https://convexityos.com/api/v1/webhooks".to_string(),
                    amount: Money::new("USD", "0.10"),
                    effect: Some(Effect::Write),
                    purpose: None,
                    task_id: None,
                }),
                step_up_approved: false,
            },
        )
        .unwrap_err();
        assert_eq!(write_err.code, MandateErrorCode::EffectNotPermitted);

        // Grandchild cannot escape root's Read caveat by widening to Write.
        let grandchild = generate_policy_signing_key();
        let g_did = did_key_from_verifying_key(&grandchild.verifying_key());
        let bad_gc_claims = make_claims(
            &s_did,
            &g_did,
            &grandchild.verifying_key(),
            Some("mnd_child"),
            "mnd_gc",
            MandateCaveats {
                effect: Some(Effect::Write),
                ..Default::default()
            },
        );
        let bad_gc_tok = sign_mandate_jws(&bad_gc_claims, &subagent, None);
        let widen_err = verify_mandate_chain(
            &[root_tok, child_tok, bad_gc_tok],
            &VerifyMandateOptions {
                trusted_root_issuers: trusted,
                ledger: "https://ledger.example".to_string(),
                now: 2000,
                skew_seconds: DEFAULT_SKEW_SECONDS,
                request: None,
                step_up_approved: false,
            },
        )
        .unwrap_err();
        assert_eq!(widen_err.code, MandateErrorCode::AttenuationViolation);
    }

    #[test]
    fn spend_tracker_enforces_budgets_and_refunds_only_read_failures() {
        let principal = generate_policy_signing_key();
        let agent = generate_policy_signing_key();
        let p_did = did_key_from_verifying_key(&principal.verifying_key());
        let a_did = did_key_from_verifying_key(&agent.verifying_key());

        let claims = make_claims(
            &p_did,
            &a_did,
            &agent.verifying_key(),
            None,
            "mnd_budget",
            MandateCaveats {
                currency: Some("USD".to_string()),
                max_total: Some(Money::new("USD", "1.00")),
                ..Default::default()
            },
        );
        let tok = sign_mandate_jws(&claims, &principal, None);
        let mut trusted = HashSet::new();
        trusted.insert(p_did);
        let verified = verify_mandate_chain(
            &[tok],
            &VerifyMandateOptions {
                trusted_root_issuers: trusted,
                ledger: "https://ledger.example".to_string(),
                now: 2000,
                skew_seconds: 60,
                request: None,
                step_up_approved: false,
            },
        )
        .unwrap();

        let tracker = MandateSpendTracker::new();
        let r1 = tracker
            .reserve(&verified, "https://api.example/read", &Money::new("USD", "0.60"), Effect::Read, 2001)
            .unwrap();
        assert!(tracker.refund_if_reversible(&r1.spend_id));
        assert_eq!(tracker.spent_on("mnd_budget"), Some(Money::new("USD", "0")));

        let w1 = tracker
            .reserve(&verified, "https://api.example/write", &Money::new("USD", "0.70"), Effect::Write, 2002)
            .unwrap();
        // Write calls MUST NOT be refunded on failure.
        assert!(!tracker.refund_if_reversible(&w1.spend_id));
        assert_eq!(tracker.spent_on("mnd_budget"), Some(Money::new("USD", "0.7")));

        let over = tracker
            .reserve(&verified, "https://api.example/more", &Money::new("USD", "0.40"), Effect::Read, 2003)
            .unwrap_err();
        assert_eq!(over.code, MandateErrorCode::BudgetExhausted);
    }

    #[test]
    fn verifies_published_agent_mandate_ts_vectors() {
        // Published RFC 8032 test seeds from convexityos/agent-mandate/vectors/keys.json
        let principal_seed: [u8; 32] = [0x01; 32];
        let agent_a_seed: [u8; 32] = [0x02; 32];
        let principal_vk = SigningKey::from_bytes(&principal_seed).verifying_key();
        let agent_a_vk = SigningKey::from_bytes(&agent_a_seed).verifying_key();

        assert_eq!(
            did_key_from_verifying_key(&principal_vk),
            "did:key:z6Mkon3Necd6NkkyfoGoHxid2znGc59LU3K7mubaRcFbLfLX"
        );
        assert_eq!(
            jwk_thumbprint_from_verifying_key(&principal_vk),
            "UDDReOZl1ipXAfp9wYsm13sDBMK5og--QWdBjzuf6o4"
        );
        assert_eq!(
            did_key_from_verifying_key(&agent_a_vk),
            "did:key:z6Mko9hTggMwjSTEaJaPUfE6tqcy2xvU6BnNq3e3o8qVBiyH"
        );
        assert_eq!(
            jwk_thumbprint_from_verifying_key(&agent_a_vk),
            "aVBtapLd11SUVKIMGJfPzOEDuN0sXcmzJQNVT-_sKEU"
        );

        // Published am/1 JWS tokens from convexityos/agent-mandate/vectors/chain.json
        let root_tok = "eyJhbGciOiJFZERTQSIsInR5cCI6ImFwcGxpY2F0aW9uL2FnZW50LW1hbmRhdGUrand0Iiwia2lkIjoiZGlkOmtleTp6Nk1rb24zTmVjZDZOa2t5Zm9Hb0h4aWQyem5HYzU5TFUzSzdtdWJhUmNGYkxmTFgifQ.eyJ2IjoiYW0vMSIsInR5cCI6Im1hbmRhdGUiLCJqdGkiOiJtbmRfMCIsImlzcyI6ImRpZDprZXk6ejZNa29uM05lY2Q2TmtreWZvR29IeGlkMnpuR2M1OUxVM0s3bXViYVJjRmJMZkxYIiwic3ViIjoiZGlkOmtleTp6Nk1rbzloVGdnTXdqU1RFYUphUFVmRTZ0cWN5Mnh2VTZCbk5xM2UzbzhxVkJpeUgiLCJwYXIiOm51bGwsImlhdCI6MTc5OTk5OTk0MCwibmJmIjoxNzk5OTk5OTQwLCJleHAiOjE4MDAwMDM2MDAsImNuZiI6eyJqa3QiOiJhVkJ0YXBMZDExU1VWS0lNR0pmUHpPRUR1TjBzWGNtekpRTlZULV9zS0VVIn0sImxlZGdlciI6Imh0dHBzOi8vbGVkZ2VyLmV4YW1wbGUuY29tIiwiY2F2Ijp7Im1heF90b3RhbCI6eyJjdXIiOiJVU0QiLCJhbXQiOiIxMDAuMDAifSwibWF4X3Blcl90eG4iOnsiY3VyIjoiVVNEIiwiYW10IjoiMS4wMCJ9LCJhdWQiOlsiKi53ZWF0aGVyY28uY29tIl0sInJlc291cmNlcyI6WyJodHRwczovL2FwaS53ZWF0aGVyY28uY29tL3YzLyoiXSwicHVycG9zZSI6WyJyZXNlYXJjaC5yZWFkb25seSJdLCJkZXB0aCI6MywiZGVsZWdhYmxlIjp0cnVlLCJjdXJyZW5jeSI6IlVTRCJ9fQ.nM7HeV-LeN6DP3sGIFHNINW8X2fVwsR0BKBzy-Zi8Chd6HkWruRdO3V27OwC-G_FvFepfBBjPMZONxt2pVbBCQ";
        let child_ok = "eyJhbGciOiJFZERTQSIsInR5cCI6ImFwcGxpY2F0aW9uL2FnZW50LW1hbmRhdGUrand0Iiwia2lkIjoiZGlkOmtleTp6Nk1rbzloVGdnTXdqU1RFYUphUFVmRTZ0cWN5Mnh2VTZCbk5xM2UzbzhxVkJpeUgifQ.eyJ2IjoiYW0vMSIsInR5cCI6Im1hbmRhdGUiLCJqdGkiOiJtbmRfMSIsImlzcyI6ImRpZDprZXk6ejZNa285aFRnZ013alNURWFKYVBVZkU2dHFjeTJ4dlU2Qm5OcTNlM284cVZCaXlIIiwic3ViIjoiZGlkOmtleTp6Nk1rdlJYTlljRTdNTWR1eW5XVGdlS2JEYVQxaWlqRFNDOHBacVhaYzhySFByZjIiLCJwYXIiOiJtbmRfMCIsImlhdCI6MTc5OTk5OTk0MCwibmJmIjoxNzk5OTk5OTQwLCJleHAiOjE4MDAwMDM2MDAsImNuZiI6eyJqa3QiOiJuUklFMlZtS2RNakwxSkQ3dGJWN2ZYVmdYeG12MEdLbk1GV1JVSk1UZjlRIn0sImxlZGdlciI6Imh0dHBzOi8vbGVkZ2VyLmV4YW1wbGUuY29tIiwiY2F2Ijp7Im1heF90b3RhbCI6eyJjdXIiOiJVU0QiLCJhbXQiOiIxMC4wMCJ9LCJtYXhfcGVyX3R4biI6eyJjdXIiOiJVU0QiLCJhbXQiOiIwLjUwIn0sImRlcHRoIjoyfX0.zTqeYjLRoUDTQSPxCjRJthFC7YlapUdffkEh6SEmGSDkjVwyPkJUef3aqr-iFuL5QtvMQq9-00g37U5FhrmvAA";
        let child_widens = "eyJhbGciOiJFZERTQSIsInR5cCI6ImFwcGxpY2F0aW9uL2FnZW50LW1hbmRhdGUrand0Iiwia2lkIjoiZGlkOmtleTp6Nk1rbzloVGdnTXdqU1RFYUphUFVmRTZ0cWN5Mnh2VTZCbk5xM2UzbzhxVkJpeUgifQ.eyJ2IjoiYW0vMSIsInR5cCI6Im1hbmRhdGUiLCJqdGkiOiJtbmRfMSIsImlzcyI6ImRpZDprZXk6ejZNa285aFRnZ013alNURWFKYVBVZkU2dHFjeTJ4dlU2Qm5OcTNlM284cVZCaXlIIiwic3ViIjoiZGlkOmtleTp6Nk1rdlJYTlljRTdNTWR1eW5XVGdlS2JEYVQxaWlqRFNDOHBacVhaYzhySFByZjIiLCJwYXIiOiJtbmRfMCIsImlhdCI6MTc5OTk5OTk0MCwibmJmIjoxNzk5OTk5OTQwLCJleHAiOjE4MDAwMDM2MDAsImNuZiI6eyJqa3QiOiJuUklFMlZtS2RNakwxSkQ3dGJWN2ZYVmdYeG12MEdLbk1GV1JVSk1UZjlRIn0sImxlZGdlciI6Imh0dHBzOi8vbGVkZ2VyLmV4YW1wbGUuY29tIiwiY2F2Ijp7Im1heF9wZXJfdHhuIjp7ImN1ciI6IlVTRCIsImFtdCI6IjUuMDAifX19.UJH8U6aDl0I211PFJi1hFrwLbwNGKAF8VF5EDOE-9O_ew4YxUe70vAiZ4HUuh11z5AIifzs1-c8YDnEFdEpWAg";

        let mut trusted = HashSet::new();
        trusted.insert("did:key:z6Mkon3Necd6NkkyfoGoHxid2znGc59LU3K7mubaRcFbLfLX".to_string());

        let verified = verify_mandate_chain(
            &[root_tok.to_string(), child_ok.to_string()],
            &VerifyMandateOptions {
                trusted_root_issuers: trusted.clone(),
                ledger: "https://ledger.example.com".to_string(),
                now: 1800000000,
                skew_seconds: DEFAULT_SKEW_SECONDS,
                request: Some(MandateActionRequest {
                    resource: "https://api.weatherco.com/v3/forecast/94110".to_string(),
                    amount: Money::new("USD", "0.02"),
                    effect: Some(Effect::Read),
                    purpose: Some("research.readonly".to_string()),
                    task_id: Some("tsk_conformance".to_string()),
                }),
                step_up_approved: false,
            },
        )
        .expect("published TS-signed two-hop chain must verify in Rust");

        assert_eq!(verified.path, vec!["mnd_0", "mnd_1"]);
        assert_eq!(verified.effective.max_total, Some(Money::new("USD", "10.00")));
        assert_eq!(verified.effective.max_per_txn, Some(Money::new("USD", "0.50")));
        assert_eq!(verified.effective.depth, Some(2));

        let err = verify_mandate_chain(
            &[root_tok.to_string(), child_widens.to_string()],
            &VerifyMandateOptions {
                trusted_root_issuers: trusted,
                ledger: "https://ledger.example.com".to_string(),
                now: 1800000000,
                skew_seconds: DEFAULT_SKEW_SECONDS,
                request: Some(MandateActionRequest {
                    resource: "https://api.weatherco.com/v3/forecast/94110".to_string(),
                    amount: Money::new("USD", "0.02"),
                    effect: Some(Effect::Read),
                    purpose: Some("research.readonly".to_string()),
                    task_id: Some("tsk_conformance".to_string()),
                }),
                step_up_approved: false,
            },
        )
        .unwrap_err();
        assert_eq!(err.code, MandateErrorCode::AttenuationViolation);
    }
}
