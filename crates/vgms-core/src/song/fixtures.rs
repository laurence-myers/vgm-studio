//! Test data shared across the crate's unit tests.

use crate::song::dro_data::{DroDataV1, DroDataV2};
use crate::song::{DroSong, OplType};

/// `(0xB1 + 0xC100) * 2` -- the v2 fixture's two short delays plus two long ones.
pub(crate) const SONG_LENGTH: u32 = (0xB1 + 0xC100) * 2;

/// Decodes to 14 instructions: five register writes, a short delay of 177 ms, a
/// long delay of 49408 ms, then the same again.
pub(crate) fn dro_data_v2() -> DroDataV2 {
    let mut data: Vec<u8> = (0..10).collect();
    data.extend_from_slice(&[0xFE, 0xB0, 0xFF, 0xC0]);
    data.extend_from_within(..);
    DroDataV2::new(
        data,
        vec![0x10, 0x20, 0x30, 0x40, 0x50, 0x60, 0x70, 0x80, 0x90, 0xA0],
        0xFE,
        0xFF,
    )
    .expect("fixture is well-formed")
}

pub(crate) fn dro_song_v2() -> DroSong {
    DroSong::dro_v2(
        "test.dro".to_owned(),
        dro_data_v2(),
        SONG_LENGTH,
        OplType::Opl3,
    )
}

/// A v1 stream exercising every opcode: a plain register write, a short delay, a
/// long delay, both bank switches, the escape opcode, and one more register.
pub(crate) fn dro_data_v1() -> DroDataV1 {
    DroDataV1::new(vec![
        0x20, 0x01, // 0: register 0x20 = 0x01
        0x00, 0xB0, // 1: short delay, 0xB0 + 1 = 177 ms
        0x01, 0x34, 0x12, // 2: long delay, 0x1234 + 1 = 4661 ms
        0x02, // 3: bank switch, low
        0x03, // 4: bank switch, high
        0x04, 0x01, 0xFF, // 5: escaped register 0x01 = 0xFF
        0xBD, 0x20, // 6: register 0xBD = 0x20
    ])
    .expect("fixture is well-formed")
}

/// The delays in [`dro_data_v1`]: 177 ms + 4661 ms.
pub(crate) const V1_SONG_LENGTH: u32 = 177 + 0x1234 + 1;

pub(crate) fn dro_song_v1() -> DroSong {
    DroSong::dro_v1(
        "test_v1.dro".to_owned(),
        dro_data_v1(),
        V1_SONG_LENGTH,
        OplType::Opl2,
    )
}
