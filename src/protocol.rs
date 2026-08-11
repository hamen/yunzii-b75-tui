//! Yunzii B75 Pro Max screen-control wire protocol.
//!
//! Implements exactly what `fields.json` (Phase 0, PR #1) documents. Every
//! report is a 64-byte HID buffer:
//!
//! ```text
//! offset 0    : opcode (0x40 info-package / 0x41 data-packet / 0x42 finish)
//! offset 1-2  : data offset, u16 little-endian (0x00 0x00 for every command
//!               except picture-upload's bulk packets)
//! offset 3    : length (of the payload that follows)
//! offset 4-5  : checksum, u16 little-endian -- for ALL opcodes
//! offset 6    : status (0x00 outbound / 0x55 device ACK)
//! offset 7-63 : payload (length bytes), then 0x00 padding
//! ```
//!
//! Checksum is a plain byte sum, NOT a CRC:
//! `opcode + byte1 + byte2 + length + 0 + 0 + 0 + sum(payload)`, truncated to
//! 16 bits and stored little-endian. Bytes 4-6 count as zero while summing:
//! the vendor builds the array with 0x00 placeholders there, sums it, then
//! writes the two checksum bytes back into offsets 4 and 5.
//!
//! # Correction, Milestone 3
//!
//! Offsets 1-2 were previously documented as "reserved" and offset 5 as a
//! reserved zero with an 8-bit checksum at offset 4. Byte 4 was in fact
//! always right -- it is the low byte under either model -- but byte 5 is the
//! HIGH byte, not a reserved zero, and bytes 1-2 carry the bulk data offset.
//! Every command decoded before picture upload had a total sum below 256 and
//! a zero offset, so the old model emitted identical bytes and looked
//! correct. In the 552-report picture-upload capture, byte 5 is non-zero in
//! 551 of them. The existing `set-time` / `switch-page` / `clear-picture`
//! fixture tests still pass byte-for-byte after this change -- that is the
//! regression guard for the refactor.

pub const OPCODE_INFO_PACKAGE: u8 = 0x40;
pub const OPCODE_DATA_PACKET: u8 = 0x41;
pub const OPCODE_FINISH: u8 = 0x42;

/// The cmd-9 (set clock) info-package payload -- a vendor-precomputed
/// constant, including its own inner CRC-16 (unrelated to the outer
/// checksum below). See fields.json's `commands.cmd9_setClock`.
pub const CMD9_INFO_PAYLOAD: [u8; 7] = [165, 90, 9, 0, 3, 195, 225];

/// The cmd-10 (set date) info-package payload, same structure.
pub const CMD10_INFO_PAYLOAD: [u8; 7] = [165, 90, 10, 0, 4, 1, 80];

/// The cmd-11 (switch to homepage) info-package payload -- constant, no
/// data-packet follows. See fields.json's `commands.cmd11_switchToHomepage`.
pub const CMD11_INFO_PAYLOAD: [u8; 7] = [165, 90, 11, 0, 0, 2, 0];

/// The cmd-13 (switch to picture page) info-package payload.
pub const CMD13_INFO_PAYLOAD: [u8; 7] = [165, 90, 13, 0, 0, 3, 224];

/// The cmd-14 (clear the picture) info-package payload. The full
/// clear-picture command repeats the info+finish pair 16 times -- see
/// `build_clear_picture_sequence()`.
pub const CMD14_INFO_PAYLOAD: [u8; 7] = [165, 90, 14, 0, 0, 3, 16];

/// The cmd-15 (switch to GIF page) info-package payload.
pub const CMD15_INFO_PAYLOAD: [u8; 7] = [165, 90, 15, 0, 0, 195, 65];

/// The TFT page a `switch-page` command switches to. A typed enum rather
/// than a raw `u8` command byte, so an invalid page value is unrepresentable
/// instead of a runtime check.
///
/// `Gif`'s wire bytes (`CMD15_INFO_PAYLOAD`) are resolved and correct, and
/// `main.rs`'s CLI still does not expose a way to construct `Page::Gif` --
/// but the REASON changed in Milestone 3, so the old one is not repeated
/// here.
///
/// Milestone 2 deferred it believing cmd15 did not visibly switch the panel.
/// It does. What was actually wrong was how the GIF had been stored: the
/// vendor's GIF save has three modes, and every earlier test used mode 0
/// ("set as boot animation"), which puts the frames somewhere that never
/// plays. Saving with mode 1 ("save to the device") makes the GIF appear,
/// and cmd15 then switches to it. See `fields.json`'s `unresolved[0]`.
///
/// So this stays unexposed only because GIF *upload* is not implemented
/// yet: switching to an empty page is still not a useful command. That is a
/// milestone, not a mystery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Page {
    Home,
    Picture,
    #[allow(dead_code)]
    Gif,
}

