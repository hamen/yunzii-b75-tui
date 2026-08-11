//! Yunzii B75 Pro Max screen-control wire protocol.
//!
//! Implements exactly what `fields.json` (Phase 0, PR #1) documents. Every
//! report is a 64-byte HID buffer:
//!
//! ```text
//! offset 0    : opcode (0x40 info-package / 0x41 data-packet / 0x42 finish)
//! offset 1-2  : 0x00 0x00 (reserved)
//! offset 3    : length (of the payload that follows)
//! offset 4    : checksum8, for 0x41/0x42
//! offset 4-5  : checksum16 little-endian, for 0x40 (no separate reserved
//!               byte at offset 5 for this opcode -- the checksum occupies
//!               both offset 4 and 5)
//! offset 5    : 0x00 (reserved), for 0x41/0x42 only
//! offset 6    : status (0x00 outbound / 0x55 device ACK)
//! offset 7-63 : payload (length bytes), then 0x00 padding
//! ```
//!
//! Checksum is a plain byte sum, NOT a CRC: `opcode + length + sum(payload)`,
//! truncated to 8 or 16 bits depending on opcode.

pub const OPCODE_INFO_PACKAGE: u8 = 0x40;
pub const OPCODE_DATA_PACKET: u8 = 0x41;
pub const OPCODE_FINISH: u8 = 0x42;

/// The cmd-9 (set clock) info-package payload -- a vendor-precomputed
/// constant, including its own inner CRC-16 (unrelated to the outer
/// checksum below). See fields.json's `commands.cmd9_setClock`.
pub const CMD9_INFO_PAYLOAD: [u8; 7] = [165, 90, 9, 0, 3, 195, 225];

/// The cmd-10 (set date) info-package payload, same structure.
pub const CMD10_INFO_PAYLOAD: [u8; 7] = [165, 90, 10, 0, 4, 1, 80];

/// The `finish` report's length byte is a fixed constant, NOT derived from
/// any real payload -- `finishScreenControlDataPacket()` is called with no
/// arguments in the vendor's own code. A generic "length = payload.len()"
/// builder must never be reused for this report (Phase 0's own JS checks
/// tripped on exactly this trap once already).
const FINISH_LENGTH: u8 = 0x38;

fn checksum8(opcode: u8, length: u8, payload: &[u8]) -> u8 {
    let sum: u32 = opcode as u32 + length as u32 + payload.iter().map(|&b| b as u32).sum::<u32>();
    (sum & 0xff) as u8
}

fn checksum16_le(opcode: u8, length: u8, payload: &[u8]) -> [u8; 2] {
    let sum: u32 = opcode as u32 + length as u32 + payload.iter().map(|&b| b as u32).sum::<u32>();
    [(sum & 0xff) as u8, ((sum >> 8) & 0xff) as u8]
}

/// Builds a 64-byte report body (NOT including any Linux hidraw report-ID
/// framing byte -- that's a transport concern, handled by the caller/
/// `device` module, not the wire-format layer).
fn build_report(opcode: u8, length: u8, payload: &[u8]) -> [u8; 64] {
    // A real assert (not debug_assert): the slice copy below panics anyway
    // on an oversized payload, so this only changes the panic into a clear
    // message instead of an opaque index-out-of-bounds -- worth paying for
    // in release too, since it's one comparison.
    assert!(
        payload.len() <= 64 - 7,
        "payload of {} bytes doesn't fit in the 57 bytes available after the 7-byte header",
        payload.len()
    );
    let mut bytes = [0u8; 64];
    bytes[0] = opcode;
    bytes[3] = length;
    if opcode == OPCODE_INFO_PACKAGE {
        let cs = checksum16_le(opcode, length, payload);
        bytes[4] = cs[0];
        bytes[5] = cs[1];
    } else {
        bytes[4] = checksum8(opcode, length, payload);
        // bytes[5] stays 0x00 (reserved)
    }
    // bytes[6] stays 0x00 (outbound status); ACKs are a device response, not built here.
    bytes[7..7 + payload.len()].copy_from_slice(payload);
    bytes
}

/// Builds the `0x40` info-package report for a given constant payload
/// (`CMD9_INFO_PAYLOAD` or `CMD10_INFO_PAYLOAD`).
pub fn build_info_package(payload: &[u8; 7]) -> [u8; 64] {
    build_report(OPCODE_INFO_PACKAGE, payload.len() as u8, payload)
}

