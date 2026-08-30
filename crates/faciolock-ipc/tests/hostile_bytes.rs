use faciolock_ipc::{Op, Request, Response, Verdict};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 16
    }

    fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    fn upto(&mut self, n: usize) -> usize {
        (self.next() as usize) % n
    }
}

#[test]
fn no_hostile_response_decodes_to_an_allow() {
    let mut rng = Rng(0x5EED_FAC1_0107_2026);
    let mut accepted = 0usize;
    let mut allows = 0usize;

    const VERDICTS: [Verdict; 6] = [
        Verdict::Deny,
        Verdict::Allow,
        Verdict::Unavailable,
        Verdict::NoEnrolment,
        Verdict::RateLimited,
        Verdict::Ack,
    ];

    for _ in 0..200_000 {
        let bytes: Vec<u8> = match rng.upto(4) {
            0 => (0..rng.upto(9)).map(|_| rng.byte()).collect(),
            _ => {
                let verdict = VERDICTS[rng.upto(VERDICTS.len())];
                let mut b = Response::new(verdict, rng.next() as u16).encode().to_vec();
                if rng.upto(4) != 0 {
                    let at = rng.upto(b.len());
                    b[at] ^= 1 << rng.upto(8);
                }
                b
            }
        };

        let Ok(resp) = Response::decode(&bytes) else {
            continue;
        };
        accepted += 1;
        assert_eq!(
            &resp.encode()[..],
            &bytes[..],
            "the decoder accepted bytes the encoder would never write: {bytes:02x?}"
        );
        if resp.verdict.is_allow() {
            allows += 1;
            assert_eq!(resp.verdict, Verdict::Allow, "only Allow is an allow: {bytes:02x?}");
        }
    }

    assert!(accepted > 1_000, "only {accepted} inputs decoded; the generator is not reaching it");
    assert!(allows > 0, "no Allow was ever produced; the strongest assertion never ran");
}

#[test]
fn no_hostile_request_panics_or_survives_a_round_trip_wrong() {
    let mut rng = Rng(0xC0FF_EE00_1234_5678);
    let mut accepted = 0usize;

    for _ in 0..200_000 {
        let len = rng.upto(48);
        let mut bytes: Vec<u8> = (0..len).map(|_| rng.byte()).collect();

        if !bytes.is_empty() && rng.upto(2) == 0 {
            let seed = Request::new(Op::Auth, "operator", 1_500).expect("valid");
            let mut buf = [0u8; faciolock_ipc::REQUEST_MAX];
            let n = seed.encode(&mut buf);
            bytes = buf[..n].to_vec();
            let at = rng.upto(n);
            bytes[at] ^= 1 << rng.upto(8);
        }

        let Ok(req) = Request::decode(&bytes) else {
            continue;
        };
        accepted += 1;

        let name = req.username();

        let mut out = [0u8; faciolock_ipc::REQUEST_MAX];
        let n = req.encode(&mut out);
        let round = Request::decode(&out[..n]).expect("a re-encoded request must decode");
        assert_eq!(round.op, req.op);
        assert_eq!(round.username(), name, "the username did not survive a round trip");
    }

    assert!(accepted > 100, "only {accepted} inputs decoded; the generator is not reaching it");
}