impl Page {
    fn info_payload(self) -> &'static [u8; 7] {
        match self {
            Page::Home => &CMD11_INFO_PAYLOAD,
            Page::Picture => &CMD13_INFO_PAYLOAD,
            Page::Gif => &CMD15_INFO_PAYLOAD,
        }
    }
}

/// The `finish` report's length byte is a fixed constant, NOT derived from
/// any real payload -- `finishScreenControlDataPacket()` is called with no
/// arguments in the vendor's own code. A generic "length = payload.len()"
/// builder must never be reused for this report (Phase 0's own JS checks
/// tripped on exactly this trap once already).
const FINISH_LENGTH: u8 = 0x38;

/// The one checksum used by every opcode. `b1`/`b2` are report bytes 1-2:
/// zero for every command except picture-upload's bulk packets, where they
/// carry the little-endian data offset -- and part of the sum either way.
fn checksum16_le(opcode: u8, b1: u8, b2: u8, length: u8, payload: &[u8]) -> [u8; 2] {
    let sum: u32 = opcode as u32
        + b1 as u32
        + b2 as u32
        + length as u32
        + payload.iter().map(|&b| b as u32).sum::<u32>();
    [(sum & 0xff) as u8, ((sum >> 8) & 0xff) as u8]
}

/// Builds a 64-byte report body (NOT including any Linux hidraw report-ID
/// framing byte -- that's a transport concern, handled by the caller/
/// `device` module, not the wire-format layer).
fn build_report_at(opcode: u8, b1: u8, b2: u8, length: u8, payload: &[u8]) -> [u8; 64] {
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
    bytes[1] = b1;
    bytes[2] = b2;
    bytes[3] = length;
    let cs = checksum16_le(opcode, b1, b2, length, payload);
    bytes[4] = cs[0];
    bytes[5] = cs[1];
    // bytes[6] stays 0x00 (outbound status); ACKs are a device response, not built here.
    bytes[7..7 + payload.len()].copy_from_slice(payload);
    bytes
}

/// The common case: a report with no data offset (bytes 1-2 zero).
fn build_report(opcode: u8, length: u8, payload: &[u8]) -> [u8; 64] {
    build_report_at(opcode, 0, 0, length, payload)
}

