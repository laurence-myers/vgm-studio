//! The VGM command stream, its GD3 tag, and the header fields we model.

/// The VGM commands this app understands. Anything else is a hard error -- a
/// trimmer must not silently drop data it cannot re-encode.
pub mod command {
    /// `0x5A aa dd` -- YM3812 (OPL2), write `dd` to register `aa`.
    pub const YM3812: u8 = 0x5A;
    /// `0x5E aa dd` -- YMF262 (OPL3) port 0.
    pub const YMF262_PORT_0: u8 = 0x5E;
    /// `0x5F aa dd` -- YMF262 (OPL3) port 1.
    pub const YMF262_PORT_1: u8 = 0x5F;
    /// `0xAA aa dd` -- YM3812, second chip (dual OPL2).
    pub const YM3812_CHIP_2: u8 = 0xAA;
    /// `0x61 nn nn` -- wait `nn nn` samples, 0..=65535.
    pub const WAIT: u8 = 0x61;
    /// `0x62` -- wait 735 samples (a 60th of a second).
    pub const WAIT_60TH: u8 = 0x62;
    /// `0x63` -- wait 882 samples (a 50th of a second).
    pub const WAIT_50TH: u8 = 0x63;
    /// `0x66` -- end of sound data. Not stored in the stream.
    pub const END: u8 = 0x66;
    /// `0x70..=0x7F` -- wait `n + 1` samples, 1..=16.
    pub const SHORT_WAIT_BASE: u8 = 0x70;
    pub const SHORT_WAIT_LAST: u8 = 0x7F;

    /// Samples waited by `0x62`.
    pub const SAMPLES_60TH: u32 = 735;
    /// Samples waited by `0x63`.
    pub const SAMPLES_50TH: u32 = 882;
}

/// Appends `samples` as chunked `0x61 nn nn` waits (each up to 65535 samples),
/// returning how many commands were written.
///
/// The one place the "emit a wait, capped at 65535, as many times as it takes"
/// loop lives: the crop tail, the song splitter, and the optimiser's bulk chunks
/// all call it. A zero wait writes nothing -- the DRO->VGM converter, which must
/// keep a zero-length delay in the stream for byte-exactness, emits its own
/// single `0x61 0000` instead.
pub(crate) fn append_wait(bytes: &mut Vec<u8>, samples: u64) -> usize {
    let mut remaining = samples;
    let mut commands = 0;
    while remaining > 0 {
        let chunk = remaining.min(u64::from(u16::MAX));
        bytes.push(command::WAIT);
        bytes.extend_from_slice(&(chunk as u16).to_le_bytes());
        remaining -= chunk;
        commands += 1;
    }
    commands
}

/// A GD3 tag: eleven strings, in this order.
///
/// Stored as UTF-16LE with two-byte null terminators. Rust strings are UTF-8, so
/// they are transcoded on the way in and out.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Gd3Tag {
    pub track_name_en: String,
    pub track_name_native: String,
    pub game_name_en: String,
    pub game_name_native: String,
    pub system_name_en: String,
    pub system_name_native: String,
    pub track_author_en: String,
    pub track_author_native: String,
    pub release_date: String,
    pub creator: String,
    pub notes: String,
}

/// The eleven GD3 fields must be written in exactly this order.
pub const GD3_FIELD_COUNT: usize = 11;

impl Gd3Tag {
    /// The fields, in file order.
    #[must_use]
    pub fn fields(&self) -> [&str; GD3_FIELD_COUNT] {
        [
            &self.track_name_en,
            &self.track_name_native,
            &self.game_name_en,
            &self.game_name_native,
            &self.system_name_en,
            &self.system_name_native,
            &self.track_author_en,
            &self.track_author_native,
            &self.release_date,
            &self.creator,
            &self.notes,
        ]
    }

    /// Builds a tag from the eleven fields, in file order.
    #[must_use]
    pub fn from_fields(fields: [String; GD3_FIELD_COUNT]) -> Self {
        let [
            track_name_en,
            track_name_native,
            game_name_en,
            game_name_native,
            system_name_en,
            system_name_native,
            track_author_en,
            track_author_native,
            release_date,
            creator,
            notes,
        ] = fields;
        Self {
            track_name_en,
            track_name_native,
            game_name_en,
            game_name_native,
            system_name_en,
            system_name_native,
            track_author_en,
            track_author_native,
            release_date,
            creator,
            notes,
        }
    }
}

/// VGM header fields that DRO songs have no equivalent for.
///
/// `header` holds the file's own header bytes verbatim. Writing copies them and
/// patches only the fields that can have changed, so a read-then-write of an
/// unedited file reproduces it exactly -- including the chip clocks, the `rate`
/// field, and any v1.70 extra-header offset we do not otherwise model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VgmMeta {
    /// The command the loop restarts at, or `None` if the file does not loop.
    ///
    /// The file stores this as a *byte* offset, which trimming invalidates: delete
    /// a command before the loop and every later byte shifts. Holding an
    /// instruction index instead lets edits move it, and the writer converts back.
    ///
    /// The matching `loop # samples` field is not stored at all -- it is derived
    /// from the command stream and [`Self::loop_end`], so trimming inside the loop
    /// cannot leave it stale.
    pub loop_point: Option<usize>,
    /// Where the loop stops, as an **exclusive** instruction index, or `None` for
    /// the end of the song.
    ///
    /// VGM has no loop-end field. The header carries `loop # samples`, which the
    /// spec defines as the wait total from the loop point to the end of the file,
    /// and that is exactly what a `None` here writes. Holding an end index lets
    /// the editor express a loop that stops short of the tail, and the writer
    /// emits that region's length in the same field -- so it survives a save and
    /// a reload.
    ///
    /// Be aware that other players restart at the end-of-data command regardless
    /// of the declared length, so a `Some(end)` short of the song's end is
    /// honoured here but not elsewhere; trimming the tail is what makes it
    /// universal.
    ///
    /// Only meaningful alongside a [`Self::loop_point`], and always strictly
    /// greater than it.
    pub loop_end: Option<usize>,
    pub loop_base: u8,
    pub loop_modifier: u8,
    pub volume_modifier: u8,
    pub tag: Option<Gd3Tag>,
    pub(crate) header: Vec<u8>,
}

impl VgmMeta {
    /// A header for a song that has no loop, no tag, and default modifiers.
    #[must_use]
    pub fn new(header: Vec<u8>) -> Self {
        Self {
            loop_point: None,
            loop_end: None,
            loop_base: 0,
            loop_modifier: 0,
            volume_modifier: 0,
            tag: None,
            header,
        }
    }

    /// The file's header bytes, from the magic up to the start of the command stream.
    #[must_use]
    pub fn header(&self) -> &[u8] {
        &self.header
    }

    /// Replaces the header bytes wholesale.
    ///
    /// For the header audit, which corrects fields this type does not model
    /// and hands back the corrected bytes. Nothing else should need it: the
    /// writer patches the fields it owns and leaves the rest alone, which is
    /// what keeps an unedited round trip byte-exact.
    pub fn set_header(&mut self, header: Vec<u8>) {
        self.header = header;
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gd3_fields_round_trip_through_from_fields() {
        let fields: [String; GD3_FIELD_COUNT] = core::array::from_fn(|i| format!("field {i}"));
        let tag = Gd3Tag::from_fields(fields.clone());
        let borrowed: Vec<&str> = fields.iter().map(String::as_str).collect();
        assert_eq!(tag.fields().to_vec(), borrowed);
        assert_eq!(tag.track_name_en, "field 0");
        assert_eq!(tag.notes, "field 10");
    }
}