/// Builds the `0x41` data-packet report for a given payload (3 bytes for
/// cmd9 clock `[hour, minute, second]`, 4 bytes for cmd10 date
/// `[year2digit, weekday, month, date]`).
pub fn build_data_packet(payload: &[u8]) -> [u8; 64] {
    build_report(OPCODE_DATA_PACKET, payload.len() as u8, payload)
}

/// Builds the constant `0x42` finish report. Takes no payload argument --
/// see `FINISH_LENGTH`'s doc comment for why.
pub fn build_finish() -> [u8; 64] {
    build_report(OPCODE_FINISH, FINISH_LENGTH, &[])
}

/// The clock payload for cmd9: `[hour, minute, second]`.
pub fn clock_payload(hour: u8, minute: u8, second: u8) -> [u8; 3] {
    [hour, minute, second]
}

/// The date payload for cmd10: `[year2digit, weekday, month, date]`.
/// `weekday` must already be in the vendor's convention: Monday=1..Sunday=7.
pub fn date_payload(year2digit: u8, weekday: u8, month: u8, date: u8) -> [u8; 4] {
    [year2digit, weekday, month, date]
}

/// The full "Update device time" sequence: 6 reports per repeat (cmd9
/// info+data+finish, cmd10 info+data+finish), repeated 3 times -- matching
/// the vendor's own `for (i=0; i<3; i++)` loop exactly (Phase 0,
/// `scripts/vendor-source-excerpt.js`). Returns them in send order.
pub fn build_set_time_sequence(
    hour: u8,
    minute: u8,
    second: u8,
    year2digit: u8,
    weekday: u8,
    month: u8,
    date: u8,
) -> Vec<[u8; 64]> {
    let clock = clock_payload(hour, minute, second);
    let cal = date_payload(year2digit, weekday, month, date);

    let mut reports = Vec::with_capacity(18);
    for _ in 0..3 {
        reports.push(build_info_package(&CMD9_INFO_PAYLOAD));
        reports.push(build_data_packet(&clock));
        reports.push(build_finish());
        reports.push(build_info_package(&CMD10_INFO_PAYLOAD));
        reports.push(build_data_packet(&cal));
        reports.push(build_finish());
    }
    reports
}

#[cfg(test)]
mod tests {
    use super::*;

    // Load the ACTUAL bytes committed in fixtures/cap1.json (Phase 0, PR #1)
    // at test time via include_str! + serde_json -- NOT hand-typed hex
    // literals. An earlier version of this file hand-typed these same
    // constants and got the exact same 63-vs-64-byte transcription bug that
    // Phase 0 hit twice already; this file's own length assertion caught it
    // immediately, but the fix is to stop hand-typing 64-token hex strings
    // entirely, not to try to type them more carefully a third time.
    fn parse_fixture_hex(hex: &str) -> [u8; 64] {
        let bytes: Vec<u8> = hex
            .split_whitespace()
            .map(|tok| u8::from_str_radix(tok, 16).unwrap())
            .collect();
        assert_eq!(bytes.len(), 64, "fixture hex must be exactly 64 bytes");
        bytes.try_into().unwrap()
    }

    fn load_fixture_report(command_name: &str) -> [u8; 64] {
        let json_str = include_str!("../fixtures/cap1.json");
        let data: serde_json::Value =
            serde_json::from_str(json_str).expect("fixtures/cap1.json must be valid JSON");
        let reports = data["reports"]
            .as_array()
            .expect("fixtures/cap1.json must have a reports array");
        let report = reports
            .iter()
            .find(|r| r["command_name"] == command_name && r["direction"] == "out")
            .unwrap_or_else(|| {
                panic!("no outbound report named {command_name:?} in fixtures/cap1.json")
            });
        let hex = report["payload_hex"]
            .as_str()
            .expect("payload_hex must be a string");
        parse_fixture_hex(hex)
    }

    // cap1's exact wall-clock fields, also read from the fixture rather than
    // hand-typed, so there's a single source of truth for "what cap1 was."
    fn load_cap1_decoded_payload(command_name: &str) -> serde_json::Value {
        let json_str = include_str!("../fixtures/cap1.json");
        let data: serde_json::Value = serde_json::from_str(json_str).unwrap();
        let reports = data["reports"].as_array().unwrap();
        let report = reports
            .iter()
            .find(|r| r["command_name"] == command_name && r["direction"] == "out")
            .unwrap();
        report["decoded_payload"].clone()
    }

    #[test]
    fn cmd9_info_package_matches_fixture() {
        assert_eq!(
            build_info_package(&CMD9_INFO_PAYLOAD),
            load_fixture_report("cmd9-time-infoPackage")
        );
    }

