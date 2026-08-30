#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(req) = faciolock_ipc::Request::decode(data) else {
        return;
    };

    let _ = req.username();

    let mut out = [0u8; faciolock_ipc::REQUEST_MAX];
    let n = req.encode(&mut out);
    let round = faciolock_ipc::Request::decode(&out[..n]).expect("re-encoded must decode");
    assert_eq!(round.op, req.op);
    assert_eq!(round.username(), req.username());
});
