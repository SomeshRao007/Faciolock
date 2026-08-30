#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(resp) = faciolock_ipc::Response::decode(data) else {
        return;
    };

    assert_eq!(
        &resp.encode()[..],
        data,
        "the decoder accepted bytes the encoder would never write"
    );
    if resp.verdict.is_allow() {
        assert_eq!(resp.verdict, faciolock_ipc::Verdict::Allow, "only Allow is an allow");
    }
});