/// Builds the `0x40` info-package report for a given constant payload.
///
/// Takes a slice, not `&[u8; 7]`: every command through Milestone 2 had a
/// 7-byte info payload, but `CMD16_START_PAYLOAD` (picture upload) has 8.
/// A slice keeps one builder instead of adding a parallel 8-byte variant.
pub fn build_info_package(payload: &[u8]) -> [u8; 64] {
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

/// The full "switch to page" sequence: `infoPackage(0x40) -> finish(0x42)`,
/// 2 reports total. No data-packet, no repeat loop -- unlike `set-time`.
pub fn build_page_switch_sequence(page: Page) -> Vec<[u8; 64]> {
    vec![build_info_package(page.info_payload()), build_finish()]
}

/// The vendor's own `for (a=0; a<16; a++)` loop count for "clear the
/// picture" (`scripts/vendor-source-excerpt.js`) -- named so the loop size
/// lives in one place instead of the bare `16`/`32` scattered across the
/// builder, its capacity hint, and its tests.
const CLEAR_PICTURE_REPEAT_COUNT: usize = 16;

/// The full "clear the picture" sequence: the `infoPackage(0x40) ->
/// finish(0x42)` pair, repeated `CLEAR_PICTURE_REPEAT_COUNT` times (32
/// reports total) -- matching the vendor's own loop exactly (Phase
/// 0/Milestone 2, `scripts/vendor-source-excerpt.js`). Every repeat is
/// byte-identical: the command carries no per-iteration state.
pub fn build_clear_picture_sequence() -> Vec<[u8; 64]> {
    let mut reports = Vec::with_capacity(CLEAR_PICTURE_REPEAT_COUNT * 2);
    for _ in 0..CLEAR_PICTURE_REPEAT_COUNT {
        reports.push(build_info_package(&CMD14_INFO_PAYLOAD));
        reports.push(build_finish());
    }
    reports
}

// --- Milestone 3: picture upload ---

/// The TFT panel's fixed pixel size. Confirmed two ways: the pixel stream is
/// always 30720 bytes (= 160 * 96 * 2), and the vendor bundle maps this
/// product ID to `{width: 160, height: 96}` in its per-product screen table.
pub const PANEL_W: u32 = 160;
pub const PANEL_H: u32 = 96;

/// Bytes of RGB565 pixel data for one full frame.
pub const PICTURE_BYTES: usize = (PANEL_W as usize) * (PANEL_H as usize) * 2;

/// Payload bytes per bulk data packet: the report size minus the 7-byte
/// header, which is the vendor's own `i = reqLen - 7`.
const BULK_CHUNK: usize = 56;

/// The cmd-16 (start a picture upload) info-package payload. EIGHT bytes,
/// unlike every command above it: `[magic, cmd=16, 0x00, len=1,
/// innerCrc16(0x10,0x00,0x01), 0x01]`.
pub const CMD16_START_PAYLOAD: [u8; 8] = [165, 90, 16, 0, 1, 197, 177, 1];

/// The cmd-12 (declare the pixel-stream size) payload. Sent as a *data
/// packet* (0x41), not an info package -- the only command whose leading
/// report is a 0x41. `120, 0` is 30720 encoded `[hi, lo]`, the opposite
/// order from the little-endian offset in bytes 1-2 of the same report.
pub const CMD12_DECLARE_SIZE_PAYLOAD: [u8; 7] = [165, 90, 12, 120, 0, 195, 147];

/// The pause the vendor takes between the start report and declare-size.
///
/// It belongs HERE, between those two reports -- not before the bulk data.
/// That is the order in the vendor's own source (`await r(re); await
/// sleep(300); await o(Ue); await o(xe); await i()`), and it is where the
/// gap shows up in the captured timestamps: 300 ms between the start ACK and
/// the declare-size write, and no gap at all before the first bulk packet.
pub const START_TO_DECLARE_DELAY_MS: u64 = 300;

/// Encodes RGBA bytes as the panel's RGB565, big-endian per pixel.
///
/// `v = (R>>3)<<11 | (G>>2)<<5 | (B>>3)`, emitted as `[v>>8, v&0xFF]`,
/// row-major, top row first, left to right. Fully transparent pixels are
/// flattened to black first, matching the vendor's own pre-pass -- it starts
/// from a transparent-black canvas, so anything with alpha 0 lands on black.
///
/// Pure function: no image decoding, no I/O, no device.
pub fn rgb565_encode(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() / 2);
    for px in rgba.chunks_exact(4) {
        let (r, g, b) = if px[3] == 0 {
            (0u8, 0u8, 0u8)
        } else {
            (px[0], px[1], px[2])
        };
        let v: u16 = ((r as u16 >> 3) << 11) | ((g as u16 >> 2) << 5) | (b as u16 >> 3);
        out.push((v >> 8) as u8);
        out.push((v & 0xff) as u8);
    }
    out
}

/// The first report of a picture upload, sent on its own so the caller can
/// sleep `START_TO_DECLARE_DELAY_MS` before sending the rest. The delay is
/// sequenced by the caller, visibly, rather than hidden inside a builder
/// that is otherwise pure.
pub fn build_picture_upload_start() -> [u8; 64] {
    build_info_package(&CMD16_START_PAYLOAD)
}

/// Everything after the delay: declare-size, then the bulk pixel packets,
/// then finish. 551 reports for a full 30720-byte frame.
///
/// The length byte of each bulk packet is the REAL remaining byte count, so
/// the final packet declares 32 rather than a zero-padded 56 (548 * 56 + 32
/// = 30720). That matters beyond the length field itself: the length byte is
/// summed into the checksum, so padding it would also corrupt byte 4-5 of
/// that packet.
pub fn build_picture_upload_body(pixels: &[u8]) -> Vec<[u8; 64]> {
    assert_eq!(
        pixels.len(),
        PICTURE_BYTES,
        "picture upload needs exactly {PICTURE_BYTES} bytes of RGB565 data ({PANEL_W}x{PANEL_H}), got {}",
        pixels.len()
    );

    let chunks = pixels.len().div_ceil(BULK_CHUNK);
    let mut reports = Vec::with_capacity(chunks + 2);
    reports.push(build_data_packet(&CMD12_DECLARE_SIZE_PAYLOAD));
    for (i, chunk) in pixels.chunks(BULK_CHUNK).enumerate() {
        let offset = i * BULK_CHUNK;
        reports.push(build_report_at(
            OPCODE_DATA_PACKET,
            (offset & 0xff) as u8,
            ((offset >> 8) & 0xff) as u8,
            chunk.len() as u8,
            chunk,
        ));
    }
    reports.push(build_finish());
    reports
}

