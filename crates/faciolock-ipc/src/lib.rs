//! The wire between the PAM module and the daemon.
//!
//! wiki/04 specifies this protocol in full and this crate is that specification, typed.
//! Fixed layout, hard-capped sizes, **no allocation driven by the peer** — a username
//! lives in a `[u8; 32]` rather than a `String`, so a hostile length field cannot ask the
//! root process to reserve anything.
//!
//! `#![no_std]` is deliberate and checkable. The crate is linked into every process that
//! authenticates a user, `sudo` included, against wiki/06's 200 KB budget for the module.
//!
//! **This is the type definitions only.** Phase 3 owns the security work — `catch_unwind`
//! at the FFI boundary, the `SO_PEERCRED` gate, remote-session rejection, `cargo-fuzz` on
//! this parser, and the CI assertion on the dependency tree. What is here now exists
//! because the Phase 2 exit criterion needs the daemon to answer a socket, and writing the
//! specified messages is strictly less work than writing throwaways and replacing them.

#![no_std]

#[cfg(test)]
extern crate std;

use core::fmt;
use core::str;

/// Protocol version. A peer speaking anything else is refused rather than guessed at.
pub const VERSION: u8 = 1;

/// wiki/04's cap. `useradd` allows 32 characters, and this is a fixed array, so the cap is
/// a compile-time fact rather than a runtime check that could be forgotten.
pub const MAX_USERNAME: usize = 32;

/// `version | op | timeout_ms | username_len` before the name itself.
const HEADER: usize = 5;

/// Largest legal request. Also the receive buffer size — a datagram longer than this is
/// refused without being read into anything.
pub const REQUEST_MAX: usize = HEADER + MAX_USERNAME;

/// Responses are fixed-width. Nothing about them varies with the request.
pub const RESPONSE_LEN: usize = 4;

/// Little-endian on the wire.
///
/// Both ends are always the same machine — this is a Unix socket — so native order would
/// work. It is specified anyway: "whatever this architecture does" is not a wire format,
/// and Debian builds on big-endian s390x, where a native-order reader and a native-order
/// writer agreeing with each other would hide the fact that nobody ever decided.
fn u16_from(bytes: [u8; 2]) -> u16 {
    u16::from_le_bytes(bytes)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Op {
    Auth = 1,
    Enroll = 2,
    Status = 3,
}

impl Op {
    fn from_wire(v: u8) -> Result<Op, ProtocolError> {
        match v {
            1 => Ok(Op::Auth),
            2 => Ok(Op::Enroll),
            3 => Ok(Op::Status),
            other => Err(ProtocolError::UnknownOp(other)),
        }
    }
}

/// What the daemon tells the PAM module.
///
/// **Narrower than `faciolock_vision::Verdict` on purpose.** The pipeline distinguishes
/// `NoFace` from `NotLive` from `NoMatch`; none of that crosses the socket. Telling a
/// caller which check rejected them is telling an attacker what to fix, and the detail
/// belongs in the journal, where Phase 4 puts it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Verdict {
    Deny = 0,
    Allow = 1,
    Unavailable = 2,
    NoEnrolment = 3,
    RateLimited = 4,
}

impl Verdict {
    /// The single place this side of the socket recognises a success.
    ///
    /// Exhaustive rather than a `==`, so a variant added later cannot default to allow by
    /// being forgotten — the same discipline as `faciolock_vision::Verdict::is_allow` and
    /// as wiki/09's single-`PAM_SUCCESS`-construction-site rule.
    pub fn is_allow(&self) -> bool {
        match self {
            Verdict::Allow => true,
            Verdict::Deny
            | Verdict::Unavailable
            | Verdict::NoEnrolment
            | Verdict::RateLimited => false,
        }
    }

    fn from_wire(v: u8) -> Result<Verdict, ProtocolError> {
        match v {
            0 => Ok(Verdict::Deny),
            1 => Ok(Verdict::Allow),
            2 => Ok(Verdict::Unavailable),
            3 => Ok(Verdict::NoEnrolment),
            4 => Ok(Verdict::RateLimited),
            other => Err(ProtocolError::UnknownVerdict(other)),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ProtocolError {
    /// Peer speaks a version we do not.
    Version(u8),
    /// Datagram shorter than its own header claims.
    Truncated,
    /// Datagram longer than the message it contains. `SOCK_SEQPACKET` preserves message
    /// boundaries, so trailing bytes are not framing slack — they are someone trying
    /// something.
    Trailing,
    UnknownOp(u8),
    UnknownVerdict(u8),
    UsernameTooLong(usize),
    /// A username outside the POSIX portable filename character set.
    UsernameCharset,
}

impl fmt::Display for ProtocolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProtocolError::Version(v) => write!(f, "unsupported protocol version {v}"),
            ProtocolError::Truncated => write!(f, "message shorter than its header claims"),
            ProtocolError::Trailing => write!(f, "trailing bytes after the message"),
            ProtocolError::UnknownOp(v) => write!(f, "unknown op {v}"),
            ProtocolError::UnknownVerdict(v) => write!(f, "unknown verdict {v}"),
            ProtocolError::UsernameTooLong(n) => {
                write!(f, "username of {n} bytes exceeds {MAX_USERNAME}")
            }
            ProtocolError::UsernameCharset => write!(f, "username outside the portable charset"),
        }
    }
}