    #[test]
    fn cmd9_data_packet_matches_fixture() {
        let dp = load_cap1_decoded_payload("cmd9-time-dataPacket");
        let payload = clock_payload(
            dp["hour"].as_u64().unwrap() as u8,
            dp["minute"].as_u64().unwrap() as u8,
            dp["second"].as_u64().unwrap() as u8,
        );
        assert_eq!(
            build_data_packet(&payload),
            load_fixture_report("cmd9-time-dataPacket")
        );
    }

    #[test]
    fn finish_matches_fixture() {
        assert_eq!(build_finish(), load_fixture_report("cmd9-finish"));
        // Both groups' finish reports are byte-identical, per Phase 0.
        assert_eq!(build_finish(), load_fixture_report("cmd10-finish"));
    }

    #[test]
    fn cmd10_info_package_matches_fixture() {
        assert_eq!(
            build_info_package(&CMD10_INFO_PAYLOAD),
            load_fixture_report("cmd10-date-infoPackage")
        );
    }

    #[test]
    fn cmd10_data_packet_matches_fixture() {
        let dp = load_cap1_decoded_payload("cmd10-date-dataPacket");
        let payload = date_payload(
            dp["year2digit"].as_u64().unwrap() as u8,
            dp["weekday"].as_u64().unwrap() as u8,
            dp["month"].as_u64().unwrap() as u8,
            dp["date"].as_u64().unwrap() as u8,
        );
        assert_eq!(
            build_data_packet(&payload),
            load_fixture_report("cmd10-date-dataPacket")
        );
    }

    #[test]
    fn full_sequence_is_18_reports_matching_the_three_repeat_loop() {
        let clock = load_cap1_decoded_payload("cmd9-time-dataPacket");
        let cal = load_cap1_decoded_payload("cmd10-date-dataPacket");
        let seq = build_set_time_sequence(
            clock["hour"].as_u64().unwrap() as u8,
            clock["minute"].as_u64().unwrap() as u8,
            clock["second"].as_u64().unwrap() as u8,
            cal["year2digit"].as_u64().unwrap() as u8,
            cal["weekday"].as_u64().unwrap() as u8,
            cal["month"].as_u64().unwrap() as u8,
            cal["date"].as_u64().unwrap() as u8,
        );
        assert_eq!(seq.len(), 18);
        // Every repeat must be byte-identical to the first (all inputs are fixed).
        for repeat in 0..3 {
            let base = repeat * 6;
            assert_eq!(seq[base], load_fixture_report("cmd9-time-infoPackage"));
            assert_eq!(seq[base + 1], load_fixture_report("cmd9-time-dataPacket"));
            assert_eq!(seq[base + 2], load_fixture_report("cmd9-finish"));
            assert_eq!(seq[base + 3], load_fixture_report("cmd10-date-infoPackage"));
            assert_eq!(seq[base + 4], load_fixture_report("cmd10-date-dataPacket"));
            assert_eq!(seq[base + 5], load_fixture_report("cmd10-finish"));
        }
    }

    #[test]
    fn checksum8_wraps_at_256_not_just_correct_for_small_samples() {
        // opcode(0x41=65) + length(255) + payload sum(255*3=765) = 65+255+765 = 1085.
        // 1085 mod 256 = 61 = 0x3d. This exercises the truncation path, not just
        // small in-range values like the fixture samples happen to be.
        let payload = [255u8, 255, 255];
        let report = build_report(OPCODE_DATA_PACKET, 255, &payload);
        assert_eq!(report[4], 0x3d);
    }

    #[test]
    fn checksum16_le_byte_order_is_low_then_high() {
        // Directly re-derive from the D constant, which Phase 0 verified
        // against the vendor's own hardcoded literal checksum bytes.
        let cs = checksum16_le(
            OPCODE_INFO_PACKAGE,
            CMD9_INFO_PAYLOAD.len() as u8,
            &CMD9_INFO_PAYLOAD,
        );
        assert_eq!(cs, [0xf6, 0x02]);
    }

    #[test]
    fn weekday_never_captured_but_reachable_via_encoding_contract() {
        // date_payload() takes weekday as a caller-supplied already-encoded
        // value (Monday=1..Sunday=7) -- this test documents and locks that
        // contract at the protocol layer, independent of whatever time
        // crate/weekday-mapping the `time` module (src/time.rs) uses.
        let sunday_encoded = 7u8;
        let payload = date_payload(26, sunday_encoded, 8, 16);
        assert_eq!(payload[1], 7);
    }
}