/// Total reports in one picture upload, start included: 1 + 1 + 549 + 1.
pub fn picture_upload_report_count() -> usize {
    1 + 1 + PICTURE_BYTES.div_ceil(BULK_CHUNK) + 1
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
        load_fixture_report_from(
            include_str!("../fixtures/cap1.json"),
            "cap1.json",
            command_name,
        )
    }

    fn load_fixture_report_from(json_str: &str, file_label: &str, command_name: &str) -> [u8; 64] {
        let data: serde_json::Value = serde_json::from_str(json_str)
            .unwrap_or_else(|_| panic!("{file_label} must be valid JSON"));
        let reports = data["reports"]
            .as_array()
            .unwrap_or_else(|| panic!("{file_label} must have a reports array"));
        let report = reports
            .iter()
            .find(|r| r["command_name"] == command_name && r["direction"] == "out")
            .unwrap_or_else(|| panic!("no outbound report named {command_name:?} in {file_label}"));
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
    fn checksum_carries_into_the_high_byte_instead_of_being_truncated() {
        // opcode(0x41=65) + length(255) + payload sum(255*3=765) = 1085 = 0x043d.
        // The old model kept only 0x3d and wrote a reserved 0x00 at byte 5; the
        // real wire format keeps the 0x04. This is the truncation path the
        // fixtures' own small sums never exercise.
        let payload = [255u8, 255, 255];
        let report = build_report(OPCODE_DATA_PACKET, 255, &payload);
        assert_eq!([report[4], report[5]], [0x3d, 0x04]);
    }

    #[test]
    fn checksum16_le_byte_order_is_low_then_high() {
        // Directly re-derive from the D constant, which Phase 0 verified
        // against the vendor's own hardcoded literal checksum bytes.
        let cs = checksum16_le(
            OPCODE_INFO_PACKAGE,
            0,
            0,
            CMD9_INFO_PAYLOAD.len() as u8,
            &CMD9_INFO_PAYLOAD,
        );
        assert_eq!(cs, [0xf6, 0x02]);
    }

    #[test]
    fn data_offset_bytes_are_part_of_the_checksum() {
        // Two bulk packets with an identical payload but different offsets
        // must NOT share a checksum. Real values from the capture: offset 0
        // gives [0x99, 0x1b], offset 56 gives [0xd1, 0x1b] -- a shift of
        // exactly 56, the offset byte itself. If bytes 1-2 were left out of
        // the sum (as "reserved" implied), both would be [0x99, 0x1b].
        let payload: Vec<u8> = std::iter::repeat_n([0xf8u8, 0x00], 28).flatten().collect();
        let at0 = build_report_at(OPCODE_DATA_PACKET, 0x00, 0x00, 0x38, &payload);
        let at56 = build_report_at(OPCODE_DATA_PACKET, 0x38, 0x00, 0x38, &payload);
        assert_eq!([at0[4], at0[5]], [0x99, 0x1b]);
        assert_eq!([at56[4], at56[5]], [0xd1, 0x1b]);
        assert_ne!([at0[4], at0[5]], [at56[4], at56[5]]);
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

    // --- Milestone 2: page-switch (cmd11/13/15) ---

    fn page_switch_fixture(command_name: &str) -> [u8; 64] {
        load_fixture_report_from(
            include_str!("../fixtures/page-switch.json"),
            "page-switch.json",
            command_name,
        )
    }

    #[test]
    fn switch_page_home_matches_fixture() {
        assert_eq!(
            build_info_package(&CMD11_INFO_PAYLOAD),
            page_switch_fixture("cmd11-switch-homepage-infoPackage")
        );
    }

    #[test]
    fn switch_page_picture_matches_fixture() {
        assert_eq!(
            build_info_package(&CMD13_INFO_PAYLOAD),
            page_switch_fixture("cmd13-switch-picture-page-infoPackage")
        );
    }

    #[test]
    fn switch_page_gif_matches_fixture() {
        assert_eq!(
            build_info_package(&CMD15_INFO_PAYLOAD),
            page_switch_fixture("cmd15-switch-gif-page-infoPackage")
        );
    }

    #[test]
    fn build_page_switch_sequence_is_exactly_info_then_finish_in_order() {
        for (page, fixture_prefix) in [
            (Page::Home, "cmd11-switch-homepage"),
            (Page::Picture, "cmd13-switch-picture-page"),
            (Page::Gif, "cmd15-switch-gif-page"),
        ] {
            let seq = build_page_switch_sequence(page);
            assert_eq!(seq.len(), 2, "{fixture_prefix}: expected exactly 2 reports");
            assert_eq!(
                seq[0],
                page_switch_fixture(&format!("{fixture_prefix}-infoPackage")),
                "{fixture_prefix}: report 0 must be the info-package"
            );
            assert_eq!(
                seq[1],
                page_switch_fixture(&format!("{fixture_prefix}-finish")),
                "{fixture_prefix}: report 1 must be finish"
            );
        }
    }

    // --- Milestone 2: clear-picture (cmd14, 16x repeat) ---

    fn clear_picture_fixture(command_name: &str) -> [u8; 64] {
        load_fixture_report_from(
            include_str!("../fixtures/clear-picture.json"),
            "clear-picture.json",
            command_name,
        )
    }

    #[test]
    fn clear_picture_info_package_matches_fixture() {
        assert_eq!(
            build_info_package(&CMD14_INFO_PAYLOAD),
            clear_picture_fixture("cmd14-clear-picture-infoPackage")
        );
    }

    #[test]
    fn build_clear_picture_sequence_is_32_reports_in_exact_16x_info_finish_order() {
        let seq = build_clear_picture_sequence();
        assert_eq!(
            seq.len(),
            CLEAR_PICTURE_REPEAT_COUNT * 2,
            "expected {CLEAR_PICTURE_REPEAT_COUNT}x (info, finish) reports"
        );

        let expected_info = clear_picture_fixture("cmd14-clear-picture-infoPackage");
        let expected_finish = clear_picture_fixture("cmd14-clear-picture-finish");

        for repeat in 0..CLEAR_PICTURE_REPEAT_COUNT {
            let base = repeat * 2;
            assert_eq!(
                seq[base], expected_info,
                "repeat {repeat}: report at index {base} must be the info-package"
            );
            assert_eq!(
                seq[base + 1],
                expected_finish,
                "repeat {repeat}: report at index {} must be finish",
                base + 1
            );
        }

        // All info-package instances are byte-identical to each other --
        // the command carries no per-iteration state.
        for repeat in 1..CLEAR_PICTURE_REPEAT_COUNT {
            assert_eq!(
                seq[repeat * 2],
                seq[0],
                "repeat {repeat}'s info-package must be byte-identical to repeat 0's"
            );
        }
    }

    // --- Milestone 3: picture upload (cmd16 / cmd12 / bulk / finish) ---

    #[test]
    fn rgb565_encodes_known_colours_including_truncation_boundaries() {
        // Primaries and extremes: these pin channel order, bit widths, and
        // big-endian byte order all at once.
        let cases: [([u8; 4], [u8; 2]); 8] = [
            ([255, 0, 0, 255], [0xf8, 0x00]),     // red
            ([0, 255, 0, 255], [0x07, 0xe0]),     // green
            ([0, 0, 255, 255], [0x00, 0x1f]),     // blue
            ([255, 255, 255, 255], [0xff, 0xff]), // white
            ([0, 0, 0, 255], [0x00, 0x00]),       // black
            // Truncation: the low 3 bits of R/B and the low 2 of G are
            // dropped, so these land on the same value as the primaries.
            ([7, 3, 7, 255], [0x00, 0x00]),
            ([248, 252, 248, 255], [0xff, 0xff]),
            // Alpha 0 is flattened to black regardless of its colour.
            ([255, 128, 64, 0], [0x00, 0x00]),
        ];
        for (rgba, expected) in cases {
            assert_eq!(
                rgb565_encode(&rgba),
                expected.to_vec(),
                "rgba {rgba:?} should encode to {expected:02x?}"
            );
        }
    }

    #[test]
    fn rgb565_encode_length_is_two_bytes_per_pixel() {
        let rgba = vec![0u8; PICTURE_BYTES * 2]; // 4 bytes/px -> 2 bytes/px
        assert_eq!(rgb565_encode(&rgba).len(), PICTURE_BYTES);
    }

    /// The whole picture-upload fixture, as (command_name, bytes) in order.
    fn picture_upload_fixture() -> Vec<(String, [u8; 64])> {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/picture-upload.json"))
                .expect("picture-upload.json must be valid JSON");
        data["reports"]
            .as_array()
            .expect("reports array")
            .iter()
            .map(|r| {
                (
                    r["command_name"].as_str().unwrap().to_string(),
                    parse_fixture_hex(r["payload_hex"].as_str().unwrap()),
                )
            })
            .collect()
    }

    /// The 30720 pixel bytes the fixture actually carries, reassembled from
    /// its bulk packets -- i.e. read back out of the wire format rather than
    /// recomputed, so a test using it is comparing against the capture.
    fn fixture_pixel_stream() -> Vec<u8> {
        let mut out = Vec::with_capacity(PICTURE_BYTES);
        for (name, bytes) in picture_upload_fixture() {
            if name.starts_with("picture-bulk-") {
                let len = bytes[3] as usize;
                out.extend_from_slice(&bytes[7..7 + len]);
            }
        }
        out
    }

    #[test]
    fn picture_upload_start_matches_fixture() {
        let fixture = picture_upload_fixture();
        assert_eq!(fixture[0].0, "cmd16-picture-start-infoPackage");
        assert_eq!(build_picture_upload_start(), fixture[0].1);
    }

    #[test]
    fn picture_upload_body_matches_the_full_552_report_capture() {
        let fixture = picture_upload_fixture();
        assert_eq!(
            fixture.len(),
            picture_upload_report_count(),
            "fixture should hold all {} reports",
            picture_upload_report_count()
        );

        let body = build_picture_upload_body(&fixture_pixel_stream());
        assert_eq!(
            body.len(),
            fixture.len() - 1,
            "body is everything after start"
        );

        // Byte-for-byte, in order, against real captured hardware traffic.
        for (i, report) in body.iter().enumerate() {
            let (name, expected) = &fixture[i + 1];
            assert_eq!(
                report,
                expected,
                "report {} ({name}) differs from the capture",
                i + 1
            );
        }
    }

    #[test]
    fn picture_upload_bulk_offsets_and_lengths_are_exact() {
        let body = build_picture_upload_body(&fixture_pixel_stream());
        let bulk = &body[1..body.len() - 1];
        assert_eq!(bulk.len(), 549);

        let mut total = 0usize;
        for (i, r) in bulk.iter().enumerate() {
            let offset = r[1] as usize | ((r[2] as usize) << 8);
            assert_eq!(offset, i * BULK_CHUNK, "packet {i} has the wrong offset");
            total += r[3] as usize;
        }
        assert_eq!(
            total, PICTURE_BYTES,
            "declared lengths must sum to the frame size"
        );

        // The one that an implementation is most likely to get wrong: the
        // last packet declares its REAL length, 32, not a padded 56.
        assert_eq!(bulk[bulk.len() - 1][3], 0x20);
        assert_eq!(
            bulk[bulk.len() - 1][1] as usize | ((bulk[bulk.len() - 1][2] as usize) << 8),
            30688
        );
        assert!(
            bulk[..bulk.len() - 1].iter().all(|r| r[3] == 0x38),
            "every packet except the last carries a full 56 bytes"
        );
    }

    #[test]
    fn picture_upload_opcode_order_is_declare_then_bulk_then_finish() {
        let body = build_picture_upload_body(&fixture_pixel_stream());
        assert_eq!(build_picture_upload_start()[0], OPCODE_INFO_PACKAGE);
        assert_eq!(body[0][0], OPCODE_DATA_PACKET);
        assert_eq!(&body[0][7..14], &CMD12_DECLARE_SIZE_PAYLOAD);
        assert!(
            body[1..body.len() - 1]
                .iter()
                .all(|r| r[0] == OPCODE_DATA_PACKET)
        );
        assert_eq!(*body.last().unwrap(), build_finish());
    }

    #[test]
    #[should_panic(expected = "exactly 30720 bytes")]
    fn picture_upload_body_rejects_a_wrong_sized_frame() {
        // Guards against silently uploading a truncated or oversized frame if
        // a future caller forgets to resize first.
        build_picture_upload_body(&vec![0u8; PICTURE_BYTES * 2]);
    }
}