/// A request from the PAM module.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub op: Op,
    /// What the client would like. The daemon applies its own hard timeout regardless —
    /// wiki/04 — so this is a hint, not a contract.
    pub timeout_ms: u16,
    name: [u8; MAX_USERNAME],
    len: u8,
}

impl Request {
    pub fn new(op: Op, username: &str, timeout_ms: u16) -> Result<Request, ProtocolError> {
        let bytes = username.as_bytes();
        if bytes.len() > MAX_USERNAME {
            return Err(ProtocolError::UsernameTooLong(bytes.len()));
        }
        if !is_portable_username(bytes) {
            return Err(ProtocolError::UsernameCharset);
        }
        let mut name = [0u8; MAX_USERNAME];
        name[..bytes.len()].copy_from_slice(bytes);
        Ok(Request { op, timeout_ms, name, len: bytes.len() as u8 })
    }

    /// Always valid UTF-8: the charset check at construction and at decode admits only
    /// ASCII, so this cannot fail on a `Request` that exists.
    pub fn username(&self) -> &str {
        str::from_utf8(&self.name[..self.len as usize]).unwrap_or("")
    }

    /// Serialise into a caller-owned buffer. Returns the number of bytes written.
    pub fn encode(&self, out: &mut [u8; REQUEST_MAX]) -> usize {
        let n = self.len as usize;
        out[0] = VERSION;
        out[1] = self.op as u8;
        out[2..4].copy_from_slice(&self.timeout_ms.to_le_bytes());
        out[4] = self.len;
        out[HEADER..HEADER + n].copy_from_slice(&self.name[..n]);
        HEADER + n
    }

    pub fn decode(bytes: &[u8]) -> Result<Request, ProtocolError> {
        if bytes.len() < HEADER {
            return Err(ProtocolError::Truncated);
        }
        if bytes[0] != VERSION {
            return Err(ProtocolError::Version(bytes[0]));
        }
        let op = Op::from_wire(bytes[1])?;
        let timeout_ms = u16_from([bytes[2], bytes[3]]);

        // The length is read, bounds-checked against a constant, and only then used. It
        // never sizes an allocation, because there is no allocation to size.
        let len = bytes[4] as usize;
        if len > MAX_USERNAME {
            return Err(ProtocolError::UsernameTooLong(len));
        }
        match (HEADER + len).cmp(&bytes.len()) {
            core::cmp::Ordering::Greater => return Err(ProtocolError::Truncated),
            core::cmp::Ordering::Less => return Err(ProtocolError::Trailing),
            core::cmp::Ordering::Equal => {}
        }

        let raw = &bytes[HEADER..HEADER + len];
        if !is_portable_username(raw) {
            return Err(ProtocolError::UsernameCharset);
        }
        let mut name = [0u8; MAX_USERNAME];
        name[..len].copy_from_slice(raw);
        Ok(Request { op, timeout_ms, name, len: len as u8 })
    }
}

/// Shows the name rather than 32 bytes of mostly padding. A username is not a secret —
/// it is already in the journal, the process table and `PAM_USER` — but the trailing zeros
/// are noise that makes a failing assertion harder to read.
impl fmt::Debug for Request {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Request")
            .field("op", &self.op)
            .field("timeout_ms", &self.timeout_ms)
            .field("username", &self.username())
            .finish()
    }
}

/// The daemon's answer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Response {
    pub verdict: Verdict,
    pub elapsed_ms: u16,
}

impl Response {
    pub fn new(verdict: Verdict, elapsed_ms: u16) -> Response {
        Response { verdict, elapsed_ms }
    }

    pub fn encode(&self) -> [u8; RESPONSE_LEN] {
        let e = self.elapsed_ms.to_le_bytes();
        [VERSION, self.verdict as u8, e[0], e[1]]
    }

    pub fn decode(bytes: &[u8]) -> Result<Response, ProtocolError> {
        match bytes.len().cmp(&RESPONSE_LEN) {
            core::cmp::Ordering::Less => return Err(ProtocolError::Truncated),
            core::cmp::Ordering::Greater => return Err(ProtocolError::Trailing),
            core::cmp::Ordering::Equal => {}
        }
        if bytes[0] != VERSION {
            return Err(ProtocolError::Version(bytes[0]));
        }
        Ok(Response {
            verdict: Verdict::from_wire(bytes[1])?,
            elapsed_ms: u16_from([bytes[2], bytes[3]]),
        })
    }
}

