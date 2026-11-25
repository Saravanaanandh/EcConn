//! Pairing token enforcement (safe: a rejected datagram never reaches
//! `SendInput`, so this test cannot move the real mouse).

use common::{input_msg_encode, InputMsg};
use server::input::{InputInjector, MonitorRect};

#[test]
fn rejects_unpaired_token_without_injecting() {
    let rect = MonitorRect {
        left: 0,
        top: 0,
        width: 100,
        height: 100,
    };
    let mut injector = InputInjector::with_token(rect, 0xAA);
    let mut payload = [0u8; 17];
    input_msg_encode(InputMsg::MouseMove, 1, 0xBB, &mut payload);
    injector.process(&payload);
    let (received, injected, rejected) = injector.take_counters();
    assert_eq!((received, injected, rejected), (0, 0, 1));
}

#[test]
fn accepts_paired_token() {
    let rect = MonitorRect {
        left: 0,
        top: 0,
        width: 100,
        height: 100,
    };
    let token = 0xAABB_CCDD_EEFF_0011;
    let mut injector = InputInjector::with_token(rect, token);
    // ReleaseAll performs no OS input, so it is safe in a test and still
    // exercises the token/sequence path.
    let mut payload = [0u8; 17];
    input_msg_encode(InputMsg::ReleaseAll, 1, token, &mut payload);
    injector.process(&payload);
    let (received, injected, rejected) = injector.take_counters();
    assert_eq!((received, injected, rejected), (1, 0, 0));
}