/// POSIX portable filename character set, minus a leading hyphen.
///
/// Usernames reach the daemon from a root process and are used to look up a uid, so this
/// is not the last line of defence. It is the cheapest one: refusing `/`, NUL, whitespace
/// and shell metacharacters at the wire boundary means no later stage has to remember to.
fn is_portable_username(bytes: &[u8]) -> bool {
    if bytes.is_empty() || bytes[0] == b'-' {
        return false;
    }
    bytes.iter().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_survives_a_round_trip() {
        let req = Request::new(Op::Auth, "csr", 400).expect("valid");
        let mut buf = [0u8; REQUEST_MAX];
        let n = req.encode(&mut buf);
        assert_eq!(n, HEADER + 3);

        let back = Request::decode(&buf[..n]).expect("decodes");
        assert_eq!(back.op, Op::Auth);
        assert_eq!(back.timeout_ms, 400);
        assert_eq!(back.username(), "csr");
    }

    #[test]
    fn a_response_survives_a_round_trip() {
        let res = Response::new(Verdict::Allow, 412);
        assert_eq!(Response::decode(&res.encode()), Ok(res));
    }

    /// Exactly one verdict is an allow, and a variant added later must not join it by
    /// accident.
    #[test]
    fn only_allow_is_an_allow() {
        assert!(Verdict::Allow.is_allow());
        for deny in
            [Verdict::Deny, Verdict::Unavailable, Verdict::NoEnrolment, Verdict::RateLimited]
        {
            assert!(!deny.is_allow(), "{deny:?} must not be an allow");
        }
    }

    /// A length field larger than the datagram must not read past it. There is no
    /// allocation to overshoot, but there is a slice.
    #[test]
    fn a_lying_length_field_is_refused_not_trusted() {
        let mut buf = [0u8; REQUEST_MAX];
        buf[0] = VERSION;
        buf[1] = Op::Auth as u8;
        buf[4] = 31; // claims 31 bytes of username in a 6-byte datagram
        assert_eq!(Request::decode(&buf[..6]), Err(ProtocolError::Truncated));
    }

    #[test]
    fn a_length_beyond_the_cap_is_refused_before_anything_is_indexed() {
        let mut buf = [0u8; 64];
        buf[0] = VERSION;
        buf[1] = Op::Auth as u8;
        buf[4] = 200;
        assert_eq!(Request::decode(&buf), Err(ProtocolError::UsernameTooLong(200)));
    }

    /// `SOCK_SEQPACKET` preserves message boundaries, so extra bytes are not slack.
    #[test]
    fn trailing_bytes_are_an_error_not_padding() {
        let req = Request::new(Op::Auth, "csr", 400).expect("valid");
        let mut buf = [0u8; REQUEST_MAX];
        let n = req.encode(&mut buf);
        assert_eq!(Request::decode(&buf[..n + 1]), Err(ProtocolError::Trailing));
    }

    #[test]
    fn an_unknown_version_is_refused_rather_than_guessed_at() {
        let mut buf = [0u8; REQUEST_MAX];
        let n = Request::new(Op::Auth, "csr", 400).expect("valid").encode(&mut buf);
        buf[0] = VERSION.wrapping_add(1);
        assert_eq!(Request::decode(&buf[..n]), Err(ProtocolError::Version(VERSION + 1)));
    }

    #[test]
    fn unknown_op_and_verdict_codes_do_not_become_defaults() {
        let mut buf = [0u8; REQUEST_MAX];
        let n = Request::new(Op::Auth, "csr", 400).expect("valid").encode(&mut buf);
        buf[1] = 9;
        assert_eq!(Request::decode(&buf[..n]), Err(ProtocolError::UnknownOp(9)));

        let mut res = Response::new(Verdict::Deny, 0).encode();
        res[1] = 9;
        assert_eq!(Response::decode(&res), Err(ProtocolError::UnknownVerdict(9)));
    }

    #[test]
    fn usernames_outside_the_portable_charset_never_reach_a_lookup() {
        for bad in ["ro ot", "root/../x", "ro\0ot", "-rf", "", "üser", "a;b"] {
            assert!(
                Request::new(Op::Auth, bad, 400).is_err(),
                "{bad:?} should not encode"
            );
        }
        for good in ["csr", "root", "user.name", "user_1", "a-b"] {
            assert!(Request::new(Op::Auth, good, 400).is_ok(), "{good:?} should encode");
        }
    }

    /// A charset violation must be caught on the way *in* as well as on the way out — the
    /// decoder is the side an attacker controls.
    #[test]
    fn the_decoder_enforces_the_charset_too() {
        let mut buf = [0u8; REQUEST_MAX];
        buf[0] = VERSION;
        buf[1] = Op::Auth as u8;
        buf[4] = 3;
        buf[HEADER..HEADER + 3].copy_from_slice(b"a/b");
        assert_eq!(Request::decode(&buf[..HEADER + 3]), Err(ProtocolError::UsernameCharset));
    }

    #[test]
    fn a_username_at_the_cap_still_fits_the_buffer() {
        // Spelled out rather than built, because this crate is `no_std` and a test that
        // needed `alloc` would quietly weaken the property the attribute exists to hold.
        let name = "abcdefghijklmnopqrstuvwxyz012345";
        assert_eq!(name.len(), MAX_USERNAME);
        let req = Request::new(Op::Auth, name, 0).expect("32 bytes is legal");
        let mut buf = [0u8; REQUEST_MAX];
        let n = req.encode(&mut buf);
        assert_eq!(n, REQUEST_MAX);
        assert_eq!(Request::decode(&buf[..n]).expect("decodes").username(), name);
    }
}
